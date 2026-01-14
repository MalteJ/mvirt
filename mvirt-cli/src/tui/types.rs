use tokio::sync::mpsc;

use crate::net_proto::{Network, Nic};
use crate::proto::{SystemInfo, Vm};
use crate::zfs_proto::{ImportJob, PoolStats, Template, Volume};
use mvirt_log::LogEntry;

#[derive(Clone, Copy, PartialEq, Default)]
pub enum EscapeState {
    #[default]
    Normal,
    SawCtrlA,
}

/// Source type for VM boot disk
#[derive(Clone, Copy, PartialEq, Default)]
pub enum DiskSourceType {
    #[default]
    Template,
    Volume,
}

#[derive(Clone, Copy, PartialEq, Default)]
pub enum UserDataMode {
    #[default]
    None,
    SshKeys,
    File,
}

#[derive(Clone, Copy, PartialEq, Default)]
pub enum SshKeySource {
    #[default]
    GitHub,
    Local,
}

#[derive(Default, Clone)]
pub struct SshKeysConfig {
    pub username: String,
    pub source: SshKeySource,
    pub github_user: String,
    pub local_path: String,
    pub root_password: String,
}

impl SshKeysConfig {
    pub fn new() -> Self {
        Self {
            local_path: dirs::home_dir()
                .map(|p| p.join(".ssh/id_rsa.pub").to_string_lossy().to_string())
                .unwrap_or_else(|| "~/.ssh/id_rsa.pub".to_string()),
            ..Default::default()
        }
    }
}

#[derive(Clone)]
pub struct CreateVmParams {
    pub name: Option<String>,
    pub disk_source_type: DiskSourceType,
    pub disk_name: String, // volume or template name
    pub vcpus: u32,
    pub memory_mb: u64,
    pub nested_virt: bool,
    pub user_data_mode: UserDataMode,
    pub user_data_file: Option<String>,
    pub ssh_keys_config: Option<SshKeysConfig>,
    pub network_id: Option<String>, // Network to join (creates vNIC automatically)
}

/// Network item for selection in VM create modal
#[derive(Clone)]
pub struct NetworkItem {
    pub id: String,
    pub name: String,
}

/// Active view in the TUI
#[derive(Clone, Copy, PartialEq, Default)]
pub enum View {
    #[default]
    Vm,
    Storage,
    Network,
    Logs,
}

/// Focus within storage view
#[derive(Clone, Copy, PartialEq, Default)]
pub enum StorageFocus {
    #[default]
    Volumes,
    Templates,
}

/// Storage state from mvirt-zfs
#[derive(Default)]
pub struct StorageState {
    pub pool: Option<PoolStats>,
    pub volumes: Vec<Volume>,
    pub templates: Vec<Template>,
    pub import_jobs: Vec<ImportJob>,
}

/// Focus within network view
#[derive(Clone, Copy, PartialEq, Default)]
pub enum NetworkFocus {
    #[default]
    Networks,
    Nics,
}

/// Network state from mvirt-net
#[derive(Default)]
pub struct NetworkState {
    pub networks: Vec<Network>,
    pub nics: Vec<Nic>,
    pub selected_network_id: Option<String>,
}

#[allow(dead_code)] // Storage actions will be used when modals are implemented
pub enum Action {
    // VM actions
    Refresh,
    RefreshSystemInfo,
    Start(String),
    Stop(String),
    Kill(String),
    Delete(String),
    Create(Box<CreateVmParams>),
    OpenConsole {
        vm_id: String,
        vm_name: Option<String>,
    },

    // Storage actions
    RefreshStorage,
    CreateVolume {
        name: String,
        size_bytes: u64,
    },
    DeleteVolume(String),
    ResizeVolume {
        name: String,
        new_size: u64,
    },
    ImportVolume {
        name: String,
        source: String,
    },
    CancelImport(String),
    CreateSnapshot {
        volume: String,
        name: String,
    },
    DeleteSnapshot {
        volume: String,
        name: String,
    },
    RollbackSnapshot {
        volume: String,
        name: String,
    },
    PromoteSnapshot {
        volume: String,
        snapshot: String,
        template_name: String,
    },
    DeleteTemplate(String),
    CloneTemplate {
        template: String,
        new_volume: String,
        size_bytes: Option<u64>,
    },

    // Log actions
    RefreshLogs {
        limit: u32,
    },

    // Modal preparation
    PrepareCreateVmModal,
    PrepareVmDetailModal {
        vm_id: String,
    },
    PrepareVolumeDetailModal {
        volume_name: String,
    },

    // Network actions
    RefreshNetworks,
    CreateNetwork {
        name: String,
        ipv4_subnet: Option<String>,
        ipv6_prefix: Option<String>,
    },
    DeleteNetwork {
        id: String,
    },
    LoadNics {
        network_id: String,
    },
    CreateNic {
        network_id: String,
        name: Option<String>,
    },
    DeleteNic {
        id: String,
    },
}

pub enum ActionResult {
    // VM results
    Refreshed(Result<Vec<Vm>, String>),
    SystemInfoRefreshed(Result<SystemInfo, String>),
    Started(String, Result<(), String>),
    Stopped(String, Result<(), String>),
    Killed(String, Result<(), String>),
    Deleted(String, Result<(), String>),
    Created(Result<String, String>),
    ConsoleOpened {
        vm_id: String,
        vm_name: Option<String>,
        input_tx: mpsc::UnboundedSender<Vec<u8>>,
    },
    ConsoleOutput(Vec<u8>),
    ConsoleClosed(Option<String>),

    // Storage results
    StorageRefreshed(Result<StorageState, String>),
    VolumeCreated(Result<(), String>),
    VolumeDeleted(Result<(), String>),
    VolumeResized(Result<(), String>),
    ImportStarted(Result<String, String>),
    ImportCancelled(Result<(), String>),
    SnapshotCreated(Result<(), String>),
    SnapshotDeleted(Result<(), String>),
    SnapshotRolledBack(Result<(), String>),
    TemplateCreated(Result<(), String>),
    TemplateDeleted(Result<(), String>),
    VolumeCloned(Result<(), String>),

    // Log results
    LogsRefreshed(Result<Vec<LogEntry>, String>),

    // Modal preparation results
    CreateVmModalReady {
        templates: Vec<Template>,
        volumes: Vec<Volume>,
        networks: Vec<Network>,
    },
    VmDetailModalReady {
        vm_id: String,
        logs: Vec<LogEntry>,
    },
    VolumeDetailModalReady {
        volume_name: String,
        logs: Vec<LogEntry>,
    },

    // Network results
    NetworksRefreshed(Result<Vec<Network>, String>),
    NetworkCreated(Result<Network, String>),
    NetworkDeleted(Result<(), String>),
    NicsLoaded(Result<Vec<Nic>, String>),
    NicCreated(Result<Nic, String>),
    NicDeleted(Result<(), String>),
}
