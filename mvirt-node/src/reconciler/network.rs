//! Network reconciler - reconciles network specs with mvirt-net.

use anyhow::Result;
use async_trait::async_trait;
use tracing::{debug, info};

use super::Reconciler;

/// Network spec from the API.
#[derive(Debug, Clone)]
pub struct NetworkSpec {
    pub id: String,
    pub name: String,
    pub ipv4_enabled: bool,
    pub ipv4_subnet: Option<String>,
    pub ipv6_enabled: bool,
    pub ipv6_prefix: Option<String>,
    pub dns_servers: Vec<String>,
    pub ntp_servers: Vec<String>,
    pub is_public: bool,
}

/// Network status to report back.
#[derive(Debug, Clone)]
pub struct NetworkStatus {
    pub phase: NetworkPhase,
    pub message: Option<String>,
}

/// Network lifecycle phase.
#[derive(Debug, Clone, Copy)]
pub enum NetworkPhase {
    Pending,
    Creating,
    Ready,
    Updating,
    Deleting,
    Failed,
}

/// Network reconciler that interacts with mvirt-net.
pub struct NetworkReconciler {
    net_endpoint: String,
}

impl NetworkReconciler {
    pub fn new(net_endpoint: String) -> Self {
        Self { net_endpoint }
    }
}

#[async_trait]
impl Reconciler for NetworkReconciler {
    type Spec = NetworkSpec;
    type Status = NetworkStatus;

    async fn reconcile(&self, id: &str, spec: &Self::Spec) -> Result<Self::Status> {
        info!("Reconciling network {} ({})", spec.name, id);
        debug!("Network spec: {:?}", spec);

        // TODO: Connect to mvirt-net and check current state
        // For now, just return Ready status

        // 1. Get current state from mvirt-net
        // let current = self.net_client.get_network(&spec.name).await?;

        // 2. Compare with spec
        // if current.is_none() {
        //     // Create network
        //     self.net_client.create_network(spec).await?;
        //     return Ok(NetworkStatus { phase: NetworkPhase::Creating, message: None });
        // }

        // 3. Check if update needed
        // if needs_update(&current, spec) {
        //     self.net_client.update_network(spec).await?;
        //     return Ok(NetworkStatus { phase: NetworkPhase::Updating, message: None });
        // }

        Ok(NetworkStatus {
            phase: NetworkPhase::Ready,
            message: None,
        })
    }

    async fn finalize(&self, id: &str) -> Result<()> {
        info!("Finalizing (deleting) network {}", id);

        // TODO: Connect to mvirt-net and delete
        // self.net_client.delete_network(id).await?;

        Ok(())
    }
}
