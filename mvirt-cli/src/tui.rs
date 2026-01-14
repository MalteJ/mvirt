use std::io;
use std::path::PathBuf;
use std::time::Duration;

use chrono::Local;

use crossterm::ExecutableCommand;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState};
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tonic::transport::Channel;

use crate::proto::vm_service_client::VmServiceClient;
use crate::proto::*;

pub(crate) enum Action {
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

#[derive(Clone)]
pub(crate) struct CreateVmParams {
    pub name: Option<String>,
    pub boot_mode: i32, // 1=disk, 2=kernel
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

pub(crate) enum ActionResult {
    Refreshed(Result<Vec<Vm>, String>),
    SystemInfoRefreshed(Result<SystemInfo, String>),
    Started(String, Result<(), String>),
    Stopped(String, Result<(), String>),
    Killed(String, Result<(), String>),
    Deleted(String, Result<(), String>),
    Created(Result<String, String>), // Ok(vm_id) or Err(error)
    ConsoleOpened {
        vm_id: String,
        vm_name: Option<String>,
        input_tx: mpsc::UnboundedSender<Vec<u8>>,
    },
    ConsoleOutput(Vec<u8>),
    ConsoleClosed(Option<String>), // Optional error message
}

#[derive(Clone, Copy, PartialEq, Default)]
enum EscapeState {
    #[default]
    Normal,
    SawCtrlA, // Waiting for 't' to exit
}

struct ConsoleSession {
    vm_id: String,
    vm_name: Option<String>,
    parser: vt100::Parser,
    escape_state: EscapeState,
    input_tx: mpsc::UnboundedSender<Vec<u8>>,
}

struct FilePicker {
    current_path: PathBuf,
    entries: Vec<PathBuf>,
    selected: usize,
    scroll_offset: usize,
    target_field: usize, // Which CreateModal field to populate
}

impl FilePicker {
    fn new(start_path: PathBuf, target_field: usize) -> Self {
        let mut picker = Self {
            current_path: start_path,
            entries: Vec::new(),
            selected: 0,
            scroll_offset: 0,
            target_field,
        };
        picker.refresh_entries();
        picker
    }

    fn refresh_entries(&mut self) {
        self.entries.clear();

        // Add parent directory entry if not at root
        if self.current_path.parent().is_some() {
            self.entries.push(PathBuf::from(".."));
        }

        // Read directory contents
        if let Ok(read_dir) = std::fs::read_dir(&self.current_path) {
            let mut dirs: Vec<PathBuf> = Vec::new();
            let mut files: Vec<PathBuf> = Vec::new();

            for entry in read_dir.flatten() {
                let path = entry.path();
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                // Skip hidden files
                if name.starts_with('.') {
                    continue;
                }
                if path.is_dir() {
                    dirs.push(path);
                } else {
                    files.push(path);
                }
            }

            // Sort directories and files separately
            dirs.sort();
            files.sort();

            // Add directories first, then files
            self.entries.extend(dirs);
            self.entries.extend(files);
        }

        self.selected = 0;
        self.scroll_offset = 0;
    }

    fn select_next(&mut self) {
        if !self.entries.is_empty() {
            self.selected = (self.selected + 1) % self.entries.len();
        }
    }

    fn select_prev(&mut self) {
        if !self.entries.is_empty() {
            self.selected = if self.selected == 0 {
                self.entries.len() - 1
            } else {
                self.selected - 1
            };
        }
    }

    fn enter_selected(&mut self) -> Option<PathBuf> {
        let entry = self.entries.get(self.selected)?;

        if entry == &PathBuf::from("..") {
            // Go to parent directory
            if let Some(parent) = self.current_path.parent() {
                self.current_path = parent.to_path_buf();
                self.refresh_entries();
            }
            None
        } else if entry.is_dir() {
            // Enter directory
            self.current_path = entry.clone();
            self.refresh_entries();
            None
        } else {
            // Select file
            Some(entry.clone())
        }
    }
}

#[derive(Clone, Copy, PartialEq, Default)]
enum CreateBootMode {
    #[default]
    Disk,
    Kernel,
}

#[derive(Clone, Copy, PartialEq, Default)]
pub(crate) enum UserDataMode {
    #[default]
    None,
    SshKeys,
    File,
}

#[derive(Clone, Copy, PartialEq, Default)]
pub(crate) enum SshKeySource {
    #[default]
    GitHub,
    Local,
}

#[derive(Default, Clone)]
pub(crate) struct SshKeysConfig {
    username: String,
    source: SshKeySource,
    github_user: String,
    local_path: String,
    root_password: String,
}

impl SshKeysConfig {
    fn new() -> Self {
        Self {
            local_path: dirs::home_dir()
                .map(|p| p.join(".ssh/id_rsa.pub").to_string_lossy().to_string())
                .unwrap_or_else(|| "~/.ssh/id_rsa.pub".to_string()),
            ..Default::default()
        }
    }
}

struct SshKeysModal {
    config: SshKeysConfig,
    focused_field: usize, // 0=username, 1=source, 2=github/path, 3=root_password, 4=add, 5=cancel
}

impl SshKeysModal {
    fn new() -> Self {
        Self {
            config: SshKeysConfig::new(),
            focused_field: 0,
        }
    }

    fn field_count(&self) -> usize {
        6 // username, source, github/path, root_password, add, cancel
    }

    fn focus_next(&mut self) {
        self.focused_field = (self.focused_field + 1) % self.field_count();
    }

    fn focus_prev(&mut self) {
        self.focused_field = if self.focused_field == 0 {
            self.field_count() - 1
        } else {
            self.focused_field - 1
        };
    }

    fn toggle_source(&mut self) {
        self.config.source = match self.config.source {
            SshKeySource::GitHub => SshKeySource::Local,
            SshKeySource::Local => SshKeySource::GitHub,
        };
    }

    fn current_input(&mut self) -> Option<&mut String> {
        match self.focused_field {
            0 => Some(&mut self.config.username),
            2 => match self.config.source {
                SshKeySource::GitHub => Some(&mut self.config.github_user),
                SshKeySource::Local => Some(&mut self.config.local_path),
            },
            3 => Some(&mut self.config.root_password),
            _ => None,
        }
    }

    fn is_source_field(&self) -> bool {
        self.focused_field == 1
    }

    fn is_add_button(&self) -> bool {
        self.focused_field == 4
    }

    fn is_cancel_button(&self) -> bool {
        self.focused_field == 5
    }

    fn validate(&self) -> Result<(), &'static str> {
        if self.config.username.is_empty() {
            return Err("Username is required");
        }
        match self.config.source {
            SshKeySource::GitHub => {
                if self.config.github_user.is_empty() {
                    return Err("GitHub username is required");
                }
            }
            SshKeySource::Local => {
                if self.config.local_path.is_empty() {
                    return Err("Key file path is required");
                }
            }
        }
        Ok(())
    }
}

#[derive(Default)]
struct CreateModal {
    name: String,
    boot_mode: CreateBootMode,
    kernel: String,
    initramfs: String,
    cmdline: String,
    disk: String,
    vcpus: String,
    memory_mb: String,
    nested_virt: bool,
    user_data_mode: UserDataMode,
    user_data_file: String,
    ssh_keys_config: Option<SshKeysConfig>,
    focused_field: usize, // 0=name, 1=boot_mode, 2=kernel, 3=initramfs, 4=cmdline, 5=disk, 6=vcpus, 7=memory, 8=nested_virt, 9=user_data_mode, 10=submit
}

impl CreateModal {
    fn new() -> Self {
        Self {
            vcpus: "1".to_string(),
            memory_mb: "512".to_string(),
            boot_mode: CreateBootMode::Disk,
            user_data_mode: UserDataMode::None,
            ..Default::default()
        }
    }

    fn field_count() -> usize {
        11 // name, boot_mode, kernel, initramfs, cmdline, disk, vcpus, memory, nested_virt, user_data_mode, submit
    }

    fn focus_next(&mut self) {
        loop {
            self.focused_field = (self.focused_field + 1) % Self::field_count();
            if self.is_field_visible(self.focused_field) {
                break;
            }
        }
    }

    fn focus_prev(&mut self) {
        loop {
            self.focused_field = if self.focused_field == 0 {
                Self::field_count() - 1
            } else {
                self.focused_field - 1
            };
            if self.is_field_visible(self.focused_field) {
                break;
            }
        }
    }

    fn is_field_visible(&self, field: usize) -> bool {
        match field {
            // Kernel, Initramfs, Cmdline only visible in Kernel mode
            2..=4 => self.boot_mode == CreateBootMode::Kernel,
            // All other fields always visible
            _ => true,
        }
    }

    fn current_input(&mut self) -> Option<&mut String> {
        match self.focused_field {
            0 => Some(&mut self.name),
            // 1 is boot_mode toggle, not text input
            2 => Some(&mut self.kernel),
            3 => Some(&mut self.initramfs),
            4 => Some(&mut self.cmdline),
            5 => Some(&mut self.disk),
            6 => Some(&mut self.vcpus),
            7 => Some(&mut self.memory_mb),
            // 8 is nested_virt toggle, not text input
            // 9 is user_data_mode toggle, not text input
            _ => None,
        }
    }

    fn is_name_field(&self) -> bool {
        self.focused_field == 0
    }

    fn is_boot_mode_field(&self) -> bool {
        self.focused_field == 1
    }

    fn is_file_field(&self) -> bool {
        // kernel=2, initramfs=3, disk=5
        // Only return true if field is visible
        match self.focused_field {
            2 | 3 => self.boot_mode == CreateBootMode::Kernel,
            5 => true,
            _ => false,
        }
    }

    fn is_nested_virt_field(&self) -> bool {
        self.focused_field == 8
    }

    fn toggle_nested_virt(&mut self) {
        self.nested_virt = !self.nested_virt;
    }

    fn is_user_data_mode_field(&self) -> bool {
        self.focused_field == 9
    }

    fn cycle_user_data_mode(&mut self) {
        self.user_data_mode = match self.user_data_mode {
            UserDataMode::None => UserDataMode::SshKeys,
            UserDataMode::SshKeys => UserDataMode::File,
            UserDataMode::File => UserDataMode::None,
        };
        // Clear the other mode's data when switching
        match self.user_data_mode {
            UserDataMode::None => {
                self.ssh_keys_config = None;
                self.user_data_file.clear();
            }
            UserDataMode::SshKeys => {
                self.user_data_file.clear();
            }
            UserDataMode::File => {
                self.ssh_keys_config = None;
            }
        }
    }

    fn is_numeric_field(&self) -> bool {
        matches!(self.focused_field, 6 | 7) // vcpus, memory_mb
    }

    fn is_valid_name_char(c: char) -> bool {
        c.is_ascii_alphanumeric() || c == '-' || c == '_'
    }

    fn set_field(&mut self, field: usize, value: String) {
        match field {
            0 => self.name = value,
            2 => self.kernel = value,
            3 => self.initramfs = value,
            4 => self.cmdline = value,
            5 => self.disk = value,
            6 => self.vcpus = value,
            7 => self.memory_mb = value,
            _ => {}
        }
    }

    fn set_user_data_file(&mut self, path: String) {
        self.user_data_file = path;
    }

    fn set_ssh_keys_config(&mut self, config: SshKeysConfig) {
        self.ssh_keys_config = Some(config);
    }

    fn toggle_boot_mode(&mut self) {
        self.boot_mode = match self.boot_mode {
            CreateBootMode::Disk => CreateBootMode::Kernel,
            CreateBootMode::Kernel => CreateBootMode::Disk,
        };
    }

    fn validate(&self) -> Result<CreateVmParams, &'static str> {
        match self.boot_mode {
            CreateBootMode::Disk => {
                if self.disk.is_empty() {
                    return Err("Disk path is required for disk boot");
                }
            }
            CreateBootMode::Kernel => {
                if self.kernel.is_empty() {
                    return Err("Kernel path is required for kernel boot");
                }
            }
        }

        // Validate user data mode
        match self.user_data_mode {
            UserDataMode::None => {}
            UserDataMode::SshKeys => {
                if self.ssh_keys_config.is_none() {
                    return Err("SSH keys not configured - press Enter to configure");
                }
            }
            UserDataMode::File => {
                if self.user_data_file.is_empty() {
                    return Err("User-data file not selected - press Enter to browse");
                }
            }
        }

        let vcpus: u32 = self.vcpus.parse().map_err(|_| "Invalid vcpus")?;
        let memory_mb: u64 = self.memory_mb.parse().map_err(|_| "Invalid memory")?;

        let boot_mode = match self.boot_mode {
            CreateBootMode::Disk => 1,
            CreateBootMode::Kernel => 2,
        };

        Ok(CreateVmParams {
            name: if self.name.is_empty() {
                None
            } else {
                Some(self.name.clone())
            },
            boot_mode,
            kernel: if self.kernel.is_empty() {
                None
            } else {
                Some(self.kernel.clone())
            },
            initramfs: if self.initramfs.is_empty() {
                None
            } else {
                Some(self.initramfs.clone())
            },
            cmdline: if self.cmdline.is_empty() {
                None
            } else {
                Some(self.cmdline.clone())
            },
            disk: self.disk.clone(),
            vcpus,
            memory_mb,
            nested_virt: self.nested_virt,
            user_data_mode: self.user_data_mode,
            user_data_file: if self.user_data_file.is_empty() {
                None
            } else {
                Some(self.user_data_file.clone())
            },
            ssh_keys_config: self.ssh_keys_config.clone(),
        })
    }
}

pub struct App {
    vms: Vec<Vm>,
    system_info: Option<SystemInfo>,
    table_state: TableState,
    should_quit: bool,
    status_message: Option<String>,
    action_tx: mpsc::UnboundedSender<Action>,
    result_rx: mpsc::UnboundedReceiver<ActionResult>,
    busy: bool,
    confirm_delete: Option<String>, // VM ID pending deletion
    confirm_kill: Option<String>,   // VM ID pending kill
    last_refresh: Option<chrono::DateTime<chrono::Local>>,
    create_modal: Option<CreateModal>,
    file_picker: Option<FilePicker>,
    file_picker_for_user_data: bool, // True if file picker is for user-data, false for create modal fields
    ssh_keys_modal: Option<SshKeysModal>,
    detail_view: Option<String>, // VM ID for detail view
    console_session: Option<ConsoleSession>,
}

impl App {
    pub fn new(
        action_tx: mpsc::UnboundedSender<Action>,
        result_rx: mpsc::UnboundedReceiver<ActionResult>,
    ) -> Self {
        Self {
            vms: Vec::new(),
            system_info: None,
            table_state: TableState::default(),
            should_quit: false,
            status_message: None,
            action_tx,
            result_rx,
            busy: false,
            confirm_delete: None,
            confirm_kill: None,
            last_refresh: None,
            create_modal: None,
            file_picker: None,
            file_picker_for_user_data: false,
            ssh_keys_modal: None,
            detail_view: None,
            console_session: None,
        }
    }

    fn send_action(&mut self, action: Action) {
        if self.busy {
            return;
        }
        self.busy = true;
        let _ = self.action_tx.send(action);
    }

    fn handle_result(&mut self, result: ActionResult) {
        self.busy = false;
        match result {
            ActionResult::Refreshed(Ok(vms)) => {
                self.vms = vms;
                self.status_message = None;
                self.last_refresh = Some(Local::now());
                if self.vms.is_empty() {
                    self.table_state.select(None);
                } else if self.table_state.selected().is_none() {
                    self.table_state.select(Some(0));
                } else if let Some(selected) = self.table_state.selected()
                    && selected >= self.vms.len()
                {
                    self.table_state
                        .select(Some(self.vms.len().saturating_sub(1)));
                }
            }
            ActionResult::Refreshed(Err(e)) => {
                self.status_message = Some(format!("Error: {}", e));
            }
            ActionResult::SystemInfoRefreshed(Ok(info)) => {
                self.system_info = Some(info);
            }
            ActionResult::SystemInfoRefreshed(Err(_)) => {
                // Silently ignore system info errors
            }
            ActionResult::Started(id, Ok(())) => {
                self.status_message = Some(format!("Started {}", id));
                self.send_refresh();
            }
            ActionResult::Started(_, Err(e)) => {
                self.status_message = Some(format!("Error: {}", e));
            }
            ActionResult::Stopped(id, Ok(())) => {
                self.status_message = Some(format!("Stopped {}", id));
                self.send_refresh();
            }
            ActionResult::Stopped(_, Err(e)) => {
                self.status_message = Some(format!("Error: {}", e));
            }
            ActionResult::Killed(id, Ok(())) => {
                self.status_message = Some(format!("Killed {}", id));
                self.send_refresh();
            }
            ActionResult::Killed(_, Err(e)) => {
                self.status_message = Some(format!("Error: {}", e));
            }
            ActionResult::Deleted(id, Ok(())) => {
                self.status_message = Some(format!("Deleted {}", id));
                self.send_refresh();
            }
            ActionResult::Deleted(_, Err(e)) => {
                self.status_message = Some(format!("Error: {}", e));
            }
            ActionResult::Created(Ok(id)) => {
                self.status_message = Some(format!("Created {}", id));
                self.send_refresh();
            }
            ActionResult::Created(Err(e)) => {
                self.status_message = Some(format!("Error: {}", e));
            }
            ActionResult::ConsoleOpened {
                vm_id,
                vm_name,
                input_tx,
            } => {
                // Create vt100 parser with reasonable terminal size
                let parser = vt100::Parser::new(24, 80, 10000); // rows, cols, scrollback
                self.console_session = Some(ConsoleSession {
                    vm_id,
                    vm_name,
                    parser,
                    escape_state: EscapeState::Normal,
                    input_tx,
                });
                self.status_message = None;
            }
            ActionResult::ConsoleOutput(data) => {
                if let Some(ref mut session) = self.console_session {
                    session.parser.process(&data);
                    // Only auto-scroll if already at bottom (not scrolled up)
                    // This allows user to scroll up without being snapped back
                }
            }
            ActionResult::ConsoleClosed(error) => {
                self.console_session = None;
                if let Some(e) = error {
                    self.status_message = Some(format!("Console error: {}", e));
                } else {
                    self.status_message = Some("Console disconnected".to_string());
                }
            }
        }
    }

    fn selected_vm(&self) -> Option<&Vm> {
        self.table_state.selected().and_then(|i| self.vms.get(i))
    }

    fn start_selected(&mut self) {
        let Some(id) = self.selected_vm().map(|vm| vm.id.clone()) else {
            return;
        };
        self.status_message = Some(format!("Starting {}...", id));
        self.send_action(Action::Start(id));
    }

    fn stop_selected(&mut self) {
        let Some(id) = self.selected_vm().map(|vm| vm.id.clone()) else {
            return;
        };
        self.status_message = Some(format!("Stopping {}...", id));
        self.send_action(Action::Stop(id));
    }

    fn kill_selected(&mut self) {
        let Some(id) = self.selected_vm().map(|vm| vm.id.clone()) else {
            return;
        };
        self.confirm_kill = Some(id);
    }

    fn confirm_kill(&mut self) {
        if let Some(id) = self.confirm_kill.take() {
            self.status_message = Some(format!("Killing {}...", id));
            self.send_action(Action::Kill(id));
        }
    }

    fn cancel_kill(&mut self) {
        self.confirm_kill = None;
    }

    fn delete_selected(&mut self) {
        let Some(id) = self.selected_vm().map(|vm| vm.id.clone()) else {
            return;
        };
        self.confirm_delete = Some(id);
    }

    fn confirm_delete(&mut self) {
        if let Some(id) = self.confirm_delete.take() {
            self.status_message = Some(format!("Deleting {}...", id));
            self.send_action(Action::Delete(id));
        }
    }

    fn cancel_delete(&mut self) {
        self.confirm_delete = None;
    }

    fn open_console(&mut self) {
        let Some(vm) = self.selected_vm() else {
            return;
        };
        // Only allow console on running VMs
        if vm.state != VmState::Running as i32 {
            self.status_message = Some("Console only available for running VMs".to_string());
            return;
        }
        let vm_id = vm.id.clone();
        let vm_name = vm.name.clone();
        self.status_message = Some("Connecting to console...".to_string());
        self.send_action(Action::OpenConsole { vm_id, vm_name });
    }

    fn handle_console_key(&mut self, key_code: KeyCode, modifiers: KeyModifiers) {
        let Some(ref mut session) = self.console_session else {
            return;
        };

        // Handle escape sequence: Ctrl+A then 't' to exit
        match session.escape_state {
            EscapeState::Normal => {
                if modifiers.contains(KeyModifiers::CONTROL)
                    && let KeyCode::Char('a') | KeyCode::Char('A') = key_code
                {
                    session.escape_state = EscapeState::SawCtrlA;
                    return;
                }
            }
            EscapeState::SawCtrlA => {
                session.escape_state = EscapeState::Normal;
                if let KeyCode::Char('t') | KeyCode::Char('T') = key_code {
                    // Exit console
                    self.console_session = None;
                    self.status_message = Some("Disconnected from console".to_string());
                    return;
                }
                // Not 't', send Ctrl+A then continue with current key
                let _ = session.input_tx.send(vec![0x01]);
            }
        }

        // Map keys to bytes/escape sequences
        let data: Option<Vec<u8>> = match key_code {
            KeyCode::Char(c) => {
                if modifiers.contains(KeyModifiers::CONTROL) {
                    // Ctrl+char: send as control code
                    let ctrl_char = (c.to_ascii_lowercase() as u8)
                        .wrapping_sub(b'a')
                        .wrapping_add(1);
                    Some(vec![ctrl_char])
                } else {
                    // Regular character - encode as UTF-8
                    let mut buf = [0u8; 4];
                    let s = c.encode_utf8(&mut buf);
                    Some(s.as_bytes().to_vec())
                }
            }
            KeyCode::Enter => Some(vec![b'\r']),
            KeyCode::Backspace => Some(vec![0x7f]),
            KeyCode::Tab => Some(vec![b'\t']),
            KeyCode::Esc => Some(vec![0x1b]),
            KeyCode::Up => Some(b"\x1b[A".to_vec()),
            KeyCode::Down => Some(b"\x1b[B".to_vec()),
            KeyCode::Right => Some(b"\x1b[C".to_vec()),
            KeyCode::Left => Some(b"\x1b[D".to_vec()),
            KeyCode::Home => Some(b"\x1b[H".to_vec()),
            KeyCode::End => Some(b"\x1b[F".to_vec()),
            KeyCode::PageUp => Some(b"\x1b[5~".to_vec()),
            KeyCode::PageDown => Some(b"\x1b[6~".to_vec()),
            KeyCode::Delete => Some(b"\x1b[3~".to_vec()),
            KeyCode::Insert => Some(b"\x1b[2~".to_vec()),
            KeyCode::F(n) => {
                // F1-F12 escape sequences
                let seq = match n {
                    1 => b"\x1bOP".to_vec(),
                    2 => b"\x1bOQ".to_vec(),
                    3 => b"\x1bOR".to_vec(),
                    4 => b"\x1bOS".to_vec(),
                    5 => b"\x1b[15~".to_vec(),
                    6 => b"\x1b[17~".to_vec(),
                    7 => b"\x1b[18~".to_vec(),
                    8 => b"\x1b[19~".to_vec(),
                    9 => b"\x1b[20~".to_vec(),
                    10 => b"\x1b[21~".to_vec(),
                    11 => b"\x1b[23~".to_vec(),
                    12 => b"\x1b[24~".to_vec(),
                    _ => return,
                };
                Some(seq)
            }
            _ => None,
        };

        if let Some(bytes) = data {
            let _ = session.input_tx.send(bytes);
        }
    }

    fn open_create_modal(&mut self) {
        self.create_modal = Some(CreateModal::new());
    }

    fn close_create_modal(&mut self) {
        self.create_modal = None;
    }

    fn submit_create(&mut self) {
        if let Some(modal) = &self.create_modal {
            match modal.validate() {
                Ok(params) => {
                    self.status_message = Some("Creating VM...".to_string());
                    self.send_action(Action::Create(Box::new(params)));
                    self.create_modal = None;
                }
                Err(e) => {
                    self.status_message = Some(format!("Error: {}", e));
                }
            }
        }
    }

    fn open_file_picker(&mut self) {
        if let Some(modal) = &self.create_modal
            && modal.is_file_field()
        {
            let start_path = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
            self.file_picker = Some(FilePicker::new(start_path, modal.focused_field));
        }
    }

    fn close_file_picker(&mut self) {
        self.file_picker = None;
    }

    fn select_file(&mut self) {
        if let Some(picker) = &mut self.file_picker
            && let Some(path) = picker.enter_selected()
        {
            let path_str = path.to_string_lossy().to_string();
            if self.file_picker_for_user_data {
                if let Some(modal) = &mut self.create_modal {
                    modal.set_user_data_file(path_str);
                }
            } else {
                let target_field = picker.target_field;
                if let Some(modal) = &mut self.create_modal {
                    modal.set_field(target_field, path_str);
                }
            }
            self.file_picker = None;
            self.file_picker_for_user_data = false;
        }
    }

    fn open_user_data_file_picker(&mut self) {
        let start_path = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        self.file_picker = Some(FilePicker::new(start_path, 0)); // field doesn't matter for user-data
        self.file_picker_for_user_data = true;
    }

    fn open_ssh_keys_modal(&mut self) {
        self.ssh_keys_modal = Some(SshKeysModal::new());
    }

    fn close_ssh_keys_modal(&mut self) {
        self.ssh_keys_modal = None;
    }

    fn confirm_ssh_keys(&mut self) {
        if let Some(modal) = &self.ssh_keys_modal {
            match modal.validate() {
                Ok(()) => {
                    let config = modal.config.clone();
                    if let Some(create_modal) = &mut self.create_modal {
                        create_modal.set_ssh_keys_config(config);
                    }
                    self.ssh_keys_modal = None;
                }
                Err(e) => {
                    self.status_message = Some(format!("Error: {}", e));
                }
            }
        }
    }

    fn send_refresh(&self) {
        let _ = self.action_tx.send(Action::Refresh);
        let _ = self.action_tx.send(Action::RefreshSystemInfo);
    }

    fn refresh(&mut self) {
        self.status_message = Some("Refreshing...".to_string());
        self.send_refresh();
    }

    fn next(&mut self) {
        if self.vms.is_empty() {
            return;
        }
        let i = match self.table_state.selected() {
            Some(i) => {
                if i >= self.vms.len() - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.table_state.select(Some(i));
    }

    fn previous(&mut self) {
        if self.vms.is_empty() {
            return;
        }
        let i = match self.table_state.selected() {
            Some(i) => {
                if i == 0 {
                    self.vms.len() - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.table_state.select(Some(i));
    }

    fn open_detail_view(&mut self) {
        if let Some(vm) = self.selected_vm() {
            self.detail_view = Some(vm.id.clone());
        }
    }

    fn close_detail_view(&mut self) {
        self.detail_view = None;
    }

    fn get_vm_by_id(&self, id: &str) -> Option<&Vm> {
        self.vms.iter().find(|vm| vm.id == id)
    }
}

fn format_state(state: i32) -> &'static str {
    match VmState::try_from(state).unwrap_or(VmState::Unspecified) {
        VmState::Unspecified => "○ unknown",
        VmState::Stopped => "○ stopped",
        VmState::Starting => "◐ starting",
        VmState::Running => "● running",
        VmState::Stopping => "◑ stopping",
    }
}

fn state_style(state: i32) -> Style {
    match VmState::try_from(state).unwrap_or(VmState::Unspecified) {
        VmState::Running => Style::default().fg(Color::Green).bold(),
        VmState::Stopped => Style::default().fg(Color::DarkGray),
        VmState::Starting | VmState::Stopping => Style::default().fg(Color::Yellow),
        VmState::Unspecified => Style::default().fg(Color::DarkGray),
    }
}

fn draw(frame: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Title bar
            Constraint::Min(5),    // Table
            Constraint::Length(1), // Legend
            Constraint::Length(1), // Status
        ])
        .split(frame.area());

    // Title bar with system resource info
    let title = if let Some(ref info) = app.system_info {
        let cpu_color = if info.allocated_cpus > info.total_cpus * 8 / 10 {
            Color::Red
        } else if info.allocated_cpus > info.total_cpus / 2 {
            Color::Yellow
        } else {
            Color::Green
        };
        let mem_color = if info.allocated_memory_mb > info.total_memory_mb * 8 / 10 {
            Color::Red
        } else if info.allocated_memory_mb > info.total_memory_mb / 2 {
            Color::Yellow
        } else {
            Color::Green
        };
        let total_mem_gib = info.total_memory_mb as f64 / 1024.0;
        let alloc_mem_gib = info.allocated_memory_mb as f64 / 1024.0;

        Line::from(vec![
            Span::styled(" mvirt ", Style::default().fg(Color::Cyan).bold()),
            Span::styled("│", Style::default().fg(Color::DarkGray)),
            Span::styled(" CPU ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{}", info.allocated_cpus),
                Style::default().fg(cpu_color).bold(),
            ),
            Span::styled(
                format!("/{}", info.total_cpus),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(" │", Style::default().fg(Color::DarkGray)),
            Span::styled(" RAM ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:.1}", alloc_mem_gib),
                Style::default().fg(mem_color).bold(),
            ),
            Span::styled(
                format!("/{:.1} GiB", total_mem_gib),
                Style::default().fg(Color::DarkGray),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled(" mvirt ", Style::default().fg(Color::Cyan).bold()),
            Span::styled("│", Style::default().fg(Color::DarkGray)),
            Span::styled(" loading...", Style::default().fg(Color::DarkGray)),
        ])
    };
    let load_text = if let Some(ref info) = app.system_info {
        Line::from(vec![Span::styled(
            format!(
                "Load {:.2} {:.2} {:.2} ",
                info.load_1, info.load_5, info.load_15
            ),
            Style::default().fg(Color::DarkGray),
        )])
    } else {
        Line::from("")
    };
    let title_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    frame.render_widget(title_block.clone(), chunks[0]);
    let title_inner = title_block.inner(chunks[0]);
    let title_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(22)])
        .split(title_inner);
    frame.render_widget(Paragraph::new(title), title_chunks[0]);
    frame.render_widget(
        Paragraph::new(load_text).alignment(ratatui::prelude::Alignment::Right),
        title_chunks[1],
    );

    // VM Table
    let header = Row::new(vec![
        Cell::from("ID").style(Style::default().fg(Color::Cyan)),
        Cell::from("NAME").style(Style::default().fg(Color::Cyan)),
        Cell::from("STATE").style(Style::default().fg(Color::Cyan)),
        Cell::from("CPU").style(Style::default().fg(Color::Cyan)),
        Cell::from("MEM").style(Style::default().fg(Color::Cyan)),
    ])
    .style(Style::default().bold())
    .bottom_margin(1);

    let selected_idx = app.table_state.selected();
    let rows: Vec<Row> = app
        .vms
        .iter()
        .enumerate()
        .map(|(idx, vm)| {
            let config = vm.config.as_ref();
            let state = vm.state;
            let is_selected = selected_idx == Some(idx);
            let bg = if is_selected {
                Color::Indexed(236) // Very dark gray - contrasts with DarkGray text
            } else {
                Color::Reset
            };

            Row::new(vec![
                Cell::from(Span::styled(
                    format!("{}…", &vm.id[..8]), // Show short ID with ellipsis
                    Style::default().fg(Color::DarkGray).bg(bg),
                )),
                Cell::from(Span::styled(
                    vm.name.clone().unwrap_or_else(|| "-".to_string()),
                    Style::default()
                        .fg(if is_selected {
                            Color::White
                        } else {
                            Color::Reset
                        })
                        .bg(bg),
                )),
                Cell::from(Span::styled(format_state(state), state_style(state).bg(bg))),
                Cell::from(Span::styled(
                    config.map(|c| c.vcpus.to_string()).unwrap_or_default(),
                    Style::default()
                        .fg(if is_selected {
                            Color::White
                        } else {
                            Color::Reset
                        })
                        .bg(bg),
                )),
                Cell::from(Span::styled(
                    config
                        .map(|c| format!("{} MB", c.memory_mb))
                        .unwrap_or_default(),
                    Style::default()
                        .fg(if is_selected {
                            Color::White
                        } else {
                            Color::Reset
                        })
                        .bg(bg),
                )),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(11), // 8 chars + ellipsis + padding
            Constraint::Min(15),
            Constraint::Length(12),
            Constraint::Length(5),
            Constraint::Length(10),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    )
    .row_highlight_style(Style::default().bg(Color::Indexed(236)));

    frame.render_stateful_widget(table, chunks[1], &mut app.table_state);

    // Hotkey legend with refresh time
    let legend_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(12)])
        .split(chunks[2]);

    let legend = Line::from(vec![
        Span::styled(" ↵", Style::default().fg(Color::White).bold()),
        Span::styled(" Details ", Style::default().fg(Color::DarkGray)),
        Span::styled("n", Style::default().fg(Color::Cyan).bold()),
        Span::styled(" New ", Style::default().fg(Color::DarkGray)),
        Span::styled("c", Style::default().fg(Color::Cyan).bold()),
        Span::styled(" Console ", Style::default().fg(Color::DarkGray)),
        Span::styled("s", Style::default().fg(Color::Green).bold()),
        Span::styled(" Start ", Style::default().fg(Color::DarkGray)),
        Span::styled("S", Style::default().fg(Color::Yellow).bold()),
        Span::styled(" Stop ", Style::default().fg(Color::DarkGray)),
        Span::styled("k", Style::default().fg(Color::Red).bold()),
        Span::styled(" Kill ", Style::default().fg(Color::DarkGray)),
        Span::styled("d", Style::default().fg(Color::Red).bold()),
        Span::styled(" Delete ", Style::default().fg(Color::DarkGray)),
        Span::styled("q", Style::default().fg(Color::Magenta).bold()),
        Span::styled(" Quit", Style::default().fg(Color::DarkGray)),
    ]);
    frame.render_widget(Paragraph::new(legend), legend_chunks[0]);

    let refresh_time = app
        .last_refresh
        .map(|t| t.format("%H:%M:%S").to_string())
        .unwrap_or_else(|| "--:--:--".to_string());
    let refresh_text = Line::from(vec![Span::styled(
        format!("{} ", refresh_time),
        Style::default().fg(Color::DarkGray),
    )]);
    frame.render_widget(
        Paragraph::new(refresh_text).alignment(ratatui::prelude::Alignment::Right),
        legend_chunks[1],
    );

    // Status bar / Confirmation
    if let Some(ref id) = app.confirm_kill {
        let vm_display = if let Some(vm) = app.get_vm_by_id(id) {
            format!("{} ({}…)", vm.name.as_deref().unwrap_or(&id[..8]), &id[..8])
        } else {
            format!("{}…", &id[..8])
        };
        let confirm_line = Line::from(vec![
            Span::styled(" ⚠ ", Style::default().fg(Color::Red)),
            Span::styled(
                format!("Kill VM {}? ", vm_display),
                Style::default().fg(Color::Red).bold(),
            ),
            Span::styled("[y]", Style::default().fg(Color::Green).bold()),
            Span::styled("es / ", Style::default().fg(Color::DarkGray)),
            Span::styled("[n]", Style::default().fg(Color::Red).bold()),
            Span::styled("o", Style::default().fg(Color::DarkGray)),
        ]);
        frame.render_widget(Paragraph::new(confirm_line), chunks[3]);
    } else if let Some(ref id) = app.confirm_delete {
        let vm_display = if let Some(vm) = app.get_vm_by_id(id) {
            format!("{} ({}…)", vm.name.as_deref().unwrap_or(&id[..8]), &id[..8])
        } else {
            format!("{}…", &id[..8])
        };
        let confirm_line = Line::from(vec![
            Span::styled(" ⚠ ", Style::default().fg(Color::Red)),
            Span::styled(
                format!("Delete VM {}? ", vm_display),
                Style::default().fg(Color::Red).bold(),
            ),
            Span::styled("[y]", Style::default().fg(Color::Green).bold()),
            Span::styled("es / ", Style::default().fg(Color::DarkGray)),
            Span::styled("[n]", Style::default().fg(Color::Red).bold()),
            Span::styled("o", Style::default().fg(Color::DarkGray)),
        ]);
        frame.render_widget(Paragraph::new(confirm_line), chunks[3]);
    } else if let Some(status) = &app.status_message {
        let status_line = Line::from(vec![Span::styled(
            format!(" {}", status),
            Style::default().fg(Color::Yellow),
        )]);
        frame.render_widget(Paragraph::new(status_line), chunks[3]);
    }

    // Detail View
    if let Some(ref vm_id) = app.detail_view
        && let Some(vm) = app.get_vm_by_id(vm_id)
    {
        draw_detail_view(frame, vm);
    }

    // Create VM Modal
    if let Some(modal) = &app.create_modal {
        draw_create_modal(frame, modal);
    }

    // File Picker (on top of modal)
    if let Some(picker) = &app.file_picker {
        draw_file_picker(frame, picker);
    }

    // SSH Keys Modal (on top of create modal)
    if let Some(modal) = &app.ssh_keys_modal {
        draw_ssh_keys_modal(frame, modal);
    }

    // Console (takes over the whole screen)
    if let Some(ref mut session) = app.console_session {
        draw_console(frame, session);
    }
}

fn draw_create_modal(frame: &mut Frame, modal: &CreateModal) {
    let area = frame.area();
    let modal_width = 70.min(area.width.saturating_sub(4));

    // Dynamic height based on boot mode (+1 for top padding)
    let field_count = if modal.boot_mode == CreateBootMode::Kernel {
        11
    } else {
        8
    };
    let modal_height = ((field_count * 2) + 3).min(area.height.saturating_sub(4) as usize) as u16;

    let modal_area = Rect {
        x: (area.width - modal_width) / 2,
        y: (area.height - modal_height) / 2,
        width: modal_width,
        height: modal_height,
    };

    // Clear the modal area
    frame.render_widget(Clear, modal_area);

    // Modal block
    let title = Line::from(vec![
        Span::styled(" Create VM ", Style::default().fg(Color::Cyan).bold()),
        Span::styled("│", Style::default().fg(Color::DarkGray)),
        Span::styled(" Tab", Style::default().fg(Color::Yellow)),
        Span::styled(": next ", Style::default().fg(Color::DarkGray)),
        Span::styled("Esc", Style::default().fg(Color::Red)),
        Span::styled(": cancel ", Style::default().fg(Color::DarkGray)),
    ]);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(title);
    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    let label_focused = Style::default().fg(Color::Cyan).bold();
    let label_normal = Style::default().fg(Color::DarkGray);
    let value_focused = Style::default().fg(Color::White);
    let value_normal = Style::default().fg(Color::Gray);

    // Build constraints based on boot mode
    let constraints: Vec<Constraint> = if modal.boot_mode == CreateBootMode::Kernel {
        vec![
            Constraint::Length(1), // Top padding
            Constraint::Length(2), // Name
            Constraint::Length(2), // Boot Mode
            Constraint::Length(2), // Kernel
            Constraint::Length(2), // Initramfs
            Constraint::Length(2), // Cmdline
            Constraint::Length(2), // Disk
            Constraint::Length(2), // VCPUs
            Constraint::Length(2), // Memory
            Constraint::Length(2), // Nested Virt
            Constraint::Length(2), // User-Data
            Constraint::Length(2), // Submit
        ]
    } else {
        vec![
            Constraint::Length(1), // Top padding
            Constraint::Length(2), // Name
            Constraint::Length(2), // Boot Mode
            Constraint::Length(2), // Disk
            Constraint::Length(2), // VCPUs
            Constraint::Length(2), // Memory
            Constraint::Length(2), // Nested Virt
            Constraint::Length(2), // User-Data
            Constraint::Length(2), // Submit
        ]
    };

    let field_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    // Helper closure to render a field
    let render_field =
        |frame: &mut Frame, area: Rect, label: &str, value: &str, focused: bool, hint: &str| {
            let cursor = if focused { "▌" } else { "" };
            let hint_span = if focused && !hint.is_empty() {
                Span::styled(format!(" [{}]", hint), Style::default().fg(Color::Yellow))
            } else {
                Span::raw("")
            };
            let line = Line::from(vec![
                Span::styled(
                    format!(" {:<12}", label),
                    if focused { label_focused } else { label_normal },
                ),
                Span::styled(
                    format!("{}{}", value, cursor),
                    if focused { value_focused } else { value_normal },
                ),
                hint_span,
            ]);
            frame.render_widget(Paragraph::new(line), area);
        };

    let mut row = 1; // Start at 1 to skip top padding

    // Name
    render_field(
        frame,
        field_chunks[row],
        "Name:",
        &modal.name,
        modal.focused_field == 0,
        "",
    );
    row += 1;

    // Boot Mode (toggle)
    let boot_str = match modal.boot_mode {
        CreateBootMode::Disk => "(●) Disk  ( ) Kernel",
        CreateBootMode::Kernel => "( ) Disk  (●) Kernel",
    };
    let boot_focused = modal.focused_field == 1;
    let boot_line = Line::from(vec![
        Span::styled(
            " Boot:       ",
            if boot_focused {
                label_focused
            } else {
                label_normal
            },
        ),
        Span::styled(
            boot_str,
            if boot_focused {
                value_focused
            } else {
                value_normal
            },
        ),
        if boot_focused {
            Span::styled(" [Space: toggle]", Style::default().fg(Color::Yellow))
        } else {
            Span::raw("")
        },
    ]);
    frame.render_widget(Paragraph::new(boot_line), field_chunks[row]);
    row += 1;

    // Kernel mode specific fields
    if modal.boot_mode == CreateBootMode::Kernel {
        render_field(
            frame,
            field_chunks[row],
            "Kernel:",
            &modal.kernel,
            modal.focused_field == 2,
            "Enter: browse",
        );
        row += 1;
        render_field(
            frame,
            field_chunks[row],
            "Initramfs:",
            &modal.initramfs,
            modal.focused_field == 3,
            "Enter: browse",
        );
        row += 1;
        render_field(
            frame,
            field_chunks[row],
            "Cmdline:",
            &modal.cmdline,
            modal.focused_field == 4,
            "",
        );
        row += 1;
    }

    // Common fields
    render_field(
        frame,
        field_chunks[row],
        "Disk:",
        &modal.disk,
        modal.focused_field == 5,
        "Enter: browse",
    );
    row += 1;
    render_field(
        frame,
        field_chunks[row],
        "VCPUs:",
        &modal.vcpus,
        modal.focused_field == 6,
        "",
    );
    row += 1;
    render_field(
        frame,
        field_chunks[row],
        "Memory:",
        &modal.memory_mb,
        modal.focused_field == 7,
        "MB",
    );
    row += 1;

    // Nested Virtualization toggle
    let nested_focused = modal.focused_field == 8;
    let nested_str = if modal.nested_virt {
        "[x] Enabled"
    } else {
        "[ ] Disabled"
    };
    let nested_line = Line::from(vec![
        Span::styled(
            " Nested Virt:",
            if nested_focused {
                label_focused
            } else {
                label_normal
            },
        ),
        Span::styled(
            nested_str,
            if nested_focused {
                value_focused
            } else {
                value_normal
            },
        ),
        if nested_focused {
            Span::styled(" [Space: toggle]", Style::default().fg(Color::Yellow))
        } else {
            Span::raw("")
        },
    ]);
    frame.render_widget(Paragraph::new(nested_line), field_chunks[row]);
    row += 1;

    // User-Data mode (toggle with Space, Enter to configure)
    let user_data_focused = modal.focused_field == 9;
    let (user_data_mode_str, user_data_value, user_data_hint) = match modal.user_data_mode {
        UserDataMode::None => ("None", "".to_string(), "[Space: cycle]"),
        UserDataMode::SshKeys => {
            let value = if let Some(ref cfg) = modal.ssh_keys_config {
                format!(
                    "{} ({})",
                    cfg.username,
                    match cfg.source {
                        SshKeySource::GitHub => format!("github:{}", cfg.github_user),
                        SshKeySource::Local => "local".to_string(),
                    }
                )
            } else {
                "not configured".to_string()
            };
            ("SSH Keys", value, "[Space: cycle, Enter: configure]")
        }
        UserDataMode::File => {
            let value = if modal.user_data_file.is_empty() {
                "not selected".to_string()
            } else {
                modal.user_data_file.clone()
            };
            ("File", value, "[Space: cycle, Enter: browse]")
        }
    };
    let user_data_line = Line::from(vec![
        Span::styled(
            " User-Data:  ",
            if user_data_focused {
                label_focused
            } else {
                label_normal
            },
        ),
        Span::styled(
            format!("[{}] ", user_data_mode_str),
            if user_data_focused {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::DarkGray)
            },
        ),
        Span::styled(
            user_data_value,
            if user_data_focused {
                value_focused
            } else {
                value_normal
            },
        ),
        if user_data_focused {
            Span::styled(
                format!(" {}", user_data_hint),
                Style::default().fg(Color::Yellow),
            )
        } else {
            Span::raw("")
        },
    ]);
    frame.render_widget(Paragraph::new(user_data_line), field_chunks[row]);
    row += 1;

    // Submit button
    let submit_style = if modal.focused_field == 10 {
        Style::default().fg(Color::Black).bg(Color::Green).bold()
    } else {
        Style::default().fg(Color::Green)
    };
    let submit = Paragraph::new(Line::from(vec![Span::styled(
        "  ▶ Create VM  ",
        submit_style,
    )]))
    .alignment(ratatui::prelude::Alignment::Center);
    frame.render_widget(submit, field_chunks[row]);
}

fn draw_file_picker(frame: &mut Frame, picker: &FilePicker) {
    let area = frame.area();
    let modal_width = 60.min(area.width.saturating_sub(6));
    let modal_height = 20.min(area.height.saturating_sub(6));

    let modal_area = Rect {
        x: (area.width - modal_width) / 2,
        y: (area.height - modal_height) / 2,
        width: modal_width,
        height: modal_height,
    };

    // Clear the modal area
    frame.render_widget(Clear, modal_area);

    // Modal block with current path in title
    let title = format!(
        " {} (Enter: select, Esc: cancel) ",
        picker.current_path.display()
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .style(Style::default().bg(Color::Black));
    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    // Calculate visible entries based on scroll
    let visible_height = inner.height as usize;

    // Adjust scroll to keep selected item visible
    let scroll_offset = if picker.selected >= visible_height {
        picker.selected - visible_height + 1
    } else {
        0
    };

    // Render entries
    for (i, entry) in picker
        .entries
        .iter()
        .skip(scroll_offset)
        .take(visible_height)
        .enumerate()
    {
        let actual_index = i + scroll_offset;
        let is_selected = actual_index == picker.selected;

        let (name, style) = if entry == &PathBuf::from("..") {
            (
                "..".to_string(),
                if is_selected {
                    Style::default().fg(Color::Cyan).bold().reversed()
                } else {
                    Style::default().fg(Color::Cyan)
                },
            )
        } else if entry.is_dir() {
            let name = entry
                .file_name()
                .map(|n| format!("{}/", n.to_string_lossy()))
                .unwrap_or_else(|| "???/".to_string());
            (
                name,
                if is_selected {
                    Style::default().fg(Color::Blue).bold().reversed()
                } else {
                    Style::default().fg(Color::Blue)
                },
            )
        } else {
            let name = entry
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "???".to_string());
            (
                name,
                if is_selected {
                    Style::default().reversed()
                } else {
                    Style::default()
                },
            )
        };

        let line_area = Rect {
            x: inner.x,
            y: inner.y + i as u16,
            width: inner.width,
            height: 1,
        };
        frame.render_widget(Paragraph::new(Span::styled(name, style)), line_area);
    }
}

fn draw_ssh_keys_modal(frame: &mut Frame, modal: &SshKeysModal) {
    let area = frame.area();
    let modal_width = 60.min(area.width.saturating_sub(6));
    let modal_height = 15.min(area.height.saturating_sub(6)); // +1 for top padding

    let modal_area = Rect {
        x: (area.width - modal_width) / 2,
        y: (area.height - modal_height) / 2,
        width: modal_width,
        height: modal_height,
    };

    // Clear the modal area
    frame.render_widget(Clear, modal_area);

    // Modal block
    let title = Line::from(vec![
        Span::styled(" SSH Keys ", Style::default().fg(Color::Cyan).bold()),
        Span::styled("│", Style::default().fg(Color::DarkGray)),
        Span::styled(" Tab", Style::default().fg(Color::Yellow)),
        Span::styled(": next ", Style::default().fg(Color::DarkGray)),
        Span::styled("Esc", Style::default().fg(Color::Red)),
        Span::styled(": cancel ", Style::default().fg(Color::DarkGray)),
    ]);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(title);
    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    let label_focused = Style::default().fg(Color::Cyan).bold();
    let label_normal = Style::default().fg(Color::DarkGray);
    let value_focused = Style::default().fg(Color::White);
    let value_normal = Style::default().fg(Color::Gray);

    let field_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Top padding
            Constraint::Length(2), // Username
            Constraint::Length(2), // Source
            Constraint::Length(2), // GitHub user / Local path
            Constraint::Length(2), // Root password
            Constraint::Length(1), // Spacer
            Constraint::Length(2), // Buttons
        ])
        .split(inner);

    // Username field (index 1 to skip top padding)
    let username_focused = modal.focused_field == 0;
    let cursor = if username_focused { "▌" } else { "" };
    let username_line = Line::from(vec![
        Span::styled(
            " Username:   ",
            if username_focused {
                label_focused
            } else {
                label_normal
            },
        ),
        Span::styled(
            format!("{}{}", modal.config.username, cursor),
            if username_focused {
                value_focused
            } else {
                value_normal
            },
        ),
    ]);
    frame.render_widget(Paragraph::new(username_line), field_chunks[1]);

    // Source toggle
    let source_focused = modal.focused_field == 1;
    let source_str = match modal.config.source {
        SshKeySource::GitHub => "(●) GitHub  ( ) Local",
        SshKeySource::Local => "( ) GitHub  (●) Local",
    };
    let source_line = Line::from(vec![
        Span::styled(
            " Source:     ",
            if source_focused {
                label_focused
            } else {
                label_normal
            },
        ),
        Span::styled(
            source_str,
            if source_focused {
                value_focused
            } else {
                value_normal
            },
        ),
        if source_focused {
            Span::styled(" [Space: toggle]", Style::default().fg(Color::Yellow))
        } else {
            Span::raw("")
        },
    ]);
    frame.render_widget(Paragraph::new(source_line), field_chunks[2]);

    // GitHub username or Local path
    let value_focused_field = modal.focused_field == 2;
    let cursor = if value_focused_field { "▌" } else { "" };
    let (label, value) = match modal.config.source {
        SshKeySource::GitHub => ("GitHub User:", &modal.config.github_user),
        SshKeySource::Local => ("Key File:", &modal.config.local_path),
    };
    let value_line = Line::from(vec![
        Span::styled(
            format!(" {:<11}", label),
            if value_focused_field {
                label_focused
            } else {
                label_normal
            },
        ),
        Span::styled(
            format!("{}{}", value, cursor),
            if value_focused_field {
                value_focused
            } else {
                value_normal
            },
        ),
    ]);
    frame.render_widget(Paragraph::new(value_line), field_chunks[3]);

    // Root password field
    let password_focused = modal.focused_field == 3;
    let cursor = if password_focused { "▌" } else { "" };
    let password_display = if modal.config.root_password.is_empty() {
        "(none)".to_string()
    } else {
        "*".repeat(modal.config.root_password.len())
    };
    let password_line = Line::from(vec![
        Span::styled(
            " Root Pass:  ",
            if password_focused {
                label_focused
            } else {
                label_normal
            },
        ),
        Span::styled(
            format!("{}{}", password_display, cursor),
            if password_focused {
                value_focused
            } else {
                value_normal
            },
        ),
        if !password_focused && modal.config.root_password.is_empty() {
            Span::styled(" (optional)", Style::default().fg(Color::DarkGray))
        } else {
            Span::raw("")
        },
    ]);
    frame.render_widget(Paragraph::new(password_line), field_chunks[4]);

    // Buttons
    let button_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(field_chunks[6]);

    let add_style = if modal.focused_field == 4 {
        Style::default().fg(Color::Black).bg(Color::Green).bold()
    } else {
        Style::default().fg(Color::Green)
    };
    let cancel_style = if modal.focused_field == 5 {
        Style::default().fg(Color::Black).bg(Color::Red).bold()
    } else {
        Style::default().fg(Color::Red)
    };

    frame.render_widget(
        Paragraph::new(Span::styled("  ▶ Add  ", add_style))
            .alignment(ratatui::prelude::Alignment::Center),
        button_chunks[0],
    );
    frame.render_widget(
        Paragraph::new(Span::styled("  ✕ Cancel  ", cancel_style))
            .alignment(ratatui::prelude::Alignment::Center),
        button_chunks[1],
    );
}

/// Convert vt100 color to ratatui color
fn vt100_color_to_ratatui(color: vt100::Color) -> Color {
    match color {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

fn draw_console(frame: &mut Frame, session: &mut ConsoleSession) {
    let area = frame.area();

    // Clear the screen with a background
    frame.render_widget(Clear, area);

    // Title bar
    let title = Line::from(vec![
        Span::styled(" Console: ", Style::default().fg(Color::Cyan).bold()),
        Span::styled(
            session
                .vm_name
                .as_deref()
                .unwrap_or(&session.vm_id[..8.min(session.vm_id.len())]),
            Style::default().fg(Color::White),
        ),
        Span::styled(" │ ", Style::default().fg(Color::DarkGray)),
        Span::styled("Ctrl+A t", Style::default().fg(Color::Yellow)),
        Span::styled(": exit", Style::default().fg(Color::DarkGray)),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(title);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let view_height = inner.height as usize;
    let view_width = inner.width as usize;

    // Resize vt100 parser if terminal size changed
    let (current_rows, current_cols) = session.parser.screen().size();
    if current_rows as usize != view_height || current_cols as usize != view_width {
        session
            .parser
            .set_size(view_height as u16, view_width as u16);
    }

    let screen = session.parser.screen();
    let (cursor_row, cursor_col) = screen.cursor_position();

    // Build lines from vt100 screen
    let mut lines: Vec<Line> = Vec::with_capacity(view_height);

    for row in 0..view_height {
        let mut spans: Vec<Span> = Vec::new();
        let mut current_text = String::new();
        let mut current_style = Style::default();

        for col in 0..view_width {
            let cell = screen.cell(row as u16, col as u16);
            let (ch, cell_fg, cell_bg, bold) = if let Some(cell) = cell {
                (
                    cell.contents(),
                    vt100_color_to_ratatui(cell.fgcolor()),
                    vt100_color_to_ratatui(cell.bgcolor()),
                    cell.bold(),
                )
            } else {
                (" ".to_string(), Color::Reset, Color::Reset, false)
            };

            // Check if this is the cursor position
            let is_cursor = row == cursor_row as usize && col == cursor_col as usize;

            let cell_style = if is_cursor {
                Style::default().fg(Color::Black).bg(Color::White)
            } else {
                let mut s = Style::default().fg(cell_fg).bg(cell_bg);
                if bold {
                    s = s.bold();
                }
                s
            };

            // If style changed, flush current span
            if cell_style != current_style && !current_text.is_empty() {
                spans.push(Span::styled(
                    std::mem::take(&mut current_text),
                    current_style,
                ));
            }
            current_style = cell_style;

            if ch.is_empty() {
                current_text.push(' ');
            } else {
                current_text.push_str(&ch);
            }
        }

        // Flush remaining text
        if !current_text.is_empty() {
            spans.push(Span::styled(current_text, current_style));
        }

        lines.push(Line::from(spans));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_splash(frame: &mut Frame) {
    let area = frame.area();

    // ASCII art for "mvirt"
    let ascii_art = "
                              ███             █████   
                             ░░░             ░░███    
 █████████████   █████ █████ ████  ████████  ███████  
░░███░░███░░███ ░░███ ░░███ ░░███ ░░███░░███░░░███░   
 ░███ ░███ ░███  ░███  ░███  ░███  ░███ ░░░   ░███    
 ░███ ░███ ░███  ░░███ ███   ░███  ░███       ░███ ███
 █████░███ █████  ░░█████    █████ █████      ░░█████ 
░░░░░ ░░░ ░░░░░    ░░░░░    ░░░░░ ░░░░░        ░░░░░  
                                                      ";

    let art_height = ascii_art.lines().count() as u16;

    // Center vertically
    let start_y = (area.height.saturating_sub(art_height)) / 2;

    let lines: Vec<Line> = ascii_art
        .lines()
        .map(|line| Line::from(Span::styled(line, Style::default().fg(Color::Cyan))))
        .collect();

    let splash_area = Rect {
        x: area.x,
        y: start_y,
        width: area.width,
        height: art_height.min(area.height),
    };

    frame.render_widget(
        Paragraph::new(lines).alignment(ratatui::prelude::Alignment::Center),
        splash_area,
    );

    // Rainbow pride bar at the bottom
    let rainbow_colors: [Color; 6] = [
        Color::Indexed(196), // Red
        Color::Indexed(208), // Orange
        Color::Indexed(226), // Yellow
        Color::Indexed(46),  // Green
        Color::Indexed(21),  // Blue
        Color::Indexed(129), // Purple
    ];
    let section_width = area.width as f32 / rainbow_colors.len() as f32;
    let make_rainbow_line = || {
        Line::from(
            (0..area.width)
                .map(|i| {
                    let color_idx = (i as f32 / section_width) as usize;
                    let color = rainbow_colors[color_idx.min(rainbow_colors.len() - 1)];
                    Span::styled("█", Style::default().fg(color))
                })
                .collect::<Vec<_>>(),
        )
    };

    let bottom_row = Rect {
        x: area.x,
        y: area.height.saturating_sub(1),
        width: area.width,
        height: 1,
    };
    frame.render_widget(Paragraph::new(make_rainbow_line()), bottom_row);
}

fn draw_detail_view(frame: &mut Frame, vm: &Vm) {
    let area = frame.area();
    let modal_width = 70.min(area.width.saturating_sub(4));
    let modal_height = 22.min(area.height.saturating_sub(4));

    let modal_area = Rect {
        x: (area.width - modal_width) / 2,
        y: (area.height - modal_height) / 2,
        width: modal_width,
        height: modal_height,
    };

    // Clear the modal area
    frame.render_widget(Clear, modal_area);

    // Modal block
    let name_str = vm.name.clone().unwrap_or_else(|| "unnamed".to_string());
    let title = Line::from(vec![
        Span::styled(
            format!(" {} ", name_str),
            Style::default().fg(Color::Cyan).bold(),
        ),
        Span::styled("│", Style::default().fg(Color::DarkGray)),
        Span::styled(" Esc", Style::default().fg(Color::Yellow)),
        Span::styled(": close ", Style::default().fg(Color::DarkGray)),
    ]);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(title);
    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    let config = vm.config.as_ref();
    let label_style = Style::default().fg(Color::DarkGray);
    let value_style = Style::default().fg(Color::White);

    // Build the detail lines
    let mut lines: Vec<Line> = Vec::new();

    // ID
    lines.push(Line::from(vec![
        Span::styled(" ID:          ", label_style),
        Span::styled(&vm.id, value_style),
    ]));

    // State
    let state_text = format_state(vm.state);
    lines.push(Line::from(vec![
        Span::styled(" State:       ", label_style),
        Span::styled(state_text, state_style(vm.state)),
    ]));

    // VCPUs and Memory
    if let Some(cfg) = config {
        lines.push(Line::from(vec![
            Span::styled(" VCPUs:       ", label_style),
            Span::styled(cfg.vcpus.to_string(), value_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled(" Memory:      ", label_style),
            Span::styled(format!("{} MB", cfg.memory_mb), value_style),
        ]));

        // Boot mode
        let boot_mode_str = match BootMode::try_from(cfg.boot_mode) {
            Ok(BootMode::Disk) | Ok(BootMode::Unspecified) | Err(_) => "Disk (UEFI)",
            Ok(BootMode::Kernel) => "Kernel (Direct)",
        };
        lines.push(Line::from(vec![
            Span::styled(" Boot Mode:   ", label_style),
            Span::styled(boot_mode_str, value_style),
        ]));

        // Kernel (if set)
        if let Some(ref kernel) = cfg.kernel {
            lines.push(Line::from(vec![
                Span::styled(" Kernel:      ", label_style),
                Span::styled(kernel, value_style),
            ]));
        }

        // Initramfs (if set)
        if let Some(ref initramfs) = cfg.initramfs {
            lines.push(Line::from(vec![
                Span::styled(" Initramfs:   ", label_style),
                Span::styled(initramfs, value_style),
            ]));
        }

        // Cmdline (if set)
        if let Some(ref cmdline) = cfg.cmdline {
            let display_cmdline = if cmdline.len() > 50 {
                format!("{}…", &cmdline[..50])
            } else {
                cmdline.clone()
            };
            lines.push(Line::from(vec![
                Span::styled(" Cmdline:     ", label_style),
                Span::styled(display_cmdline, value_style),
            ]));
        }

        // Disks
        for (i, disk) in cfg.disks.iter().enumerate() {
            let label = if i == 0 {
                " Disks:       "
            } else {
                "              "
            };
            let readonly = if disk.readonly { " (ro)" } else { "" };
            lines.push(Line::from(vec![
                Span::styled(label, label_style),
                Span::styled(format!("{}{}", disk.path, readonly), value_style),
            ]));
        }

        // NICs
        for (i, nic) in cfg.nics.iter().enumerate() {
            let label = if i == 0 {
                " NICs:        "
            } else {
                "              "
            };
            let nic_info = match (&nic.tap, &nic.mac) {
                (Some(tap), Some(mac)) => format!("{} ({})", tap, mac),
                (Some(tap), None) => tap.clone(),
                (None, Some(mac)) => format!("({})", mac),
                (None, None) => "default".to_string(),
            };
            lines.push(Line::from(vec![
                Span::styled(label, label_style),
                Span::styled(nic_info, value_style),
            ]));
        }

        // User-Data indicator
        if cfg.user_data.is_some() {
            lines.push(Line::from(vec![
                Span::styled(" User-Data:   ", label_style),
                Span::styled("configured", Style::default().fg(Color::Green)),
            ]));
        }
    }

    // Timestamps
    lines.push(Line::from(vec![Span::raw("")])); // Empty line

    let created_time = chrono::DateTime::from_timestamp(vm.created_at, 0)
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| "-".to_string());
    lines.push(Line::from(vec![
        Span::styled(" Created:     ", label_style),
        Span::styled(created_time, value_style),
    ]));

    if let Some(started_at) = vm.started_at {
        let started_time = chrono::DateTime::from_timestamp(started_at, 0)
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| "-".to_string());
        lines.push(Line::from(vec![
            Span::styled(" Started:     ", label_style),
            Span::styled(started_time, value_style),
        ]));
    }

    let text = Text::from(lines);
    frame.render_widget(Paragraph::new(text), inner);
}

async fn action_worker(
    mut client: VmServiceClient<Channel>,
    mut action_rx: mpsc::UnboundedReceiver<Action>,
    result_tx: mpsc::UnboundedSender<ActionResult>,
) {
    while let Some(action) = action_rx.recv().await {
        let result = match action {
            Action::Refresh => match client.list_vms(ListVmsRequest {}).await {
                Ok(response) => ActionResult::Refreshed(Ok(response.into_inner().vms)),
                Err(e) => ActionResult::Refreshed(Err(e.message().to_string())),
            },
            Action::RefreshSystemInfo => {
                match client.get_system_info(GetSystemInfoRequest {}).await {
                    Ok(response) => ActionResult::SystemInfoRefreshed(Ok(response.into_inner())),
                    Err(e) => ActionResult::SystemInfoRefreshed(Err(e.message().to_string())),
                }
            }
            Action::Start(id) => match client.start_vm(StartVmRequest { id: id.clone() }).await {
                Ok(_) => ActionResult::Started(id, Ok(())),
                Err(e) => ActionResult::Started(id, Err(e.message().to_string())),
            },
            Action::Stop(id) => {
                match client
                    .stop_vm(StopVmRequest {
                        id: id.clone(),
                        timeout_seconds: 30,
                    })
                    .await
                {
                    Ok(_) => ActionResult::Stopped(id, Ok(())),
                    Err(e) => ActionResult::Stopped(id, Err(e.message().to_string())),
                }
            }
            Action::Kill(id) => match client.kill_vm(KillVmRequest { id: id.clone() }).await {
                Ok(_) => ActionResult::Killed(id, Ok(())),
                Err(e) => ActionResult::Killed(id, Err(e.message().to_string())),
            },
            Action::Delete(id) => {
                match client.delete_vm(DeleteVmRequest { id: id.clone() }).await {
                    Ok(_) => ActionResult::Deleted(id, Ok(())),
                    Err(e) => ActionResult::Deleted(id, Err(e.message().to_string())),
                }
            }
            Action::Create(params) => {
                // Generate user_data content based on mode
                let user_data_content = match params.user_data_mode {
                    UserDataMode::None => None,
                    UserDataMode::File => {
                        if let Some(path) = &params.user_data_file {
                            match tokio::fs::read_to_string(path).await {
                                Ok(content) => Some(content),
                                Err(e) => {
                                    let _ = result_tx.send(ActionResult::Created(Err(format!(
                                        "Failed to read user-data file: {}",
                                        e
                                    ))));
                                    continue;
                                }
                            }
                        } else {
                            None
                        }
                    }
                    UserDataMode::SshKeys => {
                        if let Some(ref cfg) = params.ssh_keys_config {
                            // Fetch SSH keys based on source
                            let ssh_keys = match cfg.source {
                                SshKeySource::GitHub => {
                                    let url =
                                        format!("https://github.com/{}.keys", cfg.github_user);
                                    match reqwest::get(&url).await {
                                        Ok(resp) => {
                                            if resp.status().is_success() {
                                                match resp.text().await {
                                                    Ok(keys) => keys
                                                        .lines()
                                                        .filter(|l| !l.is_empty())
                                                        .map(|s| s.to_string())
                                                        .collect::<Vec<_>>(),
                                                    Err(e) => {
                                                        let _ = result_tx.send(
                                                            ActionResult::Created(Err(format!(
                                                                "Failed to read GitHub keys: {}",
                                                                e
                                                            ))),
                                                        );
                                                        continue;
                                                    }
                                                }
                                            } else {
                                                let _ = result_tx.send(ActionResult::Created(Err(
                                                    format!(
                                                        "Failed to fetch GitHub keys: HTTP {}",
                                                        resp.status()
                                                    ),
                                                )));
                                                continue;
                                            }
                                        }
                                        Err(e) => {
                                            let _ = result_tx.send(ActionResult::Created(Err(
                                                format!("Failed to fetch GitHub keys: {}", e),
                                            )));
                                            continue;
                                        }
                                    }
                                }
                                SshKeySource::Local => {
                                    match tokio::fs::read_to_string(&cfg.local_path).await {
                                        Ok(content) => content
                                            .lines()
                                            .filter(|l| !l.is_empty())
                                            .map(|s| s.to_string())
                                            .collect::<Vec<_>>(),
                                        Err(e) => {
                                            let _ = result_tx.send(ActionResult::Created(Err(
                                                format!("Failed to read SSH key file: {}", e),
                                            )));
                                            continue;
                                        }
                                    }
                                }
                            };

                            // Generate cloud-init user-data
                            let keys_yaml = ssh_keys
                                .iter()
                                .map(|k| format!("      - {}", k))
                                .collect::<Vec<_>>()
                                .join("\n");

                            // Build user-data with optional root password
                            let password_yaml = if !cfg.root_password.is_empty() {
                                format!(
                                    "\n    lock_passwd: false\n    plain_text_passwd: {}",
                                    cfg.root_password
                                )
                            } else {
                                String::new()
                            };

                            let chpasswd_yaml = if !cfg.root_password.is_empty() {
                                "\nchpasswd:\n  expire: false\nssh_pwauth: true"
                            } else {
                                ""
                            };

                            Some(format!(
                                "#cloud-config\nusers:\n  - name: {}\n    sudo: ALL=(ALL) NOPASSWD:ALL\n    shell: /bin/bash{}\n    ssh_authorized_keys:\n{}{}",
                                cfg.username, password_yaml, keys_yaml, chpasswd_yaml
                            ))
                        } else {
                            None
                        }
                    }
                };

                let disks = if params.disk.is_empty() {
                    vec![]
                } else {
                    vec![DiskConfig {
                        path: params.disk,
                        readonly: false,
                    }]
                };
                let config = VmConfig {
                    vcpus: params.vcpus,
                    memory_mb: params.memory_mb,
                    boot_mode: params.boot_mode,
                    kernel: params.kernel,
                    initramfs: params.initramfs,
                    cmdline: params.cmdline,
                    disks,
                    nics: vec![NicConfig {
                        tap: None,
                        mac: None,
                    }],
                    user_data: user_data_content,
                    nested_virt: params.nested_virt,
                };
                match client
                    .create_vm(CreateVmRequest {
                        name: params.name,
                        config: Some(config),
                    })
                    .await
                {
                    Ok(response) => ActionResult::Created(Ok(response.into_inner().id)),
                    Err(e) => ActionResult::Created(Err(e.message().to_string())),
                }
            }
            Action::OpenConsole { vm_id, vm_name } => {
                // Create channel for sending input to console
                let (input_tx, input_rx) = mpsc::unbounded_channel::<Vec<u8>>();

                // Convert to gRPC stream - first message has VM ID, rest just have data
                let vm_id_clone = vm_id.clone();
                let input_stream = UnboundedReceiverStream::new(input_rx).map(move |data| {
                    ConsoleInput {
                        vm_id: String::new(), // VM ID only needed in first message
                        data,
                    }
                });

                // Wrap in a stream that prepends the initial message with VM ID
                let initial_msg = ConsoleInput {
                    vm_id: vm_id_clone,
                    data: vec![],
                };
                let full_stream = tokio_stream::once(initial_msg).chain(input_stream);

                // Start console connection
                match client.console(full_stream).await {
                    Ok(response) => {
                        let mut output_stream = response.into_inner();
                        let result_tx_clone = result_tx.clone();
                        let vm_id_for_close = vm_id.clone();

                        // Send success with input channel
                        let _ = result_tx.send(ActionResult::ConsoleOpened {
                            vm_id,
                            vm_name,
                            input_tx,
                        });

                        // Spawn task to read output and forward to TUI
                        tokio::spawn(async move {
                            while let Some(result) = output_stream.next().await {
                                match result {
                                    Ok(output) => {
                                        if result_tx_clone
                                            .send(ActionResult::ConsoleOutput(output.data))
                                            .is_err()
                                        {
                                            break;
                                        }
                                    }
                                    Err(e) => {
                                        let _ = result_tx_clone.send(ActionResult::ConsoleClosed(
                                            Some(e.message().to_string()),
                                        ));
                                        return;
                                    }
                                }
                            }
                            let _ = result_tx_clone.send(ActionResult::ConsoleClosed(None));
                            drop(vm_id_for_close); // Silence unused warning
                        });

                        continue; // Don't send result below, we handled it
                    }
                    Err(e) => ActionResult::ConsoleClosed(Some(e.message().to_string())),
                }
            }
        };
        if result_tx.send(result).is_err() {
            break;
        }
    }
}

pub async fn run(client: VmServiceClient<Channel>) -> io::Result<()> {
    // Setup channels
    let (action_tx, action_rx) = mpsc::unbounded_channel();
    let (result_tx, result_rx) = mpsc::unbounded_channel();

    // Spawn background worker
    tokio::spawn(action_worker(client, action_rx, result_tx));

    // Setup terminal
    enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;

    // Show splash screen for 1 second
    terminal.draw(draw_splash)?;
    tokio::time::sleep(Duration::from_secs(1)).await;

    let mut app = App::new(action_tx, result_rx);
    app.refresh();

    let mut last_refresh = std::time::Instant::now();

    loop {
        // Check for results from background worker
        while let Ok(result) = app.result_rx.try_recv() {
            app.handle_result(result);
            last_refresh = std::time::Instant::now();
        }

        // Auto-refresh every 2 seconds
        if last_refresh.elapsed() >= Duration::from_secs(2) && !app.busy {
            app.send_action(Action::Refresh);
            last_refresh = std::time::Instant::now();
        }

        terminal.draw(|frame| draw(frame, &mut app))?;

        if event::poll(Duration::from_millis(50))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            // Console mode takes highest priority - all keys go to VM
            if app.console_session.is_some() {
                app.handle_console_key(key.code, key.modifiers);
                continue;
            }

            // Handle SSH keys modal first (highest priority)
            if app.ssh_keys_modal.is_some() {
                match key.code {
                    KeyCode::Esc => app.close_ssh_keys_modal(),
                    KeyCode::Tab | KeyCode::Down => {
                        if let Some(modal) = &mut app.ssh_keys_modal {
                            modal.focus_next();
                        }
                    }
                    KeyCode::BackTab | KeyCode::Up => {
                        if let Some(modal) = &mut app.ssh_keys_modal {
                            modal.focus_prev();
                        }
                    }
                    KeyCode::Enter => {
                        if let Some(modal) = &app.ssh_keys_modal {
                            if modal.is_add_button() {
                                app.confirm_ssh_keys();
                            } else if modal.is_cancel_button() {
                                app.close_ssh_keys_modal();
                            } else if let Some(modal) = &mut app.ssh_keys_modal {
                                modal.focus_next();
                            }
                        }
                    }
                    KeyCode::Char(' ') => {
                        if let Some(modal) = &mut app.ssh_keys_modal
                            && modal.is_source_field()
                        {
                            modal.toggle_source();
                        }
                    }
                    KeyCode::Backspace => {
                        if let Some(modal) = &mut app.ssh_keys_modal
                            && let Some(input) = modal.current_input()
                        {
                            input.pop();
                        }
                    }
                    KeyCode::Char(c) => {
                        if let Some(modal) = &mut app.ssh_keys_modal
                            && let Some(input) = modal.current_input()
                        {
                            input.push(c);
                        }
                    }
                    _ => {}
                }
            } else if app.file_picker.is_some() {
                // Handle file picker
                match key.code {
                    KeyCode::Esc => app.close_file_picker(),
                    KeyCode::Down => {
                        if let Some(picker) = &mut app.file_picker {
                            picker.select_next();
                        }
                    }
                    KeyCode::Up => {
                        if let Some(picker) = &mut app.file_picker {
                            picker.select_prev();
                        }
                    }
                    KeyCode::Enter => app.select_file(),
                    _ => {}
                }
            } else if app.detail_view.is_some() {
                // Handle detail view
                match key.code {
                    KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => app.close_detail_view(),
                    _ => {}
                }
            } else if app.create_modal.is_some() {
                // Handle create modal
                match key.code {
                    KeyCode::Esc => app.close_create_modal(),
                    KeyCode::Tab | KeyCode::Down => {
                        if let Some(modal) = &mut app.create_modal {
                            modal.focus_next();
                        }
                    }
                    KeyCode::BackTab | KeyCode::Up => {
                        if let Some(modal) = &mut app.create_modal {
                            modal.focus_prev();
                        }
                    }
                    KeyCode::Enter => {
                        if let Some(modal) = &app.create_modal {
                            if modal.focused_field == 10 {
                                // Submit button focused
                                app.submit_create();
                            } else if modal.is_file_field() {
                                // Open file picker for file fields
                                app.open_file_picker();
                            } else if modal.is_user_data_mode_field() {
                                // Handle user data mode action
                                match modal.user_data_mode {
                                    UserDataMode::None => {
                                        // Just move to next field
                                        if let Some(modal) = &mut app.create_modal {
                                            modal.focus_next();
                                        }
                                    }
                                    UserDataMode::SshKeys => {
                                        app.open_ssh_keys_modal();
                                    }
                                    UserDataMode::File => {
                                        app.open_user_data_file_picker();
                                    }
                                }
                            } else if let Some(modal) = &mut app.create_modal {
                                modal.focus_next();
                            }
                        }
                    }
                    KeyCode::Char(' ') => {
                        if let Some(modal) = &mut app.create_modal {
                            if modal.is_boot_mode_field() {
                                modal.toggle_boot_mode();
                            } else if modal.is_nested_virt_field() {
                                modal.toggle_nested_virt();
                            } else if modal.is_user_data_mode_field() {
                                modal.cycle_user_data_mode();
                            }
                        }
                    }
                    KeyCode::Backspace => {
                        if let Some(modal) = &mut app.create_modal
                            && let Some(input) = modal.current_input()
                        {
                            input.pop();
                        }
                    }
                    KeyCode::Char(c) => {
                        if let Some(modal) = &mut app.create_modal {
                            if modal.is_numeric_field() {
                                // Only accept digits for numeric fields
                                if c.is_ascii_digit()
                                    && let Some(input) = modal.current_input()
                                {
                                    input.push(c);
                                }
                            } else if modal.is_name_field() {
                                // Only accept [a-zA-Z0-9-_] for name field
                                if CreateModal::is_valid_name_char(c)
                                    && let Some(input) = modal.current_input()
                                {
                                    input.push(c);
                                }
                            } else if let Some(input) = modal.current_input() {
                                input.push(c);
                            }
                        }
                    }
                    _ => {}
                }
            } else if app.confirm_kill.is_some() {
                // Handle kill confirmation dialog
                match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => app.confirm_kill(),
                    KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => app.cancel_kill(),
                    _ => {}
                }
            } else if app.confirm_delete.is_some() {
                // Handle delete confirmation dialog
                match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => app.confirm_delete(),
                    KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => app.cancel_delete(),
                    _ => {}
                }
            } else {
                // Normal mode
                match key.code {
                    KeyCode::Char('q') => app.should_quit = true,
                    KeyCode::Down => app.next(),
                    KeyCode::Up => app.previous(),
                    KeyCode::Enter => app.open_detail_view(),
                    KeyCode::Char('n') => app.open_create_modal(),
                    KeyCode::Char('c') => app.open_console(),
                    KeyCode::Char('s') => app.start_selected(),
                    KeyCode::Char('S') => app.stop_selected(),
                    KeyCode::Char('k') => app.kill_selected(),
                    KeyCode::Char('d') => app.delete_selected(),
                    _ => {}
                }
            }
        }

        if app.should_quit {
            break;
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;
    Ok(())
}
