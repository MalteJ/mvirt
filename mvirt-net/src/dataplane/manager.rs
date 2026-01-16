//! Reactor manager for coordinating multiple reactors
//!
//! Manages the lifecycle of vNIC and TUN reactors.

use std::collections::HashMap;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use arc_swap::ArcSwap;
use ipnet::{Ipv4Net, Ipv6Net};
use nix::libc;
use tracing::{debug, info, warn};
use vhost::vhost_user::Listener;
use vhost_user_backend::VhostUserDaemon;
use vm_memory::GuestMemoryAtomic;

use crate::config::{NetworkEntry, NicEntry};

use super::backend::{TunBackend, VhostBackend};
use super::buffer::BufferPool;
use super::reactor::{Reactor, ReactorConfig, ReactorRegistry};
use super::router::NetworkRouter;
use super::tun::TunDevice;
use super::vhost::{VhostNetBackend, parse_mac};

/// Handle to a running reactor
pub struct ReactorHandle {
    /// Shutdown signal sender (for Reactor thread)
    shutdown: crossbeam_channel::Sender<()>,
    /// Shutdown flag (for VhostUserDaemon thread)
    shutdown_flag: Arc<AtomicBool>,
    /// Reactor thread handle
    reactor_thread: Option<JoinHandle<()>>,
    /// VhostUserDaemon thread handle
    daemon_thread: Option<JoinHandle<()>>,
    /// Reactor ID (NIC ID)
    pub id: String,
    /// Socket path
    pub socket_path: PathBuf,
    /// Network ID
    pub network_id: String,
}

impl ReactorHandle {
    /// Signal the reactor to stop
    pub fn stop(&self) {
        let _ = self.shutdown.send(());
        self.shutdown_flag.store(true, Ordering::SeqCst);
    }

    /// Check if still running
    pub fn is_running(&self) -> bool {
        self.reactor_thread
            .as_ref()
            .is_some_and(|h| !h.is_finished())
            || self
                .daemon_thread
                .as_ref()
                .is_some_and(|h| !h.is_finished())
    }
}

impl Drop for ReactorHandle {
    fn drop(&mut self) {
        self.stop();
        // Join both threads
        if let Some(handle) = self.reactor_thread.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.daemon_thread.take() {
            let _ = handle.join();
        }
        // Clean up socket
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

/// Shared routers (lock-free reads via ArcSwap)
type SharedRouters = Arc<ArcSwap<HashMap<String, NetworkRouter>>>;

/// Manages all reactors (vNIC + TUN)
pub struct ReactorManager {
    /// Active vNIC reactors by NIC ID
    reactors: HashMap<String, ReactorHandle>,
    /// Per-network routers
    routers: SharedRouters,
    /// Shared registry for inter-reactor communication
    registry: Arc<ReactorRegistry>,
    /// TUN reactor shutdown signal
    tun_shutdown: Option<crossbeam_channel::Sender<()>>,
    /// TUN reactor thread
    tun_thread: Option<JoinHandle<()>>,
    /// Shared buffer pool (used when spawning vNIC reactors)
    #[allow(dead_code)]
    pool: Arc<BufferPool>,
}

impl ReactorManager {
    /// Create a new manager with TUN reactor
    pub fn new() -> Result<Self, String> {
        info!("Creating ReactorManager with TUN device");

        // Create buffer pool
        let pool =
            Arc::new(BufferPool::new().map_err(|e| format!("Failed to create buffer pool: {e}"))?);

        // Create TUN device
        let tun = TunDevice::new().map_err(|e| format!("Failed to create TUN device: {e}"))?;
        tun.bring_up()
            .map_err(|e| format!("Failed to bring up TUN device: {e}"))?;

        if let Err(e) = tun.enable_offload() {
            warn!(error = %e, "Failed to enable TUN offload");
        }

        info!(name = %tun.name(), "TUN device created");

        // Create TUN backend
        let tun_backend =
            TunBackend::new(tun).map_err(|e| format!("Failed to create TUN backend: {e}"))?;

        // Create shared registry and routers
        let registry = Arc::new(ReactorRegistry::new());
        let routers: SharedRouters = Arc::new(ArcSwap::from_pointee(HashMap::new()));

        // Create TUN reactor (Layer 3 only)
        let tun_config = ReactorConfig::tun("tun".to_string(), "global".to_string());
        let tun_router = NetworkRouter::new("global".to_string(), true);

        let (shutdown_tx, shutdown_rx) = crossbeam_channel::bounded(1);

        let (mut tun_reactor, tun_sender) = Reactor::new(
            tun_backend,
            tun_config,
            pool.clone(),
            registry.clone(),
            tun_router,
            shutdown_rx,
        );

        // Register TUN globally
        registry.register_tun("global".to_string(), tun_sender);

        // Spawn TUN reactor thread
        let tun_thread = std::thread::Builder::new()
            .name("tun-reactor".to_string())
            .spawn(move || {
                tun_reactor.run();
            })
            .map_err(|e| format!("Failed to spawn TUN reactor: {e}"))?;

        Ok(Self {
            reactors: HashMap::new(),
            routers,
            registry,
            tun_shutdown: Some(shutdown_tx),
            tun_thread: Some(tun_thread),
            pool,
        })
    }

    /// Start a reactor for a vNIC
    pub fn start(&mut self, nic: NicEntry, network: NetworkEntry) -> Result<(), String> {
        if self.reactors.contains_key(&nic.id) {
            return Err(format!("Reactor for NIC {} already running", nic.id));
        }

        // Get or create router for this network (used when spawning vNIC reactors)
        let _router = {
            let current = self.routers.load();
            if let Some(existing) = current.get(&network.id) {
                existing.clone()
            } else {
                let new_router = NetworkRouter::new(network.id.clone(), network.is_public);
                let mut new_map = (**current).clone();
                new_map.insert(network.id.clone(), new_router.clone());
                self.routers.store(Arc::new(new_map));
                new_router
            }
        };

        // Register TUN for this network if public
        if network.is_public
            && let Some(tun_sender) = self.registry.get_tun("global")
        {
            self.registry.register_tun(network.id.clone(), tun_sender);
        }

        // 1. Parse MAC address
        let mac = parse_mac(&nic.mac_address)
            .ok_or_else(|| format!("Invalid MAC address: {}", nic.mac_address))?;

        // 2. Parse IP addresses (optional)
        let ipv4_addr = nic
            .ipv4_address
            .as_ref()
            .map(|s| s.parse())
            .transpose()
            .map_err(|e| format!("Invalid IPv4 address: {e}"))?
            .unwrap_or(std::net::Ipv4Addr::UNSPECIFIED);
        let ipv6_addr = nic
            .ipv6_address
            .as_ref()
            .map(|s| s.parse())
            .transpose()
            .map_err(|e| format!("Invalid IPv6 address: {e}"))?
            .unwrap_or(std::net::Ipv6Addr::UNSPECIFIED);

        // 3. Create shutdown mechanisms
        let (shutdown_tx, shutdown_rx) = crossbeam_channel::bounded(1);
        let shutdown_flag = Arc::new(AtomicBool::new(false));

        // 4. Create VhostNetBackend
        let vhost_net_backend = Arc::new(
            VhostNetBackend::new(nic.clone(), shutdown_flag.clone(), self.pool.clone())
                .map_err(|e| format!("Failed to create VhostNetBackend: {e}"))?,
        );

        // 5. Create VhostBackend wrapper + packet channel
        let (vhost_backend, packet_sender) = VhostBackend::new(vhost_net_backend.clone());

        // 6. Set packet handler to forward guest TX to VhostBackend channel
        vhost_net_backend.set_packet_handler(Box::new(move |buffer, virtio_hdr| {
            let _ = packet_sender.try_send((buffer, virtio_hdr));
        }));

        // 7. Create ReactorConfig
        let reactor_config = ReactorConfig::vnic(
            nic.id.clone(),
            network.id.clone(),
            mac,
            ipv4_addr,
            ipv6_addr,
            network.is_public,
        );

        // 8. Use router from earlier (rename _router to router)
        let router = _router;

        // 9. Create Reactor
        let (mut reactor, reactor_sender) = Reactor::new(
            vhost_backend,
            reactor_config,
            self.pool.clone(),
            self.registry.clone(),
            router.clone(),
            shutdown_rx,
        );

        // 10. Register NIC with registry
        self.registry
            .register_nic(mac, nic.id.clone(), reactor_sender);

        // 11. Add routes for NIC's IPs
        if !ipv4_addr.is_unspecified() {
            router.add_ipv4_route(
                Ipv4Net::new(ipv4_addr, 32).expect("Valid /32 prefix"),
                nic.id.clone(),
                true,
            );
        }
        if !ipv6_addr.is_unspecified() {
            router.add_ipv6_route(
                Ipv6Net::new(ipv6_addr, 128).expect("Valid /128 prefix"),
                nic.id.clone(),
                true,
            );
        }

        // 12. Spawn VhostUserDaemon thread
        let socket_path = PathBuf::from(&nic.socket_path);
        let vhost_for_daemon = vhost_net_backend;
        let shutdown_for_daemon = shutdown_flag.clone();
        let nic_id_daemon = nic.id.clone();

        let daemon_thread = std::thread::Builder::new()
            .name(format!("vhost-{}", nic.id))
            .spawn(move || {
                run_vhost_daemon(
                    &socket_path,
                    vhost_for_daemon,
                    shutdown_for_daemon,
                    &nic_id_daemon,
                );
            })
            .map_err(|e| format!("Failed to spawn vhost daemon: {e}"))?;

        // 13. Spawn Reactor thread
        let nic_id_reactor = nic.id.clone();
        let reactor_thread = std::thread::Builder::new()
            .name(format!("reactor-{}", nic.id))
            .spawn(move || {
                reactor.run();
                debug!(nic_id = %nic_id_reactor, "Reactor thread stopped");
            })
            .map_err(|e| format!("Failed to spawn reactor: {e}"))?;

        // 14. Create ReactorHandle
        let handle = ReactorHandle {
            shutdown: shutdown_tx,
            shutdown_flag,
            reactor_thread: Some(reactor_thread),
            daemon_thread: Some(daemon_thread),
            id: nic.id.clone(),
            socket_path: PathBuf::from(&nic.socket_path),
            network_id: network.id,
        };

        self.reactors.insert(nic.id.clone(), handle);
        info!(nic_id = %nic.id, "vNIC reactor started");
        Ok(())
    }

    /// Stop a reactor for a vNIC
    pub fn stop(&mut self, nic_id: &str) -> Result<(), String> {
        if let Some(handle) = self.reactors.remove(nic_id) {
            handle.stop();
            // Unregister from router
            let routers = self.routers.load();
            if let Some(router) = routers.get(&handle.network_id) {
                router.unregister_nic(nic_id);
            }
            Ok(())
        } else {
            Err(format!("No reactor for NIC {}", nic_id))
        }
    }

    /// Stop all reactors
    pub fn stop_all(&mut self) {
        // Stop vNIC reactors
        for (nic_id, handle) in self.reactors.drain() {
            handle.stop();
            let routers = self.routers.load();
            if let Some(router) = routers.get(&handle.network_id) {
                router.unregister_nic(&nic_id);
            }
        }

        // Stop TUN reactor
        if let Some(shutdown) = self.tun_shutdown.take() {
            info!("Stopping TUN reactor");
            let _ = shutdown.send(());
        }
        if let Some(thread) = self.tun_thread.take() {
            let _ = thread.join();
        }
    }

    /// Remove a network's router
    pub fn remove_network(&mut self, network_id: &str) {
        let current = self.routers.load();
        let mut new_map = (**current).clone();
        new_map.remove(network_id);
        self.routers.store(Arc::new(new_map));
    }

    /// Check if a reactor is running
    pub fn is_running(&self, nic_id: &str) -> bool {
        self.reactors.get(nic_id).is_some_and(|h| h.is_running())
    }

    /// Get list of active NIC IDs
    pub fn active_nics(&self) -> Vec<String> {
        self.reactors.keys().cloned().collect()
    }

    /// Get router for a network
    pub fn router(&self, network_id: &str) -> Option<NetworkRouter> {
        self.routers.load().get(network_id).cloned()
    }
}

impl Drop for ReactorManager {
    fn drop(&mut self) {
        self.stop_all();
    }
}

/// Run the vhost-user daemon for a vNIC
///
/// This function handles the vhost-user protocol with the guest VM.
/// It runs until the shutdown flag is set.
fn run_vhost_daemon(
    socket_path: &Path,
    backend: Arc<VhostNetBackend>,
    shutdown: Arc<AtomicBool>,
    nic_id: &str,
) {
    let mut listener = match Listener::new(socket_path.to_string_lossy().as_ref(), true) {
        Ok(l) => l,
        Err(e) => {
            warn!(nic_id, path = %socket_path.display(), error = %e, "Failed to create vhost listener");
            return;
        }
    };

    let mut daemon = match VhostUserDaemon::new(
        format!("mvirt-{}", nic_id),
        backend,
        GuestMemoryAtomic::new(vm_memory::GuestMemoryMmap::new()),
    ) {
        Ok(d) => d,
        Err(e) => {
            warn!(nic_id, error = %e, "Failed to create VhostUserDaemon");
            return;
        }
    };

    info!(nic_id, path = %socket_path.display(), "vhost-user daemon listening");

    // Poll for connections with periodic shutdown checks
    while !shutdown.load(Ordering::SeqCst) {
        let mut pollfd = libc::pollfd {
            fd: listener.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };

        // Poll with 500ms timeout to allow checking shutdown flag
        let ret = unsafe { libc::poll(&mut pollfd, 1, 500) };
        if ret <= 0 {
            continue;
        }

        debug!(nic_id, "Accepting vhost-user connection");

        if let Err(e) = daemon.start(&mut listener) {
            warn!(nic_id, error = %e, "VhostUserDaemon start error");
            break;
        }

        // Wait for shutdown or disconnect
        while !shutdown.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(100));
        }
        break;
    }

    debug!(nic_id, "vhost-user daemon stopped");
}
