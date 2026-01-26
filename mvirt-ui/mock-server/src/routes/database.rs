use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use uuid::Uuid;

use crate::state::{AppState, Database, DatabaseState, DatabaseType, LogEntry, LogLevel};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseListResponse {
    databases: Vec<Database>,
}

pub async fn list_databases(State(state): State<AppState>) -> Json<DatabaseListResponse> {
    let inner = state.inner.read().await;
    let databases: Vec<Database> = inner.databases.values().cloned().collect();
    Json(DatabaseListResponse { databases })
}

pub async fn get_database(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Database>, StatusCode> {
    let inner = state.inner.read().await;
    inner
        .databases
        .get(&id)
        .cloned()
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateDatabaseRequest {
    name: String,
    #[serde(rename = "type")]
    db_type: DatabaseType,
    version: String,
    network_id: String,
    storage_size_gb: u32,
    username: String,
    #[allow(dead_code)]
    password: String,
}

pub async fn create_database(
    State(state): State<AppState>,
    Json(req): Json<CreateDatabaseRequest>,
) -> Result<Json<Database>, StatusCode> {
    let mut inner = state.inner.write().await;

    let db = Database {
        id: Uuid::new_v4().to_string(),
        name: req.name.clone(),
        state: DatabaseState::Creating,
        db_type: req.db_type,
        version: req.version,
        network_id: req.network_id,
        host: None,
        port: None,
        username: req.username,
        storage_size_gb: req.storage_size_gb,
        used_storage_gb: 0,
        connections: 0,
        max_connections: 100,
        created_at: chrono::Utc::now().to_rfc3339(),
        started_at: None,
        error_message: None,
    };

    let log_entry = LogEntry {
        id: Uuid::new_v4().to_string(),
        timestamp_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
        message: format!("Database '{}' created", req.name),
        level: LogLevel::Audit,
        component: "database".to_string(),
        related_object_ids: vec![db.id.clone()],
    };
    inner.logs.push(log_entry.clone());
    let _ = state.log_events_tx.send(log_entry);

    let db_id = db.id.clone();
    inner.databases.insert(db.id.clone(), db.clone());

    // Simulate creation delay then transition to Running
    let state_clone = state.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(3)).await;
        let mut inner = state_clone.inner.write().await;
        if let Some(db) = inner.databases.get_mut(&db_id) {
            db.state = DatabaseState::Running;
            db.host = Some(format!("10.0.2.{}", rand::random::<u8>()));
            db.port = Some(match db.db_type {
                DatabaseType::Postgresql => 5432,
                DatabaseType::Redis => 6379,
            });
            db.started_at = Some(chrono::Utc::now().to_rfc3339());

            let log_entry = LogEntry {
                id: Uuid::new_v4().to_string(),
                timestamp_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
                message: format!("Database '{}' started", db.name),
                level: LogLevel::Audit,
                component: "database".to_string(),
                related_object_ids: vec![db_id],
            };
            inner.logs.push(log_entry.clone());
            let _ = state_clone.log_events_tx.send(log_entry);
        }
    });

    Ok(Json(db))
}

pub async fn delete_database(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    let mut inner = state.inner.write().await;

    if let Some(db) = inner.databases.remove(&id) {
        let log_entry = LogEntry {
            id: Uuid::new_v4().to_string(),
            timestamp_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
            message: format!("Database '{}' deleted", db.name),
            level: LogLevel::Audit,
            component: "database".to_string(),
            related_object_ids: vec![id],
        };
        inner.logs.push(log_entry.clone());
        let _ = state.log_events_tx.send(log_entry);

        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

pub async fn start_database(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Database>, StatusCode> {
    let mut inner = state.inner.write().await;

    if let Some(db) = inner.databases.get_mut(&id) {
        if matches!(db.state, DatabaseState::Stopped) {
            db.state = DatabaseState::Creating;
            let db_clone = db.clone();

            let state_clone = state.clone();
            let id_clone = id.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let mut inner = state_clone.inner.write().await;
                if let Some(db) = inner.databases.get_mut(&id_clone) {
                    db.state = DatabaseState::Running;
                    db.host = Some(format!("10.0.2.{}", rand::random::<u8>()));
                    db.port = Some(match db.db_type {
                        DatabaseType::Postgresql => 5432,
                        DatabaseType::Redis => 6379,
                    });
                    db.started_at = Some(chrono::Utc::now().to_rfc3339());

                    let log_entry = LogEntry {
                        id: Uuid::new_v4().to_string(),
                        timestamp_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
                        message: format!("Database '{}' started", db.name),
                        level: LogLevel::Audit,
                        component: "database".to_string(),
                        related_object_ids: vec![id_clone],
                    };
                    inner.logs.push(log_entry.clone());
                    let _ = state_clone.log_events_tx.send(log_entry);
                }
            });

            Ok(Json(db_clone))
        } else {
            Err(StatusCode::CONFLICT)
        }
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

pub async fn stop_database(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Database>, StatusCode> {
    let mut inner = state.inner.write().await;

    if let Some(db) = inner.databases.get_mut(&id) {
        if matches!(db.state, DatabaseState::Running) {
            db.state = DatabaseState::Stopped;
            db.host = None;
            db.port = None;
            db.started_at = None;
            db.connections = 0;
            let db_clone = db.clone();

            let log_entry = LogEntry {
                id: Uuid::new_v4().to_string(),
                timestamp_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
                message: format!("Database '{}' stopped", db.name),
                level: LogLevel::Audit,
                component: "database".to_string(),
                related_object_ids: vec![id],
            };
            inner.logs.push(log_entry.clone());
            let _ = state.log_events_tx.send(log_entry);

            Ok(Json(db_clone))
        } else {
            Err(StatusCode::CONFLICT)
        }
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}
