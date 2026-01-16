//! Reactor manager for coordinating multiple reactors
//!
//! Manages the lifecycle of vNIC and TUN reactors.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;

use arc_swap::ArcSwap;
use tracing::{info, warn};

use crate::config::{NetworkEntry, NicEntry};

use super::backend::TunBackend;
use super::buffer::BufferPool;
use super::reactor::{Reactor, ReactorConfig, ReactorRegistry};
use super::router::NetworkRouter;
use super::tun::TunDevice;

/// Handle to a running reactor
pub struct ReactorHandle {
    /// Shutdown signal sender
    shutdown: crossbeam_channel::Sender<()>,
    /// Thread handle
    thread: Option<JoinHandle<()>>,
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
    }

    /// Check if still running
    pub fn is_running(&self) -> bool {
        self.thread.as_ref().is_some_and(|h| !h.is_finished())
    }
}

impl Drop for ReactorHandle {
    fn drop(&mut self) {
        self.stop();
        if let Some(handle) = self.thread.take() {
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

        // TODO: Create VhostBackend and spawn vNIC reactor
        // For now, just create a placeholder handle
        let (shutdown_tx, _shutdown_rx) = crossbeam_channel::bounded(1);

        let handle = ReactorHandle {
            shutdown: shutdown_tx,
            thread: None, // TODO: spawn actual reactor
            id: nic.id.clone(),
            socket_path: PathBuf::from(&nic.socket_path),
            network_id: network.id,
        };

        self.reactors.insert(nic.id.clone(), handle);
        warn!(nic_id = %nic.id, "VhostBackend not yet implemented - vNIC reactor placeholder only");
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
