//! Route reconciler - manages IP-in-IPv6 tunnel routes via kernel commands.

use anyhow::Result;
use async_trait::async_trait;
use tokio::process::Command;
use tracing::{error, info};

use super::Reconciler;
use crate::proto::node::{ResourcePhase, RouteSpec, RouteStatus};

/// Route reconciler that manages kernel routing for inter-node IP-in-IPv6 tunnels.
pub struct RouteReconciler;

impl RouteReconciler {
    pub fn new() -> Self {
        Self
    }

    /// Create or verify an IP-in-IPv6 tunnel interface.
    async fn ensure_tunnel(&self, tunnel_name: &str, remote: &str) -> Result<()> {
        // Check if tunnel already exists
        let check = Command::new("ip")
            .args(["link", "show", tunnel_name])
            .output()
            .await?;

        if check.status.success() {
            return Ok(());
        }

        // Create ip6tnl tunnel
        let output = Command::new("ip")
            .args([
                "tunnel",
                "add",
                tunnel_name,
                "mode",
                "ipip6",
                "remote",
                remote,
            ])
            .output()
            .await?;

        if !output.status.success() {
            anyhow::bail!(
                "Failed to create tunnel {}: {}",
                tunnel_name,
                String::from_utf8_lossy(&output.stderr)
            );
        }

        // Bring tunnel up
        let output = Command::new("ip")
            .args(["link", "set", tunnel_name, "up"])
            .output()
            .await?;

        if !output.status.success() {
            anyhow::bail!(
                "Failed to bring up tunnel {}: {}",
                tunnel_name,
                String::from_utf8_lossy(&output.stderr)
            );
        }

        Ok(())
    }

    /// Add a route via the tunnel interface.
    async fn add_route(&self, destination: &str, tunnel_name: &str) -> Result<()> {
        let output = Command::new("ip")
            .args(["route", "replace", destination, "dev", tunnel_name])
            .output()
            .await?;

        if !output.status.success() {
            anyhow::bail!(
                "Failed to add route {} via {}: {}",
                destination,
                tunnel_name,
                String::from_utf8_lossy(&output.stderr)
            );
        }

        Ok(())
    }

    /// Remove a route and optionally the tunnel interface.
    async fn remove_route(&self, destination: &str, tunnel_name: &str) -> Result<()> {
        // Remove route
        let _ = Command::new("ip")
            .args(["route", "del", destination, "dev", tunnel_name])
            .output()
            .await;

        // Remove tunnel interface
        let _ = Command::new("ip")
            .args(["tunnel", "del", tunnel_name])
            .output()
            .await;

        Ok(())
    }
}

#[async_trait]
impl Reconciler for RouteReconciler {
    type Spec = RouteSpec;
    type Status = RouteStatus;

    async fn reconcile(&self, id: &str, spec: &Self::Spec) -> Result<Self::Status> {
        let meta = spec.meta.as_ref().expect("RouteSpec must have meta");
        info!(
            "Reconciling route {} ({}) → {} via {}",
            meta.name, id, spec.destination, spec.tunnel_remote
        );

        let tunnel_name = format!("mvrt-{}", &id[..8.min(id.len())]);

        // Create tunnel and add route
        if let Err(e) = self.ensure_tunnel(&tunnel_name, &spec.tunnel_remote).await {
            error!("Failed to create tunnel for route {}: {}", id, e);
            return Ok(RouteStatus {
                id: id.to_string(),
                phase: ResourcePhase::Failed as i32,
                message: Some(format!("Tunnel creation failed: {}", e)),
                active: false,
            });
        }

        if let Err(e) = self.add_route(&spec.destination, &tunnel_name).await {
            error!("Failed to add route {}: {}", id, e);
            return Ok(RouteStatus {
                id: id.to_string(),
                phase: ResourcePhase::Failed as i32,
                message: Some(format!("Route add failed: {}", e)),
                active: false,
            });
        }

        Ok(RouteStatus {
            id: id.to_string(),
            phase: ResourcePhase::Ready as i32,
            message: None,
            active: true,
        })
    }

    async fn finalize(&self, id: &str) -> Result<()> {
        info!("Finalizing (deleting) route {}", id);
        let tunnel_name = format!("mvrt-{}", &id[..8.min(id.len())]);
        // Best-effort removal; destination unknown here, just remove tunnel
        let _ = Command::new("ip")
            .args(["tunnel", "del", &tunnel_name])
            .output()
            .await;
        Ok(())
    }
}
