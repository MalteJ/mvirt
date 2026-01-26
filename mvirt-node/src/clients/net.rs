//! Client for mvirt-net daemon.

use anyhow::Result;
use tracing::debug;

/// Client for interacting with mvirt-net.
pub struct NetClient {
    endpoint: String,
}

impl NetClient {
    pub fn new(endpoint: String) -> Self {
        Self { endpoint }
    }

    /// Check if connected to mvirt-net.
    pub async fn health_check(&self) -> Result<bool> {
        debug!("Health check for mvirt-net at {}", self.endpoint);
        // TODO: Implement actual health check
        Ok(true)
    }

    /// Create a network.
    pub async fn create_network(
        &self,
        name: &str,
        ipv4_subnet: Option<&str>,
        ipv6_prefix: Option<&str>,
    ) -> Result<()> {
        debug!("Creating network {} in mvirt-net", name);
        // TODO: Implement via gRPC
        Ok(())
    }

    /// Delete a network.
    pub async fn delete_network(&self, name: &str) -> Result<()> {
        debug!("Deleting network {} in mvirt-net", name);
        // TODO: Implement via gRPC
        Ok(())
    }

    /// Create a NIC.
    pub async fn create_nic(
        &self,
        network_name: &str,
        mac_address: &str,
        ipv4_address: Option<&str>,
    ) -> Result<String> {
        debug!("Creating NIC in network {} in mvirt-net", network_name);
        // TODO: Implement via gRPC
        // Returns socket path
        Ok(format!(
            "/run/mvirt/nics/{}.sock",
            mac_address.replace(':', "")
        ))
    }

    /// Delete a NIC.
    pub async fn delete_nic(&self, nic_id: &str) -> Result<()> {
        debug!("Deleting NIC {} in mvirt-net", nic_id);
        // TODO: Implement via gRPC
        Ok(())
    }
}
