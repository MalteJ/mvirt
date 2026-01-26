use axum::{
    Router,
    routing::{delete, get, patch, post},
};
use std::sync::Arc;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use super::handlers::{self, AppState};

#[derive(OpenApi)]
#[openapi(
    info(
        title = "mvirt Control Plane API",
        version = "0.1.0",
        description = "REST API for the mvirt Cluster Control Plane. Provides distributed state management for Networks and NICs via Raft consensus.",
        license(name = "MIT")
    ),
    tags(
        (name = "system", description = "System information"),
        (name = "cluster", description = "Cluster management"),
        (name = "networks", description = "Network CRUD operations"),
        (name = "nics", description = "NIC CRUD operations")
    ),
    paths(
        handlers::get_version,
        handlers::get_cluster_info,
        handlers::create_network,
        handlers::get_network,
        handlers::list_networks,
        handlers::update_network,
        handlers::delete_network,
        handlers::create_nic,
        handlers::get_nic,
        handlers::list_nics,
        handlers::update_nic,
        handlers::delete_nic,
    ),
    components(schemas(
        handlers::VersionInfo,
        handlers::ClusterInfo,
        handlers::NodeInfo,
        handlers::ApiError,
        handlers::CreateNetworkRequest,
        handlers::Network,
        handlers::UpdateNetworkRequest,
        handlers::DeleteNetworkQuery,
        handlers::DeleteNetworkResponse,
        handlers::CreateNicRequest,
        handlers::Nic,
        handlers::ListNicsQuery,
        handlers::UpdateNicRequest,
        handlers::DeleteNicResponse,
    ))
)]
pub struct ApiDoc;

pub fn create_router(state: Arc<AppState>) -> Router {
    let api_routes = Router::new()
        // System
        .route("/version", get(handlers::get_version))
        // Cluster
        .route("/cluster", get(handlers::get_cluster_info))
        // Networks
        .route("/networks", get(handlers::list_networks))
        .route("/networks", post(handlers::create_network))
        .route("/networks/{id}", get(handlers::get_network))
        .route("/networks/{id}", patch(handlers::update_network))
        .route("/networks/{id}", delete(handlers::delete_network))
        // NICs
        .route("/nics", get(handlers::list_nics))
        .route("/nics", post(handlers::create_nic))
        .route("/nics/{id}", get(handlers::get_nic))
        .route("/nics/{id}", patch(handlers::update_nic))
        .route("/nics/{id}", delete(handlers::delete_nic));

    Router::new()
        .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi()))
        .nest("/api/v1", api_routes)
        .with_state(state)
}
