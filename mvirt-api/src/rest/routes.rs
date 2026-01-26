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
    let api_routes = Router::new()
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
        .route("/nodes/{id}", delete(handlers::deregister_hypervisor_node))
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
        .route("/nics/{id}", delete(handlers::delete_nic))
        // VMs
        .route("/vms", get(handlers::list_vms))
        .route("/vms", post(handlers::create_vm))
        .route("/vms/{id}", get(handlers::get_vm))
        .route("/vms/{id}/spec", patch(handlers::update_vm_spec))
        .route("/vms/{id}/status", patch(handlers::update_vm_status))
        .route("/vms/{id}", delete(handlers::delete_vm));

    Router::new()
        .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi()))
        .nest("/api/v1", api_routes)
        .with_state(state)
}
