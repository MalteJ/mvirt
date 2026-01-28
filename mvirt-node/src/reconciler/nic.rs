//! NIC reconciler - reconciles NIC specs with mvirt-net.

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::{error, info};

use super::Reconciler;
use crate::clients::NetClient;
use crate::proto::node::{NicSpec, NicStatus, ResourcePhase};

/// NIC reconciler that interacts with mvirt-net.
pub struct NicReconciler {
    net: Mutex<NetClient>,
}

impl NicReconciler {
    pub fn new(net: NetClient) -> Self {
        Self {
            net: Mutex::new(net),
        }
    }
}

#[async_trait]
impl Reconciler for NicReconciler {
    type Spec = NicSpec;
    type Status = NicStatus;

    async fn reconcile(&self, id: &str, spec: &Self::Spec) -> Result<Self::Status> {
        info!("Reconciling NIC {} in network {}", id, spec.network_id);

        let mut net = self.net.lock().await;

        // Check if NIC already exists
        match net.get_nic(id).await? {
            Some(nic) => {
                // NIC exists, report its current state
                Ok(NicStatus {
                    id: id.to_string(),
                    phase: ResourcePhase::Ready as i32,
                    message: None,
                    socket_path: nic.socket_path,
                })
            }
            None => {
                // NIC doesn't exist, create it
                let meta = spec.meta.as_ref().expect("NicSpec must have meta");
                match net
                    .create_nic(
                        &spec.network_id,
                        &meta.name,
                        &spec.mac_address,
                        &spec.ipv4_address.clone().unwrap_or_default(),
                        &spec.ipv6_address.clone().unwrap_or_default(),
                        spec.routed_ipv4_prefixes.clone(),
                        spec.routed_ipv6_prefixes.clone(),
                    )
                    .await
                {
                    Ok(nic) => {
                        // If security group is set, attach it
                        if !spec.security_group_id.is_empty() {
                            if let Err(e) = net
                                .attach_security_group(&nic.id, &spec.security_group_id)
                                .await
                            {
                                error!("Failed to attach security group: {}", e);
                            }
                        }

                        Ok(NicStatus {
                            id: id.to_string(),
                            phase: ResourcePhase::Ready as i32,
                            message: None,
                            socket_path: nic.socket_path,
                        })
                    }
                    Err(e) => {
                        error!("Failed to create NIC {}: {}", id, e);
                        Ok(NicStatus {
                            id: id.to_string(),
                            phase: ResourcePhase::Failed as i32,
                            message: Some(format!("Failed to create: {}", e)),
                            socket_path: String::new(),
                        })
                    }
                }
            }
        }
    }

    async fn finalize(&self, id: &str) -> Result<()> {
        info!("Finalizing (deleting) NIC {}", id);
        let mut net = self.net.lock().await;
        net.delete_nic(id).await?;
        Ok(())
    }
}
