//! Client for mvirt-vmm daemon.

use anyhow::{Context, Result};
use tonic::transport::Channel;
use tracing::debug;

use crate::proto::vmm::{
    vm_service_client::VmServiceClient, CreateVmRequest, DeleteVmRequest, GetVmRequest,
    KillVmRequest, StartVmRequest, StopVmRequest,
};

pub use crate::proto::vmm::{BootMode, DiskConfig, NicConfig, Vm, VmConfig, VmState};

/// Client for interacting with mvirt-vmm.
pub struct VmmClient {
    client: VmServiceClient<Channel>,
}

impl VmmClient {
    pub async fn connect(endpoint: &str) -> Result<Self> {
        let client = VmServiceClient::connect(endpoint.to_string())
            .await
            .context("Failed to connect to mvirt-vmm")?;
        Ok(Self { client })
    }

    /// Get VM by ID.
    pub async fn get_vm(&mut self, id: &str) -> Result<Option<Vm>> {
        debug!("Getting VM {} from mvirt-vmm", id);
        match self
            .client
            .get_vm(GetVmRequest { id: id.to_string() })
            .await
        {
            Ok(resp) => Ok(Some(resp.into_inner())),
            Err(status) if status.code() == tonic::Code::NotFound => Ok(None),
            Err(e) => Err(e).context("Failed to get VM"),
        }
    }

    /// Create a VM.
    pub async fn create_vm(&mut self, name: &str, config: VmConfig) -> Result<Vm> {
        debug!("Creating VM {} in mvirt-vmm", name);
        let resp = self
            .client
            .create_vm(CreateVmRequest {
                name: Some(name.to_string()),
                config: Some(config),
            })
            .await
            .context("Failed to create VM")?;
        Ok(resp.into_inner())
    }

    /// Start a VM.
    pub async fn start_vm(&mut self, id: &str) -> Result<Vm> {
        debug!("Starting VM {} in mvirt-vmm", id);
        let resp = self
            .client
            .start_vm(StartVmRequest { id: id.to_string() })
            .await
            .context("Failed to start VM")?;
        Ok(resp.into_inner())
    }

    /// Stop a VM gracefully.
    pub async fn stop_vm(&mut self, id: &str) -> Result<Vm> {
        debug!("Stopping VM {} in mvirt-vmm", id);
        let resp = self
            .client
            .stop_vm(StopVmRequest {
                id: id.to_string(),
                timeout_seconds: 30,
            })
            .await
            .context("Failed to stop VM")?;
        Ok(resp.into_inner())
    }

    /// Kill a VM (force stop).
    pub async fn kill_vm(&mut self, id: &str) -> Result<Vm> {
        debug!("Killing VM {} in mvirt-vmm", id);
        let resp = self
            .client
            .kill_vm(KillVmRequest { id: id.to_string() })
            .await
            .context("Failed to kill VM")?;
        Ok(resp.into_inner())
    }

    /// Delete a VM.
    pub async fn delete_vm(&mut self, id: &str) -> Result<()> {
        debug!("Deleting VM {} in mvirt-vmm", id);
        self.client
            .delete_vm(DeleteVmRequest { id: id.to_string() })
            .await
            .context("Failed to delete VM")?;
        Ok(())
    }
}
