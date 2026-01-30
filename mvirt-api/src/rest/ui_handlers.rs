//! UI-compatible REST handlers.
//!
//! These handlers match the mock-server's JSON structure for compatibility with mvirt-ui.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Sse, sse::Event as SseEvent},
};
use futures::stream::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use std::{convert::Infallible, sync::Arc, time::Duration};
use utoipa::ToSchema;

use super::handlers::{ApiError, AppState};
use super::ui_types::*;
use crate::command::{VmDesiredState, VmPhase, VmSpec, VmStatus};
use crate::store::{
    CreateNetworkRequest as StoreCreateNetworkRequest, CreateNicRequest as StoreCreateNicRequest,
    CreateProjectRequest as StoreCreateProjectRequest,
    CreateSnapshotRequest as StoreCreateSnapshotRequest, CreateVmRequest as StoreCreateVmRequest,
    CreateVolumeRequest as StoreCreateVolumeRequest,
    ImportTemplateRequest as StoreImportTemplateRequest,
    ResizeVolumeRequest as StoreResizeVolumeRequest,
    UpdateVmSpecRequest as StoreUpdateVmSpecRequest,
    UpdateVmStatusRequest as StoreUpdateVmStatusRequest,
};

// =============================================================================
// Project Handlers (global, not project-scoped)
// =============================================================================

/// List all projects
#[utoipa::path(get, path = "/v1/projects", responses((status = 200, body = ProjectListResponse)), tag = "projects")]
pub async fn list_projects(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ProjectListResponse>, ApiError> {
    let projects = state.store.list_projects().await?;
    Ok(Json(ProjectListResponse {
        projects: projects.into_iter().map(UiProject::from).collect(),
    }))
}

/// Get a project by ID
#[utoipa::path(get, path = "/v1/projects/{id}", params(("id" = String, Path)), responses((status = 200, body = UiProject), (status = 404, body = ApiError)), tag = "projects")]
pub async fn get_project(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<UiProject>, ApiError> {
    let project = state
        .store
        .get_project(&id)
        .await?
        .or(state.store.get_project_by_name(&id).await?);

    match project {
        Some(data) => Ok(Json(UiProject::from(data))),
        None => Err(ApiError {
            error: "Project not found".to_string(),
            code: 404,
        }),
    }
}

/// Create a new project
#[utoipa::path(post, path = "/v1/projects", request_body = UiCreateProjectRequest, responses((status = 200, body = UiProject), (status = 400, body = ApiError), (status = 409, body = ApiError)), tag = "projects")]
pub async fn create_project(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UiCreateProjectRequest>,
) -> Result<Json<UiProject>, ApiError> {
    // Validate project ID format: must be lowercase alphanumeric
    if req.id.is_empty() {
        return Err(ApiError {
            error: "Project ID cannot be empty".to_string(),
            code: 400,
        });
    }
    if !req
        .id
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    {
        return Err(ApiError {
            error: "Project ID must contain only lowercase letters and numbers".to_string(),
            code: 400,
        });
    }

    // Check if project ID already exists
    if state.store.get_project(&req.id).await?.is_some() {
        return Err(ApiError {
            error: format!("Project ID '{}' already exists", req.id),
            code: 409,
        });
    }

    let store_req = StoreCreateProjectRequest {
        id: req.id,
        name: req.name,
        description: req.description,
    };

    let data = state.store.create_project(store_req).await?;
    state.audit.project_created(&data.id, &data.name);
    Ok(Json(UiProject::from(data)))
}

/// Delete a project
#[utoipa::path(delete, path = "/v1/projects/{id}", params(("id" = String, Path)), responses((status = 204)), tag = "projects")]
pub async fn delete_project(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.store.delete_project(&id).await?;
    state.audit.project_deleted(&id);
    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// VM Handlers
// =============================================================================

/// List VMs in a project
#[utoipa::path(get, path = "/v1/projects/{project_id}/vms", params(("project_id" = String, Path), ("node_id" = Option<String>, Query)), responses((status = 200, body = VmListResponse)), tag = "vms")]
pub async fn list_vms(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<String>,
    Query(query): Query<ListVmsQuery>,
) -> Result<Json<VmListResponse>, ApiError> {
    let vms = state.store.list_vms_by_project(&project_id).await?;

    // Further filter by node if specified
    let vms: Vec<UiVm> = vms
        .into_iter()
        .filter(|vm| {
            query
                .node_id
                .as_ref()
                .is_none_or(|nid| vm.status.node_id.as_deref() == Some(nid.as_str()))
        })
        .map(UiVm::from)
        .collect();

    Ok(Json(VmListResponse { vms }))
}

/// Get a VM by ID
#[utoipa::path(get, path = "/v1/vms/{id}", params(("id" = String, Path)), responses((status = 200, body = UiVm), (status = 404, body = ApiError)), tag = "vms")]
pub async fn get_vm(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<UiVm>, ApiError> {
    let vm = state
        .store
        .get_vm(&id)
        .await?
        .or(state.store.get_vm_by_name(&id).await?);

    match vm {
        Some(data) => Ok(Json(UiVm::from(data))),
        None => Err(ApiError {
            error: "VM not found".to_string(),
            code: 404,
        }),
    }
}

/// Create a new VM
#[utoipa::path(post, path = "/v1/projects/{project_id}/vms", params(("project_id" = String, Path)), request_body = UiCreateVmRequest, responses((status = 200, body = UiVm), (status = 503, body = ApiError)), tag = "vms")]
pub async fn create_vm(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<String>,
    Json(req): Json<UiCreateVmRequest>,
) -> Result<Json<UiVm>, ApiError> {
    let spec = VmSpec {
        name: req.name.clone(),
        project_id,
        node_selector: req.node_selector,
        cpu_cores: req.config.vcpus,
        memory_mb: req.config.memory_mb,
        volume_id: req.config.volume_id,
        nic_id: req.config.nic_id,
        image: req.config.image,
        desired_state: VmDesiredState::Running,
    };

    let store_req = StoreCreateVmRequest { spec };

    let data = state.store.create_and_schedule_vm(store_req).await?;
    state.audit.vm_created(&data.id, &data.spec.name);
    Ok(Json(UiVm::from(data)))
}

/// Delete a VM
#[utoipa::path(delete, path = "/v1/vms/{id}", params(("id" = String, Path)), responses((status = 204), (status = 404, body = ApiError)), tag = "vms")]
pub async fn delete_vm(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.store.delete_vm(&id).await?;
    state.audit.vm_deleted(&id);
    Ok(StatusCode::NO_CONTENT)
}

/// Start a VM
#[utoipa::path(post, path = "/v1/vms/{id}/start", params(("id" = String, Path)), responses((status = 200, body = UiVm), (status = 404, body = ApiError)), tag = "vms")]
pub async fn start_vm(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<UiVm>, ApiError> {
    let store_req = StoreUpdateVmSpecRequest {
        desired_state: VmDesiredState::Running,
    };
    let vm = state.store.update_vm_spec(&id, store_req).await?;
    state.audit.vm_started(&vm.id);

    // Simulate transition (2s delay like mock-server)
    let store = state.store.clone();
    let id_clone = id.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let _ = store
            .update_vm_status(
                &id_clone,
                StoreUpdateVmStatusRequest {
                    status: VmStatus {
                        phase: VmPhase::Running,
                        node_id: None,
                        ip_address: None,
                        message: None,
                    },
                },
            )
            .await;
    });

    Ok(Json(UiVm::from(vm)))
}

/// Stop a VM
#[utoipa::path(post, path = "/v1/vms/{id}/stop", params(("id" = String, Path)), responses((status = 200, body = UiVm), (status = 404, body = ApiError)), tag = "vms")]
pub async fn stop_vm(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<UiVm>, ApiError> {
    let store_req = StoreUpdateVmSpecRequest {
        desired_state: VmDesiredState::Stopped,
    };
    let vm = state.store.update_vm_spec(&id, store_req).await?;
    state.audit.vm_stopped(&vm.id);

    let store = state.store.clone();
    let id_clone = id.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let _ = store
            .update_vm_status(
                &id_clone,
                StoreUpdateVmStatusRequest {
                    status: VmStatus {
                        phase: VmPhase::Stopped,
                        node_id: None,
                        ip_address: None,
                        message: None,
                    },
                },
            )
            .await;
    });

    Ok(Json(UiVm::from(vm)))
}

/// Kill a VM (immediate stop)
#[utoipa::path(post, path = "/v1/vms/{id}/kill", params(("id" = String, Path)), responses((status = 200, body = UiVm), (status = 404, body = ApiError)), tag = "vms")]
pub async fn kill_vm(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<UiVm>, ApiError> {
    let store_req = StoreUpdateVmSpecRequest {
        desired_state: VmDesiredState::Stopped,
    };
    state.store.update_vm_spec(&id, store_req).await?;

    let status_req = StoreUpdateVmStatusRequest {
        status: VmStatus {
            phase: VmPhase::Stopped,
            node_id: None,
            ip_address: None,
            message: Some("Killed".to_string()),
        },
    };
    let vm = state.store.update_vm_status(&id, status_req).await?;
    state.audit.vm_killed(&vm.id);

    Ok(Json(UiVm::from(vm)))
}

/// SSE stream for VM events
pub async fn vm_events(
    State(state): State<Arc<AppState>>,
    Path(_project_id): Path<String>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let mut rx = state.store.subscribe();

    let stream = async_stream::stream! {
        while let Ok(event) = rx.recv().await {
            if event.resource_type() == "vm" {
                let data = serde_json::json!({
                    "type": match &event {
                        crate::store::Event::VmCreated(_) => "created",
                        crate::store::Event::VmUpdated { .. } => "updated",
                        crate::store::Event::VmStatusUpdated { .. } => "status_updated",
                        crate::store::Event::VmDeleted { .. } => "deleted",
                        _ => "unknown",
                    },
                    "id": event.resource_id(),
                });
                yield Ok(SseEvent::default().data(data.to_string()));
            }
        }
    };

    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_secs(30))
            .text("ping"),
    )
}

// =============================================================================
// Network Handlers
// =============================================================================

/// List networks in a project
#[utoipa::path(get, path = "/v1/projects/{project_id}/networks", params(("project_id" = String, Path)), responses((status = 200, body = NetworkListResponse)), tag = "networks")]
pub async fn list_networks(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<String>,
) -> Result<Json<NetworkListResponse>, ApiError> {
    let networks = state.store.list_networks_by_project(&project_id).await?;
    Ok(Json(NetworkListResponse {
        networks: networks.into_iter().map(UiNetwork::from).collect(),
    }))
}

/// Get a network by ID
#[utoipa::path(get, path = "/v1/networks/{id}", params(("id" = String, Path)), responses((status = 200, body = UiNetwork), (status = 404, body = ApiError)), tag = "networks")]
pub async fn get_network(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<UiNetwork>, ApiError> {
    let network = state
        .store
        .get_network(&id)
        .await?
        .or(state.store.get_network_by_name(&id).await?);

    match network {
        Some(data) => Ok(Json(UiNetwork::from(data))),
        None => Err(ApiError {
            error: "Network not found".to_string(),
            code: 404,
        }),
    }
}

/// Create a new network
#[utoipa::path(post, path = "/v1/projects/{project_id}/networks", params(("project_id" = String, Path)), request_body = UiCreateNetworkRequest, responses((status = 200, body = UiNetwork), (status = 409, body = ApiError)), tag = "networks")]
pub async fn create_network(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<String>,
    Json(req): Json<UiCreateNetworkRequest>,
) -> Result<Json<UiNetwork>, ApiError> {
    let store_req = StoreCreateNetworkRequest {
        project_id,
        name: req.name.clone(),
        ipv4_enabled: req.ipv4_enabled,
        ipv4_prefix: req.ipv4_prefix,
        ipv6_enabled: req.ipv6_enabled,
        ipv6_prefix: req.ipv6_prefix,
        dns_servers: req.dns_servers,
        ntp_servers: req.ntp_servers,
        is_public: req.is_public,
    };

    let data = state.store.create_network(store_req).await?;
    state.audit.network_created(&data.id, &data.name);
    Ok(Json(UiNetwork::from(data)))
}

/// Delete a network
#[utoipa::path(delete, path = "/v1/networks/{id}", params(("id" = String, Path)), responses((status = 204), (status = 404, body = ApiError)), tag = "networks")]
pub async fn delete_network(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.store.delete_network(&id, false).await?;
    state.audit.network_deleted(&id);
    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// NIC Handlers
// =============================================================================

/// List NICs in a project
#[utoipa::path(get, path = "/v1/projects/{project_id}/nics", params(("project_id" = String, Path), ("network_id" = Option<String>, Query)), responses((status = 200, body = NicListResponse)), tag = "nics")]
pub async fn list_nics(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<String>,
    Query(query): Query<ListNicsQuery>,
) -> Result<Json<NicListResponse>, ApiError> {
    let nics = state.store.list_nics_by_project(&project_id).await?;
    // Further filter by network if specified
    let nics: Vec<UiNic> = nics
        .into_iter()
        .filter(|nic| {
            query
                .network_id
                .as_ref()
                .is_none_or(|nid| &nic.network_id == nid)
        })
        .map(UiNic::from)
        .collect();
    Ok(Json(NicListResponse { nics }))
}

/// Get a NIC by ID
#[utoipa::path(get, path = "/v1/nics/{id}", params(("id" = String, Path)), responses((status = 200, body = UiNic), (status = 404, body = ApiError)), tag = "nics")]
pub async fn get_nic(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<UiNic>, ApiError> {
    let nic = state
        .store
        .get_nic(&id)
        .await?
        .or(state.store.get_nic_by_name(&id).await?);

    match nic {
        Some(data) => Ok(Json(UiNic::from(data))),
        None => Err(ApiError {
            error: "NIC not found".to_string(),
            code: 404,
        }),
    }
}

/// Create a new NIC
#[utoipa::path(post, path = "/v1/projects/{project_id}/nics", params(("project_id" = String, Path)), request_body = UiCreateNicRequest, responses((status = 200, body = UiNic), (status = 404, body = ApiError)), tag = "nics")]
pub async fn create_nic(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<String>,
    Json(req): Json<UiCreateNicRequest>,
) -> Result<Json<UiNic>, ApiError> {
    let store_req = StoreCreateNicRequest {
        project_id,
        network_id: req.network_id,
        name: req.name,
        mac_address: req.mac_address,
        ipv4_address: req.ipv4_address,
        ipv6_address: req.ipv6_address,
        routed_ipv4_prefixes: vec![],
        routed_ipv6_prefixes: vec![],
        security_group_id: req.security_group_id,
    };

    let data = state.store.create_nic(store_req).await?;
    state
        .audit
        .nic_created(&data.id, &data.network_id, &data.mac_address);
    Ok(Json(UiNic::from(data)))
}

/// Delete a NIC
#[utoipa::path(delete, path = "/v1/nics/{id}", params(("id" = String, Path)), responses((status = 204), (status = 404, body = ApiError)), tag = "nics")]
pub async fn delete_nic(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.store.delete_nic(&id).await?;
    state.audit.nic_deleted(&id);
    Ok(StatusCode::NO_CONTENT)
}

/// Attach a NIC to a VM
#[utoipa::path(post, path = "/v1/nics/{id}/attach", params(("id" = String, Path)), request_body = UiAttachNicRequest, responses((status = 200, body = UiNic), (status = 404, body = ApiError), (status = 409, body = ApiError)), tag = "nics")]
pub async fn attach_nic(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<UiAttachNicRequest>,
) -> Result<Json<UiNic>, ApiError> {
    let nic = state.store.attach_nic(&id, &req.vm_id).await?;
    Ok(Json(UiNic::from(nic)))
}

/// Detach a NIC from a VM
#[utoipa::path(post, path = "/v1/nics/{id}/detach", params(("id" = String, Path)), responses((status = 200, body = UiNic), (status = 404, body = ApiError)), tag = "nics")]
pub async fn detach_nic(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<UiNic>, ApiError> {
    let nic = state.store.detach_nic(&id).await?;
    Ok(Json(UiNic::from(nic)))
}

// =============================================================================
// Storage Handlers
// =============================================================================

/// List volumes in a project
#[utoipa::path(get, path = "/v1/projects/{project_id}/volumes", params(("project_id" = String, Path), ("node_id" = Option<String>, Query)), responses((status = 200, body = VolumeListResponse)), tag = "storage")]
pub async fn list_volumes(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<String>,
    Query(query): Query<ListVolumesQuery>,
) -> Result<Json<VolumeListResponse>, ApiError> {
    let volumes = state
        .store
        .list_volumes(Some(&project_id), query.node_id.as_deref())
        .await?;
    Ok(Json(VolumeListResponse {
        volumes: volumes.into_iter().map(UiVolume::from).collect(),
    }))
}

/// Get a volume by ID
#[utoipa::path(get, path = "/v1/volumes/{id}", params(("id" = String, Path)), responses((status = 200, body = UiVolume), (status = 404, body = ApiError)), tag = "storage")]
pub async fn get_volume(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<UiVolume>, ApiError> {
    match state.store.get_volume(&id).await? {
        Some(data) => Ok(Json(UiVolume::from(data))),
        None => Err(ApiError {
            error: "Volume not found".to_string(),
            code: 404,
        }),
    }
}

/// Create a new volume
#[utoipa::path(post, path = "/v1/projects/{project_id}/volumes", params(("project_id" = String, Path)), request_body = UiCreateVolumeRequest, responses((status = 200, body = UiVolume), (status = 503, body = ApiError)), tag = "storage")]
pub async fn create_volume(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<String>,
    Json(req): Json<UiCreateVolumeRequest>,
) -> Result<Json<UiVolume>, ApiError> {
    let store_req = StoreCreateVolumeRequest {
        project_id,
        node_id: req.node_id,
        name: req.name,
        size_bytes: req.size_bytes,
        template_id: req.template_id,
    };

    let data = state.store.create_volume(store_req).await?;
    state.audit.volume_created(&data.id, &data.name);
    Ok(Json(UiVolume::from(data)))
}

/// Delete a volume
#[utoipa::path(delete, path = "/v1/volumes/{id}", params(("id" = String, Path)), responses((status = 204), (status = 404, body = ApiError)), tag = "storage")]
pub async fn delete_volume(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.store.delete_volume(&id).await?;
    state.audit.volume_deleted(&id);
    Ok(StatusCode::NO_CONTENT)
}

/// Resize a volume
#[utoipa::path(post, path = "/v1/volumes/{id}/resize", params(("id" = String, Path)), request_body = UiResizeVolumeRequest, responses((status = 200, body = UiVolume), (status = 404, body = ApiError)), tag = "storage")]
pub async fn resize_volume(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<UiResizeVolumeRequest>,
) -> Result<Json<UiVolume>, ApiError> {
    let store_req = StoreResizeVolumeRequest {
        size_bytes: req.size_bytes,
    };

    let data = state.store.resize_volume(&id, store_req).await?;
    state.audit.volume_resized(&data.id, data.size_bytes);
    Ok(Json(UiVolume::from(data)))
}

/// Create a snapshot on a volume
#[utoipa::path(post, path = "/v1/volumes/{id}/snapshots", params(("id" = String, Path)), request_body = UiCreateSnapshotRequest, responses((status = 200, body = UiVolume), (status = 404, body = ApiError)), tag = "storage")]
pub async fn create_snapshot(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<UiCreateSnapshotRequest>,
) -> Result<Json<UiVolume>, ApiError> {
    let store_req = StoreCreateSnapshotRequest { name: req.name };

    let data = state.store.create_snapshot(&id, store_req).await?;
    state.audit.snapshot_created(&id);
    Ok(Json(UiVolume::from(data)))
}

/// List templates in a project
#[utoipa::path(get, path = "/v1/projects/{project_id}/templates", params(("project_id" = String, Path)), responses((status = 200, body = TemplateListResponse)), tag = "storage")]
pub async fn list_templates(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<String>,
) -> Result<Json<TemplateListResponse>, ApiError> {
    let templates = state.store.list_templates_by_project(&project_id).await?;
    Ok(Json(TemplateListResponse {
        templates: templates.into_iter().map(UiTemplate::from).collect(),
    }))
}

/// Import a template
#[utoipa::path(post, path = "/v1/projects/{project_id}/templates/import", params(("project_id" = String, Path)), request_body = UiImportTemplateRequest, responses((status = 200, body = UiImportJob), (status = 503, body = ApiError)), tag = "storage")]
pub async fn import_template(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<String>,
    Json(req): Json<UiImportTemplateRequest>,
) -> Result<Json<UiImportJob>, ApiError> {
    let store_req = StoreImportTemplateRequest {
        project_id,
        node_id: req.node_id,
        name: req.name,
        url: req.url,
        total_bytes: req.total_bytes,
    };

    let data = state.store.import_template(store_req).await?;
    state.audit.template_import_started(&data.id);
    Ok(Json(UiImportJob::from(data)))
}

/// Get an import job by ID (global)
#[utoipa::path(get, path = "/v1/import-jobs/{id}", params(("id" = String, Path)), responses((status = 200, body = UiImportJob), (status = 404, body = ApiError)), tag = "storage")]
pub async fn get_import_job(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<UiImportJob>, ApiError> {
    match state.store.get_import_job(&id).await? {
        Some(data) => Ok(Json(UiImportJob::from(data))),
        None => Err(ApiError {
            error: "Import job not found".to_string(),
            code: 404,
        }),
    }
}

/// Get storage pool statistics (global)
#[utoipa::path(get, path = "/v1/pool", responses((status = 200, body = UiPoolStats)), tag = "storage")]
pub async fn get_pool_stats(
    State(state): State<Arc<AppState>>,
) -> Result<Json<UiPoolStats>, ApiError> {
    let nodes = state.store.list_nodes().await?;
    let mut total_bytes: u64 = 0;
    let mut available_bytes: u64 = 0;
    for node in &nodes {
        total_bytes += node.resources.storage_gb * 1_073_741_824;
        available_bytes += node.resources.available_storage_gb * 1_073_741_824;
    }
    let used_bytes = total_bytes.saturating_sub(available_bytes);
    let compression_ratio = if used_bytes > 0 {
        let volumes = state.store.list_volumes(None, None).await?;
        let total_ratio: f64 = volumes.iter().map(|v| v.compression_ratio).sum::<f64>();
        if volumes.is_empty() {
            1.0
        } else {
            total_ratio / volumes.len() as f64
        }
    } else {
        1.0
    };
    Ok(Json(UiPoolStats {
        total_bytes,
        used_bytes,
        available_bytes,
        compression_ratio,
    }))
}

// =============================================================================
// Logs Handlers (global)
// =============================================================================

/// Query parameters for logs
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct QueryLogsParams {
    #[serde(default)]
    pub object_id: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
}

/// Log entry for UI
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiLogEntry {
    pub id: String,
    pub timestamp: String,
    pub message: String,
    pub level: String,
    pub component: String,
}

/// Response wrapper for logs
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LogsResponse {
    pub logs: Vec<UiLogEntry>,
}

/// Query logs via mvirt-log
#[utoipa::path(get, path = "/v1/logs", params(("object_id" = Option<String>, Query), ("limit" = Option<u32>, Query)), responses((status = 200, body = LogsResponse), (status = 503, body = ApiError)), tag = "logs")]
pub async fn query_logs(
    State(state): State<Arc<AppState>>,
    Query(params): Query<QueryLogsParams>,
) -> Result<Json<LogsResponse>, ApiError> {
    use mvirt_log::{LogServiceClient, QueryRequest};

    let mut client = LogServiceClient::connect(state.log_endpoint.clone())
        .await
        .map_err(|e| ApiError {
            error: format!("Failed to connect to log service: {}", e),
            code: 503,
        })?;

    let req = QueryRequest {
        object_id: params.object_id,
        start_time_ns: None,
        end_time_ns: None,
        limit: params.limit.unwrap_or(100),
        follow: false,
    };

    let mut stream = client
        .query(req)
        .await
        .map_err(|e| ApiError {
            error: format!("Log query failed: {}", e),
            code: 500,
        })?
        .into_inner();

    let mut logs = Vec::new();
    while let Some(entry) = stream.message().await.map_err(|e| ApiError {
        error: format!("Log stream error: {}", e),
        code: 500,
    })? {
        logs.push(UiLogEntry {
            id: entry.id,
            timestamp: chrono::DateTime::from_timestamp_nanos(entry.timestamp_ns).to_rfc3339(),
            message: entry.message,
            level: format!(
                "{:?}",
                mvirt_log::LogLevel::try_from(entry.level).unwrap_or(mvirt_log::LogLevel::Info)
            ),
            component: entry.component,
        });
    }

    Ok(Json(LogsResponse { logs }))
}

/// SSE stream for log events via mvirt-log
pub async fn log_events(
    State(state): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    use mvirt_log::{LogServiceClient, QueryRequest};

    let endpoint = state.log_endpoint.clone();
    let stream = async_stream::try_stream! {
        let mut client = LogServiceClient::connect(endpoint).await
            .map_err(|_| std::io::Error::other("connection failed"))?;

        let req = QueryRequest {
            object_id: None,
            start_time_ns: None,
            end_time_ns: None,
            limit: 0,
            follow: true,
        };

        let mut log_stream = client.query(req).await
            .map_err(|_| std::io::Error::other("query failed"))?
            .into_inner();

        while let Some(entry) = log_stream.message().await
            .map_err(|_| std::io::Error::other("stream error"))? {
            let log_entry = UiLogEntry {
                id: entry.id,
                timestamp: chrono::DateTime::from_timestamp_nanos(entry.timestamp_ns).to_rfc3339(),
                message: entry.message,
                level: format!("{:?}", mvirt_log::LogLevel::try_from(entry.level).unwrap_or(mvirt_log::LogLevel::Info)),
                component: entry.component,
            };
            yield SseEvent::default()
                .json_data(&log_entry)
                .unwrap_or_else(|_| SseEvent::default().data("error"));
        }
    };

    let stream = stream
        .filter_map(|result: Result<SseEvent, std::io::Error>| std::future::ready(result.ok()))
        .map(Ok);

    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_secs(30))
            .text("ping"),
    )
}

// =============================================================================
// Console Handler (stub)
// =============================================================================

/// WebSocket handler for VM console (stub)
pub async fn console_ws(
    State(_state): State<Arc<AppState>>,
    Path(_id): Path<String>,
) -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        "Console WebSocket not implemented",
    )
}

// =============================================================================
// Security Group Handlers
// =============================================================================

/// List security groups in a project
#[utoipa::path(get, path = "/v1/projects/{project_id}/security-groups", params(("project_id" = String, Path)), responses((status = 200, body = SecurityGroupListResponse)), tag = "security-groups")]
pub async fn list_security_groups(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<String>,
) -> Result<Json<SecurityGroupListResponse>, ApiError> {
    let groups = state.store.list_security_groups(Some(&project_id)).await?;
    Ok(Json(SecurityGroupListResponse {
        security_groups: groups.into_iter().map(UiSecurityGroup::from).collect(),
    }))
}

/// Get a security group by ID
#[utoipa::path(get, path = "/v1/security-groups/{id}", params(("id" = String, Path)), responses((status = 200, body = UiSecurityGroup), (status = 404, body = ApiError)), tag = "security-groups")]
pub async fn get_security_group(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<UiSecurityGroup>, ApiError> {
    let sg = state.store.get_security_group(&id).await?.ok_or(ApiError {
        error: "Security group not found".to_string(),
        code: 404,
    })?;
    Ok(Json(UiSecurityGroup::from(sg)))
}

/// Create a new security group
#[utoipa::path(post, path = "/v1/projects/{project_id}/security-groups", params(("project_id" = String, Path)), request_body = UiCreateSecurityGroupRequest, responses((status = 200, body = UiSecurityGroup), (status = 409, body = ApiError)), tag = "security-groups")]
pub async fn create_security_group(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<String>,
    Json(req): Json<UiCreateSecurityGroupRequest>,
) -> Result<Json<UiSecurityGroup>, ApiError> {
    use crate::store::CreateSecurityGroupRequest;

    let sg = state
        .store
        .create_security_group(CreateSecurityGroupRequest {
            project_id,
            name: req.name,
            description: req.description,
        })
        .await?;

    state.audit.security_group_created(&sg.id, &sg.name);

    Ok(Json(UiSecurityGroup::from(sg)))
}

/// Delete a security group
#[utoipa::path(delete, path = "/v1/security-groups/{id}", params(("id" = String, Path)), responses((status = 204), (status = 404, body = ApiError), (status = 409, body = ApiError)), tag = "security-groups")]
pub async fn delete_security_group(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.store.delete_security_group(&id).await?;

    state.audit.security_group_deleted(&id);

    Ok(StatusCode::NO_CONTENT)
}

/// Create a rule in a security group
#[utoipa::path(post, path = "/v1/security-groups/{id}/rules", params(("id" = String, Path)), request_body = UiCreateSecurityGroupRuleRequest, responses((status = 200, body = UiSecurityGroup), (status = 404, body = ApiError)), tag = "security-groups")]
pub async fn create_security_group_rule(
    State(state): State<Arc<AppState>>,
    Path(sg_id): Path<String>,
    Json(req): Json<UiCreateSecurityGroupRuleRequest>,
) -> Result<Json<UiSecurityGroup>, ApiError> {
    use crate::store::CreateSecurityGroupRuleRequest;

    let sg = state
        .store
        .create_security_group_rule(
            &sg_id,
            CreateSecurityGroupRuleRequest {
                direction: req.direction.to_command_direction(),
                protocol: req.protocol.to_protocol_string(),
                port_range_start: req.port_start,
                port_range_end: req.port_end,
                cidr: req.cidr,
                description: req.description,
            },
        )
        .await?;

    state.audit.security_group_rule_created(&sg.id);

    Ok(Json(UiSecurityGroup::from(sg)))
}

/// Delete a rule from a security group
#[utoipa::path(delete, path = "/v1/security-groups/{sg_id}/rules/{rule_id}", params(("sg_id" = String, Path), ("rule_id" = String, Path)), responses((status = 204), (status = 404, body = ApiError)), tag = "security-groups")]
pub async fn delete_security_group_rule(
    State(state): State<Arc<AppState>>,
    Path((sg_id, rule_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    state
        .store
        .delete_security_group_rule(&sg_id, &rule_id)
        .await?;

    state.audit.security_group_rule_deleted(&sg_id, &rule_id);

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Notification Handlers (stub - returns empty data)
// =============================================================================

/// List notifications (stub - returns empty array)
pub async fn list_notifications(State(_state): State<Arc<AppState>>) -> Json<Vec<()>> {
    Json(vec![])
}

/// Mark a notification as read (stub - no-op)
pub async fn mark_notification_read(
    State(_state): State<Arc<AppState>>,
    Path(_id): Path<String>,
) -> StatusCode {
    StatusCode::NO_CONTENT
}

/// Mark all notifications as read (stub - no-op)
pub async fn mark_all_notifications_read(State(_state): State<Arc<AppState>>) -> StatusCode {
    StatusCode::NO_CONTENT
}

// =============================================================================
// Pod Handlers (stub - returns empty data / not implemented)
// =============================================================================

/// List pods (stub)
#[utoipa::path(get, path = "/v1/pods", params(("projectId" = Option<String>, Query)), responses((status = 200, body = super::ui_types::PodListResponse)), tag = "pods")]
pub async fn list_pods(
    State(_state): State<Arc<AppState>>,
    Query(_params): Query<ListPodsQuery>,
) -> Json<super::ui_types::PodListResponse> {
    Json(super::ui_types::PodListResponse { pods: vec![] })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListPodsQuery {
    #[serde(default)]
    pub project_id: Option<String>,
}

/// Get a pod by ID (stub)
#[utoipa::path(get, path = "/v1/pods/{id}", params(("id" = String, Path)), responses((status = 404, body = ApiError)), tag = "pods")]
pub async fn get_pod(
    State(_state): State<Arc<AppState>>,
    Path(_id): Path<String>,
) -> Result<Json<super::ui_types::UiPod>, ApiError> {
    Err(ApiError {
        code: 404,
        error: "Pod not found".into(),
    })
}

/// Create a pod (stub)
#[utoipa::path(post, path = "/v1/pods", request_body = super::ui_types::UiCreatePodRequest, responses((status = 501)), tag = "pods")]
pub async fn create_pod(
    State(_state): State<Arc<AppState>>,
    Json(_request): Json<super::ui_types::UiCreatePodRequest>,
) -> Result<Json<super::ui_types::UiPod>, ApiError> {
    Err(ApiError {
        code: 501,
        error: "Pod creation not yet implemented".into(),
    })
}

/// Delete a pod (stub)
#[utoipa::path(delete, path = "/v1/pods/{id}", params(("id" = String, Path)), responses((status = 404, body = ApiError)), tag = "pods")]
pub async fn delete_pod(
    State(_state): State<Arc<AppState>>,
    Path(_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    Err(ApiError {
        code: 404,
        error: "Pod not found".into(),
    })
}

/// Start a pod (stub)
#[utoipa::path(post, path = "/v1/pods/{id}/start", params(("id" = String, Path)), responses((status = 404, body = ApiError)), tag = "pods")]
pub async fn start_pod(
    State(_state): State<Arc<AppState>>,
    Path(_id): Path<String>,
) -> Result<Json<super::ui_types::UiPod>, ApiError> {
    Err(ApiError {
        code: 404,
        error: "Pod not found".into(),
    })
}

/// Stop a pod (stub)
#[utoipa::path(post, path = "/v1/pods/{id}/stop", params(("id" = String, Path)), responses((status = 404, body = ApiError)), tag = "pods")]
pub async fn stop_pod(
    State(_state): State<Arc<AppState>>,
    Path(_id): Path<String>,
) -> Result<Json<super::ui_types::UiPod>, ApiError> {
    Err(ApiError {
        code: 404,
        error: "Pod not found".into(),
    })
}
