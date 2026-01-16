//! Per-vNIC worker thread management
//!
//! Each vNIC gets its own worker thread that handles:
//! - vhost-user socket listener
//! - Packet processing (ARP, NDP, DHCP, routing)
//! - RX injection for routed packets from other vNICs

use std::collections::HashMap;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError};
use ipnet::{Ipv4Net, Ipv6Net};
use nix::libc;
use tracing::{debug, error, info, trace, warn};
use vhost::vhost_user::Listener;
use vhost_user_backend::VhostUserDaemon;
use vm_memory::{ByteValued, GuestMemoryAtomic};
use vmm_sys_util::eventfd::EventFd;

use crate::config::{NetworkEntry, NicEntry};

use super::arp::ArpResponder;
use super::dhcpv4::Dhcpv4Server;
use super::dhcpv6::Dhcpv6Server;
use super::icmp::IcmpResponder;
use super::icmpv6::Icmpv6Responder;
use super::ndp::NdpResponder;
use super::router::{NetworkRouter, NicChannel, RouteResult};
use super::tun::TunDevice;
use super::vhost::{VhostNetBackend, VirtioNetHdr};

use super::buffer::{BufferPool, ETH_HEADROOM, PoolBuffer, VIRTIO_HDR_SIZE};

/// Message for inter-worker routing (zero-copy)
pub struct RoutedPacket {
    /// Target NIC ID
    pub target_nic_id: String,
    /// Raw Ethernet frame in PoolBuffer
    pub buffer: PoolBuffer,
}

/// Packet to send to internet via TUN device (zero-copy)
pub struct TunPacket {
    /// Raw IP packet (no Ethernet header) in PoolBuffer
    pub buffer: PoolBuffer,
    /// Virtio-net header with GSO/checksum offload metadata
    pub virtio_hdr: VirtioNetHdr,
}

/// Configuration for a worker thread
pub struct WorkerConfig {
    /// NIC configuration
    pub nic: NicEntry,
    /// Network configuration
    pub network: NetworkEntry,
    /// Receiver for routed packets (from other workers)
    pub router_rx: Receiver<RoutedPacket>,
    /// Shared router for inter-vNIC routing (per-network)
    pub router: NetworkRouter,
    /// Optional sender for packets destined to internet (public networks only)
    pub tun_tx: Option<crossbeam_channel::Sender<TunPacket>>,
    /// Buffer pool for zero-copy packet processing
    pub pool: Arc<BufferPool>,
}

/// Handle to a running worker
pub struct WorkerHandle {
    /// Worker thread join handle
    thread: Option<JoinHandle<()>>,
    /// Shutdown signal (atomic bool)
    shutdown: Arc<AtomicBool>,
    /// Exit event for waking up blocked worker
    exit_event: EventFd,
    /// NIC ID
    pub nic_id: String,
    /// Socket path
    pub socket_path: PathBuf,
    /// Network ID this worker belongs to
    pub network_id: String,
}

impl WorkerHandle {
    /// Signal the worker to stop
    pub fn stop(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        // Wake up worker if it's blocked
        if let Err(e) = self.exit_event.write(1) {
            warn!(nic_id = %self.nic_id, error = %e, "Failed to signal exit event");
        }
    }

    /// Wait for the worker to finish
    pub fn join(mut self) -> Result<(), String> {
        if let Some(handle) = self.thread.take() {
            handle
                .join()
                .map_err(|_| "Worker thread panicked".to_string())
        } else {
            Ok(())
        }
    }

    /// Check if the worker is still running
    pub fn is_running(&self) -> bool {
        self.thread.as_ref().is_some_and(|h| !h.is_finished())
    }
}

impl Drop for WorkerHandle {
    fn drop(&mut self) {
        self.stop();
        // Clean up socket
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

/// Spawn a new worker thread for a vNIC
pub fn spawn_worker(config: WorkerConfig) -> Result<WorkerHandle, String> {
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_clone = shutdown.clone();
    let nic_id = config.nic.id.clone();
    let nic_id_for_thread = nic_id.clone();
    let network_id = config.network.id.clone();
    let socket_path = PathBuf::from(&config.nic.socket_path);
    let socket_path_clone = socket_path.clone();

    // Create exit event for clean shutdown
    let exit_event = EventFd::new(libc::EFD_NONBLOCK)
        .map_err(|e| format!("Failed to create exit event: {e}"))?;
    let exit_event_clone = exit_event
        .try_clone()
        .map_err(|e| format!("Failed to clone exit event: {e}"))?;

    // Channel to signal when socket is ready
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<Result<(), String>>(0);

    let thread = thread::Builder::new()
        .name(format!("nic-{}", &nic_id[..8]))
        .spawn(move || {
            if let Err(e) = run_worker(config, shutdown_clone, exit_event_clone, ready_tx) {
                error!(nic_id = %nic_id_for_thread, error = %e, "Worker failed");
            }
        })
        .map_err(|e| format!("Failed to spawn worker thread: {e}"))?;

    // Wait for socket to be ready (with timeout)
    match ready_rx.recv_timeout(std::time::Duration::from_secs(5)) {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(e),
        Err(_) => return Err("Timeout waiting for socket to be ready".to_string()),
    }

    Ok(WorkerHandle {
        thread: Some(thread),
        shutdown,
        exit_event,
        nic_id,
        socket_path: socket_path_clone,
        network_id,
    })
}

/// Packet processor that combines all protocol handlers
struct PacketProcessor {
    nic_id: String,
    mac: [u8; 6],
    arp: ArpResponder,
    icmp: IcmpResponder,
    icmpv6: Icmpv6Responder,
    ndp: NdpResponder,
    dhcpv4: Option<Dhcpv4Server>,
    dhcpv6: Option<Dhcpv6Server>,
    router: NetworkRouter,
    /// Sender for packets destined to internet via TUN (public networks only)
    tun_tx: Option<crossbeam_channel::Sender<TunPacket>>,
}

impl PacketProcessor {
    fn new(
        nic: &NicEntry,
        network: &NetworkEntry,
        router: NetworkRouter,
        tun_tx: Option<crossbeam_channel::Sender<TunPacket>>,
    ) -> Self {
        // Parse MAC address
        let mac = parse_mac(&nic.mac_address).unwrap_or([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);

        // Set up ARP responder
        let arp = ArpResponder::new(mac);

        // Set up ICMP responder for gateway (IPv4)
        let icmp = IcmpResponder::new();

        // Set up ICMPv6 responder for gateway (IPv6)
        let icmpv6 = Icmpv6Responder::new();

        // Set up NDP responder (no prefix - all addresses via DHCPv6 only)
        // This ensures VMs route all IPv6 traffic through the gateway
        // Public networks announce themselves as default router, non-public don't
        let ndp = NdpResponder::new(mac, network.is_public);

        // Set up DHCPv4 server if IPv4 is enabled and address is assigned
        let dhcpv4 = if network.ipv4_enabled {
            nic.ipv4_address.as_ref().and_then(|addr_str| {
                addr_str.parse::<Ipv4Addr>().ok().map(|addr| {
                    // Public networks announce default route, non-public don't (isolation)
                    let mut server = Dhcpv4Server::new(addr, network.is_public);
                    // Set DNS servers from network config
                    let dns_servers: Vec<Ipv4Addr> = network
                        .dns_servers
                        .iter()
                        .filter_map(|s| s.parse().ok())
                        .collect();
                    if !dns_servers.is_empty() {
                        server.set_dns_servers(dns_servers);
                    }
                    server
                })
            })
        } else {
            None
        };

        // Set up DHCPv6 server if IPv6 is enabled and address is assigned
        let dhcpv6 = if network.ipv6_enabled {
            nic.ipv6_address.as_ref().and_then(|addr_str| {
                addr_str.parse::<Ipv6Addr>().ok().map(|addr| {
                    let mut server = Dhcpv6Server::new(addr);
                    // Set DNS servers from network config
                    let dns_servers: Vec<Ipv6Addr> = network
                        .dns_servers
                        .iter()
                        .filter_map(|s| s.parse().ok())
                        .collect();
                    if !dns_servers.is_empty() {
                        server.set_dns_servers(dns_servers);
                    }
                    server
                })
            })
        } else {
            None
        };

        Self {
            nic_id: nic.id.clone(),
            mac,
            arp,
            icmp,
            icmpv6,
            ndp,
            dhcpv4,
            dhcpv6,
            router,
            tun_tx,
        }
    }

    /// Try all protocol handlers, return response if any matches
    /// This does NOT consume the buffer - just reads data for protocol detection
    fn try_protocols(&self, packet: &[u8]) -> Option<Vec<u8>> {
        if packet.len() < 14 {
            return None;
        }

        // Try ARP
        if let Some(reply) = self.arp.process(packet) {
            return Some(reply);
        }

        // Try ICMP (ping to gateway, IPv4)
        if let Some(reply) = self.icmp.process(packet) {
            return Some(reply);
        }

        // Try NDP (Neighbor Discovery, Router Advertisement)
        if let Some(reply) = self.ndp.process(packet) {
            return Some(reply);
        }

        // Try ICMPv6 (ping to gateway, IPv6)
        if let Some(reply) = self.icmpv6.process(packet) {
            return Some(reply);
        }

        // Try DHCPv4
        if let Some(ref dhcpv4) = self.dhcpv4
            && let Some(reply) = dhcpv4.process(packet, self.mac)
        {
            return Some(reply);
        }

        // Try DHCPv6
        if let Some(ref dhcpv6) = self.dhcpv6
            && let Some(reply) = dhcpv6.process(packet, self.mac)
        {
            return Some(reply);
        }

        None
    }

    /// Route a packet to another vNIC or internet (consumes PoolBuffer for zero-copy)
    /// The virtio_hdr contains GSO/checksum offload info for TUN packets
    fn route(&self, buffer: PoolBuffer, virtio_hdr: VirtioNetHdr) {
        let len = buffer.len;
        match self.router.route_packet(&self.nic_id, buffer) {
            RouteResult::Routed => {
                debug!(nic_id = %self.nic_id, len = len, "Routed packet to local vNIC");
            }
            RouteResult::ToInternet(buf) => {
                if let Some(ref tx) = self.tun_tx {
                    // Pass virtio header for GSO/checksum offload
                    if let Err(e) = tx.send(TunPacket {
                        buffer: buf,
                        virtio_hdr,
                    }) {
                        debug!(nic_id = %self.nic_id, error = %e, "Failed to send packet to TUN");
                    } else {
                        debug!(
                            nic_id = %self.nic_id,
                            gso_type = virtio_hdr.gso_type,
                            "Sent packet to TUN for internet routing"
                        );
                    }
                } else {
                    debug!(nic_id = %self.nic_id, "ToInternet packet but no TUN available");
                }
            }
            RouteResult::Dropped => {
                // Packet was dropped (no route, TTL expired, etc.)
            }
        }
    }
}

/// Parse MAC address from string
fn parse_mac(s: &str) -> Option<[u8; 6]> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 6 {
        return None;
    }

    let mut mac = [0u8; 6];
    for (i, part) in parts.iter().enumerate() {
        mac[i] = u8::from_str_radix(part, 16).ok()?;
    }
    Some(mac)
}

/// Main worker loop
fn run_worker(
    config: WorkerConfig,
    shutdown: Arc<AtomicBool>,
    exit_event: EventFd,
    ready_tx: std::sync::mpsc::SyncSender<Result<(), String>>,
) -> Result<(), String> {
    let nic_id = &config.nic.id;
    let socket_path = &config.nic.socket_path;

    info!(
        nic_id = %nic_id,
        socket_path = %socket_path,
        "Starting vNIC worker"
    );

    // Register routes for this NIC
    register_nic_routes(&config.router, &config.nic, &config.network);

    // Create listener BEFORE signaling ready - this binds the socket
    let listener = match Listener::new(socket_path, true) {
        Ok(l) => l,
        Err(e) => {
            let err = format!("Failed to create listener: {e}");
            let _ = ready_tx.send(Err(err.clone()));
            return Err(err);
        }
    };

    info!(
        nic_id = %nic_id,
        socket_path = %socket_path,
        "Listening for vhost-user connections"
    );

    // Signal that we're ready - socket is now bound
    let _ = ready_tx.send(Ok(()));
    drop(ready_tx); // Don't need anymore

    // Main loop - keeps worker alive to accept new VM connections after disconnect
    let mut listener = Some(listener);
    loop {
        if shutdown.load(Ordering::SeqCst) {
            info!(nic_id = %nic_id, "Shutdown requested");
            config.router.unregister_nic(nic_id);
            return Ok(());
        }

        // Create a new listener if we don't have one (e.g., after VM disconnect)
        let mut current_listener = if let Some(l) = listener.take() {
            l
        } else {
            info!(nic_id = %nic_id, socket_path = %socket_path, "Creating new listener after disconnect");
            match Listener::new(socket_path, true) {
                Ok(l) => {
                    info!(nic_id = %nic_id, socket_path = %socket_path, "New listener created successfully");
                    l
                }
                Err(e) => {
                    error!(nic_id = %nic_id, socket_path = %socket_path, error = %e, "Failed to create new listener");
                    // Wait a bit before retrying
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    continue;
                }
            }
        };

        // Wait for VM connection with exit event polling
        let connected = loop {
            if shutdown.load(Ordering::SeqCst) {
                info!(nic_id = %nic_id, "Shutdown requested before VM connected");
                config.router.unregister_nic(nic_id);
                return Ok(());
            }

            // Poll the listener socket for incoming connections
            let listener_fd = current_listener.as_raw_fd();
            let exit_fd = exit_event.as_raw_fd();

            let mut pollfds = [
                libc::pollfd {
                    fd: listener_fd,
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: exit_fd,
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];

            let ret = unsafe { libc::poll(pollfds.as_mut_ptr(), 2, 1000) };
            debug!(
                nic_id = %nic_id,
                poll_ret = ret,
                listener_fd = listener_fd,
                exit_fd = exit_fd,
                listener_revents = pollfds[0].revents,
                exit_revents = pollfds[1].revents,
                "Poll returned"
            );
            if ret < 0 {
                let err = std::io::Error::last_os_error();
                if err.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                error!(nic_id = %nic_id, error = %err, "Poll failed");
                break false;
            }

            // Check for exit event
            if pollfds[1].revents & libc::POLLIN != 0 {
                info!(nic_id = %nic_id, "Exit event received");
                config.router.unregister_nic(nic_id);
                return Ok(());
            }

            // Check for incoming connection
            if pollfds[0].revents & libc::POLLIN != 0 {
                info!(nic_id = %nic_id, "VM connecting");
                break true;
            }
        };

        if !connected {
            // Poll error, retry with new listener
            continue;
        }

        // Create new daemon and backend for each connection
        let backend =
            match VhostNetBackend::new(config.nic.clone(), shutdown.clone(), config.pool.clone()) {
                Ok(b) => Arc::new(b),
                Err(e) => {
                    error!(nic_id = %nic_id, error = %e, "Failed to create backend");
                    continue;
                }
            };

        // Set up packet processor for this connection
        let processor = PacketProcessor::new(
            &config.nic,
            &config.network,
            config.router.clone(),
            config.tun_tx.clone(),
        );

        // Clone backend for use in packet handler (for injecting protocol responses)
        let backend_for_handler = backend.clone();
        backend.set_packet_handler(Box::new(move |buffer: PoolBuffer, virtio_hdr: VirtioNetHdr| {
            // First try protocol handlers (ARP, DHCP, ICMP, etc.)
            // These just read the data and may return a response
            if let Some(response) = processor.try_protocols(buffer.data()) {
                // Protocol generated a response - inject it as Vec (small local packet)
                backend_for_handler.inject_vec(response);
                // Original buffer is dropped here (returned to pool)
                return;
            }

            // No protocol match - route the packet (zero-copy, consumes buffer)
            // Pass virtio_hdr for GSO/checksum offload to TUN
            processor.route(buffer, virtio_hdr);
        }));

        // Spawn RX injection thread to handle routed packets from other vNICs
        let rx_channel = config.router_rx.clone();
        let backend_for_rx = backend.clone();
        let shutdown_for_rx = shutdown.clone();
        let nic_id_for_rx = nic_id.clone();

        let rx_thread = thread::Builder::new()
            .name(format!("rx-{}", &nic_id[..8]))
            .spawn(move || {
                run_rx_injection(rx_channel, backend_for_rx, shutdown_for_rx, nic_id_for_rx);
            })
            .map_err(|e| format!("Failed to spawn RX injection thread: {e}"))?;

        // Create vhost-user daemon
        let mut daemon = match VhostUserDaemon::new(
            format!("mvirt-net-{}", &nic_id[..8]),
            backend.clone(),
            GuestMemoryAtomic::new(vm_memory::GuestMemoryMmap::new()),
        ) {
            Ok(d) => d,
            Err(e) => {
                error!(nic_id = %nic_id, error = %e, "Failed to create daemon");
                continue;
            }
        };

        // Start the daemon (accepts connection and spawns handler thread)
        info!(nic_id = %nic_id, "Starting vhost-user daemon");
        if let Err(e) = daemon.start(&mut current_listener) {
            error!(nic_id = %nic_id, error = %e, "Failed to start daemon");
            continue;
        }

        info!(nic_id = %nic_id, "VM connected, running vhost-user daemon");

        // Wait for daemon to finish (VM disconnect)
        info!(nic_id = %nic_id, "Waiting for VM to disconnect");
        let wait_result = daemon.wait();
        info!(nic_id = %nic_id, "VM disconnected, daemon.wait() returned");

        if let Err(e) = wait_result {
            let err_str = e.to_string();
            if err_str.contains("disconnected") || err_str.contains("PartialMessage") {
                // Normal VM shutdown - not an error
                debug!(nic_id = %nic_id, "VM disconnected from vhost-user socket");
            } else {
                // Real error
                error!(nic_id = %nic_id, error = %e, "Daemon error");
            }
        }

        // RX thread will exit when it sees shutdown or channel disconnect
        // We don't need to explicitly join it here as it will terminate on its own
        info!(nic_id = %nic_id, "Dropping rx_thread handle");
        drop(rx_thread);
        info!(nic_id = %nic_id, "Dropped rx_thread handle");

        // Explicitly drop daemon to release the socket before creating a new listener
        // The vhost-user library drops the Listener inside daemon.start(), which deletes
        // the socket file. We need to recreate it for the next connection.
        info!(nic_id = %nic_id, "Dropping daemon");
        drop(daemon);
        info!(nic_id = %nic_id, "Dropped daemon");

        info!(nic_id = %nic_id, "Dropping backend");
        drop(backend);
        info!(nic_id = %nic_id, "Dropped backend, ready for new VM connection");

        // listener is None now, will be recreated at top of loop
    }
}

/// RX injection thread - reads routed packets from channel and injects them into the VM
/// Uses batch processing to amortize syscall overhead across multiple packets
fn run_rx_injection(
    rx_channel: Receiver<RoutedPacket>,
    backend: Arc<VhostNetBackend>,
    shutdown: Arc<AtomicBool>,
    nic_id: String,
) {
    debug!(nic_id = %nic_id, "RX injection thread started");

    // Batch parameters: collect up to BATCH_SIZE packets or until BATCH_TIMEOUT
    const BATCH_SIZE: usize = 32;
    const BATCH_TIMEOUT: Duration = Duration::from_micros(100);

    loop {
        if shutdown.load(Ordering::SeqCst) {
            debug!(nic_id = %nic_id, "RX injection thread shutting down");
            break;
        }

        // Collect a batch of packets
        let mut batch: Vec<RoutedPacket> = Vec::with_capacity(BATCH_SIZE);
        let deadline = Instant::now() + BATCH_TIMEOUT;

        // First packet: use longer timeout to avoid busy-waiting when idle
        match rx_channel.recv_timeout(Duration::from_millis(100)) {
            Ok(packet) => batch.push(packet),
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => {
                debug!(nic_id = %nic_id, "RX channel disconnected");
                break;
            }
        }

        // Collect more packets until batch full or timeout
        while batch.len() < BATCH_SIZE {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }

            match rx_channel.recv_timeout(remaining) {
                Ok(packet) => batch.push(packet),
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }

        // Inject entire batch with single signal (zero-copy!)
        if !batch.is_empty() {
            trace!(
                nic_id = %nic_id,
                batch_size = batch.len(),
                "Injecting packet batch (zero-copy)"
            );
            // Extract PoolBuffers from RoutedPackets and inject directly
            backend.inject_buffer_batch(batch.into_iter().map(|p| p.buffer));
        }
    }
}

/// Register routes for a NIC in the router
fn register_nic_routes(router: &NetworkRouter, nic: &NicEntry, network: &NetworkEntry) {
    // Add direct route for NIC's IPv4 address
    if network.ipv4_enabled {
        if let Some(ref addr_str) = nic.ipv4_address
            && let Ok(addr) = addr_str.parse::<Ipv4Addr>()
        {
            let prefix = Ipv4Net::new(addr, 32).unwrap();
            router.add_ipv4_route(prefix, nic.id.clone(), true);
            debug!(nic_id = %nic.id, addr = %addr, "Added IPv4 direct route");
        }

        // Add routed prefixes
        for prefix_str in &nic.routed_ipv4_prefixes {
            if let Ok(prefix) = prefix_str.parse::<Ipv4Net>() {
                router.add_ipv4_route(prefix, nic.id.clone(), false);
                debug!(nic_id = %nic.id, prefix = %prefix, "Added IPv4 routed prefix");
            }
        }
    }

    // Add direct route for NIC's IPv6 address
    if network.ipv6_enabled {
        if let Some(ref addr_str) = nic.ipv6_address
            && let Ok(addr) = addr_str.parse::<Ipv6Addr>()
        {
            let prefix = Ipv6Net::new(addr, 128).unwrap();
            router.add_ipv6_route(prefix, nic.id.clone(), true);
            debug!(nic_id = %nic.id, addr = %addr, "Added IPv6 direct route");
        }

        // Add routed prefixes
        for prefix_str in &nic.routed_ipv6_prefixes {
            if let Ok(prefix) = prefix_str.parse::<Ipv6Net>() {
                router.add_ipv6_route(prefix, nic.id.clone(), false);
                debug!(nic_id = %nic.id, prefix = %prefix, "Added IPv6 routed prefix");
            }
        }
    }
}

/// Handle to the TUN reader thread
struct TunReaderHandle {
    thread: JoinHandle<()>,
    shutdown: Arc<AtomicBool>,
}

impl TunReaderHandle {
    fn stop(self) {
        self.shutdown.store(true, Ordering::SeqCst);
        // Thread will exit on next poll timeout
        let _ = self.thread.join();
    }
}

/// Shared routers accessible from TUN IO thread
type SharedRouters = Arc<std::sync::RwLock<HashMap<String, NetworkRouter>>>;

/// Worker manager that tracks all active workers with per-network routing
pub struct WorkerManager {
    /// Active workers by NIC ID
    workers: HashMap<String, WorkerHandle>,
    /// Per-network routers (shared with TUN thread)
    routers: SharedRouters,
    /// Global TUN device sender
    tun_tx: crossbeam_channel::Sender<TunPacket>,
    /// TUN reader thread handle
    tun_reader: Option<TunReaderHandle>,
    /// Shared buffer pool for zero-copy packet processing
    pool: Arc<BufferPool>,
}

impl WorkerManager {
    /// Create a new worker manager with TUN device
    pub fn new() -> Result<Self, String> {
        info!("Creating global TUN device 'mvirt-net'");

        // Create buffer pool for zero-copy packet processing
        let pool =
            Arc::new(BufferPool::new().map_err(|e| format!("Failed to create buffer pool: {e}"))?);

        // Create TUN device
        let tun = TunDevice::new().map_err(|e| format!("Failed to create TUN device: {e}"))?;
        tun.bring_up()
            .map_err(|e| format!("Failed to bring up TUN device: {e}"))?;

        // Enable TSO/checksum offload on TUN device
        if let Err(e) = tun.enable_offload() {
            warn!(error = %e, "Failed to enable TUN offload (GSO may not work)");
        }

        info!(name = %tun.name(), "TUN device created and brought up");

        // Create channel for outgoing packets (workers -> TUN)
        let (tun_tx, tun_rx) = crossbeam_channel::unbounded::<TunPacket>();

        // Shared routers accessible from TUN thread
        let routers: SharedRouters = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let routers_for_tun = routers.clone();
        let pool_for_tun = pool.clone();

        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = shutdown.clone();

        // Spawn TUN IO thread
        let thread = thread::Builder::new()
            .name("tun-io".to_string())
            .spawn(move || {
                run_tun_io(tun, tun_rx, routers_for_tun, shutdown_clone, pool_for_tun);
            })
            .map_err(|e| format!("Failed to spawn TUN IO thread: {e}"))?;

        Ok(Self {
            workers: HashMap::new(),
            routers,
            tun_tx,
            tun_reader: Some(TunReaderHandle { thread, shutdown }),
            pool,
        })
    }

    /// Start a worker for a NIC
    pub fn start(&mut self, nic: NicEntry, network: NetworkEntry) -> Result<(), String> {
        if self.workers.contains_key(&nic.id) {
            return Err(format!("Worker for NIC {} already running", nic.id));
        }

        // Get or create router for this network
        let router = {
            let mut routers = self.routers.write().unwrap();
            routers
                .entry(network.id.clone())
                .or_insert_with(|| NetworkRouter::new(network.id.clone(), network.is_public))
                .clone()
        };

        // For public networks, pass TUN sender
        let tun_tx = if network.is_public {
            Some(self.tun_tx.clone())
        } else {
            None
        };

        // Create dedicated channel for this worker
        let (worker_tx, worker_rx) = crossbeam_channel::unbounded();

        // Parse NIC MAC address for Ethernet header rewriting
        let mac = parse_mac(&nic.mac_address)
            .ok_or_else(|| format!("Invalid MAC address: {}", nic.mac_address))?;

        // Register with router
        router.register_nic(
            nic.id.clone(),
            NicChannel {
                sender: worker_tx,
                mac,
            },
        );

        let config = WorkerConfig {
            nic: nic.clone(),
            network,
            router_rx: worker_rx,
            router,
            tun_tx,
            pool: self.pool.clone(),
        };

        let handle = spawn_worker(config)?;
        self.workers.insert(nic.id, handle);
        Ok(())
    }

    /// Stop a worker for a NIC
    pub fn stop(&mut self, nic_id: &str) -> Result<(), String> {
        if let Some(handle) = self.workers.remove(nic_id) {
            handle.stop();
            // Unregister from the network's router
            let routers = self.routers.read().unwrap();
            if let Some(router) = routers.get(&handle.network_id) {
                router.unregister_nic(nic_id);
            }
            Ok(())
        } else {
            Err(format!("No worker for NIC {}", nic_id))
        }
    }

    /// Stop all workers
    pub fn stop_all(&mut self) {
        let routers = self.routers.read().unwrap();
        for (nic_id, handle) in self.workers.drain() {
            handle.stop();
            if let Some(router) = routers.get(&handle.network_id) {
                router.unregister_nic(&nic_id);
            }
        }
        drop(routers);

        // Stop TUN reader thread if running
        if let Some(tun_reader) = self.tun_reader.take() {
            info!("Stopping TUN IO thread");
            tun_reader.stop();
        }
    }

    /// Remove a network's router (call when network is deleted)
    pub fn remove_network(&mut self, network_id: &str) {
        self.routers.write().unwrap().remove(network_id);
    }

    /// Check if a worker is running
    pub fn is_running(&self, nic_id: &str) -> bool {
        self.workers.get(nic_id).is_some_and(|h| h.is_running())
    }

    /// Get list of active NIC IDs
    pub fn active_nics(&self) -> Vec<String> {
        self.workers.keys().cloned().collect()
    }

    /// Get router for a network (if it exists)
    pub fn router(&self, network_id: &str) -> Option<NetworkRouter> {
        self.routers.read().unwrap().get(network_id).cloned()
    }
}

/// Run TUN I/O loop - handles both reading from TUN and writing to TUN
///
/// This function:
/// - Reads packets from channel (VM -> Internet) and writes them to TUN (zero-copy with writev)
/// - Reads packets from TUN (Internet -> VM) and routes them to correct vNIC
fn run_tun_io(
    mut tun: TunDevice,
    tun_rx: crossbeam_channel::Receiver<TunPacket>,
    routers: SharedRouters,
    shutdown: Arc<AtomicBool>,
    pool: Arc<BufferPool>,
) {
    use crossbeam_channel::TryRecvError;
    use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
    use nix::sys::uio::writev;
    use std::os::fd::BorrowedFd;

    info!("TUN IO thread started");

    let tun_fd = tun.as_raw_fd();

    loop {
        if shutdown.load(Ordering::SeqCst) {
            break;
        }

        let mut did_work = false;

        // 1. First drain ALL outgoing packets (VM -> TUN) without blocking
        loop {
            match tun_rx.try_recv() {
                Ok(packet) => {
                    did_work = true;
                    // Adjust virtio header offsets: VM sends offsets relative to
                    // Ethernet frame, but TUN operates on raw IP packets (no Ethernet).
                    // Subtract ETH_HEADROOM (14) from csum_start and hdr_len.
                    let mut hdr = packet.virtio_hdr;
                    let eth_offset = ETH_HEADROOM as u16;
                    let orig_csum_start = hdr.csum_start.to_native();
                    let orig_hdr_len = hdr.hdr_len.to_native();
                    if orig_csum_start >= eth_offset {
                        hdr.csum_start = (orig_csum_start - eth_offset).into();
                    }
                    if orig_hdr_len >= eth_offset {
                        hdr.hdr_len = (orig_hdr_len - eth_offset).into();
                    }

                    let payload_len = packet.buffer.len;
                    debug!(
                        flags = hdr.flags,
                        gso_type = hdr.gso_type,
                        orig_hdr_len,
                        orig_csum_start,
                        csum_offset = hdr.csum_offset.to_native(),
                        gso_size = hdr.gso_size.to_native(),
                        adj_hdr_len = hdr.hdr_len.to_native(),
                        adj_csum_start = hdr.csum_start.to_native(),
                        payload_len,
                        "TUN write virtio header"
                    );

                    // Use scatter-gather I/O: virtio header + payload
                    // With IFF_VNET_HDR, kernel expects virtio_net_hdr prepended
                    let hdr_bytes = hdr.as_slice();
                    let payload = packet.buffer.as_io_slice();
                    let iov = [
                        std::io::IoSlice::new(hdr_bytes),
                        std::io::IoSlice::new(payload.as_ref()),
                    ];
                    let fd = unsafe { BorrowedFd::borrow_raw(tun_fd) };
                    match writev(fd, &iov) {
                        Ok(written) => {
                            trace!(
                                len = written,
                                gso_type = hdr.gso_type,
                                "Wrote packet to TUN with virtio header"
                            );
                        }
                        Err(e) => {
                            warn!(
                                error = %e,
                                flags = hdr.flags,
                                gso_type = hdr.gso_type,
                                hdr_len = hdr.hdr_len.to_native(),
                                csum_start = hdr.csum_start.to_native(),
                                csum_offset = hdr.csum_offset.to_native(),
                                gso_size = hdr.gso_size.to_native(),
                                payload_len,
                                "Failed to write packet to TUN"
                            );
                        }
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    info!("TUN channel disconnected, exiting");
                    return;
                }
            }
        }

        // 2. Then drain ALL incoming packets (Internet -> VM) without blocking
        loop {
            let poll_fd = PollFd::new(unsafe { BorrowedFd::borrow_raw(tun_fd) }, PollFlags::POLLIN);

            match poll(&mut [poll_fd], PollTimeout::ZERO) {
                Ok(n) if n > 0 => {
                    did_work = true;
                    let Some(mut buffer) = pool.alloc() else {
                        warn!("Buffer pool exhausted, dropping incoming TUN packet");
                        let mut tmp = [0u8; 1500];
                        let _ = tun.read_packet(&mut tmp);
                        continue;
                    };

                    match tun.read_packet(buffer.write_area()) {
                        Ok(len) => {
                            // With IFF_VNET_HDR, kernel prepends 12-byte virtio_net_hdr
                            // Skip it to get the raw IP packet
                            if len > VIRTIO_HDR_SIZE {
                                buffer.start += VIRTIO_HDR_SIZE;
                                buffer.len = len - VIRTIO_HDR_SIZE;
                                route_tun_packet_to_nic_buffer(buffer, &routers, &pool);
                            } else {
                                trace!(len, "TUN packet too small, dropping");
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(e) => {
                            warn!(error = %e, "Failed to read from TUN");
                            break;
                        }
                    }
                }
                Ok(_) => break, // No data available
                Err(e) => {
                    warn!(error = %e, "Poll on TUN fd failed");
                    break;
                }
            }
        }

        // 3. If no work was done, wait briefly to avoid busy-spinning
        if !did_work {
            // Poll TUN with 1ms timeout - wake up quickly for incoming packets
            let poll_fd = PollFd::new(unsafe { BorrowedFd::borrow_raw(tun_fd) }, PollFlags::POLLIN);
            let _ = poll(&mut [poll_fd], PollTimeout::from(1u16));
        }
    }

    info!("TUN IO thread stopped");
}

/// Route a packet from TUN to the correct vNIC (zero-copy version using PoolBuffer)
///
/// The buffer contains a raw IP packet. We prepend an Ethernet header using headroom
/// and route it to the correct vNIC.
fn route_tun_packet_to_nic_buffer(
    mut buffer: PoolBuffer,
    routers: &SharedRouters,
    _pool: &Arc<BufferPool>,
) {
    use super::packet::GATEWAY_MAC;
    use smoltcp::wire::{Ipv4Packet, Ipv6Packet};
    use std::net::{Ipv4Addr, Ipv6Addr};

    let ip_packet = buffer.data();
    if ip_packet.is_empty() {
        return;
    }

    // Determine IP version from first nibble
    let version = ip_packet[0] >> 4;

    // Parse destination address and ethertype
    let (dst_ipv4, dst_ipv6, ethertype): (Option<Ipv4Addr>, Option<Ipv6Addr>, u16) = match version {
        4 => {
            if ip_packet.len() < 20 {
                return;
            }
            match Ipv4Packet::new_checked(ip_packet) {
                Ok(ipv4) => (Some(ipv4.dst_addr()), None, 0x0800u16),
                Err(_) => return,
            }
        }
        6 => {
            if ip_packet.len() < 40 {
                return;
            }
            match Ipv6Packet::new_checked(ip_packet) {
                Ok(ipv6) => (None, Some(ipv6.dst_addr()), 0x86DDu16),
                Err(_) => return,
            }
        }
        _ => return,
    };

    // Prepend Ethernet header using headroom (zero-copy!)
    // Dst MAC is placeholder - the router will overwrite it
    buffer.prepend_eth_header([0u8; 6], GATEWAY_MAC, ethertype);

    // First find which router has a route for this destination (without consuming buffer)
    let routers_guard = routers.read().unwrap();
    let matching_router = routers_guard.values().find(|router| {
        if !router.is_public() {
            return false;
        }
        // Check if this router has a route for the destination
        match (dst_ipv4, dst_ipv6) {
            (Some(addr), _) => router.lookup_ipv4(addr).is_some(),
            (_, Some(addr)) => router.lookup_ipv6(addr).is_some(),
            _ => false,
        }
    });

    if let Some(router) = matching_router {
        // Clone the router to release the lock before calling route_packet
        let router = router.clone();
        drop(routers_guard);

        // Route the packet (zero-copy, consumes buffer)
        match router.route_packet("tun", buffer) {
            RouteResult::Routed => {
                debug!(
                    version = version,
                    "Routed TUN packet to local vNIC (zero-copy)"
                );
            }
            RouteResult::ToInternet(_) => {
                // Shouldn't happen for incoming TUN packets
            }
            RouteResult::Dropped => {
                debug!(version = version, "TUN packet dropped by router");
            }
        }
    } else {
        debug!(version = version, "No route found for TUN packet");
    }
}

impl Drop for WorkerManager {
    fn drop(&mut self) {
        self.stop_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_mac() {
        let mac = parse_mac("52:54:00:12:34:56").unwrap();
        assert_eq!(mac, [0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);

        assert!(parse_mac("invalid").is_none());
        assert!(parse_mac("52:54:00:12:34").is_none()); // Too short
        assert!(parse_mac("52:54:00:12:34:56:78").is_none()); // Too long
        assert!(parse_mac("GG:54:00:12:34:56").is_none()); // Invalid hex
    }
}
