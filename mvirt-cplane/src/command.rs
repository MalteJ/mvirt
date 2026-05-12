use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Commands that can be replicated through Raft
///
/// IMPORTANT: All timestamps must be set BEFORE the command is submitted to Raft.
/// Using `Utc::now()` inside the state machine's `apply()` breaks Raft's determinism
/// guarantee - different nodes would compute different timestamps, causing state divergence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Command {
    // Node operations
    RegisterNode {
        request_id: String,
        id: String,
        timestamp: String,
        name: String,
        address: String,
        resources: NodeResources,
        labels: HashMap<String, String>,
    },
    UpdateNodeStatus {
        request_id: String,
        node_id: String,
        timestamp: String,
        status: NodeStatus,
        resources: Option<NodeResources>,
    },
    DeregisterNode {
        request_id: String,
        node_id: String,
    },

    // Network operations
    CreateNetwork {
        request_id: String,
        /// Pre-generated ID for the network (deterministic across nodes)
        id: String,
        /// Timestamp when command was created (set before Raft replication)
        timestamp: String,
        project_slug: String,
        name: String,
        ipv4_enabled: bool,
        ipv4_prefix: Option<String>,
        ipv6_enabled: bool,
        ipv6_prefix: Option<String>,
        dns_servers: Vec<String>,
        ntp_servers: Vec<String>,
        is_public: bool,
    },
    UpdateNetwork {
        request_id: String,
        id: String,
        /// Timestamp when command was created (set before Raft replication)
        timestamp: String,
        dns_servers: Vec<String>,
        ntp_servers: Vec<String>,
    },
    DeleteNetwork {
        request_id: String,
        id: String,
        force: bool,
    },

    // NIC operations
    CreateNic {
        request_id: String,
        /// Pre-generated ID for the NIC (deterministic across nodes)
        id: String,
        /// Timestamp when command was created (set before Raft replication)
        timestamp: String,
        project_slug: String,
        network_id: String,
        name: Option<String>,
        mac_address: Option<String>,
        ipv4_address: Option<String>,
        ipv6_address: Option<String>,
        routed_ipv4_prefixes: Vec<String>,
        routed_ipv6_prefixes: Vec<String>,
        security_group_id: Option<String>,
    },
    UpdateNic {
        request_id: String,
        id: String,
        /// Timestamp when command was created (set before Raft replication)
        timestamp: String,
        routed_ipv4_prefixes: Vec<String>,
        routed_ipv6_prefixes: Vec<String>,
    },
    DeleteNic {
        request_id: String,
        id: String,
    },
    AttachNic {
        request_id: String,
        id: String,
        timestamp: String,
        vm_id: String,
    },
    DetachNic {
        request_id: String,
        id: String,
        timestamp: String,
    },
    UpdateNicStatus {
        request_id: String,
        id: String,
        timestamp: String,
        phase: NicPhase,
        socket_path: String,
        message: Option<String>,
    },

    // VM operations
    CreateVm {
        request_id: String,
        id: String,
        timestamp: String,
        spec: VmSpec,
    },
    UpdateVmSpec {
        request_id: String,
        id: String,
        timestamp: String,
        desired_state: VmDesiredState,
    },
    UpdateVmStatus {
        request_id: String,
        id: String,
        timestamp: String,
        status: VmStatus,
    },
    DeleteVm {
        request_id: String,
        id: String,
    },

    // Org operations — Org is identified by its slug (no separate UUID).
    CreateOrg {
        request_id: String,
        timestamp: String,
        slug: String,
        name: String,
        contact: OrgContact,
    },
    UpdateOrg {
        request_id: String,
        slug: String,
        timestamp: String,
        name: Option<String>,
        // Contact patch — `Some(value)` writes that value (including
        // `Some(None)` to clear an individual sub-field), `None` leaves the
        // sub-field unchanged. The outer `Option` distinguishes "patch not
        // included" from "patch with explicit nulls".
        contact: Option<OrgContact>,
    },
    DeleteOrg {
        request_id: String,
        slug: String,
    },

    // Project operations — Project is identified by its slug (no separate UUID).
    CreateProject {
        request_id: String,
        timestamp: String,
        org_slug: String,
        slug: String,
        name: String,
        description: Option<String>,
    },
    DeleteProject {
        request_id: String,
        slug: String,
    },

    // -- Node onboarding (ADR-0006) -----------------------------------------
    /// Bootstrap the internal CA if it doesn't exist. Idempotent: noop on
    /// subsequent calls. Triggered by the leader on startup. The CA material
    /// in the command is pre-generated (deterministic across replicas because
    /// the leader generates once and replicates).
    EnsureInternalCa {
        request_id: String,
        timestamp: String,
        ca: crate::ca::InternalCa,
    },

    /// Persist a newly-signed server cert for the cplane tunnel listener.
    /// Triggered by the leader at startup and on rotation.
    UpdateServerCert {
        request_id: String,
        timestamp: String,
        cert_pem: String,
        key_pem: String,
        serial_hex: String,
        not_after: String,
    },

    /// Issue a new onboarding token bound to a Cluster. Pre-allocates a
    /// `node_id` and writes a placeholder Node row with `status: Onboarding`
    /// so the operator sees the host in the cluster's node list immediately.
    /// Hostname is operator-supplied (display label).
    CreateOnboardingToken {
        request_id: String,
        timestamp: String,
        /// `tok_<short-random>` display id.
        id: String,
        /// sha256(bare token), 32 bytes hex.
        token_hash_hex: String,
        cluster_slug: String,
        /// Pre-allocated `node_<uuid>` — placeholder row in NODES gets this id.
        node_id: String,
        /// Display hostname for the node (required).
        hostname: String,
        description: Option<String>,
        expires_at: String,
        created_by_account: String,
    },

    /// Operator-initiated token revocation (before redemption).
    DeleteOnboardingToken {
        request_id: String,
        cluster_slug: String,
        id: String,
    },

    /// Atomic: validate the token (hash + not-used + not-expired + cluster
    /// exists), mint a new `node_id`, sign a leaf cert against `csr_pem`,
    /// mark the token used, write the Node row, append `node_id` to the
    /// Cluster's `node_ids`. Returns `Response::BootstrapResult` carrying
    /// the cert chain back to the caller.
    RedeemOnboardingToken {
        request_id: String,
        timestamp: String,
        token_hash_hex: String,
        csr_pem: String,
        hostname: String,
        agent_version: String,
        kernel_version: String,
        arch: String,
    },

    /// Revoke a Node's current cert (compromise response). Node row stays;
    /// the cert serial moves to the revocation set.
    RevokeNodeCert {
        request_id: String,
        timestamp: String,
        node_id: String,
        reason: RevocationReason,
    },

    // -- Accounts + Memberships (ADR-0004) ---------------------------------
    /// Operator-initiated Account pre-creation by email — the
    /// invite-by-email path. The Account is written with `email` but no
    /// `(external_iss, external_sub)`; the OIDC apply reconciles by email
    /// on first login and links the identity then. Rejects with 409 if
    /// the email is already taken.
    CreateAccountByEmail {
        request_id: String,
        timestamp: String,
        id: String,
        email: String,
        display_name: Option<String>,
    },

    /// Lazy-create or refresh an Account from an OIDC login. Idempotent —
    /// returns the existing row if `(iss, sub)` already matches. The
    /// authoritative identity key is `(iss, sub)`; email/display_name are
    /// cached for UI but never the identity key (ADR-0004 §"Account model").
    EnsureAccountFromOidc {
        request_id: String,
        timestamp: String,
        /// Pre-generated id used iff a new row is inserted; ignored if a
        /// match already exists.
        new_id: String,
        iss: String,
        sub: String,
        email: Option<String>,
        display_name: Option<String>,
    },

    /// Grant a (scope, role) membership to an Account.
    CreateMembership {
        request_id: String,
        timestamp: String,
        id: String,
        account_id: String,
        scope: MembershipScope,
        role: Role,
        created_by_account: String,
    },

    /// Remove a membership by id.
    DeleteMembership {
        request_id: String,
        id: String,
    },

    /// Idempotent: grants `(Platform, PlatformAdmin)` to `account_id` only
    /// when no platform-admin membership exists yet. Used by the initial
    /// admin bootstrap (ADR-0004 §"Initial admin bootstrap") so two parallel
    /// logins can't race-create two platform admins.
    BootstrapInitialPlatformAdmin {
        request_id: String,
        timestamp: String,
        id: String,
        account_id: String,
    },

    // Cluster operations — keyed by slug (platform-wide unique). See ADR-0005.
    CreateCluster {
        request_id: String,
        timestamp: String,
        org_slug: String,
        slug: String,
        name: String,
        description: Option<String>,
        location: Option<String>,
    },
    UpdateCluster {
        request_id: String,
        slug: String,
        timestamp: String,
        name: Option<String>,
        /// `Some(value)` writes (`Some(None)` clears); `None` leaves untouched.
        description: Option<Option<String>>,
        /// `Some(value)` writes (`Some(None)` clears); `None` leaves untouched.
        location: Option<Option<String>>,
    },
    DeleteCluster {
        request_id: String,
        slug: String,
    },
    /// Add a Node id to a Cluster's `node_ids`. Validates Cluster + Node both
    /// exist; idempotent if the Node is already a member. See ADR-0005.
    AddNodeToCluster {
        request_id: String,
        timestamp: String,
        cluster_slug: String,
        node_id: String,
    },
    /// Remove a Node id from a Cluster's `node_ids`. Idempotent if not a
    /// member. Per ADR-0005 the apply handler does **not** check for resources
    /// still on the Node — that gate lives at higher levels (REST handler /
    /// reconciler) once VM placement carries `cluster_slug`.
    RemoveNodeFromCluster {
        request_id: String,
        timestamp: String,
        cluster_slug: String,
        node_id: String,
    },

    // Volume operations (node_id for data locality - Shared Nothing architecture)
    CreateVolume {
        request_id: String,
        id: String,
        timestamp: String,
        project_slug: String,
        node_id: String,
        name: String,
        size_bytes: u64,
        template_id: Option<String>,
    },
    DeleteVolume {
        request_id: String,
        id: String,
    },
    UpdateVolumeStatus {
        request_id: String,
        id: String,
        timestamp: String,
        phase: VolumePhase,
        path: Option<String>,
        used_bytes: u64,
        error: Option<String>,
    },
    ResizeVolume {
        request_id: String,
        id: String,
        timestamp: String,
        size_bytes: u64,
    },
    CreateSnapshot {
        request_id: String,
        id: String,
        timestamp: String,
        volume_id: String,
        name: String,
    },

    // Template operations (node_id for locality)
    CreateTemplate {
        request_id: String,
        id: String,
        timestamp: String,
        project_slug: String,
        node_id: String,
        name: String,
        size_bytes: u64,
        source_url: Option<String>,
        total_bytes: u64,
    },
    UpdateTemplateStatus {
        request_id: String,
        id: String,
        timestamp: String,
        phase: TemplatePhase,
        bytes_written: u64,
        size_bytes: u64,
        error: Option<String>,
    },

    // Security group operations
    CreateSecurityGroup {
        request_id: String,
        id: String,
        timestamp: String,
        project_slug: String,
        name: String,
        description: Option<String>,
    },
    /// Patch an existing security group's mutable fields.
    UpdateSecurityGroup {
        request_id: String,
        timestamp: String,
        id: String,
        /// `Some(value)` writes the new name; `None` leaves it unchanged.
        name: Option<String>,
        /// `Some(value)` writes the new description (`Some(None)` clears it);
        /// `None` leaves it unchanged.
        description: Option<Option<String>>,
    },
    DeleteSecurityGroup {
        request_id: String,
        id: String,
    },
    CreateSecurityGroupRule {
        request_id: String,
        id: String,
        timestamp: String,
        security_group_id: String,
        direction: RuleDirection,
        protocol: Option<String>,
        port_range_start: Option<u16>,
        port_range_end: Option<u16>,
        cidr: Option<String>,
        description: Option<String>,
    },
    DeleteSecurityGroupRule {
        request_id: String,
        security_group_id: String,
        rule_id: String,
    },
    /// Update a single rule's mutable fields. Only `description` is mutable
    /// today; protocol/ports/cidr/direction are immutable (use delete +
    /// create for those). Each `Option` distinguishes "no change" from
    /// "set to this value (including null to clear)".
    UpdateSecurityGroupRule {
        request_id: String,
        timestamp: String,
        security_group_id: String,
        rule_id: String,
        description: Option<Option<String>>,
    },
}

impl Command {
    pub fn request_id(&self) -> &str {
        match self {
            Command::RegisterNode { request_id, .. } => request_id,
            Command::UpdateNodeStatus { request_id, .. } => request_id,
            Command::DeregisterNode { request_id, .. } => request_id,
            Command::CreateNetwork { request_id, .. } => request_id,
            Command::UpdateNetwork { request_id, .. } => request_id,
            Command::DeleteNetwork { request_id, .. } => request_id,
            Command::CreateNic { request_id, .. } => request_id,
            Command::UpdateNic { request_id, .. } => request_id,
            Command::DeleteNic { request_id, .. } => request_id,
            Command::AttachNic { request_id, .. } => request_id,
            Command::DetachNic { request_id, .. } => request_id,
            Command::UpdateNicStatus { request_id, .. } => request_id,
            Command::CreateVm { request_id, .. } => request_id,
            Command::UpdateVmSpec { request_id, .. } => request_id,
            Command::UpdateVmStatus { request_id, .. } => request_id,
            Command::DeleteVm { request_id, .. } => request_id,
            Command::CreateOrg { request_id, .. } => request_id,
            Command::UpdateOrg { request_id, .. } => request_id,
            Command::DeleteOrg { request_id, .. } => request_id,
            Command::CreateProject { request_id, .. } => request_id,
            Command::DeleteProject { request_id, .. } => request_id,
            Command::CreateCluster { request_id, .. } => request_id,
            Command::UpdateCluster { request_id, .. } => request_id,
            Command::DeleteCluster { request_id, .. } => request_id,
            Command::AddNodeToCluster { request_id, .. } => request_id,
            Command::RemoveNodeFromCluster { request_id, .. } => request_id,
            Command::EnsureInternalCa { request_id, .. } => request_id,
            Command::UpdateServerCert { request_id, .. } => request_id,
            Command::CreateOnboardingToken { request_id, .. } => request_id,
            Command::DeleteOnboardingToken { request_id, .. } => request_id,
            Command::RedeemOnboardingToken { request_id, .. } => request_id,
            Command::RevokeNodeCert { request_id, .. } => request_id,
            Command::CreateAccountByEmail { request_id, .. } => request_id,
            Command::EnsureAccountFromOidc { request_id, .. } => request_id,
            Command::CreateMembership { request_id, .. } => request_id,
            Command::DeleteMembership { request_id, .. } => request_id,
            Command::BootstrapInitialPlatformAdmin { request_id, .. } => request_id,
            Command::CreateVolume { request_id, .. } => request_id,
            Command::DeleteVolume { request_id, .. } => request_id,
            Command::UpdateVolumeStatus { request_id, .. } => request_id,
            Command::ResizeVolume { request_id, .. } => request_id,
            Command::CreateSnapshot { request_id, .. } => request_id,
            Command::CreateTemplate { request_id, .. } => request_id,
            Command::UpdateTemplateStatus { request_id, .. } => request_id,
            Command::CreateSecurityGroup { request_id, .. } => request_id,
            Command::UpdateSecurityGroup { request_id, .. } => request_id,
            Command::DeleteSecurityGroup { request_id, .. } => request_id,
            Command::CreateSecurityGroupRule { request_id, .. } => request_id,
            Command::DeleteSecurityGroupRule { request_id, .. } => request_id,
            Command::UpdateSecurityGroupRule { request_id, .. } => request_id,
        }
    }
}

// =============================================================================
// Node Types
// =============================================================================

/// Node (hypervisor agent) data stored in the state machine
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeData {
    pub id: String,
    pub name: String,
    pub address: String,
    pub status: NodeStatus,
    pub resources: NodeResources,
    pub labels: HashMap<String, String>,
    pub last_heartbeat: String,
    pub created_at: String,
    pub updated_at: String,
    /// Cluster this node was onboarded into. None for legacy/test rows.
    #[serde(default)]
    pub cluster_slug: Option<String>,
    /// Hex serial of the current mTLS client cert. None for legacy rows.
    #[serde(default)]
    pub cert_serial_hex: Option<String>,
    /// When the current cert expires (RFC3339). None for legacy rows.
    #[serde(default)]
    pub cert_expires_at: Option<String>,
    /// Hostname reported during onboarding. None for legacy rows.
    #[serde(default)]
    pub hostname: Option<String>,
    /// Agent version reported during onboarding.
    #[serde(default)]
    pub agent_version: Option<String>,
}

/// Node status - health state of the hypervisor
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum NodeStatus {
    /// Node is connected and healthy
    Online,
    /// Node failed to send heartbeat within timeout
    Offline,
    /// Node status is unknown (initial state)
    #[default]
    Unknown,
    /// Onboarding token issued, node hasn't redeemed yet. Placeholder Node
    /// row created by the operator with hostname pre-filled. Transitions to
    /// `Online` on successful redeem.
    Onboarding,
    /// Node onboarding completed, but cert revoked. Node is kept in the
    /// table for audit, must not be allowed to reconnect.
    Revoked,
}

/// Reason for revoking a node's cert. Recorded alongside the serial.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum RevocationReason {
    /// Operator-initiated decommission. Node row is deleted from the table.
    Decommission,
    /// Operator-initiated response to suspected key compromise. Node row
    /// stays in the table with status=Revoked.
    Compromise,
    /// Catch-all.
    Other,
}

/// Node resource capacity and availability
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NodeResources {
    pub cpu_cores: u32,
    pub memory_mb: u64,
    pub storage_gb: u64,
    pub available_cpu_cores: u32,
    pub available_memory_mb: u64,
    pub available_storage_gb: u64,
}

// =============================================================================
// Network Types
// =============================================================================

/// Network data stored in the state machine
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkData {
    pub id: String,
    pub project_slug: String,
    pub name: String,
    pub ipv4_enabled: bool,
    pub ipv4_prefix: Option<String>,
    pub ipv6_enabled: bool,
    pub ipv6_prefix: Option<String>,
    pub dns_servers: Vec<String>,
    pub ntp_servers: Vec<String>,
    pub is_public: bool,
    pub nic_count: u32,
    pub created_at: String,
    pub updated_at: String,
}

/// NIC data stored in the state machine
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NicData {
    pub id: String,
    pub spec: NicSpec,
    pub status: NicStatus,
    pub created_at: String,
    pub updated_at: String,
}

/// NicSpec — desired state, written by REST.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NicSpec {
    pub project_slug: String,
    pub name: Option<String>,
    pub network_id: String,
    pub mac_address: String,
    pub ipv4_address: Option<String>,
    pub ipv6_address: Option<String>,
    pub routed_ipv4_prefixes: Vec<String>,
    pub routed_ipv6_prefixes: Vec<String>,
    pub security_group_id: Option<String>,
    pub vm_id: Option<String>,
}

/// NicStatus — observed state, written by the reconciler.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NicStatus {
    pub phase: NicPhase,
    pub socket_path: String,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum NicPhase {
    #[default]
    Pending,
    Active,
    Failed,
}

// =============================================================================
// VM Types (with Spec/Status pattern)
// =============================================================================

/// VM resource - combines spec, status, and metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmData {
    pub id: String,
    pub spec: VmSpec,
    pub status: VmStatus,
    pub created_at: String,
    pub updated_at: String,
}

/// VmSpec - desired state for a VM (user-defined)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmSpec {
    pub name: String,
    pub project_slug: String,
    pub node_selector: Option<String>, // Optional: require specific node
    pub cpu_cores: u32,
    pub memory_mb: u64,
    pub volume_id: String, // Boot volume reference
    pub nic_id: String,    // NIC reference (network comes via NIC)
    pub image: String,     // Boot image reference
    pub desired_state: VmDesiredState,
}

/// Desired power state for a VM
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum VmDesiredState {
    #[default]
    Running,
    Stopped,
}

/// VmStatus - actual observed state (from nodes)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VmStatus {
    pub phase: VmPhase,
    pub node_id: Option<String>,    // Assigned node
    pub ip_address: Option<String>, // Assigned IP
    pub message: Option<String>,    // Error or status message
}

/// VM lifecycle phase
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum VmPhase {
    /// Waiting to be scheduled to a node
    #[default]
    Pending,
    /// Assigned to a node, waiting for creation
    Scheduled,
    /// Being created on the node
    Creating,
    /// VM is running
    Running,
    /// VM is stopping
    Stopping,
    /// VM is stopped
    Stopped,
    /// VM creation/operation failed
    Failed,
}

// =============================================================================
// Common Types
// =============================================================================

/// Generic resource phase for Networks, NICs, etc.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ResourcePhase {
    /// Resource is pending creation
    #[default]
    Pending,
    /// Resource is being created
    Creating,
    /// Resource is ready/active
    Ready,
    /// Resource is being updated
    Updating,
    /// Resource is being deleted
    Deleting,
    /// Resource operation failed
    Failed,
}

// =============================================================================
// Project Types
// =============================================================================

// =============================================================================
// Organization Types
// =============================================================================

/// Contact / billing details for an Organization. All fields optional —
/// a fresh Org has nothing filled in.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrgContact {
    /// Legal company name (Firmenname).
    pub legal_name: Option<String>,
    /// Street and house number on a single line (Straße/Hausnummer).
    pub street_address: Option<String>,
    /// Postal code (PLZ).
    pub postal_code: Option<String>,
    /// City (Ort).
    pub city: Option<String>,
    /// ISO-3166 country (free text for now; we'll tighten later).
    pub country: Option<String>,
    /// Technical contact (incident notifications, abuse).
    pub technical_contact_email: Option<String>,
    /// Billing contact (invoices, dunning).
    pub billing_contact_email: Option<String>,
    /// VAT identification number (USt-IdNr.).
    pub vat_id: Option<String>,
}

/// Organization data — the tenancy container above Project. See ADR-0004.
/// The slug is the primary key (kebab-case, platform-unique, immutable).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgData {
    pub slug: String,
    pub name: String,
    pub contact: OrgContact,
    pub created_at: String,
    pub updated_at: String,
}

/// Project data — the K8s-namespace-equivalent tenancy unit. See ADR-0004.
/// The slug is the primary key (kebab-case, platform-unique, immutable);
/// `org_slug` is the foreign key to the parent Org.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectData {
    pub slug: String,
    pub org_slug: String,
    pub name: String,
    pub description: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Persisted onboarding token. Only the hash of the bare token survives —
/// the bare value is shown to the operator once at creation time. See
/// ADR-0006.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnboardingTokenData {
    pub id: String,
    pub token_hash_hex: String,
    pub cluster_slug: String,
    /// Node id pre-allocated at token-issuance time. The placeholder row in
    /// NODES has this id and `status: Onboarding`; redeem flips it to Online.
    pub node_id: String,
    /// Operator-supplied display hostname; also written to NodeData.name.
    pub hostname: String,
    pub description: Option<String>,
    pub expires_at: String,
    pub used_at: Option<String>,
    pub used_by_node_id: Option<String>,
    pub created_by_account: String,
    pub created_at: String,
}

/// Revoked cert serial; checked by the tunnel listener on every handshake.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevokedCertData {
    pub serial_hex: String,
    pub node_id: String,
    pub revoked_at: String,
    pub reason: RevocationReason,
}

// =============================================================================
// Account + Membership Types (ADR-0004)
// =============================================================================

/// What kind of Account this row represents. ServiceAccount is reserved in
/// v1 — all live rows are User accounts. See ADR-0004 §"Account model".
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AccountKind {
    User,
    ServiceAccount,
}

/// User-or-ServiceAccount. Identity key for Users is `(external_iss,
/// external_sub)`; never email. Email + display_name are cached from the
/// IdP for the UI and may go stale.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountData {
    pub id: String,
    pub kind: AccountKind,
    /// OIDC issuer URL. None for ServiceAccounts.
    pub external_iss: Option<String>,
    /// OIDC `sub` claim. None for ServiceAccounts.
    pub external_sub: Option<String>,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Authorization scope for a membership row. Cluster scope is reserved per
/// ADR-0005 but not used in v1.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MembershipScope {
    Platform,
    Org { org_slug: String },
    Project { project_slug: String },
}

/// Roles available within a scope. ADR-0004 settled on three; cluster-admin
/// was reclaimed for the future per-Cluster role and renamed away from the
/// platform-wide superadmin role.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Role {
    /// Full access across the whole platform. Cascades downward.
    PlatformAdmin,
    /// Full access within one Org (and its Projects/Clusters).
    OrgAdmin,
    /// Full access within one Project.
    ProjectAdmin,
}

/// A grant of `role` at `scope` to an Account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MembershipData {
    pub id: String,
    pub account_id: String,
    pub scope: MembershipScope,
    pub role: Role,
    /// The Account that issued this grant. For bootstrap rows it points at
    /// the freshly-bootstrapped admin itself (self-grant).
    pub created_by_account: String,
    pub created_at: String,
}

/// Current cplane server cert + key, persisted in raft so a freshly-elected
/// leader can serve TLS without re-minting. Rotation is leader-driven.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerCertData {
    pub cert_pem: String,
    pub key_pem: String,
    pub serial_hex: String,
    pub not_after: String,
    pub created_at: String,
}

/// Cluster data — a named, explicitly-listed group of Nodes within an Org.
/// See ADR-0005. The slug is the primary key (kebab-case, platform-unique,
/// immutable); `org_slug` is the foreign key to the parent Org. `node_ids`
/// is the eligibility set the scheduler picks from when placing resources.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterData {
    pub slug: String,
    pub org_slug: String,
    pub name: String,
    pub description: Option<String>,
    pub location: Option<String>,
    pub node_ids: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

// =============================================================================
// Storage Types (Volumes, Templates, Import Jobs)
// =============================================================================

/// Volume data stored in the state machine (bound to a specific node)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeData {
    pub id: String,
    pub spec: VolumeSpec,
    pub status: VolumeStatus,
    pub created_at: String,
    pub updated_at: String,
}

/// VolumeSpec — desired state, written by REST via Create/Update*Spec commands.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeSpec {
    pub project_slug: String,
    pub node_id: String, // Node where the volume is stored (Shared Nothing)
    pub name: String,
    pub size_bytes: u64,
    pub template_id: Option<String>,
}

/// VolumeStatus — observed state, written exclusively by the reconciler via UpdateVolumeStatus.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VolumeStatus {
    pub phase: VolumePhase,
    pub path: String, // ZFS path e.g., /dev/zvol/pool/vol-xxx, set by reconciler
    pub used_bytes: u64,
    pub compression_ratio: f64,
    pub snapshots: Vec<SnapshotData>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum VolumePhase {
    #[default]
    Pending,
    Creating,
    Ready,
    Failed,
}

/// Snapshot data stored inline in VolumeData
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotData {
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub used_bytes: u64,
}

/// Template data stored in the state machine (bound to a specific node)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateData {
    pub id: String,
    pub spec: TemplateSpec,
    pub status: TemplateStatus,
    pub created_at: String,
    pub updated_at: String,
}

/// TemplateSpec — desired state, written by REST.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateSpec {
    pub project_slug: String,
    pub node_id: String, // Node where the template is stored
    pub name: String,
    pub source_url: Option<String>,
}

/// TemplateStatus — observed state (import progress + clone count + final size).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TemplateStatus {
    pub phase: TemplatePhase,
    pub size_bytes: u64,
    pub bytes_written: u64,
    pub total_bytes: u64,
    pub clone_count: u32,
    pub error: Option<String>,
}

/// Template lifecycle phase
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum TemplatePhase {
    #[default]
    Pending,
    Importing,
    Ready,
    Failed,
}

// =============================================================================
// Security Group Types
// =============================================================================

/// Security group data stored in the state machine
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityGroupData {
    pub id: String,
    pub project_slug: String,
    pub name: String,
    pub description: Option<String>,
    pub rules: Vec<SecurityGroupRuleData>,
    pub nic_count: u32,
    pub created_at: String,
    pub updated_at: String,
}

/// Security group rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityGroupRuleData {
    pub id: String,
    pub direction: RuleDirection,
    pub protocol: Option<String>,
    pub port_range_start: Option<u16>,
    pub port_range_end: Option<u16>,
    pub cidr: Option<String>,
    pub description: Option<String>,
    pub created_at: String,
}

/// Direction of a firewall rule
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum RuleDirection {
    Inbound,
    Outbound,
}

// =============================================================================
// Response Types
// =============================================================================

/// Response from applying a command
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Response {
    Node(NodeData),
    Network(NetworkData),
    Nic(NicData),
    Vm(VmData),
    Org(OrgData),
    Project(ProjectData),
    Cluster(ClusterData),
    OnboardingToken(OnboardingTokenData),
    Account(AccountData),
    Membership(MembershipData),
    /// Result of a successful `RedeemOnboardingToken` apply: the freshly-
    /// minted Node + signed leaf + CA root (so the node can verify the
    /// cplane server cert on the next mTLS handshake).
    BootstrapResult {
        node: NodeData,
        client_cert_pem: String,
        ca_cert_pem: String,
    },
    /// Result of `EnsureInternalCa` / `UpdateServerCert` when the caller
    /// just needs a confirmation that the apply ran.
    Ack,
    Volume(VolumeData),
    Template(TemplateData),
    SecurityGroup(SecurityGroupData),
    Deleted {
        id: String,
    },
    DeletedWithCount {
        id: String,
        nics_deleted: u32,
    },
    Error {
        code: u32,
        message: String,
    },
}

impl Default for Response {
    fn default() -> Self {
        Response::Error {
            code: 0,
            message: "No response".to_string(),
        }
    }
}
