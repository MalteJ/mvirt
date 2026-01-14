use std::path::PathBuf;

use chrono::Local;
use ratatui::widgets::TableState;
use tokio::sync::mpsc;

use crate::proto::{SystemInfo, Vm, VmState};
use crate::tui::modals::ssh_keys::SshKeysModal;
use crate::tui::modals::vm_create::CreateModal;
use crate::tui::types::{Action, ActionResult, UserDataMode};
use crate::tui::widgets::console::ConsoleSession;
use crate::tui::widgets::file_picker::FilePicker;

pub struct App {
    pub vms: Vec<Vm>,
    pub system_info: Option<SystemInfo>,
    pub table_state: TableState,
    pub should_quit: bool,
    pub status_message: Option<String>,
    pub action_tx: mpsc::UnboundedSender<Action>,
    pub result_rx: mpsc::UnboundedReceiver<ActionResult>,
    pub busy: bool,
    pub confirm_delete: Option<String>,
    pub confirm_kill: Option<String>,
    pub last_refresh: Option<chrono::DateTime<chrono::Local>>,
    pub create_modal: Option<CreateModal>,
    pub file_picker: Option<FilePicker>,
    pub file_picker_for_user_data: bool,
    pub ssh_keys_modal: Option<SshKeysModal>,
    pub detail_view: Option<String>,
    pub console_session: Option<ConsoleSession>,
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
        self.create_modal = Some(CreateModal::new());
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

    pub fn open_file_picker(&mut self) {
        if let Some(modal) = &self.create_modal
            && modal.is_file_field()
        {
            let start_path = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
            self.file_picker = Some(FilePicker::new(start_path, modal.focused_field));
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
}
