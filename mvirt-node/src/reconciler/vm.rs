//! VM reconciler - reconciles VM specs with mvirt-vmm.

use anyhow::Result;
use async_trait::async_trait;
use tracing::{debug, info};

use super::Reconciler;

/// VM spec from the API.
#[derive(Debug, Clone)]
pub struct VmSpec {
    pub id: String,
    pub name: String,
    pub cpu_cores: u32,
    pub memory_mb: u64,
    pub disk_gb: u64,
    pub network_id: String,
    pub nic_id: Option<String>,
    pub image: String,
    pub desired_state: VmDesiredState,
}

/// Desired power state for a VM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmDesiredState {
    Running,
    Stopped,
}

/// VM status to report back.
#[derive(Debug, Clone)]
pub struct VmStatus {
    pub phase: VmPhase,
    pub ip_address: Option<String>,
    pub message: Option<String>,
}

/// VM lifecycle phase.
#[derive(Debug, Clone, Copy)]
pub enum VmPhase {
    Pending,
    Scheduled,
    Creating,
    Running,
    Stopping,
    Stopped,
    Failed,
}

/// VM reconciler that interacts with mvirt-vmm.
pub struct VmReconciler {
    vmm_endpoint: String,
    zfs_endpoint: String,
}

impl VmReconciler {
    pub fn new(vmm_endpoint: String, zfs_endpoint: String) -> Self {
        Self {
            vmm_endpoint,
            zfs_endpoint,
        }
    }
}

#[async_trait]
impl Reconciler for VmReconciler {
    type Spec = VmSpec;
    type Status = VmStatus;

    async fn reconcile(&self, id: &str, spec: &Self::Spec) -> Result<Self::Status> {
        info!("Reconciling VM {} ({})", spec.name, id);
        debug!("VM spec: {:?}", spec);

        // TODO: Implement full reconciliation logic
        // This is a placeholder implementation

        // 1. Check if VM exists in mvirt-vmm
        // let current = self.vmm_client.get_vm(id).await?;

        // 2. If VM doesn't exist and desired state is Running, create it
        // if current.is_none() && spec.desired_state == VmDesiredState::Running {
        //     // First ensure disk exists (mvirt-zfs)
        //     // self.zfs_client.create_volume(&spec.name, spec.disk_gb).await?;
        //
        //     // Then create VM
        //     // self.vmm_client.create_vm(spec).await?;
        //     return Ok(VmStatus {
        //         phase: VmPhase::Creating,
        //         ip_address: None,
        //         message: Some("Creating VM".to_string()),
        //     });
        // }

        // 3. If VM exists, check if state matches desired
        // match (current.state, spec.desired_state) {
        //     (VmState::Running, VmDesiredState::Stopped) => {
        //         self.vmm_client.stop_vm(id).await?;
        //         return Ok(VmStatus { phase: VmPhase::Stopping, .. });
        //     }
        //     (VmState::Stopped, VmDesiredState::Running) => {
        //         self.vmm_client.start_vm(id).await?;
        //         return Ok(VmStatus { phase: VmPhase::Creating, .. });
        //     }
        //     _ => {}
        // }

        // For now, return a placeholder status
        let phase = match spec.desired_state {
            VmDesiredState::Running => VmPhase::Running,
            VmDesiredState::Stopped => VmPhase::Stopped,
        };

        Ok(VmStatus {
            phase,
            ip_address: None,
            message: None,
        })
    }

    async fn finalize(&self, id: &str) -> Result<()> {
        info!("Finalizing (deleting) VM {}", id);

        // TODO: Implement deletion
        // 1. Stop VM if running
        // self.vmm_client.stop_vm(id).await?;
        //
        // 2. Delete VM
        // self.vmm_client.delete_vm(id).await?;
        //
        // 3. Delete disk
        // self.zfs_client.delete_volume(id).await?;

        Ok(())
    }
}
