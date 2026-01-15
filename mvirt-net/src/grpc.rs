use std::sync::{Arc, Mutex};

use tonic::{Request, Response, Status};
use tracing::{info, warn};

use crate::audit::NetAuditLogger;
use crate::config::{NetworkEntry, NicEntry, NicEntryBuilder, NicState};
use crate::dataplane::WorkerManager;
use crate::proto::net_service_server::NetService;
use crate::proto::*;
use crate::store::Store;

/// Reconcile routes for public networks
///
/// Ensures all routes for public networks exist in the kernel routing table,
/// and removes stale routes that no longer correspond to any public network.
pub async fn reconcile_routes(store: &Store) {
    let networks = match store.list_networks().await {
        Ok(n) => n,
        Err(e) => {
            warn!(error = %e, "Failed to list networks for route reconciliation");
            return;
        }
    };

    // Collect expected routes from public networks
    let mut expected_routes: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    for network in &networks {
        if network.is_public {
            if let Some(ref subnet) = network.ipv4_subnet {
                expected_routes.insert(subnet.clone());
            }
            if let Some(ref prefix) = network.ipv6_prefix {
                expected_routes.insert(prefix.clone());
            }
        }
    }

    // Get current routes from kernel
    let current_routes = match crate::dataplane::get_routes() {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "Failed to get current routes");
            return;
        }
    };
    let current_set: std::collections::HashSet<String> = current_routes.into_iter().collect();

    // Add missing routes
    for route in expected_routes.difference(&current_set) {
        if let Err(e) = crate::dataplane::add_route(route) {
            warn!(route = %route, error = %e, "Failed to add missing route");
        } else {
            info!(route = %route, "Added missing route to TUN");
        }
    }

    // Remove stale routes
    for route in current_set.difference(&expected_routes) {
        if let Err(e) = crate::dataplane::remove_route(route) {
            warn!(route = %route, error = %e, "Failed to remove stale route");
        } else {
            info!(route = %route, "Removed stale route from TUN");
        }
    }
}

pub struct NetServiceImpl {
    socket_dir: String,
    store: Arc<Store>,
    audit: Arc<NetAuditLogger>,
    workers: Mutex<WorkerManager>,
}

impl NetServiceImpl {
    pub fn new(
        socket_dir: String,
        store: Arc<Store>,
        audit: Arc<NetAuditLogger>,
    ) -> Result<Self, String> {
        Ok(Self {
            socket_dir,
            store,
            audit,
            workers: Mutex::new(WorkerManager::new()?),
        })
    }


    /// Recover workers for existing NICs after service restart
    pub async fn recover_nics(&self) {
        info!("Recovering workers for existing NICs");

        let nics = match self.store.list_nics(None).await {
            Ok(nics) => nics,
            Err(e) => {
                warn!(error = %e, "Failed to list NICs for recovery");
                return;
            }
        };

        for nic in nics {
            // Get the network for this NIC
            let network = match self.store.get_network(&nic.network_id).await {
                Ok(Some(n)) => n,
                Ok(None) => {
                    warn!(nic_id = %nic.id, "Network not found for NIC, skipping");
                    continue;
                }
                Err(e) => {
                    warn!(nic_id = %nic.id, error = %e, "Failed to get network, skipping");
                    continue;
                }
            };

            // Start worker for this NIC
            if let Err(e) = self.workers.lock().unwrap().start(nic.clone(), network) {
                warn!(nic_id = %nic.id, error = %e, "Failed to recover worker for NIC");
            } else {
                info!(nic_id = %nic.id, socket = %nic.socket_path, "Recovered worker for NIC");
            }
        }
    }

    /// Generate a MAC address with the local/unicast bit set
    fn generate_mac(&self) -> String {
        use uuid::Uuid;
        let uuid = Uuid::new_v4();
        let bytes = uuid.as_bytes();
        // Set local bit (0x02) and clear multicast bit
        let b0 = (bytes[0] & 0xfe) | 0x02;
        format!(
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            b0, bytes[1], bytes[2], bytes[3], bytes[4], bytes[5]
        )
    }

    /// Allocate next available IPv4 address from subnet
    async fn allocate_ipv4(&self, network: &NetworkEntry) -> Result<Option<String>, Status> {
        let subnet = match &network.ipv4_subnet {
            Some(s) => s,
            None => return Ok(None),
        };

        // Parse subnet (e.g., "10.0.0.0/24")
        let net: ipnet::Ipv4Net = subnet
            .parse()
            .map_err(|e| Status::internal(format!("Invalid subnet: {}", e)))?;

        // Skip network address and gateway, start from .2
        for addr in net.hosts().skip(1) {
            let addr_str = addr.to_string();
            if !self
                .store
                .is_address_allocated(&network.id, &addr_str)
                .await
                .map_err(|e| Status::internal(e.to_string()))?
            {
                return Ok(Some(addr_str));
            }
        }

        Err(Status::resource_exhausted("No IPv4 addresses available"))
    }

    /// Check if the given IPv4/IPv6 ranges overlap with any existing public networks
    ///
    /// Public networks must have disjoint IP address ranges to avoid routing conflicts.
    async fn check_public_network_overlap(
        &self,
        ipv4_subnet: Option<&str>,
        ipv6_prefix: Option<&str>,
        exclude_id: Option<&str>,
    ) -> Result<(), Status> {
        let networks = self
            .store
            .list_networks()
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        for net in networks.iter().filter(|n| n.is_public) {
            // Skip the network we're updating (if any)
            if exclude_id == Some(&net.id) {
                continue;
            }

            // Check IPv4 overlap
            if let (Some(new_subnet), Some(existing_subnet)) = (ipv4_subnet, &net.ipv4_subnet)
                && subnets_overlap_v4(new_subnet, existing_subnet)?
            {
                return Err(Status::invalid_argument(format!(
                    "IPv4 subnet {} overlaps with public network '{}' ({})",
                    new_subnet, net.name, existing_subnet
                )));
            }

            // Check IPv6 overlap
            if let (Some(new_prefix), Some(existing_prefix)) = (ipv6_prefix, &net.ipv6_prefix)
                && prefixes_overlap_v6(new_prefix, existing_prefix)?
            {
                return Err(Status::invalid_argument(format!(
                    "IPv6 prefix {} overlaps with public network '{}' ({})",
                    new_prefix, net.name, existing_prefix
                )));
            }
        }

        Ok(())
    }

    /// Allocate next available IPv6 address from prefix
    async fn allocate_ipv6(&self, network: &NetworkEntry) -> Result<Option<String>, Status> {
        let prefix = match &network.ipv6_prefix {
            Some(p) => p,
            None => return Ok(None),
        };

        // Parse prefix (e.g., "fd00::/64")
        let net: ipnet::Ipv6Net = prefix
            .parse()
            .map_err(|e| Status::internal(format!("Invalid prefix: {}", e)))?;

        // Skip network address and ::1 (gateway), start from ::2
        for (i, addr) in net.hosts().enumerate() {
            if i == 0 {
                continue; // Skip ::0
            }
            if i > 65534 {
                // Limit search space
                break;
            }
            let addr_str = addr.to_string();
            if !self
                .store
                .is_address_allocated(&network.id, &addr_str)
                .await
                .map_err(|e| Status::internal(e.to_string()))?
            {
                return Ok(Some(addr_str));
            }
        }

        Err(Status::resource_exhausted("No IPv6 addresses available"))
    }
}

#[tonic::async_trait]
impl NetService for NetServiceImpl {
    // === Network operations ===

    async fn create_network(
        &self,
        request: Request<CreateNetworkRequest>,
    ) -> Result<Response<Network>, Status> {
        let req = request.into_inner();

        // Validate name
        if req.name.is_empty() {
            return Err(Status::invalid_argument("name is required"));
        }

        // Validate at least one address family is enabled
        if !req.ipv4_enabled && !req.ipv6_enabled {
            return Err(Status::invalid_argument(
                "at least one of ipv4_enabled or ipv6_enabled must be true",
            ));
        }

        // Validate subnet/prefix if enabled
        if req.ipv4_enabled && req.ipv4_subnet.is_empty() {
            return Err(Status::invalid_argument(
                "ipv4_subnet is required when ipv4_enabled",
            ));
        }
        if req.ipv6_enabled && req.ipv6_prefix.is_empty() {
            return Err(Status::invalid_argument(
                "ipv6_prefix is required when ipv6_enabled",
            ));
        }

        // Validate subnet format
        if req.ipv4_enabled {
            let _: ipnet::Ipv4Net = req
                .ipv4_subnet
                .parse()
                .map_err(|e| Status::invalid_argument(format!("invalid ipv4_subnet: {}", e)))?;
        }
        if req.ipv6_enabled {
            let _: ipnet::Ipv6Net = req
                .ipv6_prefix
                .parse()
                .map_err(|e| Status::invalid_argument(format!("invalid ipv6_prefix: {}", e)))?;
        }

        // Check for duplicate name
        if self
            .store
            .get_network_by_name(&req.name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .is_some()
        {
            return Err(Status::already_exists(format!(
                "network '{}' already exists",
                req.name
            )));
        }

        // For public networks, check for IP overlap with other public networks
        if req.is_public {
            self.check_public_network_overlap(
                if req.ipv4_enabled {
                    Some(&req.ipv4_subnet)
                } else {
                    None
                },
                if req.ipv6_enabled {
                    Some(&req.ipv6_prefix)
                } else {
                    None
                },
                None, // No network to exclude (creating new)
            )
            .await?;
        }

        // Create entry
        let entry = NetworkEntry::new(
            req.name.clone(),
            req.ipv4_enabled,
            if req.ipv4_enabled {
                Some(req.ipv4_subnet)
            } else {
                None
            },
            req.ipv6_enabled,
            if req.ipv6_enabled {
                Some(req.ipv6_prefix)
            } else {
                None
            },
            req.dns_servers,
            req.ntp_servers,
            req.is_public,
        );

        // Store
        self.store
            .create_network(&entry)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // For public networks, add routes to the TUN device
        if entry.is_public {
            if let Some(ref subnet) = entry.ipv4_subnet {
                if let Err(e) = crate::dataplane::add_route(subnet) {
                    warn!(subnet = %subnet, error = %e, "Failed to add IPv4 route to TUN");
                } else {
                    info!(subnet = %subnet, "Added IPv4 route to TUN");
                }
            }
            if let Some(ref prefix) = entry.ipv6_prefix {
                if let Err(e) = crate::dataplane::add_route(prefix) {
                    warn!(prefix = %prefix, error = %e, "Failed to add IPv6 route to TUN");
                } else {
                    info!(prefix = %prefix, "Added IPv6 route to TUN");
                }
            }
        }

        info!(network_id = %entry.id, name = %entry.name, "Network created");
        self.audit.network_created(&entry.id, &entry.name).await;

        Ok(Response::new(network_to_proto(&entry, 0)))
    }

    async fn get_network(
        &self,
        request: Request<GetNetworkRequest>,
    ) -> Result<Response<Network>, Status> {
        let req = request.into_inner();

        let network = match req.identifier {
            Some(get_network_request::Identifier::Id(id)) => self
                .store
                .get_network(&id)
                .await
                .map_err(|e| Status::internal(e.to_string()))?,
            Some(get_network_request::Identifier::Name(name)) => self
                .store
                .get_network_by_name(&name)
                .await
                .map_err(|e| Status::internal(e.to_string()))?,
            None => return Err(Status::invalid_argument("id or name is required")),
        };

        let network = network.ok_or_else(|| Status::not_found("network not found"))?;

        let nic_count = self
            .store
            .count_nics_in_network(&network.id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(network_to_proto(&network, nic_count)))
    }

    async fn list_networks(
        &self,
        _request: Request<ListNetworksRequest>,
    ) -> Result<Response<ListNetworksResponse>, Status> {
        let networks = self
            .store
            .list_networks()
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        let mut protos = Vec::with_capacity(networks.len());
        for network in networks {
            let nic_count = self
                .store
                .count_nics_in_network(&network.id)
                .await
                .unwrap_or(0);
            protos.push(network_to_proto(&network, nic_count));
        }

        Ok(Response::new(ListNetworksResponse { networks: protos }))
    }

    async fn update_network(
        &self,
        request: Request<UpdateNetworkRequest>,
    ) -> Result<Response<Network>, Status> {
        let req = request.into_inner();

        if req.id.is_empty() {
            return Err(Status::invalid_argument("id is required"));
        }

        // Check network exists
        let network = self
            .store
            .get_network(&req.id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::not_found("network not found"))?;

        // Update
        self.store
            .update_network(&req.id, &req.dns_servers, &req.ntp_servers)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        info!(network_id = %req.id, "Network updated");
        self.audit.network_updated(&req.id, &network.name).await;

        // Fetch updated
        let updated = self
            .store
            .get_network(&req.id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::internal("network disappeared"))?;

        let nic_count = self
            .store
            .count_nics_in_network(&updated.id)
            .await
            .unwrap_or(0);

        Ok(Response::new(network_to_proto(&updated, nic_count)))
    }

    async fn delete_network(
        &self,
        request: Request<DeleteNetworkRequest>,
    ) -> Result<Response<DeleteNetworkResponse>, Status> {
        let req = request.into_inner();

        if req.id.is_empty() {
            return Err(Status::invalid_argument("id is required"));
        }

        // Check network exists
        let network = self
            .store
            .get_network(&req.id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::not_found("network not found"))?;

        // Check for NICs
        let nic_count = self
            .store
            .count_nics_in_network(&req.id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        if nic_count > 0 && !req.force {
            return Err(Status::failed_precondition(format!(
                "network has {} NICs, use force=true to delete",
                nic_count
            )));
        }

        // Delete (CASCADE will delete NICs)
        let deleted = self
            .store
            .delete_network(&req.id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        if deleted {
            // For public networks, remove routes from the TUN device
            if network.is_public {
                if let Some(ref subnet) = network.ipv4_subnet {
                    if let Err(e) = crate::dataplane::remove_route(subnet) {
                        warn!(subnet = %subnet, error = %e, "Failed to remove IPv4 route from TUN");
                    } else {
                        info!(subnet = %subnet, "Removed IPv4 route from TUN");
                    }
                }
                if let Some(ref prefix) = network.ipv6_prefix {
                    if let Err(e) = crate::dataplane::remove_route(prefix) {
                        warn!(prefix = %prefix, error = %e, "Failed to remove IPv6 route from TUN");
                    } else {
                        info!(prefix = %prefix, "Removed IPv6 route from TUN");
                    }
                }
            }

            info!(network_id = %req.id, nics_deleted = nic_count, "Network deleted");
            self.audit.network_deleted(&req.id, &network.name).await;

            // Clean up the network's router
            self.workers.lock().unwrap().remove_network(&req.id);
        }

        Ok(Response::new(DeleteNetworkResponse {
            deleted,
            nics_deleted: nic_count,
        }))
    }

    // === NIC operations ===

    async fn create_nic(
        &self,
        request: Request<CreateNicRequest>,
    ) -> Result<Response<Nic>, Status> {
        let req = request.into_inner();

        if req.network_id.is_empty() {
            return Err(Status::invalid_argument("network_id is required"));
        }

        // Resolve network (by ID or name)
        let network = match self
            .store
            .get_network(&req.network_id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
        {
            Some(n) => n,
            None => {
                // Try by name
                self.store
                    .get_network_by_name(&req.network_id)
                    .await
                    .map_err(|e| Status::internal(e.to_string()))?
                    .ok_or_else(|| Status::not_found("network not found"))?
            }
        };

        // Generate or use provided MAC
        let mac = if req.mac_address.is_empty() {
            self.generate_mac()
        } else {
            req.mac_address
        };

        // Allocate IPv4 address
        let ipv4 = if network.ipv4_enabled {
            if req.ipv4_address.is_empty() {
                self.allocate_ipv4(&network).await?
            } else {
                // Validate requested address
                if self
                    .store
                    .is_address_allocated(&network.id, &req.ipv4_address)
                    .await
                    .map_err(|e| Status::internal(e.to_string()))?
                {
                    return Err(Status::already_exists(format!(
                        "IPv4 address {} is already allocated",
                        req.ipv4_address
                    )));
                }
                Some(req.ipv4_address)
            }
        } else {
            None
        };

        // Allocate IPv6 address
        let ipv6 = if network.ipv6_enabled {
            if req.ipv6_address.is_empty() {
                self.allocate_ipv6(&network).await?
            } else {
                // Validate requested address
                if self
                    .store
                    .is_address_allocated(&network.id, &req.ipv6_address)
                    .await
                    .map_err(|e| Status::internal(e.to_string()))?
                {
                    return Err(Status::already_exists(format!(
                        "IPv6 address {} is already allocated",
                        req.ipv6_address
                    )));
                }
                Some(req.ipv6_address)
            }
        } else {
            None
        };

        // Create NIC entry
        let socket_path = format!(
            "{}/nic-{}.sock",
            self.socket_dir,
            &uuid::Uuid::new_v4().to_string()[..8]
        );

        let entry = NicEntryBuilder::new(network.id.clone(), mac.clone(), socket_path)
            .name(if req.name.is_empty() {
                None
            } else {
                Some(req.name)
            })
            .ipv4_address(ipv4.clone())
            .ipv6_address(ipv6.clone())
            .routed_ipv4_prefixes(req.routed_ipv4_prefixes.clone())
            .routed_ipv6_prefixes(req.routed_ipv6_prefixes.clone())
            .build();

        // Store NIC
        self.store
            .create_nic(&entry)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Allocate addresses in tracking table
        if let Some(ref addr) = ipv4 {
            self.store
                .allocate_address(&network.id, addr, &entry.id)
                .await
                .map_err(|e| Status::internal(e.to_string()))?;
        }
        if let Some(ref addr) = ipv6 {
            self.store
                .allocate_address(&network.id, addr, &entry.id)
                .await
                .map_err(|e| Status::internal(e.to_string()))?;
        }

        // Add routed prefixes
        for prefix in &req.routed_ipv4_prefixes {
            self.store
                .add_routed_prefix(&network.id, prefix, &entry.id)
                .await
                .map_err(|e| Status::internal(e.to_string()))?;
        }
        for prefix in &req.routed_ipv6_prefixes {
            self.store
                .add_routed_prefix(&network.id, prefix, &entry.id)
                .await
                .map_err(|e| Status::internal(e.to_string()))?;
        }

        info!(
            nic_id = %entry.id,
            network_id = %network.id,
            mac = %mac,
            ipv4 = ?ipv4,
            ipv6 = ?ipv6,
            "NIC created"
        );

        self.audit
            .nic_created(
                &entry.id,
                &network.id,
                &mac,
                ipv4.as_deref(),
                ipv6.as_deref(),
            )
            .await;

        // Spawn vhost-user worker thread
        if let Err(e) = self
            .workers
            .lock()
            .unwrap()
            .start(entry.clone(), network.clone())
        {
            warn!(nic_id = %entry.id, error = %e, "Failed to start vhost-user worker");
            return Err(Status::internal(format!(
                "Failed to start vhost-user worker: {e}"
            )));
        }

        Ok(Response::new(nic_to_proto(&entry)))
    }

    async fn get_nic(&self, request: Request<GetNicRequest>) -> Result<Response<Nic>, Status> {
        let req = request.into_inner();

        let nic = match req.identifier {
            Some(get_nic_request::Identifier::Id(id)) => self
                .store
                .get_nic(&id)
                .await
                .map_err(|e| Status::internal(e.to_string()))?,
            Some(get_nic_request::Identifier::Name(name)) => self
                .store
                .get_nic_by_name(&name)
                .await
                .map_err(|e| Status::internal(e.to_string()))?,
            None => return Err(Status::invalid_argument("id or name is required")),
        };

        let nic = nic.ok_or_else(|| Status::not_found("NIC not found"))?;

        Ok(Response::new(nic_to_proto(&nic)))
    }

    async fn list_nics(
        &self,
        request: Request<ListNicsRequest>,
    ) -> Result<Response<ListNicsResponse>, Status> {
        let req = request.into_inner();

        let network_id = if req.network_id.is_empty() {
            None
        } else {
            Some(req.network_id.as_str())
        };

        let nics = self
            .store
            .list_nics(network_id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        let protos: Vec<Nic> = nics.iter().map(nic_to_proto).collect();

        Ok(Response::new(ListNicsResponse { nics: protos }))
    }

    async fn update_nic(
        &self,
        request: Request<UpdateNicRequest>,
    ) -> Result<Response<Nic>, Status> {
        let req = request.into_inner();

        if req.id.is_empty() {
            return Err(Status::invalid_argument("id is required"));
        }

        // Check NIC exists
        let nic = self
            .store
            .get_nic(&req.id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::not_found("NIC not found"))?;

        // Remove old routed prefixes
        self.store
            .remove_routed_prefixes_for_nic(&nic.id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Add new routed prefixes
        for prefix in &req.routed_ipv4_prefixes {
            self.store
                .add_routed_prefix(&nic.network_id, prefix, &nic.id)
                .await
                .map_err(|e| Status::internal(e.to_string()))?;
        }
        for prefix in &req.routed_ipv6_prefixes {
            self.store
                .add_routed_prefix(&nic.network_id, prefix, &nic.id)
                .await
                .map_err(|e| Status::internal(e.to_string()))?;
        }

        // Update NIC
        self.store
            .update_nic_routed_prefixes(
                &req.id,
                &req.routed_ipv4_prefixes,
                &req.routed_ipv6_prefixes,
            )
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        info!(nic_id = %req.id, "NIC updated");
        self.audit.nic_updated(&req.id).await;

        // Fetch updated
        let updated = self
            .store
            .get_nic(&req.id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::internal("NIC disappeared"))?;

        Ok(Response::new(nic_to_proto(&updated)))
    }

    async fn delete_nic(
        &self,
        request: Request<DeleteNicRequest>,
    ) -> Result<Response<DeleteNicResponse>, Status> {
        let req = request.into_inner();

        if req.id.is_empty() {
            return Err(Status::invalid_argument("id is required"));
        }

        // Check NIC exists
        let nic = self
            .store
            .get_nic(&req.id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::not_found("NIC not found"))?;

        // Stop vhost-user worker thread
        if let Err(e) = self.workers.lock().unwrap().stop(&nic.id) {
            // Not fatal - worker might not be running (e.g., after restart)
            warn!(nic_id = %nic.id, error = %e, "Failed to stop worker (may not be running)");
        }

        // Deallocate addresses
        self.store
            .deallocate_addresses_for_nic(&nic.id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Remove routed prefixes
        self.store
            .remove_routed_prefixes_for_nic(&nic.id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Delete NIC
        let deleted = self
            .store
            .delete_nic(&req.id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        if deleted {
            info!(nic_id = %req.id, "NIC deleted");
            self.audit.nic_deleted(&req.id, &nic.network_id).await;

            // Remove socket file if exists
            let _ = tokio::fs::remove_file(&nic.socket_path).await;
        }

        Ok(Response::new(DeleteNicResponse { deleted }))
    }
}

// === Proto conversion helpers ===

fn network_to_proto(entry: &NetworkEntry, nic_count: u32) -> Network {
    Network {
        id: entry.id.clone(),
        name: entry.name.clone(),
        ipv4_enabled: entry.ipv4_enabled,
        ipv4_subnet: entry.ipv4_subnet.clone().unwrap_or_default(),
        ipv6_enabled: entry.ipv6_enabled,
        ipv6_prefix: entry.ipv6_prefix.clone().unwrap_or_default(),
        dns_servers: entry.dns_servers.clone(),
        ntp_servers: entry.ntp_servers.clone(),
        nic_count,
        created_at: entry.created_at.clone(),
        updated_at: entry.updated_at.clone(),
        is_public: entry.is_public,
    }
}

fn nic_to_proto(entry: &NicEntry) -> Nic {
    Nic {
        id: entry.id.clone(),
        name: entry.name.clone().unwrap_or_default(),
        network_id: entry.network_id.clone(),
        mac_address: entry.mac_address.clone(),
        ipv4_address: entry.ipv4_address.clone().unwrap_or_default(),
        ipv6_address: entry.ipv6_address.clone().unwrap_or_default(),
        routed_ipv4_prefixes: entry.routed_ipv4_prefixes.clone(),
        routed_ipv6_prefixes: entry.routed_ipv6_prefixes.clone(),
        socket_path: entry.socket_path.clone(),
        state: nic_state_to_proto(entry.state),
        created_at: entry.created_at.clone(),
        updated_at: entry.updated_at.clone(),
    }
}

fn nic_state_to_proto(state: NicState) -> i32 {
    match state {
        NicState::Created => crate::proto::NicState::Created as i32,
        NicState::Active => crate::proto::NicState::Active as i32,
        NicState::Error => crate::proto::NicState::Error as i32,
    }
}

/// Check if two IPv4 subnets overlap
fn subnets_overlap_v4(a: &str, b: &str) -> Result<bool, Status> {
    let net_a: ipnet::Ipv4Net = a
        .parse()
        .map_err(|e| Status::internal(format!("invalid IPv4 subnet '{}': {}", a, e)))?;
    let net_b: ipnet::Ipv4Net = b
        .parse()
        .map_err(|e| Status::internal(format!("invalid IPv4 subnet '{}': {}", b, e)))?;

    // Two subnets overlap if one contains the other or they share addresses
    Ok(net_a.contains(&net_b) || net_b.contains(&net_a))
}

/// Check if two IPv6 prefixes overlap
fn prefixes_overlap_v6(a: &str, b: &str) -> Result<bool, Status> {
    let net_a: ipnet::Ipv6Net = a
        .parse()
        .map_err(|e| Status::internal(format!("invalid IPv6 prefix '{}': {}", a, e)))?;
    let net_b: ipnet::Ipv6Net = b
        .parse()
        .map_err(|e| Status::internal(format!("invalid IPv6 prefix '{}': {}", b, e)))?;

    // Two prefixes overlap if one contains the other or they share addresses
    Ok(net_a.contains(&net_b) || net_b.contains(&net_a))
}
