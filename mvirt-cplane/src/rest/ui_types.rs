//! UI-compatible DTO types with camelCase serialization.
//!
//! These types match the mock-server's JSON structure for compatibility with mvirt-ui.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use serde::Deserializer;

use crate::command::{
    ClusterData, NetworkData, NicData, OrgContact, OrgData, ProjectData, SnapshotData,
    TemplateData, TemplatePhase, VmData, VmDesiredState, VmPhase, VolumeData, VolumePhase,
};

/// Tri-state deserializer for `Option<Option<T>>` PATCH semantics.
///
/// - field absent     → `None`        (untouched)
/// - field is null    → `Some(None)`  (clear)
/// - field is value v → `Some(Some(v))` (set)
///
/// Plain `#[serde(default)]` collapses null to `None` at the outer level,
/// which means the apply handler can't distinguish "leave alone" from
/// "clear". Pair this helper with `#[serde(default, deserialize_with = ...)]`.
fn deserialize_tristate<'de, T, D>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    T: serde::Deserialize<'de>,
    D: Deserializer<'de>,
{
    Option::<T>::deserialize(de).map(Some)
}

// =============================================================================
// VM Types
// =============================================================================

/// UI-compatible VM state enum
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
pub enum UiVmState {
    #[serde(rename = "STOPPED")]
    Stopped,
    #[serde(rename = "STARTING")]
    Starting,
    #[serde(rename = "RUNNING")]
    Running,
    #[serde(rename = "STOPPING")]
    Stopping,
}

impl UiVmState {
    /// Convert from internal VM data to UI state
    pub fn from_vm_data(data: &VmData) -> Self {
        match (data.spec.desired_state, data.status.phase) {
            (VmDesiredState::Running, VmPhase::Running) => UiVmState::Running,
            (VmDesiredState::Stopped, VmPhase::Stopped) => UiVmState::Stopped,
            (VmDesiredState::Running, _) => UiVmState::Starting,
            (VmDesiredState::Stopped, _) => UiVmState::Stopping,
        }
    }
}

/// UI-compatible VM configuration
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiVmConfig {
    pub vcpus: u32,
    pub memory_mb: u64,
    pub volume_id: String,
    pub nic_id: String,
    pub image: String,
}

/// UI-compatible VM representation
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiVm {
    pub id: String,
    pub project_slug: String,
    pub name: String,
    pub state: UiVmState,
    pub config: UiVmConfig,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip_address: Option<String>,
}

impl From<VmData> for UiVm {
    fn from(data: VmData) -> Self {
        let state = UiVmState::from_vm_data(&data);
        let started_at = if state == UiVmState::Running {
            Some(data.updated_at.clone())
        } else {
            None
        };

        Self {
            id: data.id,
            project_slug: data.spec.project_slug.clone(),
            name: data.spec.name.clone(),
            state,
            config: UiVmConfig {
                vcpus: data.spec.cpu_cores,
                memory_mb: data.spec.memory_mb,
                volume_id: data.spec.volume_id.clone(),
                nic_id: data.spec.nic_id.clone(),
                image: data.spec.image.clone(),
            },
            created_at: data.created_at,
            started_at,
            node_id: data.status.node_id,
            ip_address: data.status.ip_address,
        }
    }
}

/// Request to create a VM (UI-compatible)
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiCreateVmRequest {
    pub name: String,
    pub config: UiCreateVmConfig,
    #[serde(default)]
    pub node_selector: Option<String>,
}

/// VM configuration for creation (UI-compatible)
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiCreateVmConfig {
    pub vcpus: u32,
    pub memory_mb: u64,
    pub volume_id: String,
    pub nic_id: String,
    pub image: String,
    /// Optional cloud-init user-data (YAML). If empty/missing, the cplane
    /// supplies a minimal `#cloud-config\nhostname: <vm-name>` stub so
    /// cloud-init has a NoCloud datasource to consume and proceeds to the
    /// network stage. Without that stub, Ubuntu cloud-image hangs in the
    /// metadata-probe loop and never applies netplan, so DHCP never fires.
    #[serde(default)]
    pub user_data: Option<String>,
}

/// Response wrapper for VM list
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct VmListResponse {
    pub vms: Vec<UiVm>,
}

// =============================================================================
// Network Types
// =============================================================================

/// UI-compatible network representation
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiNetwork {
    pub id: String,
    pub project_slug: String,
    pub name: String,
    pub ipv4_enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipv4_prefix: Option<String>,
    pub ipv6_enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipv6_prefix: Option<String>,
    pub dns_servers: Vec<String>,
    pub is_public: bool,
    pub nic_count: u32,
    pub created_at: String,
}

impl From<NetworkData> for UiNetwork {
    fn from(data: NetworkData) -> Self {
        Self {
            id: data.id,
            project_slug: data.project_slug,
            name: data.name,
            ipv4_enabled: data.ipv4_enabled,
            ipv4_prefix: data.ipv4_prefix,
            ipv6_enabled: data.ipv6_enabled,
            ipv6_prefix: data.ipv6_prefix,
            dns_servers: data.dns_servers,
            is_public: data.is_public,
            nic_count: data.nic_count,
            created_at: data.created_at,
        }
    }
}

/// Request to create a network (UI-compatible)
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiCreateNetworkRequest {
    pub name: String,
    #[serde(default = "default_true")]
    pub ipv4_enabled: bool,
    #[serde(default)]
    pub ipv4_prefix: Option<String>,
    #[serde(default)]
    pub ipv6_enabled: bool,
    #[serde(default)]
    pub ipv6_prefix: Option<String>,
    #[serde(default)]
    pub dns_servers: Vec<String>,
    #[serde(default)]
    pub ntp_servers: Vec<String>,
    #[serde(default)]
    pub is_public: bool,
}

fn default_true() -> bool {
    true
}

/// Response wrapper for network list
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NetworkListResponse {
    pub networks: Vec<UiNetwork>,
}

// =============================================================================
// NIC Types
// =============================================================================

/// UI-compatible NIC representation
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiNic {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub network_id: String,
    pub mac_address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipv4_address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipv6_address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vm_id: Option<String>,
    pub state: String,
    pub created_at: String,
}

impl From<NicData> for UiNic {
    fn from(data: NicData) -> Self {
        Self {
            id: data.id,
            name: data.spec.name,
            network_id: data.spec.network_id,
            mac_address: data.spec.mac_address,
            ipv4_address: data.spec.ipv4_address,
            ipv6_address: data.spec.ipv6_address,
            vm_id: data.spec.vm_id,
            state: format!("{:?}", data.status.phase),
            created_at: data.created_at,
        }
    }
}

/// Request to create a NIC (UI-compatible)
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiCreateNicRequest {
    pub network_id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub mac_address: Option<String>,
    #[serde(default)]
    pub ipv4_address: Option<String>,
    #[serde(default)]
    pub ipv6_address: Option<String>,
    #[serde(default)]
    pub security_group_id: Option<String>,
}

/// Request to attach a NIC to a VM
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiAttachNicRequest {
    pub vm_id: String,
}

/// Response wrapper for NIC list
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NicListResponse {
    pub nics: Vec<UiNic>,
}

// =============================================================================
// Project Types
// =============================================================================

// =============================================================================
// Org Types
// =============================================================================

/// Org contact / billing details surfaced over the wire.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiOrgContact {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legal_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub street_address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub postal_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub technical_contact_email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub billing_contact_email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vat_id: Option<String>,
}

impl From<OrgContact> for UiOrgContact {
    fn from(c: OrgContact) -> Self {
        Self {
            legal_name: c.legal_name,
            street_address: c.street_address,
            postal_code: c.postal_code,
            city: c.city,
            country: c.country,
            technical_contact_email: c.technical_contact_email,
            billing_contact_email: c.billing_contact_email,
            vat_id: c.vat_id,
        }
    }
}

impl From<UiOrgContact> for OrgContact {
    fn from(c: UiOrgContact) -> Self {
        Self {
            legal_name: c.legal_name,
            street_address: c.street_address,
            postal_code: c.postal_code,
            city: c.city,
            country: c.country,
            technical_contact_email: c.technical_contact_email,
            billing_contact_email: c.billing_contact_email,
            vat_id: c.vat_id,
        }
    }
}

/// UI-compatible Org representation
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiOrg {
    pub slug: String,
    pub name: String,
    pub contact: UiOrgContact,
    pub created_at: String,
    pub updated_at: String,
}

impl From<OrgData> for UiOrg {
    fn from(data: OrgData) -> Self {
        Self {
            slug: data.slug,
            name: data.name,
            contact: UiOrgContact::from(data.contact),
            created_at: data.created_at,
            updated_at: data.updated_at,
        }
    }
}

/// Request to create an Org (UI-compatible)
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiCreateOrgRequest {
    /// URL identifier — kebab-case, platform-wide unique, immutable.
    pub slug: String,
    pub name: String,
}

/// Request to update an Org. All fields optional; unset = unchanged.
#[derive(Debug, Clone, Deserialize, Default, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiUpdateOrgRequest {
    #[serde(default)]
    pub name: Option<String>,
    /// Whole new contact record (partial updates are done client-side: the
    /// UI fetches the current Org, lets the user edit, and posts the
    /// resulting full record).
    #[serde(default)]
    pub contact: Option<UiOrgContact>,
}

/// Response wrapper for Org list
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OrgListResponse {
    pub orgs: Vec<UiOrg>,
}

/// UI-compatible project representation
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiProject {
    pub slug: String,
    pub org_slug: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl From<ProjectData> for UiProject {
    fn from(data: ProjectData) -> Self {
        Self {
            slug: data.slug,
            org_slug: data.org_slug,
            name: data.name,
            description: data.description,
            created_at: data.created_at,
            updated_at: data.updated_at,
        }
    }
}

/// Request to create a project (UI-compatible). The Org comes from the URL path
/// (`POST /v1/orgs/:org-slug/projects`); the body carries the project slug + display fields.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiCreateProjectRequest {
    /// URL identifier — kebab-case, platform-wide unique, immutable.
    pub slug: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// Response wrapper for project list
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectListResponse {
    pub projects: Vec<UiProject>,
}

// =============================================================================
// Cluster Types — see ADR-0005.
// =============================================================================

/// UI-compatible Cluster representation.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiCluster {
    pub slug: String,
    pub org_slug: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    pub node_ids: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl From<ClusterData> for UiCluster {
    fn from(data: ClusterData) -> Self {
        Self {
            slug: data.slug,
            org_slug: data.org_slug,
            name: data.name,
            description: data.description,
            location: data.location,
            node_ids: data.node_ids,
            created_at: data.created_at,
            updated_at: data.updated_at,
        }
    }
}

/// Request to create a Cluster (UI-compatible). The Org comes from the URL
/// path (`POST /v1/orgs/:org-slug/clusters`); the body carries the cluster
/// slug + display fields.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiCreateClusterRequest {
    /// URL identifier — kebab-case, platform-wide unique, immutable.
    pub slug: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
}

/// Request to update a Cluster. All fields optional; unset leaves the field
/// unchanged. For nullable fields the `Option<Option<_>>` carries tri-state:
/// key absent → untouched; key present with value → set; key present with
/// JSON `null` → clear.
#[derive(Debug, Clone, Deserialize, Default, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiUpdateClusterRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "deserialize_tristate")]
    pub description: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_tristate")]
    pub location: Option<Option<String>>,
}

/// Response wrapper for Cluster list.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClusterListResponse {
    pub clusters: Vec<UiCluster>,
}

// =============================================================================
// Node onboarding (ADR-0006)
// =============================================================================

/// UI-facing token record (no bare-token field — that's only returned at
/// create time via `UiCreateOnboardingTokenResponse`).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiOnboardingToken {
    pub id: String,
    pub cluster_slug: String,
    pub node_id: String,
    pub hostname: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub expires_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_by_node_id: Option<String>,
    pub created_by_account: String,
    pub created_at: String,
}

impl From<crate::command::OnboardingTokenData> for UiOnboardingToken {
    fn from(d: crate::command::OnboardingTokenData) -> Self {
        Self {
            id: d.id,
            cluster_slug: d.cluster_slug,
            node_id: d.node_id,
            hostname: d.hostname,
            description: d.description,
            expires_at: d.expires_at,
            used_at: d.used_at,
            used_by_node_id: d.used_by_node_id,
            created_by_account: d.created_by_account,
            created_at: d.created_at,
        }
    }
}

/// Operator-supplied parameters for issuing a new onboarding token.
/// The Cluster comes from the URL path.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiCreateOnboardingTokenRequest {
    /// Display hostname for the node. Required.
    pub hostname: String,
    #[serde(default)]
    pub ttl_seconds: Option<u64>,
    #[serde(default)]
    pub description: Option<String>,
}

/// Response to a successful token creation. **The `token` field is the only
/// time the bare token is ever revealed** — the operator copies it into
/// node config and the cplane keeps only its hash.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiCreateOnboardingTokenResponse {
    pub id: String,
    pub token: String,
    pub cluster_slug: String,
    pub node_id: String,
    pub hostname: String,
    pub expires_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingTokenListResponse {
    pub tokens: Vec<UiOnboardingToken>,
}

/// Body of `POST /v1/bootstrap/onboarding`. The bare token is sent in the
/// `Authorization: Bearer ...` header instead, so the body stays free of
/// secrets — easier to log + tcpdump for debugging.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiBootstrapRequest {
    /// PEM-encoded CSR. Subject + SAN in the CSR are ignored — the cplane
    /// fills its own. Only the public key carries.
    pub csr_pem: String,
    pub hostname: String,
    pub agent_version: String,
    pub kernel_version: String,
    pub arch: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiBootstrapResponse {
    pub node_id: String,
    pub cluster_slug: String,
    pub client_cert_pem: String,
    pub ca_cert_pem: String,
    pub cert_not_after: String,
    /// mvirt-log endpoints (URLs) the onboarded node should send audit
    /// and journald traffic to. Cplane-managed; nodes treat this as
    /// canonical, falling back to local defaults only if empty.
    pub log_endpoints: Vec<String>,
}

/// Body of `POST /v1/nodes/{id}/revoke`.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiRevokeNodeRequest {
    /// One of: `compromise`, `decommission`, `other`.
    pub reason: String,
}

// =============================================================================
// Accounts + Memberships (ADR-0004)
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiAccount {
    pub id: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl From<crate::command::AccountData> for UiAccount {
    fn from(a: crate::command::AccountData) -> Self {
        Self {
            id: a.id,
            kind: match a.kind {
                crate::command::AccountKind::User => "user".into(),
                crate::command::AccountKind::ServiceAccount => "service_account".into(),
            },
            email: a.email,
            display_name: a.display_name,
            created_at: a.created_at,
            updated_at: a.updated_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiMembership {
    pub id: String,
    pub account_id: String,
    /// `platform` | `org` | `project`.
    pub scope: String,
    /// Set only for `org` / `project` scopes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_slug: Option<String>,
    /// `platform-admin` | `org-admin` | `project-admin`.
    pub role: String,
    pub created_by_account: String,
    pub created_at: String,
}

impl From<crate::command::MembershipData> for UiMembership {
    fn from(m: crate::command::MembershipData) -> Self {
        let (scope, scope_slug) = match m.scope {
            crate::command::MembershipScope::Platform => ("platform".to_string(), None),
            crate::command::MembershipScope::Org { org_slug } => {
                ("org".to_string(), Some(org_slug))
            }
            crate::command::MembershipScope::Project { project_slug } => {
                ("project".to_string(), Some(project_slug))
            }
        };
        let role = match m.role {
            crate::command::Role::PlatformAdmin => "platform-admin",
            crate::command::Role::OrgAdmin => "org-admin",
            crate::command::Role::ProjectAdmin => "project-admin",
        };
        Self {
            id: m.id,
            account_id: m.account_id,
            scope,
            scope_slug,
            role: role.to_string(),
            created_by_account: m.created_by_account,
            created_at: m.created_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiMe {
    pub account: UiAccount,
    pub memberships: Vec<UiMembership>,
    pub is_platform_admin: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MembershipListResponse {
    pub memberships: Vec<UiMembership>,
}

/// Body for inviting a user by email — pre-creates an Account that gets
/// linked to OIDC on first login.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiInviteAccountRequest {
    pub email: String,
    #[serde(default)]
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiCreateOrgMembershipRequest {
    /// Account id to grant the membership to. Account must already exist —
    /// either pre-registered or created via a prior OIDC login.
    pub account_id: String,
    /// `org-admin`. Only one role today; surface as a string for forward-
    /// compat with future per-Org roles (member, viewer, …).
    pub role: String,
}

// =============================================================================
// Volume Types
// =============================================================================

/// UI-compatible snapshot representation
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiSnapshot {
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub used_bytes: u64,
}

impl From<&SnapshotData> for UiSnapshot {
    fn from(data: &SnapshotData) -> Self {
        Self {
            id: data.id.clone(),
            name: data.name.clone(),
            created_at: data.created_at.clone(),
            used_bytes: data.used_bytes,
        }
    }
}

/// UI-compatible volume representation
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiVolume {
    pub id: String,
    pub project_slug: String,
    pub node_id: String,
    pub name: String,
    pub size_bytes: u64,
    pub used_bytes: u64,
    pub compression_ratio: f64,
    pub snapshots: Vec<UiSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template_id: Option<String>,
    pub phase: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub created_at: String,
}

impl From<VolumeData> for UiVolume {
    fn from(data: VolumeData) -> Self {
        Self {
            id: data.id,
            project_slug: data.spec.project_slug,
            node_id: data.spec.node_id,
            name: data.spec.name,
            size_bytes: data.spec.size_bytes,
            used_bytes: data.status.used_bytes,
            compression_ratio: data.status.compression_ratio,
            snapshots: data.status.snapshots.iter().map(UiSnapshot::from).collect(),
            template_id: data.spec.template_id,
            phase: match data.status.phase {
                VolumePhase::Pending => "pending",
                VolumePhase::Creating => "creating",
                VolumePhase::Ready => "ready",
                VolumePhase::Failed => "failed",
            }
            .to_string(),
            path: data.status.path,
            error: data.status.error,
            created_at: data.created_at,
        }
    }
}

/// Request to create a volume (UI-compatible)
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiCreateVolumeRequest {
    pub node_id: String,
    pub name: String,
    pub size_bytes: u64,
    #[serde(default)]
    pub template_id: Option<String>,
}

/// Request to resize a volume (UI-compatible)
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiResizeVolumeRequest {
    pub size_bytes: u64,
}

/// Request to create a snapshot (UI-compatible)
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiCreateSnapshotRequest {
    pub name: String,
}

/// Response wrapper for volume list
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct VolumeListResponse {
    pub volumes: Vec<UiVolume>,
}

// =============================================================================
// Template Types
// =============================================================================

/// UI-compatible template representation
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiTemplate {
    pub id: String,
    pub node_id: String,
    pub name: String,
    pub size_bytes: u64,
    pub clone_count: u32,
    pub created_at: String,
}

impl From<TemplateData> for UiTemplate {
    fn from(data: TemplateData) -> Self {
        Self {
            id: data.id,
            node_id: data.spec.node_id,
            name: data.spec.name,
            size_bytes: data.status.size_bytes,
            clone_count: data.status.clone_count,
            created_at: data.created_at,
        }
    }
}

/// Request to import a template (UI-compatible)
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiImportTemplateRequest {
    #[serde(default)]
    pub node_id: String,
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub total_bytes: u64,
}

/// Response wrapper for template list
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TemplateListResponse {
    pub templates: Vec<UiTemplate>,
}

// =============================================================================
// Import Job Types
// =============================================================================

/// UI-compatible import job state
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
pub enum UiImportJobState {
    #[serde(rename = "PENDING")]
    Pending,
    #[serde(rename = "RUNNING")]
    Running,
    #[serde(rename = "COMPLETED")]
    Completed,
    #[serde(rename = "FAILED")]
    Failed,
}

impl From<TemplatePhase> for UiImportJobState {
    fn from(phase: TemplatePhase) -> Self {
        match phase {
            TemplatePhase::Pending => UiImportJobState::Pending,
            TemplatePhase::Importing => UiImportJobState::Running,
            TemplatePhase::Ready => UiImportJobState::Completed,
            TemplatePhase::Failed => UiImportJobState::Failed,
        }
    }
}

/// UI-compatible import job representation (backed by TemplateData)
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiImportJob {
    pub id: String,
    pub node_id: String,
    pub template_name: String,
    pub url: String,
    pub state: UiImportJobState,
    pub bytes_written: u64,
    pub total_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub created_at: String,
}

impl From<TemplateData> for UiImportJob {
    fn from(data: TemplateData) -> Self {
        Self {
            id: data.id,
            node_id: data.spec.node_id,
            template_name: data.spec.name,
            url: data.spec.source_url.unwrap_or_default(),
            state: data.status.phase.into(),
            bytes_written: data.status.bytes_written,
            total_bytes: data.status.total_bytes,
            error: data.status.error,
            created_at: data.created_at,
        }
    }
}

// =============================================================================
// Storage Pool Types
// =============================================================================

/// UI-compatible storage pool statistics
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiPoolStats {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub compression_ratio: f64,
}

// =============================================================================
// SSE Event Types
// =============================================================================

/// VM event for SSE streaming
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VmEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub vm: UiVm,
}

// =============================================================================
// Query Parameters
// =============================================================================

/// Query parameters for listing VMs
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListVmsQuery {
    #[serde(default)]
    pub node_id: Option<String>,
}

/// Query parameters for listing networks
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListNetworksQuery {}

/// Query parameters for listing NICs
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListNicsQuery {
    #[serde(default)]
    pub network_id: Option<String>,
}

/// Query parameters for listing volumes
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListVolumesQuery {
    #[serde(default)]
    pub node_id: Option<String>,
}

// =============================================================================
// Security Group Types
// =============================================================================

/// Rule direction
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
pub enum UiRuleDirection {
    #[serde(rename = "INGRESS")]
    Ingress,
    #[serde(rename = "EGRESS")]
    Egress,
}

/// Rule protocol
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
pub enum UiRuleProtocol {
    #[serde(rename = "ALL")]
    All,
    #[serde(rename = "TCP")]
    Tcp,
    #[serde(rename = "UDP")]
    Udp,
    #[serde(rename = "ICMP")]
    Icmp,
    #[serde(rename = "ICMPV6")]
    Icmpv6,
}

/// UI-compatible security group rule
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiSecurityGroupRule {
    pub id: String,
    pub security_group_id: String,
    pub direction: UiRuleDirection,
    pub protocol: UiRuleProtocol,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port_start: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port_end: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cidr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub created_at: String,
}

/// UI-compatible security group
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiSecurityGroup {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub rules: Vec<UiSecurityGroupRule>,
    pub nic_count: u32,
    pub created_at: String,
    pub updated_at: String,
}

impl From<crate::command::SecurityGroupData> for UiSecurityGroup {
    fn from(sg: crate::command::SecurityGroupData) -> Self {
        let sg_id = sg.id.clone();
        Self {
            id: sg.id,
            name: sg.name,
            description: sg.description,
            rules: sg
                .rules
                .into_iter()
                .map(|r| UiSecurityGroupRule {
                    id: r.id,
                    security_group_id: sg_id.clone(),
                    direction: match r.direction {
                        crate::command::RuleDirection::Inbound => UiRuleDirection::Ingress,
                        crate::command::RuleDirection::Outbound => UiRuleDirection::Egress,
                    },
                    protocol: r
                        .protocol
                        .as_deref()
                        .map(|p| match p {
                            "TCP" => UiRuleProtocol::Tcp,
                            "UDP" => UiRuleProtocol::Udp,
                            "ICMP" => UiRuleProtocol::Icmp,
                            "ICMPV6" => UiRuleProtocol::Icmpv6,
                            _ => UiRuleProtocol::All,
                        })
                        .unwrap_or(UiRuleProtocol::All),
                    port_start: r.port_range_start,
                    port_end: r.port_range_end,
                    cidr: r.cidr,
                    description: r.description,
                    created_at: r.created_at,
                })
                .collect(),
            nic_count: sg.nic_count,
            created_at: sg.created_at,
            updated_at: sg.updated_at,
        }
    }
}

impl UiRuleDirection {
    pub fn to_command_direction(self) -> crate::command::RuleDirection {
        match self {
            UiRuleDirection::Ingress => crate::command::RuleDirection::Inbound,
            UiRuleDirection::Egress => crate::command::RuleDirection::Outbound,
        }
    }
}

impl UiRuleProtocol {
    pub fn to_protocol_string(self) -> Option<String> {
        match self {
            UiRuleProtocol::All => None,
            UiRuleProtocol::Tcp => Some("TCP".into()),
            UiRuleProtocol::Udp => Some("UDP".into()),
            UiRuleProtocol::Icmp => Some("ICMP".into()),
            UiRuleProtocol::Icmpv6 => Some("ICMPV6".into()),
        }
    }
}

/// Request to create a security group
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiCreateSecurityGroupRequest {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// Request to patch a security group's mutable fields.
#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiUpdateSecurityGroupRequest {
    #[serde(default)]
    pub name: Option<String>,
    /// `description` follows the tri-state shape: absent → untouched,
    /// present-and-null → clear, present-with-value → set. The custom
    /// deserializer is required — plain `#[serde(default)]` collapses
    /// null to `None` at the outer level.
    #[serde(default, deserialize_with = "deserialize_tristate")]
    pub description: Option<Option<String>>,
}

/// Request to update a single rule's mutable fields. Currently only the
/// description is mutable. The wire shape always replaces description —
/// send the new string (or null to clear). To keep the field untouched,
/// don't call this endpoint; create/delete the rule instead for the
/// non-editable parts.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiUpdateSecurityGroupRuleRequest {
    /// The double-Option lets the cplane patch carry "set" vs "untouched".
    /// On the wire we collapse this to "always set" — the inner Option is
    /// the value (null = clear). The handler maps to the internal patch
    /// shape with `Some(req.description)`.
    #[serde(default, deserialize_with = "deserialize_tristate")]
    pub description: Option<Option<String>>,
}

/// Request to create a security group rule
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiCreateSecurityGroupRuleRequest {
    pub direction: UiRuleDirection,
    pub protocol: UiRuleProtocol,
    #[serde(default)]
    pub port_start: Option<u16>,
    #[serde(default)]
    pub port_end: Option<u16>,
    #[serde(default)]
    pub cidr: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// Response wrapper for security group list
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SecurityGroupListResponse {
    pub security_groups: Vec<UiSecurityGroup>,
}

// =============================================================================
// Pod / Container Types (stub)
// =============================================================================

/// Pod state
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub enum UiPodState {
    CREATED,
    STARTING,
    RUNNING,
    STOPPING,
    STOPPED,
    FAILED,
}

/// Container state
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub enum UiContainerState {
    CREATING,
    CREATED,
    RUNNING,
    STOPPED,
    FAILED,
}

/// A container within a pod
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiContainer {
    pub id: String,
    pub name: String,
    pub state: UiContainerState,
    pub image: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

/// A pod
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiPod {
    pub id: String,
    pub project_slug: String,
    pub name: String,
    pub state: UiPodState,
    pub network_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vm_id: Option<String>,
    pub containers: Vec<UiContainer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip_address: Option<String>,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

/// Container spec for creating a pod
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiContainerSpec {
    pub name: String,
    pub image: String,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Option<Vec<String>>,
    #[serde(default)]
    pub env: Option<std::collections::HashMap<String, String>>,
    #[serde(default)]
    pub working_dir: Option<String>,
}

/// Request to create a pod
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiCreatePodRequest {
    pub name: String,
    pub project_slug: String,
    pub network_id: String,
    pub containers: Vec<UiContainerSpec>,
}

/// Response wrapper for pod list
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PodListResponse {
    pub pods: Vec<UiPod>,
}

// =============================================================================
// ServiceAccount + StaticApiKey DTOs (ADR-0004)
// =============================================================================

/// Project-scoped ServiceAccount (an `Account` row with kind = ServiceAccount).
/// Memberships are managed via the existing Org/Project membership endpoints —
/// SA creation auto-grants project-admin in the home project.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiServiceAccount {
    pub id: String,
    pub project_slug: String,
    pub name: String,
    pub description: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl UiServiceAccount {
    pub fn from_account(a: crate::command::AccountData) -> Self {
        Self {
            id: a.id,
            project_slug: a.project_slug.unwrap_or_default(),
            name: a.display_name.unwrap_or_default(),
            description: a.description,
            created_at: a.created_at,
            updated_at: a.updated_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServiceAccountListResponse {
    pub service_accounts: Vec<UiServiceAccount>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiCreateServiceAccountRequest {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// API key metadata — `secret` is `None` everywhere except in the response
/// of `POST .../api-keys`, where the freshly-minted plaintext appears once
/// and is never returned again.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiApiKey {
    pub id: String,
    pub account_id: String,
    pub display_prefix: String,
    pub description: Option<String>,
    pub expires_at: Option<String>,
    pub last_used_at: Option<String>,
    pub revoked_at: Option<String>,
    pub created_at: String,
    /// One-time plaintext returned only on creation. The UI must show it
    /// once and discard it; the cplane stores only the BLAKE3 hash.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
}

impl UiApiKey {
    pub fn from_data(k: crate::command::ApiKeyData) -> Self {
        Self {
            id: k.id,
            account_id: k.account_id,
            display_prefix: k.display_prefix,
            description: k.description,
            expires_at: k.expires_at,
            last_used_at: k.last_used_at,
            revoked_at: k.revoked_at,
            created_at: k.created_at,
            secret: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeyListResponse {
    pub api_keys: Vec<UiApiKey>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiCreateApiKeyRequest {
    #[serde(default)]
    pub description: Option<String>,
    /// Caller-explicit RFC3339 expiry. `None` means the key never expires —
    /// ADR-0004 has no Org-default; rotation is the operator's responsibility.
    #[serde(default)]
    pub expires_at: Option<String>,
}
