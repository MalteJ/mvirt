//! Template reconciler - downloads and stores templates via mvirt-zfs.

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::{error, info};

use super::Reconciler;
use crate::clients::ZfsClient;
use crate::proto::node::{ResourcePhase, TemplateSpec, TemplateStatus};
use crate::proto::zfs::ImportJobState;

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
            info!("Template {} already exists locally", meta.name);
            return Ok(TemplateStatus {
                id: id.to_string(),
                phase: ResourcePhase::Ready as i32,
                message: None,
                size_bytes: tpl.size_bytes,
                local_path: tpl.snapshot_path.clone(),
            });
        }

        // Template doesn't exist, import it
        let job = match zfs.import_template(&meta.name, &spec.url, None).await {
            Ok(job) => job,
            Err(e) => {
                error!("Failed to start template import {}: {:?}", id, e);
                return Ok(TemplateStatus {
                    id: id.to_string(),
                    phase: ResourcePhase::Failed as i32,
                    message: Some(format!("Failed to import: {}", e)),
                    size_bytes: 0,
                    local_path: String::new(),
                });
            }
        };

        info!("Import job {} started for template {}", job.id, meta.name);

        // Poll until import completes
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;

            let job = match zfs.get_import_job(&job.id).await {
                Ok(j) => j,
                Err(e) => {
                    error!("Failed to poll import job {}: {:?}", job.id, e);
                    return Ok(TemplateStatus {
                        id: id.to_string(),
                        phase: ResourcePhase::Failed as i32,
                        message: Some(format!("Failed to poll import: {}", e)),
                        size_bytes: 0,
                        local_path: String::new(),
                    });
                }
            };

            let state = ImportJobState::try_from(job.state).unwrap_or(ImportJobState::Unspecified);
            match state {
                ImportJobState::Completed => {
                    if let Some(tpl) = job.template {
                        info!(
                            "Template {} import completed ({})",
                            meta.name, tpl.snapshot_path
                        );
                        return Ok(TemplateStatus {
                            id: id.to_string(),
                            phase: ResourcePhase::Ready as i32,
                            message: None,
                            size_bytes: tpl.size_bytes,
                            local_path: tpl.snapshot_path,
                        });
                    }
                    // Completed but no template data — fall back to list
                    let templates = zfs.list_templates().await?;
                    if let Some(tpl) = templates.iter().find(|t| t.name == meta.name) {
                        info!("Template {} import completed", meta.name);
                        return Ok(TemplateStatus {
                            id: id.to_string(),
                            phase: ResourcePhase::Ready as i32,
                            message: None,
                            size_bytes: tpl.size_bytes,
                            local_path: tpl.snapshot_path.clone(),
                        });
                    }
                    return Ok(TemplateStatus {
                        id: id.to_string(),
                        phase: ResourcePhase::Failed as i32,
                        message: Some("Import completed but template not found".to_string()),
                        size_bytes: 0,
                        local_path: String::new(),
                    });
                }
                ImportJobState::Failed => {
                    let msg = job.error.unwrap_or_else(|| "Unknown error".to_string());
                    error!("Template {} import failed: {}", meta.name, msg);
                    return Ok(TemplateStatus {
                        id: id.to_string(),
                        phase: ResourcePhase::Failed as i32,
                        message: Some(msg),
                        size_bytes: 0,
                        local_path: String::new(),
                    });
                }
                ImportJobState::Cancelled => {
                    return Ok(TemplateStatus {
                        id: id.to_string(),
                        phase: ResourcePhase::Failed as i32,
                        message: Some("Import cancelled".to_string()),
                        size_bytes: 0,
                        local_path: String::new(),
                    });
                }
                _ => {
                    // Still in progress (Pending, Downloading, Converting, Writing)
                    info!(
                        "Template {} import in progress: {:?} ({}/{})",
                        meta.name, state, job.bytes_written, job.total_bytes
                    );
                }
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
