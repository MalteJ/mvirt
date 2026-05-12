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
    /// Standard OIDC `name` (full name). Rauthy doesn't emit this — see
    /// `given_name`/`family_name` below — but other IdPs (Zitadel, Keycloak,
    /// Auth0) do, so keep accepting it.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub given_name: Option<String>,
    #[serde(default)]
    pub family_name: Option<String>,
    /// Rauthy defaults `preferred_username` to email. Used as last-resort
    /// fallback for display_name — only after `name` and `given_name`+
    /// `family_name` have been tried.
    #[serde(default)]
    pub preferred_username: Option<String>,
}

impl AuthClaims {
    /// Best display-name from the available claims. Priority:
    ///   1. `name` (standard OIDC full-name)
    ///   2. `given_name` + ` ` + `family_name` (rauthy emits these when set)
    ///   3. `preferred_username` (rauthy defaults to email; better than nothing)
    pub fn best_display_name(&self) -> Option<String> {
        if let Some(n) = self.name.as_ref().filter(|s| !s.trim().is_empty()) {
            return Some(n.trim().to_string());
        }
        let g = self.given_name.as_deref().unwrap_or("").trim();
        let f = self.family_name.as_deref().unwrap_or("").trim();
        if !g.is_empty() || !f.is_empty() {
            let joined = format!("{g} {f}").trim().to_string();
            if !joined.is_empty() {
                return Some(joined);
            }
        }
        self.preferred_username
            .as_ref()
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.trim().to_string())
    }
}

#[derive(Debug, Deserialize)]
struct OpenIdConfig {
    jwks_uri: String,
    userinfo_endpoint: Option<String>,
}

/// Profile claims pulled from the IdP's UserInfo endpoint. Rauthy emits
/// `given_name`, `family_name`, `preferred_username` here but NOT in the
/// access token, so this is the canonical source for display_name on
/// rauthy. Other IdPs may put the same data in the JWT body — handler
/// code merges both paths via `best_display_name()`.
#[derive(Debug, Default, Clone, Deserialize)]
pub struct ProfileClaims {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub given_name: Option<String>,
    #[serde(default)]
    pub family_name: Option<String>,
    #[serde(default)]
    pub preferred_username: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
}

impl ProfileClaims {
    /// Same fallback chain as [`AuthClaims::best_display_name`], applied
    /// to the UserInfo body.
    pub fn best_display_name(&self) -> Option<String> {
        if let Some(n) = self.name.as_ref().filter(|s| !s.trim().is_empty()) {
            return Some(n.trim().to_string());
        }
        let g = self.given_name.as_deref().unwrap_or("").trim();
        let f = self.family_name.as_deref().unwrap_or("").trim();
        if !g.is_empty() || !f.is_empty() {
            let joined = format!("{g} {f}").trim().to_string();
            if !joined.is_empty() {
                return Some(joined);
            }
        }
        self.preferred_username
            .as_ref()
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.trim().to_string())
    }
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
    userinfo_endpoint: Option<String>,
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
            userinfo_endpoint: cfg.userinfo_endpoint,
            keys: RwLock::new(HashMap::new()),
            http,
        };
        validator.refresh_keys().await?;
        info!(
            issuer = %validator.issuer,
            userinfo = ?validator.userinfo_endpoint,
            "JWT validator ready"
        );
        Ok(validator)
    }

    /// Pull profile claims from the IdP's UserInfo endpoint using the same
    /// bearer token. Rauthy doesn't put profile claims in the access token,
    /// so the signin handler calls this once per fresh OIDC login to
    /// backfill display_name on the Account. Not invoked from `require_auth`
    /// — that would mean a UserInfo round-trip on every request.
    /// Returns `None` when the IdP doesn't advertise UserInfo or the call
    /// fails — we never block auth on a missing display name.
    pub async fn fetch_userinfo_profile(&self, token: &str) -> Option<ProfileClaims> {
        let url = self.userinfo_endpoint.as_deref()?;
        let resp = match self
            .http
            .get(url)
            .bearer_auth(token)
            .send()
            .await
            .and_then(|r| r.error_for_status())
        {
            Ok(r) => r,
            Err(err) => {
                warn!(?err, "UserInfo fetch failed");
                return None;
            }
        };
        match resp.json::<ProfileClaims>().await {
            Ok(c) => Some(c),
            Err(err) => {
                warn!(?err, "UserInfo response not JSON-decodable");
                None
            }
        }
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
        if let Some(org) = project_org_slug
            && self.is_org_admin(org)
        {
            return true;
        }
        self.memberships.iter().any(|m| {
            m.role == Role::ProjectAdmin
                && matches!(&m.scope, MembershipScope::Project { project_slug: s } if s == project_slug)
        })
    }
}

/// Axum middleware: validates the Bearer token, lazy-creates the Account
/// from the OIDC `(iss, sub)`, attaches the `AuthContext` (claims + account
/// + memberships) to request extensions.
///
/// Initial-admin bootstrap fires here too: the first verified login matching
/// `MVIRT_INITIAL_ADMIN_EMAIL` receives Platform/PlatformAdmin atomically
/// (race-safe — the apply handler refuses if any platform-admin already
/// exists).
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

    // No bearer presented. In dev mode (no JWT validator configured) we let
    // the request through so handlers fall back to the unauthenticated-dev
    // path; in prod 401 immediately.
    let Some(token) = token else {
        if state.jwt_validator.is_none() {
            return next.run(req).await;
        }
        return (StatusCode::UNAUTHORIZED, "missing bearer token").into_response();
    };

    // Bearer prefix fork: static API keys take a non-JWT path. ADR-0004
    // §"Token validation pipeline" reserves `mvirt_sa_*` for this. Works
    // independently of OIDC configuration — operators running CI-only
    // setups can use SA keys without standing up an IdP.
    if token.starts_with("mvirt_sa_") {
        let ctx = match authenticate_static_api_key(state.as_ref(), token).await {
            Ok(c) => c,
            Err(reason) => {
                warn!(%reason, "static api key auth failed");
                return (StatusCode::UNAUTHORIZED, "invalid api key").into_response();
            }
        };
        let claims = ctx.claims.clone();
        req.extensions_mut().insert(claims);
        req.extensions_mut().insert(ctx);
        return next.run(req).await;
    }

    // JWT-style bearer: reject if no validator is configured (the caller
    // presented credentials we can't verify).
    let Some(validator) = state.jwt_validator.as_ref() else {
        return (StatusCode::UNAUTHORIZED, "JWT auth not configured").into_response();
    };
    let claims = match validator.validate(token).await {
        Ok(c) => c,
        Err(err) => {
            warn!(?err, "JWT validation failed");
            return (StatusCode::UNAUTHORIZED, "invalid token").into_response();
        }
    };

    // Lazy-create / refresh the Account row from the JWT body alone — no
    // UserInfo fetch in the hot path. With rauthy, the access token only
    // carries `email`, so `display_name` will be `None` on first request;
    // the UI calls `POST /v1/auth/signin` once per OIDC callback to
    // backfill display_name from the UserInfo endpoint.
    let ensure = crate::store::EnsureAccountRequest {
        iss: claims.iss.clone(),
        sub: claims.sub.clone(),
        email: claims.email.clone(),
        display_name: claims.best_display_name(),
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
    if let Some(target_email) = state.initial_admin_email.as_deref()
        && account.email.as_deref() == Some(target_email)
        && let Err(e) = state
            .store
            .bootstrap_initial_platform_admin(&account.id)
            .await
    {
        warn!(error = %e, "initial admin bootstrap failed");
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

/// Authenticate a `mvirt_sa_<id>_<secret>` Bearer token. Returns an
/// AuthContext with synthetic claims (sub = account id, iss =
/// `mvirt:service-account`) so downstream handlers can treat the SA
/// uniformly with User auth.
async fn authenticate_static_api_key(
    state: &crate::rest::AppState,
    token: &str,
) -> std::result::Result<AuthContext, &'static str> {
    // Token shape: `mvirt_sa_<id>.<secret>` — `.` separates id and
    // secret unambiguously (base64url contains `-` and `_`, so a `_`
    // separator would be non-trivial to parse back).
    let rest = token.strip_prefix("mvirt_sa_").ok_or("bad prefix")?;
    let (id, secret) = rest.split_once('.').ok_or("missing secret segment")?;
    if id.is_empty() || secret.is_empty() {
        return Err("empty id or secret");
    }

    let api_key = state
        .store
        .get_api_key(id)
        .await
        .map_err(|_| "key lookup failed")?
        .ok_or("unknown key id")?;
    if api_key.revoked_at.is_some() {
        return Err("key revoked");
    }
    if let Some(exp) = &api_key.expires_at {
        let now = chrono::Utc::now().to_rfc3339();
        if now.as_str() > exp.as_str() {
            return Err("key expired");
        }
    }

    // Constant-time compare via blake3::Hash equality. We hash the
    // presented secret, then load the stored hash from hex and let
    // blake3::Hash's PartialEq (which is timing-safe) do the comparison.
    let presented = blake3::hash(secret.as_bytes());
    let stored_bytes: [u8; blake3::OUT_LEN] = match hex_to_array(&api_key.hash_hex) {
        Some(b) => b,
        None => return Err("corrupt stored hash"),
    };
    if presented != blake3::Hash::from(stored_bytes) {
        return Err("secret mismatch");
    }

    let account = state
        .store
        .get_account(&api_key.account_id)
        .await
        .map_err(|_| "account lookup failed")?
        .ok_or("orphan key (account gone)")?;
    let memberships = state
        .store
        .list_memberships_for_account(&account.id)
        .await
        .unwrap_or_default();

    let claims = AuthClaims {
        sub: account.id.clone(),
        iss: "mvirt:service-account".into(),
        exp: 0,
        iat: None,
        email: account.email.clone(),
        name: account.display_name.clone(),
        given_name: None,
        family_name: None,
        preferred_username: account.display_name.clone(),
    };

    Ok(AuthContext {
        claims,
        account,
        memberships,
    })
}

fn hex_to_array(s: &str) -> Option<[u8; blake3::OUT_LEN]> {
    if s.len() != blake3::OUT_LEN * 2 {
        return None;
    }
    let mut out = [0u8; blake3::OUT_LEN];
    for (i, byte) in out.iter_mut().enumerate() {
        let pair = &s.as_bytes()[i * 2..i * 2 + 2];
        let hi = hex_nybble(pair[0])?;
        let lo = hex_nybble(pair[1])?;
        *byte = (hi << 4) | lo;
    }
    Some(out)
}

fn hex_nybble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
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
