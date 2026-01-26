//! DataStore trait definitions.
//!
//! These traits abstract away the underlying Raft implementation,
//! allowing handlers to work with domain objects instead of commands.

use async_trait::async_trait;
use tokio::sync::broadcast;

use crate::command::{
    NetworkData, NicData, NodeData, NodeResources, NodeStatus, VmData, VmDesiredState, VmSpec,
    VmStatus,
};
use std::collections::HashMap;

use super::error::Result;
use super::event::Event;

// =============================================================================
// Node Request DTOs
// =============================================================================

/// Request to register a new node.
#[derive(Debug, Clone)]
pub struct RegisterNodeRequest {
    pub name: String,
    pub address: String,
    pub resources: NodeResources,
    pub labels: HashMap<String, String>,
}

/// Request to update node status.
#[derive(Debug, Clone)]
pub struct UpdateNodeStatusRequest {
    pub status: NodeStatus,
    pub resources: Option<NodeResources>,
}

// =============================================================================
// Network Request DTOs
// =============================================================================

/// Request to create a new network.
#[derive(Debug, Clone)]
pub struct CreateNetworkRequest {
    pub name: String,
    pub ipv4_enabled: bool,
    pub ipv4_subnet: Option<String>,
    pub ipv6_enabled: bool,
    pub ipv6_prefix: Option<String>,
    pub dns_servers: Vec<String>,
    pub ntp_servers: Vec<String>,
    pub is_public: bool,
}

/// Request to update a network.
#[derive(Debug, Clone)]
pub struct UpdateNetworkRequest {
    pub dns_servers: Vec<String>,
    pub ntp_servers: Vec<String>,
}

/// Result of deleting a network.
#[derive(Debug, Clone)]
pub struct DeleteNetworkResult {
    pub nics_deleted: u32,
}

/// Request to create a new NIC.
#[derive(Debug, Clone)]
pub struct CreateNicRequest {
    pub network_id: String,
    pub name: Option<String>,
    pub mac_address: Option<String>,
    pub ipv4_address: Option<String>,
    pub ipv6_address: Option<String>,
    pub routed_ipv4_prefixes: Vec<String>,
    pub routed_ipv6_prefixes: Vec<String>,
}

/// Request to update a NIC.
#[derive(Debug, Clone)]
pub struct UpdateNicRequest {
    pub routed_ipv4_prefixes: Vec<String>,
    pub routed_ipv6_prefixes: Vec<String>,
}

// =============================================================================
// VM Request DTOs
// =============================================================================

/// Request to create a new VM.
#[derive(Debug, Clone)]
pub struct CreateVmRequest {
    pub spec: VmSpec,
}

/// Request to update a VM's spec (desired state).
#[derive(Debug, Clone)]
pub struct UpdateVmSpecRequest {
    pub desired_state: VmDesiredState,
}

/// Request to update a VM's status (from node).
#[derive(Debug, Clone)]
pub struct UpdateVmStatusRequest {
    pub status: VmStatus,
}

// =============================================================================
// Cluster DTOs
// =============================================================================

/// Cluster information.
#[derive(Debug, Clone)]
pub struct ClusterInfo {
    pub cluster_id: String,
    pub leader_id: Option<u64>,
    pub current_term: u64,
    pub commit_index: u64,
    pub node_id: u64,
    pub is_leader: bool,
}

/// Cluster membership information.
#[derive(Debug, Clone)]
pub struct Membership {
    pub voters: Vec<u64>,
    pub learners: Vec<u64>,
    pub nodes: Vec<MembershipNode>,
}

/// Node in membership.
#[derive(Debug, Clone)]
pub struct MembershipNode {
    pub id: u64,
    pub address: String,
    pub role: String,
}

// =============================================================================
// Domain Store Traits
// =============================================================================

/// Store trait for node (hypervisor agent) operations.
#[async_trait]
pub trait NodeStore: Send + Sync {
    /// List all nodes.
    async fn list_nodes(&self) -> Result<Vec<NodeData>>;

    /// List online nodes only.
    async fn list_online_nodes(&self) -> Result<Vec<NodeData>>;

    /// Get a node by ID.
    async fn get_node(&self, id: &str) -> Result<Option<NodeData>>;

    /// Get a node by name.
    async fn get_node_by_name(&self, name: &str) -> Result<Option<NodeData>>;

    /// Register a new node.
    async fn register_node(&self, req: RegisterNodeRequest) -> Result<NodeData>;

    /// Update node status (heartbeat).
    async fn update_node_status(&self, id: &str, req: UpdateNodeStatusRequest) -> Result<NodeData>;

    /// Deregister a node.
    async fn deregister_node(&self, id: &str) -> Result<()>;
}

/// Store trait for network operations.
#[async_trait]
pub trait NetworkStore: Send + Sync {
    /// List all networks.
    async fn list_networks(&self) -> Result<Vec<NetworkData>>;

    /// Get a network by ID.
    async fn get_network(&self, id: &str) -> Result<Option<NetworkData>>;

    /// Get a network by name.
    async fn get_network_by_name(&self, name: &str) -> Result<Option<NetworkData>>;

    /// Create a new network.
    async fn create_network(&self, req: CreateNetworkRequest) -> Result<NetworkData>;

    /// Update a network.
    async fn update_network(&self, id: &str, req: UpdateNetworkRequest) -> Result<NetworkData>;

    /// Delete a network.
    async fn delete_network(&self, id: &str, force: bool) -> Result<DeleteNetworkResult>;
}

/// Store trait for NIC operations.
#[async_trait]
pub trait NicStore: Send + Sync {
    /// List all NICs, optionally filtered by network.
    async fn list_nics(&self, network_id: Option<&str>) -> Result<Vec<NicData>>;

    /// Get a NIC by ID.
    async fn get_nic(&self, id: &str) -> Result<Option<NicData>>;

    /// Get a NIC by name.
    async fn get_nic_by_name(&self, name: &str) -> Result<Option<NicData>>;

    /// Create a new NIC.
    async fn create_nic(&self, req: CreateNicRequest) -> Result<NicData>;

    /// Update a NIC.
    async fn update_nic(&self, id: &str, req: UpdateNicRequest) -> Result<NicData>;

    /// Delete a NIC.
    async fn delete_nic(&self, id: &str) -> Result<()>;
}

/// Store trait for VM operations.
#[async_trait]
pub trait VmStore: Send + Sync {
    /// List all VMs.
    async fn list_vms(&self) -> Result<Vec<VmData>>;

    /// List VMs on a specific node.
    async fn list_vms_by_node(&self, node_id: &str) -> Result<Vec<VmData>>;

    /// Get a VM by ID.
    async fn get_vm(&self, id: &str) -> Result<Option<VmData>>;

    /// Get a VM by name.
    async fn get_vm_by_name(&self, name: &str) -> Result<Option<VmData>>;

    /// Create a new VM.
    async fn create_vm(&self, req: CreateVmRequest) -> Result<VmData>;

    /// Create a new VM and schedule it to a node.
    ///
    /// This combines VM creation with immediate scheduling.
    /// The VM is created in Pending state, then scheduled to a node.
    async fn create_and_schedule_vm(&self, req: CreateVmRequest) -> Result<VmData>;

    /// Update a VM's spec (desired state).
    async fn update_vm_spec(&self, id: &str, req: UpdateVmSpecRequest) -> Result<VmData>;

    /// Update a VM's status (from node).
    async fn update_vm_status(&self, id: &str, req: UpdateVmStatusRequest) -> Result<VmData>;

    /// Delete a VM.
    async fn delete_vm(&self, id: &str) -> Result<()>;
}

/// Store trait for cluster operations.
#[async_trait]
pub trait ClusterStore: Send + Sync {
    /// Get cluster information.
    async fn get_cluster_info(&self) -> Result<ClusterInfo>;

    /// Get cluster membership.
    async fn get_membership(&self) -> Result<Membership>;

    /// Create a join token for a new node.
    async fn create_join_token(&self, node_id: u64, valid_for_secs: u64) -> Result<String>;

    /// Remove a node from the cluster.
    async fn remove_node(&self, node_id: u64) -> Result<()>;
}

// =============================================================================
// Composite DataStore Trait
// =============================================================================

/// Composite data store trait combining all domain stores.
///
/// This is the main trait that handlers should use. It provides:
/// - Node (hypervisor) registration and status
/// - Network CRUD operations
/// - NIC CRUD operations
/// - VM CRUD operations
/// - Cluster management operations
/// - Event subscription for real-time updates
pub trait DataStore:
    NodeStore + NetworkStore + NicStore + VmStore + ClusterStore + Send + Sync
{
    /// Subscribe to state change events.
    ///
    /// Returns a broadcast receiver that will receive events when
    /// networks or NICs are created, updated, or deleted.
    fn subscribe(&self) -> broadcast::Receiver<Event>;
}
