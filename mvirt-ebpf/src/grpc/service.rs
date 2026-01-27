//! gRPC EbpfNetService implementation.

use super::proto::net_service_server::NetService;
use super::proto::*;
use super::storage::{NetworkData, NicData, NicState, Storage, generate_mac_address};
use super::validation::{
    ValidationError, allocate_ipv4_address, allocate_ipv6_address, parse_routed_prefixes,
    validate_create_network, validate_create_nic,
};
use crate::audit::EbpfAuditLogger;
use crate::ebpf_loader::{ACTION_REDIRECT, EbpfManager, RouteEntry};
use crate::proto_handler::ProtocolHandler;
use crate::tap::{TapDevice, delete_tap_interface, tap_name_from_nic_id};
use chrono::Utc;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tonic::{Request, Response, Status};
use tracing::{info, warn};
use uuid::Uuid;

/// Version string for GetVersion RPC.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Convert storage error to gRPC status.
fn storage_err_to_status(e: super::storage::StorageError) -> Status {
    match e {
        super::storage::StorageError::NetworkNotFound(id) => {
            Status::not_found(format!("Network not found: {}", id))
        }
        super::storage::StorageError::NicNotFound(id) => {
            Status::not_found(format!("NIC not found: {}", id))
        }
        super::storage::StorageError::NetworkNameExists(name) => {
            Status::already_exists(format!("Network name already exists: {}", name))
        }
        super::storage::StorageError::IpAddressInUse(addr) => {
            Status::already_exists(format!("IP address already in use: {}", addr))
        }
        _ => Status::internal(e.to_string()),
    }
}

/// Convert validation error to gRPC status.
fn validation_err_to_status(e: ValidationError) -> Status {
    Status::invalid_argument(e.to_string())
}

/// Convert NetworkData to proto Network.
fn network_data_to_proto(data: &NetworkData, nic_count: u32) -> Network {
    Network {
        id: data.id.to_string(),
        name: data.name.clone(),
        ipv4_enabled: data.ipv4_enabled,
        ipv4_subnet: data.ipv4_subnet.map(|s| s.to_string()).unwrap_or_default(),
        ipv6_enabled: data.ipv6_enabled,
        ipv6_prefix: data.ipv6_prefix.map(|s| s.to_string()).unwrap_or_default(),
        dns_servers: data.dns_servers.iter().map(|a| a.to_string()).collect(),
        ntp_servers: data.ntp_servers.iter().map(|a| a.to_string()).collect(),
        nic_count,
        created_at: data.created_at.to_rfc3339(),
        updated_at: data.updated_at.to_rfc3339(),
        is_public: data.is_public,
    }
}

/// Convert NicData to proto Nic.
fn nic_data_to_proto(data: &NicData) -> Nic {
    Nic {
        id: data.id.to_string(),
        name: data.name.clone().unwrap_or_default(),
        network_id: data.network_id.to_string(),
        mac_address: data.mac_string(),
        ipv4_address: data.ipv4_address.map(|a| a.to_string()).unwrap_or_default(),
        ipv6_address: data.ipv6_address.map(|a| a.to_string()).unwrap_or_default(),
        routed_ipv4_prefixes: data
            .routed_ipv4_prefixes
            .iter()
            .map(|p| p.to_string())
            .collect(),
        routed_ipv6_prefixes: data
            .routed_ipv6_prefixes
            .iter()
            .map(|p| p.to_string())
            .collect(),
        socket_path: format!("tap:{}", data.tap_name),
        state: data.state as i32,
        created_at: data.created_at.to_rfc3339(),
        updated_at: data.updated_at.to_rfc3339(),
    }
}

/// Managed TAP device with handler task.
struct ManagedNic {
    tap: TapDevice,
    handler_task: tokio::task::JoinHandle<()>,
}

/// EbpfNetService gRPC implementation.
pub struct EbpfNetServiceImpl {
    storage: Arc<Storage>,
    ebpf: Arc<EbpfManager>,
    proto_handler: Arc<ProtocolHandler>,
    audit: Arc<EbpfAuditLogger>,
    /// Active NICs with their TAP devices
    nics: Arc<RwLock<HashMap<Uuid, ManagedNic>>>,
}

impl EbpfNetServiceImpl {
    /// Create a new EbpfNetServiceImpl.
    pub fn new(
        storage: Arc<Storage>,
        ebpf: Arc<EbpfManager>,
        proto_handler: Arc<ProtocolHandler>,
        audit: Arc<EbpfAuditLogger>,
    ) -> Self {
        Self {
            storage,
            ebpf,
            proto_handler,
            audit,
            nics: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Resolve network by ID or name.
    async fn resolve_network(&self, id: &str, name: &str) -> Result<NetworkData, Status> {
        if !id.is_empty() {
            let uuid = Uuid::parse_str(id)
                .map_err(|_| Status::invalid_argument(format!("Invalid network ID: {}", id)))?;
            self.storage
                .get_network_by_id(&uuid)
                .map_err(storage_err_to_status)?
                .ok_or_else(|| Status::not_found(format!("Network not found: {}", id)))
        } else if !name.is_empty() {
            self.storage
                .get_network_by_name(name)
                .map_err(storage_err_to_status)?
                .ok_or_else(|| Status::not_found(format!("Network not found: {}", name)))
        } else {
            Err(Status::invalid_argument("Network ID or name required"))
        }
    }

    /// Setup a NIC: create TAP, attach eBPF, start protocol handler.
    async fn setup_nic(&self, nic: &NicData, network: &NetworkData) -> Result<(), Status> {
        // Create TAP device
        let tap = TapDevice::create(&nic.tap_name)
            .map_err(|e| Status::internal(format!("Failed to create TAP: {}", e)))?;

        // Set TAP interface up
        tap.set_up()
            .map_err(|e| Status::internal(format!("Failed to set TAP up: {}", e)))?;

        // Attach TC egress program
        self.ebpf
            .attach_egress(tap.if_index, &nic.tap_name)
            .await
            .map_err(|e| Status::internal(format!("Failed to attach eBPF: {}", e)))?;

        // Register with protocol handler
        self.proto_handler
            .register_nic(tap.if_index, nic.clone(), network.clone())
            .await;

        // Spawn protocol handler task
        let handler_task = self
            .proto_handler
            .spawn_handler(nic.tap_name.clone(), tap.if_index);

        // Add routes to eBPF maps
        if let Some(ipv4) = nic.ipv4_address {
            // Route to this NIC for its assigned IP
            let route = RouteEntry::new(
                ACTION_REDIRECT,
                tap.if_index,
                nic.mac_address,
                gateway_mac_for_network(network),
            );
            self.ebpf
                .add_egress_route_v4(ipv4, 32, route)
                .await
                .map_err(|e| Status::internal(format!("Failed to add route: {}", e)))?;
        }

        if let Some(ipv6) = nic.ipv6_address {
            let route = RouteEntry::new(
                ACTION_REDIRECT,
                tap.if_index,
                nic.mac_address,
                gateway_mac_for_network(network),
            );
            self.ebpf
                .add_egress_route_v6(ipv6, 128, route)
                .await
                .map_err(|e| Status::internal(format!("Failed to add route: {}", e)))?;
        }

        // Store managed NIC
        let mut nics = self.nics.write().await;
        nics.insert(nic.id, ManagedNic { tap, handler_task });

        info!(
            nic_id = %nic.id,
            tap_name = %nic.tap_name,
            "NIC setup complete"
        );

        Ok(())
    }

    /// Teardown a NIC: remove routes, stop handler, delete TAP.
    async fn teardown_nic(&self, nic: &NicData) -> Result<(), Status> {
        // Remove routes
        if let Some(ipv4) = nic.ipv4_address {
            let _ = self.ebpf.remove_egress_route_v4(ipv4, 32).await;
        }
        if let Some(ipv6) = nic.ipv6_address {
            let _ = self.ebpf.remove_egress_route_v6(ipv6, 128).await;
        }

        // Get and remove managed NIC
        let mut nics = self.nics.write().await;
        if let Some(managed) = nics.remove(&nic.id) {
            // Unregister from protocol handler
            self.proto_handler
                .unregister_nic(managed.tap.if_index)
                .await;

            // Abort handler task
            managed.handler_task.abort();

            // TAP is automatically cleaned up when dropped
        }

        // Also try to delete the interface directly (in case we're recovering)
        let _ = delete_tap_interface(&nic.tap_name).await;

        info!(
            nic_id = %nic.id,
            tap_name = %nic.tap_name,
            "NIC teardown complete"
        );

        Ok(())
    }

    /// Recover NICs from database on startup.
    pub async fn recover_nics(&self) -> Result<(), Status> {
        let nics = self.storage.list_nics().map_err(storage_err_to_status)?;

        let mut recovered = 0;
        let mut failed = 0;

        for nic in nics {
            // Get network
            let network = match self.storage.get_network_by_id(&nic.network_id) {
                Ok(Some(n)) => n,
                _ => {
                    warn!(nic_id = %nic.id, "Network not found for NIC, skipping");
                    failed += 1;
                    continue;
                }
            };

            match self.setup_nic(&nic, &network).await {
                Ok(_) => recovered += 1,
                Err(e) => {
                    warn!(nic_id = %nic.id, error = %e, "Failed to recover NIC");
                    failed += 1;
                }
            }
        }

        info!(recovered, failed, "NIC recovery complete");
        Ok(())
    }
}

#[tonic::async_trait]
impl NetService for EbpfNetServiceImpl {
    // ========== System ==========

    async fn get_version(
        &self,
        _request: Request<GetVersionRequest>,
    ) -> Result<Response<VersionInfo>, Status> {
        Ok(Response::new(VersionInfo {
            version: VERSION.to_string(),
        }))
    }

    // ========== Network Operations ==========

    async fn create_network(
        &self,
        request: Request<CreateNetworkRequest>,
    ) -> Result<Response<Network>, Status> {
        let req = request.into_inner();

        info!(name = %req.name, is_public = req.is_public, "CreateNetwork");

        // Validate
        let (ipv4_subnet, ipv6_prefix, dns_servers) = validate_create_network(
            &req.name,
            req.ipv4_enabled,
            &req.ipv4_subnet,
            req.ipv6_enabled,
            &req.ipv6_prefix,
            req.is_public,
            &req.dns_servers,
            &self.storage,
        )
        .map_err(validation_err_to_status)?;

        // Parse NTP servers
        let ntp_servers: Vec<IpAddr> = req
            .ntp_servers
            .iter()
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse().ok())
            .collect();

        let now = Utc::now();
        let network = NetworkData {
            id: Uuid::new_v4(),
            name: req.name,
            ipv4_enabled: req.ipv4_enabled,
            ipv4_subnet,
            ipv6_enabled: req.ipv6_enabled,
            ipv6_prefix,
            dns_servers,
            ntp_servers,
            is_public: req.is_public,
            created_at: now,
            updated_at: now,
        };

        // Store
        self.storage
            .create_network(&network)
            .map_err(storage_err_to_status)?;

        info!(id = %network.id, name = %network.name, "Network created");
        self.audit
            .network_created(&network.id.to_string(), &network.name);

        Ok(Response::new(network_data_to_proto(&network, 0)))
    }

    async fn get_network(
        &self,
        request: Request<GetNetworkRequest>,
    ) -> Result<Response<Network>, Status> {
        let req = request.into_inner();

        let (id, name) = match req.identifier {
            Some(get_network_request::Identifier::Id(id)) => (id, String::new()),
            Some(get_network_request::Identifier::Name(name)) => (String::new(), name),
            None => return Err(Status::invalid_argument("Network ID or name required")),
        };

        let network = self.resolve_network(&id, &name).await?;
        let nic_count = self
            .storage
            .count_nics_in_network(&network.id)
            .map_err(storage_err_to_status)?;

        Ok(Response::new(network_data_to_proto(&network, nic_count)))
    }

    async fn list_networks(
        &self,
        _request: Request<ListNetworksRequest>,
    ) -> Result<Response<ListNetworksResponse>, Status> {
        let networks = self
            .storage
            .list_networks()
            .map_err(storage_err_to_status)?;

        let mut protos = Vec::with_capacity(networks.len());
        for network in networks {
            let nic_count = self.storage.count_nics_in_network(&network.id).unwrap_or(0);
            protos.push(network_data_to_proto(&network, nic_count));
        }

        Ok(Response::new(ListNetworksResponse { networks: protos }))
    }

    async fn update_network(
        &self,
        request: Request<UpdateNetworkRequest>,
    ) -> Result<Response<Network>, Status> {
        let req = request.into_inner();

        let uuid = Uuid::parse_str(&req.id)
            .map_err(|_| Status::invalid_argument(format!("Invalid network ID: {}", req.id)))?;

        // Parse servers
        let dns_servers: Vec<IpAddr> = req
            .dns_servers
            .iter()
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse().ok())
            .collect();

        let ntp_servers: Vec<IpAddr> = req
            .ntp_servers
            .iter()
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse().ok())
            .collect();

        self.storage
            .update_network(&uuid, &dns_servers, &ntp_servers)
            .map_err(storage_err_to_status)?;

        let network = self
            .storage
            .get_network_by_id(&uuid)
            .map_err(storage_err_to_status)?
            .ok_or_else(|| Status::not_found("Network not found"))?;

        let nic_count = self
            .storage
            .count_nics_in_network(&uuid)
            .map_err(storage_err_to_status)?;

        self.audit
            .network_updated(&network.id.to_string(), &network.name);

        Ok(Response::new(network_data_to_proto(&network, nic_count)))
    }

    async fn delete_network(
        &self,
        request: Request<DeleteNetworkRequest>,
    ) -> Result<Response<DeleteNetworkResponse>, Status> {
        let req = request.into_inner();

        let uuid = Uuid::parse_str(&req.id)
            .map_err(|_| Status::invalid_argument(format!("Invalid network ID: {}", req.id)))?;

        // Check for NICs
        let nic_count = self
            .storage
            .count_nics_in_network(&uuid)
            .map_err(storage_err_to_status)?;

        if nic_count > 0 && !req.force {
            return Err(Status::failed_precondition(format!(
                "Network has {} NICs, use force=true to delete",
                nic_count
            )));
        }

        // Get network for audit
        let network = self
            .storage
            .get_network_by_id(&uuid)
            .map_err(storage_err_to_status)?;

        // Delete NICs if force
        if req.force && nic_count > 0 {
            let nics = self
                .storage
                .list_nics_in_network(&uuid)
                .map_err(storage_err_to_status)?;

            for nic in &nics {
                self.teardown_nic(nic).await?;
            }
        }

        // Delete network (CASCADE deletes NICs in DB)
        let deleted = self
            .storage
            .delete_network(&uuid)
            .map_err(storage_err_to_status)?;

        if let Some(n) = network {
            self.audit.network_deleted(&n.id.to_string(), &n.name);
        }

        Ok(Response::new(DeleteNetworkResponse {
            deleted,
            nics_deleted: if req.force { nic_count } else { 0 },
        }))
    }

    // ========== NIC Operations ==========

    async fn create_nic(
        &self,
        request: Request<CreateNicRequest>,
    ) -> Result<Response<Nic>, Status> {
        let req = request.into_inner();

        info!(network_id = %req.network_id, "CreateNic");

        // Resolve network
        let network = self.resolve_network(&req.network_id, "").await?;

        // Validate
        let (mac, ipv4, ipv6) = validate_create_nic(
            &req.mac_address,
            &req.ipv4_address,
            &req.ipv6_address,
            network.ipv4_subnet,
            network.ipv6_prefix,
        )
        .map_err(validation_err_to_status)?;

        // Generate or use provided MAC
        let mac_address = mac.unwrap_or_else(generate_mac_address);

        // Allocate or use provided IPv4
        let ipv4_address = if let Some(addr) = ipv4 {
            // Check if in use
            if self
                .storage
                .is_ipv4_in_use(&network.id, addr)
                .map_err(storage_err_to_status)?
            {
                return Err(Status::already_exists(format!(
                    "IPv4 address {} is already in use",
                    addr
                )));
            }
            Some(addr)
        } else if let Some(subnet) = network.ipv4_subnet {
            // Allocate
            let used = self
                .storage
                .get_used_ipv4_addresses(&network.id)
                .map_err(storage_err_to_status)?;
            let gateway = network.ipv4_gateway().unwrap();
            allocate_ipv4_address(subnet, &used, gateway)
        } else {
            None
        };

        // Allocate or use provided IPv6
        let ipv6_address = if let Some(addr) = ipv6 {
            if self
                .storage
                .is_ipv6_in_use(&network.id, addr)
                .map_err(storage_err_to_status)?
            {
                return Err(Status::already_exists(format!(
                    "IPv6 address {} is already in use",
                    addr
                )));
            }
            Some(addr)
        } else if let Some(prefix) = network.ipv6_prefix {
            let used = self
                .storage
                .get_used_ipv6_addresses(&network.id)
                .map_err(storage_err_to_status)?;
            let gateway = network.ipv6_gateway().unwrap();
            allocate_ipv6_address(prefix, &used, gateway)
        } else {
            None
        };

        // Parse routed prefixes
        let (routed_v4, routed_v6) =
            parse_routed_prefixes(&req.routed_ipv4_prefixes, &req.routed_ipv6_prefixes)
                .map_err(validation_err_to_status)?;

        let now = Utc::now();
        let nic_id = Uuid::new_v4();
        let tap_name = tap_name_from_nic_id(&nic_id);

        let nic = NicData {
            id: nic_id,
            name: if req.name.is_empty() {
                None
            } else {
                Some(req.name)
            },
            network_id: network.id,
            mac_address,
            ipv4_address,
            ipv6_address,
            routed_ipv4_prefixes: routed_v4,
            routed_ipv6_prefixes: routed_v6,
            tap_name,
            state: NicState::Created,
            created_at: now,
            updated_at: now,
        };

        // Store first (to ensure DB consistency)
        self.storage
            .create_nic(&nic)
            .map_err(storage_err_to_status)?;

        // Setup TAP, eBPF, protocol handler
        if let Err(e) = self.setup_nic(&nic, &network).await {
            // Rollback DB
            let _ = self.storage.delete_nic(&nic.id);
            return Err(e);
        }

        info!(
            id = %nic.id,
            tap_name = %nic.tap_name,
            ipv4 = ?ipv4_address,
            ipv6 = ?ipv6_address,
            "NIC created"
        );

        self.audit.nic_created(
            &nic.id.to_string(),
            &network.id.to_string(),
            &nic.mac_string(),
            &nic.tap_name,
            ipv4_address.map(|a| a.to_string()).as_deref(),
            ipv6_address.map(|a| a.to_string()).as_deref(),
        );

        Ok(Response::new(nic_data_to_proto(&nic)))
    }

    async fn get_nic(&self, request: Request<GetNicRequest>) -> Result<Response<Nic>, Status> {
        let req = request.into_inner();

        let nic = match req.identifier {
            Some(get_nic_request::Identifier::Id(id)) => {
                let uuid = Uuid::parse_str(&id)
                    .map_err(|_| Status::invalid_argument(format!("Invalid NIC ID: {}", id)))?;
                self.storage
                    .get_nic_by_id(&uuid)
                    .map_err(storage_err_to_status)?
                    .ok_or_else(|| Status::not_found(format!("NIC not found: {}", id)))?
            }
            Some(get_nic_request::Identifier::Name(name)) => self
                .storage
                .get_nic_by_name(&name)
                .map_err(storage_err_to_status)?
                .ok_or_else(|| Status::not_found(format!("NIC not found: {}", name)))?,
            None => return Err(Status::invalid_argument("NIC ID or name required")),
        };

        Ok(Response::new(nic_data_to_proto(&nic)))
    }

    async fn list_nics(
        &self,
        request: Request<ListNicsRequest>,
    ) -> Result<Response<ListNicsResponse>, Status> {
        let req = request.into_inner();

        let nics = if req.network_id.is_empty() {
            self.storage.list_nics().map_err(storage_err_to_status)?
        } else {
            let uuid = Uuid::parse_str(&req.network_id).map_err(|_| {
                Status::invalid_argument(format!("Invalid network ID: {}", req.network_id))
            })?;
            self.storage
                .list_nics_in_network(&uuid)
                .map_err(storage_err_to_status)?
        };

        let protos: Vec<Nic> = nics.iter().map(nic_data_to_proto).collect();

        Ok(Response::new(ListNicsResponse { nics: protos }))
    }

    async fn update_nic(
        &self,
        request: Request<UpdateNicRequest>,
    ) -> Result<Response<Nic>, Status> {
        let req = request.into_inner();

        let uuid = Uuid::parse_str(&req.id)
            .map_err(|_| Status::invalid_argument(format!("Invalid NIC ID: {}", req.id)))?;

        // Parse routed prefixes
        let (routed_v4, routed_v6) =
            parse_routed_prefixes(&req.routed_ipv4_prefixes, &req.routed_ipv6_prefixes)
                .map_err(validation_err_to_status)?;

        self.storage
            .update_nic_routed_prefixes(&uuid, &routed_v4, &routed_v6)
            .map_err(storage_err_to_status)?;

        let nic = self
            .storage
            .get_nic_by_id(&uuid)
            .map_err(storage_err_to_status)?
            .ok_or_else(|| Status::not_found("NIC not found"))?;

        self.audit.nic_updated(&nic.id.to_string());

        Ok(Response::new(nic_data_to_proto(&nic)))
    }

    async fn delete_nic(
        &self,
        request: Request<DeleteNicRequest>,
    ) -> Result<Response<DeleteNicResponse>, Status> {
        let req = request.into_inner();

        let uuid = Uuid::parse_str(&req.id)
            .map_err(|_| Status::invalid_argument(format!("Invalid NIC ID: {}", req.id)))?;

        // Get NIC for teardown
        let nic = self
            .storage
            .get_nic_by_id(&uuid)
            .map_err(storage_err_to_status)?;

        if let Some(nic) = &nic {
            // Teardown TAP, eBPF, handler
            self.teardown_nic(nic).await?;

            self.audit
                .nic_deleted(&nic.id.to_string(), &nic.network_id.to_string());
        }

        // Delete from DB
        let deleted = self
            .storage
            .delete_nic(&uuid)
            .map_err(storage_err_to_status)?;

        Ok(Response::new(DeleteNicResponse { deleted }))
    }
}

/// Generate a deterministic gateway MAC for a network.
fn gateway_mac_for_network(network: &NetworkData) -> [u8; 6] {
    let id_bytes = network.id.as_bytes();
    let mut mac = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01];
    mac[1] = id_bytes[0];
    mac[2] = id_bytes[1];
    mac[3] = id_bytes[2];
    mac[4] = id_bytes[3];
    mac[5] = id_bytes[4];
    mac
}
