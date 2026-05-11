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
    CreateClusterRequest as StoreCreateClusterRequest,
    CreateMembershipRequest as StoreCreateMembershipRequest,
    CreateNetworkRequest as StoreCreateNetworkRequest, CreateNicRequest as StoreCreateNicRequest,
    CreateOnboardingTokenRequest as StoreCreateOnboardingTokenRequest,
    CreateOrgRequest as StoreCreateOrgRequest, CreateProjectRequest as StoreCreateProjectRequest,
    CreateSnapshotRequest as StoreCreateSnapshotRequest,
    CreateTemplateRequest as StoreCreateTemplateRequest, CreateVmRequest as StoreCreateVmRequest,
    CreateVolumeRequest as StoreCreateVolumeRequest,
    RedeemOnboardingTokenRequest as StoreRedeemOnboardingTokenRequest,
    ResizeVolumeRequest as StoreResizeVolumeRequest,
    UpdateClusterRequest as StoreUpdateClusterRequest, UpdateOrgRequest as StoreUpdateOrgRequest,
    UpdateVmSpecRequest as StoreUpdateVmSpecRequest,
    UpdateVmStatusRequest as StoreUpdateVmStatusRequest,
};

// =============================================================================
// Org Handlers
// =============================================================================

/// List all Orgs
#[utoipa::path(
    get,
    path = "/v1/orgs",
    responses((status = 200, body = OrgListResponse)),
    tag = "orgs"
)]
pub async fn list_orgs(
    State(state): State<Arc<AppState>>,
) -> Result<Json<OrgListResponse>, ApiError> {
    let orgs = state.store.list_orgs().await?;
    Ok(Json(OrgListResponse {
        orgs: orgs.into_iter().map(UiOrg::from).collect(),
    }))
}

/// Get an Org by slug
#[utoipa::path(
    get,
    path = "/v1/orgs/{slug}",
    params(("slug" = String, Path)),
    responses((status = 200, body = UiOrg), (status = 404, body = ApiError)),
    tag = "orgs"
)]
pub async fn get_org(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> Result<Json<UiOrg>, ApiError> {
    state
        .store
        .get_org(&slug)
        .await?
        .map(|o| Json(UiOrg::from(o)))
        .ok_or_else(|| ApiError {
            error: format!("Org '{}' not found", slug),
            code: 404,
        })
}

/// Create a new Org
#[utoipa::path(
    post,
    path = "/v1/orgs",
    request_body = UiCreateOrgRequest,
    responses(
        (status = 200, body = UiOrg),
        (status = 400, body = ApiError),
        (status = 409, body = ApiError)
    ),
    tag = "orgs"
)]
pub async fn create_org(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UiCreateOrgRequest>,
) -> Result<Json<UiOrg>, ApiError> {
    validate_slug(&req.slug, "Org slug")?;
    let store_req = StoreCreateOrgRequest {
        slug: req.slug,
        name: req.name,
        default_static_key_ttl_days: req.default_static_key_ttl_days,
        disallow_static_keys: req.disallow_static_keys,
    };
    let data = state.store.create_org(store_req).await?;
    Ok(Json(UiOrg::from(data)))
}

/// Update an Org
#[utoipa::path(
    patch,
    path = "/v1/orgs/{slug}",
    params(("slug" = String, Path)),
    request_body = UiUpdateOrgRequest,
    responses((status = 200, body = UiOrg), (status = 404, body = ApiError)),
    tag = "orgs"
)]
pub async fn update_org(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
    Json(req): Json<UiUpdateOrgRequest>,
) -> Result<Json<UiOrg>, ApiError> {
    let org = state.store.get_org(&slug).await?.ok_or_else(|| ApiError {
        error: format!("Org '{}' not found", slug),
        code: 404,
    })?;
    let store_req = StoreUpdateOrgRequest {
        name: req.name,
        default_static_key_ttl_days: req.default_static_key_ttl_days,
        disallow_static_keys: req.disallow_static_keys,
        contact: req.contact.map(Into::into),
    };
    let data = state.store.update_org(&org.slug, store_req).await?;
    Ok(Json(UiOrg::from(data)))
}

/// Delete an Org. Rejects with 409 if the Org still has Projects.
#[utoipa::path(
    delete,
    path = "/v1/orgs/{slug}",
    params(("slug" = String, Path)),
    responses(
        (status = 204),
        (status = 404, body = ApiError),
        (status = 409, body = ApiError)
    ),
    tag = "orgs"
)]
pub async fn delete_org(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> Result<StatusCode, ApiError> {
    let org = state.store.get_org(&slug).await?.ok_or_else(|| ApiError {
        error: format!("Org '{}' not found", slug),
        code: 404,
    })?;
    state.store.delete_org(&org.slug).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// List Projects within an Org
#[utoipa::path(
    get,
    path = "/v1/orgs/{org_slug}/projects",
    params(("org_slug" = String, Path)),
    responses((status = 200, body = ProjectListResponse), (status = 404, body = ApiError)),
    tag = "projects"
)]
pub async fn list_projects_in_org(
    State(state): State<Arc<AppState>>,
    Path(org_slug): Path<String>,
) -> Result<Json<ProjectListResponse>, ApiError> {
    let org = state
        .store
        .get_org(&org_slug)
        .await?
        .ok_or_else(|| ApiError {
            error: format!("Org '{}' not found", org_slug),
            code: 404,
        })?;
    let projects = state.store.list_projects_by_org(&org.slug).await?;
    Ok(Json(ProjectListResponse {
        projects: projects.into_iter().map(UiProject::from).collect(),
    }))
}

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
    let project = state.store.get_project(&id).await?;

    match project {
        Some(data) => Ok(Json(UiProject::from(data))),
        None => Err(ApiError {
            error: "Project not found".to_string(),
            code: 404,
        }),
    }
}

/// Create a new project under the named Org.
#[utoipa::path(
    post,
    path = "/v1/orgs/{org_slug}/projects",
    params(("org_slug" = String, Path)),
    request_body = UiCreateProjectRequest,
    responses(
        (status = 200, body = UiProject),
        (status = 400, body = ApiError),
        (status = 404, body = ApiError),
        (status = 409, body = ApiError)
    ),
    tag = "projects"
)]
pub async fn create_project(
    State(state): State<Arc<AppState>>,
    Path(org_slug): Path<String>,
    Json(req): Json<UiCreateProjectRequest>,
) -> Result<Json<UiProject>, ApiError> {
    validate_slug(&req.slug, "Project slug")?;

    let org = state
        .store
        .get_org(&org_slug)
        .await?
        .ok_or_else(|| ApiError {
            error: format!("Org '{}' not found", org_slug),
            code: 404,
        })?;

    let store_req = StoreCreateProjectRequest {
        org_slug: org.slug,
        slug: req.slug,
        name: req.name,
        description: req.description,
    };

    let data = state.store.create_project(store_req).await?;
    state.audit.project_created(&data.slug, &data.name);
    Ok(Json(UiProject::from(data)))
}

fn validate_slug(slug: &str, field: &str) -> Result<(), ApiError> {
    if slug.is_empty() {
        return Err(ApiError {
            error: format!("{} cannot be empty", field),
            code: 400,
        });
    }
    if slug.len() > 63 {
        return Err(ApiError {
            error: format!("{} must be 63 characters or fewer", field),
            code: 400,
        });
    }
    let valid = slug
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !slug.starts_with('-')
        && !slug.ends_with('-');
    if !valid {
        return Err(ApiError {
            error: format!(
                "{} must be kebab-case (lowercase letters, digits, hyphens; no leading/trailing hyphen)",
                field
            ),
            code: 400,
        });
    }
    Ok(())
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
// Cluster Handlers (ADR-0005)
// =============================================================================

/// List Clusters within an Org.
#[utoipa::path(
    get,
    path = "/v1/orgs/{org_slug}/clusters",
    params(("org_slug" = String, Path)),
    responses((status = 200, body = ClusterListResponse), (status = 404, body = ApiError)),
    tag = "clusters"
)]
pub async fn list_clusters_in_org(
    State(state): State<Arc<AppState>>,
    Path(org_slug): Path<String>,
) -> Result<Json<ClusterListResponse>, ApiError> {
    let org = state
        .store
        .get_org(&org_slug)
        .await?
        .ok_or_else(|| ApiError {
            error: format!("Org '{}' not found", org_slug),
            code: 404,
        })?;
    let clusters = state.store.list_clusters_by_org(&org.slug).await?;
    Ok(Json(ClusterListResponse {
        clusters: clusters.into_iter().map(UiCluster::from).collect(),
    }))
}

/// List all Clusters across all Orgs.
#[utoipa::path(
    get,
    path = "/v1/clusters",
    responses((status = 200, body = ClusterListResponse)),
    tag = "clusters"
)]
pub async fn list_clusters(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ClusterListResponse>, ApiError> {
    let clusters = state.store.list_clusters().await?;
    Ok(Json(ClusterListResponse {
        clusters: clusters.into_iter().map(UiCluster::from).collect(),
    }))
}

/// Get a Cluster by slug.
#[utoipa::path(
    get,
    path = "/v1/clusters/{slug}",
    params(("slug" = String, Path)),
    responses((status = 200, body = UiCluster), (status = 404, body = ApiError)),
    tag = "clusters"
)]
pub async fn get_cluster(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> Result<Json<UiCluster>, ApiError> {
    state
        .store
        .get_cluster(&slug)
        .await?
        .map(|c| Json(UiCluster::from(c)))
        .ok_or_else(|| ApiError {
            error: format!("Cluster '{}' not found", slug),
            code: 404,
        })
}

/// Create a new Cluster under the named Org.
#[utoipa::path(
    post,
    path = "/v1/orgs/{org_slug}/clusters",
    params(("org_slug" = String, Path)),
    request_body = UiCreateClusterRequest,
    responses(
        (status = 200, body = UiCluster),
        (status = 400, body = ApiError),
        (status = 404, body = ApiError),
        (status = 409, body = ApiError)
    ),
    tag = "clusters"
)]
pub async fn create_cluster(
    State(state): State<Arc<AppState>>,
    Path(org_slug): Path<String>,
    Json(req): Json<UiCreateClusterRequest>,
) -> Result<Json<UiCluster>, ApiError> {
    validate_slug(&req.slug, "Cluster slug")?;

    let org = state
        .store
        .get_org(&org_slug)
        .await?
        .ok_or_else(|| ApiError {
            error: format!("Org '{}' not found", org_slug),
            code: 404,
        })?;

    let store_req = StoreCreateClusterRequest {
        org_slug: org.slug,
        slug: req.slug,
        name: req.name,
        description: req.description,
        location: req.location,
    };
    let data = state.store.create_cluster(store_req).await?;
    Ok(Json(UiCluster::from(data)))
}

/// Update a Cluster.
#[utoipa::path(
    patch,
    path = "/v1/clusters/{slug}",
    params(("slug" = String, Path)),
    request_body = UiUpdateClusterRequest,
    responses((status = 200, body = UiCluster), (status = 404, body = ApiError)),
    tag = "clusters"
)]
pub async fn update_cluster(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
    Json(req): Json<UiUpdateClusterRequest>,
) -> Result<Json<UiCluster>, ApiError> {
    let store_req = StoreUpdateClusterRequest {
        name: req.name,
        description: req.description,
        location: req.location,
    };
    let data = state.store.update_cluster(&slug, store_req).await?;
    Ok(Json(UiCluster::from(data)))
}

/// Delete a Cluster. Rejects with 409 if any resource references it.
#[utoipa::path(
    delete,
    path = "/v1/clusters/{slug}",
    params(("slug" = String, Path)),
    responses(
        (status = 204),
        (status = 404, body = ApiError),
        (status = 409, body = ApiError)
    ),
    tag = "clusters"
)]
pub async fn delete_cluster(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.store.delete_cluster(&slug).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Add a Node to a Cluster's `node_ids`. Idempotent.
#[utoipa::path(
    post,
    path = "/v1/clusters/{slug}/nodes/{node_id}",
    params(("slug" = String, Path), ("node_id" = String, Path)),
    responses(
        (status = 200, body = UiCluster),
        (status = 404, body = ApiError)
    ),
    tag = "clusters"
)]
pub async fn add_node_to_cluster(
    State(state): State<Arc<AppState>>,
    Path((slug, node_id)): Path<(String, String)>,
) -> Result<Json<UiCluster>, ApiError> {
    let data = state.store.add_node_to_cluster(&slug, &node_id).await?;
    Ok(Json(UiCluster::from(data)))
}

/// Remove a Node from a Cluster's `node_ids`. Idempotent.
#[utoipa::path(
    delete,
    path = "/v1/clusters/{slug}/nodes/{node_id}",
    params(("slug" = String, Path), ("node_id" = String, Path)),
    responses(
        (status = 200, body = UiCluster),
        (status = 404, body = ApiError)
    ),
    tag = "clusters"
)]
pub async fn remove_node_from_cluster(
    State(state): State<Arc<AppState>>,
    Path((slug, node_id)): Path<(String, String)>,
) -> Result<Json<UiCluster>, ApiError> {
    let data = state
        .store
        .remove_node_from_cluster(&slug, &node_id)
        .await?;
    Ok(Json(UiCluster::from(data)))
}

// =============================================================================
// Node onboarding handlers (ADR-0006)
// =============================================================================

/// Default onboarding-token TTL — 6 hours. Long enough that an operator
/// can copy the token to the node, install the binary, and start the
/// service without rushing. ADR-0006 caps at 7 days.
const DEFAULT_ONBOARDING_TOKEN_TTL_SECONDS: u64 = 6 * 3600;

/// List onboarding tokens issued for a given Cluster.
#[utoipa::path(
    get,
    path = "/v1/clusters/{slug}/onboarding-tokens",
    params(("slug" = String, Path)),
    responses(
        (status = 200, body = OnboardingTokenListResponse),
        (status = 404, body = ApiError)
    ),
    tag = "clusters"
)]
pub async fn list_onboarding_tokens_in_cluster(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> Result<Json<OnboardingTokenListResponse>, ApiError> {
    if state.store.get_cluster(&slug).await?.is_none() {
        return Err(ApiError {
            error: format!("Cluster '{}' not found", slug),
            code: 404,
        });
    }
    let tokens = state.store.list_onboarding_tokens_by_cluster(&slug).await?;
    Ok(Json(OnboardingTokenListResponse {
        tokens: tokens.into_iter().map(UiOnboardingToken::from).collect(),
    }))
}

/// Mint a new onboarding token bound to a Cluster. The bare token is in the
/// response body — only chance the operator has to see it.
#[utoipa::path(
    post,
    path = "/v1/clusters/{slug}/onboarding-tokens",
    params(("slug" = String, Path)),
    request_body = UiCreateOnboardingTokenRequest,
    responses(
        (status = 200, body = UiCreateOnboardingTokenResponse),
        (status = 404, body = ApiError)
    ),
    tag = "clusters"
)]
pub async fn create_onboarding_token(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
    Json(req): Json<UiCreateOnboardingTokenRequest>,
) -> Result<Json<UiCreateOnboardingTokenResponse>, ApiError> {
    if state.store.get_cluster(&slug).await?.is_none() {
        return Err(ApiError {
            error: format!("Cluster '{}' not found", slug),
            code: 404,
        });
    }
    let hostname = req.hostname.trim().to_string();
    if hostname.is_empty() {
        return Err(ApiError {
            error: "hostname is required".to_string(),
            code: 400,
        });
    }
    let store_req = StoreCreateOnboardingTokenRequest {
        cluster_slug: slug,
        hostname,
        description: req.description,
        ttl_seconds: req
            .ttl_seconds
            .unwrap_or(DEFAULT_ONBOARDING_TOKEN_TTL_SECONDS),
        // No Account auth in v1 (ADR-0004 deferred). Stamp a placeholder so
        // audit carries something — real Account lands with the auth layer.
        created_by_account: "operator".to_string(),
    };
    let (record, bare) = state.store.create_onboarding_token(store_req).await?;
    Ok(Json(UiCreateOnboardingTokenResponse {
        id: record.id,
        token: bare,
        cluster_slug: record.cluster_slug,
        node_id: record.node_id,
        hostname: record.hostname,
        expires_at: record.expires_at,
        description: record.description,
    }))
}

/// Revoke an unredeemed onboarding token.
#[utoipa::path(
    delete,
    path = "/v1/clusters/{slug}/onboarding-tokens/{id}",
    params(("slug" = String, Path), ("id" = String, Path)),
    responses(
        (status = 204),
        (status = 404, body = ApiError)
    ),
    tag = "clusters"
)]
pub async fn delete_onboarding_token(
    State(state): State<Arc<AppState>>,
    Path((slug, id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    state.store.delete_onboarding_token(&slug, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Bootstrap endpoint. Token-authed via `Authorization: Bearer ...`; not
/// gated by the regular Account-session middleware. ADR-0006.
#[utoipa::path(
    post,
    path = "/v1/bootstrap/onboarding",
    request_body = UiBootstrapRequest,
    responses(
        (status = 200, body = UiBootstrapResponse),
        (status = 400, body = ApiError),
        (status = 401, body = ApiError),
        (status = 410, body = ApiError)
    ),
    tag = "bootstrap"
)]
pub async fn bootstrap_onboarding(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<UiBootstrapRequest>,
) -> Result<Json<UiBootstrapResponse>, ApiError> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(|s| s.trim().to_string())
        .ok_or_else(|| ApiError {
            error: "missing Authorization: Bearer header".into(),
            code: 401,
        })?;

    // Lazy CA bootstrap so the very first redeem in a fresh deployment
    // doesn't fail with 503. After the first call the CA is cached in raft.
    state.store.ensure_internal_ca("mvirt").await?;

    let outcome = state
        .store
        .redeem_onboarding_token(StoreRedeemOnboardingTokenRequest {
            token,
            csr_pem: req.csr_pem,
            hostname: req.hostname,
            agent_version: req.agent_version,
            kernel_version: req.kernel_version,
            arch: req.arch,
        })
        .await?;

    Ok(Json(UiBootstrapResponse {
        node_id: outcome.node_id,
        cluster_slug: outcome.cluster_slug,
        client_cert_pem: outcome.client_cert_pem,
        ca_cert_pem: outcome.ca_cert_pem,
        cert_not_after: outcome.cert_not_after,
    }))
}

// =============================================================================
// Account + Membership handlers (ADR-0004)
// =============================================================================

/// Current user — Account + memberships. Driven by the AuthContext that the
/// auth middleware attached after JWT validation. Returns 401 when auth is
/// disabled (dev mode) — the UI's localStorage stub kicks in instead.
#[utoipa::path(
    get,
    path = "/v1/me",
    responses(
        (status = 200, body = UiMe),
        (status = 401, body = ApiError)
    ),
    tag = "auth"
)]
pub async fn get_me(
    crate::auth::AuthenticatedAccount(ctx): crate::auth::AuthenticatedAccount,
) -> Result<Json<UiMe>, ApiError> {
    let is_platform_admin = ctx.is_platform_admin();
    Ok(Json(UiMe {
        account: UiAccount::from(ctx.account),
        memberships: ctx
            .memberships
            .into_iter()
            .map(UiMembership::from)
            .collect(),
        is_platform_admin,
    }))
}

/// List all Accounts. Platform-admin only — cross-Org visibility.
#[utoipa::path(
    get,
    path = "/v1/accounts",
    responses(
        (status = 200, body = Vec<UiAccount>),
        (status = 403, body = ApiError)
    ),
    tag = "auth"
)]
pub async fn list_accounts(
    State(state): State<Arc<AppState>>,
    auth: Option<crate::auth::AuthenticatedAccount>,
) -> Result<Json<Vec<UiAccount>>, ApiError> {
    if state.jwt_validator.is_some() {
        let ctx = auth.ok_or_else(|| ApiError {
            error: "missing auth".into(),
            code: 401,
        })?;
        if !ctx.0.is_platform_admin() {
            return Err(ApiError {
                error: "platform-admin required".into(),
                code: 403,
            });
        }
    }
    let accounts = state.store.list_accounts().await?;
    Ok(Json(accounts.into_iter().map(UiAccount::from).collect()))
}

/// List members of an Org. Org-admin or platform-admin only.
#[utoipa::path(
    get,
    path = "/v1/orgs/{org_slug}/members",
    params(("org_slug" = String, Path)),
    responses(
        (status = 200, body = MembershipListResponse),
        (status = 403, body = ApiError),
        (status = 404, body = ApiError)
    ),
    tag = "orgs"
)]
pub async fn list_org_members(
    State(state): State<Arc<AppState>>,
    Path(org_slug): Path<String>,
    auth: Option<crate::auth::AuthenticatedAccount>,
) -> Result<Json<MembershipListResponse>, ApiError> {
    if state.jwt_validator.is_some() {
        let ctx = auth.ok_or_else(|| ApiError {
            error: "missing auth".into(),
            code: 401,
        })?;
        if !ctx.0.is_org_admin(&org_slug) {
            return Err(ApiError {
                error: "org-admin required".into(),
                code: 403,
            });
        }
    }
    if state.store.get_org(&org_slug).await?.is_none() {
        return Err(ApiError {
            error: format!("Org '{}' not found", org_slug),
            code: 404,
        });
    }
    let scope = crate::command::MembershipScope::Org {
        org_slug: org_slug.clone(),
    };
    let memberships = state.store.list_memberships_at_scope(&scope).await?;
    Ok(Json(MembershipListResponse {
        memberships: memberships.into_iter().map(UiMembership::from).collect(),
    }))
}

/// Grant a membership in this Org. Org-admin or platform-admin only.
#[utoipa::path(
    post,
    path = "/v1/orgs/{org_slug}/members",
    params(("org_slug" = String, Path)),
    request_body = UiCreateOrgMembershipRequest,
    responses(
        (status = 200, body = UiMembership),
        (status = 403, body = ApiError),
        (status = 404, body = ApiError),
        (status = 409, body = ApiError)
    ),
    tag = "orgs"
)]
pub async fn create_org_member(
    State(state): State<Arc<AppState>>,
    Path(org_slug): Path<String>,
    auth: Option<crate::auth::AuthenticatedAccount>,
    Json(req): Json<UiCreateOrgMembershipRequest>,
) -> Result<Json<UiMembership>, ApiError> {
    let caller_account = if state.jwt_validator.is_some() {
        let ctx = auth.ok_or_else(|| ApiError {
            error: "missing auth".into(),
            code: 401,
        })?;
        if !ctx.0.is_org_admin(&org_slug) {
            return Err(ApiError {
                error: "org-admin required".into(),
                code: 403,
            });
        }
        ctx.0.account.id
    } else {
        // Auth disabled (dev / test) — anonymous caller becomes the
        // bootstrap admin in audit fields.
        "system".to_string()
    };
    let role = match req.role.as_str() {
        "org-admin" => crate::command::Role::OrgAdmin,
        other => {
            return Err(ApiError {
                error: format!("unknown role '{}'; allowed: org-admin", other),
                code: 400,
            });
        }
    };
    let m = state
        .store
        .create_membership(StoreCreateMembershipRequest {
            account_id: req.account_id,
            scope: crate::command::MembershipScope::Org { org_slug },
            role,
            created_by_account: caller_account,
        })
        .await?;
    Ok(Json(UiMembership::from(m)))
}

/// Revoke a membership. Org-admin or platform-admin only.
#[utoipa::path(
    delete,
    path = "/v1/orgs/{org_slug}/members/{id}",
    params(("org_slug" = String, Path), ("id" = String, Path)),
    responses(
        (status = 204),
        (status = 403, body = ApiError),
        (status = 404, body = ApiError)
    ),
    tag = "orgs"
)]
pub async fn delete_org_member(
    State(state): State<Arc<AppState>>,
    Path((org_slug, id)): Path<(String, String)>,
    auth: Option<crate::auth::AuthenticatedAccount>,
) -> Result<StatusCode, ApiError> {
    if state.jwt_validator.is_some() {
        let ctx = auth.ok_or_else(|| ApiError {
            error: "missing auth".into(),
            code: 401,
        })?;
        if !ctx.0.is_org_admin(&org_slug) {
            return Err(ApiError {
                error: "org-admin required".into(),
                code: 403,
            });
        }
    }
    state.store.delete_membership(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Revoke a Node's cert. `decommission` also deletes the Node row (and
/// drops it from every Cluster's `node_ids`).
#[utoipa::path(
    post,
    path = "/v1/nodes/{id}/revoke",
    params(("id" = String, Path)),
    request_body = UiRevokeNodeRequest,
    responses(
        (status = 204),
        (status = 400, body = ApiError),
        (status = 404, body = ApiError)
    ),
    tag = "nodes"
)]
pub async fn revoke_node(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<UiRevokeNodeRequest>,
) -> Result<StatusCode, ApiError> {
    let reason = match req.reason.as_str() {
        "decommission" => crate::command::RevocationReason::Decommission,
        "compromise" => crate::command::RevocationReason::Compromise,
        "other" => crate::command::RevocationReason::Other,
        other => {
            return Err(ApiError {
                error: format!(
                    "unknown reason '{}'; allowed: decommission, compromise, other",
                    other
                ),
                code: 400,
            });
        }
    };
    state.store.revoke_node_cert(&id, reason).await?;
    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// VM Handlers
// =============================================================================

/// List VMs in a project
#[utoipa::path(get, path = "/v1/projects/{project_slug}/vms", params(("project_slug" = String, Path), ("node_id" = Option<String>, Query)), responses((status = 200, body = VmListResponse)), tag = "vms")]
pub async fn list_vms(
    State(state): State<Arc<AppState>>,
    Path(project_slug): Path<String>,
    Query(query): Query<ListVmsQuery>,
) -> Result<Json<VmListResponse>, ApiError> {
    let vms = state.store.list_vms_by_project(&project_slug).await?;

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
#[utoipa::path(post, path = "/v1/projects/{project_slug}/vms", params(("project_slug" = String, Path)), request_body = UiCreateVmRequest, responses((status = 200, body = UiVm), (status = 503, body = ApiError)), tag = "vms")]
pub async fn create_vm(
    State(state): State<Arc<AppState>>,
    Path(project_slug): Path<String>,
    Json(req): Json<UiCreateVmRequest>,
) -> Result<Json<UiVm>, ApiError> {
    let spec = VmSpec {
        name: req.name.clone(),
        project_slug,
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
    Path(_project_slug): Path<String>,
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
#[utoipa::path(get, path = "/v1/projects/{project_slug}/networks", params(("project_slug" = String, Path)), responses((status = 200, body = NetworkListResponse)), tag = "networks")]
pub async fn list_networks(
    State(state): State<Arc<AppState>>,
    Path(project_slug): Path<String>,
) -> Result<Json<NetworkListResponse>, ApiError> {
    let networks = state.store.list_networks_by_project(&project_slug).await?;
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
#[utoipa::path(post, path = "/v1/projects/{project_slug}/networks", params(("project_slug" = String, Path)), request_body = UiCreateNetworkRequest, responses((status = 200, body = UiNetwork), (status = 409, body = ApiError)), tag = "networks")]
pub async fn create_network(
    State(state): State<Arc<AppState>>,
    Path(project_slug): Path<String>,
    Json(req): Json<UiCreateNetworkRequest>,
) -> Result<Json<UiNetwork>, ApiError> {
    let store_req = StoreCreateNetworkRequest {
        project_slug,
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
#[utoipa::path(get, path = "/v1/projects/{project_slug}/nics", params(("project_slug" = String, Path), ("network_id" = Option<String>, Query)), responses((status = 200, body = NicListResponse)), tag = "nics")]
pub async fn list_nics(
    State(state): State<Arc<AppState>>,
    Path(project_slug): Path<String>,
    Query(query): Query<ListNicsQuery>,
) -> Result<Json<NicListResponse>, ApiError> {
    let nics = state.store.list_nics_by_project(&project_slug).await?;
    // Further filter by network if specified
    let nics: Vec<UiNic> = nics
        .into_iter()
        .filter(|nic| {
            query
                .network_id
                .as_ref()
                .is_none_or(|nid| &nic.spec.network_id == nid)
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
#[utoipa::path(post, path = "/v1/projects/{project_slug}/nics", params(("project_slug" = String, Path)), request_body = UiCreateNicRequest, responses((status = 200, body = UiNic), (status = 404, body = ApiError)), tag = "nics")]
pub async fn create_nic(
    State(state): State<Arc<AppState>>,
    Path(project_slug): Path<String>,
    Json(req): Json<UiCreateNicRequest>,
) -> Result<Json<UiNic>, ApiError> {
    let store_req = StoreCreateNicRequest {
        project_slug,
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
        .nic_created(&data.id, &data.spec.network_id, &data.spec.mac_address);
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
#[utoipa::path(get, path = "/v1/projects/{project_slug}/volumes", params(("project_slug" = String, Path), ("node_id" = Option<String>, Query)), responses((status = 200, body = VolumeListResponse)), tag = "storage")]
pub async fn list_volumes(
    State(state): State<Arc<AppState>>,
    Path(project_slug): Path<String>,
    Query(query): Query<ListVolumesQuery>,
) -> Result<Json<VolumeListResponse>, ApiError> {
    let volumes = state
        .store
        .list_volumes(Some(&project_slug), query.node_id.as_deref())
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
#[utoipa::path(post, path = "/v1/projects/{project_slug}/volumes", params(("project_slug" = String, Path)), request_body = UiCreateVolumeRequest, responses((status = 200, body = UiVolume), (status = 503, body = ApiError)), tag = "storage")]
pub async fn create_volume(
    State(state): State<Arc<AppState>>,
    Path(project_slug): Path<String>,
    Json(req): Json<UiCreateVolumeRequest>,
) -> Result<Json<UiVolume>, ApiError> {
    let store_req = StoreCreateVolumeRequest {
        project_slug,
        node_id: req.node_id,
        name: req.name,
        size_bytes: req.size_bytes,
        template_id: req.template_id,
    };

    let data = state.store.create_volume(store_req).await?;
    state.audit.volume_created(&data.id, &data.spec.name);
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
    state.audit.volume_resized(&data.id, data.spec.size_bytes);
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
#[utoipa::path(get, path = "/v1/projects/{project_slug}/templates", params(("project_slug" = String, Path)), responses((status = 200, body = TemplateListResponse)), tag = "storage")]
pub async fn list_templates(
    State(state): State<Arc<AppState>>,
    Path(project_slug): Path<String>,
) -> Result<Json<TemplateListResponse>, ApiError> {
    let templates = state.store.list_templates_by_project(&project_slug).await?;
    Ok(Json(TemplateListResponse {
        templates: templates.into_iter().map(UiTemplate::from).collect(),
    }))
}

/// Import a template
#[utoipa::path(post, path = "/v1/projects/{project_slug}/templates/import", params(("project_slug" = String, Path)), request_body = UiImportTemplateRequest, responses((status = 200, body = UiImportJob), (status = 503, body = ApiError)), tag = "storage")]
pub async fn import_template(
    State(state): State<Arc<AppState>>,
    Path(project_slug): Path<String>,
    Json(req): Json<UiImportTemplateRequest>,
) -> Result<Json<UiImportJob>, ApiError> {
    let store_req = StoreCreateTemplateRequest {
        project_slug,
        node_id: req.node_id,
        name: req.name,
        size_bytes: 0,
        source_url: Some(req.url),
        total_bytes: req.total_bytes,
    };

    let data = state.store.create_template(store_req).await?;
    state.audit.template_import_started(&data.id);
    Ok(Json(UiImportJob::from(data)))
}

/// Get an import job by ID (global)
#[utoipa::path(get, path = "/v1/import-jobs/{id}", params(("id" = String, Path)), responses((status = 200, body = UiImportJob), (status = 404, body = ApiError)), tag = "storage")]
pub async fn get_import_job(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<UiImportJob>, ApiError> {
    match state.store.get_template(&id).await? {
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
        let total_ratio: f64 = volumes
            .iter()
            .map(|v| v.status.compression_ratio)
            .sum::<f64>();
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
#[utoipa::path(get, path = "/v1/projects/{project_slug}/security-groups", params(("project_slug" = String, Path)), responses((status = 200, body = SecurityGroupListResponse)), tag = "security-groups")]
pub async fn list_security_groups(
    State(state): State<Arc<AppState>>,
    Path(project_slug): Path<String>,
) -> Result<Json<SecurityGroupListResponse>, ApiError> {
    let groups = state
        .store
        .list_security_groups(Some(&project_slug))
        .await?;
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
#[utoipa::path(post, path = "/v1/projects/{project_slug}/security-groups", params(("project_slug" = String, Path)), request_body = UiCreateSecurityGroupRequest, responses((status = 200, body = UiSecurityGroup), (status = 409, body = ApiError)), tag = "security-groups")]
pub async fn create_security_group(
    State(state): State<Arc<AppState>>,
    Path(project_slug): Path<String>,
    Json(req): Json<UiCreateSecurityGroupRequest>,
) -> Result<Json<UiSecurityGroup>, ApiError> {
    use crate::store::CreateSecurityGroupRequest;

    let sg = state
        .store
        .create_security_group(CreateSecurityGroupRequest {
            project_slug,
            name: req.name,
            description: req.description,
        })
        .await?;

    state.audit.security_group_created(&sg.id, &sg.name);

    Ok(Json(UiSecurityGroup::from(sg)))
}

/// Update a security group's mutable fields.
#[utoipa::path(
    patch,
    path = "/v1/security-groups/{id}",
    params(("id" = String, Path)),
    request_body = UiUpdateSecurityGroupRequest,
    responses(
        (status = 200, body = UiSecurityGroup),
        (status = 404, body = ApiError)
    ),
    tag = "security-groups"
)]
pub async fn update_security_group(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<UiUpdateSecurityGroupRequest>,
) -> Result<Json<UiSecurityGroup>, ApiError> {
    use crate::store::UpdateSecurityGroupRequest;

    let sg = state
        .store
        .update_security_group(
            &id,
            UpdateSecurityGroupRequest {
                name: req.name,
                description: req.description,
            },
        )
        .await?;
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

/// Update a rule's mutable fields (currently description only).
#[utoipa::path(
    patch,
    path = "/v1/security-groups/{sg_id}/rules/{rule_id}",
    params(("sg_id" = String, Path), ("rule_id" = String, Path)),
    request_body = UiUpdateSecurityGroupRuleRequest,
    responses(
        (status = 200, body = UiSecurityGroup),
        (status = 404, body = ApiError)
    ),
    tag = "security-groups"
)]
pub async fn update_security_group_rule(
    State(state): State<Arc<AppState>>,
    Path((sg_id, rule_id)): Path<(String, String)>,
    Json(req): Json<UiUpdateSecurityGroupRuleRequest>,
) -> Result<Json<UiSecurityGroup>, ApiError> {
    use crate::store::UpdateSecurityGroupRuleRequest;

    // The wire shape uses a sentinel: send `description: null` to clear the
    // field, omit the key to leave it untouched. Serde turns "key present
    // with null" into `Some(None)` thanks to `default + skip_if_none`.
    let sg = state
        .store
        .update_security_group_rule(
            &sg_id,
            &rule_id,
            UpdateSecurityGroupRuleRequest {
                description: req.description,
            },
        )
        .await?;
    Ok(Json(UiSecurityGroup::from(sg)))
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
    pub project_slug: Option<String>,
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
