//! Security group reconciler - manages firewall rules via mvirt-net.

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::{error, info};

use super::Reconciler;
use crate::clients::NetClient;
use crate::proto::node::{ResourcePhase, SecurityGroupSpec, SecurityGroupStatus};

/// Security group reconciler that interacts with mvirt-net.
pub struct SecurityGroupReconciler {
    net: Mutex<NetClient>,
}

impl SecurityGroupReconciler {
    pub fn new(net: NetClient) -> Self {
        Self {
            net: Mutex::new(net),
        }
    }
}

#[async_trait]
impl Reconciler for SecurityGroupReconciler {
    type Spec = SecurityGroupSpec;
    type Status = SecurityGroupStatus;

    async fn reconcile(&self, id: &str, spec: &Self::Spec) -> Result<Self::Status> {
        let meta = spec
            .meta
            .as_ref()
            .expect("SecurityGroupSpec must have meta");
        info!("Reconciling security group {} ({})", meta.name, id);

        let mut net = self.net.lock().await;

        // Create the security group
        match net.create_security_group(&meta.name, "").await {
            Ok(sg) => {
                // Add all rules
                for rule in &spec.rules {
                    let cidr = match &rule.target {
                        Some(crate::proto::node::security_rule::Target::Cidr(c)) => c.clone(),
                        _ => String::new(),
                    };
                    if let Err(e) = net
                        .add_rule(
                            &sg.id,
                            rule.direction,
                            rule.protocol,
                            rule.port_start.unwrap_or(0),
                            rule.port_end.unwrap_or(0),
                            &cidr,
                        )
                        .await
                    {
                        error!("Failed to add rule to SG {}: {}", id, e);
                    }
                }

                Ok(SecurityGroupStatus {
                    id: id.to_string(),
                    phase: ResourcePhase::Ready as i32,
                    message: None,
                    rule_count: spec.rules.len() as u32,
                })
            }
            Err(e) => {
                error!("Failed to create security group {}: {}", id, e);
                Ok(SecurityGroupStatus {
                    id: id.to_string(),
                    phase: ResourcePhase::Failed as i32,
                    message: Some(format!("Failed to create: {}", e)),
                    rule_count: 0,
                })
            }
        }
    }

    async fn finalize(&self, id: &str) -> Result<()> {
        info!("Finalizing (deleting) security group {}", id);
        let mut net = self.net.lock().await;
        net.delete_security_group(id, true).await?;
        Ok(())
    }
}
