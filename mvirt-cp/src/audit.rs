use mvirt_log::{AuditLogger, LogLevel};
use std::sync::Arc;

/// Control Plane specific audit logger
pub struct CpAuditLogger {
    inner: Arc<AuditLogger>,
}

impl CpAuditLogger {
    pub fn new(log_endpoint: &str) -> Self {
        Self {
            inner: Arc::new(AuditLogger::new(log_endpoint, "cp")),
        }
    }

    pub fn new_noop() -> Self {
        Self {
            inner: Arc::new(AuditLogger::new_noop()),
        }
    }

    fn log_async(&self, level: LogLevel, message: String, object_ids: Vec<String>) {
        let inner = Arc::clone(&self.inner);
        tokio::spawn(async move {
            inner.log(level, message, object_ids).await;
        });
    }

    // Cluster events
    pub fn node_joined(&self, node_id: u64, node_name: &str, address: &str) {
        self.log_async(
            LogLevel::Audit,
            format!("Node joined: {} ({}) at {}", node_name, node_id, address),
            vec![format!("node-{}", node_id)],
        );
    }

    pub fn node_removed(&self, node_id: u64) {
        self.log_async(
            LogLevel::Audit,
            format!("Node removed: {}", node_id),
            vec![format!("node-{}", node_id)],
        );
    }

    pub fn leader_elected(&self, node_id: u64, term: u64) {
        self.log_async(
            LogLevel::Info,
            format!("Leader elected: node {} for term {}", node_id, term),
            vec![format!("node-{}", node_id)],
        );
    }

    // Network events
    pub fn network_created(&self, network_id: &str, network_name: &str) {
        self.log_async(
            LogLevel::Audit,
            format!("Network created: {} ({})", network_name, network_id),
            vec![network_id.to_string()],
        );
    }

    pub fn network_updated(&self, network_id: &str) {
        self.log_async(
            LogLevel::Audit,
            format!("Network updated: {}", network_id),
            vec![network_id.to_string()],
        );
    }

    pub fn network_deleted(&self, network_id: &str) {
        self.log_async(
            LogLevel::Audit,
            format!("Network deleted: {}", network_id),
            vec![network_id.to_string()],
        );
    }

    // NIC events
    pub fn nic_created(&self, nic_id: &str, network_id: &str, mac: &str) {
        self.log_async(
            LogLevel::Audit,
            format!(
                "NIC created: {} (MAC: {}) in network {}",
                nic_id, mac, network_id
            ),
            vec![nic_id.to_string(), network_id.to_string()],
        );
    }

    pub fn nic_updated(&self, nic_id: &str) {
        self.log_async(
            LogLevel::Audit,
            format!("NIC updated: {}", nic_id),
            vec![nic_id.to_string()],
        );
    }

    pub fn nic_deleted(&self, nic_id: &str) {
        self.log_async(
            LogLevel::Audit,
            format!("NIC deleted: {}", nic_id),
            vec![nic_id.to_string()],
        );
    }
}

pub fn create_audit_logger(log_endpoint: &str) -> Arc<CpAuditLogger> {
    Arc::new(CpAuditLogger::new(log_endpoint))
}
