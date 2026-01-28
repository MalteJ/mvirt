//! Network reconciler - networks are info-only on the node.
//!
//! Networks are not materialized on the node. They are just routing scopes
//! for NICs. The node receives NetworkSpec for informational purposes
//! (NICs inherit DNS, NTP, etc. from their network).

use anyhow::Result;
use async_trait::async_trait;
use tracing::info;

use super::Reconciler;
use crate::proto::node::NetworkSpec;

/// Placeholder status for networks (never sent to API).
pub struct NetworkStatus;

/// Network reconciler - no-op since networks are info-only.
pub struct NetworkReconciler;

impl NetworkReconciler {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Reconciler for NetworkReconciler {
    type Spec = NetworkSpec;
    type Status = NetworkStatus;

    async fn reconcile(&self, id: &str, spec: &Self::Spec) -> Result<Self::Status> {
        let name = spec
            .meta
            .as_ref()
            .map(|m| m.name.as_str())
            .unwrap_or("unknown");
        info!(
            "Network {} ({}) received (info-only, no reconciliation)",
            name, id
        );
        Ok(NetworkStatus)
    }

    async fn finalize(&self, id: &str) -> Result<()> {
        info!("Network {} removed (info-only)", id);
        Ok(())
    }
}
