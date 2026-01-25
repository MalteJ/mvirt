use axum::{
    extract::{Path, State, WebSocketUpgrade},
    http::StatusCode,
    response::{sse::Event, IntoResponse, Sse},
    Json,
};
use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use std::{convert::Infallible, time::Duration};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use uuid::Uuid;

use crate::state::{AppState, LogEntry, LogLevel, Vm, VmConfig, VmState};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VmListResponse {
    vms: Vec<Vm>,
}

pub async fn list_vms(State(state): State<AppState>) -> Json<VmListResponse> {
    let inner = state.inner.read().await;
    let vms: Vec<Vm> = inner.vms.values().cloned().collect();
    Json(VmListResponse { vms })
}

pub async fn get_vm(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vm>, StatusCode> {
    let inner = state.inner.read().await;
    inner
        .vms
        .get(&id)
        .cloned()
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateVmRequest {
    name: String,
    config: VmConfig,
}

pub async fn create_vm(
    State(state): State<AppState>,
    Json(req): Json<CreateVmRequest>,
) -> Result<Json<Vm>, StatusCode> {
    let mut inner = state.inner.write().await;

    let vm = Vm {
        id: Uuid::new_v4().to_string(),
        name: req.name.clone(),
        state: VmState::Stopped,
        config: req.config,
        created_at: chrono::Utc::now().to_rfc3339(),
        started_at: None,
    };

    let log_entry = LogEntry {
        id: Uuid::new_v4().to_string(),
        timestamp_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
        message: format!("VM '{}' created", req.name),
        level: LogLevel::Audit,
        component: "vmm".to_string(),
        related_object_ids: vec![vm.id.clone()],
    };
    inner.logs.push(log_entry.clone());
    let _ = state.log_events_tx.send(log_entry);

    inner.vms.insert(vm.id.clone(), vm.clone());
    Ok(Json(vm))
}

pub async fn delete_vm(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    let mut inner = state.inner.write().await;

    if let Some(vm) = inner.vms.remove(&id) {
        let log_entry = LogEntry {
            id: Uuid::new_v4().to_string(),
            timestamp_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
            message: format!("VM '{}' deleted", vm.name),
            level: LogLevel::Audit,
            component: "vmm".to_string(),
            related_object_ids: vec![id],
        };
        inner.logs.push(log_entry.clone());
        let _ = state.log_events_tx.send(log_entry);

        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

pub async fn start_vm(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vm>, StatusCode> {
    let mut inner = state.inner.write().await;

    if let Some(vm) = inner.vms.get_mut(&id) {
        if matches!(vm.state, VmState::Stopped) {
            vm.state = VmState::Starting;
            let vm_clone = vm.clone();
            let _ = state.vm_events_tx.send(vm_clone.clone());

            // Simulate startup delay
            let state_clone = state.clone();
            let id_clone = id.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let mut inner = state_clone.inner.write().await;
                if let Some(vm) = inner.vms.get_mut(&id_clone) {
                    vm.state = VmState::Running;
                    vm.started_at = Some(chrono::Utc::now().to_rfc3339());
                    let _ = state_clone.vm_events_tx.send(vm.clone());

                    let log_entry = LogEntry {
                        id: Uuid::new_v4().to_string(),
                        timestamp_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
                        message: format!("VM '{}' started", vm.name),
                        level: LogLevel::Audit,
                        component: "vmm".to_string(),
                        related_object_ids: vec![id_clone],
                    };
                    inner.logs.push(log_entry.clone());
                    let _ = state_clone.log_events_tx.send(log_entry);
                }
            });

            Ok(Json(vm_clone))
        } else {
            Err(StatusCode::CONFLICT)
        }
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

pub async fn stop_vm(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vm>, StatusCode> {
    let mut inner = state.inner.write().await;

    if let Some(vm) = inner.vms.get_mut(&id) {
        if matches!(vm.state, VmState::Running) {
            vm.state = VmState::Stopping;
            let vm_clone = vm.clone();
            let _ = state.vm_events_tx.send(vm_clone.clone());

            let state_clone = state.clone();
            let id_clone = id.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let mut inner = state_clone.inner.write().await;
                if let Some(vm) = inner.vms.get_mut(&id_clone) {
                    vm.state = VmState::Stopped;
                    vm.started_at = None;
                    let _ = state_clone.vm_events_tx.send(vm.clone());

                    let log_entry = LogEntry {
                        id: Uuid::new_v4().to_string(),
                        timestamp_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
                        message: format!("VM '{}' stopped", vm.name),
                        level: LogLevel::Audit,
                        component: "vmm".to_string(),
                        related_object_ids: vec![id_clone],
                    };
                    inner.logs.push(log_entry.clone());
                    let _ = state_clone.log_events_tx.send(log_entry);
                }
            });

            Ok(Json(vm_clone))
        } else {
            Err(StatusCode::CONFLICT)
        }
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

pub async fn kill_vm(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vm>, StatusCode> {
    let mut inner = state.inner.write().await;

    if let Some(vm) = inner.vms.get_mut(&id) {
        vm.state = VmState::Stopped;
        vm.started_at = None;
        let vm_clone = vm.clone();
        let _ = state.vm_events_tx.send(vm_clone.clone());

        let log_entry = LogEntry {
            id: Uuid::new_v4().to_string(),
            timestamp_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
            message: format!("VM '{}' killed", vm.name),
            level: LogLevel::Audit,
            component: "vmm".to_string(),
            related_object_ids: vec![id],
        };
        inner.logs.push(log_entry.clone());
        let _ = state.log_events_tx.send(log_entry);

        Ok(Json(vm_clone))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

pub async fn console_ws(ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(|mut socket| async move {
        use axum::extract::ws::Message;

        // Echo server for mock console
        while let Some(Ok(msg)) = socket.recv().await {
            if let Message::Text(text) = msg {
                // Echo back the input
                if socket.send(Message::Text(text)).await.is_err() {
                    break;
                }
            }
        }
    })
}

pub async fn vm_events(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.vm_events_tx.subscribe();
    let stream = BroadcastStream::new(rx)
        .filter_map(|result| result.ok())
        .map(|vm| Ok(Event::default().data(serde_json::to_string(&vm).unwrap_or_default())));

    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    )
}
