//! NetworkManager - Router lifecycle management for networks and NICs.

use super::storage::{NetworkData, NicData, Storage};
use crate::reactor::{ReactorId, ReactorRegistry};
use crate::router::{Router, VhostConfig};
use crate::routing::{IpPrefix, RouteTarget};
use ipnet::{Ipv4Net, Ipv6Net};
use nix::libc;
use std::collections::HashMap;
use std::io;
use std::net::Ipv4Addr;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Directory for vhost-user sockets.
const SOCKET_DIR: &str = "/run/mvirt/net";

/// Manager errors.
#[derive(Debug, Error)]
pub enum ManagerError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("Storage error: {0}")]
    Storage(#[from] super::storage::StorageError),

    #[error("Network not found: {0}")]
    NetworkNotFound(String),

    #[error("NIC not found: {0}")]
    NicNotFound(String),

    #[error("Router creation failed: {0}")]
    RouterCreationFailed(String),

    #[error("TUN device not available")]
    TunNotAvailable,
}

pub type Result<T> = std::result::Result<T, ManagerError>;

/// Managed NIC with its router instance.
struct ManagedNic {
    #[allow(dead_code)]
    data: NicData,
    router: Router,
}

/// NetworkManager manages the lifecycle of routers for networks and NICs.
///
/// - Creates vhost routers for NICs
/// - Configures routing tables for VM-to-VM and TUN routing
/// - Manages the global TUN device for public networks
pub struct NetworkManager {
    /// Shared reactor registry for all reactors
    registry: Arc<ReactorRegistry>,
    /// Storage backend
    storage: Arc<Storage>,
    /// Global TUN router (for public network routing)
    tun_router: Mutex<Option<Router>>,
    /// TUN routing table ID
    tun_table_id: Mutex<Option<Uuid>>,
    /// Managed NICs by ID
    nics: Mutex<HashMap<Uuid, ManagedNic>>,
}

impl NetworkManager {
    /// Create a new NetworkManager.
    pub fn new(storage: Arc<Storage>) -> Self {
        let registry = Arc::new(ReactorRegistry::new());

        Self {
            registry,
            storage,
            tun_router: Mutex::new(None),
            tun_table_id: Mutex::new(None),
            nics: Mutex::new(HashMap::new()),
        }
    }

    /// Get a reference to the reactor registry.
    pub fn registry(&self) -> &Arc<ReactorRegistry> {
        &self.registry
    }

    /// Initialize the global TUN device for public networks.
    ///
    /// This creates a TUN device that serves as the default gateway for all public networks.
    /// The TUN's routing table receives routes for all public network subnets.
    pub async fn init_tun(&self, tun_name: &str) -> Result<()> {
        let mut tun_guard = self.tun_router.lock().await;
        if tun_guard.is_some() {
            info!("TUN already initialized");
            return Ok(());
        }

        info!(name = %tun_name, "Initializing global TUN device");

        let router = Router::with_shared_registry(
            tun_name,
            None, // No IP address on TUN device
            4096,
            4096,
            4096,
            None,
            Arc::clone(&self.registry),
        )
        .await
        .map_err(|e| ManagerError::RouterCreationFailed(e.to_string()))?;

        // Create routing table for TUN
        let table_id = Uuid::new_v4();
        router.reactor_handle().create_table(table_id, "tun-routes");
        router.reactor_handle().set_default_table(table_id);

        // Store table ID
        {
            let mut table_guard = self.tun_table_id.lock().await;
            *table_guard = Some(table_id);
        }

        *tun_guard = Some(router);

        // Release lock before syncing routes (sync_public_network_routes needs the lock)
        drop(tun_guard);

        // Add routes for existing public networks
        self.sync_public_network_routes().await?;

        Ok(())
    }

    /// Recover NIC routers from database on startup.
    ///
    /// This recreates routers for all NICs that exist in the database,
    /// ensuring that vhost-user sockets are available after a restart.
    pub async fn recover_nics(&self) -> Result<()> {
        info!("Recovering NIC routers from database...");

        let nics = self.storage.list_nics()?;
        let mut recovered = 0;
        let mut failed = 0;

        for nic in nics {
            // Get the network for this NIC
            let network = match self.storage.get_network_by_id(&nic.network_id) {
                Ok(Some(n)) => n,
                Ok(None) => {
                    warn!(nic_id = %nic.id, network_id = %nic.network_id, "NIC's network not found, skipping");
                    failed += 1;
                    continue;
                }
                Err(e) => {
                    warn!(nic_id = %nic.id, error = %e, "Failed to get network for NIC, skipping");
                    failed += 1;
                    continue;
                }
            };

            // Create router for this NIC
            match self.create_nic_router(&nic, &network).await {
                Ok(()) => {
                    info!(nic_id = %nic.id, socket = %nic.socket_path, "Recovered NIC router");
                    recovered += 1;
                }
                Err(e) => {
                    warn!(nic_id = %nic.id, error = %e, "Failed to recover NIC router");
                    failed += 1;
                }
            }
        }

        info!(recovered, failed, "NIC recovery complete");
        Ok(())
    }

    /// Get the TUN reactor ID.
    pub async fn tun_reactor_id(&self) -> Option<ReactorId> {
        let guard = self.tun_router.lock().await;
        guard.as_ref().map(|r| r.reactor_id())
    }

    /// Create a vhost router for a NIC.
    pub async fn create_nic_router(&self, nic: &NicData, network: &NetworkData) -> Result<()> {
        let mut nics_guard = self.nics.lock().await;

        if nics_guard.contains_key(&nic.id) {
            debug!(nic_id = %nic.id, "NIC router already exists");
            return Ok(());
        }

        // Ensure socket directory exists
        std::fs::create_dir_all(SOCKET_DIR)?;

        // Build VhostConfig
        let mut vhost_config = VhostConfig::new(&nic.socket_path, nic.mac_address);

        // Configure IPv4
        if let (Some(addr), Some(gateway), Some(subnet)) = (
            nic.ipv4_address,
            network.ipv4_gateway(),
            network.ipv4_subnet,
        ) {
            vhost_config = vhost_config.with_ipv4(addr, gateway, subnet.prefix_len());
        }

        // Configure IPv6
        if let (Some(addr), Some(gateway), Some(prefix)) = (
            nic.ipv6_address,
            network.ipv6_gateway(),
            network.ipv6_prefix,
        ) {
            vhost_config = vhost_config.with_ipv6(addr, gateway, prefix.prefix_len());
        }

        // Add DNS servers
        vhost_config = vhost_config.with_dns(network.dns_servers.clone());

        // Create TUN for this NIC (each NIC needs its own TUN for routing)
        // Using a unique TUN name based on NIC ID
        let tun_name = format!("nic-{}", &nic.id.to_string()[..8]);
        let tun_ip = nic.ipv4_address.unwrap_or(Ipv4Addr::new(169, 254, 1, 1));
        let prefix_len = network.ipv4_subnet.map(|s| s.prefix_len()).unwrap_or(24);

        info!(
            nic_id = %nic.id,
            socket = %nic.socket_path,
            tun = %tun_name,
            "Creating vhost router for NIC"
        );

        let router = Router::with_shared_registry(
            &tun_name,
            Some((tun_ip, prefix_len)),
            4096,
            4096,
            4096,
            Some(vhost_config),
            Arc::clone(&self.registry),
        )
        .await
        .map_err(|e| ManagerError::RouterCreationFailed(e.to_string()))?;

        let reactor_id = router.reactor_id();

        // Create routing table for this NIC
        let table_id = Uuid::new_v4();
        router
            .reactor_handle()
            .create_table(table_id, format!("nic-{}", nic.id));
        router.reactor_handle().set_default_table(table_id);

        // Add route for NIC's own IP (local handling)
        if let Some(ipv4) = nic.ipv4_address {
            let prefix = Ipv4Net::new(ipv4, 32).unwrap();
            router.reactor_handle().add_route(
                table_id,
                IpPrefix::V4(prefix),
                RouteTarget::reactor(reactor_id),
            );
        }

        if let Some(ipv6) = nic.ipv6_address {
            let prefix = Ipv6Net::new(ipv6, 128).unwrap();
            router.reactor_handle().add_route(
                table_id,
                IpPrefix::V6(prefix),
                RouteTarget::reactor(reactor_id),
            );
        }

        // Configure routing based on network type
        if network.is_public {
            // Public network: add default route to global TUN
            if let Some(tun_reactor_id) = self.tun_reactor_id().await {
                router.reactor_handle().add_route(
                    table_id,
                    IpPrefix::V4("0.0.0.0/0".parse().unwrap()),
                    RouteTarget::reactor(tun_reactor_id),
                );
                router.reactor_handle().add_route(
                    table_id,
                    IpPrefix::V6("::/0".parse().unwrap()),
                    RouteTarget::reactor(tun_reactor_id),
                );
            }

            // Add host route in TUN for this NIC's IP
            self.add_nic_route_to_tun(nic, reactor_id).await?;
        }

        // Add VM-to-VM routes for other NICs in the same network
        self.add_vm_to_vm_routes(&nics_guard, &router, nic, network);

        nics_guard.insert(
            nic.id,
            ManagedNic {
                data: nic.clone(),
                router,
            },
        );

        info!(nic_id = %nic.id, reactor_id = %reactor_id, "NIC router created");

        Ok(())
    }

    /// Shutdown and remove a NIC router.
    pub async fn remove_nic_router(&self, nic_id: &Uuid) -> Result<()> {
        let mut nics_guard = self.nics.lock().await;

        if let Some(managed) = nics_guard.remove(nic_id) {
            info!(nic_id = %nic_id, "Removing NIC router");

            // Remove from TUN routing table
            if let Some(ipv4) = managed.data.ipv4_address {
                self.remove_nic_route_from_tun(ipv4).await?;
            }

            // Prepare shutdown
            managed.router.prepare_shutdown();

            // Shutdown router
            if let Err(e) = managed.router.shutdown().await {
                warn!(nic_id = %nic_id, error = %e, "Error during router shutdown");
            }

            // Remove socket file
            let _ = std::fs::remove_file(&managed.data.socket_path);
        }

        Ok(())
    }

    /// Add host route for a NIC to the global TUN.
    async fn add_nic_route_to_tun(&self, nic: &NicData, reactor_id: ReactorId) -> Result<()> {
        let tun_guard = self.tun_router.lock().await;
        let table_guard = self.tun_table_id.lock().await;

        if let (Some(router), Some(table_id)) = (tun_guard.as_ref(), *table_guard) {
            if let Some(ipv4) = nic.ipv4_address {
                let prefix = Ipv4Net::new(ipv4, 32).unwrap();
                router.reactor_handle().add_route(
                    table_id,
                    IpPrefix::V4(prefix),
                    RouteTarget::reactor(reactor_id),
                );
                debug!(ipv4 = %ipv4, reactor_id = %reactor_id, "Added host route to TUN");
            }

            if let Some(ipv6) = nic.ipv6_address {
                let prefix = Ipv6Net::new(ipv6, 128).unwrap();
                router.reactor_handle().add_route(
                    table_id,
                    IpPrefix::V6(prefix),
                    RouteTarget::reactor(reactor_id),
                );
                debug!(ipv6 = %ipv6, reactor_id = %reactor_id, "Added host route to TUN");
            }
        }

        Ok(())
    }

    /// Remove host route for a NIC from the global TUN.
    async fn remove_nic_route_from_tun(&self, ipv4: Ipv4Addr) -> Result<()> {
        let tun_guard = self.tun_router.lock().await;
        let table_guard = self.tun_table_id.lock().await;

        if let (Some(router), Some(table_id)) = (tun_guard.as_ref(), *table_guard) {
            let prefix = Ipv4Net::new(ipv4, 32).unwrap();
            router
                .reactor_handle()
                .remove_route(table_id, IpPrefix::V4(prefix));
            debug!(ipv4 = %ipv4, "Removed host route from TUN");
        }

        Ok(())
    }

    /// Add VM-to-VM routes for other NICs in the same network.
    /// Note: Caller must pass the nics_guard to avoid deadlock.
    fn add_vm_to_vm_routes(
        &self,
        nics_guard: &HashMap<Uuid, ManagedNic>,
        router: &Router,
        nic: &NicData,
        network: &NetworkData,
    ) {
        let table_id = Uuid::new_v4();

        // For each existing NIC in the same network, add bidirectional routes
        for (other_id, other_managed) in nics_guard.iter() {
            if *other_id == nic.id {
                continue;
            }

            if other_managed.data.network_id != network.id {
                continue;
            }

            // Add route from new NIC to existing NIC
            if let Some(other_ipv4) = other_managed.data.ipv4_address {
                let prefix = Ipv4Net::new(other_ipv4, 32).unwrap();
                router.reactor_handle().add_route(
                    table_id,
                    IpPrefix::V4(prefix),
                    RouteTarget::reactor(other_managed.router.reactor_id()),
                );
            }

            // Add route from existing NIC to new NIC
            if let Some(nic_ipv4) = nic.ipv4_address {
                let prefix = Ipv4Net::new(nic_ipv4, 32).unwrap();
                // Get the table ID for the other router
                // Note: We use a fixed naming convention for table lookup
                other_managed.router.reactor_handle().add_route(
                    table_id,
                    IpPrefix::V4(prefix),
                    RouteTarget::reactor(router.reactor_id()),
                );
            }
        }
    }

    /// Sync public network routes to TUN.
    ///
    /// This adds both:
    /// - Kernel routes via rtnetlink (for return traffic from the network stack)
    /// - Internal LPM routes (for packet routing within mvirt-net)
    async fn sync_public_network_routes(&self) -> Result<()> {
        let tun_guard = self.tun_router.lock().await;
        let table_guard = self.tun_table_id.lock().await;

        if let (Some(router), Some(table_id)) = (tun_guard.as_ref(), *table_guard) {
            let networks = self.storage.list_public_networks()?;
            let tun_if_index = router.tun_if_index();

            for network in networks {
                // Add IPv4 subnet routes
                if let Some(subnet) = network.ipv4_subnet {
                    // Add kernel route via rtnetlink
                    if let Err(e) = Self::add_kernel_route_v4(tun_if_index, subnet).await {
                        warn!(subnet = %subnet, error = %e, "Failed to add kernel route");
                    }

                    // Add internal LPM route
                    router.reactor_handle().add_route(
                        table_id,
                        IpPrefix::V4(subnet),
                        RouteTarget::Tun {
                            if_index: tun_if_index,
                        },
                    );
                    debug!(subnet = %subnet, "Added public network subnet to TUN");
                }

                // Add IPv6 prefix routes
                if let Some(prefix) = network.ipv6_prefix {
                    // Add kernel route via rtnetlink
                    if let Err(e) = Self::add_kernel_route_v6(tun_if_index, prefix).await {
                        warn!(prefix = %prefix, error = %e, "Failed to add IPv6 kernel route");
                    }

                    // Add internal LPM route
                    router.reactor_handle().add_route(
                        table_id,
                        IpPrefix::V6(prefix),
                        RouteTarget::Tun {
                            if_index: tun_if_index,
                        },
                    );
                    debug!(prefix = %prefix, "Added public network IPv6 prefix to TUN");
                }
            }
        }

        Ok(())
    }

    /// Add a kernel route via rtnetlink.
    async fn add_kernel_route_v4(if_index: u32, subnet: Ipv4Net) -> io::Result<()> {
        let (connection, handle, _) = rtnetlink::new_connection().map_err(io::Error::other)?;
        tokio::spawn(connection);

        match handle
            .route()
            .add()
            .v4()
            .destination_prefix(subnet.addr(), subnet.prefix_len())
            .output_interface(if_index)
            .execute()
            .await
        {
            Ok(()) => {
                info!(subnet = %subnet, if_index, "Kernel route added");
                Ok(())
            }
            Err(rtnetlink::Error::NetlinkError(e)) if e.raw_code() == -libc::EEXIST => {
                debug!(subnet = %subnet, "Kernel route already exists");
                Ok(())
            }
            Err(e) => Err(io::Error::other(e)),
        }
    }

    /// Add an IPv6 kernel route via rtnetlink.
    async fn add_kernel_route_v6(if_index: u32, prefix: Ipv6Net) -> io::Result<()> {
        let (connection, handle, _) = rtnetlink::new_connection().map_err(io::Error::other)?;
        tokio::spawn(connection);

        match handle
            .route()
            .add()
            .v6()
            .destination_prefix(prefix.addr(), prefix.prefix_len())
            .output_interface(if_index)
            .execute()
            .await
        {
            Ok(()) => {
                info!(prefix = %prefix, if_index, "IPv6 kernel route added");
                Ok(())
            }
            Err(rtnetlink::Error::NetlinkError(e)) if e.raw_code() == -libc::EEXIST => {
                debug!(prefix = %prefix, "IPv6 kernel route already exists");
                Ok(())
            }
            Err(e) => Err(io::Error::other(e)),
        }
    }

    /// Shutdown the manager and all routers.
    pub async fn shutdown(&self) -> Result<()> {
        info!("Shutting down NetworkManager");

        // Shutdown all NIC routers
        let mut nics_guard = self.nics.lock().await;
        for (nic_id, managed) in nics_guard.drain() {
            info!(nic_id = %nic_id, "Shutting down NIC router");
            managed.router.prepare_shutdown();
            if let Err(e) = managed.router.shutdown().await {
                warn!(nic_id = %nic_id, error = %e, "Error during NIC router shutdown");
            }
        }

        // Shutdown TUN router
        let mut tun_guard = self.tun_router.lock().await;
        if let Some(router) = tun_guard.take() {
            info!("Shutting down TUN router");
            router.prepare_shutdown();
            if let Err(e) = router.shutdown().await {
                warn!(error = %e, "Error during TUN router shutdown");
            }
        }

        Ok(())
    }
}

/// Generate socket path for a NIC.
pub fn generate_socket_path(nic_id: &Uuid) -> String {
    format!("{}/nic-{}.sock", SOCKET_DIR, nic_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_socket_path() {
        let id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        let path = generate_socket_path(&id);
        assert_eq!(
            path,
            "/run/mvirt/net/nic-550e8400-e29b-41d4-a716-446655440000.sock"
        );
    }
}
