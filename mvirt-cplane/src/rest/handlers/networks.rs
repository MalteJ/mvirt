use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use crate::command::NetworkData;
use crate::store::{
    CreateNetworkRequest as StoreCreateNetworkRequest,
    UpdateNetworkRequest as StoreUpdateNetworkRequest,
};

use super::{ApiError, AppState};

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
    path = "/v1/networks",
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
    path = "/v1/networks/{id}",
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
    path = "/v1/networks",
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
    path = "/v1/networks/{id}",
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
    path = "/v1/networks/{id}",
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
