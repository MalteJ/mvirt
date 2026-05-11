use axum::{
    Router, middleware,
    routing::{delete, get, patch, post},
};
use std::sync::Arc;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use super::handlers::{self, AppState};
use super::ui_handlers;
use super::ui_types;
use crate::auth::require_auth;

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
        (name = "controlplane", description = "Control plane management"),
        (name = "nodes", description = "Hypervisor node registration and status"),
        (name = "orgs", description = "Organization management (tenancy container above Project)"),
        (name = "projects", description = "Project management"),
        (name = "clusters", description = "Cluster management (named groups of Nodes within an Org)"),
        (name = "bootstrap", description = "Node onboarding (token-authed; no Account session)"),
        (name = "auth", description = "Current user, sessions"),
        (name = "networks", description = "Network CRUD operations"),
        (name = "nics", description = "NIC CRUD operations"),
        (name = "vms", description = "VM CRUD and lifecycle operations"),
        (name = "storage", description = "Volumes, templates, and storage pool"),
        (name = "security-groups", description = "Security group and firewall rule management"),
        (name = "pods", description = "Pod and container management (stub)"),
        (name = "logs", description = "Audit log queries")
    ),
    paths(
        // System & Cluster (internal)
        handlers::get_version,
        handlers::get_controlplane_info,
        handlers::get_controlplane_membership,
        handlers::create_controlplane_join_token,
        handlers::remove_peer,
        handlers::register_hypervisor_node,
        handlers::get_hypervisor_node,
        handlers::list_hypervisor_nodes,
        handlers::update_hypervisor_node_status,
        handlers::deregister_hypervisor_node,
        // Orgs
        ui_handlers::list_orgs,
        ui_handlers::get_org,
        ui_handlers::create_org,
        ui_handlers::update_org,
        ui_handlers::delete_org,
        ui_handlers::list_projects_in_org,
        // Projects
        ui_handlers::list_projects,
        ui_handlers::get_project,
        ui_handlers::create_project,
        ui_handlers::delete_project,
        // Clusters
        ui_handlers::list_clusters,
        ui_handlers::list_clusters_in_org,
        ui_handlers::get_cluster,
        ui_handlers::create_cluster,
        ui_handlers::update_cluster,
        ui_handlers::delete_cluster,
        ui_handlers::add_node_to_cluster,
        ui_handlers::remove_node_from_cluster,
        // Node onboarding (ADR-0006)
        ui_handlers::list_onboarding_tokens_in_cluster,
        ui_handlers::create_onboarding_token,
        ui_handlers::delete_onboarding_token,
        ui_handlers::bootstrap_onboarding,
        ui_handlers::revoke_node,
        // Auth + Members (ADR-0004)
        ui_handlers::get_me,
        ui_handlers::list_accounts,
        ui_handlers::list_org_members,
        ui_handlers::create_org_member,
        ui_handlers::delete_org_member,
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
        ui_handlers::update_security_group,
        ui_handlers::create_security_group_rule,
        ui_handlers::delete_security_group_rule,
        ui_handlers::update_security_group_rule,
        // Pods (stub)
        ui_handlers::list_pods,
        ui_handlers::get_pod,
        ui_handlers::create_pod,
        ui_handlers::delete_pod,
        ui_handlers::start_pod,
        ui_handlers::stop_pod,
        // Logs
        ui_handlers::query_logs,
    ),
    components(schemas(
        // Internal API schemas
        handlers::VersionInfo,
        handlers::ControlplaneInfo,
        handlers::PeerInfo,
        handlers::ControlplaneMembership,
        handlers::MembershipPeer,
        handlers::CreateJoinTokenRequest,
        handlers::CreateJoinTokenResponse,
        handlers::RemovePeerRequest,
        handlers::RemovePeerResponse,
        handlers::RegisterHypervisorNodeRequest,
        handlers::HypervisorNodeResources,
        handlers::HypervisorNode,
        handlers::ListNodesQuery,
        handlers::UpdateNodeStatusRequest,
        handlers::DeregisterNodeResponse,
        handlers::ApiError,
        // UI schemas - Orgs
        ui_types::UiOrg,
        ui_types::UiOrgContact,
        ui_types::UiCreateOrgRequest,
        ui_types::UiUpdateOrgRequest,
        ui_types::OrgListResponse,
        // UI schemas - Projects
        ui_types::UiProject,
        ui_types::UiCreateProjectRequest,
        ui_types::ProjectListResponse,
        // UI schemas - Clusters
        ui_types::UiCluster,
        ui_types::UiCreateClusterRequest,
        ui_types::UiUpdateClusterRequest,
        ui_types::ClusterListResponse,
        // UI schemas - Onboarding (ADR-0006)
        ui_types::UiOnboardingToken,
        ui_types::UiCreateOnboardingTokenRequest,
        ui_types::UiCreateOnboardingTokenResponse,
        ui_types::OnboardingTokenListResponse,
        ui_types::UiBootstrapRequest,
        ui_types::UiBootstrapResponse,
        ui_types::UiRevokeNodeRequest,
        // UI schemas - Auth + Members
        ui_types::UiAccount,
        ui_types::UiMembership,
        ui_types::UiMe,
        ui_types::MembershipListResponse,
        ui_types::UiCreateOrgMembershipRequest,
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
        ui_types::UiUpdateSecurityGroupRequest,
        ui_types::UiCreateSecurityGroupRuleRequest,
        ui_types::UiUpdateSecurityGroupRuleRequest,
        ui_types::SecurityGroupListResponse,
        // UI schemas - Pods
        ui_types::UiPod,
        ui_types::UiPodState,
        ui_types::UiContainer,
        ui_types::UiContainerState,
        ui_types::UiContainerSpec,
        ui_types::UiCreatePodRequest,
        ui_types::PodListResponse,
        // UI schemas - Logs
        ui_handlers::UiLogEntry,
        ui_handlers::LogsResponse,
    ))
)]
pub struct ApiDoc;

pub fn create_router(state: Arc<AppState>) -> Router {
    // The middleware reaches the JWT validator + DataStore via AppState,
    // so we no longer need a separate validator parameter — auth is on
    // iff `state.jwt_validator.is_some()`.
    let auth_enabled = state.jwt_validator.is_some();
    // Internal API routes (for hypervisor nodes, cluster management)
    let internal_routes = Router::new()
        // System
        .route("/version", get(handlers::get_version))
        // Controlplane
        .route("/controlplane", get(handlers::get_controlplane_info))
        .route(
            "/controlplane/membership",
            get(handlers::get_controlplane_membership),
        )
        .route(
            "/controlplane/join-token",
            post(handlers::create_controlplane_join_token),
        )
        .route("/controlplane/peers/{id}", delete(handlers::remove_peer))
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
        // Orgs (tenancy container)
        .route("/orgs", get(ui_handlers::list_orgs))
        .route("/orgs", post(ui_handlers::create_org))
        .route("/orgs/{slug}", get(ui_handlers::get_org))
        .route("/orgs/{slug}", patch(ui_handlers::update_org))
        .route("/orgs/{slug}", delete(ui_handlers::delete_org))
        // Projects within an Org (Org-scoped list + create)
        .route(
            "/orgs/{org_slug}/projects",
            get(ui_handlers::list_projects_in_org),
        )
        .route(
            "/orgs/{org_slug}/projects",
            post(ui_handlers::create_project),
        )
        // Projects (flat: cross-org list, individual ops by slug-or-id)
        .route("/projects", get(ui_handlers::list_projects))
        .route("/projects/{id}", get(ui_handlers::get_project))
        .route("/projects/{id}", delete(ui_handlers::delete_project))
        // Clusters within an Org (Org-scoped list + create)
        .route(
            "/orgs/{org_slug}/clusters",
            get(ui_handlers::list_clusters_in_org),
        )
        .route(
            "/orgs/{org_slug}/clusters",
            post(ui_handlers::create_cluster),
        )
        // Clusters (flat: cross-org list, individual ops by slug)
        .route("/clusters", get(ui_handlers::list_clusters))
        .route("/clusters/{slug}", get(ui_handlers::get_cluster))
        .route("/clusters/{slug}", patch(ui_handlers::update_cluster))
        .route("/clusters/{slug}", delete(ui_handlers::delete_cluster))
        // Cluster ↔ Node membership
        .route(
            "/clusters/{slug}/nodes/{node_id}",
            post(ui_handlers::add_node_to_cluster),
        )
        .route(
            "/clusters/{slug}/nodes/{node_id}",
            delete(ui_handlers::remove_node_from_cluster),
        )
        // Onboarding token management (ADR-0006)
        .route(
            "/clusters/{slug}/onboarding-tokens",
            get(ui_handlers::list_onboarding_tokens_in_cluster),
        )
        .route(
            "/clusters/{slug}/onboarding-tokens",
            post(ui_handlers::create_onboarding_token),
        )
        .route(
            "/clusters/{slug}/onboarding-tokens/{id}",
            delete(ui_handlers::delete_onboarding_token),
        )
        // Node revoke (cert revocation; Decommission also deletes the row)
        .route("/nodes/{id}/revoke", post(ui_handlers::revoke_node))
        // Auth (ADR-0004): current user + accounts + org members
        .route("/me", get(ui_handlers::get_me))
        .route("/accounts", get(ui_handlers::list_accounts))
        .route(
            "/orgs/{org_slug}/members",
            get(ui_handlers::list_org_members).post(ui_handlers::create_org_member),
        )
        .route(
            "/orgs/{org_slug}/members/{id}",
            delete(ui_handlers::delete_org_member),
        )
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
        // Pods (stub)
        .route("/pods", get(ui_handlers::list_pods))
        .route("/pods", post(ui_handlers::create_pod))
        .route("/pods/{id}", get(ui_handlers::get_pod))
        .route("/pods/{id}", delete(ui_handlers::delete_pod))
        .route("/pods/{id}/start", post(ui_handlers::start_pod))
        .route("/pods/{id}/stop", post(ui_handlers::stop_pod))
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
            delete(ui_handlers::delete_security_group).patch(ui_handlers::update_security_group),
        )
        .route(
            "/security-groups/{id}/rules",
            post(ui_handlers::create_security_group_rule),
        )
        .route(
            "/security-groups/{sg_id}/rules/{rule_id}",
            delete(ui_handlers::delete_security_group_rule)
                .patch(ui_handlers::update_security_group_rule),
        );

    // Project-scoped routes: /v1/projects/{project_slug}/...
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
        .route("/security-groups", post(ui_handlers::create_security_group));

    // Bootstrap endpoint (ADR-0006). Token-authed via the Authorization
    // header inside the handler — *not* through the Account JWT middleware.
    // Hosted under /v1/bootstrap/... so the Account-auth layer above can
    // short-circuit on path prefix.
    let bootstrap_routes = Router::new().route(
        "/bootstrap/onboarding",
        post(ui_handlers::bootstrap_onboarding),
    );

    // User-facing routes get JWT auth applied when a validator is configured.
    // Internal routes are reached via the cplane-to-cplane network, not the
    // public REST endpoint, and stay unauthenticated for now.
    let (global_routes, project_routes) = if auth_enabled {
        (
            global_routes.layer(middleware::from_fn_with_state(state.clone(), require_auth)),
            project_routes.layer(middleware::from_fn_with_state(state.clone(), require_auth)),
        )
    } else {
        (global_routes, project_routes)
    };

    Router::new()
        .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi()))
        .nest("/v1", internal_routes)
        .nest("/v1", global_routes)
        .nest("/v1", bootstrap_routes)
        .nest("/v1/projects/{project_slug}", project_routes)
        .with_state(state)
        .layer(
            // `CorsLayer::permissive()` sets `Access-Control-Allow-Headers: *`,
            // which per CORS spec does **not** cover `Authorization`. Browsers
            // (Firefox first, then Chrome) are dropping that header from
            // requests unless it's listed explicitly. List the headers we
            // actually use so the JWT-auth Authorization header survives.
            tower_http::cors::CorsLayer::new()
                .allow_origin(tower_http::cors::Any)
                .allow_methods(tower_http::cors::Any)
                .allow_headers([
                    axum::http::header::AUTHORIZATION,
                    axum::http::header::CONTENT_TYPE,
                    axum::http::header::ACCEPT,
                ]),
        )
}
