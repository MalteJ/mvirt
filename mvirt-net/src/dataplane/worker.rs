//! Per-vNIC worker thread management
//!
//! Each vNIC gets its own worker thread that handles:
//! - vhost-user socket listener
//! - Packet processing (ARP, NDP, DHCP, routing)

use std::net::{Ipv4Addr, Ipv6Addr};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

use crossbeam_channel::{Receiver, Sender};
use ipnet::{Ipv4Net, Ipv6Net};
use smoltcp::wire::Ipv6Address;
use tracing::{debug, error, info};
use vhost_user_backend::VhostUserDaemon;
use vm_memory::GuestMemoryAtomic;

use crate::config::{NetworkEntry, NicEntry};

use super::arp::ArpResponder;
use super::dhcpv4::Dhcpv4Server;
use super::dhcpv6::Dhcpv6Server;
use super::ndp::NdpResponder;
use super::router::Router;
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
    /// Sender for routed packets (to other workers)
    pub router_tx: Sender<RoutedPacket>,
    /// Receiver for routed packets (from other workers)
    pub router_rx: Receiver<RoutedPacket>,
    /// Shared router for inter-vNIC routing
    pub router: Router,
}

/// Handle to a running worker
pub struct WorkerHandle {
    /// Worker thread join handle
    thread: Option<JoinHandle<()>>,
    /// Shutdown signal
    shutdown: Arc<AtomicBool>,
    /// NIC ID
    pub nic_id: String,
    /// Socket path
    pub socket_path: PathBuf,
}

impl WorkerHandle {
    /// Signal the worker to stop
    pub fn stop(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
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
    let socket_path = PathBuf::from(&config.nic.socket_path);
    let socket_path_clone = socket_path.clone();

    let thread = thread::Builder::new()
        .name(format!("nic-{}", &nic_id[..8]))
        .spawn(move || {
            if let Err(e) = run_worker(config, shutdown_clone) {
                error!(nic_id = %nic_id_for_thread, error = %e, "Worker failed");
            }
        })
        .map_err(|e| format!("Failed to spawn worker thread: {e}"))?;

    Ok(WorkerHandle {
        thread: Some(thread),
        shutdown,
        nic_id,
        socket_path: socket_path_clone,
    })
}

/// Packet processor that combines all protocol handlers
struct PacketProcessor {
    nic_id: String,
    mac: [u8; 6],
    arp: ArpResponder,
    ndp: NdpResponder,
    dhcpv4: Option<Dhcpv4Server>,
    dhcpv6: Option<Dhcpv6Server>,
    router: Router,
}

impl PacketProcessor {
    fn new(nic: &NicEntry, network: &NetworkEntry, router: Router) -> Self {
        // Parse MAC address
        let mac = parse_mac(&nic.mac_address).unwrap_or([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);

        // Set up ARP responder
        let arp = ArpResponder::new(mac);

        // Set up NDP responder with prefix if IPv6 is enabled
        let mut ndp = NdpResponder::new(mac);
        if network.ipv6_enabled
            && let Some(ref prefix_str) = network.ipv6_prefix
            && let Ok(prefix) = prefix_str.parse::<Ipv6Net>()
        {
            let addr_bytes = prefix.network().octets();
            let addr = Ipv6Address::from_bytes(&addr_bytes);
            ndp.set_prefix(addr, prefix.prefix_len());
        }

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
            debug!(nic_id = %self.nic_id, "Sent ARP reply");
            return Some(reply);
        }

        // Try NDP
        if let Some(reply) = self.ndp.process(packet) {
            debug!(nic_id = %self.nic_id, "Sent NDP reply");
            return Some(reply);
        }

        // Try DHCPv4
        if let Some(ref dhcpv4) = self.dhcpv4
            && let Some(reply) = dhcpv4.process(packet, self.mac)
        {
            debug!(nic_id = %self.nic_id, "Sent DHCPv4 reply");
            return Some(reply);
        }

        // Try DHCPv6
        if let Some(ref dhcpv6) = self.dhcpv6
            && let Some(reply) = dhcpv6.process(packet, self.mac)
        {
            debug!(nic_id = %self.nic_id, "Sent DHCPv6 reply");
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
fn run_worker(config: WorkerConfig, shutdown: Arc<AtomicBool>) -> Result<(), String> {
    let nic_id = &config.nic.id;
    let socket_path = &config.nic.socket_path;

    info!(
        nic_id = %nic_id,
        socket_path = %socket_path,
        "Starting vNIC worker"
    );

    // Register routes for this NIC
    register_nic_routes(&config.router, &config.nic, &config.network);

    // Register this NIC's channel with the router
    let (worker_tx, _) = crossbeam_channel::unbounded();
    config.router.register_nic(nic_id.clone(), worker_tx);

    // Remove existing socket if present
    let _ = std::fs::remove_file(socket_path);

    // Create vhost-user backend
    let backend = Arc::new(
        VhostNetBackend::new(config.nic.clone(), shutdown.clone())
            .map_err(|e| format!("Failed to create backend: {e}"))?,
    );

    // Set up packet processor
    let processor = PacketProcessor::new(&config.nic, &config.network, config.router.clone());

    backend.set_packet_handler(Box::new(move |packet| processor.process(packet)));

    // Create Unix listener
    let listener = UnixListener::bind(socket_path)
        .map_err(|e| format!("Failed to bind socket {socket_path}: {e}"))?;

    info!(
        nic_id = %nic_id,
        socket_path = %socket_path,
        "Listening for vhost-user connections"
    );

    // Set socket to non-blocking for shutdown handling
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("Failed to set non-blocking: {e}"))?;

    // Wait for connection
    let mut daemon = None;
    while !shutdown.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((socket, _addr)) => {
                info!(nic_id = %nic_id, "VM connected");

                // Create vhost-user daemon
                let d = VhostUserDaemon::new(
                    format!("mvirt-net-{}", &nic_id[..8]),
                    backend.clone(),
                    GuestMemoryAtomic::new(vm_memory::GuestMemoryMmap::new()),
                )
                .map_err(|e| format!("Failed to create daemon: {e}"))?;

                daemon = Some((d, socket));
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // No connection yet, check for routed packets
                while let Ok(routed) = config.router_rx.try_recv() {
                    debug!(
                        nic_id = %nic_id,
                        from_len = routed.data.len(),
                        "Received routed packet"
                    );
                    backend.inject_packet(routed.data);
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(e) => {
                return Err(format!("Accept failed: {e}"));
            }
        }
    }

    // If we got a connection, run the daemon
    if let Some((_d, _socket)) = daemon {
        info!(nic_id = %nic_id, "Running vhost-user daemon");

        // Start daemon (this blocks until connection closes or error)
        // Note: The actual vhost-user-backend crate runs its own epoll loop
        // For now we use a simple approach
        while !shutdown.load(Ordering::Relaxed) {
            // Process routed packets
            while let Ok(routed) = config.router_rx.try_recv() {
                debug!(
                    nic_id = %nic_id,
                    from_len = routed.data.len(),
                    "Received routed packet"
                );
                backend.inject_packet(routed.data);
            }

            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    // Unregister from router
    config.router.unregister_nic(nic_id);

    info!(nic_id = %nic_id, "Worker shutting down");
    Ok(())
}

/// Register routes for a NIC in the router
fn register_nic_routes(router: &Router, nic: &NicEntry, network: &NetworkEntry) {
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

/// Worker manager that tracks all active workers
pub struct WorkerManager {
    /// Active workers by NIC ID
    workers: std::collections::HashMap<String, WorkerHandle>,
    /// Shared router for inter-vNIC routing
    router: Router,
}

impl WorkerManager {
    /// Create a new worker manager
    pub fn new() -> Self {
        Self {
            workers: std::collections::HashMap::new(),
            router: Router::new(),
        }
    }

    /// Start a worker for a NIC
    pub fn start(&mut self, nic: NicEntry, network: NetworkEntry) -> Result<(), String> {
        if self.workers.contains_key(&nic.id) {
            return Err(format!("Worker for NIC {} already running", nic.id));
        }

        // Create dedicated channel for this worker
        let (worker_tx, worker_rx) = crossbeam_channel::unbounded();

        // Register with router
        self.router.register_nic(nic.id.clone(), worker_tx.clone());

        let config = WorkerConfig {
            nic: nic.clone(),
            network,
            router_tx: worker_tx,
            router_rx: worker_rx,
            router: self.router.clone(),
        };

        let handle = spawn_worker(config)?;
        self.workers.insert(nic.id, handle);
        Ok(())
    }

    /// Stop a worker for a NIC
    pub fn stop(&mut self, nic_id: &str) -> Result<(), String> {
        if let Some(handle) = self.workers.remove(nic_id) {
            handle.stop();
            self.router.unregister_nic(nic_id);
            Ok(())
        } else {
            Err(format!("No worker for NIC {}", nic_id))
        }
    }

    /// Stop all workers
    pub fn stop_all(&mut self) {
        for (nic_id, handle) in self.workers.drain() {
            handle.stop();
            self.router.unregister_nic(&nic_id);
        }
    }

    /// Check if a worker is running
    pub fn is_running(&self, nic_id: &str) -> bool {
        self.workers.get(nic_id).is_some_and(|h| h.is_running())
    }

    /// Get list of active NIC IDs
    pub fn active_nics(&self) -> Vec<String> {
        self.workers.keys().cloned().collect()
    }

    /// Get a reference to the router (for external route management)
    pub fn router(&self) -> &Router {
        &self.router
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
