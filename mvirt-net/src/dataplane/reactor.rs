//! Generic Reactor for packet processing
//!
//! The Reactor is the core event loop that handles:
//! - Packet I/O via pluggable backends (vhost-user, TUN)
//! - Lock-free routing lookups
//! - Inter-reactor communication via MPSC channels
//! - Protocol handling (ARP, DHCP, ICMP, NDP)
//!
//! Each vNIC and the TUN gateway gets its own Reactor running in a dedicated thread.

use std::collections::HashMap;
use std::os::fd::BorrowedFd;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use crossbeam_channel::{Receiver, Sender, TryRecvError, bounded};
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use tracing::{debug, info, trace, warn};

use super::backend::{ReactorBackend, RecvResult};
use super::buffer::{BufferPool, PoolBuffer, VIRTIO_HDR_SIZE};
use super::packet::GATEWAY_MAC;
use super::router::NetworkRouter;
use super::vhost::VirtioNetHdr;
use super::{
    ArpResponder, Dhcpv4Server, Dhcpv6Server, IcmpResponder, Icmpv6Responder, NdpResponder,
};

/// Channel capacity for inter-reactor packet queue
const INBOX_CAPACITY: usize = 1024;

/// Maximum packets to process per iteration (batching)
const BATCH_LIMIT: usize = 64;

/// A packet routed from another reactor
pub struct InboundPacket {
    pub buffer: PoolBuffer,
    pub virtio_hdr: VirtioNetHdr,
}

/// Sender half for routing packets to a reactor
pub type ReactorSender = Sender<InboundPacket>;

/// Receiver half for getting packets from other reactors
pub type ReactorReceiver = Receiver<InboundPacket>;

/// Create a new reactor channel pair
pub fn reactor_channel() -> (ReactorSender, ReactorReceiver) {
    bounded(INBOX_CAPACITY)
}

/// Registry mapping destinations to reactor senders
///
/// This is built once at startup and shared (read-only) across all reactors.
/// Uses ArcSwap for lock-free updates when NICs are added/removed.
pub struct ReactorRegistry {
    /// MAC address -> Reactor sender (for L2 forwarding within a network)
    mac_to_sender: ArcSwap<HashMap<[u8; 6], ReactorSender>>,
    /// NIC ID -> Reactor sender (for routing by NIC ID)
    nic_to_sender: ArcSwap<HashMap<String, ReactorSender>>,
    /// NIC ID -> MAC address (for TUN responses that need to construct Ethernet headers)
    nic_to_mac: ArcSwap<HashMap<String, [u8; 6]>>,
    /// Network ID -> TUN reactor sender (for internet-bound packets)
    network_to_tun: ArcSwap<HashMap<String, ReactorSender>>,
}

impl ReactorRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            mac_to_sender: ArcSwap::new(Arc::new(HashMap::new())),
            nic_to_sender: ArcSwap::new(Arc::new(HashMap::new())),
            nic_to_mac: ArcSwap::new(Arc::new(HashMap::new())),
            network_to_tun: ArcSwap::new(Arc::new(HashMap::new())),
        }
    }

    /// Register a vNIC reactor
    pub fn register_nic(&self, mac: [u8; 6], nic_id: String, sender: ReactorSender) {
        // Update MAC -> sender mapping
        let mut mac_map = (**self.mac_to_sender.load()).clone();
        mac_map.insert(mac, sender.clone());
        self.mac_to_sender.store(Arc::new(mac_map));

        // Update NIC ID -> sender mapping
        let mut nic_map = (**self.nic_to_sender.load()).clone();
        nic_map.insert(nic_id.clone(), sender);
        self.nic_to_sender.store(Arc::new(nic_map));

        // Update NIC ID -> MAC mapping (for TUN to construct Ethernet headers)
        let mut nic_mac_map = (**self.nic_to_mac.load()).clone();
        nic_mac_map.insert(nic_id, mac);
        self.nic_to_mac.store(Arc::new(nic_mac_map));
    }

    /// Unregister a vNIC reactor
    pub fn unregister_nic(&self, mac: [u8; 6], nic_id: &str) {
        let mut mac_map = (**self.mac_to_sender.load()).clone();
        mac_map.remove(&mac);
        self.mac_to_sender.store(Arc::new(mac_map));

        let mut nic_map = (**self.nic_to_sender.load()).clone();
        nic_map.remove(nic_id);
        self.nic_to_sender.store(Arc::new(nic_map));

        let mut nic_mac_map = (**self.nic_to_mac.load()).clone();
        nic_mac_map.remove(nic_id);
        self.nic_to_mac.store(Arc::new(nic_mac_map));
    }

    /// Register a TUN reactor for a network
    pub fn register_tun(&self, network_id: String, sender: ReactorSender) {
        let mut tun_map = (**self.network_to_tun.load()).clone();
        tun_map.insert(network_id, sender);
        self.network_to_tun.store(Arc::new(tun_map));
    }

    /// Get sender for a destination MAC
    pub fn get_by_mac(&self, mac: &[u8; 6]) -> Option<ReactorSender> {
        self.mac_to_sender.load().get(mac).cloned()
    }

    /// Get sender for a NIC ID
    pub fn get_by_nic_id(&self, nic_id: &str) -> Option<ReactorSender> {
        self.nic_to_sender.load().get(nic_id).cloned()
    }

    /// Get MAC address for a NIC ID (for TUN to construct Ethernet headers)
    pub fn get_mac_by_nic_id(&self, nic_id: &str) -> Option<[u8; 6]> {
        self.nic_to_mac.load().get(nic_id).copied()
    }

    /// Get TUN sender for a network
    pub fn get_tun(&self, network_id: &str) -> Option<ReactorSender> {
        self.network_to_tun.load().get(network_id).cloned()
    }
}

impl Default for ReactorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Layer 2 (Ethernet) configuration for vNIC reactors
///
/// When present, the reactor handles ARP, DHCP, ICMP, and NDP protocols.
/// For TUN reactors, this should be `None` (Layer 3 only).
#[derive(Clone)]
pub struct Layer2Config {
    /// NIC's MAC address
    pub mac: [u8; 6],
    /// Assigned IPv4 address (for DHCP server)
    pub ipv4_addr: std::net::Ipv4Addr,
    /// Assigned IPv6 address (for DHCPv6 server)
    pub ipv6_addr: std::net::Ipv6Addr,
    /// Whether this is a public network (affects DHCP gateway/DNS)
    pub is_public: bool,
}

/// Configuration for a reactor
#[derive(Clone)]
pub struct ReactorConfig {
    /// Reactor identifier (NIC ID for vNIC, arbitrary for TUN)
    pub id: String,
    /// Network ID this reactor belongs to
    pub network_id: String,
    /// Optional Layer 2 config - Some for vNIC, None for TUN (Layer 3 only)
    pub layer2: Option<Layer2Config>,
}

impl ReactorConfig {
    /// Create config for a vNIC reactor (with Layer 2 handling)
    pub fn vnic(
        nic_id: String,
        network_id: String,
        mac: [u8; 6],
        ipv4_addr: std::net::Ipv4Addr,
        ipv6_addr: std::net::Ipv6Addr,
        is_public: bool,
    ) -> Self {
        Self {
            id: nic_id,
            network_id,
            layer2: Some(Layer2Config {
                mac,
                ipv4_addr,
                ipv6_addr,
                is_public,
            }),
        }
    }

    /// Create config for a TUN reactor (Layer 3 only, no protocol handling)
    pub fn tun(id: String, network_id: String) -> Self {
        Self {
            id,
            network_id,
            layer2: None,
        }
    }

    /// Get MAC address (only for vNIC reactors)
    pub fn mac(&self) -> Option<[u8; 6]> {
        self.layer2.as_ref().map(|l2| l2.mac)
    }

    /// Check if this is a public network
    pub fn is_public(&self) -> bool {
        self.layer2.as_ref().is_some_and(|l2| l2.is_public)
    }
}

/// Protocol handlers for a reactor (Layer 2 only)
struct ProtocolHandlers {
    arp: ArpResponder,
    dhcpv4: Option<Dhcpv4Server>,
    dhcpv6: Option<Dhcpv6Server>,
    icmp: IcmpResponder,
    icmpv6: Icmpv6Responder,
    ndp: NdpResponder,
}

impl ProtocolHandlers {
    fn new(l2: &Layer2Config) -> Self {
        // DHCP servers are optional - only created if IP addresses are assigned
        let dhcpv4 = if !l2.ipv4_addr.is_unspecified() {
            Some(Dhcpv4Server::new(l2.ipv4_addr, l2.is_public))
        } else {
            None
        };

        let dhcpv6 = if !l2.ipv6_addr.is_unspecified() {
            Some(Dhcpv6Server::new(l2.ipv6_addr))
        } else {
            None
        };

        Self {
            arp: ArpResponder::new(l2.mac),
            dhcpv4,
            dhcpv6,
            icmp: IcmpResponder::new(),
            icmpv6: Icmpv6Responder::new(),
            ndp: NdpResponder::new(l2.mac, l2.is_public),
        }
    }
}

/// Generic Reactor for packet processing
///
/// Type parameter `B` is the I/O backend (VhostBackend or TunBackend).
/// For vNIC reactors, Layer 2 protocol handlers (ARP, DHCP, ICMP, NDP) are enabled.
/// For TUN reactors, only Layer 3 routing is performed (no protocol handling).
pub struct Reactor<B: ReactorBackend> {
    /// I/O backend for packet TX/RX
    backend: B,
    /// Inbox for packets from other reactors
    inbox: ReactorReceiver,
    /// Outbox sender (for self-reference in routing)
    #[allow(dead_code)]
    outbox: ReactorSender,
    /// Buffer pool for zero-copy packet handling
    pool: Arc<BufferPool>,
    /// Registry for inter-reactor routing
    registry: Arc<ReactorRegistry>,
    /// Network router for this reactor's network
    router: NetworkRouter,
    /// Protocol handlers (ARP, DHCP, ICMP, NDP) - None for TUN reactors
    handlers: Option<ProtocolHandlers>,
    /// Reactor configuration
    config: ReactorConfig,
    /// Shutdown signal
    shutdown: Receiver<()>,
    /// Pending inbox packet from poll_with_timeout (avoids message loss)
    pending_inbox: Option<InboundPacket>,
}

impl<B: ReactorBackend> Reactor<B> {
    /// Create a new reactor
    ///
    /// For vNIC reactors, pass a config with `layer2: Some(...)` to enable protocol handling.
    /// For TUN reactors, pass a config with `layer2: None` for Layer 3 only.
    pub fn new(
        backend: B,
        config: ReactorConfig,
        pool: Arc<BufferPool>,
        registry: Arc<ReactorRegistry>,
        router: NetworkRouter,
        shutdown: Receiver<()>,
    ) -> (Self, ReactorSender) {
        let (outbox, inbox) = reactor_channel();

        // Create protocol handlers only for Layer 2 (vNIC) reactors
        let handlers = config.layer2.as_ref().map(ProtocolHandlers::new);

        let reactor = Self {
            backend,
            inbox,
            outbox: outbox.clone(),
            pool,
            registry,
            router,
            handlers,
            config,
            shutdown,
            pending_inbox: None,
        };

        (reactor, outbox)
    }

    /// Run the reactor event loop
    pub fn run(&mut self) {
        info!(reactor_id = %self.config.id, "Reactor started");

        loop {
            // Check for shutdown
            if self.shutdown.try_recv().is_ok() {
                info!(reactor_id = %self.config.id, "Reactor shutting down");
                break;
            }

            let mut did_work = false;

            // 1. Process inbox (packets from other reactors)
            did_work |= self.process_inbox();

            // 2. Process backend RX (packets from device)
            did_work |= self.process_backend_rx();

            // 3. Flush RX queue to guest (single signal for all packets - interrupt coalescing)
            if let Err(e) = self.backend.flush_rx() {
                trace!(reactor_id = %self.config.id, error = %e, "Flush RX failed");
            }

            // 4. Process completions (e.g., TX completions for vhost)
            if let Err(e) = self.backend.process_completions() {
                warn!(reactor_id = %self.config.id, error = %e, "Completion processing failed");
            }

            // 5. If no work, poll with timeout
            if !did_work {
                self.poll_with_timeout();
            }
        }

        info!(reactor_id = %self.config.id, "Reactor stopped");
    }

    /// Process packets from inbox (other reactors)
    fn process_inbox(&mut self) -> bool {
        let mut count = 0;

        // First, process any pending packet from poll_with_timeout
        if let Some(packet) = self.pending_inbox.take() {
            count += 1;
            if let Err(e) = self.backend.send(packet.buffer, packet.virtio_hdr) {
                trace!(reactor_id = %self.config.id, error = %e, "Failed to send to backend");
            }
        }

        while count < BATCH_LIMIT {
            match self.inbox.try_recv() {
                Ok(packet) => {
                    count += 1;
                    // Packet from another reactor -> send to backend
                    if let Err(e) = self.backend.send(packet.buffer, packet.virtio_hdr) {
                        trace!(reactor_id = %self.config.id, error = %e, "Failed to send to backend");
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    info!(reactor_id = %self.config.id, "Inbox disconnected");
                    break;
                }
            }
        }

        count > 0
    }

    /// Process packets from backend (device RX)
    fn process_backend_rx(&mut self) -> bool {
        let mut count = 0;

        while count < BATCH_LIMIT {
            // Allocate buffer (may not be used for zero-copy backends)
            let Some(mut buffer) = self.pool.alloc() else {
                warn!(reactor_id = %self.config.id, "Buffer pool exhausted");
                break;
            };

            // Try to receive
            match self.backend.try_recv(&mut buffer) {
                Ok(RecvResult::Packet { len, virtio_hdr }) => {
                    // Standard path: data was copied into buffer
                    count += 1;
                    buffer.len = len;
                    self.handle_rx_packet(buffer, virtio_hdr);
                }
                Ok(RecvResult::PacketOwned {
                    buffer: owned_buffer,
                    virtio_hdr,
                }) => {
                    // Zero-copy path: use the buffer from the channel directly
                    // The pre-allocated buffer is dropped and returned to pool
                    count += 1;
                    self.handle_rx_packet(owned_buffer, virtio_hdr);
                }
                Ok(RecvResult::WouldBlock) => break,
                Ok(RecvResult::Done) => {
                    info!(reactor_id = %self.config.id, "Backend done");
                    break;
                }
                Err(e) => {
                    trace!(reactor_id = %self.config.id, error = %e, "Backend recv error");
                    break;
                }
            }
        }

        count > 0
    }

    /// Handle a received packet from the backend
    fn handle_rx_packet(&mut self, buffer: PoolBuffer, virtio_hdr: VirtioNetHdr) {
        // TUN reactor receives raw IP packets with virtio header prepended
        // vNIC reactors receive Ethernet frames
        if self.handlers.is_none() {
            // TUN reactor: [virtio_hdr (12B)][raw IP packet]
            // Pass through the virtio_hdr for checksum/GSO offload info
            self.handle_tun_rx_packet(buffer, virtio_hdr);
            return;
        }

        // vNIC reactor: Ethernet frame
        let data = buffer.data();
        if data.len() < 14 {
            return; // Too small for Ethernet
        }

        // Parse Ethernet header
        let ethertype = u16::from_be_bytes([data[12], data[13]]);

        // Protocol handlers for Layer 2 (vNIC) reactors
        match ethertype {
            0x0806 => {
                // ARP - handled and consumed
                if let Some(ref handlers) = self.handlers
                    && let Some(reply) = handlers.arp.process(data)
                {
                    self.send_reply(&reply);
                }
                return;
            }
            0x0800 => {
                // IPv4 - check ICMP/DHCP
                if self.handle_ipv4_protocols(&buffer) {
                    return;
                }
            }
            0x86DD => {
                // IPv6 - check ICMPv6/NDP/DHCPv6
                if self.handle_ipv6_protocols(&buffer) {
                    return;
                }
            }
            _ => {}
        }

        // Route the packet (with virtio_hdr for checksum offload)
        self.route_packet(buffer, virtio_hdr);
    }

    /// Handle a packet received from the TUN device (Layer 3, raw IP)
    ///
    /// TUN packets have format: [virtio_hdr (12B)][raw IP packet]
    /// We need to:
    /// 1. Strip the virtio header
    /// 2. Determine IP version and extract destination IP
    /// 3. Look up destination NIC in routing table
    /// 4. Prepend Ethernet header with destination NIC's MAC
    /// 5. Forward to the destination vNIC reactor with the virtio header preserved
    fn handle_tun_rx_packet(&mut self, mut buffer: PoolBuffer, virtio_hdr: VirtioNetHdr) {
        let data = buffer.data();

        // Need at least virtio header + minimal IP header
        if data.len() < VIRTIO_HDR_SIZE + 20 {
            debug!(
                reactor_id = %self.config.id,
                len = data.len(),
                "TUN packet too small"
            );
            return;
        }

        // Strip virtio header to get raw IP packet
        buffer.strip_virtio_hdr();
        let ip_data = buffer.data();

        // Parse IP version from first nibble
        let ip_version = ip_data[0] >> 4;

        // Look up destination NIC and MAC based on IP version
        let (nic_id, sender, dst_mac, ethertype) = match ip_version {
            4 => {
                // IPv4: destination IP at offset 16-20
                if ip_data.len() < 20 {
                    return;
                }
                let dst_ip =
                    std::net::Ipv4Addr::new(ip_data[16], ip_data[17], ip_data[18], ip_data[19]);

                // Look up destination in routing table
                let Some(entry) = self.router.lookup_ipv4(dst_ip) else {
                    debug!(
                        reactor_id = %self.config.id,
                        %dst_ip,
                        "No route for incoming TUN IPv4 packet"
                    );
                    return;
                };

                // Get sender for this NIC
                let Some(sender) = self.registry.get_by_nic_id(&entry.nic_id) else {
                    debug!(
                        reactor_id = %self.config.id,
                        %dst_ip,
                        nic_id = %entry.nic_id,
                        "Destination NIC not found in registry"
                    );
                    return;
                };

                // Get destination MAC for Ethernet header
                let Some(dst_mac) = self.registry.get_mac_by_nic_id(&entry.nic_id) else {
                    debug!(
                        reactor_id = %self.config.id,
                        nic_id = %entry.nic_id,
                        "Destination NIC MAC not found in registry"
                    );
                    return;
                };

                (entry.nic_id, sender, dst_mac, 0x0800u16)
            }
            6 => {
                // IPv6: destination IP at offset 24-40
                if ip_data.len() < 40 {
                    return;
                }
                let mut dst_bytes = [0u8; 16];
                dst_bytes.copy_from_slice(&ip_data[24..40]);
                let dst_ip = std::net::Ipv6Addr::from(dst_bytes);

                // Look up destination in routing table
                let Some(entry) = self.router.lookup_ipv6(dst_ip) else {
                    debug!(
                        reactor_id = %self.config.id,
                        %dst_ip,
                        "No route for incoming TUN IPv6 packet"
                    );
                    return;
                };

                // Get sender for this NIC
                let Some(sender) = self.registry.get_by_nic_id(&entry.nic_id) else {
                    debug!(
                        reactor_id = %self.config.id,
                        %dst_ip,
                        nic_id = %entry.nic_id,
                        "Destination NIC not found in registry"
                    );
                    return;
                };

                // Get destination MAC for Ethernet header
                let Some(dst_mac) = self.registry.get_mac_by_nic_id(&entry.nic_id) else {
                    debug!(
                        reactor_id = %self.config.id,
                        nic_id = %entry.nic_id,
                        "Destination NIC MAC not found in registry"
                    );
                    return;
                };

                (entry.nic_id, sender, dst_mac, 0x86DDu16)
            }
            _ => {
                debug!(
                    reactor_id = %self.config.id,
                    ip_version,
                    "Unknown IP version from TUN"
                );
                return;
            }
        };

        // Prepend Ethernet header
        buffer.prepend_eth_header(dst_mac, GATEWAY_MAC, ethertype);

        // Adjust virtio header offsets for the added Ethernet header (14 bytes)
        // csum_start and hdr_len are relative to the start of the packet
        let mut adjusted_hdr = virtio_hdr;
        const ETH_HDR_LEN: u16 = 14;
        if adjusted_hdr.csum_start.to_native() > 0 {
            adjusted_hdr.csum_start =
                vm_memory::Le16::from(adjusted_hdr.csum_start.to_native() + ETH_HDR_LEN);
        }
        // csum_offset is relative to csum_start, so it doesn't need adjustment
        // hdr_len adjustment for GSO if needed
        if adjusted_hdr.hdr_len.to_native() > 0 {
            adjusted_hdr.hdr_len =
                vm_memory::Le16::from(adjusted_hdr.hdr_len.to_native() + ETH_HDR_LEN);
        }

        // Forward to destination vNIC with adjusted virtio header
        trace!(
            reactor_id = %self.config.id,
            dst_nic = %nic_id,
            len = buffer.len,
            "Forwarding TUN packet to vNIC"
        );

        if sender
            .try_send(InboundPacket {
                buffer,
                virtio_hdr: adjusted_hdr,
            })
            .is_err()
        {
            warn!(
                reactor_id = %self.config.id,
                dst_nic = %nic_id,
                "vNIC inbox full, dropping packet from TUN"
            );
        }
    }

    /// Send a reply packet (copies Vec<u8> into PoolBuffer)
    fn send_reply(&mut self, reply: &[u8]) {
        if let Some(mut buf) = self.pool.alloc() {
            let write_area = buf.write_area();
            if write_area.len() >= reply.len() {
                write_area[..reply.len()].copy_from_slice(reply);
                buf.len = reply.len();
                // Local protocol responses don't need GSO/checksum offload
                let _ = self.backend.send(buf, VirtioNetHdr::default());
            }
        }
    }

    /// Handle IPv4 protocols (ICMP, DHCP)
    /// Returns true if packet was consumed
    fn handle_ipv4_protocols(&mut self, buffer: &PoolBuffer) -> bool {
        let Some(ref handlers) = self.handlers else {
            return false;
        };

        let data = buffer.data();
        if data.len() < 34 {
            return false; // Eth(14) + IP(20) minimum
        }

        let ip_header = &data[14..];
        let protocol = ip_header[9];

        match protocol {
            1 => {
                // ICMP
                if let Some(reply) = handlers.icmp.process(data) {
                    self.send_reply(&reply);
                    return true;
                }
            }
            17 => {
                // UDP - check for DHCP
                if data.len() >= 42 {
                    let dst_port = u16::from_be_bytes([data[14 + 20 + 2], data[14 + 20 + 3]]);
                    if dst_port == 67 {
                        // DHCP server port - extract client MAC from Ethernet header
                        let client_mac: [u8; 6] = data[6..12].try_into().unwrap();
                        if let Some(ref dhcpv4) = handlers.dhcpv4
                            && let Some(reply) = dhcpv4.process(data, client_mac)
                        {
                            self.send_reply(&reply);
                            return true;
                        }
                    }
                }
            }
            _ => {}
        }

        false
    }

    /// Handle IPv6 protocols (ICMPv6, NDP, DHCPv6)
    /// Returns true if packet was consumed
    fn handle_ipv6_protocols(&mut self, buffer: &PoolBuffer) -> bool {
        let Some(ref handlers) = self.handlers else {
            return false;
        };

        let data = buffer.data();
        if data.len() < 54 {
            return false; // Eth(14) + IPv6(40) minimum
        }

        let next_header = data[14 + 6];

        match next_header {
            58 => {
                // ICMPv6
                // Check if it's NDP first
                if let Some(reply) = handlers.ndp.process(data) {
                    self.send_reply(&reply);
                    return true;
                }
                // Otherwise check regular ICMPv6 echo
                if let Some(reply) = handlers.icmpv6.process(data) {
                    self.send_reply(&reply);
                    return true;
                }
            }
            17 => {
                // UDP - check for DHCPv6
                if data.len() >= 62 {
                    let dst_port = u16::from_be_bytes([data[14 + 40 + 2], data[14 + 40 + 3]]);
                    if dst_port == 547 {
                        // DHCPv6 server port - extract client MAC from Ethernet header
                        let client_mac: [u8; 6] = data[6..12].try_into().unwrap();
                        if let Some(ref dhcpv6) = handlers.dhcpv6
                            && let Some(reply) = dhcpv6.process(data, client_mac)
                        {
                            self.send_reply(&reply);
                            return true;
                        }
                    }
                }
            }
            _ => {}
        }

        false
    }

    /// Route a packet to another reactor or TUN
    ///
    /// The virtio_hdr carries checksum offload info from the guest.
    /// For packets to TUN, we pass this through so TUN can compute the checksum.
    fn route_packet(&self, buffer: PoolBuffer, virtio_hdr: VirtioNetHdr) {
        let data = buffer.data();
        if data.len() < 14 {
            return;
        }

        // Check destination MAC
        let dst_mac: [u8; 6] = data[0..6].try_into().unwrap();

        // Broadcast/multicast - drop (handled by protocol handlers)
        if dst_mac[0] & 0x01 != 0 {
            debug!(reactor_id = %self.config.id, "Dropping broadcast/multicast");
            return;
        }

        // Try L2 forwarding by MAC
        if let Some(sender) = self.registry.get_by_mac(&dst_mac) {
            if sender
                .try_send(InboundPacket { buffer, virtio_hdr })
                .is_err()
            {
                warn!(
                    reactor_id = %self.config.id,
                    dst_mac = ?dst_mac,
                    "L2 destination inbox full, dropping packet"
                );
            }
            return;
        }

        // Try L3 routing
        let ethertype = u16::from_be_bytes([data[12], data[13]]);
        match ethertype {
            0x0800 => self.route_ipv4(buffer, virtio_hdr),
            0x86DD => self.route_ipv6(buffer, virtio_hdr),
            _ => {
                debug!(reactor_id = %self.config.id, ethertype, "Unknown ethertype, dropping");
            }
        }
    }

    /// Route an IPv4 packet
    fn route_ipv4(&self, buffer: PoolBuffer, virtio_hdr: VirtioNetHdr) {
        let data = buffer.data();
        if data.len() < 34 {
            return;
        }

        // Extract destination IP
        let dst_ip =
            std::net::Ipv4Addr::new(data[14 + 16], data[14 + 17], data[14 + 18], data[14 + 19]);

        // Lookup in routing table
        if let Some(entry) = self.router.lookup_ipv4(dst_ip) {
            // Route to local NIC
            if let Some(sender) = self.registry.get_by_nic_id(&entry.nic_id)
                && sender
                    .try_send(InboundPacket { buffer, virtio_hdr })
                    .is_err()
            {
                warn!(
                    reactor_id = %self.config.id,
                    %dst_ip,
                    dst_nic = %entry.nic_id,
                    "Destination NIC inbox full, dropping IPv4 packet"
                );
            }
        } else if self.router.is_public() {
            // No local route, send to TUN for internet
            // Pass through virtio_hdr so TUN can compute checksum if needed
            if let Some(sender) = self.registry.get_tun(&self.config.network_id)
                && sender
                    .try_send(InboundPacket { buffer, virtio_hdr })
                    .is_err()
            {
                warn!(
                    reactor_id = %self.config.id,
                    %dst_ip,
                    "TUN inbox full, dropping IPv4 packet"
                );
            }
        } else {
            debug!(reactor_id = %self.config.id, %dst_ip, "No route for IPv4 (non-public network)");
        }
    }

    /// Route an IPv6 packet
    fn route_ipv6(&self, buffer: PoolBuffer, virtio_hdr: VirtioNetHdr) {
        let data = buffer.data();
        if data.len() < 54 {
            return;
        }

        // Extract destination IP
        let mut dst_bytes = [0u8; 16];
        dst_bytes.copy_from_slice(&data[14 + 24..14 + 40]);
        let dst_ip = std::net::Ipv6Addr::from(dst_bytes);

        // Lookup in routing table
        if let Some(entry) = self.router.lookup_ipv6(dst_ip) {
            // Route to local NIC
            if let Some(sender) = self.registry.get_by_nic_id(&entry.nic_id)
                && sender
                    .try_send(InboundPacket { buffer, virtio_hdr })
                    .is_err()
            {
                warn!(
                    reactor_id = %self.config.id,
                    %dst_ip,
                    dst_nic = %entry.nic_id,
                    "Destination NIC inbox full, dropping IPv6 packet"
                );
            }
        } else if self.router.is_public() {
            // No local route, send to TUN for internet
            // Pass through virtio_hdr so TUN can compute checksum if needed
            if let Some(sender) = self.registry.get_tun(&self.config.network_id)
                && sender
                    .try_send(InboundPacket { buffer, virtio_hdr })
                    .is_err()
            {
                warn!(
                    reactor_id = %self.config.id,
                    %dst_ip,
                    "TUN inbox full, dropping IPv6 packet"
                );
            }
        } else {
            debug!(reactor_id = %self.config.id, %dst_ip, "No route for IPv6 (non-public network)");
        }
    }

    /// Poll with timeout when idle
    fn poll_with_timeout(&mut self) {
        if let Some(fd) = self.backend.poll_fd() {
            let poll_fd = PollFd::new(unsafe { BorrowedFd::borrow_raw(fd) }, PollFlags::POLLIN);
            // 1ms timeout for fd-based backends (TUN)
            let _ = poll(&mut [poll_fd], PollTimeout::from(1u16));
        } else {
            // Channel-based backends (vhost): use very short timeout (10µs instead of 1ms)
            // This is critical for performance! The vhost backend receives ACKs via the rx
            // channel from VhostNetBackend, but this poll_with_timeout only waits on the
            // inbox channel. A 1ms delay here kills TCP throughput because ACKs from the
            // VM are delayed, causing the TCP window to close.
            // 10µs provides a good balance between latency and CPU usage.
            if let Ok(packet) = self.inbox.recv_timeout(Duration::from_micros(10)) {
                self.pending_inbox = Some(packet);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataplane::backend::RecvResult;
    use crate::dataplane::packet::GATEWAY_IPV4;
    use std::collections::VecDeque;
    use std::io;
    use std::os::fd::RawFd;
    use std::sync::Mutex;

    // ========================================================================
    // Mock Backend for Testing
    // ========================================================================

    /// Mock backend that allows controlled packet injection and capture
    struct MockBackend {
        /// Packets to return on try_recv (simulates RX from device)
        rx_queue: Mutex<VecDeque<Vec<u8>>>,
        /// Packets captured on send (simulates TX to device)
        tx_queue: Mutex<Vec<Vec<u8>>>,
        /// Whether the backend is "connected"
        connected: bool,
    }

    impl MockBackend {
        fn new() -> Self {
            Self {
                rx_queue: Mutex::new(VecDeque::new()),
                tx_queue: Mutex::new(Vec::new()),
                connected: true,
            }
        }

        /// Inject a packet to be returned by try_recv
        fn inject_rx(&self, packet: Vec<u8>) {
            self.rx_queue.lock().unwrap().push_back(packet);
        }

        /// Get all packets that were sent via send()
        fn get_sent_packets(&self) -> Vec<Vec<u8>> {
            self.tx_queue.lock().unwrap().clone()
        }

        /// Clear sent packets
        #[allow(dead_code)]
        fn clear_sent(&self) {
            self.tx_queue.lock().unwrap().clear();
        }
    }

    impl ReactorBackend for MockBackend {
        fn try_recv(&mut self, buf: &mut PoolBuffer) -> io::Result<RecvResult> {
            if let Some(packet) = self.rx_queue.lock().unwrap().pop_front() {
                let write_area = buf.write_area();
                let len = packet.len().min(write_area.len());
                write_area[..len].copy_from_slice(&packet[..len]);
                Ok(RecvResult::Packet {
                    len,
                    virtio_hdr: VirtioNetHdr::default(),
                })
            } else {
                Ok(RecvResult::WouldBlock)
            }
        }

        fn send(&mut self, buf: PoolBuffer, _virtio_hdr: VirtioNetHdr) -> io::Result<()> {
            self.tx_queue.lock().unwrap().push(buf.data().to_vec());
            Ok(())
        }

        fn poll_fd(&self) -> Option<RawFd> {
            None // No real fd in mock
        }

        fn is_connected(&self) -> bool {
            self.connected
        }
    }

    // ========================================================================
    // Helper Functions
    // ========================================================================

    /// Create a test reactor config for vNIC
    fn test_config(nic_id: &str, mac: [u8; 6]) -> ReactorConfig {
        ReactorConfig::vnic(
            nic_id.to_string(),
            "test-network".to_string(),
            mac,
            "10.200.0.10".parse().unwrap(),
            "fd00::10".parse().unwrap(),
            true,
        )
    }

    /// Create a minimal Ethernet frame
    fn make_eth_frame(dst: [u8; 6], src: [u8; 6], ethertype: u16, payload: &[u8]) -> Vec<u8> {
        let mut frame = Vec::with_capacity(14 + payload.len());
        frame.extend_from_slice(&dst);
        frame.extend_from_slice(&src);
        frame.extend_from_slice(&ethertype.to_be_bytes());
        frame.extend_from_slice(payload);
        frame
    }

    /// Create an ARP request packet
    fn make_arp_request(sender_mac: [u8; 6], sender_ip: [u8; 4], target_ip: [u8; 4]) -> Vec<u8> {
        let mut packet = vec![
            // Ethernet header: broadcast dst, sender src, ARP ethertype
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, // dst MAC (broadcast)
        ];
        packet.extend_from_slice(&sender_mac); // src MAC
        packet.extend_from_slice(&[0x08, 0x06]); // ARP ethertype

        // ARP payload
        packet.extend_from_slice(&[
            0x00, 0x01, // hardware type: Ethernet
            0x08, 0x00, // protocol type: IPv4
            0x06, // hardware size: 6
            0x04, // protocol size: 4
            0x00, 0x01, // opcode: request
        ]);
        packet.extend_from_slice(&sender_mac); // sender MAC
        packet.extend_from_slice(&sender_ip); // sender IP
        packet.extend_from_slice(&[0x00; 6]); // target MAC (unknown)
        packet.extend_from_slice(&target_ip); // target IP

        packet
    }

    /// Create an ICMP echo request (ping) packet using smoltcp for correct checksums
    fn make_icmp_echo_request(
        dst_mac: [u8; 6],
        src_mac: [u8; 6],
        dst_ip: [u8; 4],
        src_ip: [u8; 4],
        seq: u16,
    ) -> Vec<u8> {
        use smoltcp::wire::{
            EthernetAddress, EthernetFrame, EthernetProtocol, EthernetRepr, Icmpv4Packet,
            Icmpv4Repr, IpProtocol, Ipv4Packet, Ipv4Repr,
        };

        let src_eth = EthernetAddress::from_bytes(&src_mac);
        let dst_eth = EthernetAddress::from_bytes(&dst_mac);
        let src_ipv4 = smoltcp::wire::Ipv4Address::from_octets(src_ip);
        let dst_ipv4 = smoltcp::wire::Ipv4Address::from_octets(dst_ip);

        // ICMP payload
        let icmp_repr = Icmpv4Repr::EchoRequest {
            ident: 1,
            seq_no: seq,
            data: b"ping",
        };
        let icmp_len = icmp_repr.buffer_len();

        // IPv4 repr - smoltcp IPv4 header is always 20 bytes (no options)
        let ipv4_header_len = 20usize;
        let ipv4_repr = Ipv4Repr {
            src_addr: src_ipv4,
            dst_addr: dst_ipv4,
            hop_limit: 64,
            next_header: IpProtocol::Icmp,
            payload_len: icmp_len,
        };
        let ipv4_len = ipv4_header_len + icmp_len;

        // Ethernet repr
        let eth_repr = EthernetRepr {
            src_addr: src_eth,
            dst_addr: dst_eth,
            ethertype: EthernetProtocol::Ipv4,
        };
        let total_len = eth_repr.buffer_len() + ipv4_len;

        // Build packet
        let mut buffer = vec![0u8; total_len];

        // Write Ethernet frame
        let mut eth_frame = EthernetFrame::new_unchecked(&mut buffer);
        eth_repr.emit(&mut eth_frame);

        // Write IPv4 packet
        let mut ipv4_packet = Ipv4Packet::new_unchecked(eth_frame.payload_mut());
        ipv4_repr.emit(
            &mut ipv4_packet,
            &smoltcp::phy::ChecksumCapabilities::default(),
        );

        // Write ICMP packet
        let mut icmp_packet = Icmpv4Packet::new_unchecked(ipv4_packet.payload_mut());
        icmp_repr.emit(
            &mut icmp_packet,
            &smoltcp::phy::ChecksumCapabilities::default(),
        );

        buffer
    }

    // ========================================================================
    // Registry Tests
    // ========================================================================

    #[test]
    fn test_registry_new() {
        let registry = ReactorRegistry::new();
        assert!(registry.get_by_mac(&[0; 6]).is_none());
    }

    #[test]
    fn test_registry_register_nic() {
        let registry = ReactorRegistry::new();
        let (sender, _receiver) = reactor_channel();
        let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];

        registry.register_nic(mac, "nic-1".to_string(), sender);

        assert!(registry.get_by_mac(&mac).is_some());
        assert!(registry.get_by_nic_id("nic-1").is_some());
        assert!(registry.get_by_nic_id("nic-2").is_none());
    }

    #[test]
    fn test_registry_unregister_nic() {
        let registry = ReactorRegistry::new();
        let (sender, _receiver) = reactor_channel();
        let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];

        registry.register_nic(mac, "nic-1".to_string(), sender);
        assert!(registry.get_by_mac(&mac).is_some());

        registry.unregister_nic(mac, "nic-1");
        assert!(registry.get_by_mac(&mac).is_none());
        assert!(registry.get_by_nic_id("nic-1").is_none());
    }

    #[test]
    fn test_registry_tun() {
        let registry = ReactorRegistry::new();
        let (sender, _receiver) = reactor_channel();

        registry.register_tun("network-1".to_string(), sender);

        assert!(registry.get_tun("network-1").is_some());
        assert!(registry.get_tun("network-2").is_none());
    }

    #[test]
    fn test_reactor_channel() {
        let (tx, rx) = reactor_channel();

        // Channel should be bounded
        assert!(tx.is_empty());
        assert!(rx.is_empty());
    }

    #[test]
    fn test_registry_multiple_nics() {
        let registry = ReactorRegistry::new();

        let mac1 = [0x52, 0x54, 0x00, 0x00, 0x00, 0x01];
        let mac2 = [0x52, 0x54, 0x00, 0x00, 0x00, 0x02];
        let mac3 = [0x52, 0x54, 0x00, 0x00, 0x00, 0x03];

        let (sender1, _) = reactor_channel();
        let (sender2, _) = reactor_channel();
        let (sender3, _) = reactor_channel();

        registry.register_nic(mac1, "nic-1".to_string(), sender1);
        registry.register_nic(mac2, "nic-2".to_string(), sender2);
        registry.register_nic(mac3, "nic-3".to_string(), sender3);

        assert!(registry.get_by_mac(&mac1).is_some());
        assert!(registry.get_by_mac(&mac2).is_some());
        assert!(registry.get_by_mac(&mac3).is_some());
        assert!(registry.get_by_nic_id("nic-1").is_some());
        assert!(registry.get_by_nic_id("nic-2").is_some());
        assert!(registry.get_by_nic_id("nic-3").is_some());

        // Unregister middle one
        registry.unregister_nic(mac2, "nic-2");
        assert!(registry.get_by_mac(&mac1).is_some());
        assert!(registry.get_by_mac(&mac2).is_none());
        assert!(registry.get_by_mac(&mac3).is_some());
    }

    // ========================================================================
    // Reactor Tests with MockBackend
    // ========================================================================

    #[test]
    fn test_reactor_creation() {
        let backend = MockBackend::new();
        let pool = Arc::new(BufferPool::new().unwrap());
        let registry = Arc::new(ReactorRegistry::new());
        let router = NetworkRouter::new("test-network".to_string(), true);
        let (_shutdown_tx, shutdown_rx) = crossbeam_channel::bounded(1);

        let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        let config = test_config("nic-1", mac);

        let (reactor, sender) = Reactor::new(backend, config, pool, registry, router, shutdown_rx);

        assert!(!sender.is_full());
        assert_eq!(reactor.config.id, "nic-1");
        assert_eq!(reactor.config.network_id, "test-network");
        assert!(reactor.handlers.is_some()); // vNIC has handlers
    }

    #[test]
    fn test_reactor_arp_response() {
        let backend = MockBackend::new();
        let pool = Arc::new(BufferPool::new().unwrap());
        let registry = Arc::new(ReactorRegistry::new());
        let router = NetworkRouter::new("test-network".to_string(), true);
        let (_shutdown_tx, shutdown_rx) = crossbeam_channel::bounded(1);

        let nic_mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        let config = test_config("nic-1", nic_mac);

        let (mut reactor, _sender) =
            Reactor::new(backend, config, pool, registry, router, shutdown_rx);

        // Inject ARP request for gateway IP
        let gateway_ip: [u8; 4] = GATEWAY_IPV4.octets();
        let sender_mac = [0x52, 0x54, 0x00, 0xaa, 0xbb, 0xcc];
        let sender_ip = [10, 200, 0, 10];
        let arp_request = make_arp_request(sender_mac, sender_ip, gateway_ip);
        reactor.backend.inject_rx(arp_request);

        // Process one RX iteration
        reactor.process_backend_rx();

        // Should have sent an ARP reply
        let sent = reactor.backend.get_sent_packets();
        assert_eq!(sent.len(), 1, "Expected exactly one ARP reply");

        let reply = &sent[0];
        assert!(reply.len() >= 42, "ARP reply too short");

        // Check it's an ARP reply (ethertype 0x0806, opcode 0x0002)
        assert_eq!(&reply[12..14], &[0x08, 0x06], "Not ARP ethertype");
        assert_eq!(&reply[20..22], &[0x00, 0x02], "Not ARP reply opcode");
    }

    #[test]
    fn test_reactor_icmp_echo_response() {
        let backend = MockBackend::new();
        let pool = Arc::new(BufferPool::new().unwrap());
        let registry = Arc::new(ReactorRegistry::new());
        let router = NetworkRouter::new("test-network".to_string(), true);
        let (_shutdown_tx, shutdown_rx) = crossbeam_channel::bounded(1);

        let nic_mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        let config = test_config("nic-1", nic_mac);

        let (mut reactor, _sender) =
            Reactor::new(backend, config, pool, registry, router, shutdown_rx);

        // Inject ICMP echo request to gateway
        let gateway_ip: [u8; 4] = GATEWAY_IPV4.octets();
        let gateway_mac = [0x52, 0x54, 0x00, 0x00, 0x00, 0x01]; // gateway MAC
        let sender_mac = [0x52, 0x54, 0x00, 0xaa, 0xbb, 0xcc];
        let sender_ip = [10, 200, 0, 10];
        let icmp_request =
            make_icmp_echo_request(gateway_mac, sender_mac, gateway_ip, sender_ip, 1);
        reactor.backend.inject_rx(icmp_request);

        // Process one RX iteration
        reactor.process_backend_rx();

        // Should have sent an ICMP echo reply
        let sent = reactor.backend.get_sent_packets();
        assert_eq!(sent.len(), 1, "Expected exactly one ICMP reply");

        let reply = &sent[0];
        assert!(reply.len() >= 34, "ICMP reply too short");

        // Check it's IPv4 (ethertype 0x0800)
        assert_eq!(&reply[12..14], &[0x08, 0x00], "Not IPv4 ethertype");

        // Check ICMP type is echo reply (0x00)
        assert_eq!(reply[14 + 20], 0x00, "Not ICMP echo reply type");
    }

    #[test]
    fn test_reactor_inbox_processing() {
        let backend = MockBackend::new();
        let pool = Arc::new(BufferPool::new().unwrap());
        let registry = Arc::new(ReactorRegistry::new());
        let router = NetworkRouter::new("test-network".to_string(), true);
        let (_shutdown_tx, shutdown_rx) = crossbeam_channel::bounded(1);

        let nic_mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        let config = test_config("nic-1", nic_mac);

        let (mut reactor, sender) =
            Reactor::new(backend, config, pool.clone(), registry, router, shutdown_rx);

        // Send a packet to the reactor's inbox
        let mut buf = pool.alloc().unwrap();
        let test_frame = make_eth_frame(nic_mac, [0x52, 0x54, 0x00, 0xaa, 0xbb, 0xcc], 0x0800, &[]);
        buf.write_area()[..test_frame.len()].copy_from_slice(&test_frame);
        buf.len = test_frame.len();

        sender
            .send(InboundPacket {
                buffer: buf,
                virtio_hdr: VirtioNetHdr::default(),
            })
            .unwrap();

        // Process inbox
        let did_work = reactor.process_inbox();
        assert!(did_work, "Should have processed inbox packet");

        // Packet should have been sent to backend
        let sent = reactor.backend.get_sent_packets();
        assert_eq!(sent.len(), 1, "Expected packet to be sent to backend");
    }

    #[test]
    fn test_reactor_inter_reactor_routing() {
        // Create two reactors with different MACs
        let pool = Arc::new(BufferPool::new().unwrap());
        let registry = Arc::new(ReactorRegistry::new());
        let router = NetworkRouter::new("test-network".to_string(), true);
        let (_shutdown_tx1, shutdown_rx1) = crossbeam_channel::bounded(1);
        let (_shutdown_tx2, shutdown_rx2) = crossbeam_channel::bounded(1);

        let mac1 = [0x52, 0x54, 0x00, 0x00, 0x00, 0x01];
        let mac2 = [0x52, 0x54, 0x00, 0x00, 0x00, 0x02];

        let config1 = test_config("nic-1", mac1);
        let config2 = test_config("nic-2", mac2);

        let backend1 = MockBackend::new();
        let backend2 = MockBackend::new();

        let (mut reactor1, sender1) = Reactor::new(
            backend1,
            config1,
            pool.clone(),
            registry.clone(),
            router.clone(),
            shutdown_rx1,
        );
        let (mut reactor2, sender2) = Reactor::new(
            backend2,
            config2,
            pool.clone(),
            registry.clone(),
            router.clone(),
            shutdown_rx2,
        );

        // Register both in the registry
        registry.register_nic(mac1, "nic-1".to_string(), sender1);
        registry.register_nic(mac2, "nic-2".to_string(), sender2);

        // Inject a packet to reactor1 destined for mac2
        let frame = make_eth_frame(mac2, mac1, 0x0800, &[0x45; 20]); // minimal IPv4
        reactor1.backend.inject_rx(frame);

        // Process RX on reactor1 - should route to reactor2
        reactor1.process_backend_rx();

        // Process inbox on reactor2 - should receive the packet
        let did_work = reactor2.process_inbox();
        assert!(did_work, "Reactor2 should have received packet");

        // Packet should have been sent to reactor2's backend
        let sent = reactor2.backend.get_sent_packets();
        assert_eq!(sent.len(), 1, "Packet should be forwarded to reactor2");
    }

    #[test]
    fn test_reactor_drops_broadcast() {
        let backend = MockBackend::new();
        let pool = Arc::new(BufferPool::new().unwrap());
        let registry = Arc::new(ReactorRegistry::new());
        let router = NetworkRouter::new("test-network".to_string(), true);
        let (_shutdown_tx, shutdown_rx) = crossbeam_channel::bounded(1);

        let nic_mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        let config = test_config("nic-1", nic_mac);

        let (mut reactor, _sender) =
            Reactor::new(backend, config, pool, registry, router, shutdown_rx);

        // Inject a broadcast frame (not ARP - just random ethertype)
        let broadcast_mac = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
        let frame = make_eth_frame(broadcast_mac, nic_mac, 0x9999, &[0x00; 10]);
        reactor.backend.inject_rx(frame);

        // Process RX
        reactor.process_backend_rx();

        // Should not forward broadcast (unless it's a protocol we handle)
        // The packet with unknown ethertype should be dropped
        let sent = reactor.backend.get_sent_packets();
        assert!(
            sent.is_empty(),
            "Broadcast with unknown ethertype should be dropped"
        );
    }

    #[test]
    fn test_reactor_routes_to_tun_for_unknown_ip() {
        let backend = MockBackend::new();
        let pool = Arc::new(BufferPool::new().unwrap());
        let registry = Arc::new(ReactorRegistry::new());
        let router = NetworkRouter::new("test-network".to_string(), true);
        let (_shutdown_tx, shutdown_rx) = crossbeam_channel::bounded(1);

        let nic_mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        let config = test_config("nic-1", nic_mac);

        // Register a TUN reactor for this network
        let (tun_sender, tun_receiver) = reactor_channel();
        registry.register_tun("test-network".to_string(), tun_sender);

        let (mut reactor, _sender) =
            Reactor::new(backend, config, pool, registry, router, shutdown_rx);

        // Create an IPv4 packet to external IP (8.8.8.8)
        let gateway_mac = [0x52, 0x54, 0x00, 0x00, 0x00, 0x01];
        let src_ip = [10, 200, 0, 10];
        let dst_ip = [8, 8, 8, 8]; // External IP

        // Minimal IPv4 header
        let mut ipv4_payload = vec![
            0x45, 0x00, 0x00, 0x14, // version, IHL, DSCP, total length
            0x00, 0x00, 0x00, 0x00, // identification, flags, fragment
            0x40, 0x01, 0x00, 0x00, // TTL, protocol (ICMP), checksum
        ];
        ipv4_payload.extend_from_slice(&src_ip);
        ipv4_payload.extend_from_slice(&dst_ip);

        let frame = make_eth_frame(gateway_mac, nic_mac, 0x0800, &ipv4_payload);
        reactor.backend.inject_rx(frame);

        // Process RX
        reactor.process_backend_rx();

        // Should have been sent to TUN
        assert!(
            tun_receiver.try_recv().is_ok(),
            "Packet to external IP should be routed to TUN"
        );
    }

    #[test]
    fn test_mock_backend_recv_wouldblock() {
        let mut backend = MockBackend::new();
        let pool = Arc::new(BufferPool::new().unwrap());
        let mut buf = pool.alloc().unwrap();

        // Empty queue should return WouldBlock
        match backend.try_recv(&mut buf) {
            Ok(RecvResult::WouldBlock) => {}
            other => panic!("Expected WouldBlock, got {:?}", other.map(|_| "Packet")),
        }
    }

    #[test]
    fn test_mock_backend_send_receive() {
        let mut backend = MockBackend::new();
        let pool = Arc::new(BufferPool::new().unwrap());

        // Inject a packet
        let test_data = vec![0x01, 0x02, 0x03, 0x04];
        backend.inject_rx(test_data.clone());

        // Receive it
        let mut buf = pool.alloc().unwrap();
        match backend.try_recv(&mut buf) {
            Ok(RecvResult::Packet { len, .. }) => {
                assert_eq!(len, test_data.len());
                buf.len = len;
                assert_eq!(buf.data(), &test_data[..]);
            }
            other => panic!("Expected Packet, got {:?}", other.map(|_| "other")),
        }

        // Should be empty now
        let mut buf2 = pool.alloc().unwrap();
        match backend.try_recv(&mut buf2) {
            Ok(RecvResult::WouldBlock) => {}
            other => panic!(
                "Expected WouldBlock after drain, got {:?}",
                other.map(|_| "other")
            ),
        }
    }

    #[test]
    fn test_protocol_handlers_creation() {
        let l2 = Layer2Config {
            mac: [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
            ipv4_addr: "10.200.0.10".parse().unwrap(),
            ipv6_addr: "fd00::10".parse().unwrap(),
            is_public: true,
        };
        let handlers = ProtocolHandlers::new(&l2);

        // DHCP servers should be created because IPs are specified
        assert!(handlers.dhcpv4.is_some());
        assert!(handlers.dhcpv6.is_some());
    }

    #[test]
    fn test_protocol_handlers_no_dhcp_for_unspecified_ip() {
        let l2 = Layer2Config {
            mac: [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
            ipv4_addr: std::net::Ipv4Addr::UNSPECIFIED,
            ipv6_addr: std::net::Ipv6Addr::UNSPECIFIED,
            is_public: true,
        };
        let handlers = ProtocolHandlers::new(&l2);

        // DHCP servers should NOT be created for unspecified IPs
        assert!(handlers.dhcpv4.is_none());
        assert!(handlers.dhcpv6.is_none());
    }

    #[test]
    fn test_tun_reactor_no_handlers() {
        let backend = MockBackend::new();
        let pool = Arc::new(BufferPool::new().unwrap());
        let registry = Arc::new(ReactorRegistry::new());
        let router = NetworkRouter::new("test-network".to_string(), true);
        let (_shutdown_tx, shutdown_rx) = crossbeam_channel::bounded(1);

        // Create TUN config (no Layer 2)
        let config = ReactorConfig::tun("tun-0".to_string(), "test-network".to_string());

        let (reactor, _sender) = Reactor::new(backend, config, pool, registry, router, shutdown_rx);

        // TUN reactor should have no protocol handlers
        assert!(reactor.handlers.is_none());
        assert_eq!(reactor.config.id, "tun-0");
    }

    // ========================================================================
    // Threaded Integration Tests
    // ========================================================================

    #[test]
    fn test_reactor_thread_shutdown() {
        use std::thread;
        use std::time::Duration;

        let backend = MockBackend::new();
        let pool = Arc::new(BufferPool::new().unwrap());
        let registry = Arc::new(ReactorRegistry::new());
        let router = NetworkRouter::new("test-network".to_string(), true);
        let (shutdown_tx, shutdown_rx) = crossbeam_channel::bounded(1);

        let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        let config = test_config("nic-1", mac);

        let (mut reactor, _sender) =
            Reactor::new(backend, config, pool, registry, router, shutdown_rx);

        // Spawn reactor in a thread
        let handle = thread::spawn(move || {
            reactor.run();
        });

        // Let it run briefly
        thread::sleep(Duration::from_millis(50));

        // Signal shutdown
        shutdown_tx.send(()).unwrap();

        // Should terminate within a reasonable time
        let result = handle.join();
        assert!(result.is_ok(), "Reactor thread should terminate cleanly");
    }

    #[test]
    fn test_multi_reactor_communication() {
        use std::thread;
        use std::time::Duration;

        let pool = Arc::new(BufferPool::new().unwrap());
        let registry = Arc::new(ReactorRegistry::new());
        let router = NetworkRouter::new("test-network".to_string(), true);

        // Create two reactors
        let mac1 = [0x52, 0x54, 0x00, 0x00, 0x00, 0x01];
        let mac2 = [0x52, 0x54, 0x00, 0x00, 0x00, 0x02];

        let config1 = test_config("nic-1", mac1);
        let config2 = test_config("nic-2", mac2);

        let backend1 = MockBackend::new();
        let backend2 = MockBackend::new();

        let (shutdown_tx1, shutdown_rx1) = crossbeam_channel::bounded(1);
        let (shutdown_tx2, shutdown_rx2) = crossbeam_channel::bounded(1);

        let (mut reactor1, sender1) = Reactor::new(
            backend1,
            config1,
            pool.clone(),
            registry.clone(),
            router.clone(),
            shutdown_rx1,
        );
        let (mut reactor2, sender2) = Reactor::new(
            backend2,
            config2,
            pool.clone(),
            registry.clone(),
            router.clone(),
            shutdown_rx2,
        );

        // Register both reactors
        registry.register_nic(mac1, "nic-1".to_string(), sender1);
        registry.register_nic(mac2, "nic-2".to_string(), sender2);

        // Inject a packet to reactor1 destined for mac2
        let frame = make_eth_frame(mac2, mac1, 0x0800, &[0x45; 20]);
        reactor1.backend.inject_rx(frame);

        // Spawn reactor threads
        let handle1 = thread::spawn(move || {
            reactor1.run();
        });
        let handle2 = thread::spawn(move || {
            reactor2.run();
        });

        // Let them run briefly (enough for packet routing)
        thread::sleep(Duration::from_millis(100));

        // Shutdown both
        shutdown_tx1.send(()).unwrap();
        shutdown_tx2.send(()).unwrap();

        // Wait for termination
        assert!(handle1.join().is_ok(), "Reactor 1 should terminate");
        assert!(handle2.join().is_ok(), "Reactor 2 should terminate");
    }

    #[test]
    fn test_reactor_processes_multiple_packets() {
        let backend = MockBackend::new();
        let pool = Arc::new(BufferPool::new().unwrap());
        let registry = Arc::new(ReactorRegistry::new());
        let router = NetworkRouter::new("test-network".to_string(), true);
        let (_shutdown_tx, shutdown_rx) = crossbeam_channel::bounded(1);

        let nic_mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        let config = test_config("nic-1", nic_mac);

        let (mut reactor, _sender) =
            Reactor::new(backend, config, pool, registry, router, shutdown_rx);

        // Inject multiple ARP requests
        let gateway_ip: [u8; 4] = GATEWAY_IPV4.octets();
        let sender_mac = [0x52, 0x54, 0x00, 0xaa, 0xbb, 0xcc];

        for i in 0..5 {
            let sender_ip = [10, 200, 0, (10 + i) as u8];
            let arp_request = make_arp_request(sender_mac, sender_ip, gateway_ip);
            reactor.backend.inject_rx(arp_request);
        }

        // Process all packets
        reactor.process_backend_rx();

        // Should have 5 ARP replies
        let sent = reactor.backend.get_sent_packets();
        assert_eq!(sent.len(), 5, "Expected 5 ARP replies");
    }

    #[test]
    fn test_reactor_tun_integration() {
        // Test that packets to external IPs get routed to the TUN sender
        let pool = Arc::new(BufferPool::new().unwrap());
        let registry = Arc::new(ReactorRegistry::new());
        let router = NetworkRouter::new("public-network".to_string(), true);
        let (_shutdown_tx, shutdown_rx) = crossbeam_channel::bounded(1);

        let nic_mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        let config = ReactorConfig::vnic(
            "nic-1".to_string(),
            "public-network".to_string(),
            nic_mac,
            "10.200.0.10".parse().unwrap(),
            "fd00::10".parse().unwrap(),
            true,
        );

        // Register TUN reactor
        let (tun_sender, tun_receiver) = reactor_channel();
        registry.register_tun("public-network".to_string(), tun_sender);

        let backend = MockBackend::new();
        let (mut reactor, _sender) =
            Reactor::new(backend, config, pool, registry, router, shutdown_rx);

        // Send packets to various external IPs
        let gateway_mac = [0x52, 0x54, 0x00, 0x00, 0x00, 0x01];
        let external_ips = [[8, 8, 8, 8], [1, 1, 1, 1], [9, 9, 9, 9]];

        for dst_ip in external_ips {
            let src_ip = [10, 200, 0, 10];
            let mut ipv4_payload = vec![
                0x45, 0x00, 0x00, 0x14, 0x00, 0x00, 0x00, 0x00, 0x40, 0x01, 0x00, 0x00,
            ];
            ipv4_payload.extend_from_slice(&src_ip);
            ipv4_payload.extend_from_slice(&dst_ip);

            let frame = make_eth_frame(gateway_mac, nic_mac, 0x0800, &ipv4_payload);
            reactor.backend.inject_rx(frame);
        }

        // Process all packets
        reactor.process_backend_rx();

        // All 3 packets should have been sent to TUN
        let mut count = 0;
        while tun_receiver.try_recv().is_ok() {
            count += 1;
        }
        assert_eq!(count, 3, "All 3 external packets should be routed to TUN");
    }

    #[test]
    fn test_reactor_ipv6_routing_to_tun() {
        let pool = Arc::new(BufferPool::new().unwrap());
        let registry = Arc::new(ReactorRegistry::new());
        let router = NetworkRouter::new("public-network".to_string(), true);
        let (_shutdown_tx, shutdown_rx) = crossbeam_channel::bounded(1);

        let nic_mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        let config = ReactorConfig::vnic(
            "nic-1".to_string(),
            "public-network".to_string(),
            nic_mac,
            "10.200.0.10".parse().unwrap(),
            "fd00::10".parse().unwrap(),
            true,
        );

        // Register TUN reactor
        let (tun_sender, tun_receiver) = reactor_channel();
        registry.register_tun("public-network".to_string(), tun_sender);

        let backend = MockBackend::new();
        let (mut reactor, _sender) =
            Reactor::new(backend, config, pool, registry, router, shutdown_rx);

        // Create IPv6 packet to external address (2606:4700:4700::1111 - Cloudflare)
        let gateway_mac = [0x52, 0x54, 0x00, 0x00, 0x00, 0x01];
        let src_ipv6 = [0xfd, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x10];
        let dst_ipv6 = [
            0x26, 0x06, 0x47, 0x00, 0x47, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0x11, 0x11,
        ];

        let mut ipv6_payload = vec![
            0x60, 0x00, 0x00, 0x00, // version, traffic class, flow label
            0x00, 0x00, // payload length
            0x3a, 0x40, // next header (ICMPv6), hop limit
        ];
        ipv6_payload.extend_from_slice(&src_ipv6);
        ipv6_payload.extend_from_slice(&dst_ipv6);

        let frame = make_eth_frame(gateway_mac, nic_mac, 0x86DD, &ipv6_payload);
        reactor.backend.inject_rx(frame);

        // Process
        reactor.process_backend_rx();

        // Should be sent to TUN
        assert!(
            tun_receiver.try_recv().is_ok(),
            "IPv6 packet to external address should be routed to TUN"
        );
    }

    // ========================================================================
    // VhostBackend Integration Tests
    // ========================================================================

    /// Test end-to-end routing between two Reactors using VhostBackend-style flow
    ///
    /// This simulates the full architecture:
    /// 1. VM A sends packet → VhostNetBackend.packet_handler → Reactor A inbox
    /// 2. Reactor A processes → routes to Reactor B
    /// 3. Reactor B receives in inbox → sends to backend (VM B)
    #[test]
    fn test_two_reactor_routing_via_vhost_flow() {
        // Create shared infrastructure
        let pool = Arc::new(BufferPool::new().unwrap());
        let registry = Arc::new(ReactorRegistry::new());
        let router = NetworkRouter::new("test-network".to_string(), false);

        // NIC A config
        let mac_a = [0x52, 0x54, 0x00, 0x00, 0x00, 0xAA];
        let ip_a: std::net::Ipv4Addr = "10.0.0.10".parse().unwrap();
        let config_a = ReactorConfig::vnic(
            "nic-a".to_string(),
            "test-network".to_string(),
            mac_a,
            ip_a,
            "fd00::a".parse().unwrap(),
            false,
        );

        // NIC B config
        let mac_b = [0x52, 0x54, 0x00, 0x00, 0x00, 0xBB];
        let ip_b: std::net::Ipv4Addr = "10.0.0.20".parse().unwrap();
        let config_b = ReactorConfig::vnic(
            "nic-b".to_string(),
            "test-network".to_string(),
            mac_b,
            ip_b,
            "fd00::b".parse().unwrap(),
            false,
        );

        // Create backends
        let backend_a = MockBackend::new();
        let backend_b = MockBackend::new();

        // Create reactors
        let (_shutdown_tx_a, shutdown_rx_a) = crossbeam_channel::bounded(1);
        let (_shutdown_tx_b, shutdown_rx_b) = crossbeam_channel::bounded(1);

        let (mut reactor_a, sender_a) = Reactor::new(
            backend_a,
            config_a,
            pool.clone(),
            registry.clone(),
            router.clone(),
            shutdown_rx_a,
        );

        let (mut reactor_b, sender_b) = Reactor::new(
            backend_b,
            config_b,
            pool.clone(),
            registry.clone(),
            router.clone(),
            shutdown_rx_b,
        );

        // Register NICs with registry (MAC + NIC ID → sender)
        registry.register_nic(mac_a, "nic-a".to_string(), sender_a);
        registry.register_nic(mac_b, "nic-b".to_string(), sender_b);

        // Add routes
        use ipnet::Ipv4Net;
        router.add_ipv4_route(Ipv4Net::new(ip_a, 32).unwrap(), "nic-a".to_string(), true);
        router.add_ipv4_route(Ipv4Net::new(ip_b, 32).unwrap(), "nic-b".to_string(), true);

        // === Simulate packet flow: A sends ICMP to B ===

        // Create an ICMP echo request from A to B
        // The packet is addressed to gateway MAC (for routing) with B's IP as destination
        let gateway_mac = crate::dataplane::packet::GATEWAY_MAC;

        // Build IPv4 ICMP echo request
        let ip_payload = vec![
            0x45, 0x00, 0x00, 0x1c, // IPv4 header: ver+IHL, DSCP, total length
            0x00, 0x00, 0x00, 0x00, // identification, flags, fragment offset
            0x40, 0x01, 0x00,
            0x00, // TTL=64, protocol=ICMP, checksum (will be wrong but ok for test)
            10, 0, 0, 10, // src IP (A)
            10, 0, 0, 20, // dst IP (B)
            // ICMP echo request
            0x08, 0x00, 0x00, 0x00, // type=echo request, code, checksum
            0x12, 0x34, 0x00, 0x01, // identifier, sequence
        ];

        let frame_a_to_b = make_eth_frame(gateway_mac, mac_a, 0x0800, &ip_payload);

        // Inject packet to Reactor A's backend (simulating guest TX via VhostBackend flow)
        reactor_a.backend.inject_rx(frame_a_to_b);

        // Reactor A processes the packet from its backend
        // This should: parse, skip protocol handlers (not for gateway), route to B
        let did_work_a = reactor_a.process_backend_rx();
        assert!(did_work_a, "Reactor A should have processed a packet");

        // Reactor B should now have a packet in its inbox (from A's routing)
        let did_work_b = reactor_b.process_inbox();
        assert!(did_work_b, "Reactor B should have received routed packet");

        // Verify the packet was sent to B's backend (mock stores sent packets)
        let sent_to_b = reactor_b.backend.get_sent_packets();
        assert_eq!(
            sent_to_b.len(),
            1,
            "Reactor B should have sent 1 packet to backend"
        );

        // Verify the packet has correct structure
        let received_packet = &sent_to_b[0];
        assert!(
            received_packet.len() >= 34,
            "Packet should have eth + IP headers"
        );

        // Check IP addresses preserved through routing
        let src_ip = &received_packet[26..30];
        let dst_ip = &received_packet[30..34];
        assert_eq!(src_ip, &[10, 0, 0, 10], "Src IP should be A");
        assert_eq!(dst_ip, &[10, 0, 0, 20], "Dst IP should be B");

        // Check ethertype preserved
        let ethertype = u16::from_be_bytes([received_packet[12], received_packet[13]]);
        assert_eq!(ethertype, 0x0800, "Ethertype should be IPv4");
    }
}
