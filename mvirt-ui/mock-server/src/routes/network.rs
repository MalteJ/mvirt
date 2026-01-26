use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::state::{AppState, LogEntry, LogLevel, Network, Nic, NicState};

#[derive(Debug, Deserialize)]
pub struct ListNetworksQuery {
    #[serde(alias = "projectId")]
    project_id: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkListResponse {
    networks: Vec<Network>,
}

pub async fn list_networks(
    State(state): State<AppState>,
    Query(query): Query<ListNetworksQuery>,
) -> Json<NetworkListResponse> {
    let inner = state.inner.read().await;
    let networks: Vec<Network> = inner
        .networks
        .values()
        .filter(|n| {
            query
                .project_id
                .as_ref()
                .map_or(true, |pid| &n.project_id == pid)
        })
        .cloned()
        .collect();
    Json(NetworkListResponse { networks })
}

pub async fn get_network(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Network>, StatusCode> {
    let inner = state.inner.read().await;
    inner
        .networks
        .get(&id)
        .cloned()
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateNetworkRequest {
    name: String,
    project_id: String,
    ipv4_subnet: Option<String>,
    ipv6_prefix: Option<String>,
}

pub async fn create_network(
    State(state): State<AppState>,
    Json(req): Json<CreateNetworkRequest>,
) -> Result<Json<Network>, StatusCode> {
    let mut inner = state.inner.write().await;

    let network = Network {
        id: Uuid::new_v4().to_string(),
        project_id: req.project_id,
        name: req.name.clone(),
        ipv4_subnet: req.ipv4_subnet,
        ipv6_prefix: req.ipv6_prefix,
        nic_count: 0,
    };

    let log_entry = LogEntry {
        id: Uuid::new_v4().to_string(),
        project_id: network.project_id.clone(),
        timestamp_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
        message: format!("Network '{}' created", req.name),
        level: LogLevel::Audit,
        component: "net".to_string(),
        related_object_ids: vec![network.id.clone()],
    };
    inner.logs.push(log_entry.clone());
    let _ = state.log_events_tx.send(log_entry);

    inner.networks.insert(network.id.clone(), network.clone());
    Ok(Json(network))
}

pub async fn delete_network(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    let mut inner = state.inner.write().await;

    if let Some(net) = inner.networks.remove(&id) {
        let log_entry = LogEntry {
            id: Uuid::new_v4().to_string(),
            project_id: net.project_id.clone(),
            timestamp_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
            message: format!("Network '{}' deleted", net.name),
            level: LogLevel::Audit,
            component: "net".to_string(),
            related_object_ids: vec![id],
        };
        inner.logs.push(log_entry.clone());
        let _ = state.log_events_tx.send(log_entry);

        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

#[derive(Debug, Deserialize)]
pub struct ListNicsQuery {
    #[serde(alias = "projectId")]
    project_id: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NicListResponse {
    nics: Vec<Nic>,
}

pub async fn list_nics(
    State(state): State<AppState>,
    Query(query): Query<ListNicsQuery>,
) -> Json<NicListResponse> {
    let inner = state.inner.read().await;
    let nics: Vec<Nic> = inner
        .nics
        .values()
        .filter(|n| {
            query
                .project_id
                .as_ref()
                .map_or(true, |pid| &n.project_id == pid)
        })
        .cloned()
        .collect();
    Json(NicListResponse { nics })
}

pub async fn get_nic(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Nic>, StatusCode> {
    let inner = state.inner.read().await;
    inner
        .nics
        .get(&id)
        .cloned()
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

fn generate_mac() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    format!(
        "52:54:00:{:02x}:{:02x}:{:02x}",
        rng.gen::<u8>(),
        rng.gen::<u8>(),
        rng.gen::<u8>()
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateNicRequest {
    name: String,
    project_id: String,
    network_id: String,
    mac_address: Option<String>,
}

pub async fn create_nic(
    State(state): State<AppState>,
    Json(req): Json<CreateNicRequest>,
) -> Result<Json<Nic>, StatusCode> {
    let mut inner = state.inner.write().await;

    // Check if network exists
    if !inner.networks.contains_key(&req.network_id) {
        return Err(StatusCode::NOT_FOUND);
    }

    let nic = Nic {
        id: Uuid::new_v4().to_string(),
        project_id: req.project_id,
        name: req.name.clone(),
        mac_address: req.mac_address.unwrap_or_else(generate_mac),
        network_id: req.network_id.clone(),
        vm_id: None,
        state: NicState::Detached,
        ipv4_address: None,
        ipv6_address: None,
    };

    // Increment network nic_count
    if let Some(net) = inner.networks.get_mut(&req.network_id) {
        net.nic_count += 1;
    }

    let log_entry = LogEntry {
        id: Uuid::new_v4().to_string(),
        project_id: nic.project_id.clone(),
        timestamp_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
        message: format!("NIC '{}' created", req.name),
        level: LogLevel::Audit,
        component: "net".to_string(),
        related_object_ids: vec![nic.id.clone(), req.network_id],
    };
    inner.logs.push(log_entry.clone());
    let _ = state.log_events_tx.send(log_entry);

    inner.nics.insert(nic.id.clone(), nic.clone());
    Ok(Json(nic))
}

pub async fn delete_nic(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    let mut inner = state.inner.write().await;

    if let Some(nic) = inner.nics.remove(&id) {
        // Decrement network nic_count
        if let Some(net) = inner.networks.get_mut(&nic.network_id) {
            net.nic_count = net.nic_count.saturating_sub(1);
        }

        let log_entry = LogEntry {
            id: Uuid::new_v4().to_string(),
            project_id: nic.project_id.clone(),
            timestamp_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
            message: format!("NIC '{}' deleted", nic.name),
            level: LogLevel::Audit,
            component: "net".to_string(),
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
pub struct AttachNicRequest {
    vm_id: String,
}

pub async fn attach_nic(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<AttachNicRequest>,
) -> Result<Json<Nic>, StatusCode> {
    let mut inner = state.inner.write().await;

    // Check if VM exists
    if !inner.vms.contains_key(&req.vm_id) {
        return Err(StatusCode::NOT_FOUND);
    }

    if let Some(nic) = inner.nics.get_mut(&id) {
        nic.vm_id = Some(req.vm_id.clone());
        nic.state = NicState::Attached;
        let nic_clone = nic.clone();

        let log_entry = LogEntry {
            id: Uuid::new_v4().to_string(),
            project_id: nic.project_id.clone(),
            timestamp_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
            message: format!("NIC '{}' attached to VM", nic.name),
            level: LogLevel::Audit,
            component: "net".to_string(),
            related_object_ids: vec![id, req.vm_id],
        };
        inner.logs.push(log_entry.clone());
        let _ = state.log_events_tx.send(log_entry);

        Ok(Json(nic_clone))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

pub async fn detach_nic(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Nic>, StatusCode> {
    let mut inner = state.inner.write().await;

    if let Some(nic) = inner.nics.get_mut(&id) {
        nic.vm_id = None;
        nic.state = NicState::Detached;
        let nic_clone = nic.clone();

        let log_entry = LogEntry {
            id: Uuid::new_v4().to_string(),
            project_id: nic.project_id.clone(),
            timestamp_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
            message: format!("NIC '{}' detached", nic.name),
            level: LogLevel::Audit,
            component: "net".to_string(),
            related_object_ids: vec![id],
        };
        inner.logs.push(log_entry.clone());
        let _ = state.log_events_tx.send(log_entry);

        Ok(Json(nic_clone))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}
