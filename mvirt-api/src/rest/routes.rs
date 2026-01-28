use axum::{
    Router,
    routing::{delete, get, patch, post},
};
use std::sync::Arc;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use super::handlers::{self, AppState};
use super::ui_handlers;
use super::ui_types;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "mvirt API Server",
        version = "0.1.0",
        description = "REST API for the mvirt API Server. Provides distributed state management for Nodes, Networks, NICs, VMs, Volumes, Templates, Security Groups, and Projects via Raft consensus.",
        license(name = "MIT")
    ),
    tags(
        (name = "system", description = "System information"),
        (name = "cluster", description = "Cluster management"),
        (name = "nodes", description = "Hypervisor node registration and status"),
        (name = "projects", description = "Project management"),
        (name = "networks", description = "Network CRUD operations"),
        (name = "nics", description = "NIC CRUD operations"),
        (name = "vms", description = "VM CRUD and lifecycle operations"),
        (name = "storage", description = "Volumes, templates, and storage pool"),
        (name = "security-groups", description = "Security group and firewall rule management"),
        (name = "logs", description = "Audit log queries")
    ),
    paths(
        // System & Cluster (internal)
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
        // Projects
        ui_handlers::list_projects,
        ui_handlers::get_project,
        ui_handlers::create_project,
        ui_handlers::delete_project,
        // VMs (UI)
        ui_handlers::list_vms,
        ui_handlers::get_vm,
        ui_handlers::create_vm,
        ui_handlers::delete_vm,
        ui_handlers::start_vm,
        ui_handlers::stop_vm,
        ui_handlers::kill_vm,
        // Networks (UI)
        ui_handlers::list_networks,
        ui_handlers::get_network,
        ui_handlers::create_network,
        ui_handlers::delete_network,
        // NICs (UI)
        ui_handlers::list_nics,
        ui_handlers::get_nic,
        ui_handlers::create_nic,
        ui_handlers::delete_nic,
        ui_handlers::attach_nic,
        ui_handlers::detach_nic,
        // Storage
        ui_handlers::list_volumes,
        ui_handlers::get_volume,
        ui_handlers::create_volume,
        ui_handlers::delete_volume,
        ui_handlers::resize_volume,
        ui_handlers::create_snapshot,
        ui_handlers::list_templates,
        ui_handlers::import_template,
        ui_handlers::get_import_job,
        ui_handlers::get_pool_stats,
        // Security Groups
        ui_handlers::list_security_groups,
        ui_handlers::get_security_group,
        ui_handlers::create_security_group,
        ui_handlers::delete_security_group,
        ui_handlers::create_security_group_rule,
        ui_handlers::delete_security_group_rule,
        // Logs
        ui_handlers::query_logs,
    ),
    components(schemas(
        // Internal API schemas
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
        // UI schemas - Projects
        ui_types::UiProject,
        ui_types::UiCreateProjectRequest,
        ui_types::ProjectListResponse,
        // UI schemas - VMs
        ui_types::UiVm,
        ui_types::UiVmState,
        ui_types::UiVmConfig,
        ui_types::UiCreateVmRequest,
        ui_types::UiCreateVmConfig,
        ui_types::VmListResponse,
        // UI schemas - Networks
        ui_types::UiNetwork,
        ui_types::UiCreateNetworkRequest,
        ui_types::NetworkListResponse,
        // UI schemas - NICs
        ui_types::UiNic,
        ui_types::UiCreateNicRequest,
        ui_types::UiAttachNicRequest,
        ui_types::NicListResponse,
        // UI schemas - Storage
        ui_types::UiVolume,
        ui_types::UiSnapshot,
        ui_types::UiCreateVolumeRequest,
        ui_types::UiResizeVolumeRequest,
        ui_types::UiCreateSnapshotRequest,
        ui_types::VolumeListResponse,
        ui_types::UiTemplate,
        ui_types::UiImportTemplateRequest,
        ui_types::TemplateListResponse,
        ui_types::UiImportJob,
        ui_types::UiImportJobState,
        ui_types::UiPoolStats,
        // UI schemas - Security Groups
        ui_types::UiSecurityGroup,
        ui_types::UiSecurityGroupRule,
        ui_types::UiRuleDirection,
        ui_types::UiRuleProtocol,
        ui_types::UiCreateSecurityGroupRequest,
        ui_types::UiCreateSecurityGroupRuleRequest,
        ui_types::SecurityGroupListResponse,
        // UI schemas - Logs
        ui_handlers::UiLogEntry,
        ui_handlers::LogsResponse,
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

    // Global UI routes (not project-scoped)
    let global_routes = Router::new()
        // Projects
        .route("/projects", get(ui_handlers::list_projects))
        .route("/projects", post(ui_handlers::create_project))
        .route("/projects/{id}", get(ui_handlers::get_project))
        .route("/projects/{id}", delete(ui_handlers::delete_project))
        // Global storage
        .route("/import-jobs/{id}", get(ui_handlers::get_import_job))
        .route("/pool", get(ui_handlers::get_pool_stats))
        // Logs
        .route("/logs", get(ui_handlers::query_logs))
        .route("/logs/stream", get(ui_handlers::log_events))
        // Notifications (stub)
        .route("/notifications", get(ui_handlers::list_notifications))
        .route(
            "/notifications/{id}/read",
            post(ui_handlers::mark_notification_read),
        )
        .route(
            "/notifications/read-all",
            post(ui_handlers::mark_all_notifications_read),
        )
        // Resource by ID (globally unique IDs)
        // VMs
        .route("/vms/{id}", get(ui_handlers::get_vm))
        .route("/vms/{id}", delete(ui_handlers::delete_vm))
        .route("/vms/{id}/start", post(ui_handlers::start_vm))
        .route("/vms/{id}/stop", post(ui_handlers::stop_vm))
        .route("/vms/{id}/kill", post(ui_handlers::kill_vm))
        .route("/vms/{id}/console", get(ui_handlers::console_ws))
        // Networks
        .route("/networks/{id}", get(ui_handlers::get_network))
        .route("/networks/{id}", delete(ui_handlers::delete_network))
        // NICs
        .route("/nics/{id}", get(ui_handlers::get_nic))
        .route("/nics/{id}", delete(ui_handlers::delete_nic))
        .route("/nics/{id}/attach", post(ui_handlers::attach_nic))
        .route("/nics/{id}/detach", post(ui_handlers::detach_nic))
        // Volumes
        .route("/volumes/{id}", get(ui_handlers::get_volume))
        .route("/volumes/{id}", delete(ui_handlers::delete_volume))
        .route("/volumes/{id}/resize", post(ui_handlers::resize_volume))
        .route(
            "/volumes/{id}/snapshots",
            post(ui_handlers::create_snapshot),
        )
        // Security Groups
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
        );

    // Project-scoped routes: /v1/projects/{project_id}/...
    // Only LIST and CREATE operations (require project context)
    let project_routes = Router::new()
        // VMs
        .route("/vms", get(ui_handlers::list_vms))
        .route("/vms", post(ui_handlers::create_vm))
        .route("/events/vms", get(ui_handlers::vm_events))
        // Networks
        .route("/networks", get(ui_handlers::list_networks))
        .route("/networks", post(ui_handlers::create_network))
        // NICs
        .route("/nics", get(ui_handlers::list_nics))
        .route("/nics", post(ui_handlers::create_nic))
        // Volumes & Templates
        .route("/volumes", get(ui_handlers::list_volumes))
        .route("/volumes", post(ui_handlers::create_volume))
        .route("/templates", get(ui_handlers::list_templates))
        .route("/templates/import", post(ui_handlers::import_template))
        // Security Groups
        .route("/security-groups", get(ui_handlers::list_security_groups))
        .route(
            "/security-groups",
            post(ui_handlers::create_security_group),
        );

    Router::new()
        .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi()))
        .nest("/v1", internal_routes)
        .nest("/v1", global_routes)
        .nest("/v1/projects/{project_id}", project_routes)
        .with_state(state)
}
