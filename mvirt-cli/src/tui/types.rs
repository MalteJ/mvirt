use tokio::sync::mpsc;

use crate::proto::{SystemInfo, Vm};
use crate::zfs_proto::{ImportJob, PoolStats, Template, Volume};

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
}

/// Active view in the TUI
#[derive(Clone, Copy, PartialEq, Default)]
pub enum View {
    #[default]
    Vm,
    Storage,
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
        size_bytes: Option<u64>,
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
    CreateTemplate {
        volume: String,
        name: String,
    },
    DeleteTemplate(String),
    CloneTemplate {
        template: String,
        new_volume: String,
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
}
