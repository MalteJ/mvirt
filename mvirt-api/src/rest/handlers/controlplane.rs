use axum::{
    Json,
    extract::{Path, State},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use super::{ApiError, AppState};

/// Control plane information
#[derive(Serialize, ToSchema)]
pub struct ControlplaneInfo {
    pub cluster_id: String,
    pub leader_id: Option<u64>,
    pub current_term: u64,
    pub commit_index: u64,
    pub peers: Vec<PeerInfo>,
}

/// Peer information
#[derive(Serialize, ToSchema)]
pub struct PeerInfo {
    pub id: u64,
    pub name: String,
    pub address: String,
    pub state: String,
    pub is_leader: bool,
}

/// Get control plane information
#[utoipa::path(
    get,
    path = "/v1/controlplane",
    responses(
        (status = 200, description = "Control plane information", body = ControlplaneInfo)
    ),
    tag = "controlplane"
)]
pub async fn get_controlplane_info(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ControlplaneInfo>, ApiError> {
    let info = state.store.get_controlplane_info().await?;
    let membership = state.store.get_membership().await?;

    let peers: Vec<PeerInfo> = membership
        .peers
        .into_iter()
        .map(|n| PeerInfo {
            id: n.id,
            name: format!("peer-{}", n.id),
            address: n.address,
            state: if Some(n.id) == info.leader_id {
                "leader".to_string()
            } else {
                "follower".to_string()
            },
            is_leader: Some(n.id) == info.leader_id,
        })
        .collect();

    Ok(Json(ControlplaneInfo {
        cluster_id: info.cluster_id,
        leader_id: info.leader_id,
        current_term: info.current_term,
        commit_index: info.commit_index,
        peers,
    }))
}

/// Control plane membership information
#[derive(Serialize, ToSchema)]
pub struct ControlplaneMembership {
    pub voters: Vec<u64>,
    pub learners: Vec<u64>,
    pub peers: Vec<MembershipPeer>,
}

/// Peer in membership
#[derive(Serialize, ToSchema)]
pub struct MembershipPeer {
    pub id: u64,
    pub address: String,
    pub role: String,
}

/// Get control plane membership
#[utoipa::path(
    get,
    path = "/v1/controlplane/membership",
    responses(
        (status = 200, description = "Control plane membership", body = ControlplaneMembership)
    ),
    tag = "controlplane"
)]
pub async fn get_controlplane_membership(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ControlplaneMembership>, ApiError> {
    let membership = state.store.get_membership().await?;

    let peers: Vec<MembershipPeer> = membership
        .peers
        .into_iter()
        .map(|n| MembershipPeer {
            id: n.id,
            address: n.address,
            role: n.role,
        })
        .collect();

    Ok(Json(ControlplaneMembership {
        voters: membership.voters,
        learners: membership.learners,
        peers,
    }))
}

/// Request to create a join token
#[derive(Deserialize, ToSchema)]
pub struct CreateJoinTokenRequest {
    /// Peer ID that will use this token
    pub peer_id: u64,
    /// Token validity in seconds (default: 3600 = 1 hour)
    pub valid_for_secs: Option<u64>,
}

/// Join token response
#[derive(Serialize, ToSchema)]
pub struct CreateJoinTokenResponse {
    pub token: String,
    pub peer_id: u64,
    pub valid_for_secs: u64,
}

/// Create a join token for a new peer
#[utoipa::path(
    post,
    path = "/v1/controlplane/join-token",
    request_body = CreateJoinTokenRequest,
    responses(
        (status = 200, description = "Join token created", body = CreateJoinTokenResponse),
        (status = 503, description = "Not the leader or cluster secret not configured", body = ApiError)
    ),
    tag = "controlplane"
)]
pub async fn create_controlplane_join_token(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateJoinTokenRequest>,
) -> Result<Json<CreateJoinTokenResponse>, ApiError> {
    let valid_for = req.valid_for_secs.unwrap_or(3600);
    let token = state
        .store
        .create_join_token(req.peer_id, valid_for)
        .await
        .map_err(|e| ApiError {
            error: format!("Failed to create join token: {}", e),
            code: 503,
        })?;

    Ok(Json(CreateJoinTokenResponse {
        token,
        peer_id: req.peer_id,
        valid_for_secs: valid_for,
    }))
}

/// Request to remove a peer from the control plane
#[derive(Deserialize, ToSchema)]
#[allow(dead_code)]
pub struct RemovePeerRequest {
    /// Force remove even if it would break quorum (not yet implemented)
    pub force: Option<bool>,
}

/// Response for remove peer
#[derive(Serialize, ToSchema)]
pub struct RemovePeerResponse {
    pub removed: bool,
    pub peer_id: u64,
}

/// Remove a peer from the control plane
#[utoipa::path(
    delete,
    path = "/v1/controlplane/peers/{id}",
    params(
        ("id" = u64, Path, description = "Peer ID to remove")
    ),
    responses(
        (status = 200, description = "Peer removed", body = RemovePeerResponse),
        (status = 404, description = "Peer not found", body = ApiError),
        (status = 503, description = "Not the leader", body = ApiError)
    ),
    tag = "controlplane"
)]
pub async fn remove_peer(
    State(state): State<Arc<AppState>>,
    Path(peer_id): Path<u64>,
) -> Result<Json<RemovePeerResponse>, ApiError> {
    state.store.remove_peer(peer_id).await?;
    state.audit.peer_removed(peer_id);

    Ok(Json(RemovePeerResponse {
        removed: true,
        peer_id,
    }))
}
