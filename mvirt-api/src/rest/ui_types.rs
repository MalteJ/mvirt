//! UI-compatible DTO types with camelCase serialization.
//!
//! These types match the mock-server's JSON structure for compatibility with mvirt-ui.

use serde::{Deserialize, Serialize};

use crate::command::{
    DiskConfig, ImportJobData, ImportJobState, NetworkData, NicData, ProjectData, SnapshotData,
    TemplateData, VmData, VmDesiredState, VmPhase, VolumeData,
};

// =============================================================================
// VM Types
// =============================================================================

/// UI-compatible VM state enum
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
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

/// UI-compatible disk configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiDiskConfig {
    pub volume_id: String,
    pub readonly: bool,
}

impl From<&DiskConfig> for UiDiskConfig {
    fn from(d: &DiskConfig) -> Self {
        Self {
            volume_id: d.volume_id.clone(),
            readonly: d.readonly,
        }
    }
}

/// UI-compatible NIC configuration for VM
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiNicConfig {
    pub nic_id: String,
}

/// UI-compatible VM configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiVmConfig {
    pub vcpus: u32,
    pub memory_mb: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kernel_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub boot_disk: Option<String>,
    pub disks: Vec<UiDiskConfig>,
    pub nics: Vec<UiNicConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_data: Option<String>,
}

/// UI-compatible VM representation
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiVm {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
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
            project_id: data.spec.project_id,
            name: data.spec.name,
            state,
            config: UiVmConfig {
                vcpus: data.spec.cpu_cores,
                memory_mb: data.spec.memory_mb,
                kernel_path: None,
                boot_disk: Some(data.spec.image.clone()),
                disks: data.spec.disks.iter().map(UiDiskConfig::from).collect(),
                nics: data
                    .spec
                    .nic_id
                    .map(|id| vec![UiNicConfig { nic_id: id }])
                    .unwrap_or_default(),
                user_data: None,
            },
            created_at: data.created_at,
            started_at,
            node_id: data.status.node_id,
            ip_address: data.status.ip_address,
        }
    }
}

/// Request to create a VM (UI-compatible)
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiCreateVmRequest {
    pub name: String,
    #[serde(default)]
    pub project_id: Option<String>,
    pub config: UiCreateVmConfig,
    #[serde(default)]
    pub node_selector: Option<String>,
}

/// VM configuration for creation (UI-compatible)
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiCreateVmConfig {
    pub vcpus: u32,
    pub memory_mb: u64,
    #[serde(default)]
    pub kernel_path: Option<String>,
    #[serde(default)]
    pub boot_disk: Option<String>,
    #[serde(default)]
    pub disks: Vec<UiDiskConfig>,
    #[serde(default)]
    pub nics: Vec<UiNicConfig>,
    #[serde(default)]
    pub user_data: Option<String>,
}

/// Response wrapper for VM list
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VmListResponse {
    pub vms: Vec<UiVm>,
}

// =============================================================================
// Network Types
// =============================================================================

/// UI-compatible network representation
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiNetwork {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub name: String,
    pub ipv4_enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipv4_subnet: Option<String>,
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
            project_id: None, // Networks don't have project_id in internal model yet
            name: data.name,
            ipv4_enabled: data.ipv4_enabled,
            ipv4_subnet: data.ipv4_subnet,
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
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiCreateNetworkRequest {
    pub name: String,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default = "default_true")]
    pub ipv4_enabled: bool,
    #[serde(default)]
    pub ipv4_subnet: Option<String>,
    #[serde(default)]
    pub ipv6_enabled: bool,
    #[serde(default)]
    pub ipv6_prefix: Option<String>,
    #[serde(default)]
    pub dns_servers: Vec<String>,
    #[serde(default)]
    pub is_public: bool,
}

fn default_true() -> bool {
    true
}

/// Response wrapper for network list
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkListResponse {
    pub networks: Vec<UiNetwork>,
}

// =============================================================================
// NIC Types
// =============================================================================

/// UI-compatible NIC representation
#[derive(Debug, Clone, Serialize, Deserialize)]
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
            vm_id: None, // Would need to look up from VMs
            state: format!("{:?}", data.state),
            created_at: data.created_at,
        }
    }
}

/// Request to create a NIC (UI-compatible)
#[derive(Debug, Clone, Deserialize)]
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
}

/// Response wrapper for NIC list
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NicListResponse {
    pub nics: Vec<UiNic>,
}

// =============================================================================
// Project Types
// =============================================================================

/// UI-compatible project representation
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiCreateProjectRequest {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// Response wrapper for project list
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectListResponse {
    pub projects: Vec<UiProject>,
}

// =============================================================================
// Volume Types
// =============================================================================

/// UI-compatible snapshot representation
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiCreateVolumeRequest {
    pub project_id: String,
    pub node_id: String,
    pub name: String,
    pub size_bytes: u64,
    #[serde(default)]
    pub template_id: Option<String>,
}

/// Request to resize a volume (UI-compatible)
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiResizeVolumeRequest {
    pub size_bytes: u64,
}

/// Request to create a snapshot (UI-compatible)
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiCreateSnapshotRequest {
    pub name: String,
}

/// Response wrapper for volume list
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeListResponse {
    pub volumes: Vec<UiVolume>,
}

// =============================================================================
// Template Types
// =============================================================================

/// UI-compatible template representation
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiImportTemplateRequest {
    pub node_id: String,
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub total_bytes: u64,
}

/// Response wrapper for template list
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateListResponse {
    pub templates: Vec<UiTemplate>,
}

// =============================================================================
// Import Job Types
// =============================================================================

/// UI-compatible import job state
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize)]
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
    pub project_id: Option<String>,
    #[serde(default)]
    pub node_id: Option<String>,
}

/// Query parameters for listing networks
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListNetworksQuery {
    #[serde(default)]
    pub project_id: Option<String>,
}

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
    pub project_id: Option<String>,
    #[serde(default)]
    pub node_id: Option<String>,
}
