use axum::{
    routing::{delete, get, post},
    Router,
};
use std::net::SocketAddr;
use tower_http::cors::{Any, CorsLayer};

mod routes;
mod state;

use state::AppState;

#[tokio::main]
async fn main() {
    let state = AppState::new();

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        // VM routes
        .route(
            "/api/v1/vms",
            get(routes::vm::list_vms).post(routes::vm::create_vm),
        )
        .route(
            "/api/v1/vms/:id",
            get(routes::vm::get_vm).delete(routes::vm::delete_vm),
        )
        .route("/api/v1/vms/:id/start", post(routes::vm::start_vm))
        .route("/api/v1/vms/:id/stop", post(routes::vm::stop_vm))
        .route("/api/v1/vms/:id/kill", post(routes::vm::kill_vm))
        .route("/api/v1/vms/:id/console", get(routes::vm::console_ws))
        .route("/api/v1/events/vms", get(routes::vm::vm_events))
        // Storage routes
        .route(
            "/api/v1/storage/volumes",
            get(routes::storage::list_volumes).post(routes::storage::create_volume),
        )
        .route(
            "/api/v1/storage/volumes/:id",
            get(routes::storage::get_volume).delete(routes::storage::delete_volume),
        )
        .route(
            "/api/v1/storage/volumes/:id/resize",
            post(routes::storage::resize_volume),
        )
        .route(
            "/api/v1/storage/volumes/:id/snapshots",
            post(routes::storage::create_snapshot),
        )
        .route(
            "/api/v1/storage/templates",
            get(routes::storage::list_templates),
        )
        .route(
            "/api/v1/storage/templates/import",
            post(routes::storage::import_template),
        )
        .route(
            "/api/v1/storage/import-jobs/:id",
            get(routes::storage::get_import_job),
        )
        .route("/api/v1/storage/pool", get(routes::storage::get_pool_stats))
        // Network routes
        .route(
            "/api/v1/networks",
            get(routes::network::list_networks).post(routes::network::create_network),
        )
        .route(
            "/api/v1/networks/:id",
            get(routes::network::get_network).delete(routes::network::delete_network),
        )
        .route(
            "/api/v1/nics",
            get(routes::network::list_nics).post(routes::network::create_nic),
        )
        .route(
            "/api/v1/nics/:id",
            get(routes::network::get_nic).delete(routes::network::delete_nic),
        )
        .route("/api/v1/nics/:id/attach", post(routes::network::attach_nic))
        .route("/api/v1/nics/:id/detach", post(routes::network::detach_nic))
        // Log routes
        .route("/api/v1/logs", get(routes::log::query_logs))
        .route("/api/v1/logs/stream", get(routes::log::log_stream))
        // System routes
        .route("/api/v1/system", get(routes::system::get_system_info))
        // Cluster routes
        .route("/api/v1/cluster", get(routes::cluster::get_cluster_info))
        .route("/api/v1/cluster/nodes", get(routes::cluster::get_nodes))
        // Notification routes
        .route("/api/v1/notifications", get(routes::notification::get_notifications))
        .route("/api/v1/notifications/:id/read", post(routes::notification::mark_notification_read))
        .route("/api/v1/notifications/read-all", post(routes::notification::mark_all_read))
        .layer(cors)
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], 8080));
    println!("Mock server listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
