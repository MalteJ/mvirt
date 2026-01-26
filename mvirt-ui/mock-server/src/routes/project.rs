use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Serialize;

use crate::state::{AppState, Project};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectListResponse {
    projects: Vec<Project>,
}

pub async fn list_projects(State(state): State<AppState>) -> Json<ProjectListResponse> {
    let inner = state.inner.read().await;
    let projects: Vec<Project> = inner.projects.values().cloned().collect();
    Json(ProjectListResponse { projects })
}

pub async fn get_project(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Project>, StatusCode> {
    let inner = state.inner.read().await;
    inner
        .projects
        .get(&id)
        .cloned()
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}
