// Allow dead code for legacy handlers during transition to UI-compatible API
#[allow(dead_code)]
mod controlplane;
#[allow(dead_code)]
mod networks;
#[allow(dead_code)]
mod nics;
#[allow(dead_code)]
mod nodes;
#[allow(dead_code)]
mod vms;

use axum::{Json, http::StatusCode, response::IntoResponse};
use mraft::NodeId;
use serde::Serialize;
use std::sync::Arc;
use utoipa::ToSchema;

use crate::audit::ApiAuditLogger;
use crate::store::{DataStore, StoreError};

#[allow(unused_imports)]
pub use controlplane::*;
#[allow(unused_imports)]
pub use networks::*;
#[allow(unused_imports)]
pub use nics::*;
#[allow(unused_imports)]
pub use nodes::*;
#[allow(unused_imports)]
pub use vms::*;

/// Shared application state
pub struct AppState {
    pub store: Arc<dyn DataStore>,
    pub audit: Arc<ApiAuditLogger>,
    pub node_id: NodeId,
    pub log_endpoint: String,
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
            501 => StatusCode::NOT_IMPLEMENTED,
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

/// Version information
#[derive(Serialize, ToSchema)]
pub struct VersionInfo {
    pub version: String,
}

/// Get service version
#[utoipa::path(
    get,
    path = "/v1/version",
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
