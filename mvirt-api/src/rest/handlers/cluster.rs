use axum::{
    Json,
    extract::{Path, State},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use super::{ApiError, AppState};

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
    path = "/v1/cluster",
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
    path = "/v1/cluster/membership",
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
    path = "/v1/cluster/join-token",
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
    path = "/v1/cluster/nodes/{id}",
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
