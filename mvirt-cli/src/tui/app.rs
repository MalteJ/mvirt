use std::path::PathBuf;

use chrono::Local;
use ratatui::widgets::TableState;
use tokio::sync::mpsc;

use crate::proto::{SystemInfo, Vm, VmState};
use crate::tui::modals::ssh_keys::SshKeysModal;
use crate::tui::modals::vm_create::CreateModal;
use crate::tui::modals::volume_clone::VolumeCloneModal;
use crate::tui::modals::volume_create::VolumeCreateModal;
use crate::tui::modals::volume_import::VolumeImportModal;
use crate::tui::modals::volume_resize::VolumeResizeModal;
use crate::tui::modals::volume_snapshot::VolumeSnapshotModal;
use crate::tui::modals::volume_template::VolumeTemplateModal;
use crate::tui::types::{Action, ActionResult, StorageFocus, StorageState, UserDataMode, View};
use crate::tui::widgets::console::ConsoleSession;
use crate::tui::widgets::file_picker::FilePicker;
use crate::zfs_proto::{Template, Volume};

pub struct App {
    // VM state
    pub vms: Vec<Vm>,
    pub system_info: Option<SystemInfo>,
    pub table_state: TableState,

    // Common state
    pub should_quit: bool,
    pub status_message: Option<String>,
    pub action_tx: mpsc::UnboundedSender<Action>,
    pub result_rx: mpsc::UnboundedReceiver<ActionResult>,
    pub busy: bool,
    pub last_refresh: Option<chrono::DateTime<chrono::Local>>,

    // View navigation
    pub active_view: View,
    pub vm_available: bool,
    pub zfs_available: bool,

    // VM-specific state
    pub confirm_delete: Option<String>,
    pub confirm_kill: Option<String>,
    pub create_modal: Option<CreateModal>,
    pub file_picker: Option<FilePicker>,
    pub file_picker_for_user_data: bool,
    pub ssh_keys_modal: Option<SshKeysModal>,
    pub detail_view: Option<String>,
    pub console_session: Option<ConsoleSession>,

    // Storage state
    pub storage: StorageState,
    pub volume_table_state: TableState,
    pub template_table_state: TableState,
    pub storage_focus: StorageFocus,
    pub confirm_delete_volume: Option<String>,
    pub confirm_delete_template: Option<String>,

    // Storage modals
    pub volume_create_modal: Option<VolumeCreateModal>,
    pub volume_import_modal: Option<VolumeImportModal>,
    pub volume_resize_modal: Option<VolumeResizeModal>,
    pub volume_snapshot_modal: Option<VolumeSnapshotModal>,
    pub volume_template_modal: Option<VolumeTemplateModal>,
    pub volume_clone_modal: Option<VolumeCloneModal>,
}

impl App {
    pub fn new(
        action_tx: mpsc::UnboundedSender<Action>,
        result_rx: mpsc::UnboundedReceiver<ActionResult>,
        vm_available: bool,
        zfs_available: bool,
    ) -> Self {
        Self {
            // VM state
            vms: Vec::new(),
            system_info: None,
            table_state: TableState::default(),

            // Common state
            should_quit: false,
            status_message: None,
            action_tx,
            result_rx,
            busy: false,
            last_refresh: None,

            // View navigation
            active_view: View::Vm,
            vm_available,
            zfs_available,

            // VM-specific state
            confirm_delete: None,
            confirm_kill: None,
            create_modal: None,
            file_picker: None,
            file_picker_for_user_data: false,
            ssh_keys_modal: None,
            detail_view: None,
            console_session: None,

            // Storage state
            storage: StorageState::default(),
            volume_table_state: TableState::default(),
            template_table_state: TableState::default(),
            storage_focus: StorageFocus::Volumes,
            confirm_delete_volume: None,
            confirm_delete_template: None,

            // Storage modals
            volume_create_modal: None,
            volume_import_modal: None,
            volume_resize_modal: None,
            volume_snapshot_modal: None,
            volume_template_modal: None,
            volume_clone_modal: None,
        }
    }

    pub fn send_action(&mut self, action: Action) {
        if self.busy {
            return;
        }
        self.busy = true;
        let _ = self.action_tx.send(action);
    }

    pub fn handle_result(&mut self, result: ActionResult) {
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
            ActionResult::SystemInfoRefreshed(Err(_)) => {}
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
                self.console_session = Some(ConsoleSession::new(vm_id, vm_name, input_tx));
                self.status_message = None;
            }
            ActionResult::ConsoleOutput(data) => {
                if let Some(ref mut session) = self.console_session {
                    session.process_output(&data);
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

            // Storage results
            ActionResult::StorageRefreshed(Ok(state)) => {
                self.storage = state;
                self.status_message = None;
                self.last_refresh = Some(Local::now());
                // Update table selections
                if self.storage.volumes.is_empty() {
                    self.volume_table_state.select(None);
                } else if self.volume_table_state.selected().is_none() {
                    self.volume_table_state.select(Some(0));
                } else if let Some(selected) = self.volume_table_state.selected()
                    && selected >= self.storage.volumes.len()
                {
                    self.volume_table_state
                        .select(Some(self.storage.volumes.len().saturating_sub(1)));
                }
                if self.storage.templates.is_empty() {
                    self.template_table_state.select(None);
                } else if self.template_table_state.selected().is_none() {
                    self.template_table_state.select(Some(0));
                }
            }
            ActionResult::StorageRefreshed(Err(e)) => {
                self.status_message = Some(format!("Storage error: {}", e));
            }
            ActionResult::VolumeCreated(Ok(())) => {
                self.status_message = Some("Volume created".to_string());
                self.refresh_storage();
            }
            ActionResult::VolumeCreated(Err(e)) => {
                self.status_message = Some(format!("Error: {}", e));
            }
            ActionResult::VolumeDeleted(Ok(())) => {
                self.status_message = Some("Volume deleted".to_string());
                self.refresh_storage();
            }
            ActionResult::VolumeDeleted(Err(e)) => {
                self.status_message = Some(format!("Error: {}", e));
            }
            ActionResult::VolumeResized(Ok(())) => {
                self.status_message = Some("Volume resized".to_string());
                self.refresh_storage();
            }
            ActionResult::VolumeResized(Err(e)) => {
                self.status_message = Some(format!("Error: {}", e));
            }
            ActionResult::ImportStarted(Ok(job_id)) => {
                self.status_message = Some(format!("Import started: {}", &job_id[..8]));
                self.refresh_storage();
            }
            ActionResult::ImportStarted(Err(e)) => {
                self.status_message = Some(format!("Error: {}", e));
            }
            ActionResult::ImportCancelled(Ok(())) => {
                self.status_message = Some("Import cancelled".to_string());
                self.refresh_storage();
            }
            ActionResult::ImportCancelled(Err(e)) => {
                self.status_message = Some(format!("Error: {}", e));
            }
            ActionResult::SnapshotCreated(Ok(())) => {
                self.status_message = Some("Snapshot created".to_string());
                self.refresh_storage();
            }
            ActionResult::SnapshotCreated(Err(e)) => {
                self.status_message = Some(format!("Error: {}", e));
            }
            ActionResult::SnapshotDeleted(Ok(())) => {
                self.status_message = Some("Snapshot deleted".to_string());
                self.refresh_storage();
            }
            ActionResult::SnapshotDeleted(Err(e)) => {
                self.status_message = Some(format!("Error: {}", e));
            }
            ActionResult::SnapshotRolledBack(Ok(())) => {
                self.status_message = Some("Snapshot rolled back".to_string());
                self.refresh_storage();
            }
            ActionResult::SnapshotRolledBack(Err(e)) => {
                self.status_message = Some(format!("Error: {}", e));
            }
            ActionResult::TemplateCreated(Ok(())) => {
                self.status_message = Some("Template created".to_string());
                self.refresh_storage();
            }
            ActionResult::TemplateCreated(Err(e)) => {
                self.status_message = Some(format!("Error: {}", e));
            }
            ActionResult::TemplateDeleted(Ok(())) => {
                self.status_message = Some("Template deleted".to_string());
                self.refresh_storage();
            }
            ActionResult::TemplateDeleted(Err(e)) => {
                self.status_message = Some(format!("Error: {}", e));
            }
            ActionResult::VolumeCloned(Ok(())) => {
                self.status_message = Some("Volume cloned".to_string());
                self.refresh_storage();
            }
            ActionResult::VolumeCloned(Err(e)) => {
                self.status_message = Some(format!("Error: {}", e));
            }
        }
    }

    pub fn selected_vm(&self) -> Option<&Vm> {
        self.table_state.selected().and_then(|i| self.vms.get(i))
    }

    pub fn start_selected(&mut self) {
        let Some(id) = self.selected_vm().map(|vm| vm.id.clone()) else {
            return;
        };
        self.status_message = Some(format!("Starting {}...", id));
        self.send_action(Action::Start(id));
    }

    pub fn stop_selected(&mut self) {
        let Some(id) = self.selected_vm().map(|vm| vm.id.clone()) else {
            return;
        };
        self.status_message = Some(format!("Stopping {}...", id));
        self.send_action(Action::Stop(id));
    }

    pub fn kill_selected(&mut self) {
        let Some(id) = self.selected_vm().map(|vm| vm.id.clone()) else {
            return;
        };
        self.confirm_kill = Some(id);
    }

    pub fn confirm_kill(&mut self) {
        if let Some(id) = self.confirm_kill.take() {
            self.status_message = Some(format!("Killing {}...", id));
            self.send_action(Action::Kill(id));
        }
    }

    pub fn cancel_kill(&mut self) {
        self.confirm_kill = None;
    }

    pub fn delete_selected(&mut self) {
        let Some(id) = self.selected_vm().map(|vm| vm.id.clone()) else {
            return;
        };
        self.confirm_delete = Some(id);
    }

    pub fn confirm_delete(&mut self) {
        if let Some(id) = self.confirm_delete.take() {
            self.status_message = Some(format!("Deleting {}...", id));
            self.send_action(Action::Delete(id));
        }
    }

    pub fn cancel_delete(&mut self) {
        self.confirm_delete = None;
    }

    pub fn open_console(&mut self) {
        let Some(vm) = self.selected_vm() else {
            return;
        };
        if vm.state != VmState::Running as i32 {
            self.status_message = Some("Console only available for running VMs".to_string());
            return;
        }
        let vm_id = vm.id.clone();
        let vm_name = vm.name.clone();
        self.status_message = Some("Connecting to console...".to_string());
        self.send_action(Action::OpenConsole { vm_id, vm_name });
    }

    pub fn close_console(&mut self) {
        self.console_session = None;
        self.status_message = Some("Disconnected from console".to_string());
    }

    pub fn open_create_modal(&mut self) {
        // Pass storage data so user can select boot disk from templates/volumes
        self.create_modal = Some(CreateModal::with_storage(
            &self.storage.templates,
            &self.storage.volumes,
        ));
    }

    pub fn close_create_modal(&mut self) {
        self.create_modal = None;
    }

    pub fn submit_create(&mut self) {
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

    pub fn close_file_picker(&mut self) {
        self.file_picker = None;
    }

    pub fn select_file(&mut self) {
        if let Some(picker) = &mut self.file_picker
            && let Some(path) = picker.enter_selected()
        {
            let path_str = path.to_string_lossy().to_string();
            if self.file_picker_for_user_data
                && let Some(modal) = &mut self.create_modal
            {
                modal.set_user_data_file(path_str);
            }
            self.file_picker = None;
            self.file_picker_for_user_data = false;
        }
    }

    pub fn open_user_data_file_picker(&mut self) {
        let start_path = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        self.file_picker = Some(FilePicker::new(start_path, 0));
        self.file_picker_for_user_data = true;
    }

    pub fn open_ssh_keys_modal(&mut self) {
        self.ssh_keys_modal = Some(SshKeysModal::new());
    }

    pub fn close_ssh_keys_modal(&mut self) {
        self.ssh_keys_modal = None;
    }

    pub fn confirm_ssh_keys(&mut self) {
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

    pub fn send_refresh(&self) {
        let _ = self.action_tx.send(Action::Refresh);
        let _ = self.action_tx.send(Action::RefreshSystemInfo);
    }

    pub fn refresh(&mut self) {
        self.status_message = Some("Refreshing...".to_string());
        self.send_refresh();
    }

    pub fn next(&mut self) {
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

    pub fn previous(&mut self) {
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

    pub fn open_detail_view(&mut self) {
        if let Some(vm) = self.selected_vm() {
            self.detail_view = Some(vm.id.clone());
        }
    }

    pub fn close_detail_view(&mut self) {
        self.detail_view = None;
    }

    pub fn get_vm_by_id(&self, id: &str) -> Option<&Vm> {
        self.vms.iter().find(|vm| vm.id == id)
    }

    pub fn handle_user_data_mode_action(&mut self) {
        if let Some(modal) = &self.create_modal {
            match modal.user_data_mode {
                UserDataMode::None => {
                    if let Some(modal) = &mut self.create_modal {
                        modal.focus_next();
                    }
                }
                UserDataMode::SshKeys => {
                    self.open_ssh_keys_modal();
                }
                UserDataMode::File => {
                    self.open_user_data_file_picker();
                }
            }
        }
    }

    // === View Navigation ===

    pub fn toggle_view(&mut self) {
        if !self.zfs_available {
            self.status_message =
                Some("Storage not available (mvirt-zfs not connected)".to_string());
            return;
        }
        self.active_view = match self.active_view {
            View::Vm => View::Storage,
            View::Storage => View::Vm,
        };
        if self.active_view == View::Storage {
            self.refresh_storage();
        }
    }

    pub fn set_view(&mut self, view: View) {
        if view == View::Storage && !self.zfs_available {
            self.status_message =
                Some("Storage not available (mvirt-zfs not connected)".to_string());
            return;
        }
        self.active_view = view;
        if view == View::Storage {
            self.refresh_storage();
        }
    }

    // === Storage Methods ===

    pub fn refresh_storage(&mut self) {
        if self.zfs_available {
            let _ = self.action_tx.send(Action::RefreshStorage);
        }
    }

    pub fn toggle_storage_focus(&mut self) {
        self.storage_focus = match self.storage_focus {
            StorageFocus::Volumes => StorageFocus::Templates,
            StorageFocus::Templates => StorageFocus::Volumes,
        };
    }

    pub fn storage_next(&mut self) {
        match self.storage_focus {
            StorageFocus::Volumes => {
                if self.storage.volumes.is_empty() {
                    return;
                }
                let i = match self.volume_table_state.selected() {
                    Some(i) => {
                        if i >= self.storage.volumes.len() - 1 {
                            0
                        } else {
                            i + 1
                        }
                    }
                    None => 0,
                };
                self.volume_table_state.select(Some(i));
            }
            StorageFocus::Templates => {
                if self.storage.templates.is_empty() {
                    return;
                }
                let i = match self.template_table_state.selected() {
                    Some(i) => {
                        if i >= self.storage.templates.len() - 1 {
                            0
                        } else {
                            i + 1
                        }
                    }
                    None => 0,
                };
                self.template_table_state.select(Some(i));
            }
        }
    }

    pub fn storage_previous(&mut self) {
        match self.storage_focus {
            StorageFocus::Volumes => {
                if self.storage.volumes.is_empty() {
                    return;
                }
                let i = match self.volume_table_state.selected() {
                    Some(i) => {
                        if i == 0 {
                            self.storage.volumes.len() - 1
                        } else {
                            i - 1
                        }
                    }
                    None => 0,
                };
                self.volume_table_state.select(Some(i));
            }
            StorageFocus::Templates => {
                if self.storage.templates.is_empty() {
                    return;
                }
                let i = match self.template_table_state.selected() {
                    Some(i) => {
                        if i == 0 {
                            self.storage.templates.len() - 1
                        } else {
                            i - 1
                        }
                    }
                    None => 0,
                };
                self.template_table_state.select(Some(i));
            }
        }
    }

    pub fn selected_volume(&self) -> Option<&Volume> {
        self.volume_table_state
            .selected()
            .and_then(|i| self.storage.volumes.get(i))
    }

    pub fn delete_selected_volume(&mut self) {
        if let Some(vol) = self.selected_volume() {
            self.confirm_delete_volume = Some(vol.name.clone());
        }
    }

    pub fn confirm_delete_volume(&mut self) {
        if let Some(name) = self.confirm_delete_volume.take() {
            self.status_message = Some(format!("Deleting volume {}...", name));
            self.send_action(Action::DeleteVolume(name));
        }
    }

    pub fn cancel_delete_volume(&mut self) {
        self.confirm_delete_volume = None;
    }

    pub fn delete_selected_template(&mut self) {
        if let Some(tpl) = self
            .template_table_state
            .selected()
            .and_then(|i| self.storage.templates.get(i))
        {
            self.confirm_delete_template = Some(tpl.name.clone());
        }
    }

    pub fn confirm_delete_template(&mut self) {
        if let Some(name) = self.confirm_delete_template.take() {
            self.status_message = Some(format!("Deleting template {}...", name));
            self.send_action(Action::DeleteTemplate(name));
        }
    }

    pub fn cancel_delete_template(&mut self) {
        self.confirm_delete_template = None;
    }

    pub fn selected_template(&self) -> Option<&Template> {
        self.template_table_state
            .selected()
            .and_then(|i| self.storage.templates.get(i))
    }

    // === Storage Modal Methods ===

    // Volume Create Modal
    pub fn open_volume_create_modal(&mut self) {
        self.volume_create_modal = Some(VolumeCreateModal::new());
    }

    pub fn close_volume_create_modal(&mut self) {
        self.volume_create_modal = None;
    }

    pub fn submit_volume_create(&mut self) {
        if let Some(modal) = &self.volume_create_modal {
            match modal.validate() {
                Ok((name, size_bytes)) => {
                    self.status_message = Some(format!("Creating volume {}...", name));
                    self.send_action(Action::CreateVolume { name, size_bytes });
                    self.volume_create_modal = None;
                }
                Err(e) => {
                    self.status_message = Some(format!("Error: {}", e));
                }
            }
        }
    }

    // Volume Import Modal
    pub fn open_volume_import_modal(&mut self) {
        self.volume_import_modal = Some(VolumeImportModal::new());
    }

    pub fn close_volume_import_modal(&mut self) {
        self.volume_import_modal = None;
    }

    pub fn submit_volume_import(&mut self) {
        if let Some(modal) = &self.volume_import_modal {
            match modal.validate() {
                Ok((name, source, size_bytes)) => {
                    self.status_message = Some(format!("Importing volume {}...", name));
                    self.send_action(Action::ImportVolume {
                        name,
                        source,
                        size_bytes,
                    });
                    self.volume_import_modal = None;
                }
                Err(e) => {
                    self.status_message = Some(format!("Error: {}", e));
                }
            }
        }
    }

    // Volume Resize Modal
    pub fn open_volume_resize_modal(&mut self) {
        if let Some(vol) = self.selected_volume() {
            self.volume_resize_modal =
                Some(VolumeResizeModal::new(vol.name.clone(), vol.volsize_bytes));
        }
    }

    pub fn close_volume_resize_modal(&mut self) {
        self.volume_resize_modal = None;
    }

    pub fn submit_volume_resize(&mut self) {
        if let Some(modal) = &self.volume_resize_modal {
            match modal.validate() {
                Ok(new_size) => {
                    let name = modal.volume_name.clone();
                    self.status_message = Some(format!("Resizing volume {}...", name));
                    self.send_action(Action::ResizeVolume { name, new_size });
                    self.volume_resize_modal = None;
                }
                Err(e) => {
                    self.status_message = Some(format!("Error: {}", e));
                }
            }
        }
    }

    // Volume Snapshot Modal
    pub fn open_volume_snapshot_modal(&mut self) {
        if let Some(vol) = self.selected_volume() {
            self.volume_snapshot_modal = Some(VolumeSnapshotModal::new(vol.name.clone()));
        }
    }

    pub fn close_volume_snapshot_modal(&mut self) {
        self.volume_snapshot_modal = None;
    }

    pub fn submit_volume_snapshot(&mut self) {
        if let Some(modal) = &self.volume_snapshot_modal {
            match modal.validate() {
                Ok(snapshot_name) => {
                    let volume = modal.volume_name.clone();
                    self.status_message =
                        Some(format!("Creating snapshot {}@{}...", volume, snapshot_name));
                    self.send_action(Action::CreateSnapshot {
                        volume,
                        name: snapshot_name,
                    });
                    self.volume_snapshot_modal = None;
                }
                Err(e) => {
                    self.status_message = Some(format!("Error: {}", e));
                }
            }
        }
    }

    // Volume Template Modal
    pub fn open_volume_template_modal(&mut self) {
        if let Some(vol) = self.selected_volume() {
            self.volume_template_modal = Some(VolumeTemplateModal::new(vol.name.clone()));
        }
    }

    pub fn close_volume_template_modal(&mut self) {
        self.volume_template_modal = None;
    }

    pub fn submit_volume_template(&mut self) {
        if let Some(modal) = &self.volume_template_modal {
            match modal.validate() {
                Ok(template_name) => {
                    let volume = modal.volume_name.clone();
                    self.status_message = Some(format!(
                        "Creating template {} from {}...",
                        template_name, volume
                    ));
                    self.send_action(Action::CreateTemplate {
                        volume,
                        name: template_name,
                    });
                    self.volume_template_modal = None;
                }
                Err(e) => {
                    self.status_message = Some(format!("Error: {}", e));
                }
            }
        }
    }

    // Volume Clone Modal
    pub fn open_volume_clone_modal(&mut self) {
        if let Some(tpl) = self.selected_template() {
            self.volume_clone_modal = Some(VolumeCloneModal::new(tpl.name.clone(), tpl.size_bytes));
        }
    }

    pub fn close_volume_clone_modal(&mut self) {
        self.volume_clone_modal = None;
    }

    pub fn submit_volume_clone(&mut self) {
        if let Some(modal) = &self.volume_clone_modal {
            match modal.validate() {
                Ok(new_volume) => {
                    let template = modal.template_name.clone();
                    self.status_message = Some(format!(
                        "Cloning {} from template {}...",
                        new_volume, template
                    ));
                    self.send_action(Action::CloneTemplate {
                        template,
                        new_volume,
                    });
                    self.volume_clone_modal = None;
                }
                Err(e) => {
                    self.status_message = Some(format!("Error: {}", e));
                }
            }
        }
    }

    /// Check if any storage modal is currently open
    #[allow(dead_code)]
    pub fn has_storage_modal(&self) -> bool {
        self.volume_create_modal.is_some()
            || self.volume_import_modal.is_some()
            || self.volume_resize_modal.is_some()
            || self.volume_snapshot_modal.is_some()
            || self.volume_template_modal.is_some()
            || self.volume_clone_modal.is_some()
    }
}
