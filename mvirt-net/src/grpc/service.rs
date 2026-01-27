//! gRPC NetService implementation.

use super::manager::{NetworkManager, generate_socket_path};
use super::proto::net_service_server::NetService;
use super::proto::*;
use super::storage::{NetworkData, NicData, NicState, Storage, generate_mac_address};
use super::validation::{
    ValidationError, allocate_ipv4_address, allocate_ipv6_address, validate_create_network,
    validate_create_nic,
};
use crate::audit::NetAuditLogger;
use chrono::Utc;
use std::net::IpAddr;
use std::sync::Arc;
use tonic::{Request, Response, Status};
use tracing::{error, info};
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

/// Convert manager error to gRPC status.
fn manager_err_to_status(e: super::manager::ManagerError) -> Status {
    match e {
        super::manager::ManagerError::NetworkNotFound(id) => {
            Status::not_found(format!("Network not found: {}", id))
        }
        super::manager::ManagerError::NicNotFound(id) => {
            Status::not_found(format!("NIC not found: {}", id))
        }
        _ => Status::internal(e.to_string()),
    }
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
        socket_path: format!("vhost-user:{}", data.socket_path),
        state: data.state as i32,
        created_at: data.created_at.to_rfc3339(),
        updated_at: data.updated_at.to_rfc3339(),
    }
}

/// NetService gRPC implementation.
pub struct NetServiceImpl {
    storage: Arc<Storage>,
    manager: Arc<NetworkManager>,
    audit: Arc<NetAuditLogger>,
}

impl NetServiceImpl {
    /// Create a new NetServiceImpl.
    pub fn new(
        storage: Arc<Storage>,
        manager: Arc<NetworkManager>,
        audit: Arc<NetAuditLogger>,
    ) -> Self {
        Self {
            storage,
            manager,
            audit,
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
}

#[tonic::async_trait]
impl NetService for NetServiceImpl {
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

        self.storage
            .create_network(&network)
            .map_err(storage_err_to_status)?;

        // Add kernel routes if this is a public network
        self.manager
            .add_public_network_routes(&network)
            .await
            .map_err(manager_err_to_status)?;

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

        let mut proto_networks = Vec::with_capacity(networks.len());
        for network in &networks {
            let nic_count = self
                .storage
                .count_nics_in_network(&network.id)
                .map_err(storage_err_to_status)?;
            proto_networks.push(network_data_to_proto(network, nic_count));
        }

        Ok(Response::new(ListNetworksResponse {
            networks: proto_networks,
        }))
    }

    async fn update_network(
        &self,
        request: Request<UpdateNetworkRequest>,
    ) -> Result<Response<Network>, Status> {
        let req = request.into_inner();

        let uuid = Uuid::parse_str(&req.id)
            .map_err(|_| Status::invalid_argument(format!("Invalid network ID: {}", req.id)))?;

        // Parse DNS and NTP servers
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

        // Fetch updated network
        let network = self
            .storage
            .get_network_by_id(&uuid)
            .map_err(storage_err_to_status)?
            .ok_or_else(|| Status::not_found(format!("Network not found: {}", req.id)))?;

        let nic_count = self
            .storage
            .count_nics_in_network(&network.id)
            .map_err(storage_err_to_status)?;

        info!(id = %network.id, "Network updated");
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

        // Get network info for audit log
        let network = self
            .storage
            .get_network_by_id(&uuid)
            .map_err(storage_err_to_status)?
            .ok_or_else(|| Status::not_found(format!("Network not found: {}", req.id)))?;

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

        // If force, delete all NICs first
        let mut nics_deleted = 0u32;
        if req.force && nic_count > 0 {
            let nics = self
                .storage
                .list_nics_in_network(&uuid)
                .map_err(storage_err_to_status)?;

            for nic in &nics {
                // Remove router
                self.manager
                    .remove_nic_router(&nic.id)
                    .await
                    .map_err(manager_err_to_status)?;

                // Delete from storage
                self.storage
                    .delete_nic(&nic.id)
                    .map_err(storage_err_to_status)?;

                nics_deleted += 1;
            }
        }

        // Remove kernel routes if this was a public network
        self.manager
            .remove_public_network_routes(&network)
            .await
            .map_err(manager_err_to_status)?;

        // Delete network
        let deleted = self
            .storage
            .delete_network(&uuid)
            .map_err(storage_err_to_status)?;

        info!(id = %uuid, nics_deleted = nics_deleted, "Network deleted");
        self.audit.network_deleted(&uuid.to_string(), &network.name);

        Ok(Response::new(DeleteNetworkResponse {
            deleted,
            nics_deleted,
        }))
    }

    // ========== NIC Operations ==========

    async fn create_nic(
        &self,
        request: Request<CreateNicRequest>,
    ) -> Result<Response<Nic>, Status> {
        let req = request.into_inner();

        info!(network_id = %req.network_id, name = ?req.name, "CreateNic");

        // Resolve network (by ID or name)
        let network = if let Ok(uuid) = Uuid::parse_str(&req.network_id) {
            self.storage
                .get_network_by_id(&uuid)
                .map_err(storage_err_to_status)?
                .ok_or_else(|| {
                    Status::not_found(format!("Network not found: {}", req.network_id))
                })?
        } else {
            self.storage
                .get_network_by_name(&req.network_id)
                .map_err(storage_err_to_status)?
                .ok_or_else(|| {
                    Status::not_found(format!("Network not found: {}", req.network_id))
                })?
        };

        // Validate
        let (mac, ipv4, ipv6, routed_v4, routed_v6) = validate_create_nic(
            &network,
            &req.mac_address,
            &req.ipv4_address,
            &req.ipv6_address,
            &req.routed_ipv4_prefixes,
            &req.routed_ipv6_prefixes,
            &self.storage,
        )
        .map_err(validation_err_to_status)?;

        // Generate MAC if not provided
        let mac_address = mac.unwrap_or_else(generate_mac_address);

        // Allocate IP if not provided
        let ipv4_address = if network.ipv4_enabled {
            ipv4.or_else(|| allocate_ipv4_address(&network, &self.storage))
        } else {
            None
        };

        let ipv6_address = if network.ipv6_enabled {
            ipv6.or_else(|| allocate_ipv6_address(&network, &self.storage))
        } else {
            None
        };

        let nic_id = Uuid::new_v4();
        let socket_path = generate_socket_path(&nic_id);
        let now = Utc::now();

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
            socket_path,
            state: NicState::Created,
            created_at: now,
            updated_at: now,
        };

        // Save to storage
        self.storage
            .create_nic(&nic)
            .map_err(storage_err_to_status)?;

        // Create router
        if let Err(e) = self.manager.create_nic_router(&nic, &network).await {
            error!(nic_id = %nic.id, error = %e, "Failed to create NIC router");
            // Clean up storage
            let _ = self.storage.delete_nic(&nic.id);
            return Err(manager_err_to_status(e));
        }

        info!(
            id = %nic.id,
            network_id = %network.id,
            ipv4 = ?ipv4_address,
            ipv6 = ?ipv6_address,
            socket = %nic.socket_path,
            "NIC created"
        );
        self.audit.nic_created(
            &nic.id.to_string(),
            &network.id.to_string(),
            &nic.mac_string(),
            ipv4_address.map(|a| a.to_string()).as_deref(),
            ipv6_address.map(|a| a.to_string()).as_deref(),
        );

        Ok(Response::new(nic_data_to_proto(&nic)))
    }

    async fn get_nic(&self, request: Request<GetNicRequest>) -> Result<Response<Nic>, Status> {
        let req = request.into_inner();

        let (id, name) = match req.identifier {
            Some(get_nic_request::Identifier::Id(id)) => (id, String::new()),
            Some(get_nic_request::Identifier::Name(name)) => (String::new(), name),
            None => return Err(Status::invalid_argument("NIC ID or name required")),
        };

        let nic = if !id.is_empty() {
            let uuid = Uuid::parse_str(&id)
                .map_err(|_| Status::invalid_argument(format!("Invalid NIC ID: {}", id)))?;
            self.storage
                .get_nic_by_id(&uuid)
                .map_err(storage_err_to_status)?
                .ok_or_else(|| Status::not_found(format!("NIC not found: {}", id)))?
        } else {
            self.storage
                .get_nic_by_name(&name)
                .map_err(storage_err_to_status)?
                .ok_or_else(|| Status::not_found(format!("NIC not found: {}", name)))?
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

        let proto_nics: Vec<Nic> = nics.iter().map(nic_data_to_proto).collect();

        Ok(Response::new(ListNicsResponse { nics: proto_nics }))
    }

    async fn update_nic(
        &self,
        request: Request<UpdateNicRequest>,
    ) -> Result<Response<Nic>, Status> {
        let req = request.into_inner();

        let uuid = Uuid::parse_str(&req.id)
            .map_err(|_| Status::invalid_argument(format!("Invalid NIC ID: {}", req.id)))?;

        // Parse routed prefixes
        let routed_v4: Vec<ipnet::Ipv4Net> = req
            .routed_ipv4_prefixes
            .iter()
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse().ok())
            .collect();

        let routed_v6: Vec<ipnet::Ipv6Net> = req
            .routed_ipv6_prefixes
            .iter()
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse().ok())
            .collect();

        self.storage
            .update_nic_routed_prefixes(&uuid, &routed_v4, &routed_v6)
            .map_err(storage_err_to_status)?;

        // Fetch updated NIC
        let nic = self
            .storage
            .get_nic_by_id(&uuid)
            .map_err(storage_err_to_status)?
            .ok_or_else(|| Status::not_found(format!("NIC not found: {}", req.id)))?;

        info!(id = %nic.id, "NIC updated");
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

        // Get NIC info for audit log
        let nic = self
            .storage
            .get_nic_by_id(&uuid)
            .map_err(storage_err_to_status)?
            .ok_or_else(|| Status::not_found(format!("NIC not found: {}", req.id)))?;

        // Remove router first
        self.manager
            .remove_nic_router(&uuid)
            .await
            .map_err(manager_err_to_status)?;

        // Delete from storage
        let deleted = self
            .storage
            .delete_nic(&uuid)
            .map_err(storage_err_to_status)?;

        info!(id = %uuid, "NIC deleted");
        self.audit
            .nic_deleted(&uuid.to_string(), &nic.network_id.to_string());

        Ok(Response::new(DeleteNicResponse { deleted }))
    }
}
