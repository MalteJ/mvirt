//! UI-compatible REST handlers.
//!
//! These handlers match the mock-server's JSON structure for compatibility with mvirt-ui.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Sse, sse::Event as SseEvent},
};
use futures::stream::{self, Stream};
use serde::{Deserialize, Serialize};
use std::{convert::Infallible, sync::Arc, time::Duration};

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
// Project Handlers
// =============================================================================

/// List all projects
pub async fn list_projects(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ProjectListResponse>, ApiError> {
    let projects = state.store.list_projects().await?;
    Ok(Json(ProjectListResponse {
        projects: projects.into_iter().map(UiProject::from).collect(),
    }))
}

/// Get a project by ID
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

/// List all VMs
pub async fn list_vms(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListVmsQuery>,
) -> Result<Json<VmListResponse>, ApiError> {
    let vms = match &query.node_id {
        Some(node_id) => state.store.list_vms_by_node(node_id).await?,
        None => state.store.list_vms().await?,
    };

    // Filter by project if specified
    let vms: Vec<UiVm> = vms
        .into_iter()
        .filter(|vm| {
            query
                .project_id
                .as_ref()
                .is_none_or(|pid| &vm.spec.project_id == pid)
        })
        .map(UiVm::from)
        .collect();

    Ok(Json(VmListResponse { vms }))
}

/// Get a VM by ID
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
pub async fn create_vm(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UiCreateVmRequest>,
) -> Result<Json<UiVm>, ApiError> {
    // Convert UI request to internal spec
    let spec = VmSpec {
        name: req.name.clone(),
        project_id: req.project_id,
        node_selector: req.node_selector,
        cpu_cores: req.config.vcpus,
        memory_mb: req.config.memory_mb,
        volume_id: req.config.volume_id,
        nic_id: req.config.nic_id,
        image: req.config.image,
        desired_state: VmDesiredState::Running,
    };

    let store_req = StoreCreateVmRequest { spec };

    // Create and schedule the VM
    let data = state.store.create_and_schedule_vm(store_req).await?;
    state.audit.vm_created(&data.id, &data.spec.name);
    Ok(Json(UiVm::from(data)))
}

/// Delete a VM
pub async fn delete_vm(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.store.delete_vm(&id).await?;
    state.audit.vm_deleted(&id);
    Ok(StatusCode::NO_CONTENT)
}

/// Start a VM
pub async fn start_vm(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<UiVm>, ApiError> {
    // Set desired_state to Running
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
                        node_id: None, // Preserve existing
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
pub async fn stop_vm(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<UiVm>, ApiError> {
    // Set desired_state to Stopped
    let store_req = StoreUpdateVmSpecRequest {
        desired_state: VmDesiredState::Stopped,
    };
    let vm = state.store.update_vm_spec(&id, store_req).await?;
    state.audit.vm_stopped(&vm.id);

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
                        phase: VmPhase::Stopped,
                        node_id: None, // Preserve existing
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
pub async fn kill_vm(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<UiVm>, ApiError> {
    // Set desired_state to Stopped
    let store_req = StoreUpdateVmSpecRequest {
        desired_state: VmDesiredState::Stopped,
    };
    state.store.update_vm_spec(&id, store_req).await?;

    // Immediate phase change to Stopped
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
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let mut rx = state.store.subscribe();

    let stream = async_stream::stream! {
        while let Ok(event) = rx.recv().await {
            if event.resource_type() == "vm" {
                // For simplicity, just send the event type
                // In production, you'd include the full VM data
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

/// List all networks
pub async fn list_networks(
    State(state): State<Arc<AppState>>,
    Query(_query): Query<ListNetworksQuery>,
) -> Result<Json<NetworkListResponse>, ApiError> {
    let networks = state.store.list_networks().await?;
    Ok(Json(NetworkListResponse {
        networks: networks.into_iter().map(UiNetwork::from).collect(),
    }))
}

/// Get a network by ID
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
pub async fn create_network(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UiCreateNetworkRequest>,
) -> Result<Json<UiNetwork>, ApiError> {
    let store_req = StoreCreateNetworkRequest {
        project_id: req.project_id,
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

/// List all NICs
pub async fn list_nics(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListNicsQuery>,
) -> Result<Json<NicListResponse>, ApiError> {
    let nics = state.store.list_nics(query.network_id.as_deref()).await?;
    Ok(Json(NicListResponse {
        nics: nics.into_iter().map(UiNic::from).collect(),
    }))
}

/// Get a NIC by ID
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
pub async fn create_nic(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UiCreateNicRequest>,
) -> Result<Json<UiNic>, ApiError> {
    let store_req = StoreCreateNicRequest {
        project_id: req.project_id,
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
pub async fn delete_nic(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.store.delete_nic(&id).await?;
    state.audit.nic_deleted(&id);
    Ok(StatusCode::NO_CONTENT)
}

/// Attach a NIC to a VM (stub)
pub async fn attach_nic(
    State(_state): State<Arc<AppState>>,
    Path(_id): Path<String>,
) -> Result<Json<UiNic>, ApiError> {
    // TODO: Implement NIC attach
    Err(ApiError {
        error: "Not implemented".to_string(),
        code: 501,
    })
}

/// Detach a NIC from a VM (stub)
pub async fn detach_nic(
    State(_state): State<Arc<AppState>>,
    Path(_id): Path<String>,
) -> Result<Json<UiNic>, ApiError> {
    // TODO: Implement NIC detach
    Err(ApiError {
        error: "Not implemented".to_string(),
        code: 501,
    })
}

// =============================================================================
// Storage Handlers
// =============================================================================

/// List all volumes
pub async fn list_volumes(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListVolumesQuery>,
) -> Result<Json<VolumeListResponse>, ApiError> {
    let volumes = state
        .store
        .list_volumes(query.project_id.as_deref(), query.node_id.as_deref())
        .await?;
    Ok(Json(VolumeListResponse {
        volumes: volumes.into_iter().map(UiVolume::from).collect(),
    }))
}

/// Get a volume by ID
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
pub async fn create_volume(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UiCreateVolumeRequest>,
) -> Result<Json<UiVolume>, ApiError> {
    let store_req = StoreCreateVolumeRequest {
        project_id: req.project_id,
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
pub async fn delete_volume(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.store.delete_volume(&id).await?;
    state.audit.volume_deleted(&id);
    Ok(StatusCode::NO_CONTENT)
}

/// Resize a volume
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

/// List all templates
pub async fn list_templates(
    State(state): State<Arc<AppState>>,
) -> Result<Json<TemplateListResponse>, ApiError> {
    let templates = state.store.list_templates(None).await?;
    Ok(Json(TemplateListResponse {
        templates: templates.into_iter().map(UiTemplate::from).collect(),
    }))
}

/// Import a template
pub async fn import_template(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UiImportTemplateRequest>,
) -> Result<Json<UiImportJob>, ApiError> {
    let store_req = StoreImportTemplateRequest {
        node_id: req.node_id,
        name: req.name,
        url: req.url,
        total_bytes: req.total_bytes,
    };

    let data = state.store.import_template(store_req).await?;
    state.audit.template_import_started(&data.id);
    Ok(Json(UiImportJob::from(data)))
}

/// Get an import job by ID
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

/// Get storage pool statistics (mock for now)
pub async fn get_pool_stats(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<UiPoolStats>, ApiError> {
    // Mock stats for now
    Ok(Json(UiPoolStats {
        total_bytes: 1_000_000_000_000, // 1TB
        used_bytes: 250_000_000_000,    // 250GB
        available_bytes: 750_000_000_000,
        compression_ratio: 1.5,
    }))
}

// =============================================================================
// Logs Handlers (stub)
// =============================================================================

/// Query parameters for logs
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryLogsParams {
    #[serde(default)]
    pub object_id: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
}

/// Log entry for UI
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiLogEntry {
    pub id: String,
    pub timestamp: String,
    pub message: String,
    pub level: String,
    pub component: String,
}

/// Response wrapper for logs
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogsResponse {
    pub logs: Vec<UiLogEntry>,
}

/// Query logs (stub)
pub async fn query_logs(
    State(_state): State<Arc<AppState>>,
    Query(_params): Query<QueryLogsParams>,
) -> Result<Json<LogsResponse>, ApiError> {
    // Stub - return empty logs
    Ok(Json(LogsResponse { logs: vec![] }))
}

/// SSE stream for log events (stub)
pub async fn log_events(
    State(_state): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let stream = stream::empty();
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
    // Stub - return not implemented
    (
        StatusCode::NOT_IMPLEMENTED,
        "Console WebSocket not implemented",
    )
}

// =============================================================================
// Security Group Handlers (in-memory store)
// =============================================================================

use std::sync::{LazyLock, Mutex};

static SECURITY_GROUPS: LazyLock<Mutex<Vec<UiSecurityGroup>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

/// List all security groups
pub async fn list_security_groups(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<SecurityGroupListResponse>, ApiError> {
    let groups = SECURITY_GROUPS.lock().unwrap();
    Ok(Json(SecurityGroupListResponse {
        security_groups: groups.clone(),
    }))
}

/// Get a security group by ID
pub async fn get_security_group(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<UiSecurityGroup>, ApiError> {
    let groups = SECURITY_GROUPS.lock().unwrap();
    match groups.iter().find(|g| g.id == id) {
        Some(group) => Ok(Json(group.clone())),
        None => Err(ApiError {
            error: "Security group not found".to_string(),
            code: 404,
        }),
    }
}

/// Create a new security group
pub async fn create_security_group(
    State(_state): State<Arc<AppState>>,
    Json(req): Json<UiCreateSecurityGroupRequest>,
) -> Result<Json<UiSecurityGroup>, ApiError> {
    let now = chrono::Utc::now().to_rfc3339();
    let group = UiSecurityGroup {
        id: uuid::Uuid::new_v4().to_string(),
        name: req.name.clone(),
        description: req.description,
        rules: Vec::new(),
        nic_count: 0,
        created_at: now.clone(),
        updated_at: now,
    };

    {
        let mut groups = SECURITY_GROUPS.lock().unwrap();
        groups.push(group.clone());
    }

    Ok(Json(group))
}

/// Delete a security group
pub async fn delete_security_group(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let mut groups = SECURITY_GROUPS.lock().unwrap();
    let initial_len = groups.len();
    groups.retain(|g| g.id != id);

    if groups.len() == initial_len {
        return Err(ApiError {
            error: "Security group not found".to_string(),
            code: 404,
        });
    }

    Ok(StatusCode::NO_CONTENT)
}

/// Create a rule in a security group
pub async fn create_security_group_rule(
    State(_state): State<Arc<AppState>>,
    Path(sg_id): Path<String>,
    Json(req): Json<UiCreateSecurityGroupRuleRequest>,
) -> Result<Json<UiSecurityGroupRule>, ApiError> {
    let mut groups = SECURITY_GROUPS.lock().unwrap();
    let group = groups.iter_mut().find(|g| g.id == sg_id).ok_or(ApiError {
        error: "Security group not found".to_string(),
        code: 404,
    })?;

    let now = chrono::Utc::now().to_rfc3339();
    let rule = UiSecurityGroupRule {
        id: uuid::Uuid::new_v4().to_string(),
        security_group_id: sg_id.clone(),
        direction: req.direction,
        protocol: req.protocol,
        port_start: req.port_start,
        port_end: req.port_end,
        cidr: req.cidr,
        description: req.description,
        created_at: now.clone(),
    };

    group.rules.push(rule.clone());
    group.updated_at = now;

    Ok(Json(rule))
}

/// Delete a rule from a security group
pub async fn delete_security_group_rule(
    State(_state): State<Arc<AppState>>,
    Path((sg_id, rule_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let mut groups = SECURITY_GROUPS.lock().unwrap();
    let group = groups.iter_mut().find(|g| g.id == sg_id).ok_or(ApiError {
        error: "Security group not found".to_string(),
        code: 404,
    })?;

    let initial_len = group.rules.len();
    group.rules.retain(|r| r.id != rule_id);

    if group.rules.len() == initial_len {
        return Err(ApiError {
            error: "Rule not found".to_string(),
            code: 404,
        });
    }

    group.updated_at = chrono::Utc::now().to_rfc3339();

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
