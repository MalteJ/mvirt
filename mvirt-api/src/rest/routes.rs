use axum::{
    Router,
    routing::{delete, get, patch, post},
};
use std::sync::Arc;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use super::handlers::{self, AppState};
use super::ui_handlers;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "mvirt API Server",
        version = "0.1.0",
        description = "REST API for the mvirt API Server. Provides distributed state management for Nodes, Networks, NICs, and VMs via Raft consensus.",
        license(name = "MIT")
    ),
    tags(
        (name = "system", description = "System information"),
        (name = "cluster", description = "Cluster management"),
        (name = "nodes", description = "Hypervisor node registration and status"),
        (name = "networks", description = "Network CRUD operations"),
        (name = "nics", description = "NIC CRUD operations"),
        (name = "vms", description = "VM CRUD operations")
    ),
    paths(
        handlers::get_version,
        handlers::get_cluster_info,
        handlers::get_membership,
        handlers::create_join_token,
        handlers::remove_node,
        handlers::register_hypervisor_node,
        handlers::get_hypervisor_node,
        handlers::list_hypervisor_nodes,
        handlers::update_hypervisor_node_status,
        handlers::deregister_hypervisor_node,
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
        handlers::create_vm,
        handlers::get_vm,
        handlers::list_vms,
        handlers::update_vm_spec,
        handlers::update_vm_status,
        handlers::delete_vm,
    ),
    components(schemas(
        handlers::VersionInfo,
        handlers::ClusterInfo,
        handlers::NodeInfo,
        handlers::ClusterMembership,
        handlers::MembershipNode,
        handlers::CreateJoinTokenRequest,
        handlers::CreateJoinTokenResponse,
        handlers::RemoveNodeRequest,
        handlers::RemoveNodeResponse,
        handlers::RegisterHypervisorNodeRequest,
        handlers::HypervisorNodeResources,
        handlers::HypervisorNode,
        handlers::ListNodesQuery,
        handlers::UpdateNodeStatusRequest,
        handlers::DeregisterNodeResponse,
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
        handlers::CreateVmRequest,
        handlers::UpdateVmSpecRequest,
        handlers::UpdateVmStatusRequestBody,
        handlers::VmSpecResponse,
        handlers::VmStatusResponse,
        handlers::Vm,
        handlers::ListVmsQuery,
        handlers::DeleteVmResponse,
    ))
)]
pub struct ApiDoc;

pub fn create_router(state: Arc<AppState>) -> Router {
    // Internal API routes (for hypervisor nodes, cluster management)
    let internal_routes = Router::new()
        // System
        .route("/version", get(handlers::get_version))
        // Cluster
        .route("/cluster", get(handlers::get_cluster_info))
        .route("/cluster/membership", get(handlers::get_membership))
        .route("/cluster/join-token", post(handlers::create_join_token))
        .route("/cluster/nodes/{id}", delete(handlers::remove_node))
        // Hypervisor Nodes
        .route("/nodes", get(handlers::list_hypervisor_nodes))
        .route("/nodes", post(handlers::register_hypervisor_node))
        .route("/nodes/{id}", get(handlers::get_hypervisor_node))
        .route(
            "/nodes/{id}/status",
            patch(handlers::update_hypervisor_node_status),
        )
        .route("/nodes/{id}", delete(handlers::deregister_hypervisor_node));

    // UI-compatible API routes
    let ui_routes = Router::new()
        // Projects
        .route("/projects", get(ui_handlers::list_projects))
        .route("/projects", post(ui_handlers::create_project))
        .route("/projects/{id}", get(ui_handlers::get_project))
        .route("/projects/{id}", delete(ui_handlers::delete_project))
        // VMs (UI-compatible)
        .route("/vms", get(ui_handlers::list_vms))
        .route("/vms", post(ui_handlers::create_vm))
        .route("/vms/{id}", get(ui_handlers::get_vm))
        .route("/vms/{id}", delete(ui_handlers::delete_vm))
        .route("/vms/{id}/start", post(ui_handlers::start_vm))
        .route("/vms/{id}/stop", post(ui_handlers::stop_vm))
        .route("/vms/{id}/kill", post(ui_handlers::kill_vm))
        .route("/vms/{id}/console", get(ui_handlers::console_ws))
        .route("/events/vms", get(ui_handlers::vm_events))
        // Networks (UI-compatible)
        .route("/networks", get(ui_handlers::list_networks))
        .route("/networks", post(ui_handlers::create_network))
        .route("/networks/{id}", get(ui_handlers::get_network))
        .route("/networks/{id}", delete(ui_handlers::delete_network))
        // NICs (UI-compatible)
        .route("/nics", get(ui_handlers::list_nics))
        .route("/nics", post(ui_handlers::create_nic))
        .route("/nics/{id}", get(ui_handlers::get_nic))
        .route("/nics/{id}", delete(ui_handlers::delete_nic))
        .route("/nics/{id}/attach", post(ui_handlers::attach_nic))
        .route("/nics/{id}/detach", post(ui_handlers::detach_nic))
        // Storage
        .route("/storage/volumes", get(ui_handlers::list_volumes))
        .route("/storage/volumes", post(ui_handlers::create_volume))
        .route("/storage/volumes/{id}", get(ui_handlers::get_volume))
        .route("/storage/volumes/{id}", delete(ui_handlers::delete_volume))
        .route(
            "/storage/volumes/{id}/resize",
            post(ui_handlers::resize_volume),
        )
        .route(
            "/storage/volumes/{id}/snapshots",
            post(ui_handlers::create_snapshot),
        )
        .route("/storage/templates", get(ui_handlers::list_templates))
        .route(
            "/storage/templates/import",
            post(ui_handlers::import_template),
        )
        .route(
            "/storage/import-jobs/{id}",
            get(ui_handlers::get_import_job),
        )
        .route("/storage/pool", get(ui_handlers::get_pool_stats))
        // Logs
        .route("/logs", get(ui_handlers::query_logs))
        .route("/logs/stream", get(ui_handlers::log_events))
        // Security Groups
        .route("/security-groups", get(ui_handlers::list_security_groups))
        .route("/security-groups", post(ui_handlers::create_security_group))
        .route(
            "/security-groups/{id}",
            get(ui_handlers::get_security_group),
        )
        .route(
            "/security-groups/{id}",
            delete(ui_handlers::delete_security_group),
        )
        .route(
            "/security-groups/{id}/rules",
            post(ui_handlers::create_security_group_rule),
        )
        .route(
            "/security-groups/{sg_id}/rules/{rule_id}",
            delete(ui_handlers::delete_security_group_rule),
        )
        // Notifications (stub)
        .route("/notifications", get(ui_handlers::list_notifications))
        .route(
            "/notifications/{id}/read",
            post(ui_handlers::mark_notification_read),
        )
        .route(
            "/notifications/read-all",
            post(ui_handlers::mark_all_notifications_read),
        );

    Router::new()
        .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi()))
        .nest("/api/v1", internal_routes)
        .nest("/api/v1", ui_routes)
        .with_state(state)
}
