use axum::{
    extract::{Query, State},
    response::sse::{Event, Sse},
    Json,
};
use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use std::{convert::Infallible, time::Duration};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use crate::state::{AppState, LogEntry, LogLevel};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogQueryParams {
    #[serde(alias = "projectId")]
    project_id: Option<String>,
    object_id: Option<String>,
    level: Option<String>,
    component: Option<String>,
    limit: Option<usize>,
    #[allow(dead_code)]
    before_id: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogListResponse {
    entries: Vec<LogEntry>,
}

pub async fn query_logs(
    State(state): State<AppState>,
    Query(params): Query<LogQueryParams>,
) -> Json<LogListResponse> {
    let inner = state.inner.read().await;

    let mut entries: Vec<LogEntry> = inner
        .logs
        .iter()
        .filter(|log| {
            // Filter by project_id
            if let Some(ref pid) = params.project_id {
                if &log.project_id != pid {
                    return false;
                }
            }

            // Filter by object_id
            if let Some(ref obj_id) = params.object_id {
                if !log.related_object_ids.contains(obj_id) {
                    return false;
                }
            }

            // Filter by level
            if let Some(ref level_str) = params.level {
                let level = match level_str.to_uppercase().as_str() {
                    "DEBUG" => LogLevel::Debug,
                    "INFO" => LogLevel::Info,
                    "WARN" => LogLevel::Warn,
                    "ERROR" => LogLevel::Error,
                    "AUDIT" => LogLevel::Audit,
                    _ => return true,
                };
                if !matches!(
                    (&log.level, &level),
                    (LogLevel::Debug, LogLevel::Debug)
                        | (LogLevel::Info, LogLevel::Info)
                        | (LogLevel::Warn, LogLevel::Warn)
                        | (LogLevel::Error, LogLevel::Error)
                        | (LogLevel::Audit, LogLevel::Audit)
                ) {
                    return false;
                }
            }

            // Filter by component
            if let Some(ref comp) = params.component {
                if &log.component != comp {
                    return false;
                }
            }

            true
        })
        .cloned()
        .collect();

    // Sort by timestamp descending (most recent first)
    entries.sort_by(|a, b| b.timestamp_ns.cmp(&a.timestamp_ns));

    // Apply limit
    let limit = params.limit.unwrap_or(100);
    entries.truncate(limit);

    Json(LogListResponse { entries })
}

pub async fn log_stream(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.log_events_tx.subscribe();
    let stream = BroadcastStream::new(rx)
        .filter_map(|result| result.ok())
        .map(|entry| Ok(Event::default().data(serde_json::to_string(&entry).unwrap_or_default())));

    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    )
}
