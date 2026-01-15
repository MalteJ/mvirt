//! Test backend using the real VhostNetBackend
//!
//! This module provides test infrastructure that uses the actual mvirt-net
//! implementation rather than a mock, ensuring integration tests exercise
//! the real code paths.

use std::net::Ipv4Addr;
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use nix::libc;
use tempfile::TempDir;
use vhost::vhost_user::Listener;
use vhost_user_backend::VhostUserDaemon;
use vm_memory::GuestMemoryAtomic;

// Import real mvirt-net types
use mvirt_net::config::NicEntry;
use mvirt_net::dataplane::{ArpResponder, Dhcpv4Server, VhostNetBackend};

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
        let tmp_dir = TempDir::new().expect("Failed to create temp dir");
        let socket_path = tmp_dir.path().join("test.sock");
        let socket_str = socket_path.to_string_lossy().to_string();
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = shutdown.clone();

        let mac_str = mac.to_string();
        let ipv4_str = ipv4.map(|s| s.to_string());

        let thread = thread::spawn(move || {
            run_test_backend(&socket_str, &mac_str, ipv4_str.as_deref(), shutdown_clone);
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

fn run_test_backend(socket_path: &str, mac: &str, ipv4: Option<&str>, shutdown: Arc<AtomicBool>) {
    // Create NIC entry for the real VhostNetBackend
    let nic = NicEntry {
        id: "test-nic".to_string(),
        name: Some("test".to_string()),
        network_id: "test-network".to_string(),
        mac_address: mac.to_string(),
        ipv4_address: ipv4.map(|s| s.to_string()),
        ipv6_address: None,
        socket_path: socket_path.to_string(),
        routed_ipv4_prefixes: vec![],
        routed_ipv6_prefixes: vec![],
        state: mvirt_net::config::NicState::Active,
        created_at: String::new(),
        updated_at: String::new(),
    };

    // Create the REAL VhostNetBackend
    let backend = Arc::new(
        VhostNetBackend::new(nic.clone(), shutdown.clone())
            .expect("Failed to create VhostNetBackend"),
    );

    // Set up packet handler using real ArpResponder and Dhcpv4Server
    let mac_bytes = parse_mac(mac).expect("Invalid MAC");
    let ipv4_addr: Option<Ipv4Addr> = ipv4.map(|s| s.parse().expect("Invalid IP"));

    let arp_responder = ArpResponder::new(mac_bytes);
    let dhcpv4_server = ipv4_addr.map(|ip| {
        let mut server = Dhcpv4Server::new(ip);
        server.set_dns_servers(vec![Ipv4Addr::new(1, 1, 1, 1), Ipv4Addr::new(8, 8, 8, 8)]);
        server
    });

    backend.set_packet_handler(Box::new(move |packet| {
        // Try ARP first
        if let Some(reply) = arp_responder.process(packet) {
            return Some(reply);
        }

        // Try DHCP
        if let Some(ref server) = dhcpv4_server {
            if let Some(reply) = server.process(packet, mac_bytes) {
                return Some(reply);
            }
        }

        None
    }));

    // Create listener
    let listener = Listener::new(socket_path, true).expect("Failed to create listener");

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

        if let Err(e) = daemon.start(listener) {
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
