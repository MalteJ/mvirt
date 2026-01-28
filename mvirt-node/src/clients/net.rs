//! Client for mvirt-net daemon.

use anyhow::{Context, Result};
use tonic::transport::Channel;
use tracing::debug;

use crate::proto::net::{
    net_service_client::NetServiceClient, AddSecurityGroupRuleRequest, AttachNicRequest,
    AttachSecurityGroupRequest, CreateNetworkRequest, CreateNicRequest, CreateSecurityGroupRequest,
    DeleteNetworkRequest, DeleteNicRequest, DeleteSecurityGroupRequest, DetachSecurityGroupRequest,
    GetNicRequest, RemoveSecurityGroupRuleRequest,
};

pub use crate::proto::net::{
    get_nic_request, Network, Nic, NicState, RuleDirection, RuleProtocol, SecurityGroup,
    SecurityGroupRule,
};

/// Client for interacting with mvirt-net.
#[derive(Clone)]
pub struct NetClient {
    client: NetServiceClient<Channel>,
}

impl NetClient {
    pub async fn connect(endpoint: &str) -> Result<Self> {
        let client = NetServiceClient::connect(endpoint.to_string())
            .await
            .context("Failed to connect to mvirt-net")?;
        Ok(Self { client })
    }

    // === Network operations ===

    /// Create a network.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_network(
        &mut self,
        name: &str,
        ipv4_enabled: bool,
        ipv4_subnet: &str,
        ipv6_enabled: bool,
        ipv6_prefix: &str,
        dns_servers: Vec<String>,
        ntp_servers: Vec<String>,
        is_public: bool,
    ) -> Result<Network> {
        debug!("Creating network {} in mvirt-net", name);
        let resp = self
            .client
            .create_network(CreateNetworkRequest {
                name: name.to_string(),
                ipv4_enabled,
                ipv4_subnet: ipv4_subnet.to_string(),
                ipv6_enabled,
                ipv6_prefix: ipv6_prefix.to_string(),
                dns_servers,
                ntp_servers,
                is_public,
            })
            .await
            .context("Failed to create network")?;
        Ok(resp.into_inner())
    }

    /// Delete a network.
    pub async fn delete_network(&mut self, id: &str, force: bool) -> Result<()> {
        debug!("Deleting network {} in mvirt-net", id);
        self.client
            .delete_network(DeleteNetworkRequest {
                id: id.to_string(),
                force,
            })
            .await
            .context("Failed to delete network")?;
        Ok(())
    }

    // === NIC operations ===

    /// Get NIC by ID.
    pub async fn get_nic(&mut self, id: &str) -> Result<Option<Nic>> {
        debug!("Getting NIC {} from mvirt-net", id);
        match self
            .client
            .get_nic(GetNicRequest {
                identifier: Some(get_nic_request::Identifier::Id(id.to_string())),
            })
            .await
        {
            Ok(resp) => Ok(Some(resp.into_inner())),
            Err(status) if status.code() == tonic::Code::NotFound => Ok(None),
            Err(e) => Err(e).context("Failed to get NIC"),
        }
    }

    /// Create a NIC.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_nic(
        &mut self,
        network_id: &str,
        name: &str,
        mac_address: &str,
        ipv4_address: &str,
        ipv6_address: &str,
        routed_ipv4_prefixes: Vec<String>,
        routed_ipv6_prefixes: Vec<String>,
    ) -> Result<Nic> {
        debug!(
            "Creating NIC {} in network {} in mvirt-net",
            name, network_id
        );
        let resp = self
            .client
            .create_nic(CreateNicRequest {
                network_id: network_id.to_string(),
                name: name.to_string(),
                mac_address: mac_address.to_string(),
                ipv4_address: ipv4_address.to_string(),
                ipv6_address: ipv6_address.to_string(),
                routed_ipv4_prefixes,
                routed_ipv6_prefixes,
            })
            .await
            .context("Failed to create NIC")?;
        Ok(resp.into_inner())
    }

    /// Delete a NIC.
    pub async fn delete_nic(&mut self, id: &str) -> Result<()> {
        debug!("Deleting NIC {} in mvirt-net", id);
        self.client
            .delete_nic(DeleteNicRequest { id: id.to_string() })
            .await
            .context("Failed to delete NIC")?;
        Ok(())
    }

    /// Attach a NIC (trigger TAP attachment when VM starts).
    pub async fn attach_nic(&mut self, id: &str) -> Result<()> {
        debug!("Attaching NIC {} in mvirt-net", id);
        self.client
            .attach_nic(AttachNicRequest { id: id.to_string() })
            .await
            .context("Failed to attach NIC")?;
        Ok(())
    }

    // === Security Group operations ===

    /// Create a security group.
    pub async fn create_security_group(
        &mut self,
        name: &str,
        description: &str,
    ) -> Result<SecurityGroup> {
        debug!("Creating security group {} in mvirt-net", name);
        let resp = self
            .client
            .create_security_group(CreateSecurityGroupRequest {
                name: name.to_string(),
                description: description.to_string(),
            })
            .await
            .context("Failed to create security group")?;
        Ok(resp.into_inner())
    }

    /// Delete a security group.
    pub async fn delete_security_group(&mut self, id: &str, force: bool) -> Result<()> {
        debug!("Deleting security group {} in mvirt-net", id);
        self.client
            .delete_security_group(DeleteSecurityGroupRequest {
                id: id.to_string(),
                force,
            })
            .await
            .context("Failed to delete security group")?;
        Ok(())
    }

    /// Add a rule to a security group.
    pub async fn add_rule(
        &mut self,
        security_group_id: &str,
        direction: i32,
        protocol: i32,
        port_start: u32,
        port_end: u32,
        cidr: &str,
    ) -> Result<SecurityGroupRule> {
        debug!(
            "Adding rule to security group {} in mvirt-net",
            security_group_id
        );
        let resp = self
            .client
            .add_security_group_rule(AddSecurityGroupRuleRequest {
                security_group_id: security_group_id.to_string(),
                direction,
                protocol,
                port_start,
                port_end,
                cidr: cidr.to_string(),
                description: String::new(),
            })
            .await
            .context("Failed to add security group rule")?;
        Ok(resp.into_inner())
    }

    /// Remove a rule from a security group.
    pub async fn remove_rule(&mut self, rule_id: &str) -> Result<()> {
        debug!("Removing rule {} from security group in mvirt-net", rule_id);
        self.client
            .remove_security_group_rule(RemoveSecurityGroupRuleRequest {
                rule_id: rule_id.to_string(),
            })
            .await
            .context("Failed to remove security group rule")?;
        Ok(())
    }

    /// Attach a security group to a NIC.
    pub async fn attach_security_group(&mut self, nic_id: &str, sg_id: &str) -> Result<()> {
        debug!(
            "Attaching security group {} to NIC {} in mvirt-net",
            sg_id, nic_id
        );
        self.client
            .attach_security_group(AttachSecurityGroupRequest {
                nic_id: nic_id.to_string(),
                security_group_id: sg_id.to_string(),
            })
            .await
            .context("Failed to attach security group")?;
        Ok(())
    }

    /// Detach a security group from a NIC.
    pub async fn detach_security_group(&mut self, nic_id: &str, sg_id: &str) -> Result<()> {
        debug!(
            "Detaching security group {} from NIC {} in mvirt-net",
            sg_id, nic_id
        );
        self.client
            .detach_security_group(DetachSecurityGroupRequest {
                nic_id: nic_id.to_string(),
                security_group_id: sg_id.to_string(),
            })
            .await
            .context("Failed to detach security group")?;
        Ok(())
    }
}
