//! OIDC JWT validation against Zitadel.
//!
//! Validates Bearer tokens by fetching the issuer's JWKS once at startup and
//! refreshing on cache miss. Stashes the decoded claims into the request
//! extensions; handlers pull them out via the [`AuthenticatedUser`] extractor.
//!
//! Disabled when `MVIRT_OIDC_ISSUER` is unset — useful for local dev and the
//! existing integration tests, which talk to the REST API without tokens.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use axum::{
    Extension,
    extract::{FromRequestParts, Request, State},
    http::{StatusCode, header::AUTHORIZATION, request::Parts},
    middleware::Next,
    response::{IntoResponse, Response},
};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthClaims {
    pub sub: String,
    pub iss: String,
    pub exp: i64,
    #[serde(default)]
    pub iat: Option<i64>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub preferred_username: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenIdConfig {
    jwks_uri: String,
}

#[derive(Debug, Deserialize)]
struct JwksDoc {
    keys: Vec<JwkEntry>,
}

#[derive(Debug, Deserialize)]
struct JwkEntry {
    kid: String,
    kty: String,
    n: Option<String>,
    e: Option<String>,
}

pub struct JwtValidator {
    issuer: String,
    audience: Option<String>,
    jwks_uri: String,
    keys: RwLock<HashMap<String, DecodingKey>>,
    http: reqwest::Client,
}

impl JwtValidator {
    pub async fn from_env() -> Result<Option<Arc<Self>>> {
        let Ok(issuer) = std::env::var("MVIRT_OIDC_ISSUER") else {
            warn!("MVIRT_OIDC_ISSUER unset — REST API auth disabled");
            return Ok(None);
        };
        let audience = std::env::var("MVIRT_OIDC_AUDIENCE").ok();
        Ok(Some(Arc::new(Self::new(issuer, audience).await?)))
    }

    pub async fn new(issuer: String, audience: Option<String>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()?;
        let discovery_url = format!(
            "{}/.well-known/openid-configuration",
            issuer.trim_end_matches('/')
        );
        let cfg: OpenIdConfig = http
            .get(&discovery_url)
            .send()
            .await
            .with_context(|| format!("fetch {discovery_url}"))?
            .error_for_status()?
            .json()
            .await?;
        let validator = Self {
            issuer,
            audience,
            jwks_uri: cfg.jwks_uri,
            keys: RwLock::new(HashMap::new()),
            http,
        };
        validator.refresh_keys().await?;
        info!(issuer = %validator.issuer, "JWT validator ready");
        Ok(validator)
    }

    async fn refresh_keys(&self) -> Result<()> {
        let doc: JwksDoc = self
            .http
            .get(&self.jwks_uri)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let mut new_keys = HashMap::new();
        for jwk in doc.keys {
            if jwk.kty != "RSA" {
                continue;
            }
            let (Some(n), Some(e)) = (jwk.n.as_deref(), jwk.e.as_deref()) else {
                continue;
            };
            let key = DecodingKey::from_rsa_components(n, e)
                .with_context(|| format!("decode rsa jwk kid={}", jwk.kid))?;
            new_keys.insert(jwk.kid, key);
        }
        if new_keys.is_empty() {
            return Err(anyhow!(
                "JWKS at {} contained no usable keys",
                self.jwks_uri
            ));
        }
        *self.keys.write().await = new_keys;
        Ok(())
    }

    pub async fn validate(&self, token: &str) -> Result<AuthClaims> {
        let header = decode_header(token).context("malformed JWT header")?;
        let kid = header.kid.context("JWT missing 'kid'")?;
        let alg = header.alg;
        if !matches!(alg, Algorithm::RS256 | Algorithm::RS384 | Algorithm::RS512) {
            return Err(anyhow!("unsupported JWT alg {:?}", alg));
        }

        // Hot-path lookup; on miss, refresh JWKS once (key rotation).
        let key = match self.keys.read().await.get(&kid).cloned() {
            Some(k) => k,
            None => {
                self.refresh_keys().await?;
                self.keys
                    .read()
                    .await
                    .get(&kid)
                    .cloned()
                    .ok_or_else(|| anyhow!("unknown JWT kid {kid}"))?
            }
        };

        let mut v = Validation::new(alg);
        v.set_issuer(&[&self.issuer]);
        if let Some(aud) = &self.audience {
            v.set_audience(&[aud]);
        } else {
            // Without an explicit audience, accept any — Zitadel embeds the
            // SPA client_id by default and we don't enforce it yet.
            v.validate_aud = false;
        }
        let data = decode::<AuthClaims>(token, &key, &v).context("JWT validation failed")?;
        Ok(data.claims)
    }
}

/// Authenticated caller context. Attached to request extensions by
/// [`require_auth`] after JWT validation, Account lazy-creation, and
/// membership lookup. Handlers extract it via [`AuthenticatedUser`].
#[derive(Debug, Clone)]
pub struct AuthContext {
    pub claims: AuthClaims,
    pub account: crate::command::AccountData,
    pub memberships: Vec<crate::command::MembershipData>,
}

impl AuthContext {
    /// True if the caller has Platform/PlatformAdmin.
    pub fn is_platform_admin(&self) -> bool {
        use crate::command::{MembershipScope, Role};
        self.memberships
            .iter()
            .any(|m| m.scope == MembershipScope::Platform && m.role == Role::PlatformAdmin)
    }

    /// True if the caller has Org/OrgAdmin for `org_slug` (or platform-admin).
    pub fn is_org_admin(&self, org_slug: &str) -> bool {
        use crate::command::{MembershipScope, Role};
        if self.is_platform_admin() {
            return true;
        }
        self.memberships.iter().any(|m| {
            m.role == Role::OrgAdmin
                && matches!(&m.scope, MembershipScope::Org { org_slug: s } if s == org_slug)
        })
    }

    /// True if the caller has Project/ProjectAdmin for `project_slug` (or
    /// platform-admin / org-admin of the project's parent org).
    pub fn is_project_admin(&self, project_slug: &str, project_org_slug: Option<&str>) -> bool {
        use crate::command::{MembershipScope, Role};
        if self.is_platform_admin() {
            return true;
        }
        if let Some(org) = project_org_slug {
            if self.is_org_admin(org) {
                return true;
            }
        }
        self.memberships.iter().any(|m| {
            m.role == Role::ProjectAdmin
                && matches!(&m.scope, MembershipScope::Project { project_slug: s } if s == project_slug)
        })
    }
}

/// Axum middleware: validates the Bearer token, lazy-creates the Account
/// from the OIDC `(iss, sub)`, attaches the `AuthContext` (claims + account
/// + memberships) to request extensions. Initial-admin bootstrap fires
/// here too: the first verified login matching `MVIRT_INITIAL_ADMIN_EMAIL`
/// receives Platform/PlatformAdmin atomically (race-safe — the apply
/// handler refuses if any platform-admin already exists).
pub async fn require_auth(
    State(state): State<Arc<crate::rest::AppState>>,
    mut req: Request,
    next: Next,
) -> Response {
    let token = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "));
    let Some(token) = token else {
        return (StatusCode::UNAUTHORIZED, "missing bearer token").into_response();
    };

    let Some(validator) = state.jwt_validator.as_ref() else {
        // Auth was wired up but the validator is missing — defensive: this
        // codepath shouldn't be reachable because routes.rs only mounts the
        // middleware when a validator exists.
        return (StatusCode::SERVICE_UNAVAILABLE, "auth misconfigured").into_response();
    };
    let claims = match validator.validate(token).await {
        Ok(c) => c,
        Err(err) => {
            warn!(?err, "JWT validation failed");
            return (StatusCode::UNAUTHORIZED, "invalid token").into_response();
        }
    };

    // Lazy-create / refresh the Account row.
    let ensure = crate::store::EnsureAccountRequest {
        iss: claims.iss.clone(),
        sub: claims.sub.clone(),
        email: claims.email.clone(),
        display_name: claims
            .name
            .clone()
            .or_else(|| claims.preferred_username.clone()),
    };
    let account = match state.store.ensure_account_from_oidc(ensure).await {
        Ok(a) => a,
        Err(e) => {
            warn!(error = %e, "ensure_account_from_oidc failed");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "account provisioning failed",
            )
                .into_response();
        }
    };

    // Initial-admin bootstrap: fires only on email-match + verified email +
    // no existing platform-admin. The atomic check lives in the apply
    // handler so parallel logins can't race.
    if let Some(target_email) = state.initial_admin_email.as_deref() {
        if account.email.as_deref() == Some(target_email) {
            if let Err(e) = state
                .store
                .bootstrap_initial_platform_admin(&account.id)
                .await
            {
                warn!(error = %e, "initial admin bootstrap failed");
            }
        }
    }

    let memberships = state
        .store
        .list_memberships_for_account(&account.id)
        .await
        .unwrap_or_default();

    let ctx = AuthContext {
        claims: claims.clone(),
        account,
        memberships,
    };
    req.extensions_mut().insert(claims);
    req.extensions_mut().insert(ctx);
    next.run(req).await
}

/// Extractor that pulls [`AuthClaims`] out of the request extensions. Only
/// available on routes wrapped by [`require_auth`].
pub struct AuthenticatedUser(pub AuthClaims);

impl<S: Send + Sync> FromRequestParts<S> for AuthenticatedUser {
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Extension::<AuthClaims>::from_request_parts(parts, _state)
            .await
            .map(|Extension(c)| Self(c))
            .map_err(|_| (StatusCode::UNAUTHORIZED, "no auth claims in request"))
    }
}

/// Extractor for the full [`AuthContext`] (account + memberships). Use this
/// instead of [`AuthenticatedUser`] when the handler needs to make authz
/// decisions. Also usable as `Option<AuthenticatedAccount>` on routes that
/// run with auth disabled in dev — the option is `None` then.
pub struct AuthenticatedAccount(pub AuthContext);

impl<S: Send + Sync> FromRequestParts<S> for AuthenticatedAccount {
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Extension::<AuthContext>::from_request_parts(parts, _state)
            .await
            .map(|Extension(c)| Self(c))
            .map_err(|_| (StatusCode::UNAUTHORIZED, "no auth context in request"))
    }
}

impl<S: Send + Sync> axum::extract::OptionalFromRequestParts<S> for AuthenticatedAccount {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> Result<Option<Self>, Self::Rejection> {
        Ok(parts
            .extensions
            .get::<AuthContext>()
            .cloned()
            .map(AuthenticatedAccount))
    }
}
