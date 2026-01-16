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
use super::buffer::{BufferPool, PoolBuffer};
use super::router::NetworkRouter;
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
    /// Network ID -> TUN reactor sender (for internet-bound packets)
    network_to_tun: ArcSwap<HashMap<String, ReactorSender>>,
}

impl ReactorRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            mac_to_sender: ArcSwap::new(Arc::new(HashMap::new())),
            nic_to_sender: ArcSwap::new(Arc::new(HashMap::new())),
            network_to_tun: ArcSwap::new(Arc::new(HashMap::new())),
        }
    }

    /// Register a vNIC reactor
    pub fn register_nic(&self, mac: [u8; 6], nic_id: String, sender: ReactorSender) {
        // Update MAC mapping
        let mut mac_map = (**self.mac_to_sender.load()).clone();
        mac_map.insert(mac, sender.clone());
        self.mac_to_sender.store(Arc::new(mac_map));

        // Update NIC ID mapping
        let mut nic_map = (**self.nic_to_sender.load()).clone();
        nic_map.insert(nic_id, sender);
        self.nic_to_sender.store(Arc::new(nic_map));
    }

    /// Unregister a vNIC reactor
    pub fn unregister_nic(&self, mac: [u8; 6], nic_id: &str) {
        let mut mac_map = (**self.mac_to_sender.load()).clone();
        mac_map.remove(&mac);
        self.mac_to_sender.store(Arc::new(mac_map));

        let mut nic_map = (**self.nic_to_sender.load()).clone();
        nic_map.remove(nic_id);
        self.nic_to_sender.store(Arc::new(nic_map));
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

/// Configuration for a vNIC reactor
pub struct VnicReactorConfig {
    pub nic_id: String,
    pub network_id: String,
    pub mac: [u8; 6],
    pub ipv4_addr: std::net::Ipv4Addr,
    pub ipv6_addr: std::net::Ipv6Addr,
    pub is_public: bool,
}

/// Protocol handlers for a reactor
struct ProtocolHandlers {
    arp: ArpResponder,
    dhcpv4: Option<Dhcpv4Server>,
    dhcpv6: Option<Dhcpv6Server>,
    icmp: IcmpResponder,
    icmpv6: Icmpv6Responder,
    ndp: NdpResponder,
}

impl ProtocolHandlers {
    fn new(config: &VnicReactorConfig) -> Self {
        // DHCP servers are optional - only created if IP addresses are assigned
        let dhcpv4 = if !config.ipv4_addr.is_unspecified() {
            Some(Dhcpv4Server::new(config.ipv4_addr, config.is_public))
        } else {
            None
        };

        let dhcpv6 = if !config.ipv6_addr.is_unspecified() {
            Some(Dhcpv6Server::new(config.ipv6_addr))
        } else {
            None
        };

        Self {
            arp: ArpResponder::new(config.mac),
            dhcpv4,
            dhcpv6,
            icmp: IcmpResponder::new(),
            icmpv6: Icmpv6Responder::new(),
            ndp: NdpResponder::new(config.mac, config.is_public),
        }
    }
}

/// Generic Reactor for packet processing
///
/// Type parameter `B` is the I/O backend (VhostBackend or TunBackend).
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
    /// Protocol handlers (ARP, DHCP, ICMP, NDP)
    handlers: ProtocolHandlers,
    /// Our MAC address (used for debugging/logging)
    #[allow(dead_code)]
    mac: [u8; 6],
    /// Our NIC ID
    nic_id: String,
    /// Network ID
    network_id: String,
    /// Shutdown signal
    shutdown: Receiver<()>,
}

impl<B: ReactorBackend> Reactor<B> {
    /// Create a new reactor
    pub fn new(
        backend: B,
        config: VnicReactorConfig,
        pool: Arc<BufferPool>,
        registry: Arc<ReactorRegistry>,
        router: NetworkRouter,
        shutdown: Receiver<()>,
    ) -> (Self, ReactorSender) {
        let (outbox, inbox) = reactor_channel();

        let reactor = Self {
            backend,
            inbox,
            outbox: outbox.clone(),
            pool,
            registry,
            router,
            handlers: ProtocolHandlers::new(&config),
            mac: config.mac,
            nic_id: config.nic_id,
            network_id: config.network_id,
            shutdown,
        };

        (reactor, outbox)
    }

    /// Run the reactor event loop
    pub fn run(&mut self) {
        info!(nic_id = %self.nic_id, "Reactor started");

        loop {
            // Check for shutdown
            if self.shutdown.try_recv().is_ok() {
                info!(nic_id = %self.nic_id, "Reactor shutting down");
                break;
            }

            let mut did_work = false;

            // 1. Process inbox (packets from other reactors)
            did_work |= self.process_inbox();

            // 2. Process backend RX (packets from device)
            did_work |= self.process_backend_rx();

            // 3. Process completions (e.g., TX completions for vhost)
            if let Err(e) = self.backend.process_completions() {
                warn!(nic_id = %self.nic_id, error = %e, "Completion processing failed");
            }

            // 4. If no work, poll with timeout
            if !did_work {
                self.poll_with_timeout();
            }
        }

        info!(nic_id = %self.nic_id, "Reactor stopped");
    }

    /// Process packets from inbox (other reactors)
    fn process_inbox(&mut self) -> bool {
        let mut count = 0;

        while count < BATCH_LIMIT {
            match self.inbox.try_recv() {
                Ok(packet) => {
                    count += 1;
                    // Packet from another reactor -> send to backend
                    if let Err(e) = self.backend.send(&packet.buffer) {
                        trace!(nic_id = %self.nic_id, error = %e, "Failed to send to backend");
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    info!(nic_id = %self.nic_id, "Inbox disconnected");
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
            // Allocate buffer
            let Some(mut buffer) = self.pool.alloc() else {
                warn!(nic_id = %self.nic_id, "Buffer pool exhausted");
                break;
            };

            // Try to receive
            match self.backend.try_recv(&mut buffer) {
                Ok(RecvResult::Packet(len)) => {
                    count += 1;
                    buffer.len = len;
                    self.handle_rx_packet(buffer);
                }
                Ok(RecvResult::WouldBlock) => break,
                Ok(RecvResult::Done) => {
                    info!(nic_id = %self.nic_id, "Backend done");
                    break;
                }
                Err(e) => {
                    trace!(nic_id = %self.nic_id, error = %e, "Backend recv error");
                    break;
                }
            }
        }

        count > 0
    }

    /// Handle a received packet from the backend
    fn handle_rx_packet(&mut self, buffer: PoolBuffer) {
        let data = buffer.data();
        if data.len() < 14 {
            return; // Too small for Ethernet
        }

        // Parse Ethernet header
        let ethertype = u16::from_be_bytes([data[12], data[13]]);

        // Try protocol handlers first (they consume the packet if matched)
        match ethertype {
            0x0806 => {
                // ARP
                if let Some(reply) = self.handlers.arp.process(data) {
                    self.send_reply(&reply);
                }
                return;
            }
            0x0800 => {
                // IPv4
                if self.handle_ipv4_protocols(&buffer) {
                    return;
                }
            }
            0x86DD => {
                // IPv6
                if self.handle_ipv6_protocols(&buffer) {
                    return;
                }
            }
            _ => {}
        }

        // Route the packet
        self.route_packet(buffer);
    }

    /// Send a reply packet (copies Vec<u8> into PoolBuffer)
    fn send_reply(&mut self, reply: &[u8]) {
        if let Some(mut buf) = self.pool.alloc() {
            let write_area = buf.write_area();
            if write_area.len() >= reply.len() {
                write_area[..reply.len()].copy_from_slice(reply);
                buf.len = reply.len();
                let _ = self.backend.send(&buf);
            }
        }
    }

    /// Handle IPv4 protocols (ICMP, DHCP)
    /// Returns true if packet was consumed
    fn handle_ipv4_protocols(&mut self, buffer: &PoolBuffer) -> bool {
        let data = buffer.data();
        if data.len() < 34 {
            return false; // Eth(14) + IP(20) minimum
        }

        let ip_header = &data[14..];
        let protocol = ip_header[9];

        match protocol {
            1 => {
                // ICMP
                if let Some(reply) = self.handlers.icmp.process(data) {
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
                        if let Some(ref dhcpv4) = self.handlers.dhcpv4
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
        let data = buffer.data();
        if data.len() < 54 {
            return false; // Eth(14) + IPv6(40) minimum
        }

        let next_header = data[14 + 6];

        match next_header {
            58 => {
                // ICMPv6
                // Check if it's NDP first
                if let Some(reply) = self.handlers.ndp.process(data) {
                    self.send_reply(&reply);
                    return true;
                }
                // Otherwise check regular ICMPv6 echo
                if let Some(reply) = self.handlers.icmpv6.process(data) {
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
                        if let Some(ref dhcpv6) = self.handlers.dhcpv6
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
    fn route_packet(&self, buffer: PoolBuffer) {
        let data = buffer.data();
        if data.len() < 14 {
            return;
        }

        // Check destination MAC
        let dst_mac: [u8; 6] = data[0..6].try_into().unwrap();

        // Broadcast/multicast - drop (handled by protocol handlers)
        if dst_mac[0] & 0x01 != 0 {
            debug!(nic_id = %self.nic_id, "Dropping broadcast/multicast");
            return;
        }

        // Try L2 forwarding by MAC
        if let Some(sender) = self.registry.get_by_mac(&dst_mac) {
            let _ = sender.try_send(InboundPacket { buffer });
            return;
        }

        // Try L3 routing
        let ethertype = u16::from_be_bytes([data[12], data[13]]);
        match ethertype {
            0x0800 => self.route_ipv4(buffer),
            0x86DD => self.route_ipv6(buffer),
            _ => {
                debug!(nic_id = %self.nic_id, ethertype, "Unknown ethertype, dropping");
            }
        }
    }

    /// Route an IPv4 packet
    fn route_ipv4(&self, buffer: PoolBuffer) {
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
            if let Some(sender) = self.registry.get_by_nic_id(&entry.nic_id) {
                let _ = sender.try_send(InboundPacket { buffer });
            }
        } else if self.router.is_public() {
            // No local route, send to TUN for internet
            if let Some(sender) = self.registry.get_tun(&self.network_id) {
                let _ = sender.try_send(InboundPacket { buffer });
            }
        } else {
            debug!(nic_id = %self.nic_id, %dst_ip, "No route for IPv4 (non-public network)");
        }
    }

    /// Route an IPv6 packet
    fn route_ipv6(&self, buffer: PoolBuffer) {
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
            if let Some(sender) = self.registry.get_by_nic_id(&entry.nic_id) {
                let _ = sender.try_send(InboundPacket { buffer });
            }
        } else if self.router.is_public() {
            // No local route, send to TUN for internet
            if let Some(sender) = self.registry.get_tun(&self.network_id) {
                let _ = sender.try_send(InboundPacket { buffer });
            }
        } else {
            debug!(nic_id = %self.nic_id, %dst_ip, "No route for IPv6 (non-public network)");
        }
    }

    /// Poll with timeout when idle
    fn poll_with_timeout(&self) {
        if let Some(fd) = self.backend.poll_fd() {
            let poll_fd = PollFd::new(unsafe { BorrowedFd::borrow_raw(fd) }, PollFlags::POLLIN);
            // 1ms timeout
            let _ = poll(&mut [poll_fd], PollTimeout::from(1u16));
        } else {
            // No fd to poll, just sleep briefly
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
