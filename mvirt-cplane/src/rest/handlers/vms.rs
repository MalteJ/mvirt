use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use crate::command::{VmData, VmDesiredState, VmPhase, VmSpec, VmStatus};
use crate::store::{
    CreateVmRequest as StoreCreateVmRequest, UpdateVmSpecRequest as StoreUpdateVmSpecRequest,
    UpdateVmStatusRequest as StoreUpdateVmStatusRequest,
};

use super::{ApiError, AppState};

/// Request to create a VM
#[derive(Deserialize, ToSchema)]
pub struct CreateVmRequest {
    /// VM name
    pub name: String,
    /// Project ID
    pub project_id: String,
    /// Optional: require specific node (by ID or name)
    pub node_selector: Option<String>,
    /// CPU cores
    pub cpu_cores: u32,
    /// Memory in MB
    pub memory_mb: u64,
    /// Volume ID (boot volume)
    pub volume_id: String,
    /// NIC ID
    pub nic_id: String,
    /// Boot image reference
    pub image: String,
    /// Initial desired state (default: Running)
    pub desired_state: Option<String>,
}

/// Request to update a VM spec (desired state)
#[derive(Deserialize, ToSchema)]
pub struct UpdateVmSpecRequest {
    /// Desired power state: running or stopped
    pub desired_state: String,
}

/// Request to update a VM status (from node)
#[derive(Deserialize, ToSchema)]
pub struct UpdateVmStatusRequestBody {
    /// VM phase: pending, scheduled, creating, running, stopping, stopped, failed
    pub phase: String,
    /// Assigned node ID
    pub node_id: Option<String>,
    /// Assigned IP address
    pub ip_address: Option<String>,
    /// Error or status message
    pub message: Option<String>,
}

/// VM spec (desired state)
#[derive(Serialize, ToSchema)]
pub struct VmSpecResponse {
    pub name: String,
    pub project_id: String,
    pub node_selector: Option<String>,
    pub cpu_cores: u32,
    pub memory_mb: u64,
    pub volume_id: String,
    pub nic_id: String,
    pub image: String,
    pub desired_state: String,
}

impl From<VmSpec> for VmSpecResponse {
    fn from(spec: VmSpec) -> Self {
        Self {
            name: spec.name,
            project_id: spec.project_id,
            node_selector: spec.node_selector,
            cpu_cores: spec.cpu_cores,
            memory_mb: spec.memory_mb,
            volume_id: spec.volume_id,
            nic_id: spec.nic_id,
            image: spec.image,
            desired_state: format!("{:?}", spec.desired_state),
        }
    }
}

/// VM status (actual state)
#[derive(Serialize, ToSchema)]
pub struct VmStatusResponse {
    pub phase: String,
    pub node_id: Option<String>,
    pub ip_address: Option<String>,
    pub message: Option<String>,
}

impl From<VmStatus> for VmStatusResponse {
    fn from(status: VmStatus) -> Self {
        Self {
            phase: format!("{:?}", status.phase),
            node_id: status.node_id,
            ip_address: status.ip_address,
            message: status.message,
        }
    }
}

/// VM resource
#[derive(Serialize, ToSchema)]
pub struct Vm {
    pub id: String,
    pub spec: VmSpecResponse,
    pub status: VmStatusResponse,
    pub created_at: String,
    pub updated_at: String,
}

impl From<VmData> for Vm {
    fn from(data: VmData) -> Self {
        Self {
            id: data.id,
            spec: data.spec.into(),
            status: data.status.into(),
            created_at: data.created_at,
            updated_at: data.updated_at,
        }
    }
}

/// Create a new VM
#[utoipa::path(
    post,
    path = "/v1/vms",
    request_body = CreateVmRequest,
    responses(
        (status = 200, description = "VM created", body = Vm),
        (status = 404, description = "Network not found", body = ApiError),
        (status = 409, description = "VM name already exists", body = ApiError),
        (status = 503, description = "Not the leader", body = ApiError)
    ),
    tag = "vms"
)]
pub async fn create_vm(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateVmRequest>,
) -> Result<Json<Vm>, ApiError> {
    let desired_state = match req.desired_state.as_deref() {
        Some("stopped") | Some("Stopped") => VmDesiredState::Stopped,
        _ => VmDesiredState::Running,
    };

    let spec = VmSpec {
        name: req.name.clone(),
        project_id: req.project_id,
        node_selector: req.node_selector,
        cpu_cores: req.cpu_cores,
        memory_mb: req.memory_mb,
        volume_id: req.volume_id,
        nic_id: req.nic_id,
        image: req.image,
        desired_state,
    };

    let store_req = StoreCreateVmRequest { spec };

    // Create and schedule the VM to a node
    let data = state.store.create_and_schedule_vm(store_req).await?;
    state.audit.vm_created(&data.id, &data.spec.name);
    Ok(Json(data.into()))
}

/// Get a VM by ID or name
#[utoipa::path(
    get,
    path = "/v1/vms/{id}",
    params(
        ("id" = String, Path, description = "VM ID or name")
    ),
    responses(
        (status = 200, description = "VM found", body = Vm),
        (status = 404, description = "VM not found", body = ApiError)
    ),
    tag = "vms"
)]
pub async fn get_vm(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Vm>, ApiError> {
    // Try by ID first, then by name
    let vm = state
        .store
        .get_vm(&id)
        .await?
        .or(state.store.get_vm_by_name(&id).await?);

    match vm {
        Some(data) => Ok(Json(data.into())),
        None => Err(ApiError {
            error: "VM not found".to_string(),
            code: 404,
        }),
    }
}

/// Query parameters for list VMs
#[derive(Deserialize, ToSchema)]
pub struct ListVmsQuery {
    /// Filter by node ID
    pub node_id: Option<String>,
    /// Filter by phase
    pub phase: Option<String>,
}

/// List all VMs
#[utoipa::path(
    get,
    path = "/v1/vms",
    params(
        ("node_id" = Option<String>, Query, description = "Filter by node ID"),
        ("phase" = Option<String>, Query, description = "Filter by phase")
    ),
    responses(
        (status = 200, description = "List of VMs", body = Vec<Vm>)
    ),
    tag = "vms"
)]
pub async fn list_vms(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListVmsQuery>,
) -> Result<Json<Vec<Vm>>, ApiError> {
    let vms = match &query.node_id {
        Some(node_id) => state.store.list_vms_by_node(node_id).await?,
        None => state.store.list_vms().await?,
    };

    // Optional: filter by phase
    let vms: Vec<Vm> = vms
        .into_iter()
        .filter(|vm| {
            if let Some(ref phase) = query.phase {
                format!("{:?}", vm.status.phase).to_lowercase() == phase.to_lowercase()
            } else {
                true
            }
        })
        .map(|v| v.into())
        .collect();

    Ok(Json(vms))
}

/// Update a VM's spec (desired state)
#[utoipa::path(
    patch,
    path = "/v1/vms/{id}/spec",
    params(
        ("id" = String, Path, description = "VM ID")
    ),
    request_body = UpdateVmSpecRequest,
    responses(
        (status = 200, description = "VM spec updated", body = Vm),
        (status = 404, description = "VM not found", body = ApiError),
        (status = 503, description = "Not the leader", body = ApiError)
    ),
    tag = "vms"
)]
pub async fn update_vm_spec(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<UpdateVmSpecRequest>,
) -> Result<Json<Vm>, ApiError> {
    let desired_state = match req.desired_state.to_lowercase().as_str() {
        "stopped" => VmDesiredState::Stopped,
        _ => VmDesiredState::Running,
    };

    let store_req = StoreUpdateVmSpecRequest { desired_state };

    let data = state.store.update_vm_spec(&id, store_req).await?;
    state.audit.vm_spec_updated(&data.id);
    Ok(Json(data.into()))
}

/// Update a VM's status (from node)
#[utoipa::path(
    patch,
    path = "/v1/vms/{id}/status",
    params(
        ("id" = String, Path, description = "VM ID")
    ),
    request_body = UpdateVmStatusRequestBody,
    responses(
        (status = 200, description = "VM status updated", body = Vm),
        (status = 404, description = "VM not found", body = ApiError),
        (status = 503, description = "Not the leader", body = ApiError)
    ),
    tag = "vms"
)]
pub async fn update_vm_status(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<UpdateVmStatusRequestBody>,
) -> Result<Json<Vm>, ApiError> {
    let phase = match req.phase.to_lowercase().as_str() {
        "pending" => VmPhase::Pending,
        "scheduled" => VmPhase::Scheduled,
        "creating" => VmPhase::Creating,
        "running" => VmPhase::Running,
        "stopping" => VmPhase::Stopping,
        "stopped" => VmPhase::Stopped,
        "failed" => VmPhase::Failed,
        _ => VmPhase::Pending,
    };

    let status = VmStatus {
        phase,
        node_id: req.node_id,
        ip_address: req.ip_address,
        message: req.message,
    };

    let store_req = StoreUpdateVmStatusRequest { status };

    let data = state.store.update_vm_status(&id, store_req).await?;
    state.audit.vm_status_updated(&data.id);
    Ok(Json(data.into()))
}

/// Response for delete VM
#[derive(Serialize, ToSchema)]
pub struct DeleteVmResponse {
    pub deleted: bool,
}

/// Delete a VM
#[utoipa::path(
    delete,
    path = "/v1/vms/{id}",
    params(
        ("id" = String, Path, description = "VM ID")
    ),
    responses(
        (status = 200, description = "VM deleted", body = DeleteVmResponse),
        (status = 404, description = "VM not found", body = ApiError),
        (status = 503, description = "Not the leader", body = ApiError)
    ),
    tag = "vms"
)]
pub async fn delete_vm(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<DeleteVmResponse>, ApiError> {
    state.store.delete_vm(&id).await?;
    state.audit.vm_deleted(&id);
    Ok(Json(DeleteVmResponse { deleted: true }))
}
