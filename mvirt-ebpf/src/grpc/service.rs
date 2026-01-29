//! gRPC EbpfNetService implementation.

use super::proto::net_service_server::NetService;
use super::proto::*;
use super::storage::{
    NetworkData, NicData, NicState, SecurityGroupData, SecurityGroupRuleData, Storage,
    generate_mac_address,
};
use super::validation::{
    ValidationError, allocate_ipv4_address, allocate_ipv6_address, parse_routed_prefixes,
    validate_create_network, validate_create_nic, validate_create_security_group,
    validate_security_group_rule,
};
use crate::audit::EbpfAuditLogger;
use crate::ebpf_loader::{ACTION_REDIRECT, EbpfManager, RouteEntry};
use crate::nat;
use crate::proto_handler::{GATEWAY_MAC, ProtocolHandler};
use crate::tap::{
    add_host_route_v4, add_host_route_v6, create_persistent_tap, delete_tap_interface,
    remove_host_route_v4, remove_host_route_v6, set_interface_mac, set_interface_up,
    tap_name_from_nic_id,
};
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
        super::storage::StorageError::SecurityGroupNotFound(id) => {
            Status::not_found(format!("Security group not found: {}", id))
        }
        super::storage::StorageError::SecurityGroupNameExists(name) => {
            Status::already_exists(format!("Security group name already exists: {}", name))
        }
        super::storage::StorageError::SecurityGroupRuleNotFound(id) => {
            Status::not_found(format!("Security group rule not found: {}", id))
        }
        super::storage::StorageError::SecurityGroupHasNics(id) => {
            Status::failed_precondition(format!("Security group {} has attached NICs", id))
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

/// Convert SecurityGroupData to proto SecurityGroup.
fn security_group_data_to_proto(
    data: &SecurityGroupData,
    rules: Vec<SecurityGroupRule>,
    nic_count: u32,
) -> SecurityGroup {
    SecurityGroup {
        id: data.id.to_string(),
        name: data.name.clone(),
        description: data.description.clone().unwrap_or_default(),
        rules,
        nic_count,
        created_at: data.created_at.to_rfc3339(),
        updated_at: data.updated_at.to_rfc3339(),
    }
}

/// Convert SecurityGroupRuleData to proto SecurityGroupRule.
fn security_group_rule_data_to_proto(data: &SecurityGroupRuleData) -> SecurityGroupRule {
    SecurityGroupRule {
        id: data.id.to_string(),
        security_group_id: data.security_group_id.to_string(),
        direction: data.direction as i32,
        protocol: data.protocol as i32,
        port_start: data.port_start.unwrap_or(0) as u32,
        port_end: data.port_end.unwrap_or(0) as u32,
        cidr: data.cidr.clone().unwrap_or_default(),
        description: data.description.clone().unwrap_or_default(),
        created_at: data.created_at.to_rfc3339(),
        updated_at: data.updated_at.to_rfc3339(),
    }
}

/// Managed NIC with handler task.
struct ManagedNic {
    /// Interface index
    if_index: u32,
    /// Protocol handler task
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

    /// Setup a NIC: create persistent TAP device, attach eBPF, start protocol handler.
    async fn setup_nic(&self, nic: &NicData, network: &NetworkData) -> Result<(), Status> {
        // Create persistent TAP device (survives fd close, can be opened by cloud-hypervisor)
        let if_index = create_persistent_tap(&nic.tap_name)
            .map_err(|e| Status::internal(format!("Failed to create TAP: {}", e)))?;

        // Set TAP MAC to gateway MAC (so kernel accepts packets destined for gateway)
        set_interface_mac(&nic.tap_name, GATEWAY_MAC)
            .map_err(|e| Status::internal(format!("Failed to set TAP MAC: {}", e)))?;

        // Set TAP interface up
        set_interface_up(&nic.tap_name)
            .map_err(|e| Status::internal(format!("Failed to set TAP up: {}", e)))?;

        info!(
            nic_id = %nic.id,
            tap_name = %nic.tap_name,
            if_index,
            gateway_mac = ?GATEWAY_MAC,
            "Created persistent TAP device"
        );

        // Attach TC egress program
        self.ebpf
            .attach_egress(if_index, &nic.tap_name)
            .await
            .map_err(|e| Status::internal(format!("Failed to attach eBPF: {}", e)))?;

        // Register with protocol handler
        self.proto_handler
            .register_nic(if_index, nic.clone(), network.clone())
            .await;

        // Spawn protocol handler task (uses AF_PACKET, independent of TAP fd)
        let handler_task = self
            .proto_handler
            .spawn_handler(nic.tap_name.clone(), if_index);

        // Add routes to eBPF maps
        if let Some(ipv4) = nic.ipv4_address {
            // Route to this NIC for its assigned IP
            let route = RouteEntry::new(ACTION_REDIRECT, if_index, nic.mac_address, GATEWAY_MAC);
            self.ebpf
                .add_egress_route_v4(ipv4, 32, route)
                .await
                .map_err(|e| Status::internal(format!("Failed to add route: {}", e)))?;

            // Add kernel route for return traffic (NAT)
            add_host_route_v4(ipv4, if_index)
                .await
                .map_err(|e| Status::internal(format!("Failed to add kernel route: {}", e)))?;
        }

        if let Some(ipv6) = nic.ipv6_address {
            let route = RouteEntry::new(ACTION_REDIRECT, if_index, nic.mac_address, GATEWAY_MAC);
            self.ebpf
                .add_egress_route_v6(ipv6, 128, route)
                .await
                .map_err(|e| Status::internal(format!("Failed to add route: {}", e)))?;

            // Add kernel route for return traffic (NAT)
            add_host_route_v6(ipv6, if_index)
                .await
                .map_err(|e| Status::internal(format!("Failed to add kernel route: {}", e)))?;
        }

        // Store managed NIC
        let mut nics = self.nics.write().await;
        nics.insert(
            nic.id,
            ManagedNic {
                if_index,
                handler_task,
            },
        );

        info!(
            nic_id = %nic.id,
            tap_name = %nic.tap_name,
            "NIC setup complete"
        );

        Ok(())
    }

    /// Teardown a NIC: remove routes, stop handler, delete TAP.
    async fn teardown_nic(&self, nic: &NicData) -> Result<(), Status> {
        // Get and remove managed NIC first to get if_index
        let mut nics = self.nics.write().await;
        let if_index = if let Some(managed) = nics.remove(&nic.id) {
            // Unregister from protocol handler
            self.proto_handler.unregister_nic(managed.if_index).await;

            // Abort handler task
            managed.handler_task.abort();

            Some(managed.if_index)
        } else {
            None
        };
        drop(nics);

        // Remove eBPF and kernel routes
        if let Some(ipv4) = nic.ipv4_address {
            let _ = self.ebpf.remove_egress_route_v4(ipv4, 32).await;
            if let Some(if_idx) = if_index {
                let _ = remove_host_route_v4(ipv4, if_idx).await;
            }
        }
        if let Some(ipv6) = nic.ipv6_address {
            let _ = self.ebpf.remove_egress_route_v6(ipv6, 128).await;
            if let Some(if_idx) = if_index {
                let _ = remove_host_route_v6(ipv6, if_idx).await;
            }
        }

        // Delete the persistent TAP interface
        let _ = delete_tap_interface(&nic.tap_name).await;

        info!(
            nic_id = %nic.id,
            tap_name = %nic.tap_name,
            "NIC teardown complete"
        );

        Ok(())
    }

    /// Resolve NIC by ID or name.
    async fn resolve_nic(&self, id: &str, name: &str) -> Result<NicData, Status> {
        if !id.is_empty() {
            let uuid = Uuid::parse_str(id)
                .map_err(|_| Status::invalid_argument(format!("Invalid NIC ID: {}", id)))?;
            self.storage
                .get_nic_by_id(&uuid)
                .map_err(storage_err_to_status)?
                .ok_or_else(|| Status::not_found(format!("NIC not found: {}", id)))
        } else if !name.is_empty() {
            self.storage
                .get_nic_by_name(name)
                .map_err(storage_err_to_status)?
                .ok_or_else(|| Status::not_found(format!("NIC not found: {}", name)))
        } else {
            Err(Status::invalid_argument("NIC ID or name required"))
        }
    }

    /// Resolve security group by ID or name.
    async fn resolve_security_group(
        &self,
        id: &str,
        name: &str,
    ) -> Result<SecurityGroupData, Status> {
        if !id.is_empty() {
            let uuid = Uuid::parse_str(id).map_err(|_| {
                Status::invalid_argument(format!("Invalid security group ID: {}", id))
            })?;
            self.storage
                .get_security_group_by_id(&uuid)
                .map_err(storage_err_to_status)?
                .ok_or_else(|| Status::not_found(format!("Security group not found: {}", id)))
        } else if !name.is_empty() {
            self.storage
                .get_security_group_by_name(name)
                .map_err(storage_err_to_status)?
                .ok_or_else(|| Status::not_found(format!("Security group not found: {}", name)))
        } else {
            Err(Status::invalid_argument(
                "Security group ID or name required",
            ))
        }
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
                Ok(()) => recovered += 1,
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
        let id = if req.id.is_empty() {
            Uuid::new_v4()
        } else {
            Uuid::parse_str(&req.id).map_err(|_| {
                Status::invalid_argument(format!("Invalid network ID: {}", req.id))
            })?
        };
        let network = NetworkData {
            id,
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

        // Add masquerade rule for public networks
        if network.is_public {
            if let Some(subnet) = network.ipv4_subnet {
                match nat::get_default_interface() {
                    Ok(out_iface) => {
                        if let Err(e) = nat::add_masquerade_v4(subnet, &out_iface) {
                            warn!(subnet = %subnet, error = %e, "Failed to add masquerade rule");
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to get default interface for masquerade");
                    }
                }
            }
            if let Some(prefix) = network.ipv6_prefix {
                match nat::get_default_interface() {
                    Ok(out_iface) => {
                        if let Err(e) = nat::add_masquerade_v6(prefix, &out_iface) {
                            warn!(prefix = %prefix, error = %e, "Failed to add IPv6 masquerade rule");
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to get default interface for IPv6 masquerade");
                    }
                }
            }
        }

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

        if let Some(ref n) = network {
            // Remove masquerade rules for public networks
            if n.is_public {
                if let Some(subnet) = n.ipv4_subnet
                    && let Ok(out_iface) = nat::get_default_interface()
                {
                    let _ = nat::remove_masquerade_v4(subnet, &out_iface);
                }
                if let Some(prefix) = n.ipv6_prefix
                    && let Ok(out_iface) = nat::get_default_interface()
                {
                    let _ = nat::remove_masquerade_v6(prefix, &out_iface);
                }
            }
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

    async fn attach_nic(
        &self,
        request: Request<AttachNicRequest>,
    ) -> Result<Response<AttachNicResponse>, Status> {
        let req = request.into_inner();

        let uuid = Uuid::parse_str(&req.id)
            .map_err(|_| Status::invalid_argument(format!("Invalid NIC ID: {}", req.id)))?;

        // Get NIC from DB
        let nic = self
            .storage
            .get_nic_by_id(&uuid)
            .map_err(storage_err_to_status)?
            .ok_or_else(|| Status::not_found(format!("NIC not found: {}", req.id)))?;

        // Check if already attached
        {
            let nics = self.nics.read().await;
            if nics.contains_key(&uuid) {
                return Ok(Response::new(AttachNicResponse {
                    attached: true,
                    message: "NIC already attached".to_string(),
                }));
            }
        }

        // Get network
        let network = self
            .storage
            .get_network_by_id(&nic.network_id)
            .map_err(storage_err_to_status)?
            .ok_or_else(|| Status::internal(format!("Network not found: {}", nic.network_id)))?;

        // Try to attach (this will create TAP if it doesn't exist)
        self.setup_nic(&nic, &network).await?;
        info!(nic_id = %uuid, "NIC attached successfully");
        Ok(Response::new(AttachNicResponse {
            attached: true,
            message: "NIC attached".to_string(),
        }))
    }

    // ========== Security Group Operations ==========

    async fn create_security_group(
        &self,
        request: Request<CreateSecurityGroupRequest>,
    ) -> Result<Response<SecurityGroup>, Status> {
        let req = request.into_inner();

        info!(name = %req.name, "CreateSecurityGroup");

        // Validate
        validate_create_security_group(&req.name).map_err(validation_err_to_status)?;

        let now = Utc::now();
        let sg = SecurityGroupData {
            id: Uuid::new_v4(),
            name: req.name,
            description: if req.description.is_empty() {
                None
            } else {
                Some(req.description)
            },
            created_at: now,
            updated_at: now,
        };

        // Store
        self.storage
            .create_security_group(&sg)
            .map_err(storage_err_to_status)?;

        info!(id = %sg.id, name = %sg.name, "Security group created");
        self.audit
            .security_group_created(&sg.id.to_string(), &sg.name);

        Ok(Response::new(security_group_data_to_proto(&sg, vec![], 0)))
    }

    async fn get_security_group(
        &self,
        request: Request<GetSecurityGroupRequest>,
    ) -> Result<Response<SecurityGroup>, Status> {
        let req = request.into_inner();

        let (id, name) = match req.identifier {
            Some(get_security_group_request::Identifier::Id(id)) => (id, String::new()),
            Some(get_security_group_request::Identifier::Name(name)) => (String::new(), name),
            None => {
                return Err(Status::invalid_argument(
                    "Security group ID or name required",
                ));
            }
        };

        let sg = self.resolve_security_group(&id, &name).await?;

        // Get rules
        let rules = self
            .storage
            .list_rules_for_security_group(&sg.id)
            .map_err(storage_err_to_status)?;
        let proto_rules: Vec<SecurityGroupRule> = rules
            .iter()
            .map(security_group_rule_data_to_proto)
            .collect();

        // Get NIC count
        let nic_count = self
            .storage
            .count_nics_in_security_group(&sg.id)
            .map_err(storage_err_to_status)?;

        Ok(Response::new(security_group_data_to_proto(
            &sg,
            proto_rules,
            nic_count,
        )))
    }

    async fn list_security_groups(
        &self,
        _request: Request<ListSecurityGroupsRequest>,
    ) -> Result<Response<ListSecurityGroupsResponse>, Status> {
        let groups = self
            .storage
            .list_security_groups()
            .map_err(storage_err_to_status)?;

        let mut protos = Vec::with_capacity(groups.len());
        for sg in groups {
            let rules = self
                .storage
                .list_rules_for_security_group(&sg.id)
                .unwrap_or_default();
            let proto_rules: Vec<SecurityGroupRule> = rules
                .iter()
                .map(security_group_rule_data_to_proto)
                .collect();
            let nic_count = self
                .storage
                .count_nics_in_security_group(&sg.id)
                .unwrap_or(0);
            protos.push(security_group_data_to_proto(&sg, proto_rules, nic_count));
        }

        Ok(Response::new(ListSecurityGroupsResponse {
            security_groups: protos,
        }))
    }

    async fn delete_security_group(
        &self,
        request: Request<DeleteSecurityGroupRequest>,
    ) -> Result<Response<DeleteSecurityGroupResponse>, Status> {
        let req = request.into_inner();

        let uuid = Uuid::parse_str(&req.id).map_err(|_| {
            Status::invalid_argument(format!("Invalid security group ID: {}", req.id))
        })?;

        // Check for attached NICs
        let nic_count = self
            .storage
            .count_nics_in_security_group(&uuid)
            .map_err(storage_err_to_status)?;

        if nic_count > 0 && !req.force {
            return Err(Status::failed_precondition(format!(
                "Security group has {} attached NICs, use force=true to delete",
                nic_count
            )));
        }

        // Get security group for audit
        let sg = self
            .storage
            .get_security_group_by_id(&uuid)
            .map_err(storage_err_to_status)?;

        // Detach from all NICs if force
        let nics_detached = if req.force && nic_count > 0 {
            self.storage
                .detach_security_group_from_all_nics(&uuid)
                .map_err(storage_err_to_status)?
        } else {
            0
        };

        // Delete security group (CASCADE deletes rules)
        let deleted = self
            .storage
            .delete_security_group(&uuid)
            .map_err(storage_err_to_status)?;

        if let Some(s) = sg {
            self.audit
                .security_group_deleted(&s.id.to_string(), &s.name);
        }

        Ok(Response::new(DeleteSecurityGroupResponse {
            deleted,
            nics_detached,
        }))
    }

    async fn add_security_group_rule(
        &self,
        request: Request<AddSecurityGroupRuleRequest>,
    ) -> Result<Response<SecurityGroupRule>, Status> {
        let req = request.into_inner();

        info!(security_group_id = %req.security_group_id, "AddSecurityGroupRule");

        // Resolve security group
        let sg = self
            .resolve_security_group(&req.security_group_id, "")
            .await?;

        // Validate rule
        let (direction, protocol, port_start, port_end, _parsed_cidr) =
            validate_security_group_rule(
                req.direction,
                req.protocol,
                req.port_start,
                req.port_end,
                &req.cidr,
            )
            .map_err(validation_err_to_status)?;

        let now = Utc::now();
        let rule = SecurityGroupRuleData {
            id: Uuid::new_v4(),
            security_group_id: sg.id,
            direction,
            protocol,
            port_start,
            port_end,
            cidr: if req.cidr.is_empty() {
                None
            } else {
                Some(req.cidr)
            },
            description: if req.description.is_empty() {
                None
            } else {
                Some(req.description)
            },
            created_at: now,
            updated_at: now,
        };

        // Store
        self.storage
            .create_security_group_rule(&rule)
            .map_err(storage_err_to_status)?;

        info!(
            id = %rule.id,
            security_group_id = %sg.id,
            direction = %direction.as_str(),
            protocol = %protocol.as_str(),
            "Security group rule added"
        );
        self.audit
            .security_group_rule_added(&rule.id.to_string(), &sg.id.to_string());

        Ok(Response::new(security_group_rule_data_to_proto(&rule)))
    }

    async fn remove_security_group_rule(
        &self,
        request: Request<RemoveSecurityGroupRuleRequest>,
    ) -> Result<Response<RemoveSecurityGroupRuleResponse>, Status> {
        let req = request.into_inner();

        let uuid = Uuid::parse_str(&req.rule_id)
            .map_err(|_| Status::invalid_argument(format!("Invalid rule ID: {}", req.rule_id)))?;

        // Get rule for audit
        let rule = self
            .storage
            .get_security_group_rule_by_id(&uuid)
            .map_err(storage_err_to_status)?;

        let deleted = self
            .storage
            .delete_security_group_rule(&uuid)
            .map_err(storage_err_to_status)?;

        if let Some(r) = rule {
            self.audit
                .security_group_rule_removed(&r.id.to_string(), &r.security_group_id.to_string());
        }

        Ok(Response::new(RemoveSecurityGroupRuleResponse { deleted }))
    }

    async fn attach_security_group(
        &self,
        request: Request<AttachSecurityGroupRequest>,
    ) -> Result<Response<AttachSecurityGroupResponse>, Status> {
        let req = request.into_inner();

        info!(
            nic_id = %req.nic_id,
            security_group_id = %req.security_group_id,
            "AttachSecurityGroup"
        );

        // Resolve NIC
        let nic = self.resolve_nic(&req.nic_id, "").await?;

        // Resolve security group
        let sg = self
            .resolve_security_group(&req.security_group_id, "")
            .await?;

        // Attach
        let attached = self
            .storage
            .attach_security_group(&nic.id, &sg.id)
            .map_err(storage_err_to_status)?;

        if attached {
            info!(
                nic_id = %nic.id,
                security_group_id = %sg.id,
                "Security group attached to NIC"
            );
            self.audit
                .security_group_attached(&sg.id.to_string(), &nic.id.to_string());
        }

        Ok(Response::new(AttachSecurityGroupResponse { attached }))
    }

    async fn detach_security_group(
        &self,
        request: Request<DetachSecurityGroupRequest>,
    ) -> Result<Response<DetachSecurityGroupResponse>, Status> {
        let req = request.into_inner();

        // Resolve NIC
        let nic = self.resolve_nic(&req.nic_id, "").await?;

        // Resolve security group
        let sg = self
            .resolve_security_group(&req.security_group_id, "")
            .await?;

        // Detach
        let detached = self
            .storage
            .detach_security_group(&nic.id, &sg.id)
            .map_err(storage_err_to_status)?;

        if detached {
            info!(
                nic_id = %nic.id,
                security_group_id = %sg.id,
                "Security group detached from NIC"
            );
            self.audit
                .security_group_detached(&sg.id.to_string(), &nic.id.to_string());
        }

        Ok(Response::new(DetachSecurityGroupResponse { detached }))
    }
}
