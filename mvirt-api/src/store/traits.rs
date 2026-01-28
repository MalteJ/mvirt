//! DataStore trait definitions.
//!
//! These traits abstract away the underlying Raft implementation,
//! allowing handlers to work with domain objects instead of commands.

use async_trait::async_trait;
use tokio::sync::broadcast;

use crate::command::{
    ImportJobData, ImportJobState, NetworkData, NicData, NodeData, NodeResources, NodeStatus,
    ProjectData, RuleDirection, SecurityGroupData, TemplateData, VmData, VmDesiredState, VmSpec,
    VmStatus, VolumeData,
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
    pub project_id: String,
    pub name: String,
    pub ipv4_enabled: bool,
    pub ipv4_prefix: Option<String>,
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
    pub project_id: String,
    pub network_id: String,
    pub name: Option<String>,
    pub mac_address: Option<String>,
    pub ipv4_address: Option<String>,
    pub ipv6_address: Option<String>,
    pub routed_ipv4_prefixes: Vec<String>,
    pub routed_ipv6_prefixes: Vec<String>,
    pub security_group_id: Option<String>,
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
// Project Request DTOs
// =============================================================================

/// Request to create a new project.
#[derive(Debug, Clone)]
pub struct CreateProjectRequest {
    /// User-provided project ID (must be unique, lowercase alphanumeric)
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

// =============================================================================
// Volume Request DTOs
// =============================================================================

/// Request to create a new volume.
#[derive(Debug, Clone)]
pub struct CreateVolumeRequest {
    pub project_id: String,
    pub node_id: String, // Caller picks node (Shared Nothing)
    pub name: String,
    pub size_bytes: u64,
    pub template_id: Option<String>, // Clone from template
}

/// Request to resize a volume.
#[derive(Debug, Clone)]
pub struct ResizeVolumeRequest {
    pub size_bytes: u64,
}

/// Request to create a snapshot.
#[derive(Debug, Clone)]
pub struct CreateSnapshotRequest {
    pub name: String,
}

// =============================================================================
// Template Request DTOs
// =============================================================================

/// Request to import a template.
#[derive(Debug, Clone)]
pub struct ImportTemplateRequest {
    pub project_id: String,
    pub node_id: String,
    pub name: String,
    pub url: String,
    pub total_bytes: u64,
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

    /// List networks by project.
    async fn list_networks_by_project(&self, project_id: &str) -> Result<Vec<NetworkData>>;

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

    /// List NICs by project.
    async fn list_nics_by_project(&self, project_id: &str) -> Result<Vec<NicData>>;

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

    /// Attach a NIC to a VM.
    async fn attach_nic(&self, id: &str, vm_id: &str) -> Result<NicData>;

    /// Detach a NIC from a VM.
    async fn detach_nic(&self, id: &str) -> Result<NicData>;
}

/// Store trait for VM operations.
#[async_trait]
pub trait VmStore: Send + Sync {
    /// List all VMs.
    async fn list_vms(&self) -> Result<Vec<VmData>>;

    /// List VMs by project.
    async fn list_vms_by_project(&self, project_id: &str) -> Result<Vec<VmData>>;

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

/// Store trait for project operations.
#[async_trait]
pub trait ProjectStore: Send + Sync {
    /// List all projects.
    async fn list_projects(&self) -> Result<Vec<ProjectData>>;

    /// Get a project by ID.
    async fn get_project(&self, id: &str) -> Result<Option<ProjectData>>;

    /// Get a project by name.
    async fn get_project_by_name(&self, name: &str) -> Result<Option<ProjectData>>;

    /// Create a new project.
    async fn create_project(&self, req: CreateProjectRequest) -> Result<ProjectData>;

    /// Delete a project.
    async fn delete_project(&self, id: &str) -> Result<()>;
}

/// Store trait for volume operations.
#[async_trait]
pub trait VolumeStore: Send + Sync {
    /// List all volumes, optionally filtered by project or node.
    async fn list_volumes(
        &self,
        project_id: Option<&str>,
        node_id: Option<&str>,
    ) -> Result<Vec<VolumeData>>;

    /// Get a volume by ID.
    async fn get_volume(&self, id: &str) -> Result<Option<VolumeData>>;

    /// Create a new volume.
    async fn create_volume(&self, req: CreateVolumeRequest) -> Result<VolumeData>;

    /// Delete a volume.
    async fn delete_volume(&self, id: &str) -> Result<()>;

    /// Resize a volume.
    async fn resize_volume(&self, id: &str, req: ResizeVolumeRequest) -> Result<VolumeData>;

    /// Create a snapshot on a volume.
    async fn create_snapshot(
        &self,
        volume_id: &str,
        req: CreateSnapshotRequest,
    ) -> Result<VolumeData>;
}

/// Store trait for template operations.
#[async_trait]
pub trait TemplateStore: Send + Sync {
    /// List all templates, optionally filtered by node.
    async fn list_templates(&self, node_id: Option<&str>) -> Result<Vec<TemplateData>>;

    /// List templates by project.
    async fn list_templates_by_project(&self, project_id: &str) -> Result<Vec<TemplateData>>;

    /// Get a template by ID.
    async fn get_template(&self, id: &str) -> Result<Option<TemplateData>>;

    /// Import a template (starts an import job).
    async fn import_template(&self, req: ImportTemplateRequest) -> Result<ImportJobData>;

    /// Get an import job by ID.
    async fn get_import_job(&self, id: &str) -> Result<Option<ImportJobData>>;

    /// Update an import job's progress.
    async fn update_import_job(
        &self,
        id: &str,
        bytes_written: u64,
        state: ImportJobState,
        error: Option<String>,
    ) -> Result<ImportJobData>;
}

// =============================================================================
// Security Group Request DTOs
// =============================================================================

/// Request to create a new security group.
#[derive(Debug, Clone)]
pub struct CreateSecurityGroupRequest {
    pub project_id: String,
    pub name: String,
    pub description: Option<String>,
}

/// Request to create a security group rule.
#[derive(Debug, Clone)]
pub struct CreateSecurityGroupRuleRequest {
    pub direction: RuleDirection,
    pub protocol: Option<String>,
    pub port_range_start: Option<u16>,
    pub port_range_end: Option<u16>,
    pub cidr: Option<String>,
    pub description: Option<String>,
}

/// Store trait for security group operations.
#[async_trait]
pub trait SecurityGroupStore: Send + Sync {
    /// List all security groups, optionally filtered by project.
    async fn list_security_groups(
        &self,
        project_id: Option<&str>,
    ) -> Result<Vec<SecurityGroupData>>;

    /// Get a security group by ID.
    async fn get_security_group(&self, id: &str) -> Result<Option<SecurityGroupData>>;

    /// Create a new security group.
    async fn create_security_group(
        &self,
        req: CreateSecurityGroupRequest,
    ) -> Result<SecurityGroupData>;

    /// Delete a security group.
    async fn delete_security_group(&self, id: &str) -> Result<()>;

    /// Create a rule in a security group.
    async fn create_security_group_rule(
        &self,
        security_group_id: &str,
        req: CreateSecurityGroupRuleRequest,
    ) -> Result<SecurityGroupData>;

    /// Delete a rule from a security group.
    async fn delete_security_group_rule(
        &self,
        security_group_id: &str,
        rule_id: &str,
    ) -> Result<SecurityGroupData>;
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
/// - Project CRUD operations
/// - Volume CRUD operations
/// - Template and import operations
/// - Cluster management operations
/// - Event subscription for real-time updates
pub trait DataStore:
    NodeStore
    + NetworkStore
    + NicStore
    + VmStore
    + ProjectStore
    + VolumeStore
    + TemplateStore
    + SecurityGroupStore
    + ClusterStore
    + Send
    + Sync
{
    /// Subscribe to state change events.
    ///
    /// Returns a broadcast receiver that will receive events when
    /// networks or NICs are created, updated, or deleted.
    fn subscribe(&self) -> broadcast::Receiver<Event>;
}
