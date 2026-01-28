// Allow dead code for legacy handlers during transition to UI-compatible API
#![allow(dead_code)]

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use mraft::NodeId;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use crate::audit::ApiAuditLogger;
use crate::command::{
    NetworkData, NicData, NodeData, NodeResources, NodeStatus, VmData, VmDesiredState, VmPhase,
    VmSpec, VmStatus,
};
use crate::store::{
    CreateNetworkRequest as StoreCreateNetworkRequest, CreateNicRequest as StoreCreateNicRequest,
    CreateVmRequest as StoreCreateVmRequest, DataStore,
    RegisterNodeRequest as StoreRegisterNodeRequest, StoreError,
    UpdateNetworkRequest as StoreUpdateNetworkRequest, UpdateNicRequest as StoreUpdateNicRequest,
    UpdateNodeStatusRequest as StoreUpdateNodeStatusRequest,
    UpdateVmSpecRequest as StoreUpdateVmSpecRequest,
    UpdateVmStatusRequest as StoreUpdateVmStatusRequest,
};
use std::collections::HashMap;

/// Shared application state
pub struct AppState {
    pub store: Arc<dyn DataStore>,
    pub audit: Arc<ApiAuditLogger>,
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

impl From<StoreError> for ApiError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::NotFound(msg) => ApiError {
                error: msg,
                code: 404,
            },
            StoreError::Conflict(msg) => ApiError {
                error: msg,
                code: 409,
            },
            StoreError::NotLeader { .. } => ApiError {
                error: "Not leader".to_string(),
                code: 503,
            },
            StoreError::ScheduleFailed(msg) => ApiError {
                error: msg,
                code: 503, // Service unavailable - no nodes can handle the request
            },
            StoreError::Internal(msg) => ApiError {
                error: msg,
                code: 500,
            },
            StoreError::VersionMismatch { expected, actual } => ApiError {
                error: format!("Version mismatch: expected {}, got {}", expected, actual),
                code: 409,
            },
        }
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
    let info = state.store.get_cluster_info().await?;
    let membership = state.store.get_membership().await?;

    let nodes: Vec<NodeInfo> = membership
        .nodes
        .into_iter()
        .map(|n| NodeInfo {
            id: n.id,
            name: format!("node-{}", n.id),
            address: n.address,
            state: if Some(n.id) == info.leader_id {
                "leader".to_string()
            } else {
                "follower".to_string()
            },
            is_leader: Some(n.id) == info.leader_id,
        })
        .collect();

    Ok(Json(ClusterInfo {
        cluster_id: info.cluster_id,
        leader_id: info.leader_id,
        current_term: info.current_term,
        commit_index: info.commit_index,
        nodes,
    }))
}

// === Cluster Membership Management ===

/// Cluster membership information
#[derive(Serialize, ToSchema)]
pub struct ClusterMembership {
    pub voters: Vec<u64>,
    pub learners: Vec<u64>,
    pub nodes: Vec<MembershipNode>,
}

/// Node in membership
#[derive(Serialize, ToSchema)]
pub struct MembershipNode {
    pub id: u64,
    pub address: String,
    pub role: String,
}

/// Get cluster membership
#[utoipa::path(
    get,
    path = "/api/v1/cluster/membership",
    responses(
        (status = 200, description = "Cluster membership", body = ClusterMembership)
    ),
    tag = "cluster"
)]
pub async fn get_membership(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ClusterMembership>, ApiError> {
    let membership = state.store.get_membership().await?;

    let nodes: Vec<MembershipNode> = membership
        .nodes
        .into_iter()
        .map(|n| MembershipNode {
            id: n.id,
            address: n.address,
            role: n.role,
        })
        .collect();

    Ok(Json(ClusterMembership {
        voters: membership.voters,
        learners: membership.learners,
        nodes,
    }))
}

/// Request to create a join token
#[derive(Deserialize, ToSchema)]
pub struct CreateJoinTokenRequest {
    /// Node ID that will use this token
    pub node_id: u64,
    /// Token validity in seconds (default: 3600 = 1 hour)
    pub valid_for_secs: Option<u64>,
}

/// Join token response
#[derive(Serialize, ToSchema)]
pub struct CreateJoinTokenResponse {
    pub token: String,
    pub node_id: u64,
    pub valid_for_secs: u64,
}

/// Create a join token for a new node
#[utoipa::path(
    post,
    path = "/api/v1/cluster/join-token",
    request_body = CreateJoinTokenRequest,
    responses(
        (status = 200, description = "Join token created", body = CreateJoinTokenResponse),
        (status = 503, description = "Not the leader or cluster secret not configured", body = ApiError)
    ),
    tag = "cluster"
)]
pub async fn create_join_token(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateJoinTokenRequest>,
) -> Result<Json<CreateJoinTokenResponse>, ApiError> {
    let valid_for = req.valid_for_secs.unwrap_or(3600);
    let token = state
        .store
        .create_join_token(req.node_id, valid_for)
        .await
        .map_err(|e| ApiError {
            error: format!("Failed to create join token: {}", e),
            code: 503,
        })?;

    Ok(Json(CreateJoinTokenResponse {
        token,
        node_id: req.node_id,
        valid_for_secs: valid_for,
    }))
}

/// Request to remove a node from the cluster
#[derive(Deserialize, ToSchema)]
#[allow(dead_code)]
pub struct RemoveNodeRequest {
    /// Force remove even if it would break quorum (not yet implemented)
    pub force: Option<bool>,
}

/// Response for remove node
#[derive(Serialize, ToSchema)]
pub struct RemoveNodeResponse {
    pub removed: bool,
    pub node_id: u64,
}

/// Remove a node from the cluster
#[utoipa::path(
    delete,
    path = "/api/v1/cluster/nodes/{id}",
    params(
        ("id" = u64, Path, description = "Node ID to remove")
    ),
    responses(
        (status = 200, description = "Node removed", body = RemoveNodeResponse),
        (status = 404, description = "Node not found", body = ApiError),
        (status = 503, description = "Not the leader", body = ApiError)
    ),
    tag = "cluster"
)]
pub async fn remove_node(
    State(state): State<Arc<AppState>>,
    Path(node_id): Path<u64>,
) -> Result<Json<RemoveNodeResponse>, ApiError> {
    state.store.remove_node(node_id).await?;
    state.audit.node_removed(node_id);

    Ok(Json(RemoveNodeResponse {
        removed: true,
        node_id,
    }))
}

// === Hypervisor Nodes (Agent Registry) ===

/// Request to register a hypervisor node
#[derive(Deserialize, ToSchema)]
pub struct RegisterHypervisorNodeRequest {
    /// Unique node name (typically hostname)
    pub name: String,
    /// gRPC endpoint address for mvirt-node agent
    pub address: String,
    /// Node resource capacity
    pub resources: Option<HypervisorNodeResources>,
    /// Node labels for scheduling
    pub labels: Option<HashMap<String, String>>,
}

/// Node resource information
#[derive(Serialize, Deserialize, ToSchema, Clone)]
pub struct HypervisorNodeResources {
    pub cpu_cores: u32,
    pub memory_mb: u64,
    pub storage_gb: u64,
    pub available_cpu_cores: u32,
    pub available_memory_mb: u64,
    pub available_storage_gb: u64,
}

impl From<NodeResources> for HypervisorNodeResources {
    fn from(r: NodeResources) -> Self {
        Self {
            cpu_cores: r.cpu_cores,
            memory_mb: r.memory_mb,
            storage_gb: r.storage_gb,
            available_cpu_cores: r.available_cpu_cores,
            available_memory_mb: r.available_memory_mb,
            available_storage_gb: r.available_storage_gb,
        }
    }
}

impl From<HypervisorNodeResources> for NodeResources {
    fn from(r: HypervisorNodeResources) -> Self {
        Self {
            cpu_cores: r.cpu_cores,
            memory_mb: r.memory_mb,
            storage_gb: r.storage_gb,
            available_cpu_cores: r.available_cpu_cores,
            available_memory_mb: r.available_memory_mb,
            available_storage_gb: r.available_storage_gb,
        }
    }
}

/// Hypervisor node resource
#[derive(Serialize, ToSchema)]
pub struct HypervisorNode {
    pub id: String,
    pub name: String,
    pub address: String,
    pub status: String,
    pub resources: HypervisorNodeResources,
    pub labels: HashMap<String, String>,
    pub last_heartbeat: String,
    pub created_at: String,
    pub updated_at: String,
}

impl From<NodeData> for HypervisorNode {
    fn from(data: NodeData) -> Self {
        Self {
            id: data.id,
            name: data.name,
            address: data.address,
            status: format!("{:?}", data.status),
            resources: data.resources.into(),
            labels: data.labels,
            last_heartbeat: data.last_heartbeat,
            created_at: data.created_at,
            updated_at: data.updated_at,
        }
    }
}

/// Register a new hypervisor node
#[utoipa::path(
    post,
    path = "/api/v1/nodes",
    request_body = RegisterHypervisorNodeRequest,
    responses(
        (status = 200, description = "Node registered", body = HypervisorNode),
        (status = 409, description = "Node name already exists", body = ApiError),
        (status = 503, description = "Not the leader", body = ApiError)
    ),
    tag = "nodes"
)]
pub async fn register_hypervisor_node(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RegisterHypervisorNodeRequest>,
) -> Result<Json<HypervisorNode>, ApiError> {
    let store_req = StoreRegisterNodeRequest {
        name: req.name.clone(),
        address: req.address,
        resources: req.resources.map(|r| r.into()).unwrap_or_default(),
        labels: req.labels.unwrap_or_default(),
    };

    let data = state.store.register_node(store_req).await?;
    state.audit.hypervisor_node_registered(&data.id, &data.name);
    Ok(Json(data.into()))
}

/// Get a hypervisor node by ID or name
#[utoipa::path(
    get,
    path = "/api/v1/nodes/{id}",
    params(
        ("id" = String, Path, description = "Node ID or name")
    ),
    responses(
        (status = 200, description = "Node found", body = HypervisorNode),
        (status = 404, description = "Node not found", body = ApiError)
    ),
    tag = "nodes"
)]
pub async fn get_hypervisor_node(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<HypervisorNode>, ApiError> {
    // Try by ID first, then by name
    let node = state
        .store
        .get_node(&id)
        .await?
        .or(state.store.get_node_by_name(&id).await?);

    match node {
        Some(data) => Ok(Json(data.into())),
        None => Err(ApiError {
            error: "Node not found".to_string(),
            code: 404,
        }),
    }
}

/// Query parameters for list nodes
#[derive(Deserialize, ToSchema)]
pub struct ListNodesQuery {
    /// Filter by status (online, offline, unknown)
    pub status: Option<String>,
}

/// List all hypervisor nodes
#[utoipa::path(
    get,
    path = "/api/v1/nodes",
    params(
        ("status" = Option<String>, Query, description = "Filter by status (online, offline, unknown)")
    ),
    responses(
        (status = 200, description = "List of nodes", body = Vec<HypervisorNode>)
    ),
    tag = "nodes"
)]
pub async fn list_hypervisor_nodes(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListNodesQuery>,
) -> Result<Json<Vec<HypervisorNode>>, ApiError> {
    let nodes = match query.status.as_deref() {
        Some("online") => state.store.list_online_nodes().await?,
        _ => state.store.list_nodes().await?,
    };
    Ok(Json(nodes.into_iter().map(|n| n.into()).collect()))
}

/// Request to update node status
#[derive(Deserialize, ToSchema)]
pub struct UpdateNodeStatusRequest {
    /// Node status (online, offline, unknown)
    pub status: String,
    /// Updated resource information
    pub resources: Option<HypervisorNodeResources>,
}

/// Update hypervisor node status (heartbeat)
#[utoipa::path(
    patch,
    path = "/api/v1/nodes/{id}/status",
    params(
        ("id" = String, Path, description = "Node ID")
    ),
    request_body = UpdateNodeStatusRequest,
    responses(
        (status = 200, description = "Node status updated", body = HypervisorNode),
        (status = 404, description = "Node not found", body = ApiError),
        (status = 503, description = "Not the leader", body = ApiError)
    ),
    tag = "nodes"
)]
pub async fn update_hypervisor_node_status(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<UpdateNodeStatusRequest>,
) -> Result<Json<HypervisorNode>, ApiError> {
    let status = match req.status.to_lowercase().as_str() {
        "online" => NodeStatus::Online,
        "offline" => NodeStatus::Offline,
        _ => NodeStatus::Unknown,
    };

    let store_req = StoreUpdateNodeStatusRequest {
        status,
        resources: req.resources.map(|r| r.into()),
    };

    let data = state.store.update_node_status(&id, store_req).await?;
    Ok(Json(data.into()))
}

/// Response for deregister node
#[derive(Serialize, ToSchema)]
pub struct DeregisterNodeResponse {
    pub deregistered: bool,
}

/// Deregister a hypervisor node
#[utoipa::path(
    delete,
    path = "/api/v1/nodes/{id}",
    params(
        ("id" = String, Path, description = "Node ID")
    ),
    responses(
        (status = 200, description = "Node deregistered", body = DeregisterNodeResponse),
        (status = 404, description = "Node not found", body = ApiError),
        (status = 503, description = "Not the leader", body = ApiError)
    ),
    tag = "nodes"
)]
pub async fn deregister_hypervisor_node(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<DeregisterNodeResponse>, ApiError> {
    state.store.deregister_node(&id).await?;
    state.audit.hypervisor_node_deregistered(&id);
    Ok(Json(DeregisterNodeResponse { deregistered: true }))
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
    pub ipv4_prefix: Option<String>,
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
    pub ipv4_prefix: Option<String>,
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
            ipv4_prefix: data.ipv4_prefix,
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
            ipv4_prefix: data.ipv4_prefix.clone(),
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
    let store_req = StoreCreateNetworkRequest {
        project_id: String::new(), // Legacy handler — no project_id
        name: req.name.clone(),
        ipv4_enabled: req.ipv4_enabled.unwrap_or(true),
        ipv4_prefix: req.ipv4_prefix,
        ipv6_enabled: req.ipv6_enabled.unwrap_or(false),
        ipv6_prefix: req.ipv6_prefix,
        dns_servers: req.dns_servers.unwrap_or_default(),
        ntp_servers: req.ntp_servers.unwrap_or_default(),
        is_public: req.is_public.unwrap_or(false),
    };

    let data = state.store.create_network(store_req).await?;
    state.audit.network_created(&data.id, &data.name);
    Ok(Json(data.into()))
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
    // Try by ID first, then by name
    let network = state
        .store
        .get_network(&id)
        .await?
        .or(state.store.get_network_by_name(&id).await?);

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
    let networks = state.store.list_networks().await?;
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
    let store_req = StoreUpdateNetworkRequest {
        dns_servers: req.dns_servers.unwrap_or_default(),
        ntp_servers: req.ntp_servers.unwrap_or_default(),
    };

    let data = state.store.update_network(&id, store_req).await?;
    state.audit.network_updated(&data.id);
    Ok(Json(data.into()))
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
    let force = query.force.unwrap_or(false);
    let result = state.store.delete_network(&id, force).await?;
    state.audit.network_deleted(&id);
    Ok(Json(DeleteNetworkResponse {
        deleted: true,
        nics_deleted: result.nics_deleted,
    }))
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
    let store_req = StoreCreateNicRequest {
        project_id: String::new(), // Legacy handler — no project_id
        network_id: req.network_id,
        name: req.name,
        mac_address: req.mac_address,
        ipv4_address: req.ipv4_address,
        ipv6_address: req.ipv6_address,
        routed_ipv4_prefixes: req.routed_ipv4_prefixes.unwrap_or_default(),
        routed_ipv6_prefixes: req.routed_ipv6_prefixes.unwrap_or_default(),
        security_group_id: None,
    };

    let data = state.store.create_nic(store_req).await?;
    state
        .audit
        .nic_created(&data.id, &data.network_id, &data.mac_address);
    Ok(Json(data.into()))
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
    // Try by ID first, then by name
    let nic = state
        .store
        .get_nic(&id)
        .await?
        .or(state.store.get_nic_by_name(&id).await?);

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
    let nics = state.store.list_nics(query.network_id.as_deref()).await?;
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
    let store_req = StoreUpdateNicRequest {
        routed_ipv4_prefixes: req.routed_ipv4_prefixes.unwrap_or_default(),
        routed_ipv6_prefixes: req.routed_ipv6_prefixes.unwrap_or_default(),
    };

    let data = state.store.update_nic(&id, store_req).await?;
    state.audit.nic_updated(&data.id);
    Ok(Json(data.into()))
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
    state.store.delete_nic(&id).await?;
    state.audit.nic_deleted(&id);
    Ok(Json(DeleteNicResponse { deleted: true }))
}

// === VM CRUD ===

/// Request to create a VM
#[derive(Deserialize, ToSchema)]
pub struct CreateVmRequest {
    /// VM name
    pub name: String,
    /// Project ID
    pub project_id: String,
    /// Optional: require specific node (by ID or name)
    pub node_selector: Option<String>,
    /// CPU cores
    pub cpu_cores: u32,
    /// Memory in MB
    pub memory_mb: u64,
    /// Volume ID (boot volume)
    pub volume_id: String,
    /// NIC ID
    pub nic_id: String,
    /// Boot image reference
    pub image: String,
    /// Initial desired state (default: Running)
    pub desired_state: Option<String>,
}

/// Request to update a VM spec (desired state)
#[derive(Deserialize, ToSchema)]
pub struct UpdateVmSpecRequest {
    /// Desired power state: running or stopped
    pub desired_state: String,
}

/// Request to update a VM status (from node)
#[derive(Deserialize, ToSchema)]
pub struct UpdateVmStatusRequestBody {
    /// VM phase: pending, scheduled, creating, running, stopping, stopped, failed
    pub phase: String,
    /// Assigned node ID
    pub node_id: Option<String>,
    /// Assigned IP address
    pub ip_address: Option<String>,
    /// Error or status message
    pub message: Option<String>,
}

/// VM spec (desired state)
#[derive(Serialize, ToSchema)]
pub struct VmSpecResponse {
    pub name: String,
    pub project_id: String,
    pub node_selector: Option<String>,
    pub cpu_cores: u32,
    pub memory_mb: u64,
    pub volume_id: String,
    pub nic_id: String,
    pub image: String,
    pub desired_state: String,
}

impl From<VmSpec> for VmSpecResponse {
    fn from(spec: VmSpec) -> Self {
        Self {
            name: spec.name,
            project_id: spec.project_id,
            node_selector: spec.node_selector,
            cpu_cores: spec.cpu_cores,
            memory_mb: spec.memory_mb,
            volume_id: spec.volume_id,
            nic_id: spec.nic_id,
            image: spec.image,
            desired_state: format!("{:?}", spec.desired_state),
        }
    }
}

/// VM status (actual state)
#[derive(Serialize, ToSchema)]
pub struct VmStatusResponse {
    pub phase: String,
    pub node_id: Option<String>,
    pub ip_address: Option<String>,
    pub message: Option<String>,
}

impl From<VmStatus> for VmStatusResponse {
    fn from(status: VmStatus) -> Self {
        Self {
            phase: format!("{:?}", status.phase),
            node_id: status.node_id,
            ip_address: status.ip_address,
            message: status.message,
        }
    }
}

/// VM resource
#[derive(Serialize, ToSchema)]
pub struct Vm {
    pub id: String,
    pub spec: VmSpecResponse,
    pub status: VmStatusResponse,
    pub created_at: String,
    pub updated_at: String,
}

impl From<VmData> for Vm {
    fn from(data: VmData) -> Self {
        Self {
            id: data.id,
            spec: data.spec.into(),
            status: data.status.into(),
            created_at: data.created_at,
            updated_at: data.updated_at,
        }
    }
}

/// Create a new VM
#[utoipa::path(
    post,
    path = "/api/v1/vms",
    request_body = CreateVmRequest,
    responses(
        (status = 200, description = "VM created", body = Vm),
        (status = 404, description = "Network not found", body = ApiError),
        (status = 409, description = "VM name already exists", body = ApiError),
        (status = 503, description = "Not the leader", body = ApiError)
    ),
    tag = "vms"
)]
pub async fn create_vm(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateVmRequest>,
) -> Result<Json<Vm>, ApiError> {
    let desired_state = match req.desired_state.as_deref() {
        Some("stopped") | Some("Stopped") => VmDesiredState::Stopped,
        _ => VmDesiredState::Running,
    };

    let spec = VmSpec {
        name: req.name.clone(),
        project_id: req.project_id,
        node_selector: req.node_selector,
        cpu_cores: req.cpu_cores,
        memory_mb: req.memory_mb,
        volume_id: req.volume_id,
        nic_id: req.nic_id,
        image: req.image,
        desired_state,
    };

    let store_req = StoreCreateVmRequest { spec };

    // Create and schedule the VM to a node
    let data = state.store.create_and_schedule_vm(store_req).await?;
    state.audit.vm_created(&data.id, &data.spec.name);
    Ok(Json(data.into()))
}

/// Get a VM by ID or name
#[utoipa::path(
    get,
    path = "/api/v1/vms/{id}",
    params(
        ("id" = String, Path, description = "VM ID or name")
    ),
    responses(
        (status = 200, description = "VM found", body = Vm),
        (status = 404, description = "VM not found", body = ApiError)
    ),
    tag = "vms"
)]
pub async fn get_vm(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Vm>, ApiError> {
    // Try by ID first, then by name
    let vm = state
        .store
        .get_vm(&id)
        .await?
        .or(state.store.get_vm_by_name(&id).await?);

    match vm {
        Some(data) => Ok(Json(data.into())),
        None => Err(ApiError {
            error: "VM not found".to_string(),
            code: 404,
        }),
    }
}

/// Query parameters for list VMs
#[derive(Deserialize, ToSchema)]
pub struct ListVmsQuery {
    /// Filter by node ID
    pub node_id: Option<String>,
    /// Filter by phase
    pub phase: Option<String>,
}

/// List all VMs
#[utoipa::path(
    get,
    path = "/api/v1/vms",
    params(
        ("node_id" = Option<String>, Query, description = "Filter by node ID"),
        ("phase" = Option<String>, Query, description = "Filter by phase")
    ),
    responses(
        (status = 200, description = "List of VMs", body = Vec<Vm>)
    ),
    tag = "vms"
)]
pub async fn list_vms(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListVmsQuery>,
) -> Result<Json<Vec<Vm>>, ApiError> {
    let vms = match &query.node_id {
        Some(node_id) => state.store.list_vms_by_node(node_id).await?,
        None => state.store.list_vms().await?,
    };

    // Optional: filter by phase
    let vms: Vec<Vm> = vms
        .into_iter()
        .filter(|vm| {
            if let Some(ref phase) = query.phase {
                format!("{:?}", vm.status.phase).to_lowercase() == phase.to_lowercase()
            } else {
                true
            }
        })
        .map(|v| v.into())
        .collect();

    Ok(Json(vms))
}

/// Update a VM's spec (desired state)
#[utoipa::path(
    patch,
    path = "/api/v1/vms/{id}/spec",
    params(
        ("id" = String, Path, description = "VM ID")
    ),
    request_body = UpdateVmSpecRequest,
    responses(
        (status = 200, description = "VM spec updated", body = Vm),
        (status = 404, description = "VM not found", body = ApiError),
        (status = 503, description = "Not the leader", body = ApiError)
    ),
    tag = "vms"
)]
pub async fn update_vm_spec(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<UpdateVmSpecRequest>,
) -> Result<Json<Vm>, ApiError> {
    let desired_state = match req.desired_state.to_lowercase().as_str() {
        "stopped" => VmDesiredState::Stopped,
        _ => VmDesiredState::Running,
    };

    let store_req = StoreUpdateVmSpecRequest { desired_state };

    let data = state.store.update_vm_spec(&id, store_req).await?;
    state.audit.vm_spec_updated(&data.id);
    Ok(Json(data.into()))
}

/// Update a VM's status (from node)
#[utoipa::path(
    patch,
    path = "/api/v1/vms/{id}/status",
    params(
        ("id" = String, Path, description = "VM ID")
    ),
    request_body = UpdateVmStatusRequestBody,
    responses(
        (status = 200, description = "VM status updated", body = Vm),
        (status = 404, description = "VM not found", body = ApiError),
        (status = 503, description = "Not the leader", body = ApiError)
    ),
    tag = "vms"
)]
pub async fn update_vm_status(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<UpdateVmStatusRequestBody>,
) -> Result<Json<Vm>, ApiError> {
    let phase = match req.phase.to_lowercase().as_str() {
        "pending" => VmPhase::Pending,
        "scheduled" => VmPhase::Scheduled,
        "creating" => VmPhase::Creating,
        "running" => VmPhase::Running,
        "stopping" => VmPhase::Stopping,
        "stopped" => VmPhase::Stopped,
        "failed" => VmPhase::Failed,
        _ => VmPhase::Pending,
    };

    let status = VmStatus {
        phase,
        node_id: req.node_id,
        ip_address: req.ip_address,
        message: req.message,
    };

    let store_req = StoreUpdateVmStatusRequest { status };

    let data = state.store.update_vm_status(&id, store_req).await?;
    state.audit.vm_status_updated(&data.id);
    Ok(Json(data.into()))
}

/// Response for delete VM
#[derive(Serialize, ToSchema)]
pub struct DeleteVmResponse {
    pub deleted: bool,
}

/// Delete a VM
#[utoipa::path(
    delete,
    path = "/api/v1/vms/{id}",
    params(
        ("id" = String, Path, description = "VM ID")
    ),
    responses(
        (status = 200, description = "VM deleted", body = DeleteVmResponse),
        (status = 404, description = "VM not found", body = ApiError),
        (status = 503, description = "Not the leader", body = ApiError)
    ),
    tag = "vms"
)]
pub async fn delete_vm(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<DeleteVmResponse>, ApiError> {
    state.store.delete_vm(&id).await?;
    state.audit.vm_deleted(&id);
    Ok(Json(DeleteVmResponse { deleted: true }))
}
