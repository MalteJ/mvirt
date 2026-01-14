//! ZFS-specific audit logging
//!
//! Wraps the shared AuditLogger with ZFS-specific convenience methods.

use std::sync::Arc;

use mvirt_log::{AuditLogger, LogLevel};

/// ZFS audit logger with domain-specific methods
pub struct ZfsAuditLogger {
    inner: Arc<AuditLogger>,
}

impl ZfsAuditLogger {
    /// Create a new ZFS audit logger
    pub fn new(log_endpoint: &str) -> Self {
        Self {
            inner: Arc::new(AuditLogger::new(log_endpoint, "zfs")),
        }
    }

    /// Create a noop audit logger (for testing)
    pub fn new_noop() -> Self {
        Self {
            inner: Arc::new(AuditLogger::new_noop()),
        }
    }

    // === Volume Events ===

    pub async fn volume_created(&self, volume_id: &str, volume_name: &str, size_bytes: u64) {
        self.inner
            .log(
                LogLevel::Audit,
                format!("Volume '{}' created ({} bytes)", volume_name, size_bytes),
                vec![volume_id.to_string()],
            )
            .await;
    }

    pub async fn volume_deleted(&self, volume_id: &str, volume_name: &str) {
        self.inner
            .log(
                LogLevel::Audit,
                format!("Volume '{}' deleted", volume_name),
                vec![volume_id.to_string()],
            )
            .await;
    }

    pub async fn volume_resized(&self, volume_id: &str, volume_name: &str, new_size: u64) {
        self.inner
            .log(
                LogLevel::Audit,
                format!("Volume '{}' resized to {} bytes", volume_name, new_size),
                vec![volume_id.to_string()],
            )
            .await;
    }

    // === Import Events ===

    pub async fn import_started(&self, job_id: &str, volume_name: &str, source: &str) {
        self.inner
            .log(
                LogLevel::Audit,
                format!("Import started for '{}' from {}", volume_name, source),
                vec![job_id.to_string()],
            )
            .await;
    }

    pub async fn import_completed(&self, job_id: &str, volume_id: &str, volume_name: &str) {
        self.inner
            .log(
                LogLevel::Audit,
                format!("Import completed for '{}'", volume_name),
                vec![job_id.to_string(), volume_id.to_string()],
            )
            .await;
    }

    pub async fn import_failed(&self, job_id: &str, volume_name: &str, error: &str) {
        self.inner
            .log(
                LogLevel::Error,
                format!("Import failed for '{}': {}", volume_name, error),
                vec![job_id.to_string()],
            )
            .await;
    }

    // === Template Events ===

    pub async fn template_created(
        &self,
        template_id: &str,
        template_name: &str,
        source_volume: &str,
    ) {
        self.inner
            .log(
                LogLevel::Audit,
                format!(
                    "Template '{}' created from volume '{}'",
                    template_name, source_volume
                ),
                vec![template_id.to_string()],
            )
            .await;
    }

    pub async fn template_deleted(&self, template_id: &str, template_name: &str) {
        self.inner
            .log(
                LogLevel::Audit,
                format!("Template '{}' deleted", template_name),
                vec![template_id.to_string()],
            )
            .await;
    }

    pub async fn volume_cloned(&self, volume_id: &str, volume_name: &str, template_name: &str) {
        self.inner
            .log(
                LogLevel::Audit,
                format!(
                    "Volume '{}' cloned from template '{}'",
                    volume_name, template_name
                ),
                vec![volume_id.to_string()],
            )
            .await;
    }

    // === Snapshot Events ===

    pub async fn snapshot_created(&self, volume_id: &str, snapshot_name: &str) {
        self.inner
            .log(
                LogLevel::Audit,
                format!("Snapshot '{}' created", snapshot_name),
                vec![volume_id.to_string()],
            )
            .await;
    }

    pub async fn snapshot_deleted(&self, volume_id: &str, snapshot_name: &str) {
        self.inner
            .log(
                LogLevel::Audit,
                format!("Snapshot '{}' deleted", snapshot_name),
                vec![volume_id.to_string()],
            )
            .await;
    }

    pub async fn snapshot_rollback(&self, volume_id: &str, snapshot_name: &str) {
        self.inner
            .log(
                LogLevel::Audit,
                format!("Volume rolled back to snapshot '{}'", snapshot_name),
                vec![volume_id.to_string()],
            )
            .await;
    }
}

/// Create a shared ZFS audit logger
pub fn create_audit_logger(log_endpoint: &str) -> Arc<ZfsAuditLogger> {
    Arc::new(ZfsAuditLogger::new(log_endpoint))
}
