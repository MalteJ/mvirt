//! Network-specific audit logging
//!
//! Wraps the shared AuditLogger with network-specific convenience methods.
//! All logging is fire-and-forget (non-blocking) to avoid blocking gRPC handlers.

use std::sync::Arc;

use mvirt_log::{AuditLogger, LogLevel};

/// Network audit logger with domain-specific methods.
///
/// All log methods are fire-and-forget: they spawn a task to send the log
/// and return immediately without blocking the caller.
pub struct NetAuditLogger {
    inner: Arc<AuditLogger>,
}

impl NetAuditLogger {
    /// Create a new network audit logger
    pub fn new(log_endpoint: &str) -> Self {
        Self {
            inner: Arc::new(AuditLogger::new(log_endpoint, "net")),
        }
    }

    /// Create a noop audit logger (for testing)
    pub fn new_noop() -> Self {
        Self {
            inner: Arc::new(AuditLogger::new_noop()),
        }
    }

    /// Fire-and-forget log helper
    fn log_async(&self, level: LogLevel, message: String, object_ids: Vec<String>) {
        let inner = Arc::clone(&self.inner);
        tokio::spawn(async move {
            inner.log(level, message, object_ids).await;
        });
    }

    // === Network Events ===

    pub fn network_created(&self, network_id: &str, network_name: &str) {
        self.log_async(
            LogLevel::Audit,
            format!("Network '{}' created", network_name),
            vec![network_id.to_string()],
        );
    }

    pub fn network_updated(&self, network_id: &str, network_name: &str) {
        self.log_async(
            LogLevel::Audit,
            format!("Network '{}' updated", network_name),
            vec![network_id.to_string()],
        );
    }

    pub fn network_deleted(&self, network_id: &str, network_name: &str) {
        self.log_async(
            LogLevel::Audit,
            format!("Network '{}' deleted", network_name),
            vec![network_id.to_string()],
        );
    }

    // === NIC Events ===

    pub fn nic_created(
        &self,
        nic_id: &str,
        network_id: &str,
        mac: &str,
        ipv4: Option<&str>,
        ipv6: Option<&str>,
    ) {
        let ip_info = match (ipv4, ipv6) {
            (Some(v4), Some(v6)) => format!("{}, {}", v4, v6),
            (Some(v4), None) => v4.to_string(),
            (None, Some(v6)) => v6.to_string(),
            (None, None) => "no IP".to_string(),
        };

        self.log_async(
            LogLevel::Audit,
            format!("NIC created: MAC={}, IP={}", mac, ip_info),
            vec![nic_id.to_string(), network_id.to_string()],
        );
    }

    pub fn nic_updated(&self, nic_id: &str) {
        self.log_async(
            LogLevel::Audit,
            "NIC updated".to_string(),
            vec![nic_id.to_string()],
        );
    }

    pub fn nic_deleted(&self, nic_id: &str, network_id: &str) {
        self.log_async(
            LogLevel::Audit,
            "NIC deleted".to_string(),
            vec![nic_id.to_string(), network_id.to_string()],
        );
    }

    pub fn nic_activated(&self, nic_id: &str) {
        self.log_async(
            LogLevel::Info,
            "NIC activated (VM connected)".to_string(),
            vec![nic_id.to_string()],
        );
    }

    pub fn nic_deactivated(&self, nic_id: &str) {
        self.log_async(
            LogLevel::Info,
            "NIC deactivated (VM disconnected)".to_string(),
            vec![nic_id.to_string()],
        );
    }

    // === Routing Events ===

    pub fn route_added(&self, nic_id: &str, prefix: &str) {
        self.log_async(
            LogLevel::Info,
            format!("Route added: {} -> NIC", prefix),
            vec![nic_id.to_string()],
        );
    }

    pub fn route_removed(&self, nic_id: &str, prefix: &str) {
        self.log_async(
            LogLevel::Info,
            format!("Route removed: {}", prefix),
            vec![nic_id.to_string()],
        );
    }
}

/// Create a shared network audit logger
pub fn create_audit_logger(log_endpoint: &str) -> Arc<NetAuditLogger> {
    Arc::new(NetAuditLogger::new(log_endpoint))
}
