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
use std::time::Duration;

use crossbeam_channel::{Receiver, RecvTimeoutError};
use ipnet::{Ipv4Net, Ipv6Net};
use nix::libc;
use nix::sys::eventfd::{EfdFlags, EventFd as NixEventFd};
use tracing::{debug, error, info, warn};
use vhost::vhost_user::Listener;
use vhost_user_backend::VhostUserDaemon;
use vm_memory::GuestMemoryAtomic;
use vmm_sys_util::eventfd::EventFd;

use crate::config::{NetworkEntry, NicEntry};

use super::arp::ArpResponder;
use super::dhcpv4::Dhcpv4Server;
use super::dhcpv6::Dhcpv6Server;
use super::icmp::IcmpResponder;
use super::icmpv6::Icmpv6Responder;
use super::ndp::NdpResponder;
use super::router::{NetworkRouter, NicChannel};
use super::vhost::VhostNetBackend;

/// Message for inter-worker routing
pub struct RoutedPacket {
    /// Target NIC ID
    pub target_nic_id: String,
    /// Raw Ethernet frame
    pub data: Vec<u8>,
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
}

impl PacketProcessor {
    fn new(nic: &NicEntry, network: &NetworkEntry, router: NetworkRouter) -> Self {
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
        let ndp = NdpResponder::new(mac);

        // Set up DHCPv4 server if IPv4 is enabled and address is assigned
        let dhcpv4 = if network.ipv4_enabled {
            nic.ipv4_address.as_ref().and_then(|addr_str| {
                addr_str.parse::<Ipv4Addr>().ok().map(|addr| {
                    let mut server = Dhcpv4Server::new(addr);
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
        }
    }

    /// Process an incoming packet and return an optional response
    fn process(&self, packet: &[u8]) -> Option<Vec<u8>> {
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

        // Try routing to another vNIC
        if self.router.route_packet(&self.nic_id, packet) {
            debug!(nic_id = %self.nic_id, len = packet.len(), "Routed packet");
        }

        None
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
        let current_listener = if let Some(l) = listener.take() {
            l
        } else {
            info!(nic_id = %nic_id, "Creating new listener after disconnect");
            match Listener::new(socket_path, true) {
                Ok(l) => l,
                Err(e) => {
                    error!(nic_id = %nic_id, error = %e, "Failed to create new listener");
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
        let backend = match VhostNetBackend::new(config.nic.clone(), shutdown.clone()) {
            Ok(b) => Arc::new(b),
            Err(e) => {
                error!(nic_id = %nic_id, error = %e, "Failed to create backend");
                continue;
            }
        };

        // Set up packet processor for this connection
        let processor = PacketProcessor::new(&config.nic, &config.network, config.router.clone());
        backend.set_packet_handler(Box::new(move |packet| processor.process(packet)));

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
        if let Err(e) = daemon.start(current_listener) {
            error!(nic_id = %nic_id, error = %e, "Failed to start daemon");
            continue;
        }

        info!(nic_id = %nic_id, "VM connected, running vhost-user daemon");

        // Wait for daemon to finish (VM disconnect)
        if let Err(e) = daemon.wait() {
            let err_str = e.to_string();
            if err_str.contains("disconnected") {
                // Normal VM shutdown - not an error
                debug!(nic_id = %nic_id, "VM disconnected from vhost-user socket");
            } else {
                // Real error
                error!(nic_id = %nic_id, error = %e, "Daemon error");
            }
        }

        // RX thread will exit when it sees shutdown or channel disconnect
        // We don't need to explicitly join it here as it will terminate on its own
        drop(rx_thread);

        info!(nic_id = %nic_id, "Waiting for new VM connection");
        // listener is None now, will be recreated at top of loop
    }
}

/// RX injection thread - reads routed packets from channel and injects them into the VM
fn run_rx_injection(
    rx_channel: Receiver<RoutedPacket>,
    backend: Arc<VhostNetBackend>,
    shutdown: Arc<AtomicBool>,
    nic_id: String,
) {
    debug!(nic_id = %nic_id, "RX injection thread started");

    loop {
        if shutdown.load(Ordering::SeqCst) {
            debug!(nic_id = %nic_id, "RX injection thread shutting down");
            break;
        }

        match rx_channel.recv_timeout(Duration::from_millis(100)) {
            Ok(packet) => {
                debug!(
                    nic_id = %nic_id,
                    len = packet.data.len(),
                    "Injecting routed packet"
                );
                backend.inject_and_deliver(packet.data);
            }
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => {
                debug!(nic_id = %nic_id, "RX channel disconnected");
                break;
            }
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

/// Worker manager that tracks all active workers with per-network routing
pub struct WorkerManager {
    /// Active workers by NIC ID
    workers: HashMap<String, WorkerHandle>,
    /// Per-network routers
    routers: HashMap<String, NetworkRouter>,
}

impl WorkerManager {
    /// Create a new worker manager
    pub fn new() -> Self {
        Self {
            workers: HashMap::new(),
            routers: HashMap::new(),
        }
    }

    /// Start a worker for a NIC
    pub fn start(&mut self, nic: NicEntry, network: NetworkEntry) -> Result<(), String> {
        if self.workers.contains_key(&nic.id) {
            return Err(format!("Worker for NIC {} already running", nic.id));
        }

        // Get or create router for this network
        let router = self
            .routers
            .entry(network.id.clone())
            .or_insert_with(|| NetworkRouter::new(network.id.clone()))
            .clone();

        // Create dedicated channel for this worker
        let (worker_tx, worker_rx) = crossbeam_channel::unbounded();

        // Create wakeup EventFd for RX injection signaling
        let wakeup_eventfd = NixEventFd::from_flags(EfdFlags::EFD_NONBLOCK)
            .map_err(|e| format!("Failed to create wakeup eventfd: {e}"))?;

        // Parse NIC MAC address for Ethernet header rewriting
        let mac = parse_mac(&nic.mac_address)
            .ok_or_else(|| format!("Invalid MAC address: {}", nic.mac_address))?;

        // Register with router including wakeup signal and MAC
        router.register_nic(
            nic.id.clone(),
            NicChannel {
                sender: worker_tx,
                wakeup: Arc::new(wakeup_eventfd),
                mac,
            },
        );

        let config = WorkerConfig {
            nic: nic.clone(),
            network,
            router_rx: worker_rx,
            router,
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
            if let Some(router) = self.routers.get(&handle.network_id) {
                router.unregister_nic(nic_id);
            }
            Ok(())
        } else {
            Err(format!("No worker for NIC {}", nic_id))
        }
    }

    /// Stop all workers
    pub fn stop_all(&mut self) {
        for (nic_id, handle) in self.workers.drain() {
            handle.stop();
            if let Some(router) = self.routers.get(&handle.network_id) {
                router.unregister_nic(&nic_id);
            }
        }
    }

    /// Remove a network's router (call when network is deleted)
    pub fn remove_network(&mut self, network_id: &str) {
        self.routers.remove(network_id);
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
    pub fn router(&self, network_id: &str) -> Option<&NetworkRouter> {
        self.routers.get(network_id)
    }
}

impl Default for WorkerManager {
    fn default() -> Self {
        Self::new()
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
