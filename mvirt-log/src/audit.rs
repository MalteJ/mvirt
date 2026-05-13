//! Audit logging to mvirt-log service
//!
//! Shared audit logger for all mvirt components. Non-blocking and
//! fault-tolerant: every event is logged locally via `tracing` regardless of
//! whether the remote write succeeds, so logs never go missing entirely.
//!
//! Multi-endpoint failover via `Channel::balance_list` — connection
//! lifecycle (lazy connect, reconnect-on-failure) is handled by tonic
//! internally, no manual state machine here.

use std::path::Path;
use std::sync::Arc;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint, Identity};
use tracing::warn;

use crate::{LogEntry, LogLevel, LogRequest, LogServiceClient};

/// Audit logger client for mvirt-log
pub struct AuditLogger {
    client: Option<LogServiceClient<Channel>>,
    component: String,
}

impl AuditLogger {
    /// Build an audit logger that targets one or more mvirt-log endpoints.
    ///
    /// `tls = None` selects plain h2c (intended for dev / loopback only).
    /// Multi-endpoint connections fail over via `Channel::balance_list`.
    pub fn new(
        endpoints: Vec<String>,
        component: &str,
        tls: Option<ClientTlsConfig>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        if endpoints.is_empty() {
            return Err("audit logger needs at least one endpoint".into());
        }
        let parsed: Result<Vec<Endpoint>, _> = endpoints
            .into_iter()
            .map(|url| {
                let ep = Endpoint::from_shared(url.clone())
                    .map_err(|e| format!("invalid endpoint {url}: {e}"))?;
                if let Some(t) = tls.as_ref() {
                    ep.tls_config(t.clone()).map_err(|e| {
                        let mut msg = format!("tls config for {url}: {e}");
                        let mut src = std::error::Error::source(&e);
                        while let Some(s) = src {
                            msg.push_str(&format!(" :: {s}"));
                            src = s.source();
                        }
                        msg.into()
                    })
                } else {
                    Ok::<_, Box<dyn std::error::Error + Send + Sync>>(ep)
                }
            })
            .collect();
        let channel = Channel::balance_list(parsed?.into_iter());
        Ok(Self {
            client: Some(LogServiceClient::new(channel)),
            component: component.to_string(),
        })
    }

    /// Create a noop audit logger (for testing or when remote logging is
    /// intentionally disabled). Tracing-side logging still happens.
    pub fn new_noop() -> Self {
        Self {
            client: None,
            component: String::new(),
        }
    }

    /// Log an audit event.
    ///
    /// Always emits via local `tracing`. If a remote client is configured,
    /// also fires a `LogService.Log` RPC; transient failures are swallowed
    /// because the local trace is the durable record.
    pub async fn log(&self, level: LogLevel, message: impl Into<String>, object_ids: Vec<String>) {
        let message = message.into();

        match level {
            LogLevel::Emergency | LogLevel::Alert | LogLevel::Critical | LogLevel::Error => {
                tracing::error!(target: "audit", component = %self.component, objects = ?object_ids, "{}", message)
            }
            LogLevel::Warn => {
                tracing::warn!(target: "audit", component = %self.component, objects = ?object_ids, "{}", message)
            }
            LogLevel::Notice | LogLevel::Audit | LogLevel::Info => {
                tracing::info!(target: "audit", component = %self.component, objects = ?object_ids, "{}", message)
            }
            LogLevel::Debug => {
                tracing::debug!(target: "audit", component = %self.component, objects = ?object_ids, "{}", message)
            }
        }

        if let Some(mut client) = self.client.clone() {
            let request = LogRequest {
                entry: Some(LogEntry {
                    id: String::new(),
                    timestamp_ns: 0,
                    message,
                    level: level as i32,
                    component: self.component.clone(),
                    related_object_ids: object_ids,
                }),
            };
            if let Err(e) = client.log(request).await {
                warn!(error = %e, "audit log RPC failed");
            }
        }
    }
}

/// Build a `ClientTlsConfig` from PEM files on disk.
///
/// `ca` pins the trusted issuer; `cert` + `key` are the client identity
/// presented during the mTLS handshake. SNI / domain name defaults to the
/// host portion of the endpoint URL.
pub fn tls_config_from_paths(
    ca: &Path,
    cert: &Path,
    key: &Path,
) -> Result<ClientTlsConfig, Box<dyn std::error::Error + Send + Sync>> {
    let ca_pem = std::fs::read(ca).map_err(|e| format!("read ca {}: {e}", ca.display()))?;
    let cert_pem = std::fs::read(cert).map_err(|e| format!("read cert {}: {e}", cert.display()))?;
    let key_pem = std::fs::read(key).map_err(|e| format!("read key {}: {e}", key.display()))?;
    Ok(ClientTlsConfig::new()
        .ca_certificate(Certificate::from_pem(ca_pem))
        .identity(Identity::from_pem(cert_pem, key_pem)))
}

/// Convenience constructor used by every component's wrapper.
///
/// Endpoints + optional TLS, returns an `Arc<AuditLogger>`. Falls back to
/// a noop logger (with a warn-level diagnostic) on construction failure so
/// daemons keep running even with misconfigured log endpoints — local
/// tracing still captures the events.
pub fn create_audit_logger(
    endpoints: Vec<String>,
    component: &str,
    tls: Option<ClientTlsConfig>,
) -> Arc<AuditLogger> {
    match AuditLogger::new(endpoints, component, tls) {
        Ok(l) => Arc::new(l),
        Err(e) => {
            warn!(error = %e, component, "audit logger construction failed; falling back to noop");
            Arc::new(AuditLogger::new_noop())
        }
    }
}
