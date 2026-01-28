use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use crate::command::NicData;
use crate::store::{
    CreateNicRequest as StoreCreateNicRequest, UpdateNicRequest as StoreUpdateNicRequest,
};

use super::{ApiError, AppState};

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
    path = "/v1/nics",
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
    path = "/v1/nics/{id}",
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
    path = "/v1/nics",
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
    path = "/v1/nics/{id}",
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
    path = "/v1/nics/{id}",
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
