//! Test backend using the real VhostNetBackend
//!
//! This module provides test infrastructure that uses the actual mvirt-net
//! implementation rather than a mock, ensuring integration tests exercise
//! the real code paths.

use std::net::{Ipv4Addr, Ipv6Addr};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossbeam_channel::Receiver;
use ipnet::{Ipv4Net, Ipv6Net};
use nix::libc;
use tempfile::TempDir;
use vhost::vhost_user::Listener;
use vhost_user_backend::VhostUserDaemon;
use vm_memory::GuestMemoryAtomic;

// Import real mvirt-net types
use mvirt_net::config::NicEntry;
use mvirt_net::dataplane::{
    ArpResponder, BufferPool, Dhcpv4Server, Dhcpv6Server, IcmpResponder, Icmpv6Responder,
    InboundPacket, NdpResponder, NetworkRouter, NicChannel, VhostNetBackend,
};

use super::VhostTestClient;

/// Gateway IP as byte array for test assertions
pub const GATEWAY_IP: [u8; 4] = [169, 254, 0, 1];

/// Test backend that spawns a vhost-user daemon using the real VhostNetBackend.
///
/// # Example
///
/// ```ignore
/// let backend = TestBackend::new("52:54:00:12:34:56", Some("10.0.0.5"));
/// let mut client = backend.connect().expect("connect failed");
/// // ... run tests
/// ```
pub struct TestBackend {
    _tmp_dir: TempDir,
    socket_path: PathBuf,
    shutdown: Arc<AtomicBool>,
    _thread: thread::JoinHandle<()>,
}

impl TestBackend {
    /// Create a new test backend with the given MAC and optional IPv4 address.
    ///
    /// If `ipv4` is provided, the backend will respond to ARP and DHCP requests
    /// using the real ArpResponder and Dhcpv4Server implementations.
    pub fn new(mac: &str, ipv4: Option<&str>) -> Self {
        Self::new_with_ipv6(mac, ipv4, None)
    }

    /// Create a new test backend with IPv4 and IPv6 support.
    ///
    /// If `ipv6` is provided, the backend will respond to NDP and DHCPv6 requests.
    pub fn new_with_ipv6(mac: &str, ipv4: Option<&str>, ipv6: Option<&str>) -> Self {
        let tmp_dir = TempDir::new().expect("Failed to create temp dir");
        let socket_path = tmp_dir.path().join("test.sock");
        let socket_str = socket_path.to_string_lossy().to_string();
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = shutdown.clone();

        let mac_str = mac.to_string();
        let ipv4_str = ipv4.map(|s| s.to_string());
        let ipv6_str = ipv6.map(|s| s.to_string());

        let thread = thread::spawn(move || {
            run_test_backend(
                &socket_str,
                &mac_str,
                ipv4_str.as_deref(),
                ipv6_str.as_deref(),
                shutdown_clone,
            );
        });

        // Wait for socket to appear
        for _ in 0..50 {
            if socket_path.exists() {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }

        Self {
            _tmp_dir: tmp_dir,
            socket_path,
            shutdown,
            _thread: thread,
        }
    }

    /// Connect a test client to this backend.
    pub fn connect(&self) -> std::io::Result<VhostTestClient> {
        VhostTestClient::connect(&self.socket_path)
    }
}

impl Drop for TestBackend {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

fn parse_mac(mac: &str) -> Option<[u8; 6]> {
    let parts: Vec<&str> = mac.split(':').collect();
    if parts.len() != 6 {
        return None;
    }

    let mut bytes = [0u8; 6];
    for (i, part) in parts.iter().enumerate() {
        bytes[i] = u8::from_str_radix(part, 16).ok()?;
    }
    Some(bytes)
}

fn run_test_backend(
    socket_path: &str,
    mac: &str,
    ipv4: Option<&str>,
    ipv6: Option<&str>,
    shutdown: Arc<AtomicBool>,
) {
    // Create buffer pool for zero-copy packet processing
    let pool = Arc::new(BufferPool::new().expect("Failed to create buffer pool"));

    // Create NIC entry for the real VhostNetBackend
    let nic = NicEntry {
        id: "test-nic".to_string(),
        name: Some("test".to_string()),
        network_id: "test-network".to_string(),
        mac_address: mac.to_string(),
        ipv4_address: ipv4.map(|s| s.to_string()),
        ipv6_address: ipv6.map(|s| s.to_string()),
        socket_path: socket_path.to_string(),
        routed_ipv4_prefixes: vec![],
        routed_ipv6_prefixes: vec![],
        state: mvirt_net::config::NicState::Active,
        created_at: String::new(),
        updated_at: String::new(),
    };

    // Create the REAL VhostNetBackend
    let backend = Arc::new(
        VhostNetBackend::new(nic.clone(), shutdown.clone(), pool)
            .expect("Failed to create VhostNetBackend"),
    );

    // Set up packet handler using real protocol handlers
    let mac_bytes = parse_mac(mac).expect("Invalid MAC");
    let ipv4_addr: Option<Ipv4Addr> = ipv4.map(|s| s.parse().expect("Invalid IPv4"));
    let ipv6_addr: Option<Ipv6Addr> = ipv6.map(|s| s.parse().expect("Invalid IPv6"));

    // IPv4 handlers
    let arp_responder = ArpResponder::new(mac_bytes);
    let icmp_responder = IcmpResponder::new();
    let dhcpv4_server = ipv4_addr.map(|ip| {
        let mut server = Dhcpv4Server::new(ip, true);
        server.set_dns_servers(vec![Ipv4Addr::new(1, 1, 1, 1), Ipv4Addr::new(8, 8, 8, 8)]);
        server
    });

    // IPv6 handlers
    let ndp_responder = NdpResponder::new(mac_bytes, true);
    let icmpv6_responder = Icmpv6Responder::new();
    let dhcpv6_server = ipv6_addr.map(|ip| {
        let mut server = Dhcpv6Server::new(ip);
        server.set_dns_servers(vec![
            Ipv6Addr::new(0x2606, 0x4700, 0x4700, 0, 0, 0, 0, 0x1111), // Cloudflare
            Ipv6Addr::new(0x2001, 0x4860, 0x4860, 0, 0, 0, 0, 0x8888), // Google
        ]);
        server
    });

    // Clone backend for use in packet handler
    let backend_for_handler = backend.clone();

    backend.set_packet_handler(Box::new(move |buffer, _virtio_hdr| {
        let packet = buffer.data();

        // Try ARP first (IPv4)
        if let Some(reply) = arp_responder.process(packet) {
            backend_for_handler.inject_vec(reply);
            return;
        }

        // Try ICMP (ping to gateway, IPv4)
        if let Some(reply) = icmp_responder.process(packet) {
            backend_for_handler.inject_vec(reply);
            return;
        }

        // Try DHCPv4
        if let Some(ref server) = dhcpv4_server
            && let Some(reply) = server.process(packet, mac_bytes)
        {
            backend_for_handler.inject_vec(reply);
            return;
        }

        // Try NDP (IPv6)
        if let Some(reply) = ndp_responder.process(packet) {
            backend_for_handler.inject_vec(reply);
            return;
        }

        // Try ICMPv6 (ping to gateway)
        if let Some(reply) = icmpv6_responder.process(packet) {
            backend_for_handler.inject_vec(reply);
            return;
        }

        // Try DHCPv6
        if let Some(ref server) = dhcpv6_server
            && let Some(reply) = server.process(packet, mac_bytes)
        {
            backend_for_handler.inject_vec(reply);
            return;
        }

        // No protocol matched - for test backend, we just drop the packet
        // (real backend would route it)
    }));

    // Create listener
    let mut listener = Listener::new(socket_path, true).expect("Failed to create listener");

    eprintln!("[TEST BACKEND] Listening on {}", socket_path);

    let mut daemon = VhostUserDaemon::new(
        "test-backend".to_string(),
        backend,
        GuestMemoryAtomic::new(vm_memory::GuestMemoryMmap::new()),
    )
    .expect("Failed to create daemon");

    // Wait for connection with polling
    while !shutdown.load(Ordering::SeqCst) {
        let mut pollfd = libc::pollfd {
            fd: listener.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };

        let ret = unsafe { libc::poll(&mut pollfd, 1, 500) };
        if ret <= 0 {
            continue;
        }

        eprintln!("[TEST BACKEND] Accepting connection...");

        if let Err(e) = daemon.start(&mut listener) {
            eprintln!("[TEST BACKEND] Start error: {}", e);
            break;
        }

        // Wait for shutdown or disconnect
        loop {
            if shutdown.load(Ordering::SeqCst) {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }

        break;
    }

    eprintln!("[TEST BACKEND] Shutting down");
}

// ============================================================================
// Routing Test Backend - Two vNICs with shared NetworkRouter
// ============================================================================

/// Configuration for a vNIC in the routing test
pub struct RoutingNicConfig {
    pub id: String,
    pub mac: String,
    pub ipv4: String,
    pub ipv6: Option<String>,
}

/// Test backend for inter-vNIC routing tests.
///
/// Sets up two vNICs in the same network with a shared NetworkRouter.
/// Packets sent from one vNIC to the other's IP will be routed through
/// the NetworkRouter and delivered to the destination vNIC.
pub struct RoutingTestBackend {
    _tmp_dir: TempDir,
    socket_path_a: PathBuf,
    socket_path_b: PathBuf,
    shutdown: Arc<AtomicBool>,
    _thread_a: JoinHandle<()>,
    _thread_b: JoinHandle<()>,
    _rx_thread_a: JoinHandle<()>,
    _rx_thread_b: JoinHandle<()>,
}

impl RoutingTestBackend {
    /// Create a new routing test backend with two vNICs.
    ///
    /// # Arguments
    /// * `nic_a` - Configuration for the first vNIC
    /// * `nic_b` - Configuration for the second vNIC
    pub fn new(nic_a: RoutingNicConfig, nic_b: RoutingNicConfig) -> Self {
        let tmp_dir = TempDir::new().expect("Failed to create temp dir");
        let socket_path_a = tmp_dir.path().join("nic_a.sock");
        let socket_path_b = tmp_dir.path().join("nic_b.sock");
        let shutdown = Arc::new(AtomicBool::new(false));

        // Create buffer pool for zero-copy packet processing
        let pool = Arc::new(BufferPool::new().expect("Failed to create buffer pool"));

        // Create shared router for this network (non-public for tests)
        let router = NetworkRouter::new("test-network".to_string(), false);

        // Create channels for each NIC
        let (tx_a, rx_a) = crossbeam_channel::unbounded();
        let (tx_b, rx_b) = crossbeam_channel::unbounded();

        // Parse MAC addresses
        let mac_a = parse_mac(&nic_a.mac).expect("Invalid MAC for NIC A");
        let mac_b = parse_mac(&nic_b.mac).expect("Invalid MAC for NIC B");

        // Register NICs with router
        router.register_nic(
            nic_a.id.clone(),
            NicChannel {
                sender: tx_a,
                mac: mac_a,
            },
        );
        router.register_nic(
            nic_b.id.clone(),
            NicChannel {
                sender: tx_b,
                mac: mac_b,
            },
        );

        // Add IPv4 routes for each NIC's IP
        let ip_a: Ipv4Addr = nic_a.ipv4.parse().expect("Invalid IPv4 for NIC A");
        let ip_b: Ipv4Addr = nic_b.ipv4.parse().expect("Invalid IPv4 for NIC B");

        router.add_ipv4_route(Ipv4Net::new(ip_a, 32).unwrap(), nic_a.id.clone(), true);
        router.add_ipv4_route(Ipv4Net::new(ip_b, 32).unwrap(), nic_b.id.clone(), true);

        // Add IPv6 routes if configured
        if let Some(ref ipv6_a) = nic_a.ipv6 {
            let addr: Ipv6Addr = ipv6_a.parse().expect("Invalid IPv6 for NIC A");
            router.add_ipv6_route(Ipv6Net::new(addr, 128).unwrap(), nic_a.id.clone(), true);
        }
        if let Some(ref ipv6_b) = nic_b.ipv6 {
            let addr: Ipv6Addr = ipv6_b.parse().expect("Invalid IPv6 for NIC B");
            router.add_ipv6_route(Ipv6Net::new(addr, 128).unwrap(), nic_b.id.clone(), true);
        }

        // Create backends
        let backend_a = Arc::new(
            VhostNetBackend::new(
                NicEntry {
                    id: nic_a.id.clone(),
                    name: Some("test-a".to_string()),
                    network_id: "test-network".to_string(),
                    mac_address: nic_a.mac.clone(),
                    ipv4_address: Some(nic_a.ipv4.clone()),
                    ipv6_address: None,
                    socket_path: socket_path_a.to_string_lossy().to_string(),
                    routed_ipv4_prefixes: vec![],
                    routed_ipv6_prefixes: vec![],
                    state: mvirt_net::config::NicState::Active,
                    created_at: String::new(),
                    updated_at: String::new(),
                },
                shutdown.clone(),
                pool.clone(),
            )
            .expect("Failed to create backend A"),
        );

        let backend_b = Arc::new(
            VhostNetBackend::new(
                NicEntry {
                    id: nic_b.id.clone(),
                    name: Some("test-b".to_string()),
                    network_id: "test-network".to_string(),
                    mac_address: nic_b.mac.clone(),
                    ipv4_address: Some(nic_b.ipv4.clone()),
                    ipv6_address: None,
                    socket_path: socket_path_b.to_string_lossy().to_string(),
                    routed_ipv4_prefixes: vec![],
                    routed_ipv6_prefixes: vec![],
                    state: mvirt_net::config::NicState::Active,
                    created_at: String::new(),
                    updated_at: String::new(),
                },
                shutdown.clone(),
                pool.clone(),
            )
            .expect("Failed to create backend B"),
        );

        // Set up packet handlers with routing
        let router_a = router.clone();
        let mac_a = parse_mac(&nic_a.mac).expect("Invalid MAC A");
        let arp_a = ArpResponder::new(mac_a);
        let icmp_a = IcmpResponder::new();
        let nic_id_a = nic_a.id.clone();
        let backend_a_for_handler = backend_a.clone();

        backend_a.set_packet_handler(Box::new(move |buffer, virtio_hdr| {
            let packet = buffer.data();

            // Try ARP
            if let Some(reply) = arp_a.process(packet) {
                backend_a_for_handler.inject_vec(reply);
                return;
            }
            // Try ICMP (ping to gateway)
            if let Some(reply) = icmp_a.process(packet) {
                backend_a_for_handler.inject_vec(reply);
                return;
            }
            // Try routing to other vNIC (consumes buffer)
            router_a.route_packet(&nic_id_a, buffer, virtio_hdr);
        }));

        let router_b = router.clone();
        let mac_b = parse_mac(&nic_b.mac).expect("Invalid MAC B");
        let arp_b = ArpResponder::new(mac_b);
        let icmp_b = IcmpResponder::new();
        let nic_id_b = nic_b.id.clone();
        let backend_b_for_handler = backend_b.clone();

        backend_b.set_packet_handler(Box::new(move |buffer, virtio_hdr| {
            let packet = buffer.data();

            // Try ARP
            if let Some(reply) = arp_b.process(packet) {
                backend_b_for_handler.inject_vec(reply);
                return;
            }
            // Try ICMP (ping to gateway)
            if let Some(reply) = icmp_b.process(packet) {
                backend_b_for_handler.inject_vec(reply);
                return;
            }
            // Try routing to other vNIC (consumes buffer)
            router_b.route_packet(&nic_id_b, buffer, virtio_hdr);
        }));

        // Spawn RX injection threads
        let backend_a_rx = backend_a.clone();
        let shutdown_a_rx = shutdown.clone();
        let rx_thread_a = thread::Builder::new()
            .name("rx-a".to_string())
            .spawn(move || {
                run_rx_injection(rx_a, backend_a_rx, shutdown_a_rx);
            })
            .expect("Failed to spawn RX thread A");

        let backend_b_rx = backend_b.clone();
        let shutdown_b_rx = shutdown.clone();
        let rx_thread_b = thread::Builder::new()
            .name("rx-b".to_string())
            .spawn(move || {
                run_rx_injection(rx_b, backend_b_rx, shutdown_b_rx);
            })
            .expect("Failed to spawn RX thread B");

        // Spawn daemon threads
        let socket_a = socket_path_a.clone();
        let shutdown_a = shutdown.clone();
        let thread_a = thread::Builder::new()
            .name("daemon-a".to_string())
            .spawn(move || {
                run_routing_backend(&socket_a, backend_a, shutdown_a);
            })
            .expect("Failed to spawn thread A");

        let socket_b = socket_path_b.clone();
        let shutdown_b = shutdown.clone();
        let thread_b = thread::Builder::new()
            .name("daemon-b".to_string())
            .spawn(move || {
                run_routing_backend(&socket_b, backend_b, shutdown_b);
            })
            .expect("Failed to spawn thread B");

        // Wait for sockets to appear
        for _ in 0..50 {
            if socket_path_a.exists() && socket_path_b.exists() {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }

        Self {
            _tmp_dir: tmp_dir,
            socket_path_a,
            socket_path_b,
            shutdown,
            _thread_a: thread_a,
            _thread_b: thread_b,
            _rx_thread_a: rx_thread_a,
            _rx_thread_b: rx_thread_b,
        }
    }

    /// Connect a test client to vNIC A
    pub fn connect_a(&self) -> std::io::Result<VhostTestClient> {
        VhostTestClient::connect(&self.socket_path_a)
    }

    /// Connect a test client to vNIC B
    pub fn connect_b(&self) -> std::io::Result<VhostTestClient> {
        VhostTestClient::connect(&self.socket_path_b)
    }
}

impl Drop for RoutingTestBackend {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

/// RX injection thread for routing tests
fn run_rx_injection(
    rx_channel: Receiver<InboundPacket>,
    backend: Arc<VhostNetBackend>,
    shutdown: Arc<AtomicBool>,
) {
    loop {
        if shutdown.load(Ordering::SeqCst) {
            break;
        }

        match rx_channel.recv_timeout(Duration::from_millis(100)) {
            Ok(packet) => {
                eprintln!(
                    "[RX INJECT] Injecting packet of {} bytes",
                    packet.buffer.len
                );
                // Zero-copy injection - pass buffer and virtio header directly
                backend.inject_buffer_and_deliver(packet.buffer, packet.virtio_hdr);
                // Flush the RX queue to deliver packets to guest (interrupt coalescing)
                let _ = backend.flush_rx_queue();
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// Run a routing backend daemon
fn run_routing_backend(
    socket_path: &Path,
    backend: Arc<VhostNetBackend>,
    shutdown: Arc<AtomicBool>,
) {
    let mut listener = Listener::new(socket_path.to_string_lossy().as_ref(), true)
        .expect("Failed to create listener");

    eprintln!("[ROUTING BACKEND] Listening on {}", socket_path.display());

    let mut daemon = VhostUserDaemon::new(
        "routing-backend".to_string(),
        backend,
        GuestMemoryAtomic::new(vm_memory::GuestMemoryMmap::new()),
    )
    .expect("Failed to create daemon");

    // Wait for connection with polling
    while !shutdown.load(Ordering::SeqCst) {
        let mut pollfd = libc::pollfd {
            fd: listener.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };

        let ret = unsafe { libc::poll(&mut pollfd, 1, 500) };
        if ret <= 0 {
            continue;
        }

        eprintln!("[ROUTING BACKEND] Accepting connection...");

        if let Err(e) = daemon.start(&mut listener) {
            eprintln!("[ROUTING BACKEND] Start error: {}", e);
            break;
        }

        // Wait for shutdown or disconnect
        loop {
            if shutdown.load(Ordering::SeqCst) {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }

        break;
    }

    eprintln!("[ROUTING BACKEND] Shutting down");
}
