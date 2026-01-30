//! UI-compatible DTO types with camelCase serialization.
//!
//! These types match the mock-server's JSON structure for compatibility with mvirt-ui.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::command::{
    ImportJobData, ImportJobState, NetworkData, NicData, ProjectData, SnapshotData, TemplateData,
    VmData, VmDesiredState, VmPhase, VolumeData,
};

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
    pub project_id: String,
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
            project_id: data.spec.project_id.clone(),
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
    pub project_id: String,
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
            project_id: data.project_id,
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
            name: data.name,
            network_id: data.network_id,
            mac_address: data.mac_address,
            ipv4_address: data.ipv4_address,
            ipv6_address: data.ipv6_address,
            vm_id: data.vm_id,
            state: format!("{:?}", data.state),
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

/// UI-compatible project representation
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiProject {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub created_at: String,
}

impl From<ProjectData> for UiProject {
    fn from(data: ProjectData) -> Self {
        Self {
            id: data.id,
            name: data.name,
            description: data.description,
            created_at: data.created_at,
        }
    }
}

/// Request to create a project (UI-compatible)
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UiCreateProjectRequest {
    /// User-provided project ID (must be unique, lowercase alphanumeric)
    pub id: String,
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
    pub project_id: String,
    pub node_id: String,
    pub name: String,
    pub size_bytes: u64,
    pub used_bytes: u64,
    pub compression_ratio: f64,
    pub snapshots: Vec<UiSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template_id: Option<String>,
    pub created_at: String,
}

impl From<VolumeData> for UiVolume {
    fn from(data: VolumeData) -> Self {
        Self {
            id: data.id,
            project_id: data.project_id,
            node_id: data.node_id,
            name: data.name,
            size_bytes: data.size_bytes,
            used_bytes: data.used_bytes,
            compression_ratio: data.compression_ratio,
            snapshots: data.snapshots.iter().map(UiSnapshot::from).collect(),
            template_id: data.template_id,
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
            node_id: data.node_id,
            name: data.name,
            size_bytes: data.size_bytes,
            clone_count: data.clone_count,
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

impl From<ImportJobState> for UiImportJobState {
    fn from(state: ImportJobState) -> Self {
        match state {
            ImportJobState::Pending => UiImportJobState::Pending,
            ImportJobState::Running => UiImportJobState::Running,
            ImportJobState::Completed => UiImportJobState::Completed,
            ImportJobState::Failed => UiImportJobState::Failed,
        }
    }
}

/// UI-compatible import job representation
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

impl From<ImportJobData> for UiImportJob {
    fn from(data: ImportJobData) -> Self {
        Self {
            id: data.id,
            node_id: data.node_id,
            template_name: data.template_name,
            url: data.url,
            state: data.state.into(),
            bytes_written: data.bytes_written,
            total_bytes: data.total_bytes,
            error: data.error,
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
    pub project_id: String,
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
    pub project_id: String,
    pub network_id: String,
    pub containers: Vec<UiContainerSpec>,
}

/// Response wrapper for pod list
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PodListResponse {
    pub pods: Vec<UiPod>,
}
