//! DataStore trait definitions.
//!
//! These traits abstract away the underlying Raft implementation,
//! allowing handlers to work with domain objects instead of commands.

use async_trait::async_trait;
use tokio::sync::broadcast;

use crate::command::{
    AccountData, ClusterData, MembershipData, MembershipScope, NetworkData, NicData, NodeData,
    NodeResources, NodeStatus, OrgContact, OrgData, ProjectData, Role, RuleDirection,
    SecurityGroupData, TemplateData, TemplatePhase, VmData, VmDesiredState, VmSpec, VmStatus,
    VolumeData,
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
    pub project_slug: String,
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
    pub project_slug: String,
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

/// Request to create a new Org.
#[derive(Debug, Clone)]
pub struct CreateOrgRequest {
    /// URL identifier (kebab-case, platform-wide unique, immutable).
    pub slug: String,
    /// Display name (mutable).
    pub name: String,
}

/// Request to update an Org. All fields optional; unset fields are unchanged.
/// `contact` is the new full contact record — the handler builds it by
/// merging the user-supplied patch over the current value.
#[derive(Debug, Clone, Default)]
pub struct UpdateOrgRequest {
    pub name: Option<String>,
    pub contact: Option<OrgContact>,
}

/// Request to create a new project.
#[derive(Debug, Clone)]
pub struct CreateProjectRequest {
    /// Parent Org id (resolved by the handler from the URL path).
    pub org_slug: String,
    /// URL identifier (kebab-case, platform-wide unique, immutable). The "namespace name".
    pub slug: String,
    pub name: String,
    pub description: Option<String>,
}

// =============================================================================
// Cluster Request DTOs (ADR-0005)
// =============================================================================

/// Request to create a new Cluster within an Org.
#[derive(Debug, Clone)]
pub struct CreateClusterRequest {
    /// Parent Org slug (resolved by the handler from the URL path).
    pub org_slug: String,
    /// URL identifier (kebab-case, platform-wide unique, immutable).
    pub slug: String,
    pub name: String,
    pub description: Option<String>,
    pub location: Option<String>,
}

/// Request to patch a Cluster's mutable fields. All fields optional;
/// `None` leaves a field unchanged. `Some(None)` clears `description`/
/// `location`.
#[derive(Debug, Clone, Default)]
pub struct UpdateClusterRequest {
    pub name: Option<String>,
    pub description: Option<Option<String>>,
    pub location: Option<Option<String>>,
}

// =============================================================================
// Node-onboarding Request DTOs (ADR-0006)
// =============================================================================

/// Operator-supplied parameters for issuing a new onboarding token.
#[derive(Debug, Clone)]
pub struct CreateOnboardingTokenRequest {
    pub cluster_slug: String,
    /// Display hostname for the node. Required — pre-allocates a Node row
    /// with `status: Onboarding` so the operator sees the host in the
    /// cluster's node list immediately.
    pub hostname: String,
    pub description: Option<String>,
    /// TTL in seconds; clamped server-side.
    pub ttl_seconds: u64,
    pub created_by_account: String,
}

/// Wire-format inputs of a redeem request, validated by the handler before
/// going to raft.
#[derive(Debug, Clone)]
pub struct RedeemOnboardingTokenRequest {
    pub token: String,
    pub csr_pem: String,
    pub hostname: String,
    pub agent_version: String,
    pub kernel_version: String,
    pub arch: String,
}

/// Result of a successful redeem.
#[derive(Debug, Clone)]
pub struct BootstrapOutcome {
    pub node_id: String,
    pub cluster_slug: String,
    pub client_cert_pem: String,
    pub ca_cert_pem: String,
    pub cert_not_after: String,
}

// =============================================================================
// Volume Request DTOs
// =============================================================================

/// Request to create a new volume.
#[derive(Debug, Clone)]
pub struct CreateVolumeRequest {
    pub project_slug: String,
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

/// Request to create a template (optionally with source_url for import).
#[derive(Debug, Clone)]
pub struct CreateTemplateRequest {
    pub project_slug: String,
    pub node_id: String,
    pub name: String,
    pub size_bytes: u64,
    pub source_url: Option<String>,
    pub total_bytes: u64,
}

/// Request to update template import status.
#[derive(Debug, Clone)]
pub struct UpdateTemplateStatusRequest {
    pub phase: TemplatePhase,
    pub bytes_written: u64,
    pub size_bytes: u64,
    pub error: Option<String>,
}

// =============================================================================
// Controlplane DTOs
// =============================================================================

/// Control plane information.
#[derive(Debug, Clone)]
pub struct ControlplaneInfo {
    pub cluster_id: String,
    pub leader_id: Option<u64>,
    pub current_term: u64,
    pub commit_index: u64,
    pub peer_id: u64,
    pub is_leader: bool,
}

/// Control plane membership information.
#[derive(Debug, Clone)]
pub struct Membership {
    pub voters: Vec<u64>,
    pub learners: Vec<u64>,
    pub peers: Vec<MembershipPeer>,
}

/// Peer in membership.
#[derive(Debug, Clone)]
pub struct MembershipPeer {
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

    /// Idempotent register with a caller-supplied id. Used by the reverse-tunnel
    /// handshake — the node sends its stable id in Identify and we upsert by it,
    /// so reconnects don't fault on the name-uniqueness check.
    async fn upsert_node(&self, id: &str, req: RegisterNodeRequest) -> Result<NodeData>;

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
    async fn list_networks_by_project(&self, project_slug: &str) -> Result<Vec<NetworkData>>;

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
    async fn list_nics_by_project(&self, project_slug: &str) -> Result<Vec<NicData>>;

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
    async fn list_vms_by_project(&self, project_slug: &str) -> Result<Vec<VmData>>;

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

/// Store trait for control plane operations.
#[async_trait]
pub trait ControlplaneStore: Send + Sync {
    /// Get control plane information.
    async fn get_controlplane_info(&self) -> Result<ControlplaneInfo>;

    /// Get control plane membership.
    async fn get_membership(&self) -> Result<Membership>;

    /// Create a join token for a new peer.
    async fn create_join_token(&self, peer_id: u64, valid_for_secs: u64) -> Result<String>;

    /// Remove a peer from the control plane.
    async fn remove_peer(&self, peer_id: u64) -> Result<()>;
}

/// Store trait for Org operations. See ADR-0004. Org is keyed by slug.
#[async_trait]
pub trait OrgStore: Send + Sync {
    async fn list_orgs(&self) -> Result<Vec<OrgData>>;
    async fn get_org(&self, slug: &str) -> Result<Option<OrgData>>;
    async fn create_org(&self, req: CreateOrgRequest) -> Result<OrgData>;
    async fn update_org(&self, slug: &str, req: UpdateOrgRequest) -> Result<OrgData>;
    /// Delete an Org. Rejects with Conflict if the Org still has Projects.
    async fn delete_org(&self, slug: &str) -> Result<()>;
}

/// Store trait for project operations. Project is keyed by slug (the K8s-style
/// "namespace name").
#[async_trait]
pub trait ProjectStore: Send + Sync {
    async fn list_projects(&self) -> Result<Vec<ProjectData>>;
    async fn list_projects_by_org(&self, org_slug: &str) -> Result<Vec<ProjectData>>;
    async fn get_project(&self, slug: &str) -> Result<Option<ProjectData>>;
    async fn create_project(&self, req: CreateProjectRequest) -> Result<ProjectData>;
    async fn delete_project(&self, slug: &str) -> Result<()>;
}

/// Store trait for Cluster operations. Cluster is keyed by slug (platform-wide
/// unique). See ADR-0005.
#[async_trait]
pub trait ClusterStore: Send + Sync {
    async fn list_clusters(&self) -> Result<Vec<ClusterData>>;
    async fn list_clusters_by_org(&self, org_slug: &str) -> Result<Vec<ClusterData>>;
    async fn get_cluster(&self, slug: &str) -> Result<Option<ClusterData>>;
    async fn create_cluster(&self, req: CreateClusterRequest) -> Result<ClusterData>;
    async fn update_cluster(&self, slug: &str, req: UpdateClusterRequest) -> Result<ClusterData>;
    async fn delete_cluster(&self, slug: &str) -> Result<()>;
    /// Idempotent: noop if `node_id` is already a member of `cluster_slug`.
    async fn add_node_to_cluster(&self, cluster_slug: &str, node_id: &str) -> Result<ClusterData>;
    /// Idempotent: noop if `node_id` is not a member.
    async fn remove_node_from_cluster(
        &self,
        cluster_slug: &str,
        node_id: &str,
    ) -> Result<ClusterData>;
}

/// Operator-supplied inputs for creating an OIDC-backed User Account.
#[derive(Debug, Clone)]
pub struct EnsureAccountRequest {
    pub iss: String,
    pub sub: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
}

/// Operator-supplied inputs for the invite-by-email path. The created
/// Account has email but no `(iss, sub)`; the OIDC apply links the
/// identity on first login.
#[derive(Debug, Clone)]
pub struct CreateAccountByEmailRequest {
    pub email: String,
    pub display_name: Option<String>,
}

/// Inputs for granting a membership.
#[derive(Debug, Clone)]
pub struct CreateMembershipRequest {
    pub account_id: String,
    pub scope: MembershipScope,
    pub role: Role,
    /// Account id that's performing the grant. Used for audit; for the
    /// initial-admin bootstrap this points at the new admin itself.
    pub created_by_account: String,
}

/// Store trait for Account + Membership operations (ADR-0004).
#[async_trait]
pub trait AccountStore: Send + Sync {
    /// Lazy-create or refresh a User Account from an OIDC login. Returns
    /// the persisted row.
    async fn ensure_account_from_oidc(&self, req: EnsureAccountRequest) -> Result<AccountData>;
    /// Operator-initiated pre-create by email (invite flow). The OIDC
    /// apply links `(iss, sub)` on first login.
    async fn create_account_by_email(
        &self,
        req: CreateAccountByEmailRequest,
    ) -> Result<AccountData>;
    async fn get_account(&self, id: &str) -> Result<Option<AccountData>>;
    async fn get_account_by_oidc(&self, iss: &str, sub: &str) -> Result<Option<AccountData>>;
    async fn list_accounts(&self) -> Result<Vec<AccountData>>;
    async fn create_membership(&self, req: CreateMembershipRequest) -> Result<MembershipData>;
    async fn delete_membership(&self, id: &str) -> Result<()>;
    async fn list_memberships_for_account(&self, account_id: &str) -> Result<Vec<MembershipData>>;
    async fn list_memberships_at_scope(
        &self,
        scope: &MembershipScope,
    ) -> Result<Vec<MembershipData>>;
    /// Idempotent: grants Platform/PlatformAdmin to `account_id` only when
    /// no platform-admin exists yet. Returns the row (existing or new).
    async fn bootstrap_initial_platform_admin(&self, account_id: &str) -> Result<()>;
    async fn has_platform_admin(&self) -> Result<bool>;
}

/// Store trait for node onboarding (ADR-0006). Tokens are issued and
/// consumed via the bootstrap REST endpoint; revoke targets node certs.
#[async_trait]
pub trait OnboardingStore: Send + Sync {
    /// Idempotent: if the CA exists already, returns the existing one.
    /// Called once at cplane startup.
    async fn ensure_internal_ca(&self, deployment_name: &str) -> Result<crate::ca::InternalCa>;

    /// Returns the persisted server cert (PEM, key, serial, expiry). Used
    /// by the tunnel listener at startup.
    async fn get_server_cert(&self) -> Result<Option<crate::command::ServerCertData>>;

    /// Sign + persist a fresh server cert.
    async fn rotate_server_cert(
        &self,
        dns_names: Vec<String>,
    ) -> Result<crate::command::ServerCertData>;

    async fn create_onboarding_token(
        &self,
        req: CreateOnboardingTokenRequest,
    ) -> Result<(crate::command::OnboardingTokenData, String)>;

    async fn list_onboarding_tokens_by_cluster(
        &self,
        cluster_slug: &str,
    ) -> Result<Vec<crate::command::OnboardingTokenData>>;

    async fn delete_onboarding_token(&self, cluster_slug: &str, id: &str) -> Result<()>;

    async fn redeem_onboarding_token(
        &self,
        req: RedeemOnboardingTokenRequest,
    ) -> Result<BootstrapOutcome>;

    async fn revoke_node_cert(
        &self,
        node_id: &str,
        reason: crate::command::RevocationReason,
    ) -> Result<()>;
}

/// Store trait for volume operations.
#[async_trait]
pub trait VolumeStore: Send + Sync {
    /// List all volumes, optionally filtered by project or node.
    async fn list_volumes(
        &self,
        project_slug: Option<&str>,
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
    async fn list_templates_by_project(&self, project_slug: &str) -> Result<Vec<TemplateData>>;

    /// Get a template by ID.
    async fn get_template(&self, id: &str) -> Result<Option<TemplateData>>;

    /// Create a template (with optional source_url for import).
    async fn create_template(&self, req: CreateTemplateRequest) -> Result<TemplateData>;

    /// Update a template's import status.
    async fn update_template_status(
        &self,
        id: &str,
        req: UpdateTemplateStatusRequest,
    ) -> Result<TemplateData>;
}

// =============================================================================
// Security Group Request DTOs
// =============================================================================

/// Request to create a new security group.
#[derive(Debug, Clone)]
pub struct CreateSecurityGroupRequest {
    pub project_slug: String,
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
        project_slug: Option<&str>,
    ) -> Result<Vec<SecurityGroupData>>;

    /// Get a security group by ID.
    async fn get_security_group(&self, id: &str) -> Result<Option<SecurityGroupData>>;

    /// Create a new security group.
    async fn create_security_group(
        &self,
        req: CreateSecurityGroupRequest,
    ) -> Result<SecurityGroupData>;

    /// Patch a security group's mutable fields. Currently name and description.
    async fn update_security_group(
        &self,
        id: &str,
        req: UpdateSecurityGroupRequest,
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

    /// Update a rule's mutable fields. Currently only the description is
    /// editable; everything else is immutable (delete + recreate to change).
    async fn update_security_group_rule(
        &self,
        security_group_id: &str,
        rule_id: &str,
        req: UpdateSecurityGroupRuleRequest,
    ) -> Result<SecurityGroupData>;
}

/// Request to patch a security group's mutable fields.
#[derive(Debug, Clone, Default)]
pub struct UpdateSecurityGroupRequest {
    pub name: Option<String>,
    /// `Some(value)` writes (`Some(None)` clears); `None` leaves untouched.
    pub description: Option<Option<String>>,
}

/// Request to patch a single rule in a security group.
#[derive(Debug, Clone, Default)]
pub struct UpdateSecurityGroupRuleRequest {
    /// `Some(value)` writes that value (`Some(None)` clears the description),
    /// `None` leaves it untouched.
    pub description: Option<Option<String>>,
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
/// - Control plane management operations
/// - Event subscription for real-time updates
pub trait DataStore:
    NodeStore
    + NetworkStore
    + NicStore
    + VmStore
    + OrgStore
    + ProjectStore
    + ClusterStore
    + AccountStore
    + OnboardingStore
    + VolumeStore
    + TemplateStore
    + SecurityGroupStore
    + ControlplaneStore
    + Send
    + Sync
{
    /// Subscribe to state change events.
    ///
    /// Returns a broadcast receiver that will receive events when
    /// networks or NICs are created, updated, or deleted.
    fn subscribe(&self) -> broadcast::Receiver<Event>;
}
