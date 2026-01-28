use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use utoipa::ToSchema;

use crate::command::{NodeData, NodeResources, NodeStatus};
use crate::store::{
    RegisterNodeRequest as StoreRegisterNodeRequest,
    UpdateNodeStatusRequest as StoreUpdateNodeStatusRequest,
};

use super::{ApiError, AppState};

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
    path = "/v1/nodes",
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
    path = "/v1/nodes/{id}",
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
    path = "/v1/nodes",
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
    path = "/v1/nodes/{id}/status",
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
    path = "/v1/nodes/{id}",
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
