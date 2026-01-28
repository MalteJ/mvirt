//! Template reconciler - downloads and stores templates via mvirt-zfs.

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::{error, info};

use super::Reconciler;
use crate::clients::ZfsClient;
use crate::proto::node::{ResourcePhase, TemplateSpec, TemplateStatus};

/// Template reconciler that interacts with mvirt-zfs.
pub struct TemplateReconciler {
    zfs: Mutex<ZfsClient>,
}

impl TemplateReconciler {
    pub fn new(zfs: ZfsClient) -> Self {
        Self {
            zfs: Mutex::new(zfs),
        }
    }
}

#[async_trait]
impl Reconciler for TemplateReconciler {
    type Spec = TemplateSpec;
    type Status = TemplateStatus;

    async fn reconcile(&self, id: &str, spec: &Self::Spec) -> Result<Self::Status> {
        let meta = spec.meta.as_ref().expect("TemplateSpec must have meta");
        info!("Reconciling template {} ({})", meta.name, id);

        let mut zfs = self.zfs.lock().await;

        // Check if template already exists locally
        let templates = zfs.list_templates().await?;
        if let Some(tpl) = templates.iter().find(|t| t.name == meta.name) {
            return Ok(TemplateStatus {
                id: id.to_string(),
                phase: ResourcePhase::Ready as i32,
                message: None,
                size_bytes: tpl.size_bytes,
                local_path: tpl.snapshot_path.clone(),
            });
        }

        // Template doesn't exist, import it
        match zfs.import_template(&meta.name, &spec.url, None).await {
            Ok(job) => Ok(TemplateStatus {
                id: id.to_string(),
                phase: ResourcePhase::Creating as i32,
                message: Some(format!("Import job: {}", job.id)),
                size_bytes: 0,
                local_path: String::new(),
            }),
            Err(e) => {
                error!("Failed to import template {}: {}", id, e);
                Ok(TemplateStatus {
                    id: id.to_string(),
                    phase: ResourcePhase::Failed as i32,
                    message: Some(format!("Failed to import: {}", e)),
                    size_bytes: 0,
                    local_path: String::new(),
                })
            }
        }
    }

    async fn finalize(&self, id: &str) -> Result<()> {
        info!("Finalizing (deleting) template {}", id);
        let mut zfs = self.zfs.lock().await;
        // Use id as the template name for deletion
        let _ = zfs.delete_template(id).await;
        Ok(())
    }
}
