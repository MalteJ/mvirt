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

/// Axum middleware: requires a valid Bearer token. Inserts [`AuthClaims`] into
/// request extensions so handlers (or the [`AuthenticatedUser`] extractor)
/// can read the caller identity.
pub async fn require_auth(
    State(validator): State<Arc<JwtValidator>>,
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
    match validator.validate(token).await {
        Ok(claims) => {
            req.extensions_mut().insert(claims);
            next.run(req).await
        }
        Err(err) => {
            warn!(?err, "JWT validation failed");
            (StatusCode::UNAUTHORIZED, "invalid token").into_response()
        }
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
