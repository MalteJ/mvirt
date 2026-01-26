use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use mraft::{NodeId, RaftNode};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use utoipa::ToSchema;

use crate::audit::CpAuditLogger;
use crate::command::{Command, NetworkData, NicData, Response as CmdResponse};
use crate::state::CpState;

/// Shared application state
pub struct AppState {
    pub node: Arc<RwLock<RaftNode<Command, CmdResponse, CpState>>>,
    pub audit: Arc<CpAuditLogger>,
    pub node_id: NodeId,
}

/// API error response
#[derive(Serialize, ToSchema)]
pub struct ApiError {
    pub error: String,
    pub code: u32,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let status = match self.code {
            404 => StatusCode::NOT_FOUND,
            409 => StatusCode::CONFLICT,
            400 => StatusCode::BAD_REQUEST,
            503 => StatusCode::SERVICE_UNAVAILABLE,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(self)).into_response()
    }
}

// === Version ===

/// Version information
#[derive(Serialize, ToSchema)]
pub struct VersionInfo {
    pub version: String,
}

/// Get service version
#[utoipa::path(
    get,
    path = "/api/v1/version",
    responses(
        (status = 200, description = "Service version", body = VersionInfo)
    ),
    tag = "system"
)]
pub async fn get_version() -> Json<VersionInfo> {
    Json(VersionInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

// === Cluster Info ===

/// Cluster information
#[derive(Serialize, ToSchema)]
pub struct ClusterInfo {
    pub cluster_id: String,
    pub leader_id: Option<u64>,
    pub current_term: u64,
    pub commit_index: u64,
    pub nodes: Vec<NodeInfo>,
}

/// Node information
#[derive(Serialize, ToSchema)]
pub struct NodeInfo {
    pub id: u64,
    pub name: String,
    pub address: String,
    pub state: String,
    pub is_leader: bool,
}

/// Get cluster information
#[utoipa::path(
    get,
    path = "/api/v1/cluster",
    responses(
        (status = 200, description = "Cluster information", body = ClusterInfo)
    ),
    tag = "cluster"
)]
pub async fn get_cluster_info(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ClusterInfo>, ApiError> {
    let node = state.node.read().await;
    let metrics = node.metrics();

    let nodes: Vec<NodeInfo> = metrics
        .membership_config
        .membership()
        .nodes()
        .map(|(id, n)| {
            let is_leader = Some(*id) == metrics.current_leader;
            NodeInfo {
                id: *id,
                name: format!("node-{}", id),
                address: n.addr.clone(),
                state: if is_leader {
                    "leader".to_string()
                } else {
                    "follower".to_string()
                },
                is_leader,
            }
        })
        .collect();

    Ok(Json(ClusterInfo {
        cluster_id: "mvirt-cluster".to_string(),
        leader_id: metrics.current_leader,
        current_term: metrics.current_term,
        commit_index: metrics.last_applied.map(|l| l.index).unwrap_or(0),
        nodes,
    }))
}

// === Network CRUD ===

/// Request to create a network
#[derive(Deserialize, ToSchema)]
pub struct CreateNetworkRequest {
    /// Unique network name
    pub name: String,
    /// Enable IPv4 (default: true)
    pub ipv4_enabled: Option<bool>,
    /// IPv4 subnet in CIDR notation (e.g., "10.0.0.0/24")
    pub ipv4_subnet: Option<String>,
    /// Enable IPv6 (default: false)
    pub ipv6_enabled: Option<bool>,
    /// IPv6 prefix in CIDR notation (e.g., "fd00::/64")
    pub ipv6_prefix: Option<String>,
    /// DNS servers to announce via DHCP
    pub dns_servers: Option<Vec<String>>,
    /// NTP servers to announce via DHCP
    pub ntp_servers: Option<Vec<String>>,
    /// Enable public internet access
    pub is_public: Option<bool>,
}

/// Network resource
#[derive(Serialize, ToSchema)]
pub struct Network {
    pub id: String,
    pub name: String,
    pub ipv4_enabled: bool,
    pub ipv4_subnet: Option<String>,
    pub ipv6_enabled: bool,
    pub ipv6_prefix: Option<String>,
    pub dns_servers: Vec<String>,
    pub ntp_servers: Vec<String>,
    pub is_public: bool,
    pub nic_count: u32,
    pub created_at: String,
    pub updated_at: String,
}

impl From<NetworkData> for Network {
    fn from(data: NetworkData) -> Self {
        Self {
            id: data.id,
            name: data.name,
            ipv4_enabled: data.ipv4_enabled,
            ipv4_subnet: data.ipv4_subnet,
            ipv6_enabled: data.ipv6_enabled,
            ipv6_prefix: data.ipv6_prefix,
            dns_servers: data.dns_servers,
            ntp_servers: data.ntp_servers,
            is_public: data.is_public,
            nic_count: data.nic_count,
            created_at: data.created_at,
            updated_at: data.updated_at,
        }
    }
}

impl From<&NetworkData> for Network {
    fn from(data: &NetworkData) -> Self {
        Self {
            id: data.id.clone(),
            name: data.name.clone(),
            ipv4_enabled: data.ipv4_enabled,
            ipv4_subnet: data.ipv4_subnet.clone(),
            ipv6_enabled: data.ipv6_enabled,
            ipv6_prefix: data.ipv6_prefix.clone(),
            dns_servers: data.dns_servers.clone(),
            ntp_servers: data.ntp_servers.clone(),
            is_public: data.is_public,
            nic_count: data.nic_count,
            created_at: data.created_at.clone(),
            updated_at: data.updated_at.clone(),
        }
    }
}

/// Create a new network
#[utoipa::path(
    post,
    path = "/api/v1/networks",
    request_body = CreateNetworkRequest,
    responses(
        (status = 200, description = "Network created", body = Network),
        (status = 409, description = "Network name already exists", body = ApiError),
        (status = 503, description = "Not the leader", body = ApiError)
    ),
    tag = "networks"
)]
pub async fn create_network(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateNetworkRequest>,
) -> Result<Json<Network>, ApiError> {
    let cmd = Command::CreateNetwork {
        request_id: uuid::Uuid::new_v4().to_string(),
        id: uuid::Uuid::new_v4().to_string(),
        name: req.name.clone(),
        ipv4_enabled: req.ipv4_enabled.unwrap_or(true),
        ipv4_subnet: req.ipv4_subnet,
        ipv6_enabled: req.ipv6_enabled.unwrap_or(false),
        ipv6_prefix: req.ipv6_prefix,
        dns_servers: req.dns_servers.unwrap_or_default(),
        ntp_servers: req.ntp_servers.unwrap_or_default(),
        is_public: req.is_public.unwrap_or(false),
    };

    match write_command(&state, cmd).await? {
        CmdResponse::Network(data) => {
            state.audit.network_created(&data.id, &data.name);
            Ok(Json(data.into()))
        }
        CmdResponse::Error { code, message } => Err(ApiError {
            error: message,
            code,
        }),
        _ => Err(ApiError {
            error: "Unexpected response".to_string(),
            code: 500,
        }),
    }
}

/// Get a network by ID or name
#[utoipa::path(
    get,
    path = "/api/v1/networks/{id}",
    params(
        ("id" = String, Path, description = "Network ID or name")
    ),
    responses(
        (status = 200, description = "Network found", body = Network),
        (status = 404, description = "Network not found", body = ApiError)
    ),
    tag = "networks"
)]
pub async fn get_network(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Network>, ApiError> {
    let node = state.node.read().await;
    let cp_state = node.get_state().await;

    // Try by ID first, then by name
    let network = cp_state
        .get_network(&id)
        .or_else(|| cp_state.get_network_by_name(&id));

    match network {
        Some(data) => Ok(Json(data.into())),
        None => Err(ApiError {
            error: "Network not found".to_string(),
            code: 404,
        }),
    }
}

/// List all networks
#[utoipa::path(
    get,
    path = "/api/v1/networks",
    responses(
        (status = 200, description = "List of networks", body = Vec<Network>)
    ),
    tag = "networks"
)]
pub async fn list_networks(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<Network>>, ApiError> {
    let node = state.node.read().await;
    let cp_state = node.get_state().await;
    let networks = cp_state.list_networks();
    Ok(Json(networks.into_iter().map(|n| n.into()).collect()))
}

/// Request to update a network
#[derive(Deserialize, ToSchema)]
pub struct UpdateNetworkRequest {
    /// DNS servers to announce via DHCP
    pub dns_servers: Option<Vec<String>>,
    /// NTP servers to announce via DHCP
    pub ntp_servers: Option<Vec<String>>,
}

/// Update a network
#[utoipa::path(
    patch,
    path = "/api/v1/networks/{id}",
    params(
        ("id" = String, Path, description = "Network ID")
    ),
    request_body = UpdateNetworkRequest,
    responses(
        (status = 200, description = "Network updated", body = Network),
        (status = 404, description = "Network not found", body = ApiError),
        (status = 503, description = "Not the leader", body = ApiError)
    ),
    tag = "networks"
)]
pub async fn update_network(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<UpdateNetworkRequest>,
) -> Result<Json<Network>, ApiError> {
    let cmd = Command::UpdateNetwork {
        request_id: uuid::Uuid::new_v4().to_string(),
        id,
        dns_servers: req.dns_servers.unwrap_or_default(),
        ntp_servers: req.ntp_servers.unwrap_or_default(),
    };

    match write_command(&state, cmd).await? {
        CmdResponse::Network(data) => {
            state.audit.network_updated(&data.id);
            Ok(Json(data.into()))
        }
        CmdResponse::Error { code, message } => Err(ApiError {
            error: message,
            code,
        }),
        _ => Err(ApiError {
            error: "Unexpected response".to_string(),
            code: 500,
        }),
    }
}

/// Query parameters for delete network
#[derive(Deserialize, ToSchema)]
pub struct DeleteNetworkQuery {
    /// Force delete even if NICs exist
    pub force: Option<bool>,
}

/// Response for delete network
#[derive(Serialize, ToSchema)]
pub struct DeleteNetworkResponse {
    pub deleted: bool,
    pub nics_deleted: u32,
}

/// Delete a network
#[utoipa::path(
    delete,
    path = "/api/v1/networks/{id}",
    params(
        ("id" = String, Path, description = "Network ID"),
        ("force" = Option<bool>, Query, description = "Force delete even if NICs exist")
    ),
    responses(
        (status = 200, description = "Network deleted", body = DeleteNetworkResponse),
        (status = 404, description = "Network not found", body = ApiError),
        (status = 409, description = "Network has NICs", body = ApiError),
        (status = 503, description = "Not the leader", body = ApiError)
    ),
    tag = "networks"
)]
pub async fn delete_network(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(query): Query<DeleteNetworkQuery>,
) -> Result<Json<DeleteNetworkResponse>, ApiError> {
    let cmd = Command::DeleteNetwork {
        request_id: uuid::Uuid::new_v4().to_string(),
        id: id.clone(),
        force: query.force.unwrap_or(false),
    };

    match write_command(&state, cmd).await? {
        CmdResponse::Deleted { .. } => {
            state.audit.network_deleted(&id);
            Ok(Json(DeleteNetworkResponse {
                deleted: true,
                nics_deleted: 0,
            }))
        }
        CmdResponse::DeletedWithCount { nics_deleted, .. } => {
            state.audit.network_deleted(&id);
            Ok(Json(DeleteNetworkResponse {
                deleted: true,
                nics_deleted,
            }))
        }
        CmdResponse::Error { code, message } => Err(ApiError {
            error: message,
            code,
        }),
        _ => Err(ApiError {
            error: "Unexpected response".to_string(),
            code: 500,
        }),
    }
}

// === NIC CRUD ===

/// Request to create a NIC
#[derive(Deserialize, ToSchema)]
pub struct CreateNicRequest {
    /// Network ID to attach the NIC to
    pub network_id: String,
    /// Optional friendly name
    pub name: Option<String>,
    /// MAC address (auto-generated if not provided)
    pub mac_address: Option<String>,
    /// IPv4 address (auto-allocated if not provided)
    pub ipv4_address: Option<String>,
    /// IPv6 address (auto-allocated if not provided)
    pub ipv6_address: Option<String>,
    /// Routed IPv4 prefixes
    pub routed_ipv4_prefixes: Option<Vec<String>>,
    /// Routed IPv6 prefixes
    pub routed_ipv6_prefixes: Option<Vec<String>>,
}

/// NIC resource
#[derive(Serialize, ToSchema)]
pub struct Nic {
    pub id: String,
    pub name: Option<String>,
    pub network_id: String,
    pub mac_address: String,
    pub ipv4_address: Option<String>,
    pub ipv6_address: Option<String>,
    pub routed_ipv4_prefixes: Vec<String>,
    pub routed_ipv6_prefixes: Vec<String>,
    pub socket_path: String,
    pub state: String,
    pub created_at: String,
    pub updated_at: String,
}

impl From<NicData> for Nic {
    fn from(data: NicData) -> Self {
        Self {
            id: data.id,
            name: data.name,
            network_id: data.network_id,
            mac_address: data.mac_address,
            ipv4_address: data.ipv4_address,
            ipv6_address: data.ipv6_address,
            routed_ipv4_prefixes: data.routed_ipv4_prefixes,
            routed_ipv6_prefixes: data.routed_ipv6_prefixes,
            socket_path: data.socket_path,
            state: format!("{:?}", data.state),
            created_at: data.created_at,
            updated_at: data.updated_at,
        }
    }
}

impl From<&NicData> for Nic {
    fn from(data: &NicData) -> Self {
        Self {
            id: data.id.clone(),
            name: data.name.clone(),
            network_id: data.network_id.clone(),
            mac_address: data.mac_address.clone(),
            ipv4_address: data.ipv4_address.clone(),
            ipv6_address: data.ipv6_address.clone(),
            routed_ipv4_prefixes: data.routed_ipv4_prefixes.clone(),
            routed_ipv6_prefixes: data.routed_ipv6_prefixes.clone(),
            socket_path: data.socket_path.clone(),
            state: format!("{:?}", data.state),
            created_at: data.created_at.clone(),
            updated_at: data.updated_at.clone(),
        }
    }
}

/// Create a new NIC
#[utoipa::path(
    post,
    path = "/api/v1/nics",
    request_body = CreateNicRequest,
    responses(
        (status = 200, description = "NIC created", body = Nic),
        (status = 404, description = "Network not found", body = ApiError),
        (status = 503, description = "Not the leader", body = ApiError)
    ),
    tag = "nics"
)]
pub async fn create_nic(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateNicRequest>,
) -> Result<Json<Nic>, ApiError> {
    let cmd = Command::CreateNic {
        request_id: uuid::Uuid::new_v4().to_string(),
        id: uuid::Uuid::new_v4().to_string(),
        network_id: req.network_id,
        name: req.name,
        mac_address: req.mac_address,
        ipv4_address: req.ipv4_address,
        ipv6_address: req.ipv6_address,
        routed_ipv4_prefixes: req.routed_ipv4_prefixes.unwrap_or_default(),
        routed_ipv6_prefixes: req.routed_ipv6_prefixes.unwrap_or_default(),
    };

    match write_command(&state, cmd).await? {
        CmdResponse::Nic(data) => {
            state
                .audit
                .nic_created(&data.id, &data.network_id, &data.mac_address);
            Ok(Json(data.into()))
        }
        CmdResponse::Error { code, message } => Err(ApiError {
            error: message,
            code,
        }),
        _ => Err(ApiError {
            error: "Unexpected response".to_string(),
            code: 500,
        }),
    }
}

/// Get a NIC by ID or name
#[utoipa::path(
    get,
    path = "/api/v1/nics/{id}",
    params(
        ("id" = String, Path, description = "NIC ID or name")
    ),
    responses(
        (status = 200, description = "NIC found", body = Nic),
        (status = 404, description = "NIC not found", body = ApiError)
    ),
    tag = "nics"
)]
pub async fn get_nic(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Nic>, ApiError> {
    let node = state.node.read().await;
    let cp_state = node.get_state().await;

    // Try by ID first, then by name
    let nic = cp_state
        .get_nic(&id)
        .or_else(|| cp_state.get_nic_by_name(&id));

    match nic {
        Some(data) => Ok(Json(data.into())),
        None => Err(ApiError {
            error: "NIC not found".to_string(),
            code: 404,
        }),
    }
}

/// Query parameters for list NICs
#[derive(Deserialize, ToSchema)]
pub struct ListNicsQuery {
    /// Filter by network ID
    pub network_id: Option<String>,
}

/// List all NICs
#[utoipa::path(
    get,
    path = "/api/v1/nics",
    params(
        ("network_id" = Option<String>, Query, description = "Filter by network ID")
    ),
    responses(
        (status = 200, description = "List of NICs", body = Vec<Nic>)
    ),
    tag = "nics"
)]
pub async fn list_nics(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListNicsQuery>,
) -> Result<Json<Vec<Nic>>, ApiError> {
    let node = state.node.read().await;
    let cp_state = node.get_state().await;
    let nics = cp_state.list_nics(query.network_id.as_deref());
    Ok(Json(nics.into_iter().map(|n| n.into()).collect()))
}

/// Request to update a NIC
#[derive(Deserialize, ToSchema)]
pub struct UpdateNicRequest {
    /// Routed IPv4 prefixes (replaces existing)
    pub routed_ipv4_prefixes: Option<Vec<String>>,
    /// Routed IPv6 prefixes (replaces existing)
    pub routed_ipv6_prefixes: Option<Vec<String>>,
}

/// Update a NIC
#[utoipa::path(
    patch,
    path = "/api/v1/nics/{id}",
    params(
        ("id" = String, Path, description = "NIC ID")
    ),
    request_body = UpdateNicRequest,
    responses(
        (status = 200, description = "NIC updated", body = Nic),
        (status = 404, description = "NIC not found", body = ApiError),
        (status = 503, description = "Not the leader", body = ApiError)
    ),
    tag = "nics"
)]
pub async fn update_nic(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<UpdateNicRequest>,
) -> Result<Json<Nic>, ApiError> {
    let cmd = Command::UpdateNic {
        request_id: uuid::Uuid::new_v4().to_string(),
        id,
        routed_ipv4_prefixes: req.routed_ipv4_prefixes.unwrap_or_default(),
        routed_ipv6_prefixes: req.routed_ipv6_prefixes.unwrap_or_default(),
    };

    match write_command(&state, cmd).await? {
        CmdResponse::Nic(data) => {
            state.audit.nic_updated(&data.id);
            Ok(Json(data.into()))
        }
        CmdResponse::Error { code, message } => Err(ApiError {
            error: message,
            code,
        }),
        _ => Err(ApiError {
            error: "Unexpected response".to_string(),
            code: 500,
        }),
    }
}

/// Response for delete NIC
#[derive(Serialize, ToSchema)]
pub struct DeleteNicResponse {
    pub deleted: bool,
}

/// Delete a NIC
#[utoipa::path(
    delete,
    path = "/api/v1/nics/{id}",
    params(
        ("id" = String, Path, description = "NIC ID")
    ),
    responses(
        (status = 200, description = "NIC deleted", body = DeleteNicResponse),
        (status = 404, description = "NIC not found", body = ApiError),
        (status = 503, description = "Not the leader", body = ApiError)
    ),
    tag = "nics"
)]
pub async fn delete_nic(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<DeleteNicResponse>, ApiError> {
    let cmd = Command::DeleteNic {
        request_id: uuid::Uuid::new_v4().to_string(),
        id: id.clone(),
    };

    match write_command(&state, cmd).await? {
        CmdResponse::Deleted { .. } => {
            state.audit.nic_deleted(&id);
            Ok(Json(DeleteNicResponse { deleted: true }))
        }
        CmdResponse::Error { code, message } => Err(ApiError {
            error: message,
            code,
        }),
        _ => Err(ApiError {
            error: "Unexpected response".to_string(),
            code: 500,
        }),
    }
}

// === Helper Functions ===

async fn write_command(state: &AppState, cmd: Command) -> Result<CmdResponse, ApiError> {
    let node = state.node.read().await;

    // Use write_or_forward to automatically handle leader forwarding
    match node.write_or_forward(cmd).await {
        Ok(resp) => Ok(resp),
        Err(e) => Err(ApiError {
            error: format!("Raft write error: {}", e),
            code: 503,
        }),
    }
}
