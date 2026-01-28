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
        name: String,
        ipv4_enabled: bool,
        ipv4_subnet: Option<String>,
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
        network_id: String,
        name: Option<String>,
        mac_address: Option<String>,
        ipv4_address: Option<String>,
        ipv6_address: Option<String>,
        routed_ipv4_prefixes: Vec<String>,
        routed_ipv6_prefixes: Vec<String>,
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

    // Project operations
    CreateProject {
        request_id: String,
        id: String,
        timestamp: String,
        name: String,
        description: Option<String>,
    },
    DeleteProject {
        request_id: String,
        id: String,
    },

    // Volume operations (node_id for data locality - Shared Nothing architecture)
    CreateVolume {
        request_id: String,
        id: String,
        timestamp: String,
        project_id: String,
        node_id: String,
        name: String,
        size_bytes: u64,
        template_id: Option<String>,
    },
    DeleteVolume {
        request_id: String,
        id: String,
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
        node_id: String,
        name: String,
        size_bytes: u64,
    },

    // Import job operations
    CreateImportJob {
        request_id: String,
        id: String,
        timestamp: String,
        node_id: String,
        template_name: String,
        url: String,
        total_bytes: u64,
    },
    UpdateImportJob {
        request_id: String,
        id: String,
        timestamp: String,
        bytes_written: u64,
        state: ImportJobState,
        error: Option<String>,
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
            Command::CreateVm { request_id, .. } => request_id,
            Command::UpdateVmSpec { request_id, .. } => request_id,
            Command::UpdateVmStatus { request_id, .. } => request_id,
            Command::DeleteVm { request_id, .. } => request_id,
            Command::CreateProject { request_id, .. } => request_id,
            Command::DeleteProject { request_id, .. } => request_id,
            Command::CreateVolume { request_id, .. } => request_id,
            Command::DeleteVolume { request_id, .. } => request_id,
            Command::ResizeVolume { request_id, .. } => request_id,
            Command::CreateSnapshot { request_id, .. } => request_id,
            Command::CreateTemplate { request_id, .. } => request_id,
            Command::CreateImportJob { request_id, .. } => request_id,
            Command::UpdateImportJob { request_id, .. } => request_id,
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
    pub name: String,
    pub ipv4_enabled: bool,
    pub ipv4_subnet: Option<String>,
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
    pub name: Option<String>,
    pub network_id: String,
    pub mac_address: String,
    pub ipv4_address: Option<String>,
    pub ipv6_address: Option<String>,
    pub routed_ipv4_prefixes: Vec<String>,
    pub routed_ipv6_prefixes: Vec<String>,
    pub socket_path: String,
    pub state: NicStateData,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum NicStateData {
    Created,
    Active,
    Error,
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
    pub project_id: Option<String>,    // Project this VM belongs to
    pub node_selector: Option<String>, // Optional: require specific node
    pub cpu_cores: u32,
    pub memory_mb: u64,
    pub disk_gb: u64,
    pub network_id: String,
    pub nic_id: Option<String>, // Will be auto-created if not provided
    pub image: String,          // Boot image reference
    pub disks: Vec<DiskConfig>, // Additional disk volumes
    pub desired_state: VmDesiredState,
}

/// Disk configuration for a VM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskConfig {
    pub volume_id: String,
    pub readonly: bool,
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

/// Project data stored in the state machine
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectData {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub created_at: String,
}

// =============================================================================
// Storage Types (Volumes, Templates, Import Jobs)
// =============================================================================

/// Volume data stored in the state machine (bound to a specific node)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeData {
    pub id: String,
    pub project_id: String,
    pub node_id: String, // Node where the volume is stored (Shared Nothing)
    pub name: String,
    pub path: String, // ZFS path e.g., /dev/zvol/pool/vol-xxx
    pub size_bytes: u64,
    pub used_bytes: u64,
    pub compression_ratio: f64,
    pub snapshots: Vec<SnapshotData>,
    pub template_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
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
    pub node_id: String, // Node where the template is stored
    pub name: String,
    pub size_bytes: u64,
    pub clone_count: u32,
    pub created_at: String,
}

/// Import job data stored in the state machine
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportJobData {
    pub id: String,
    pub node_id: String,
    pub template_name: String,
    pub url: String,
    pub state: ImportJobState,
    pub bytes_written: u64,
    pub total_bytes: u64,
    pub error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Import job state
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ImportJobState {
    #[default]
    Pending,
    Running,
    Completed,
    Failed,
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
    Project(ProjectData),
    Volume(VolumeData),
    Template(TemplateData),
    ImportJob(ImportJobData),
    Deleted { id: String },
    DeletedWithCount { id: String, nics_deleted: u32 },
    Error { code: u32, message: String },
}

impl Default for Response {
    fn default() -> Self {
        Response::Error {
            code: 0,
            message: "No response".to_string(),
        }
    }
}
