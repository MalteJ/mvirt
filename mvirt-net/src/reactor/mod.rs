pub mod arp;
pub mod dhcp;
pub mod dhcpv6;
pub mod icmpv6;
pub mod registry;

// Re-export inter-reactor types for convenience
pub use crate::inter_reactor::{CompletionNotify, PacketId, PacketRef, PacketSource, ReactorId};
pub use registry::{InterfaceType, ReactorInfo, ReactorRegistry};

use crate::routing::{IpPrefix, LpmTable, RouteTarget, RoutingDecision, RoutingTables};
use crate::tun::VNET_HDR_SIZE;
use crate::vhost_user::{GuestMemoryMmapAtomic, VhostHandshake, VringType};
use crate::virtqueue::{DescriptorChain, RxVirtqueue, TxPacket, TxVirtqueue};
use io_uring::{IoUring, opcode, types};
use nix::libc;
use nix::sys::eventfd::{EfdFlags, EventFd};
use smoltcp::wire::{
    EthernetAddress, EthernetFrame, EthernetProtocol, EthernetRepr, Icmpv4Message, Icmpv4Packet,
    Icmpv4Repr, IpProtocol, Ipv4Packet, Ipv4Repr, Ipv6Packet,
};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use tracing::{debug, error, info, warn};
use uuid::Uuid;
use vhost_user_backend::VringT;
use virtio_queue::QueueT;
use vm_memory::{Address, Bytes, GuestAddressSpace, GuestMemory};

/// Gateway MAC address used for all virtual NICs.
/// This is a locally-administered unicast address (02:xx:xx:xx:xx:xx).
pub const GATEWAY_MAC: [u8; 6] = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01];

/// IPv6 link-local gateway address (fe80::1)
pub const GATEWAY_IPV6_LINK_LOCAL: Ipv6Addr = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1);

/// IPv4 link-local gateway address (like AWS/GCP).
/// Uses 169.254.0.1 to provide a consistent gateway regardless of VM subnet.
pub const GATEWAY_IPV4_LINK_LOCAL: Ipv4Addr = Ipv4Addr::new(169, 254, 0, 1);

/// NIC configuration passed to the reactor for DHCP/ARP/ND handling.
#[derive(Clone, Debug)]
pub struct NicConfig {
    /// MAC address of the virtual NIC
    pub mac: [u8; 6],
    /// IPv4 address the VM will receive via DHCP
    pub ipv4_address: Option<Ipv4Addr>,
    /// IPv4 gateway from the VM's perspective
    pub ipv4_gateway: Option<Ipv4Addr>,
    /// IPv4 subnet prefix length
    pub ipv4_prefix_len: u8,
    /// IPv6 address the VM will receive via DHCPv6
    pub ipv6_address: Option<Ipv6Addr>,
    /// IPv6 gateway from the VM's perspective
    pub ipv6_gateway: Option<Ipv6Addr>,
    /// IPv6 subnet prefix length
    pub ipv6_prefix_len: u8,
    /// DNS servers for DHCP
    pub dns_servers: Vec<IpAddr>,
}

const USER_DATA_RX_FLAG: u64 = 1 << 63;
const USER_DATA_EVENT_FLAG: u64 = 1 << 62;
const USER_DATA_VHOST_TX_FLAG: u64 = 1 << 61;
const USER_DATA_INCOMING_TUN_FLAG: u64 = 1 << 60;
const USER_DATA_TUN_POLL_FLAG: u64 = 1 << 59;

/// virtio-net header size (with VIRTIO_NET_F_MRG_RXBUF)
const VIRTIO_NET_HDR_SIZE: usize = 12;

/// Ethernet header size
const ETHERNET_HDR_SIZE: usize = 14;

/// Size of buffer for peeking at packet headers and protocol handling.
/// Must be large enough for DHCP packets (12 + 14 + 20 + 8 + ~548 = ~600 bytes).
const PEEK_BUF_SIZE: usize = 600;

/// Maximum number of iovec segments for vhost TX packets.
/// Descriptor chains rarely exceed 4 segments; 8 provides headroom.
const MAX_TX_IOVECS: usize = 8;

/// Copies `dst.len()` bytes starting at `offset` from scattered iovecs into `dst`.
/// Returns false if insufficient data available.
fn copy_from_iovecs(
    iovecs: &[libc::iovec],
    iovecs_len: usize,
    offset: usize,
    dst: &mut [u8],
) -> bool {
    let len = dst.len();
    let mut pos = 0usize;
    let mut written = 0usize;

    for iov in iovecs.iter().take(iovecs_len) {
        let iov_end = pos + iov.iov_len;
        if iov_end > offset && written < len {
            let start_in_iov = offset.saturating_sub(pos);
            let bytes_avail = iov.iov_len - start_in_iov;
            let bytes_to_copy = bytes_avail.min(len - written);
            unsafe {
                std::ptr::copy_nonoverlapping(
                    (iov.iov_base as *const u8).add(start_in_iov),
                    dst.as_mut_ptr().add(written),
                    bytes_to_copy,
                );
            }
            written += bytes_to_copy;
        }
        pos = iov_end;
        if written >= len {
            break;
        }
    }
    written >= len
}

/// Adjusts virtio_net_hdr.csum_start when stripping Ethernet header.
/// When removing the 14-byte Ethernet header, csum_start must be reduced
/// by 14 because subsequent offsets shift backward.
#[inline]
fn patch_virtio_hdr_for_eth_stripping(virtio_hdr: &mut [u8; VIRTIO_NET_HDR_SIZE]) {
    const VIRTIO_NET_HDR_F_NEEDS_CSUM: u8 = 1;
    
    // Debug-Ausgabe VOR der Änderung
    let flags = virtio_hdr[0];
    let old_csum_start = u16::from_le_bytes([virtio_hdr[6], virtio_hdr[7]]);
    let old_csum_offset = u16::from_le_bytes([virtio_hdr[8], virtio_hdr[9]]);
    let old_hdr_len = u16::from_le_bytes([virtio_hdr[2], virtio_hdr[3]]);

    if flags & VIRTIO_NET_HDR_F_NEEDS_CSUM != 0 {
        // 1. Patch csum_start
        let csum_start = old_csum_start;
        let adjusted_start = csum_start.saturating_sub(ETHERNET_HDR_SIZE as u16);
        virtio_hdr[6..8].copy_from_slice(&adjusted_start.to_le_bytes());

        // 2. Patch hdr_len
        let hdr_len = old_hdr_len;
        let adjusted_len = if hdr_len >= ETHERNET_HDR_SIZE as u16 {
            hdr_len - ETHERNET_HDR_SIZE as u16
        } else {
            hdr_len // Sollte nicht passieren
        };
        virtio_hdr[2..4].copy_from_slice(&adjusted_len.to_le_bytes());

        // Logge, was wir tun
        debug!(
            "Offload Patch: flags={:#x} csum_start={}->{} csum_offset={} hdr_len={}->{}",
            flags, old_csum_start, adjusted_start, old_csum_offset, old_hdr_len, adjusted_len
        );
    }
}

/// Offset of num_buffers field in virtio_net_hdr (with MRG_RXBUF)
const NUM_BUFFERS_OFFSET: usize = 10;

/// Patches the num_buffers field in virtio_net_hdr in guest memory.
#[inline]
fn patch_num_buffers<M: GuestMemory>(mem: &M, hdr_addr: vm_memory::GuestAddress, num_buffers: u16) {
    let num_buffers_addr = hdr_addr
        .checked_add(NUM_BUFFERS_OFFSET as u64)
        .expect("num_buffers address overflow");
    let _ = mem.write_slice(&num_buffers.to_le_bytes(), num_buffers_addr);
}

/// Prepares VhostTxInFlight for TUN transmission by:
/// 1. Copying and patching the virtio_net_hdr
/// 2. Building iovecs that skip the Ethernet header (pointing to guest memory)
///
/// Returns false if packet is malformed (too short).
fn prepare_tun_iovecs(
    src_iovecs: &[libc::iovec; MAX_TX_IOVECS],
    src_len: usize,
    in_flight: &mut VhostTxInFlight,
) -> bool {
    const SKIP_END: usize = VIRTIO_NET_HDR_SIZE + ETHERNET_HDR_SIZE; // 26

    // 1. Copy virtio_net_hdr from source iovecs
    if !copy_from_iovecs(src_iovecs, src_len, 0, &mut in_flight.patched_virtio_hdr) {
        return false;
    }

    // 2. Patch csum_start for Ethernet stripping
    patch_virtio_hdr_for_eth_stripping(&mut in_flight.patched_virtio_hdr);

    // 3. First iovec points to our local patched header
    in_flight.iovecs[0] = libc::iovec {
        iov_base: in_flight.patched_virtio_hdr.as_mut_ptr() as *mut _,
        iov_len: VIRTIO_NET_HDR_SIZE,
    };
    let mut iov_idx = 1usize;

    // 4. Build payload iovecs (skip first 26 bytes = virtio_hdr + ethernet)
    let mut pos = 0usize;
    for iov in src_iovecs.iter().take(src_len) {
        let iov_end = pos + iov.iov_len;
        if iov_end > SKIP_END && iov_idx < MAX_TX_IOVECS {
            let start_in_iov = SKIP_END.saturating_sub(pos);
            in_flight.iovecs[iov_idx] = libc::iovec {
                iov_base: unsafe { (iov.iov_base as *mut u8).add(start_in_iov) as *mut _ },
                iov_len: iov.iov_len - start_in_iov,
            };
            iov_idx += 1;
        }
        pos = iov_end;
    }

    in_flight.iovecs_len = iov_idx;
    iov_idx > 1 // Must have at least header + some payload
}

/// vhost-user queue indices
const VHOST_RX_QUEUE: usize = 0;
const VHOST_TX_QUEUE: usize = 1;

/// vhost-user state received via handshake
struct VhostState {
    mem: GuestMemoryMmapAtomic,
    vrings: Vec<VringType>,
}

/// Tracks an in-flight vhost TX operation for zero-copy I/O.
/// Uses fixed-size arrays and is Box-allocated to ensure stable memory
/// addresses for io_uring operations (HashMap insertion can move data).
struct VhostTxInFlight {
    head_index: u16,
    total_len: u32,
    iovecs: [libc::iovec; MAX_TX_IOVECS],
    iovecs_len: usize,
    keep_alive: Option<Arc<dyn std::any::Any + Send + Sync>>,
    /// Local copy of virtio_net_hdr with patched csum_start for TUN transmission
    patched_virtio_hdr: [u8; VIRTIO_NET_HDR_SIZE],
}

/// Tracks an in-flight VM-to-VM packet awaiting completion
#[allow(dead_code)]
struct VhostToVhostInFlight {
    head_index: u16,
    total_len: u32,
}

/// Tracks an in-flight incoming packet being written to TUN
struct IncomingToTunInFlight {
    /// Packet ID for completion notification
    packet_id: PacketId,
    /// Source information for completion notification
    source: PacketSource,
    /// Buffer containing the L3 packet data (owned to keep alive during async I/O)
    #[allow(dead_code)]
    buffer: Vec<u8>,
}

/// Commands that can be sent to the reactor
#[derive(Debug)]
pub enum ReactorCommand {
    Shutdown,
    /// Create a new routing table
    CreateTable {
        id: Uuid,
        name: String,
    },
    /// Delete a routing table
    DeleteTable {
        id: Uuid,
    },
    /// Add a route to a table
    AddRoute {
        table_id: Uuid,
        prefix: IpPrefix,
        target: RouteTarget,
    },
    /// Remove a route from a table
    RemoveRoute {
        table_id: Uuid,
        prefix: IpPrefix,
    },
    /// Set the default routing table
    SetDefaultTable {
        id: Uuid,
    },
}

/// Handle for controlling the reactor from outside
pub struct ReactorHandle {
    event_fd: OwnedFd,
    command_tx: Sender<ReactorCommand>,
}

impl ReactorHandle {
    /// Signal the reactor to shutdown
    pub fn shutdown(&self) {
        self.send_command(ReactorCommand::Shutdown);
    }

    /// Get a duplicated eventfd for vhost daemon to notify the reactor
    pub fn get_notify_fd(&self) -> OwnedFd {
        let dup_fd = unsafe { nix::libc::dup(self.event_fd.as_raw_fd()) };
        // SAFETY: dup returns a valid fd on success
        unsafe { OwnedFd::from_raw_fd(dup_fd) }
    }

    /// Create a new routing table
    pub fn create_table(&self, id: Uuid, name: impl Into<String>) {
        self.send_command(ReactorCommand::CreateTable {
            id,
            name: name.into(),
        });
    }

    /// Delete a routing table
    pub fn delete_table(&self, id: Uuid) {
        self.send_command(ReactorCommand::DeleteTable { id });
    }

    /// Add a route to a table
    pub fn add_route(&self, table_id: Uuid, prefix: IpPrefix, target: RouteTarget) {
        self.send_command(ReactorCommand::AddRoute {
            table_id,
            prefix,
            target,
        });
    }

    /// Remove a route from a table
    pub fn remove_route(&self, table_id: Uuid, prefix: IpPrefix) {
        self.send_command(ReactorCommand::RemoveRoute { table_id, prefix });
    }

    /// Set the default routing table
    pub fn set_default_table(&self, id: Uuid) {
        self.send_command(ReactorCommand::SetDefaultTable { id });
    }

    /// Send a command to the reactor and signal via eventfd
    fn send_command(&self, cmd: ReactorCommand) {
        let _ = self.command_tx.send(cmd);
        let buf: u64 = 1;
        unsafe {
            nix::libc::write(
                self.event_fd.as_raw_fd(),
                &buf as *const u64 as *const nix::libc::c_void,
                8,
            );
        }
    }
}

pub struct Reactor<RX, TX> {
    rx_queue: RX,
    tx_queue: TX,
    event_fd: RawFd,
    command_rx: Receiver<ReactorCommand>,
    /// Receiver for vhost handshake (optional, for vhost-user integration)
    handshake_rx: Option<Receiver<VhostHandshake>>,
    /// LPM routing tables
    routing_tables: RoutingTables,
    /// Unique identifier for this reactor
    reactor_id: ReactorId,
    /// Shared reactor registry for inter-reactor communication
    registry: Option<Arc<ReactorRegistry>>,
    /// Receiver for incoming packets from other reactors
    packet_rx: Option<Receiver<PacketRef>>,
    /// Receiver for completion notifications from other reactors
    completion_rx: Option<Receiver<CompletionNotify>>,
    /// Counter for generating unique packet IDs
    next_packet_id: u64,
    /// NIC configuration for DHCP/ARP/ND handling (for vhost interfaces)
    nic_config: Option<NicConfig>,
}

impl<RX: RxVirtqueue, TX: TxVirtqueue> Reactor<RX, TX> {
    /// Create a new reactor and return it along with a handle for control
    pub fn new(rx_queue: RX, tx_queue: TX) -> (Self, ReactorHandle) {
        Self::with_vhost(rx_queue, tx_queue, None, None)
    }

    /// Create a new reactor with optional vhost handshake receiver
    pub fn with_vhost(
        rx_queue: RX,
        tx_queue: TX,
        handshake_rx: Option<Receiver<VhostHandshake>>,
        nic_config: Option<NicConfig>,
    ) -> (Self, ReactorHandle) {
        Self::with_registry(
            rx_queue,
            tx_queue,
            handshake_rx,
            None,
            None,
            None,
            nic_config,
            None, // No initial tables
        )
    }

    /// Create a new reactor with full inter-reactor communication support.
    ///
    /// # Parameters
    /// - `initial_tables`: Optional pre-populated routing tables. When provided,
    ///   the reactor starts with these tables instead of empty ones. This is used
    ///   when registering with a RoutingManager to receive the current routing state.
    #[allow(clippy::too_many_arguments)]
    pub fn with_registry(
        rx_queue: RX,
        tx_queue: TX,
        handshake_rx: Option<Receiver<VhostHandshake>>,
        registry: Option<Arc<ReactorRegistry>>,
        packet_rx: Option<Receiver<PacketRef>>,
        completion_rx: Option<Receiver<CompletionNotify>>,
        nic_config: Option<NicConfig>,
        initial_tables: Option<RoutingTables>,
    ) -> (Self, ReactorHandle) {
        // Create eventfd for signaling
        let efd = EventFd::from_value_and_flags(0, EfdFlags::EFD_NONBLOCK)
            .expect("Failed to create eventfd");
        let event_fd_raw = efd.as_raw_fd();

        // Duplicate fd for the handle (one for reactor, one for handle)
        let efd_dup = unsafe { OwnedFd::from_raw_fd(nix::libc::dup(event_fd_raw)) };

        // Create mpsc channel for commands
        let (command_tx, command_rx) = mpsc::channel();

        let reactor = Reactor {
            rx_queue,
            tx_queue,
            event_fd: event_fd_raw,
            command_rx,
            handshake_rx,
            routing_tables: initial_tables.unwrap_or_default(),
            reactor_id: ReactorId::new(),
            registry,
            packet_rx,
            completion_rx,
            next_packet_id: 0,
            nic_config,
        };

        // Keep the original efd alive by forgetting it (reactor uses the raw fd)
        std::mem::forget(efd);

        let handle = ReactorHandle {
            event_fd: efd_dup,
            command_tx,
        };

        (reactor, handle)
    }

    /// Get the reactor's unique ID
    pub fn id(&self) -> ReactorId {
        self.reactor_id
    }

    /// Get a reference to the reactor's routing tables.
    pub fn routing_tables(&self) -> &RoutingTables {
        &self.routing_tables
    }

    /// Generate a new unique packet ID
    fn next_packet_id(&mut self) -> PacketId {
        let id = self.next_packet_id;
        self.next_packet_id = self.next_packet_id.wrapping_add(1);
        PacketId::new(id)
    }

    /// Route an IPv4 packet.
    fn route_ipv4(&self, ip_data: &[u8]) -> RoutingDecision {
        // Minimum IPv4 header is 20 bytes
        if ip_data.len() < 20 {
            debug!("route_ipv4: packet too short for IPv4 header");
            return RoutingDecision::Drop;
        }
        // Use new_unchecked because we may only have a truncated peek buffer
        let ipv4 = Ipv4Packet::new_unchecked(ip_data);

        let dst_addr = ipv4.dst_addr();
        let src_addr = ipv4.src_addr();

        debug!(
            src = %src_addr,
            dst = %dst_addr,
            "route_ipv4: processing packet"
        );

        // LPM lookup in routing tables
        if let Some(table) = self.routing_tables.get_default() {
            let dst_v4 =
                std::net::Ipv4Addr::new(dst_addr.0[0], dst_addr.0[1], dst_addr.0[2], dst_addr.0[3]);
            if let Some(target) = table.lookup_v4(dst_v4) {
                let decision = Self::route_target_to_decision(target);
                debug!(?decision, "route_ipv4: route found");
                return decision;
            }
            warn!(
                reactor_id = %self.reactor_id,
                dst = %dst_v4,
                "route_ipv4: no matching route in table"
            );
        } else {
            warn!(
                reactor_id = %self.reactor_id,
                "route_ipv4: no default routing table configured!"
            );
        }

        warn!(
            reactor_id = %self.reactor_id,
            src = %src_addr,
            dst = %dst_addr,
            "route_ipv4: DROPPING packet (no route)"
        );
        RoutingDecision::Drop
    }

    /// Route an IPv6 packet.
    fn route_ipv6(&self, ip_data: &[u8]) -> RoutingDecision {
        // IPv6 header is always 40 bytes
        if ip_data.len() < 40 {
            debug!("route_ipv6: packet too short for IPv6 header");
            return RoutingDecision::Drop;
        }
        // Use new_unchecked because we may only have a truncated peek buffer
        let ipv6 = Ipv6Packet::new_unchecked(ip_data);

        let dst_addr = ipv6.dst_addr();
        let src_addr = ipv6.src_addr();

        debug!(
            src = %src_addr,
            dst = %dst_addr,
            "route_ipv6: processing packet"
        );

        // LPM lookup in routing tables
        if let Some(table) = self.routing_tables.get_default() {
            let dst_v6 = std::net::Ipv6Addr::from(dst_addr.0);
            if let Some(target) = table.lookup_v6(dst_v6) {
                let decision = Self::route_target_to_decision(target);
                debug!(?decision, "route_ipv6: route found");
                return decision;
            }
            debug!(dst = %dst_v6, "route_ipv6: no matching route");
        } else {
            debug!("route_ipv6: no default routing table");
        }

        debug!("route_ipv6: dropping packet (no route)");
        RoutingDecision::Drop
    }

    /// Convert a RouteTarget to a RoutingDecision
    fn route_target_to_decision(target: &RouteTarget) -> RoutingDecision {
        match target {
            RouteTarget::Reactor { id } => RoutingDecision::ToVhost { reactor_id: *id },
            RouteTarget::Vhost { id: _ } => {
                // Legacy vhost target - would need registry lookup
                RoutingDecision::Drop
            }
            RouteTarget::Tun { if_index } => RoutingDecision::ToTun {
                if_index: *if_index,
            },
            RouteTarget::Custom { .. } => RoutingDecision::Drop,
            RouteTarget::Drop => RoutingDecision::Drop,
        }
    }

    pub fn run(mut self) {
        info!("Reactor running");

        // Get fd from queues (they share the same fd)
        let rx_fd = self.rx_queue.fd();
        let tx_fd = self.tx_queue.fd();
        assert_eq!(rx_fd, tx_fd, "RX and TX must use the same fd");
        let tun_fd = types::Fd(rx_fd);
        let event_fd = types::Fd(self.event_fd);

        // Combine RX and TX iovecs for registration
        let rx_iovecs = self.rx_queue.get_iovecs_for_registration();
        let tx_iovecs = self.tx_queue.get_iovecs_for_registration();

        let mut all_iovecs: Vec<libc::iovec> =
            Vec::with_capacity(rx_iovecs.len() + tx_iovecs.len());
        all_iovecs.extend(rx_iovecs);
        all_iovecs.extend(tx_iovecs);

        let ring_size = all_iovecs.len().max(256) as u32;

        let mut ring = match IoUring::new(ring_size) {
            Ok(r) => r,
            Err(e) => {
                error!(error = %e, "Failed to create io_uring");
                return;
            }
        };

        // Register all buffers with io_uring
        if !all_iovecs.is_empty() {
            if let Err(e) = unsafe { ring.submitter().register_buffers(&all_iovecs) } {
                error!(error = %e, "Failed to register buffers");
                return;
            }
            info!(count = all_iovecs.len(), "Registered buffers with io_uring");
        }

        // Track in-flight RX reads
        let mut rx_in_flight: std::collections::HashMap<u64, DescriptorChain> =
            std::collections::HashMap::new();

        // Track RX chains waiting for poll completion (EAGAIN handling)
        let mut rx_poll_pending: std::collections::HashMap<u64, DescriptorChain> =
            std::collections::HashMap::new();

        // Track in-flight TX writes
        let mut tx_in_flight: std::collections::HashMap<u64, DescriptorChain> =
            std::collections::HashMap::new();

        // Track in-flight vhost TX writes (to TUN)
        // Box ensures stable memory addresses for io_uring (HashMap can move on grow)
        let mut vhost_tx_in_flight: std::collections::HashMap<u64, Box<VhostTxInFlight>> =
            std::collections::HashMap::new();
        let mut vhost_tx_id: u64 = 0;

        // Track in-flight VM-to-VM packets (awaiting CompletionNotify)
        let mut vhost_to_vhost_in_flight: std::collections::HashMap<u64, VhostToVhostInFlight> =
            std::collections::HashMap::new();

        // Track in-flight incoming packets being written to TUN (for TUN-only reactors)
        let mut incoming_to_tun_in_flight: std::collections::HashMap<u64, IncomingToTunInFlight> =
            std::collections::HashMap::new();
        let mut incoming_tun_id: u64 = 0;

        // Track in-flight TunRx packets (zero-copy path: packet_id -> chain_id)
        // Buffer is NOT returned to pool until completion is received
        let mut tun_rx_in_flight: std::collections::HashMap<u64, u64> =
            std::collections::HashMap::new();

        // Pending TX packets to send
        let mut pending_tx: Vec<TxPacket> = Vec::new();

        // vhost-user state (populated after handshake)
        let mut vhost_state: Option<VhostState> = None;

        // Buffer for eventfd reads
        let mut eventfd_buf: u64 = 0;

        // Submit read on eventfd to watch for signals
        let eventfd_read = opcode::Read::new(event_fd, &mut eventfd_buf as *mut u64 as *mut u8, 8)
            .build()
            .user_data(USER_DATA_EVENT_FLAG);

        unsafe {
            if ring.submission().push(&eventfd_read).is_err() {
                error!("Failed to submit eventfd read");
                return;
            }
        }

        // Submit initial RX reads (limit to ring size to avoid overwhelming)
        let max_outstanding = (ring_size as usize).saturating_sub(1); // Reserve 1 for eventfd
        let mut submitted = 0;
        while submitted < max_outstanding {
            let Some(chain) = self.rx_queue.pop_available() else {
                break;
            };

            let read_e = opcode::ReadFixed::new(
                tun_fd,
                chain.buffer.ptr,
                chain.buffer.len,
                chain.buffer.buf_index,
            )
            .build()
            .user_data(chain.chain_id | USER_DATA_RX_FLAG);

            unsafe {
                if ring.submission().push(&read_e).is_err() {
                    warn!("SQ full during initial RX submit");
                    self.rx_queue.push_used(chain.chain_id, 0);
                    break;
                }
            }
            rx_in_flight.insert(chain.chain_id, chain);
            submitted += 1;
        }

        let mut shutdown_requested = false;

        loop {
            // Submit pending and wait for at least 1 completion
            if let Err(e) = ring.submit_and_wait(1) {
                error!(error = %e, "io_uring submit_and_wait failed");
                break;
            }

            // Collect completions
            let completions: Vec<(u64, i32)> = ring
                .completion()
                .map(|cqe| (cqe.user_data(), cqe.result()))
                .collect();

            // Track reactors that received packets this batch (for batched signaling)
            let mut reactors_to_signal: std::collections::HashSet<ReactorId> =
                std::collections::HashSet::new();

            // Process all completions
            for (user_data, result) in completions {
                let is_event = (user_data & USER_DATA_EVENT_FLAG) != 0;
                let is_rx = (user_data & USER_DATA_RX_FLAG) != 0;
                let chain_id = user_data
                    & !(USER_DATA_RX_FLAG | USER_DATA_EVENT_FLAG | USER_DATA_TUN_POLL_FLAG);

                if is_event {
                    // Eventfd signaled - check for commands
                    if result > 0 {
                        while let Ok(cmd) = self.command_rx.try_recv() {
                            match cmd {
                                ReactorCommand::Shutdown => {
                                    info!("Shutdown requested");
                                    shutdown_requested = true;
                                }
                                ReactorCommand::CreateTable { id, name } => {
                                    debug!(%id, %name, "Creating routing table");
                                    self.routing_tables.add_table(LpmTable::new(id, name));
                                }
                                ReactorCommand::DeleteTable { id } => {
                                    debug!(%id, "Deleting routing table");
                                    self.routing_tables.remove_table(&id);
                                }
                                ReactorCommand::AddRoute {
                                    table_id,
                                    prefix,
                                    target,
                                } => {
                                    if let Some(table) =
                                        self.routing_tables.get_table_mut(&table_id)
                                    {
                                        match prefix {
                                            IpPrefix::V4(p) => {
                                                info!(
                                                    reactor_id = %self.reactor_id,
                                                    %table_id,
                                                    %p,
                                                    ?target,
                                                    "Adding IPv4 route"
                                                );
                                                table.insert_v4(p, target);
                                            }
                                            IpPrefix::V6(p) => {
                                                info!(
                                                    reactor_id = %self.reactor_id,
                                                    %table_id,
                                                    %p,
                                                    ?target,
                                                    "Adding IPv6 route"
                                                );
                                                table.insert_v6(p, target);
                                            }
                                        }
                                    } else {
                                        warn!(%table_id, "Route add failed: table not found");
                                    }
                                }
                                ReactorCommand::RemoveRoute { table_id, prefix } => {
                                    if let Some(table) =
                                        self.routing_tables.get_table_mut(&table_id)
                                    {
                                        match prefix {
                                            IpPrefix::V4(p) => {
                                                debug!(%table_id, %p, "Removing IPv4 route");
                                                table.remove_v4(&p);
                                            }
                                            IpPrefix::V6(p) => {
                                                debug!(%table_id, %p, "Removing IPv6 route");
                                                table.remove_v6(&p);
                                            }
                                        }
                                    }
                                }
                                ReactorCommand::SetDefaultTable { id } => {
                                    debug!(%id, "Setting default routing table");
                                    self.routing_tables.set_default(id);
                                }
                            }
                        }

                        // Check for vhost handshake (supports reconnection)
                        if let Some(ref rx) = self.handshake_rx
                            && let Ok(handshake) = rx.try_recv()
                        {
                            if vhost_state.is_some() {
                                info!("Received new vhost handshake (VM reconnected)");
                            } else {
                                info!("Received vhost handshake from daemon");
                            }
                            vhost_state = Some(VhostState {
                                mem: handshake.mem,
                                vrings: handshake.vrings,
                            });
                        }

                        // Process vhost queues if we have state
                        if let Some(ref state) = vhost_state {
                            self.process_vhost_tx(
                                state,
                                &mut ring,
                                tun_fd,
                                &mut vhost_tx_in_flight,
                                &mut vhost_tx_id,
                                &mut vhost_to_vhost_in_flight,
                            );
                        }

                        // Process incoming packets from other reactors
                        if let Some(ref state) = vhost_state {
                            self.process_incoming_packets(state, &mut vhost_to_vhost_in_flight);
                        } else {
                            // TUN-only reactor: write incoming packets to TUN fd
                            self.process_incoming_packets_to_tun(
                                &mut ring,
                                tun_fd,
                                &mut incoming_to_tun_in_flight,
                                &mut incoming_tun_id,
                            );
                        }

                        // Process all completion notifications (unified handling)
                        if let Some(ref completion_rx) = self.completion_rx {
                            while let Ok(completion) = completion_rx.try_recv() {
                                debug!(
                                    id = %completion.packet_id(),
                                    result = completion.result(),
                                    reactor_id = %self.reactor_id,
                                    "Received completion notification"
                                );

                                match completion {
                                    CompletionNotify::VhostToVhostComplete {
                                        packet_id,
                                        head_index,
                                        total_len,
                                        result,
                                    }
                                    | CompletionNotify::VhostTxComplete {
                                        packet_id,
                                        head_index,
                                        total_len,
                                        result,
                                    } => {
                                        if let Some(ref state) = vhost_state {
                                            // Remove from in-flight tracking
                                            if vhost_to_vhost_in_flight
                                                .remove(&packet_id.raw())
                                                .is_some()
                                            {
                                                debug!(
                                                    packet_id = %packet_id,
                                                    head_index,
                                                    total_len,
                                                    result,
                                                    "Returning TX descriptor to guest"
                                                );
                                                // Return descriptor to guest
                                                let mem_guard = state.mem.memory();
                                                let tx_vring = &state.vrings[VHOST_TX_QUEUE];
                                                let mut vring_state = tx_vring.get_mut();
                                                let queue = vring_state.get_queue_mut();

                                                let used_len =
                                                    if result >= 0 { total_len } else { 0 };
                                                let _ = queue.add_used(
                                                    &*mem_guard,
                                                    head_index,
                                                    used_len,
                                                );
                                                let _ = vring_state.signal_used_queue();

                                                if result < 0 {
                                                    warn!(
                                                        error = -result,
                                                        "Packet delivery failed"
                                                    );
                                                }
                                            } else {
                                                warn!(
                                                    id = %packet_id,
                                                    in_flight_count = vhost_to_vhost_in_flight.len(),
                                                    "Completion for unknown packet"
                                                );
                                            }
                                        } else {
                                            debug!(
                                                id = %packet_id,
                                                "Vhost completion received but no vhost state"
                                            );
                                        }
                                    }
                                    CompletionNotify::TunRxComplete {
                                        packet_id,
                                        chain_id,
                                        result,
                                    } => {
                                        // Remove from in-flight tracking
                                        if tun_rx_in_flight.remove(&packet_id.raw()).is_some() {
                                            debug!(
                                                packet_id = %packet_id,
                                                chain_id,
                                                result,
                                                "TUN reactor: received TunRxComplete, returning buffer"
                                            );

                                            // Return buffer to pool
                                            self.rx_queue.push_used(chain_id, result as u32);

                                            // Resubmit RX read
                                            if let Some(new_chain) = self.rx_queue.pop_available() {
                                                let read_e = opcode::ReadFixed::new(
                                                    tun_fd,
                                                    new_chain.buffer.ptr,
                                                    new_chain.buffer.len,
                                                    new_chain.buffer.buf_index,
                                                )
                                                .build()
                                                .user_data(new_chain.chain_id | USER_DATA_RX_FLAG);

                                                unsafe {
                                                    if ring.submission().push(&read_e).is_ok() {
                                                        rx_in_flight
                                                            .insert(new_chain.chain_id, new_chain);
                                                    }
                                                }
                                            }
                                        } else {
                                            warn!(
                                                packet_id = %packet_id,
                                                chain_id,
                                                "TunRxComplete for unknown packet"
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // If not shutting down, resubmit eventfd read
                    if !shutdown_requested {
                        let eventfd_read =
                            opcode::Read::new(event_fd, &mut eventfd_buf as *mut u64 as *mut u8, 8)
                                .build()
                                .user_data(USER_DATA_EVENT_FLAG);

                        unsafe {
                            let _ = ring.submission().push(&eventfd_read);
                        }
                    }
                    continue;
                }

                // Check for vhost TX completion
                let is_vhost_tx = (user_data & USER_DATA_VHOST_TX_FLAG) != 0;
                if is_vhost_tx {
                    if let Some(in_flight) = vhost_tx_in_flight.remove(&user_data) {
                        if result < 0 {
                            error!(error = -result, "vhost TX writev error");
                        }
                        // Return descriptor to guest
                        if let Some(ref state) = vhost_state {
                            let mem_guard = state.mem.memory();
                            let tx_vring = &state.vrings[VHOST_TX_QUEUE];
                            let mut vring_state = tx_vring.get_mut();
                            let queue = vring_state.get_queue_mut();
                            let _ = queue.add_used(
                                &*mem_guard,
                                in_flight.head_index,
                                in_flight.total_len,
                            );
                            let _ = vring_state.signal_used_queue();
                        }
                    }
                    continue;
                }

                // Check for incoming-to-TUN completion (packets from other reactors written to TUN)
                let is_incoming_tun = (user_data & USER_DATA_INCOMING_TUN_FLAG) != 0;
                if is_incoming_tun {
                    if let Some(in_flight) = incoming_to_tun_in_flight.remove(&user_data) {
                        let write_result = if result < 0 {
                            error!(error = -result, "incoming-to-TUN write error");
                            result
                        } else {
                            debug!(
                                len = result,
                                packet_id = %in_flight.packet_id,
                                src = %in_flight.source.source_reactor(),
                                "TUN reactor: write to TUN complete, sending completion"
                            );
                            result
                        };

                        // Send completion notification back to source reactor
                        let completion = match &in_flight.source {
                            PacketSource::VhostToVhost {
                                head_index,
                                total_len,
                                ..
                            } => CompletionNotify::VhostToVhostComplete {
                                packet_id: in_flight.packet_id,
                                head_index: *head_index,
                                total_len: *total_len,
                                result: write_result,
                            },
                            PacketSource::TunRx {
                                chain_id,
                                source_reactor: _,
                                len,
                                ..
                            } => CompletionNotify::TunRxComplete {
                                packet_id: in_flight.packet_id,
                                chain_id: *chain_id,
                                result: *len as i32,
                            },
                            PacketSource::VhostTx {
                                head_index,
                                total_len,
                                source_reactor: _,
                            } => CompletionNotify::VhostTxComplete {
                                packet_id: in_flight.packet_id,
                                head_index: *head_index,
                                total_len: *total_len,
                                result: write_result,
                            },
                        };

                        if let Some(ref registry) = self.registry {
                            let source_reactor = in_flight.source.source_reactor();
                            if !registry.send_completion_to(&source_reactor, completion) {
                                warn!(src = %source_reactor, "Failed to send completion to source reactor");
                            }
                        }
                    }
                    continue;
                }

                // Handle TUN poll completion (EAGAIN recovery)
                let is_tun_poll = (user_data & USER_DATA_TUN_POLL_FLAG) != 0;
                if is_tun_poll {
                    if let Some(chain) = rx_poll_pending.remove(&chain_id) {
                        if result >= 0 {
                            // Poll succeeded - TUN is ready, resubmit ReadFixed
                            let read_e = opcode::ReadFixed::new(
                                tun_fd,
                                chain.buffer.ptr,
                                chain.buffer.len,
                                chain.buffer.buf_index,
                            )
                            .build()
                            .user_data(chain.chain_id | USER_DATA_RX_FLAG);

                            unsafe {
                                if ring.submission().push(&read_e).is_ok() {
                                    rx_in_flight.insert(chain.chain_id, chain);
                                } else {
                                    self.rx_queue.push_used(chain.chain_id, 0);
                                }
                            }
                        } else {
                            error!(error = -result, chain_id, "TUN poll error");
                            self.rx_queue.push_used(chain.chain_id, 0);
                        }
                    }
                    continue;
                }

                if is_rx {
                    // RX completion - packet received from TUN (L3 packet)
                    let chain = match rx_in_flight.remove(&chain_id) {
                        Some(c) => c,
                        None => {
                            error!(chain_id, "Unknown RX chain_id");
                            continue;
                        }
                    };

                    if result < 0 {
                        if result == -libc::EAGAIN {
                            // No data ready - submit PollAdd instead of immediate resubmit
                            let poll_e = opcode::PollAdd::new(tun_fd, libc::POLLIN as u32)
                                .build()
                                .user_data(chain.chain_id | USER_DATA_TUN_POLL_FLAG);

                            unsafe {
                                if ring.submission().push(&poll_e).is_ok() {
                                    rx_poll_pending.insert(chain.chain_id, chain);
                                } else {
                                    self.rx_queue.push_used(chain.chain_id, 0);
                                }
                            }
                            continue;
                        }
                        error!(error = -result, chain_id, "RX read error");
                        self.rx_queue.push_used(chain_id, 0);
                        continue;
                    }

                    if result == 0 {
                        info!("EOF");
                        return;
                    }

                    let len = result as usize;

                    // Route L3 packet to appropriate VM
                    if len > VNET_HDR_SIZE {
                        let packet_data =
                            unsafe { std::slice::from_raw_parts(chain.buffer.ptr, len) };
                        let ip_data = &packet_data[VNET_HDR_SIZE..];

                        // Determine IP version and route
                        let ip_version = ip_data.first().map(|b| b >> 4);
                        let routing_decision = match ip_version {
                            Some(4) => self.route_ipv4(ip_data),
                            Some(6) => self.route_ipv6(ip_data),
                            _ => {
                                debug!(len, "TUN RX: unknown IP version");
                                RoutingDecision::Drop
                            }
                        };

                        match routing_decision {
                            RoutingDecision::ToVhost {
                                reactor_id: target_reactor,
                            } => {
                                // Forward to VM - zero-copy path with Ethernet header injection at destination
                                // Clone registry to avoid borrow conflicts
                                let registry_opt = self.registry.clone();
                                if let Some(registry) = registry_opt {
                                    // Get destination MAC from registry
                                    let dst_mac = registry
                                        .get_mac_for_reactor(&target_reactor)
                                        .unwrap_or([0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);

                                    // Determine ethertype from IP version
                                    let ethertype: u16 = match ip_version {
                                        Some(4) => 0x0800, // IPv4
                                        Some(6) => 0x86DD, // IPv6
                                        _ => 0x0800,
                                    };

                                    // Create PacketRef pointing directly to TUN buffer - no allocation!
                                    let packet_id = self.next_packet_id();
                                    let source = PacketSource::TunRx {
                                        chain_id,
                                        len: len as u32,
                                        source_reactor: self.reactor_id,
                                        dst_mac,
                                        ethertype,
                                    };

                                    // Point directly to TUN buffer
                                    let iovec = libc::iovec {
                                        iov_base: chain.buffer.ptr as *mut _,
                                        iov_len: len,
                                    };

                                    let packet = PacketRef::new(
                                        packet_id,
                                        vec![iovec],
                                        source,
                                        None, // No keep_alive - buffer managed by rx_queue
                                    );

                                    if registry.send_packet_to_no_signal(&target_reactor, packet) {
                                        debug!(
                                            len,
                                            dst = %target_reactor,
                                            "TUN RX -> vhost (zero-copy)"
                                        );
                                        // Track reactor for batched signaling
                                        reactors_to_signal.insert(target_reactor);
                                        // Track in-flight - DON'T return buffer yet
                                        tun_rx_in_flight.insert(packet_id.raw(), chain_id);
                                        // Continue without returning the buffer
                                        continue;
                                    } else {
                                        debug!(dst = %target_reactor, "TUN RX: target reactor not found");
                                    }
                                } else {
                                    debug!("TUN RX: no registry for routing");
                                }
                            }
                            RoutingDecision::ToTun { .. } => {
                                // Packet from TUN shouldn't route back to TUN
                                debug!(len, "TUN RX: dropping packet (would route to TUN)");
                            }
                            RoutingDecision::Drop => {
                                // No route found, drop packet
                                debug!(len, "TUN RX: dropped (no route)");
                            }
                        }
                    }

                    // Return RX buffer
                    self.rx_queue.push_used(chain_id, result as u32);

                    // Resubmit RX read (unless shutting down)
                    if !shutdown_requested && let Some(new_chain) = self.rx_queue.pop_available() {
                        let read_e = opcode::ReadFixed::new(
                            tun_fd,
                            new_chain.buffer.ptr,
                            new_chain.buffer.len,
                            new_chain.buffer.buf_index,
                        )
                        .build()
                        .user_data(new_chain.chain_id | USER_DATA_RX_FLAG);

                        unsafe {
                            if ring.submission().push(&read_e).is_ok() {
                                rx_in_flight.insert(new_chain.chain_id, new_chain);
                            }
                        }
                    }
                } else {
                    // TX completion
                    let chain = match tx_in_flight.remove(&chain_id) {
                        Some(c) => c,
                        None => {
                            error!(chain_id, "Unknown TX chain_id");
                            continue;
                        }
                    };

                    if result < 0 {
                        error!(error = -result, chain_id, "TX write error");
                    }

                    // Return TX buffer
                    self.tx_queue.push_used(chain.chain_id);
                }
            }

            // Signal all reactors that received packets this batch (batched signaling)
            if let Some(ref registry) = self.registry {
                for reactor_id in &reactors_to_signal {
                    registry.signal_reactor(reactor_id);
                }
            }

            // Submit pending TX packets
            for tx_packet in pending_tx.drain(..) {
                let write_e = opcode::WriteFixed::new(
                    tun_fd,
                    tx_packet.chain.buffer.ptr,
                    tx_packet.len,
                    tx_packet.chain.buffer.buf_index,
                )
                .build()
                .user_data(tx_packet.chain.chain_id); // No RX flag = TX

                unsafe {
                    if ring.submission().push(&write_e).is_ok() {
                        tx_in_flight.insert(tx_packet.chain.chain_id, tx_packet.chain);
                    } else {
                        warn!("SQ full, dropping TX packet");
                        self.tx_queue.push_used(tx_packet.chain.chain_id);
                    }
                }
            }

            // Notify queues
            self.rx_queue.notify();
            self.tx_queue.notify();

            // Exit immediately when shutdown is requested
            // In-flight operations will be cancelled when io_uring is dropped
            if shutdown_requested {
                info!("Shutdown requested, exiting reactor loop");
                break;
            }
        }

        // Close the eventfd
        unsafe {
            nix::libc::close(self.event_fd);
        }

        info!("Reactor done");
    }

    /// Convert vhost descriptor chain to iovecs for zero-copy I/O.
    /// Uses fixed-size arrays to avoid heap allocation and ensure memory stability.
    fn desc_chain_to_iovecs(
        desc_chain: &virtio_queue::DescriptorChain<&vm_memory::GuestMemoryMmap>,
        mem: &vm_memory::GuestMemoryMmap,
        keep_alive: Option<Arc<dyn std::any::Any + Send + Sync>>,
    ) -> Option<VhostTxInFlight> {
        let head_index = desc_chain.head_index();
        let mut iovecs = [libc::iovec {
            iov_base: std::ptr::null_mut(),
            iov_len: 0,
        }; MAX_TX_IOVECS];
        let mut iovecs_len = 0usize;
        let mut total_len = 0u32;

        for desc in desc_chain.clone() {
            if desc.is_write_only() {
                continue; // TX: skip write-only descriptors
            }
            if iovecs_len >= MAX_TX_IOVECS {
                warn!("Descriptor chain exceeds MAX_TX_IOVECS");
                return None;
            }
            let gpa = desc.addr();
            let len = desc.len();
            total_len += len;

            // Translate GPA → HVA
            let hva = mem.get_host_address(gpa).ok()?;
            iovecs[iovecs_len] = libc::iovec {
                iov_base: hva as *mut libc::c_void,
                iov_len: len as usize,
            };
            iovecs_len += 1;
        }

        if iovecs_len == 0 {
            return None;
        }

        Some(VhostTxInFlight {
            head_index,
            total_len,
            iovecs,
            iovecs_len,
            keep_alive,
            patched_virtio_hdr: [0u8; VIRTIO_NET_HDR_SIZE],
        })
    }

    /// Handle vhost Ethernet frame and check for protocol packets that need local handling.
    ///
    /// For vhost-user, packets are Ethernet frames (virtio_net_hdr + Ethernet).
    /// This function checks for:
    /// - ARP requests (respond for gateway IP)
    /// - DHCP packets (respond with configured IP)
    /// - ICMPv6 NS/RS (respond for gateway)
    /// - DHCPv6 packets (respond with configured IPv6)
    ///
    /// Returns true if the packet was handled locally and a response was injected.
    fn handle_vhost_ethernet_protocols(&self, state: &VhostState, peek_data: &[u8]) -> bool {
        let nic_config = match &self.nic_config {
            Some(cfg) => cfg,
            None => {
                debug!("handle_vhost_ethernet_protocols: no nic_config");
                return false;
            }
        };

        if peek_data.len() <= VIRTIO_NET_HDR_SIZE {
            debug!(
                len = peek_data.len(),
                "handle_vhost_ethernet_protocols: packet too short"
            );
            return false;
        }

        let virtio_hdr = &peek_data[..VIRTIO_NET_HDR_SIZE];
        let ethernet_data = &peek_data[VIRTIO_NET_HDR_SIZE..];

        // Parse Ethernet frame
        let eth_frame = match EthernetFrame::new_checked(ethernet_data) {
            Ok(f) => f,
            Err(e) => {
                debug!(error = ?e, "handle_vhost_ethernet_protocols: invalid Ethernet frame");
                return false;
            }
        };

        debug!(
            ethertype = ?eth_frame.ethertype(),
            src_mac = ?eth_frame.src_addr(),
            dst_mac = ?eth_frame.dst_addr(),
            "handle_vhost_ethernet_protocols: checking packet"
        );

        match eth_frame.ethertype() {
            EthernetProtocol::Arp => {
                // Handle ARP
                if let Some(response) =
                    arp::handle_arp_packet(nic_config, virtio_hdr, ethernet_data)
                {
                    Self::inject_to_vhost_rx(state, &response);
                    return true;
                }
            }
            EthernetProtocol::Ipv4 => {
                // Check for DHCP (UDP port 67)
                if let Some(response) =
                    dhcp::handle_dhcp_packet(nic_config, virtio_hdr, ethernet_data)
                {
                    Self::inject_to_vhost_rx(state, &response);
                    return true;
                }
                // For ICMP echo to link-local gateway, handle it
                if let Ok(ipv4) = Ipv4Packet::new_checked(eth_frame.payload()) {
                    let dst = Ipv4Addr::from(ipv4.dst_addr().0);
                    if dst == GATEWAY_IPV4_LINK_LOCAL
                        && ipv4.next_header() == IpProtocol::Icmp
                        && let Ok(icmp) = Icmpv4Packet::new_checked(ipv4.payload())
                        && icmp.msg_type() == Icmpv4Message::EchoRequest
                    {
                        debug!(
                            src = %ipv4.src_addr(),
                            dst = %ipv4.dst_addr(),
                            "ICMP Echo Request for gateway"
                        );
                        Self::handle_vhost_ethernet_icmp_request(
                            state, nic_config, virtio_hdr, &eth_frame, &ipv4, &icmp,
                        );
                        return true;
                    }
                }
            }
            EthernetProtocol::Ipv6 => {
                // Check for ICMPv6 (NS/RS)
                if let Some(response) =
                    icmpv6::handle_icmpv6_packet(nic_config, virtio_hdr, ethernet_data)
                {
                    Self::inject_to_vhost_rx(state, &response);
                    return true;
                }
                // Check for DHCPv6
                if let Some(response) =
                    dhcpv6::handle_dhcpv6_packet(nic_config, virtio_hdr, ethernet_data)
                {
                    Self::inject_to_vhost_rx(state, &response);
                    return true;
                }
            }
            _ => {}
        }

        false
    }

    /// Handle ICMP echo request in Ethernet frame format and inject reply.
    fn handle_vhost_ethernet_icmp_request(
        state: &VhostState,
        _nic_config: &NicConfig,
        virtio_hdr: &[u8],
        eth_frame: &EthernetFrame<&[u8]>,
        ipv4: &Ipv4Packet<&[u8]>,
        icmp: &Icmpv4Packet<&[u8]>,
    ) {
        let echo_data = icmp.data();

        let icmp_repr = Icmpv4Repr::EchoReply {
            ident: icmp.echo_ident(),
            seq_no: icmp.echo_seq_no(),
            data: echo_data,
        };

        let ip_repr = Ipv4Repr {
            src_addr: ipv4.dst_addr(),
            dst_addr: ipv4.src_addr(),
            next_header: IpProtocol::Icmp,
            payload_len: icmp_repr.buffer_len(),
            hop_limit: 64,
        };

        // Build Ethernet + IP + ICMP reply
        const ETHERNET_HEADER_SIZE: usize = 14;
        let ip_len = ip_repr.buffer_len() + icmp_repr.buffer_len();
        let total_len = virtio_hdr.len() + ETHERNET_HEADER_SIZE + ip_len;

        let mut reply = vec![0u8; total_len];

        // Virtio header (zeroed)
        reply[..virtio_hdr.len()].fill(0);

        // Ethernet header
        let gateway_mac = EthernetAddress(GATEWAY_MAC);
        let eth_repr = EthernetRepr {
            src_addr: gateway_mac,
            dst_addr: eth_frame.src_addr(),
            ethertype: EthernetProtocol::Ipv4,
        };
        let mut out_eth = EthernetFrame::new_unchecked(&mut reply[virtio_hdr.len()..]);
        eth_repr.emit(&mut out_eth);

        // IP header
        let mut out_ip = Ipv4Packet::new_unchecked(out_eth.payload_mut());
        ip_repr.emit(&mut out_ip, &smoltcp::phy::ChecksumCapabilities::default());

        // ICMP
        let mut out_icmp = Icmpv4Packet::new_unchecked(out_ip.payload_mut());
        icmp_repr.emit(
            &mut out_icmp,
            &smoltcp::phy::ChecksumCapabilities::default(),
        );

        debug!(
            dst = %ipv4.src_addr(),
            id = icmp.echo_ident(),
            seq = icmp.echo_seq_no(),
            "ICMP Echo Reply (Ethernet)"
        );

        Self::inject_to_vhost_rx(state, &reply);
    }

    /// Parse Ethernet frame and route based on IP destination.
    ///
    /// For vhost-user packets, we have: virtio_net_hdr + Ethernet frame.
    /// This extracts the IP destination from the Ethernet payload.
    fn peek_and_route_ethernet(&self, peek_data: &[u8]) -> RoutingDecision {
        if peek_data.len() <= VIRTIO_NET_HDR_SIZE {
            debug!("peek_and_route_ethernet: packet too short");
            return RoutingDecision::Drop;
        }

        let ethernet_data = &peek_data[VIRTIO_NET_HDR_SIZE..];

        // Parse Ethernet frame
        let eth_frame = match EthernetFrame::new_checked(ethernet_data) {
            Ok(f) => f,
            Err(_) => {
                debug!("peek_and_route_ethernet: invalid Ethernet frame");
                return RoutingDecision::Drop;
            }
        };

        match eth_frame.ethertype() {
            EthernetProtocol::Ipv4 => self.route_ipv4(eth_frame.payload()),
            EthernetProtocol::Ipv6 => self.route_ipv6(eth_frame.payload()),
            EthernetProtocol::Arp => {
                // ARP is handled by protocol handlers before routing, drop here
                debug!("peek_and_route_ethernet: ARP packet (handled by protocol handlers)");
                RoutingDecision::Drop
            }
            _ => {
                debug!(ethertype = ?eth_frame.ethertype(), "peek_and_route_ethernet: unknown ethertype");
                RoutingDecision::Drop
            }
        }
    }

    /// Process packets from vhost TX queue (guest → network)
    fn process_vhost_tx(
        &mut self,
        state: &VhostState,
        ring: &mut IoUring,
        tun_fd: types::Fd,
        vhost_tx_in_flight: &mut std::collections::HashMap<u64, Box<VhostTxInFlight>>,
        vhost_tx_id: &mut u64,
        vhost_to_vhost_in_flight: &mut std::collections::HashMap<u64, VhostToVhostInFlight>,
    ) {
        let mem_guard = state.mem.memory();
        let tx_vring = &state.vrings[VHOST_TX_QUEUE];

        let mut vring_state = tx_vring.get_mut();

        loop {
            let mut descriptors_returned = false;

            // Create keep_alive reference to prevent guest memory from being unmapped
            // while packets are in flight (in io_uring or inter-reactor channels)
            let keep_alive: Option<Arc<dyn std::any::Any + Send + Sync>> =
                Some(Arc::new(state.mem.clone()));

            let queue = vring_state.get_queue_mut();
            while let Some(desc_chain) = queue.pop_descriptor_chain(&*mem_guard) {
                let Some(in_flight) =
                    Self::desc_chain_to_iovecs(&desc_chain, &mem_guard, keep_alive.clone())
                else {
                    // Empty chain - return immediately
                    let _ = queue.add_used(&*mem_guard, desc_chain.head_index(), 0);
                    descriptors_returned = true;
                    continue;
                };

                debug!(
                    len = in_flight.total_len,
                    iovecs = in_flight.iovecs_len,
                    "vhost TX processing"
                );

                // Peek at packet headers
                let peek_slice = Self::peek_packet_headers(&in_flight);

                // First, try to handle protocol packets locally (ARP, DHCP, ICMPv6, DHCPv6)
                // These need responses injected back to the VM
                if self.handle_vhost_ethernet_protocols(state, &peek_slice) {
                    // Protocol handler consumed the packet and injected a response
                    let _ = queue.add_used(&*mem_guard, in_flight.head_index, in_flight.total_len);
                    descriptors_returned = true;
                    continue;
                }

                // Route the packet using Ethernet-aware routing
                let routing_decision = self.peek_and_route_ethernet(&peek_slice);

                match routing_decision {
                    RoutingDecision::ToTun { if_index: _ } => {
                        // Route to TUN - strip Ethernet header for L3 TUN device
                        // Box to ensure stable memory address for io_uring
                        let mut boxed = Box::new(in_flight);

                        // Save original iovecs before prepare_tun_iovecs modifies them
                        let src_iovecs = boxed.iovecs;
                        let src_len = boxed.iovecs_len;

                        // Prepare TUN iovecs: copy+patch virtio header, skip Ethernet
                        if !prepare_tun_iovecs(&src_iovecs, src_len, &mut boxed) {
                            warn!("Packet too short for Ethernet stripping");
                            let _ = queue.add_used(&*mem_guard, boxed.head_index, 0);
                            descriptors_returned = true;
                            continue;
                        }

                        let user_data = *vhost_tx_id | USER_DATA_VHOST_TX_FLAG;
                        *vhost_tx_id = vhost_tx_id.wrapping_add(1);

                        let writev = opcode::Writev::new(
                            tun_fd,
                            boxed.iovecs.as_ptr(),
                            boxed.iovecs_len as u32,
                        )
                        .build()
                        .user_data(user_data);

                        unsafe {
                            if ring.submission().push(&writev).is_ok() {
                                debug!(len = boxed.total_len, "vhost TX -> TUN (zero-copy)");
                                vhost_tx_in_flight.insert(user_data, boxed);
                            } else {
                                warn!("SQ full, dropping vhost TX");
                                let _ = queue.add_used(&*mem_guard, boxed.head_index, 0);
                                descriptors_returned = true;
                            }
                        }
                    }

                    RoutingDecision::ToVhost {
                        reactor_id: target_reactor_id,
                    } => {
                        // Route to another VM (or TUN reactor) - send via registry
                        // Clone registry to avoid borrow conflicts
                        let registry_opt = self.registry.clone();
                        if let Some(registry) = registry_opt {
                            // Lookup destination VM's MAC address for header rewriting
                            let dst_mac = registry
                                .get_mac_for_reactor(&target_reactor_id)
                                .unwrap_or([0xff; 6]); // Fallback: broadcast

                            let packet_id = self.next_packet_id();
                            let source = PacketSource::VhostToVhost {
                                head_index: in_flight.head_index,
                                total_len: in_flight.total_len,
                                source_reactor: self.reactor_id,
                                dst_mac,
                                src_mac: GATEWAY_MAC,
                            };

                            let packet = PacketRef::new(
                                packet_id,
                                in_flight.iovecs[..in_flight.iovecs_len].to_vec(),
                                source,
                                in_flight.keep_alive.clone(),
                            );

                            debug!(
                                packet_id = %packet_id,
                                len = in_flight.total_len,
                                src = %self.reactor_id,
                                dst = %target_reactor_id,
                                head_idx = in_flight.head_index,
                                "Sending packet to target reactor"
                            );

                            if registry.send_packet_to(&target_reactor_id, packet) {
                                debug!(
                                    len = in_flight.total_len,
                                    dst = %target_reactor_id,
                                    "vhost TX -> vhost (VM-to-VM)"
                                );
                                // Track in-flight - don't return descriptor yet
                                vhost_to_vhost_in_flight.insert(
                                    packet_id.raw(),
                                    VhostToVhostInFlight {
                                        head_index: in_flight.head_index,
                                        total_len: in_flight.total_len,
                                    },
                                );
                            } else {
                                warn!(dst = %target_reactor_id, "Failed to send to target reactor");
                                let _ = queue.add_used(&*mem_guard, in_flight.head_index, 0);
                                descriptors_returned = true;
                            }
                        } else {
                            warn!("No registry configured for VM-to-VM routing");
                            let _ = queue.add_used(&*mem_guard, in_flight.head_index, 0);
                            descriptors_returned = true;
                        }
                    }

                    RoutingDecision::Drop => {
                        // Drop packet - return descriptor immediately
                        debug!(len = in_flight.total_len, "vhost TX dropped (no route)");
                        let _ = queue.add_used(&*mem_guard, in_flight.head_index, 0);
                        descriptors_returned = true;
                    }
                }
            }

            // Only signal used queue if we actually returned descriptors
            if descriptors_returned {
                let _ = vring_state.signal_used_queue();
            }

            // Re-enable notifications for event_idx mode.
            // Critical: check if more work arrived while we were processing.
            // enable_notification() returns Ok(true) if descriptors are pending.
            match vring_state.enable_notification() {
                Ok(true) => continue, // More descriptors pending, process them
                _ => break,           // Queue empty or error, safe to sleep
            }
        }
    }

    /// Peek at packet headers from iovecs, returning a slice for routing decisions
    fn peek_packet_headers(in_flight: &VhostTxInFlight) -> Vec<u8> {
        let first_iov_len = if in_flight.iovecs_len > 0 {
            in_flight.iovecs[0].iov_len
        } else {
            0
        };

        if first_iov_len >= PEEK_BUF_SIZE {
            // Fast path: direct slice from first iovec
            unsafe {
                std::slice::from_raw_parts(in_flight.iovecs[0].iov_base as *const u8, PEEK_BUF_SIZE)
            }
            .to_vec()
        } else if in_flight.total_len as usize > VIRTIO_NET_HDR_SIZE {
            // Slow path: headers may span iovecs, copy to buffer
            let mut peek_buf = vec![0u8; PEEK_BUF_SIZE];
            let mut copied = 0usize;
            for iov in in_flight.iovecs.iter().take(in_flight.iovecs_len) {
                if copied >= PEEK_BUF_SIZE {
                    break;
                }
                let src =
                    unsafe { std::slice::from_raw_parts(iov.iov_base as *const u8, iov.iov_len) };
                let to_copy = (PEEK_BUF_SIZE - copied).min(src.len());
                peek_buf[copied..copied + to_copy].copy_from_slice(&src[..to_copy]);
                copied += to_copy;
            }
            peek_buf.truncate(copied);
            peek_buf
        } else {
            Vec::new()
        }
    }

    /// Copy packet data from source iovecs to vhost RX queue.
    ///
    /// This is used for VM-to-VM routing where we need to copy from
    /// the source VM's guest memory to the destination VM's RX queue.
    ///
    /// For TunRx packets (zero-copy path), this function performs Ethernet header
    /// injection using a 3-phase scatter-gather write:
    /// - Phase 0: Copy VirtioHdr (12 bytes) from source
    /// - Phase 1: Write EthHdr (14 bytes) constructed on stack
    /// - Phase 2: Copy IP Payload from source (offset 12+)
    fn copy_to_vhost_rx(state: &VhostState, packet: &PacketRef) -> i32 {
        let mem_guard = state.mem.memory();
        let rx_vring = &state.vrings[VHOST_RX_QUEUE];

        let mut vring_state = rx_vring.get_mut();
        let queue = vring_state.get_queue_mut();

        // Check if this is a TunRx packet needing Ethernet header injection
        let tun_rx_info = match &packet.source {
            PacketSource::TunRx {
                dst_mac, ethertype, ..
            } => Some((*dst_mac, *ethertype)),
            _ => None,
        };

        // Check if this is a VM-to-VM packet needing MAC rewriting
        let vhost_to_vhost_macs = match &packet.source {
            PacketSource::VhostToVhost {
                dst_mac, src_mac, ..
            } => Some((*dst_mac, *src_mac)),
            _ => None,
        };

        // Track descriptor chains used for mergeable RX buffers
        let mut chains_used: Vec<(u16, u32)> = Vec::new();
        let mut first_hdr_addr: Option<vm_memory::GuestAddress> = None;

        // Helper to return all chains with len=0 on error
        let return_chains_on_error =
            |queue: &mut virtio_queue::Queue,
             mem: &vm_memory::GuestMemoryMmap,
             chains: &[(u16, u32)],
             current_head: Option<u16>| {
                for (head_idx, _) in chains {
                    let _ = queue.add_used(mem, *head_idx, 0);
                }
                if let Some(head) = current_head {
                    let _ = queue.add_used(mem, head, 0);
                }
            };

        let written = if let Some((dst_mac, ethertype)) = tun_rx_info {
            // Scatter-gather write for TunRx (zero-copy with Ethernet header injection):
            // Source: [VirtioHdr(12)][IP Payload]
            // Dest:   [VirtioHdr(12)][EthHdr(14)][IP Payload]

            // Build Ethernet header on stack (14 bytes)
            let mut eth_hdr = [0u8; ETHERNET_HDR_SIZE];
            eth_hdr[0..6].copy_from_slice(&dst_mac);
            eth_hdr[6..12].copy_from_slice(&GATEWAY_MAC);
            eth_hdr[12..14].copy_from_slice(&ethertype.to_be_bytes());

            // Calculate total source length
            let total_src_len: usize = packet.iovecs.iter().map(|iov| iov.iov_len).sum();

            // Total output length = source + Ethernet header
            let total_out_len = total_src_len + ETHERNET_HDR_SIZE;

            // 3-phase write: virtio_hdr, eth_hdr, ip_payload
            // Phase 0: VirtioHdr (bytes 0..12 from source)
            // Phase 1: EthHdr (14 bytes from stack)
            // Phase 2: IP Payload (bytes 12+ from source)
            let mut written = 0usize;
            let mut phase = 0u8; // 0=virtio_hdr, 1=eth_hdr, 2=ip_payload
            let mut phase_offset = 0usize;

            // Pre-read virtio header and patch csum_start for Ethernet injection
            let mut virtio_hdr_buf = [0u8; VIRTIO_NET_HDR_SIZE];
            {
                let mut tmp_idx = 0usize;
                let mut tmp_off = 0usize;
                let hdr_data = Self::read_from_iovecs(
                    &packet.iovecs,
                    &mut tmp_idx,
                    &mut tmp_off,
                    VIRTIO_NET_HDR_SIZE,
                );
                virtio_hdr_buf[..hdr_data.len()].copy_from_slice(&hdr_data);
            }

            // Adjust csum_start to account for injected Ethernet header
            patch_virtio_hdr_for_eth_injection(&mut virtio_hdr_buf);

            // Source tracking - start past virtio header since we read it into local buffer
            let mut src_iov_idx = 0usize;
            let mut src_iov_offset = 0usize;
            // Skip past the virtio header in source iovecs
            Self::skip_in_iovecs(
                &packet.iovecs,
                &mut src_iov_idx,
                &mut src_iov_offset,
                VIRTIO_NET_HDR_SIZE,
            );

            // Pop descriptor chains until all packet data is written
            while written < total_out_len {
                let Some(desc_chain) = queue.pop_descriptor_chain(&*mem_guard) else {
                    if chains_used.is_empty() {
                        debug!("No RX buffer available for TunRx packet");
                        return -libc::ENOSPC;
                    }
                    // Ran out of chains mid-packet
                    debug!(
                        written,
                        total_out_len, "Ran out of RX buffers mid-packet (TunRx)"
                    );
                    return_chains_on_error(queue, &mem_guard, &chains_used, None);
                    let _ = vring_state.signal_used_queue();
                    return -libc::ENOSPC;
                };

                let head_index = desc_chain.head_index();
                let chain_start_written = written;

                for desc in desc_chain {
                    if !desc.is_write_only() {
                        continue;
                    }

                    // Track first header address for num_buffers patching
                    if first_hdr_addr.is_none() {
                        first_hdr_addr = Some(desc.addr());
                    }

                    let available = desc.len() as usize;
                    let mut desc_offset = 0usize;

                    while desc_offset < available && written < total_out_len {
                        let to_copy = match phase {
                            0 => {
                                // Phase 0: VirtioHdr from patched local buffer
                                let remaining_in_phase = VIRTIO_NET_HDR_SIZE - phase_offset;
                                let remaining_in_desc = available - desc_offset;
                                let to_copy = remaining_in_phase.min(remaining_in_desc);

                                if to_copy > 0 {
                                    let src_slice =
                                        &virtio_hdr_buf[phase_offset..phase_offset + to_copy];
                                    let dst_addr = desc
                                        .addr()
                                        .checked_add(desc_offset as u64)
                                        .expect("Address overflow");

                                    if let Err(e) = mem_guard.write_slice(src_slice, dst_addr) {
                                        warn!(?e, "Failed to write virtio_hdr to vhost RX buffer");
                                        return_chains_on_error(
                                            queue,
                                            &mem_guard,
                                            &chains_used,
                                            Some(head_index),
                                        );
                                        let _ = vring_state.signal_used_queue();
                                        return -libc::EIO;
                                    }
                                }
                                to_copy
                            }
                            1 => {
                                // Phase 1: EthHdr from stack
                                let remaining_in_phase = ETHERNET_HDR_SIZE - phase_offset;
                                let remaining_in_desc = available - desc_offset;
                                let to_copy = remaining_in_phase.min(remaining_in_desc);

                                if to_copy > 0 {
                                    let src_slice = &eth_hdr[phase_offset..phase_offset + to_copy];
                                    let dst_addr = desc
                                        .addr()
                                        .checked_add(desc_offset as u64)
                                        .expect("Address overflow");

                                    if let Err(e) = mem_guard.write_slice(src_slice, dst_addr) {
                                        warn!(?e, "Failed to write eth_hdr to vhost RX buffer");
                                        return_chains_on_error(
                                            queue,
                                            &mem_guard,
                                            &chains_used,
                                            Some(head_index),
                                        );
                                        let _ = vring_state.signal_used_queue();
                                        return -libc::EIO;
                                    }
                                }
                                to_copy
                            }
                            2 => {
                                // Phase 2: IP Payload from source (skip virtio_hdr)
                                let ip_payload_len =
                                    total_src_len.saturating_sub(VIRTIO_NET_HDR_SIZE);
                                let remaining_in_phase = ip_payload_len - phase_offset;
                                let remaining_in_desc = available - desc_offset;
                                let to_copy = remaining_in_phase.min(remaining_in_desc);

                                if to_copy > 0 {
                                    // Read from source iovecs (already past virtio_hdr)
                                    let src_data = Self::read_from_iovecs(
                                        &packet.iovecs,
                                        &mut src_iov_idx,
                                        &mut src_iov_offset,
                                        to_copy,
                                    );

                                    let dst_addr = desc
                                        .addr()
                                        .checked_add(desc_offset as u64)
                                        .expect("Address overflow");

                                    if let Err(e) = mem_guard.write_slice(&src_data, dst_addr) {
                                        warn!(?e, "Failed to write ip_payload to vhost RX buffer");
                                        return_chains_on_error(
                                            queue,
                                            &mem_guard,
                                            &chains_used,
                                            Some(head_index),
                                        );
                                        let _ = vring_state.signal_used_queue();
                                        return -libc::EIO;
                                    }
                                }
                                to_copy
                            }
                            _ => break,
                        };

                        desc_offset += to_copy;
                        phase_offset += to_copy;
                        written += to_copy;

                        // Check if phase complete, advance to next
                        let phase_len = match phase {
                            0 => VIRTIO_NET_HDR_SIZE,
                            1 => ETHERNET_HDR_SIZE,
                            2 => total_src_len.saturating_sub(VIRTIO_NET_HDR_SIZE),
                            _ => 0,
                        };

                        if phase_offset >= phase_len {
                            phase += 1;
                            phase_offset = 0;
                        }
                    }

                    if written >= total_out_len {
                        break;
                    }
                }

                let chain_bytes = (written - chain_start_written) as u32;
                chains_used.push((head_index, chain_bytes));
            }

            debug!(
                src_len = total_src_len,
                out_len = written,
                num_buffers = chains_used.len(),
                "TunRx: copied with Ethernet header injection"
            );
            written
        } else {
            // Standard VM-to-VM copy (no Ethernet header injection)
            let mut written = 0usize;
            let mut src_iov_idx = 0usize;
            let mut src_iov_offset = 0usize;

            // Calculate total packet length
            let total_packet_len: usize = packet.iovecs.iter().map(|iov| iov.iov_len).sum();

            // Pop descriptor chains until all packet data is written
            while written < total_packet_len {
                let Some(desc_chain) = queue.pop_descriptor_chain(&*mem_guard) else {
                    if chains_used.is_empty() {
                        debug!("No RX buffer available for VM-to-VM packet");
                        return -libc::ENOSPC;
                    }
                    // Ran out of chains mid-packet
                    debug!(
                        written,
                        total_packet_len, "Ran out of RX buffers mid-packet (VM-to-VM)"
                    );
                    return_chains_on_error(queue, &mem_guard, &chains_used, None);
                    let _ = vring_state.signal_used_queue();
                    return -libc::ENOSPC;
                };

                let head_index = desc_chain.head_index();
                let chain_start_written = written;

                // Copy from source iovecs to destination descriptors
                for desc in desc_chain {
                    if !desc.is_write_only() {
                        continue;
                    }

                    // Track first header address for num_buffers patching
                    if first_hdr_addr.is_none() {
                        first_hdr_addr = Some(desc.addr());
                    }

                    let available = desc.len() as usize;
                    let mut desc_offset = 0usize;

                    while desc_offset < available && written < total_packet_len {
                        if src_iov_idx >= packet.iovecs.len() {
                            break;
                        }

                        let src_iov = &packet.iovecs[src_iov_idx];
                        let src_remaining = src_iov.iov_len - src_iov_offset;
                        let dst_remaining = available - desc_offset;
                        let to_copy = src_remaining.min(dst_remaining);

                        if to_copy > 0 {
                            // Copy from source iovec to destination descriptor
                            let src_ptr =
                                unsafe { (src_iov.iov_base as *const u8).add(src_iov_offset) };
                            let src_slice = unsafe { std::slice::from_raw_parts(src_ptr, to_copy) };

                            let dst_addr = desc
                                .addr()
                                .checked_add(desc_offset as u64)
                                .expect("Address overflow");

                            if let Err(e) = mem_guard.write_slice(src_slice, dst_addr) {
                                warn!(?e, "Failed to write to vhost RX buffer");
                                return_chains_on_error(
                                    queue,
                                    &mem_guard,
                                    &chains_used,
                                    Some(head_index),
                                );
                                let _ = vring_state.signal_used_queue();
                                return -libc::EIO;
                            }

                            desc_offset += to_copy;
                            src_iov_offset += to_copy;
                            written += to_copy;

                            // Move to next source iovec if exhausted
                            if src_iov_offset >= src_iov.iov_len {
                                src_iov_idx += 1;
                                src_iov_offset = 0;
                            }
                        }
                    }

                    if written >= total_packet_len {
                        break;
                    }
                }

                let chain_bytes = (written - chain_start_written) as u32;
                chains_used.push((head_index, chain_bytes));
            }

            // Patch MAC addresses in Ethernet header for VM-to-VM packets
            // Ethernet header starts at offset VIRTIO_NET_HDR_SIZE (12 bytes)
            // dst_mac: bytes 0-5, src_mac: bytes 6-11
            if let (Some((dst_mac, src_mac)), Some(hdr_addr)) =
                (vhost_to_vhost_macs, first_hdr_addr)
            {
                let eth_addr = hdr_addr
                    .checked_add(VIRTIO_NET_HDR_SIZE as u64)
                    .expect("Address overflow");
                // Write dst_mac (bytes 0-5)
                let _ = mem_guard.write_slice(&dst_mac, eth_addr);
                // Write src_mac (bytes 6-11)
                if let Some(src_addr) = eth_addr.checked_add(6) {
                    let _ = mem_guard.write_slice(&src_mac, src_addr);
                }
                debug!(
                    dst_mac = ?dst_mac,
                    src_mac = ?src_mac,
                    "VM-to-VM: patched Ethernet MAC addresses"
                );
            }

            debug!(
                len = written,
                num_buffers = chains_used.len(),
                "VM-to-VM: copied packet to vhost RX queue"
            );
            written
        };

        // Patch num_buffers in the first virtio_net_hdr
        let num_buffers = chains_used.len() as u16;
        if let Some(hdr_addr) = first_hdr_addr {
            patch_num_buffers(&*mem_guard, hdr_addr, num_buffers);
        }

        // Return all descriptor chains with their lengths
        for (head_idx, len) in &chains_used {
            if let Err(e) = queue.add_used(&*mem_guard, *head_idx, *len) {
                warn!(?e, head_idx, "Failed to add used descriptor to vhost RX");
            }
        }

        written as i32
    }

    /// Helper function to read data from iovecs, advancing position.
    fn read_from_iovecs(
        iovecs: &[libc::iovec],
        iov_idx: &mut usize,
        iov_offset: &mut usize,
        mut len: usize,
    ) -> Vec<u8> {
        let mut result = Vec::with_capacity(len);

        while len > 0 && *iov_idx < iovecs.len() {
            let iov = &iovecs[*iov_idx];
            let remaining = iov.iov_len - *iov_offset;
            let to_copy = remaining.min(len);

            if to_copy > 0 {
                let src_ptr = unsafe { (iov.iov_base as *const u8).add(*iov_offset) };
                let src_slice = unsafe { std::slice::from_raw_parts(src_ptr, to_copy) };
                result.extend_from_slice(src_slice);

                *iov_offset += to_copy;
                len -= to_copy;
            }

            if *iov_offset >= iov.iov_len {
                *iov_idx += 1;
                *iov_offset = 0;
            }
        }

        result
    }

    /// Skip bytes in iovec array without copying (advance position only).
    fn skip_in_iovecs(
        iovecs: &[libc::iovec],
        iov_idx: &mut usize,
        iov_offset: &mut usize,
        mut len: usize,
    ) {
        while len > 0 && *iov_idx < iovecs.len() {
            let iov = &iovecs[*iov_idx];
            let remaining = iov.iov_len - *iov_offset;
            let to_skip = remaining.min(len);

            *iov_offset += to_skip;
            len -= to_skip;

            if *iov_offset >= iov.iov_len {
                *iov_idx += 1;
                *iov_offset = 0;
            }
        }
    }

    /// Process incoming packets from other reactors (VM-to-VM receive path).
    ///
    /// Receives PacketRefs from other reactors via the packet_rx channel,
    /// copies the data to the local VM's RX queue, and sends CompletionNotify
    /// back to the source reactor.
    fn process_incoming_packets(
        &self,
        state: &VhostState,
        _vhost_to_vhost_in_flight: &mut std::collections::HashMap<u64, VhostToVhostInFlight>,
    ) {
        let Some(ref packet_rx) = self.packet_rx else {
            debug!("process_incoming_packets: no packet_rx channel");
            return;
        };

        // Process all pending packets from the channel
        let mut signal_needed = false;

        while let Ok(packet) = packet_rx.try_recv() {
            debug!(
                id = %packet.id,
                len = packet.total_len(),
                src = %packet.source.source_reactor(),
                "Processing incoming VM-to-VM packet"
            );

            // Copy packet to local RX queue
            let result = Self::copy_to_vhost_rx(state, &packet);
            if result >= 0 {
                signal_needed = true;
            }

            // Send completion notification back to source reactor
            let completion = match &packet.source {
                PacketSource::VhostToVhost {
                    head_index,
                    total_len,
                    ..
                } => CompletionNotify::VhostToVhostComplete {
                    packet_id: packet.id,
                    head_index: *head_index,
                    total_len: *total_len,
                    result,
                },
                PacketSource::TunRx { chain_id, .. } => CompletionNotify::TunRxComplete {
                    packet_id: packet.id,
                    chain_id: *chain_id,
                    result,
                },
                PacketSource::VhostTx {
                    head_index,
                    total_len,
                    source_reactor: _,
                } => CompletionNotify::VhostTxComplete {
                    packet_id: packet.id,
                    head_index: *head_index,
                    total_len: *total_len,
                    result,
                },
            };

            if let Some(ref registry) = self.registry {
                let source_reactor = packet.source.source_reactor();
                if !registry.send_completion_to(&source_reactor, completion) {
                    warn!(src = %source_reactor, "Failed to send completion to source reactor");
                }
            }
        }

        // Signal guest once for entire batch
        if signal_needed
            && let Some(vring_state) = state.vrings.get(VHOST_RX_QUEUE)
            && let Err(e) = vring_state.signal_used_queue()
        {
            warn!(?e, "Failed to signal vhost RX used queue");
        }
    }

    /// Process incoming packets from other reactors for TUN-only reactors.
    ///
    /// For TUN-only reactors (no vhost), packets from other reactors should be
    /// written to the TUN device (with Ethernet header stripped since TUN is L3).
    fn process_incoming_packets_to_tun(
        &mut self,
        ring: &mut IoUring,
        tun_fd: types::Fd,
        incoming_to_tun_in_flight: &mut std::collections::HashMap<u64, IncomingToTunInFlight>,
        incoming_tun_id: &mut u64,
    ) {
        let Some(ref packet_rx) = self.packet_rx else {
            return;
        };

        while let Ok(packet) = packet_rx.try_recv() {
            debug!(
                id = %packet.id,
                len = packet.total_len(),
                src = %packet.source.source_reactor(),
                "TUN reactor: received packet for L3 forwarding"
            );

            // Read packet data from iovecs
            let total_len = packet.total_len();
            let mut packet_data = Vec::with_capacity(total_len);

            for iov in &packet.iovecs {
                let slice =
                    unsafe { std::slice::from_raw_parts(iov.iov_base as *const u8, iov.iov_len) };
                packet_data.extend_from_slice(slice);
            }

            // Strip Ethernet header: [virtio_net_hdr (12)][Ethernet (14)][IP...] -> [virtio_net_hdr (12)][IP...]
            // Keep virtio_net_hdr (first 12 bytes), skip Ethernet header (next 14 bytes)
            if packet_data.len() < VIRTIO_NET_HDR_SIZE + ETHERNET_HDR_SIZE {
                warn!(
                    len = packet_data.len(),
                    "Packet too short for Ethernet stripping"
                );
                // Send failure completion
                self.send_incoming_completion(&packet, -libc::EINVAL);
                continue;
            }

            // Build L3 packet: virtio_net_hdr + IP packet (skip Ethernet)
            let mut l3_packet = Vec::with_capacity(packet_data.len() - ETHERNET_HDR_SIZE);
            l3_packet.extend_from_slice(&packet_data[..VIRTIO_NET_HDR_SIZE]); // virtio_net_hdr
            l3_packet.extend_from_slice(&packet_data[VIRTIO_NET_HDR_SIZE + ETHERNET_HDR_SIZE..]); // IP packet

            if let Ok(hdr_array_ref) = (&mut l3_packet[..VIRTIO_NET_HDR_SIZE]).try_into() {
                patch_virtio_hdr_for_eth_stripping(hdr_array_ref);
            }

            // Generate unique user_data for this write
            let user_data = USER_DATA_INCOMING_TUN_FLAG | *incoming_tun_id;
            *incoming_tun_id = incoming_tun_id.wrapping_add(1);

            // Queue write to TUN fd
            let write_op = opcode::Write::new(tun_fd, l3_packet.as_ptr(), l3_packet.len() as u32)
                .build()
                .user_data(user_data);

            match unsafe { ring.submission().push(&write_op) } {
                Ok(()) => {
                    debug!(
                        len = l3_packet.len(),
                        user_data, "Queued incoming packet write to TUN"
                    );
                    // Track in-flight for completion handling
                    incoming_to_tun_in_flight.insert(
                        user_data,
                        IncomingToTunInFlight {
                            packet_id: packet.id,
                            source: packet.source.clone(),
                            buffer: l3_packet, // Keep buffer alive during async I/O
                        },
                    );
                }
                Err(_) => {
                    warn!("SQ full, dropping incoming packet to TUN");
                    self.send_incoming_completion(&packet, -libc::ENOSPC);
                }
            }
        }
    }

    /// Send a completion notification for an incoming packet.
    fn send_incoming_completion(&self, packet: &PacketRef, result: i32) {
        let completion = match &packet.source {
            PacketSource::VhostToVhost {
                head_index,
                total_len,
                ..
            } => CompletionNotify::VhostToVhostComplete {
                packet_id: packet.id,
                head_index: *head_index,
                total_len: *total_len,
                result,
            },
            PacketSource::TunRx { chain_id, len, .. } => CompletionNotify::TunRxComplete {
                packet_id: packet.id,
                chain_id: *chain_id,
                result: *len as i32,
            },
            PacketSource::VhostTx {
                head_index,
                total_len,
                source_reactor: _,
            } => CompletionNotify::VhostTxComplete {
                packet_id: packet.id,
                head_index: *head_index,
                total_len: *total_len,
                result,
            },
        };

        if let Some(ref registry) = self.registry {
            let source_reactor = packet.source.source_reactor();
            if !registry.send_completion_to(&source_reactor, completion) {
                warn!(src = %source_reactor, "Failed to send completion to source reactor");
            }
        }
    }

    /// Inject a packet into the vhost RX queue (network → guest)
    fn inject_to_vhost_rx(state: &VhostState, packet: &[u8]) {
        let mem_guard = state.mem.memory();
        let rx_vring = &state.vrings[VHOST_RX_QUEUE];

        let mut vring_state = rx_vring.get_mut();
        let queue = vring_state.get_queue_mut();

        let packet_len = packet.len();
        let mut written = 0usize;

        // Track descriptor chains used for mergeable RX buffers
        let mut chains_used: Vec<(u16, u32)> = Vec::new();
        let mut first_hdr_addr: Option<vm_memory::GuestAddress> = None;

        // Pop descriptor chains until all packet data is written
        while written < packet_len {
            let Some(desc_chain) = queue.pop_descriptor_chain(&*mem_guard) else {
                if chains_used.is_empty() {
                    warn!("No RX buffer available for vhost injection");
                    return;
                }
                // Ran out of chains mid-packet - return all chains with len=0
                debug!(written, packet_len, "Ran out of RX buffers mid-packet");
                for (head_idx, _) in &chains_used {
                    let _ = queue.add_used(&*mem_guard, *head_idx, 0);
                }
                let _ = vring_state.signal_used_queue();
                return;
            };

            let head_index = desc_chain.head_index();
            let chain_start_written = written;

            // Write packet data to this descriptor chain
            for desc in desc_chain {
                if !desc.is_write_only() {
                    continue;
                }

                // Track first header address for num_buffers patching
                if first_hdr_addr.is_none() {
                    first_hdr_addr = Some(desc.addr());
                }

                let available = desc.len() as usize;
                let to_write = std::cmp::min(available, packet_len - written);

                if to_write > 0 {
                    if let Err(e) =
                        mem_guard.write_slice(&packet[written..written + to_write], desc.addr())
                    {
                        warn!(?e, "Failed to write to vhost RX buffer");
                        // Return all chains with len=0
                        for (head_idx, _) in &chains_used {
                            let _ = queue.add_used(&*mem_guard, *head_idx, 0);
                        }
                        let _ = queue.add_used(&*mem_guard, head_index, 0);
                        let _ = vring_state.signal_used_queue();
                        return;
                    }
                    written += to_write;
                }

                if written >= packet_len {
                    break;
                }
            }

            let chain_bytes = (written - chain_start_written) as u32;
            chains_used.push((head_index, chain_bytes));
        }

        // Patch num_buffers in the first virtio_net_hdr
        let num_buffers = chains_used.len() as u16;
        if let Some(hdr_addr) = first_hdr_addr {
            patch_num_buffers(&*mem_guard, hdr_addr, num_buffers);
        }

        // Return all descriptor chains with their lengths
        for (head_idx, len) in &chains_used {
            if let Err(e) = queue.add_used(&*mem_guard, *head_idx, *len) {
                warn!(?e, head_idx, "Failed to add used descriptor to vhost RX");
            }
        }

        // Signal the guest once
        if let Err(e) = vring_state.signal_used_queue() {
            warn!(?e, "Failed to signal vhost RX used queue");
        }

        debug!(
            len = written,
            num_buffers, "Injected packet to vhost RX queue"
        );
    }
}

/// Patch virtio_net_hdr for Ethernet header injection.
///
/// When injecting a 14-byte Ethernet header into L3 packets from TUN,
/// the `csum_start` offset in the virtio header must be adjusted to
/// account for the additional header bytes. Otherwise, the guest OS
/// will fail to locate the TCP/UDP header for checksum verification.
///
/// # Arguments
/// * `virtio_hdr` - Mutable reference to the 12-byte virtio_net_hdr buffer
///
/// # Virtio Header Layout
/// - offset 0: flags (bit 0 = NEEDS_CSUM)
/// - offset 6-7: csum_start (little-endian u16)
#[inline]
fn patch_virtio_hdr_for_eth_injection(virtio_hdr: &mut [u8; VIRTIO_NET_HDR_SIZE]) {
    const VIRTIO_NET_HDR_F_NEEDS_CSUM: u8 = 1;
    if virtio_hdr[0] & VIRTIO_NET_HDR_F_NEEDS_CSUM != 0 {
        let csum_start = u16::from_le_bytes([virtio_hdr[6], virtio_hdr[7]]);
        let adjusted = csum_start.wrapping_add(ETHERNET_HDR_SIZE as u16);
        virtio_hdr[6..8].copy_from_slice(&adjusted.to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test that csum_start is adjusted when NEEDS_CSUM flag is set
    #[test]
    fn test_patch_virtio_hdr_needs_csum() {
        // Create a virtio header with NEEDS_CSUM flag set
        // csum_start = 20 (typical for TCP over IPv4: IP header = 20 bytes)
        let mut hdr = [0u8; VIRTIO_NET_HDR_SIZE];
        hdr[0] = 1; // flags = VIRTIO_NET_HDR_F_NEEDS_CSUM
        hdr[6..8].copy_from_slice(&20u16.to_le_bytes()); // csum_start = 20

        patch_virtio_hdr_for_eth_injection(&mut hdr);

        // csum_start should be adjusted by ETHERNET_HDR_SIZE (14)
        let csum_start = u16::from_le_bytes([hdr[6], hdr[7]]);
        assert_eq!(csum_start, 20 + ETHERNET_HDR_SIZE as u16);
        assert_eq!(csum_start, 34);
    }

    /// Test that csum_start is NOT adjusted when NEEDS_CSUM flag is clear
    #[test]
    fn test_patch_virtio_hdr_no_csum() {
        // Create a virtio header without NEEDS_CSUM flag
        let mut hdr = [0u8; VIRTIO_NET_HDR_SIZE];
        hdr[0] = 0; // flags = 0 (no checksum offload)
        hdr[6..8].copy_from_slice(&20u16.to_le_bytes()); // csum_start = 20

        patch_virtio_hdr_for_eth_injection(&mut hdr);

        // csum_start should remain unchanged
        let csum_start = u16::from_le_bytes([hdr[6], hdr[7]]);
        assert_eq!(csum_start, 20);
    }

    /// Test csum_start adjustment for IPv6 + TCP (csum_start = 40)
    #[test]
    fn test_patch_virtio_hdr_ipv6_tcp() {
        // IPv6 header = 40 bytes, so csum_start = 40 for TCP
        let mut hdr = [0u8; VIRTIO_NET_HDR_SIZE];
        hdr[0] = 1; // flags = VIRTIO_NET_HDR_F_NEEDS_CSUM
        hdr[6..8].copy_from_slice(&40u16.to_le_bytes()); // csum_start = 40

        patch_virtio_hdr_for_eth_injection(&mut hdr);

        let csum_start = u16::from_le_bytes([hdr[6], hdr[7]]);
        assert_eq!(csum_start, 40 + ETHERNET_HDR_SIZE as u16);
        assert_eq!(csum_start, 54);
    }

    /// Test wrapping behavior for edge case (max u16 value)
    #[test]
    fn test_patch_virtio_hdr_wrapping() {
        let mut hdr = [0u8; VIRTIO_NET_HDR_SIZE];
        hdr[0] = 1; // flags = VIRTIO_NET_HDR_F_NEEDS_CSUM
        hdr[6..8].copy_from_slice(&0xFFF0u16.to_le_bytes()); // near max

        patch_virtio_hdr_for_eth_injection(&mut hdr);

        let csum_start = u16::from_le_bytes([hdr[6], hdr[7]]);
        // 0xFFF0 + 14 = 0xFFFE (no wrap)
        assert_eq!(csum_start, 0xFFFE);
    }

    // Tests for patch_virtio_hdr_for_eth_stripping (inverse of injection)

    /// Test csum_start adjustment for Ethernet stripping (typical TCP over IPv4)
    #[test]
    fn test_patch_virtio_hdr_for_eth_stripping() {
        // csum_start = 34 (typical for TCP over IPv4 with Ethernet: 14 Eth + 20 IP)
        let mut hdr = [0u8; VIRTIO_NET_HDR_SIZE];
        hdr[0] = 1; // NEEDS_CSUM
        hdr[6..8].copy_from_slice(&34u16.to_le_bytes());

        patch_virtio_hdr_for_eth_stripping(&mut hdr);

        let csum_start = u16::from_le_bytes([hdr[6], hdr[7]]);
        assert_eq!(csum_start, 20); // 34 - 14 = 20
    }

    /// Test csum_start unchanged when NEEDS_CSUM flag is clear
    #[test]
    fn test_patch_virtio_hdr_for_eth_stripping_no_csum() {
        let mut hdr = [0u8; VIRTIO_NET_HDR_SIZE];
        hdr[0] = 0; // No NEEDS_CSUM
        hdr[6..8].copy_from_slice(&34u16.to_le_bytes());

        patch_virtio_hdr_for_eth_stripping(&mut hdr);

        let csum_start = u16::from_le_bytes([hdr[6], hdr[7]]);
        assert_eq!(csum_start, 34); // Unchanged
    }

    /// Test saturating_sub clamps to 0 for edge case (csum_start < ETHERNET_HDR_SIZE)
    #[test]
    fn test_patch_virtio_hdr_for_eth_stripping_saturating() {
        // Edge case: csum_start < ETHERNET_HDR_SIZE
        let mut hdr = [0u8; VIRTIO_NET_HDR_SIZE];
        hdr[0] = 1;
        hdr[6..8].copy_from_slice(&5u16.to_le_bytes());

        patch_virtio_hdr_for_eth_stripping(&mut hdr);

        let csum_start = u16::from_le_bytes([hdr[6], hdr[7]]);
        assert_eq!(csum_start, 0); // saturating_sub clamps to 0
    }

    /// Test csum_start adjustment for IPv6 + TCP (csum_start = 54 with Ethernet)
    #[test]
    fn test_patch_virtio_hdr_for_eth_stripping_ipv6() {
        // IPv6: 14 Eth + 40 IPv6 = 54, after stripping should be 40
        let mut hdr = [0u8; VIRTIO_NET_HDR_SIZE];
        hdr[0] = 1; // NEEDS_CSUM
        hdr[6..8].copy_from_slice(&54u16.to_le_bytes());

        patch_virtio_hdr_for_eth_stripping(&mut hdr);

        let csum_start = u16::from_le_bytes([hdr[6], hdr[7]]);
        assert_eq!(csum_start, 40); // 54 - 14 = 40
    }
}
