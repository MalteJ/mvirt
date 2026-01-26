//! NIC reconciler - reconciles NIC specs with mvirt-net.

use anyhow::Result;
use async_trait::async_trait;
use tracing::{debug, info};

use super::Reconciler;

/// NIC spec from the API.
#[derive(Debug, Clone)]
pub struct NicSpec {
    pub id: String,
    pub name: Option<String>,
    pub network_id: String,
    pub mac_address: String,
    pub ipv4_address: Option<String>,
    pub ipv6_address: Option<String>,
    pub routed_ipv4_prefixes: Vec<String>,
    pub routed_ipv6_prefixes: Vec<String>,
}

/// NIC status to report back.
#[derive(Debug, Clone)]
pub struct NicStatus {
    pub phase: NicPhase,
    pub socket_path: Option<String>,
    pub message: Option<String>,
}

/// NIC lifecycle phase.
#[derive(Debug, Clone, Copy)]
pub enum NicPhase {
    Pending,
    Creating,
    Active,
    Updating,
    Deleting,
    Failed,
}

/// NIC reconciler that interacts with mvirt-net.
pub struct NicReconciler {
    net_endpoint: String,
}

impl NicReconciler {
    pub fn new(net_endpoint: String) -> Self {
        Self { net_endpoint }
    }
}

#[async_trait]
impl Reconciler for NicReconciler {
    type Spec = NicSpec;
    type Status = NicStatus;

    async fn reconcile(&self, id: &str, spec: &Self::Spec) -> Result<Self::Status> {
        info!("Reconciling NIC {} in network {}", id, spec.network_id);
        debug!("NIC spec: {:?}", spec);

        // TODO: Connect to mvirt-net and check current state
        // For now, just return Active status

        // 1. Get current state from mvirt-net
        // let current = self.net_client.get_nic(id).await?;

        // 2. Compare with spec
        // if current.is_none() {
        //     // Create NIC
        //     let result = self.net_client.create_nic(spec).await?;
        //     return Ok(NicStatus {
        //         phase: NicPhase::Creating,
        //         socket_path: Some(result.socket_path),
        //         message: None,
        //     });
        // }

        // 3. Check if update needed
        // if needs_update(&current, spec) {
        //     self.net_client.update_nic(spec).await?;
        //     return Ok(NicStatus { phase: NicPhase::Updating, .. });
        // }

        Ok(NicStatus {
            phase: NicPhase::Active,
            socket_path: Some(format!("/run/mvirt/nics/{}.sock", id)),
            message: None,
        })
    }

    async fn finalize(&self, id: &str) -> Result<()> {
        info!("Finalizing (deleting) NIC {}", id);

        // TODO: Connect to mvirt-net and delete
        // self.net_client.delete_nic(id).await?;

        Ok(())
    }
}
