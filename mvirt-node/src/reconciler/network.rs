//! Network reconciler - creates networks in mvirt-ebpf so NICs can reference them.

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::{error, info};

use super::Reconciler;
use crate::clients::NetClient;
use crate::proto::node::{NetworkSpec, ResourcePhase};

/// Network status reported back.
pub struct NetworkStatus {
    pub phase: i32,
}

/// Network reconciler that creates/deletes networks in mvirt-ebpf.
pub struct NetworkReconciler {
    net: Mutex<NetClient>,
}

impl NetworkReconciler {
    pub fn new(net: NetClient) -> Self {
        Self {
            net: Mutex::new(net),
        }
    }
}

#[async_trait]
impl Reconciler for NetworkReconciler {
    type Spec = NetworkSpec;
    type Status = NetworkStatus;

    async fn reconcile(&self, id: &str, spec: &Self::Spec) -> Result<Self::Status> {
        let meta = spec.meta.as_ref().expect("NetworkSpec must have meta");
        info!("Reconciling network {} ({})", meta.name, id);

        let mut net = self.net.lock().await;

        match net
            .create_network(
                id,
                &meta.name,
                spec.ipv4_enabled,
                &spec.ipv4_prefix,
                spec.ipv6_enabled,
                &spec.ipv6_prefix,
                spec.dns_servers.clone(),
                spec.ntp_servers.clone(),
                spec.is_public,
            )
            .await
        {
            Ok(_) => {
                info!("Network {} ({}) created in mvirt-ebpf", meta.name, id);
                Ok(NetworkStatus {
                    phase: ResourcePhase::Ready as i32,
                })
            }
            Err(e) => {
                // AlreadyExists is fine — network was already created
                let msg = format!("{:?}", e);
                if msg.contains("already exists") || msg.contains("AlreadyExists") {
                    info!(
                        "Network {} ({}) already exists in mvirt-ebpf",
                        meta.name, id
                    );
                    Ok(NetworkStatus {
                        phase: ResourcePhase::Ready as i32,
                    })
                } else {
                    error!("Failed to create network {} ({}): {:?}", meta.name, id, e);
                    Ok(NetworkStatus {
                        phase: ResourcePhase::Failed as i32,
                    })
                }
            }
        }
    }

    async fn finalize(&self, id: &str) -> Result<()> {
        info!("Finalizing (deleting) network {}", id);
        let mut net = self.net.lock().await;
        // Force delete to clean up even if NICs still reference it
        let _ = net.delete_network(id, true).await;
        Ok(())
    }
}
