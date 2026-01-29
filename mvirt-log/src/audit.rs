//! Audit logging to mvirt-log service
//!
//! Shared audit logger for all mvirt components. Non-blocking and fault-tolerant -
//! if mvirt-log is unavailable, events are logged locally via tracing and discarded.

use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, warn};

use crate::{LogEntry, LogLevel, LogRequest, LogServiceClient};

/// Audit logger client for mvirt-log
pub struct AuditLogger {
    client: RwLock<Option<LogServiceClient<tonic::transport::Channel>>>,
    log_endpoint: String,
    component: String,
}

impl AuditLogger {
    /// Create a new audit logger for a specific component
    pub fn new(log_endpoint: &str, component: &str) -> Self {
        Self {
            client: RwLock::new(None),
            log_endpoint: log_endpoint.to_string(),
            component: component.to_string(),
        }
    }

    /// Create a noop audit logger (for testing)
    /// Uses an empty endpoint so ensure_connected() always fails silently
    pub fn new_noop() -> Self {
        Self {
            client: RwLock::new(None),
            log_endpoint: String::new(),
            component: String::new(),
        }
    }

    /// Connect to mvirt-log (lazy, on first log)
    async fn ensure_connected(&self) -> Option<LogServiceClient<tonic::transport::Channel>> {
        {
            let client = self.client.read().await;
            if client.is_some() {
                return client.clone();
            }
        }

        // Try to connect
        let mut client = self.client.write().await;
        if client.is_none() {
            match LogServiceClient::connect(self.log_endpoint.clone()).await {
                Ok(c) => {
                    debug!(endpoint = %self.log_endpoint, "Connected to mvirt-log");
                    *client = Some(c);
                }
                Err(e) => {
                    debug!(error = %e, "Failed to connect to mvirt-log (audit logs disabled)");
                    return None;
                }
            }
        }
        client.clone()
    }

    /// Log an audit event
    ///
    /// Events are always logged locally via tracing, and sent to mvirt-log if available.
    pub async fn log(&self, level: LogLevel, message: impl Into<String>, object_ids: Vec<String>) {
        let message = message.into();

        // Always log locally via tracing
        match level {
            LogLevel::Emergency | LogLevel::Alert | LogLevel::Critical => {
                tracing::error!(target: "audit", component = %self.component, objects = ?object_ids, "{}", message)
            }
            LogLevel::Error => {
                tracing::error!(target: "audit", component = %self.component, objects = ?object_ids, "{}", message)
            }
            LogLevel::Warn => {
                tracing::warn!(target: "audit", component = %self.component, objects = ?object_ids, "{}", message)
            }
            LogLevel::Notice | LogLevel::Audit => {
                tracing::info!(target: "audit", component = %self.component, objects = ?object_ids, "{}", message)
            }
            LogLevel::Info => {
                tracing::info!(target: "audit", component = %self.component, objects = ?object_ids, "{}", message)
            }
            LogLevel::Debug => {
                tracing::debug!(target: "audit", component = %self.component, objects = ?object_ids, "{}", message)
            }
        }

        // Try to send to mvirt-log
        if let Some(mut client) = self.ensure_connected().await {
            let request = LogRequest {
                entry: Some(LogEntry {
                    id: String::new(), // Server generates
                    timestamp_ns: 0,   // Server generates
                    message,
                    level: level as i32,
                    component: self.component.clone(),
                    related_object_ids: object_ids,
                }),
            };

            if let Err(e) = client.log(request).await {
                warn!(error = %e, "Failed to send audit log to mvirt-log");
                // Clear client so we reconnect next time
                *self.client.write().await = None;
            }
        }
    }
}

/// Create a shared audit logger
pub fn create_audit_logger(log_endpoint: &str, component: &str) -> Arc<AuditLogger> {
    Arc::new(AuditLogger::new(log_endpoint, component))
}
