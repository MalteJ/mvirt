use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use uuid::Uuid;

use crate::state::{
    AppState, Container, ContainerSpec, ContainerState, LogEntry, LogLevel, Pod, PodState,
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PodListResponse {
    pods: Vec<Pod>,
}

pub async fn list_pods(State(state): State<AppState>) -> Json<PodListResponse> {
    let inner = state.inner.read().await;
    let pods: Vec<Pod> = inner.pods.values().cloned().collect();
    Json(PodListResponse { pods })
}

pub async fn get_pod(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Pod>, StatusCode> {
    let inner = state.inner.read().await;
    inner
        .pods
        .get(&id)
        .cloned()
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatePodRequest {
    name: String,
    network_id: String,
    containers: Vec<ContainerSpec>,
}

pub async fn create_pod(
    State(state): State<AppState>,
    Json(req): Json<CreatePodRequest>,
) -> Result<Json<Pod>, StatusCode> {
    let mut inner = state.inner.write().await;

    let containers: Vec<Container> = req
        .containers
        .iter()
        .map(|spec| Container {
            id: Uuid::new_v4().to_string(),
            name: spec.name.clone(),
            state: ContainerState::Created,
            image: spec.image.clone(),
            exit_code: None,
            error_message: None,
        })
        .collect();

    let pod = Pod {
        id: Uuid::new_v4().to_string(),
        name: req.name.clone(),
        state: PodState::Created,
        network_id: req.network_id.clone(),
        vm_id: None,
        containers,
        ip_address: None,
        created_at: chrono::Utc::now().to_rfc3339(),
        started_at: None,
        error_message: None,
    };

    let log_entry = LogEntry {
        id: Uuid::new_v4().to_string(),
        timestamp_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
        message: format!("Pod '{}' created", req.name),
        level: LogLevel::Audit,
        component: "pod".to_string(),
        related_object_ids: vec![pod.id.clone()],
    };
    inner.logs.push(log_entry.clone());
    let _ = state.log_events_tx.send(log_entry);

    inner.pods.insert(pod.id.clone(), pod.clone());
    Ok(Json(pod))
}

pub async fn delete_pod(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    let mut inner = state.inner.write().await;

    if let Some(pod) = inner.pods.remove(&id) {
        let log_entry = LogEntry {
            id: Uuid::new_v4().to_string(),
            timestamp_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
            message: format!("Pod '{}' deleted", pod.name),
            level: LogLevel::Audit,
            component: "pod".to_string(),
            related_object_ids: vec![id],
        };
        inner.logs.push(log_entry.clone());
        let _ = state.log_events_tx.send(log_entry);

        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

pub async fn start_pod(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Pod>, StatusCode> {
    let mut inner = state.inner.write().await;

    if let Some(pod) = inner.pods.get_mut(&id) {
        if matches!(pod.state, PodState::Stopped | PodState::Created) {
            pod.state = PodState::Starting;
            // Set containers to Creating
            for container in &mut pod.containers {
                container.state = ContainerState::Creating;
            }
            let pod_clone = pod.clone();

            // Simulate startup delay
            let state_clone = state.clone();
            let id_clone = id.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let mut inner = state_clone.inner.write().await;
                if let Some(pod) = inner.pods.get_mut(&id_clone) {
                    pod.state = PodState::Running;
                    pod.vm_id = Some(Uuid::new_v4().to_string());
                    pod.ip_address = Some(format!("10.0.1.{}", rand::random::<u8>()));
                    pod.started_at = Some(chrono::Utc::now().to_rfc3339());
                    // Set containers to Running
                    for container in &mut pod.containers {
                        container.state = ContainerState::Running;
                        container.exit_code = None;
                    }

                    let log_entry = LogEntry {
                        id: Uuid::new_v4().to_string(),
                        timestamp_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
                        message: format!("Pod '{}' started", pod.name),
                        level: LogLevel::Audit,
                        component: "pod".to_string(),
                        related_object_ids: vec![id_clone],
                    };
                    inner.logs.push(log_entry.clone());
                    let _ = state_clone.log_events_tx.send(log_entry);
                }
            });

            Ok(Json(pod_clone))
        } else {
            Err(StatusCode::CONFLICT)
        }
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

pub async fn stop_pod(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Pod>, StatusCode> {
    let mut inner = state.inner.write().await;

    if let Some(pod) = inner.pods.get_mut(&id) {
        if matches!(pod.state, PodState::Running) {
            pod.state = PodState::Stopping;
            let pod_clone = pod.clone();

            let state_clone = state.clone();
            let id_clone = id.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let mut inner = state_clone.inner.write().await;
                if let Some(pod) = inner.pods.get_mut(&id_clone) {
                    pod.state = PodState::Stopped;
                    pod.vm_id = None;
                    pod.ip_address = None;
                    pod.started_at = None;
                    // Set containers to Stopped
                    for container in &mut pod.containers {
                        container.state = ContainerState::Stopped;
                        container.exit_code = Some(0);
                    }

                    let log_entry = LogEntry {
                        id: Uuid::new_v4().to_string(),
                        timestamp_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
                        message: format!("Pod '{}' stopped", pod.name),
                        level: LogLevel::Audit,
                        component: "pod".to_string(),
                        related_object_ids: vec![id_clone],
                    };
                    inner.logs.push(log_entry.clone());
                    let _ = state_clone.log_events_tx.send(log_entry);
                }
            });

            Ok(Json(pod_clone))
        } else {
            Err(StatusCode::CONFLICT)
        }
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}
