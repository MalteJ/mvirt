//! VM reconciler - reconciles VM specs with mvirt-vmm.

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::{error, info};

use super::Reconciler;
use crate::clients::{VmmClient, ZfsClient};
use crate::proto::node::{VmDesiredState, VmPhase, VmSpec, VmStatus};
use crate::proto::vmm::{BootMode, DiskConfig, NicConfig, VmConfig, VmState};

/// VM reconciler that interacts with mvirt-vmm.
pub struct VmReconciler {
    vmm: Mutex<VmmClient>,
    zfs: Mutex<ZfsClient>,
}

impl VmReconciler {
    pub fn new(vmm: VmmClient, zfs: ZfsClient) -> Self {
        Self {
            vmm: Mutex::new(vmm),
            zfs: Mutex::new(zfs),
        }
    }
}

#[async_trait]
impl Reconciler for VmReconciler {
    type Spec = VmSpec;
    type Status = VmStatus;

    async fn reconcile(&self, id: &str, spec: &Self::Spec) -> Result<Self::Status> {
        let meta = spec.meta.as_ref().expect("VmSpec must have meta");
        info!("Reconciling VM {} ({})", meta.name, id);

        let mut vmm = self.vmm.lock().await;
        let current = vmm.get_vm(id).await?;
        let desired =
            VmDesiredState::try_from(spec.desired_state).unwrap_or(VmDesiredState::Running);

        match (current, desired) {
            // VM doesn't exist, desired Running → create + start
            (None, VmDesiredState::Running) => {
                info!("Creating VM {}", meta.name);

                // Look up volume device path from ZFS by volume name
                let disk_path = {
                    let mut zfs = self.zfs.lock().await;
                    match zfs.get_volume(&spec.volume_name).await {
                        Ok(Some(vol)) => vol.path,
                        Ok(None) => {
                            error!("Volume '{}' not found in ZFS", spec.volume_name);
                            return Ok(VmStatus {
                                id: id.to_string(),
                                phase: VmPhase::Failed as i32,
                                message: Some(format!("Volume '{}' not found", spec.volume_name)),
                                ip_address: None,
                                pid: None,
                            });
                        }
                        Err(e) => {
                            error!("Failed to look up volume '{}': {:?}", spec.volume_name, e);
                            return Ok(VmStatus {
                                id: id.to_string(),
                                phase: VmPhase::Failed as i32,
                                message: Some(format!("Volume lookup failed: {}", e)),
                                ip_address: None,
                                pid: None,
                            });
                        }
                    }
                };

                let config = VmConfig {
                    vcpus: spec.cpu_cores,
                    memory_mb: spec.memory_mb,
                    boot_mode: BootMode::Disk as i32,
                    kernel: None,
                    initramfs: None,
                    cmdline: None,
                    disks: vec![DiskConfig {
                        path: disk_path,
                        readonly: false,
                    }],
                    nics: vec![NicConfig {
                        tap: None,
                        mac: None,
                        vhost_socket: Some(format!("/run/mvirt-net/nic-{}.sock", spec.nic_id)),
                    }],
                    user_data: None,
                    nested_virt: false,
                };

                match vmm.create_vm(&meta.name, config).await {
                    Ok(vm) => {
                        // Auto-start after creation
                        match vmm.start_vm(&vm.id).await {
                            Ok(_) => Ok(VmStatus {
                                id: id.to_string(),
                                phase: VmPhase::Running as i32,
                                message: None,
                                ip_address: None,
                                pid: None,
                            }),
                            Err(e) => {
                                error!("Failed to start VM {}: {}", id, e);
                                Ok(VmStatus {
                                    id: id.to_string(),
                                    phase: VmPhase::Failed as i32,
                                    message: Some(format!("Failed to start: {}", e)),
                                    ip_address: None,
                                    pid: None,
                                })
                            }
                        }
                    }
                    Err(e) => {
                        error!("Failed to create VM {}: {}", id, e);
                        Ok(VmStatus {
                            id: id.to_string(),
                            phase: VmPhase::Failed as i32,
                            message: Some(format!("Failed to create: {}", e)),
                            ip_address: None,
                            pid: None,
                        })
                    }
                }
            }
            // VM doesn't exist, desired Stopped → nothing to do
            (None, VmDesiredState::Stopped) => Ok(VmStatus {
                id: id.to_string(),
                phase: VmPhase::Stopped as i32,
                message: None,
                ip_address: None,
                pid: None,
            }),
            // VM exists, desired Stopped, currently running → stop
            (Some(vm), VmDesiredState::Stopped)
                if vm.state == VmState::Running as i32 || vm.state == VmState::Starting as i32 =>
            {
                info!("Stopping VM {}", id);
                match vmm.stop_vm(&vm.id).await {
                    Ok(_) => Ok(VmStatus {
                        id: id.to_string(),
                        phase: VmPhase::Stopped as i32,
                        message: None,
                        ip_address: None,
                        pid: None,
                    }),
                    Err(e) => Ok(VmStatus {
                        id: id.to_string(),
                        phase: VmPhase::Failed as i32,
                        message: Some(format!("Failed to stop: {}", e)),
                        ip_address: None,
                        pid: None,
                    }),
                }
            }
            // VM exists, desired Running, currently stopped → start
            (Some(vm), VmDesiredState::Running) if vm.state == VmState::Stopped as i32 => {
                info!("Starting VM {}", id);
                match vmm.start_vm(&vm.id).await {
                    Ok(_) => Ok(VmStatus {
                        id: id.to_string(),
                        phase: VmPhase::Running as i32,
                        message: None,
                        ip_address: None,
                        pid: None,
                    }),
                    Err(e) => Ok(VmStatus {
                        id: id.to_string(),
                        phase: VmPhase::Failed as i32,
                        message: Some(format!("Failed to start: {}", e)),
                        ip_address: None,
                        pid: None,
                    }),
                }
            }
            // VM exists and state matches or is transitioning → report current
            (Some(vm), _) => {
                let phase = match VmState::try_from(vm.state) {
                    Ok(VmState::Running) => VmPhase::Running,
                    Ok(VmState::Stopped) => VmPhase::Stopped,
                    Ok(VmState::Starting) => VmPhase::Creating,
                    Ok(VmState::Stopping) => VmPhase::Stopping,
                    _ => VmPhase::Pending,
                };
                Ok(VmStatus {
                    id: id.to_string(),
                    phase: phase as i32,
                    message: None,
                    ip_address: None,
                    pid: None,
                })
            }
            // Catch-all
            _ => Ok(VmStatus {
                id: id.to_string(),
                phase: VmPhase::Pending as i32,
                message: None,
                ip_address: None,
                pid: None,
            }),
        }
    }

    async fn finalize(&self, id: &str) -> Result<()> {
        info!("Finalizing (deleting) VM {}", id);
        let mut vmm = self.vmm.lock().await;

        // Try to stop first, then delete
        if let Some(vm) = vmm.get_vm(id).await? {
            if vm.state == VmState::Running as i32 {
                let _ = vmm.stop_vm(&vm.id).await;
            }
            vmm.delete_vm(&vm.id).await?;
        }

        Ok(())
    }
}
