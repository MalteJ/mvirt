use tokio::sync::mpsc;

use crate::proto::{SystemInfo, Vm};

#[derive(Clone, Copy, PartialEq, Default)]
pub enum EscapeState {
    #[default]
    Normal,
    SawCtrlA,
}

#[derive(Clone, Copy, PartialEq, Default)]
pub enum CreateBootMode {
    #[default]
    Disk,
    Kernel,
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
    pub boot_mode: i32,
    pub kernel: Option<String>,
    pub initramfs: Option<String>,
    pub cmdline: Option<String>,
    pub disk: String,
    pub vcpus: u32,
    pub memory_mb: u64,
    pub nested_virt: bool,
    pub user_data_mode: UserDataMode,
    pub user_data_file: Option<String>,
    pub ssh_keys_config: Option<SshKeysConfig>,
}

pub enum Action {
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
}

pub enum ActionResult {
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
}
