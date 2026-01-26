use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::state::{
    AppState, ImportJob, ImportJobState, LogEntry, LogLevel, PoolStats, Snapshot, Template, Volume,
};

#[derive(Debug, Deserialize)]
pub struct ListVolumesQuery {
    #[serde(alias = "projectId")]
    project_id: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeListResponse {
    volumes: Vec<Volume>,
}

pub async fn list_volumes(
    State(state): State<AppState>,
    Query(query): Query<ListVolumesQuery>,
) -> Json<VolumeListResponse> {
    let inner = state.inner.read().await;
    let volumes: Vec<Volume> = inner
        .volumes
        .values()
        .filter(|v| {
            query
                .project_id
                .as_ref()
                .map_or(true, |pid| &v.project_id == pid)
        })
        .cloned()
        .collect();
    Json(VolumeListResponse { volumes })
}

pub async fn get_volume(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Volume>, StatusCode> {
    let inner = state.inner.read().await;
    inner
        .volumes
        .get(&id)
        .cloned()
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateVolumeRequest {
    name: String,
    project_id: String,
    size_bytes: u64,
    template_id: Option<String>,
}

pub async fn create_volume(
    State(state): State<AppState>,
    Json(req): Json<CreateVolumeRequest>,
) -> Result<Json<Volume>, StatusCode> {
    let mut inner = state.inner.write().await;

    let volume = Volume {
        id: Uuid::new_v4().to_string(),
        project_id: req.project_id,
        name: req.name.clone(),
        path: format!("tank/vm/{}", req.name),
        volsize_bytes: req.size_bytes,
        used_bytes: 0,
        compression_ratio: 1.0,
        snapshots: vec![],
    };

    let log_entry = LogEntry {
        id: Uuid::new_v4().to_string(),
        project_id: volume.project_id.clone(),
        timestamp_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
        message: format!("Volume '{}' created", req.name),
        level: LogLevel::Audit,
        component: "zfs".to_string(),
        related_object_ids: vec![volume.id.clone()],
    };
    inner.logs.push(log_entry.clone());
    let _ = state.log_events_tx.send(log_entry);

    inner.volumes.insert(volume.id.clone(), volume.clone());
    Ok(Json(volume))
}

pub async fn delete_volume(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    let mut inner = state.inner.write().await;

    if let Some(vol) = inner.volumes.remove(&id) {
        let log_entry = LogEntry {
            id: Uuid::new_v4().to_string(),
            project_id: vol.project_id.clone(),
            timestamp_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
            message: format!("Volume '{}' deleted", vol.name),
            level: LogLevel::Audit,
            component: "zfs".to_string(),
            related_object_ids: vec![id],
        };
        inner.logs.push(log_entry.clone());
        let _ = state.log_events_tx.send(log_entry);

        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResizeVolumeRequest {
    size_bytes: u64,
}

pub async fn resize_volume(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ResizeVolumeRequest>,
) -> Result<Json<Volume>, StatusCode> {
    let mut inner = state.inner.write().await;

    if let Some(volume) = inner.volumes.get_mut(&id) {
        volume.volsize_bytes = req.size_bytes;
        let vol_clone = volume.clone();

        let log_entry = LogEntry {
            id: Uuid::new_v4().to_string(),
            project_id: volume.project_id.clone(),
            timestamp_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
            message: format!(
                "Volume '{}' resized to {} bytes",
                volume.name, req.size_bytes
            ),
            level: LogLevel::Audit,
            component: "zfs".to_string(),
            related_object_ids: vec![id],
        };
        inner.logs.push(log_entry.clone());
        let _ = state.log_events_tx.send(log_entry);

        Ok(Json(vol_clone))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSnapshotRequest {
    name: String,
}

pub async fn create_snapshot(
    State(state): State<AppState>,
    Path(volume_id): Path<String>,
    Json(req): Json<CreateSnapshotRequest>,
) -> Result<Json<Volume>, StatusCode> {
    let mut inner = state.inner.write().await;

    if let Some(volume) = inner.volumes.get_mut(&volume_id) {
        let snapshot = Snapshot {
            id: Uuid::new_v4().to_string(),
            name: req.name.clone(),
            created_at: chrono::Utc::now().to_rfc3339(),
            used_bytes: 0,
        };
        volume.snapshots.push(snapshot.clone());
        let vol_clone = volume.clone();

        let log_entry = LogEntry {
            id: Uuid::new_v4().to_string(),
            project_id: volume.project_id.clone(),
            timestamp_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
            message: format!(
                "Snapshot '{}' created for volume '{}'",
                req.name, volume.name
            ),
            level: LogLevel::Audit,
            component: "zfs".to_string(),
            related_object_ids: vec![volume_id, snapshot.id],
        };
        inner.logs.push(log_entry.clone());
        let _ = state.log_events_tx.send(log_entry);

        Ok(Json(vol_clone))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateListResponse {
    templates: Vec<Template>,
}

pub async fn list_templates(State(state): State<AppState>) -> Json<TemplateListResponse> {
    let inner = state.inner.read().await;
    let templates: Vec<Template> = inner.templates.values().cloned().collect();
    Json(TemplateListResponse { templates })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportTemplateRequest {
    name: String,
    url: String,
}

pub async fn import_template(
    State(state): State<AppState>,
    Json(req): Json<ImportTemplateRequest>,
) -> Result<Json<ImportJob>, StatusCode> {
    let mut inner = state.inner.write().await;

    let job = ImportJob {
        id: Uuid::new_v4().to_string(),
        template_name: req.name.clone(),
        state: ImportJobState::Running,
        bytes_written: 0,
        total_bytes: 2 * 1024 * 1024 * 1024, // 2GB mock
        error: None,
    };

    inner.import_jobs.insert(job.id.clone(), job.clone());

    // Simulate import progress
    let state_clone = state.clone();
    let job_id = job.id.clone();
    let template_name = req.name;
    tokio::spawn(async move {
        let total = 2 * 1024 * 1024 * 1024u64;
        let chunk = total / 10;

        for i in 1..=10 {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let mut inner = state_clone.inner.write().await;
            if let Some(job) = inner.import_jobs.get_mut(&job_id) {
                job.bytes_written = chunk * i;
                if i == 10 {
                    job.state = ImportJobState::Completed;

                    // Create the template
                    let template = Template {
                        id: Uuid::new_v4().to_string(),
                        name: template_name.clone(),
                        size_bytes: total,
                        clone_count: 0,
                    };
                    inner.templates.insert(template.id.clone(), template);

                    let log_entry = LogEntry {
                        id: Uuid::new_v4().to_string(),
                        project_id: String::new(), // Templates are global
                        timestamp_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
                        message: format!("Template '{}' imported", template_name),
                        level: LogLevel::Audit,
                        component: "zfs".to_string(),
                        related_object_ids: vec![job_id.clone()],
                    };
                    inner.logs.push(log_entry.clone());
                    let _ = state_clone.log_events_tx.send(log_entry);
                }
            }
        }
    });

    Ok(Json(job))
}

pub async fn get_import_job(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ImportJob>, StatusCode> {
    let inner = state.inner.read().await;
    inner
        .import_jobs
        .get(&id)
        .cloned()
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

pub async fn get_pool_stats(State(_state): State<AppState>) -> Json<PoolStats> {
    Json(PoolStats {
        name: "tank".to_string(),
        total_bytes: 1024 * 1024 * 1024 * 1024, // 1TB
        used_bytes: 300 * 1024 * 1024 * 1024,   // 300GB
        free_bytes: 724 * 1024 * 1024 * 1024,   // 724GB
    })
}
