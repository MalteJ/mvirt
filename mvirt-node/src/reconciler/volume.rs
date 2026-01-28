//! Volume reconciler - creates and manages volumes via mvirt-zfs.

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::{error, info};

use super::Reconciler;
use crate::clients::ZfsClient;
use crate::proto::node::{ResourcePhase, VolumeSpec, VolumeStatus};

/// Volume reconciler that interacts with mvirt-zfs.
pub struct VolumeReconciler {
    zfs: Mutex<ZfsClient>,
}

impl VolumeReconciler {
    pub fn new(zfs: ZfsClient) -> Self {
        Self {
            zfs: Mutex::new(zfs),
        }
    }
}

#[async_trait]
impl Reconciler for VolumeReconciler {
    type Spec = VolumeSpec;
    type Status = VolumeStatus;

    async fn reconcile(&self, id: &str, spec: &Self::Spec) -> Result<Self::Status> {
        let meta = spec.meta.as_ref().expect("VolumeSpec must have meta");
        info!("Reconciling volume {} ({})", meta.name, id);

        let mut zfs = self.zfs.lock().await;

        // Check if volume already exists
        match zfs.get_volume(&meta.name).await? {
            Some(vol) => {
                // Volume exists, report current state
                Ok(VolumeStatus {
                    id: id.to_string(),
                    phase: ResourcePhase::Ready as i32,
                    message: None,
                    zfs_path: vol.name.clone(),
                    device_path: vol.path,
                    used_bytes: vol.used_bytes,
                })
            }
            None => {
                // Volume doesn't exist, create it
                let size_bytes = spec.size_gb * 1024 * 1024 * 1024;

                let result = if let Some(template_id) = &spec.template_id {
                    // Clone from template
                    info!("Cloning volume {} from template {}", meta.name, template_id);
                    zfs.clone_from_template(template_id, &meta.name, Some(size_bytes))
                        .await
                } else {
                    // Create empty volume
                    zfs.create_volume(&meta.name, size_bytes).await
                };

                match result {
                    Ok(vol) => Ok(VolumeStatus {
                        id: id.to_string(),
                        phase: ResourcePhase::Ready as i32,
                        message: None,
                        zfs_path: vol.name.clone(),
                        device_path: vol.path,
                        used_bytes: vol.used_bytes,
                    }),
                    Err(e) => {
                        error!("Failed to create volume {}: {}", id, e);
                        Ok(VolumeStatus {
                            id: id.to_string(),
                            phase: ResourcePhase::Failed as i32,
                            message: Some(format!("Failed to create: {}", e)),
                            zfs_path: String::new(),
                            device_path: String::new(),
                            used_bytes: 0,
                        })
                    }
                }
            }
        }
    }

    async fn finalize(&self, id: &str) -> Result<()> {
        info!("Finalizing (deleting) volume {}", id);
        let mut zfs = self.zfs.lock().await;
        let _ = zfs.delete_volume(id).await;
        Ok(())
    }
}
