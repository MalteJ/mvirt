use mvirt_log::{AuditLogger, LogLevel};
use std::sync::Arc;

/// API Server audit logger
pub struct ApiAuditLogger {
    inner: Arc<AuditLogger>,
}

impl ApiAuditLogger {
    pub fn new(log_endpoint: &str) -> Self {
        Self {
            inner: Arc::new(AuditLogger::new(log_endpoint, "api")),
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

    // Hypervisor node events
    pub fn hypervisor_node_registered(&self, node_id: &str, node_name: &str) {
        self.log_async(
            LogLevel::Audit,
            format!("Hypervisor node registered: {} ({})", node_name, node_id),
            vec![node_id.to_string()],
        );
    }

    pub fn hypervisor_node_deregistered(&self, node_id: &str) {
        self.log_async(
            LogLevel::Audit,
            format!("Hypervisor node deregistered: {}", node_id),
            vec![node_id.to_string()],
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

    // VM events
    pub fn vm_created(&self, vm_id: &str, vm_name: &str) {
        self.log_async(
            LogLevel::Audit,
            format!("VM created: {} ({})", vm_name, vm_id),
            vec![vm_id.to_string()],
        );
    }

    pub fn vm_spec_updated(&self, vm_id: &str) {
        self.log_async(
            LogLevel::Audit,
            format!("VM spec updated: {}", vm_id),
            vec![vm_id.to_string()],
        );
    }

    pub fn vm_status_updated(&self, vm_id: &str) {
        self.log_async(
            LogLevel::Audit,
            format!("VM status updated: {}", vm_id),
            vec![vm_id.to_string()],
        );
    }

    pub fn vm_deleted(&self, vm_id: &str) {
        self.log_async(
            LogLevel::Audit,
            format!("VM deleted: {}", vm_id),
            vec![vm_id.to_string()],
        );
    }
}

pub fn create_audit_logger(log_endpoint: &str) -> Arc<ApiAuditLogger> {
    Arc::new(ApiAuditLogger::new(log_endpoint))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_noop_logger_doesnt_panic() {
        let logger = ApiAuditLogger::new_noop();

        // Call all methods - none should panic
        logger.node_joined(1, "test-node", "127.0.0.1:6001");
        logger.node_removed(1);
        logger.leader_elected(1, 1);
        logger.network_created("net-123", "test-network");
        logger.network_updated("net-123");
        logger.network_deleted("net-123");
        logger.nic_created("nic-456", "net-123", "52:54:00:11:22:33");
        logger.nic_updated("nic-456");
        logger.nic_deleted("nic-456");
    }

    #[tokio::test]
    async fn test_create_audit_logger_with_invalid_endpoint() {
        // Should not panic even with invalid endpoint
        // The logger will just fail silently on log attempts
        let logger = create_audit_logger("http://invalid-endpoint:99999");
        logger.network_created("net-1", "test");
    }
}
