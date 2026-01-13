use std::io;
use std::path::PathBuf;
use std::time::Duration;

use chrono::Local;

use crossterm::ExecutableCommand;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState};
use tokio::sync::mpsc;
use tonic::transport::Channel;

use crate::proto::vm_service_client::VmServiceClient;
use crate::proto::*;

pub(crate) enum Action {
    Refresh,
    Start(String),
    Stop(String),
    Kill(String),
    Delete(String),
    Create(CreateVmParams),
}

#[derive(Clone)]
pub(crate) struct CreateVmParams {
    pub name: Option<String>,
    pub kernel: String,
    pub disk: String,
    pub vcpus: u32,
    pub memory_mb: u64,
    pub user_data: Option<String>,
}

pub(crate) enum ActionResult {
    Refreshed(Result<Vec<Vm>, String>),
    Started(String, Result<(), String>),
    Stopped(String, Result<(), String>),
    Killed(String, Result<(), String>),
    Deleted(String, Result<(), String>),
    Created(Result<String, String>), // Ok(vm_id) or Err(error)
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

#[derive(Default)]
struct CreateModal {
    name: String,
    kernel: String,
    disk: String,
    vcpus: String,
    memory_mb: String,
    user_data: String,
    focused_field: usize, // 0-5 for fields, 6 for submit
}

impl CreateModal {
    fn new() -> Self {
        Self {
            vcpus: "1".to_string(),
            memory_mb: "512".to_string(),
            ..Default::default()
        }
    }

    fn field_count() -> usize {
        7 // 6 fields + submit button
    }

    fn focus_next(&mut self) {
        self.focused_field = (self.focused_field + 1) % Self::field_count();
    }

    fn focus_prev(&mut self) {
        self.focused_field = if self.focused_field == 0 {
            Self::field_count() - 1
        } else {
            self.focused_field - 1
        };
    }

    fn current_input(&mut self) -> Option<&mut String> {
        match self.focused_field {
            0 => Some(&mut self.name),
            1 => Some(&mut self.kernel),
            2 => Some(&mut self.disk),
            3 => Some(&mut self.vcpus),
            4 => Some(&mut self.memory_mb),
            5 => Some(&mut self.user_data),
            _ => None,
        }
    }

    fn is_name_field(&self) -> bool {
        self.focused_field == 0
    }

    fn is_file_field(&self) -> bool {
        matches!(self.focused_field, 1 | 2 | 5) // kernel, disk, user_data
    }

    fn is_numeric_field(&self) -> bool {
        matches!(self.focused_field, 3 | 4) // vcpus, memory_mb
    }

    fn is_valid_name_char(c: char) -> bool {
        c.is_ascii_alphanumeric() || c == '-' || c == '_'
    }

    fn set_field(&mut self, field: usize, value: String) {
        match field {
            0 => self.name = value,
            1 => self.kernel = value,
            2 => self.disk = value,
            3 => self.vcpus = value,
            4 => self.memory_mb = value,
            5 => self.user_data = value,
            _ => {}
        }
    }

    fn validate(&self) -> Result<CreateVmParams, &'static str> {
        if self.kernel.is_empty() {
            return Err("Kernel path is required");
        }
        if self.disk.is_empty() {
            return Err("Disk path is required");
        }
        let vcpus: u32 = self.vcpus.parse().map_err(|_| "Invalid vcpus")?;
        let memory_mb: u64 = self.memory_mb.parse().map_err(|_| "Invalid memory")?;

        Ok(CreateVmParams {
            name: if self.name.is_empty() {
                None
            } else {
                Some(self.name.clone())
            },
            kernel: self.kernel.clone(),
            disk: self.disk.clone(),
            vcpus,
            memory_mb,
            user_data: if self.user_data.is_empty() {
                None
            } else {
                Some(self.user_data.clone())
            },
        })
    }
}

pub struct App {
    vms: Vec<Vm>,
    table_state: TableState,
    should_quit: bool,
    status_message: Option<String>,
    action_tx: mpsc::UnboundedSender<Action>,
    result_rx: mpsc::UnboundedReceiver<ActionResult>,
    busy: bool,
    confirm_delete: Option<String>, // VM ID pending deletion
    last_refresh: Option<chrono::DateTime<chrono::Local>>,
    create_modal: Option<CreateModal>,
    file_picker: Option<FilePicker>,
}

impl App {
    pub fn new(
        action_tx: mpsc::UnboundedSender<Action>,
        result_rx: mpsc::UnboundedReceiver<ActionResult>,
    ) -> Self {
        Self {
            vms: Vec::new(),
            table_state: TableState::default(),
            should_quit: false,
            status_message: None,
            action_tx,
            result_rx,
            busy: false,
            confirm_delete: None,
            last_refresh: None,
            create_modal: None,
            file_picker: None,
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
            ActionResult::Started(id, Ok(())) => {
                self.status_message = Some(format!("Started {}", id));
                self.send_action(Action::Refresh);
            }
            ActionResult::Started(_, Err(e)) => {
                self.status_message = Some(format!("Error: {}", e));
            }
            ActionResult::Stopped(id, Ok(())) => {
                self.status_message = Some(format!("Stopped {}", id));
                self.send_action(Action::Refresh);
            }
            ActionResult::Stopped(_, Err(e)) => {
                self.status_message = Some(format!("Error: {}", e));
            }
            ActionResult::Killed(id, Ok(())) => {
                self.status_message = Some(format!("Killed {}", id));
                self.send_action(Action::Refresh);
            }
            ActionResult::Killed(_, Err(e)) => {
                self.status_message = Some(format!("Error: {}", e));
            }
            ActionResult::Deleted(id, Ok(())) => {
                self.status_message = Some(format!("Deleted {}", id));
                self.send_action(Action::Refresh);
            }
            ActionResult::Deleted(_, Err(e)) => {
                self.status_message = Some(format!("Error: {}", e));
            }
            ActionResult::Created(Ok(id)) => {
                self.status_message = Some(format!("Created {}", id));
                self.send_action(Action::Refresh);
            }
            ActionResult::Created(Err(e)) => {
                self.status_message = Some(format!("Error: {}", e));
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
        self.status_message = Some(format!("Killing {}...", id));
        self.send_action(Action::Kill(id));
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
                    self.send_action(Action::Create(params));
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
            let target_field = picker.target_field;
            let path_str = path.to_string_lossy().to_string();
            if let Some(modal) = &mut self.create_modal {
                modal.set_field(target_field, path_str);
            }
            self.file_picker = None;
        }
    }

    fn refresh(&mut self) {
        self.status_message = Some("Refreshing...".to_string());
        self.send_action(Action::Refresh);
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
}

fn format_state(state: i32) -> &'static str {
    match VmState::try_from(state).unwrap_or(VmState::Unspecified) {
        VmState::Unspecified => "unknown",
        VmState::Stopped => "stopped",
        VmState::Starting => "starting",
        VmState::Running => "running",
        VmState::Stopping => "stopping",
    }
}

fn state_style(state: i32) -> Style {
    match VmState::try_from(state).unwrap_or(VmState::Unspecified) {
        VmState::Running => Style::default().fg(Color::Green),
        VmState::Stopped => Style::default().fg(Color::Red),
        VmState::Starting | VmState::Stopping => Style::default().fg(Color::Yellow),
        VmState::Unspecified => Style::default().fg(Color::Gray),
    }
}

fn draw(frame: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(5),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(frame.area());

    // VM Table
    let header = Row::new(vec!["ID", "NAME", "STATE", "VCPUS", "MEMORY"])
        .style(Style::default().bold())
        .bottom_margin(1);

    let rows: Vec<Row> = app
        .vms
        .iter()
        .map(|vm| {
            let config = vm.config.as_ref();
            let state = vm.state;
            Row::new(vec![
                Cell::from(vm.id.clone()),
                Cell::from(vm.name.clone().unwrap_or_else(|| "-".to_string())),
                Cell::from(format_state(state)).style(state_style(state)),
                Cell::from(config.map(|c| c.vcpus.to_string()).unwrap_or_default()),
                Cell::from(
                    config
                        .map(|c| format!("{}MB", c.memory_mb))
                        .unwrap_or_default(),
                ),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(36),
            Constraint::Min(15),
            Constraint::Length(10),
            Constraint::Length(6),
            Constraint::Length(10),
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::ALL).title(" VMs "))
    .row_highlight_style(Style::default().reversed());

    frame.render_stateful_widget(table, chunks[0], &mut app.table_state);

    // Hotkey legend with refresh time
    let legend_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(20)])
        .split(chunks[1]);

    let legend = Line::from(vec![
        Span::raw(" "),
        Span::styled("c", Style::default().bold()),
        Span::raw(" Create  "),
        Span::styled("s", Style::default().bold()),
        Span::raw(" Start  "),
        Span::styled("S", Style::default().bold()),
        Span::raw(" Stop  "),
        Span::styled("k", Style::default().bold()),
        Span::raw(" Kill  "),
        Span::styled("d", Style::default().bold()),
        Span::raw(" Delete  "),
        Span::styled("q", Style::default().bold()),
        Span::raw(" Quit"),
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
    if let Some(ref id) = app.confirm_delete {
        let confirm_line = Line::from(vec![
            Span::styled(
                format!(" Delete VM {}? ", id),
                Style::default().fg(Color::Red).bold(),
            ),
            Span::styled("y", Style::default().bold()),
            Span::raw("/"),
            Span::styled("n", Style::default().bold()),
        ]);
        frame.render_widget(Paragraph::new(confirm_line), chunks[2]);
    } else if let Some(status) = &app.status_message {
        let status_line = Line::from(vec![Span::styled(
            format!(" {}", status),
            Style::default().fg(Color::Yellow),
        )]);
        frame.render_widget(Paragraph::new(status_line), chunks[2]);
    }

    // Create VM Modal
    if let Some(modal) = &app.create_modal {
        draw_create_modal(frame, modal);
    }

    // File Picker (on top of modal)
    if let Some(picker) = &app.file_picker {
        draw_file_picker(frame, picker);
    }
}

fn draw_create_modal(frame: &mut Frame, modal: &CreateModal) {
    let area = frame.area();
    let modal_width = 70.min(area.width.saturating_sub(4));
    let modal_height = 17.min(area.height.saturating_sub(4));

    let modal_area = Rect {
        x: (area.width - modal_width) / 2,
        y: (area.height - modal_height) / 2,
        width: modal_width,
        height: modal_height,
    };

    // Clear the modal area
    frame.render_widget(Clear, modal_area);

    // Modal block
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Create VM (Tab: next, Enter: browse, Esc: cancel) ")
        .style(Style::default().bg(Color::DarkGray));
    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    // Field layout
    let field_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // Name
            Constraint::Length(2), // Kernel
            Constraint::Length(2), // Disk
            Constraint::Length(2), // VCPUs
            Constraint::Length(2), // Memory
            Constraint::Length(2), // User-Data
            Constraint::Length(2), // Submit button
        ])
        .split(inner);

    // Field definitions: (label, value, is_file_field)
    let fields: [(&str, &str, bool); 6] = [
        ("Name:", &modal.name, false),
        ("Kernel:", &modal.kernel, true),
        ("Disk:", &modal.disk, true),
        ("VCPUs:", &modal.vcpus, false),
        ("Memory (MB):", &modal.memory_mb, false),
        ("User-Data:", &modal.user_data, true),
    ];

    for (i, (label, value, is_file)) in fields.iter().enumerate() {
        let is_focused = modal.focused_field == i;
        let style = if is_focused {
            Style::default().fg(Color::Yellow).bold()
        } else {
            Style::default()
        };

        let cursor = if is_focused { "_" } else { "" };
        let browse_hint = if is_focused && *is_file {
            Span::styled(" [Enter: browse]", Style::default().fg(Color::Cyan))
        } else {
            Span::raw("")
        };

        let line = Line::from(vec![
            Span::styled(format!("{:<14}", label), style),
            Span::raw(format!("{}{}", value, cursor)),
            browse_hint,
        ]);
        frame.render_widget(Paragraph::new(line), field_chunks[i]);
    }

    // Submit button
    let submit_style = if modal.focused_field == 6 {
        Style::default().fg(Color::Green).bold().reversed()
    } else {
        Style::default().fg(Color::Green)
    };
    let submit = Paragraph::new(Line::from(vec![Span::styled(
        "  [ Create ]  ",
        submit_style,
    )]))
    .alignment(ratatui::prelude::Alignment::Center);
    frame.render_widget(submit, field_chunks[6]);
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
                // Read user_data file content if path is provided
                let user_data_content = if let Some(path) = &params.user_data {
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
                };

                let config = VmConfig {
                    vcpus: params.vcpus,
                    memory_mb: params.memory_mb,
                    kernel: params.kernel,
                    initramfs: None,
                    cmdline: None,
                    disks: vec![DiskConfig {
                        path: params.disk,
                        readonly: false,
                    }],
                    nics: vec![NicConfig {
                        tap: None,
                        mac: None,
                    }],
                    user_data: user_data_content,
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
            // Handle file picker first (highest priority)
            if app.file_picker.is_some() {
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
                            if modal.focused_field == 6 {
                                // Submit button focused
                                app.submit_create();
                            } else if modal.is_file_field() {
                                // Open file picker for file fields
                                app.open_file_picker();
                            } else if let Some(modal) = &mut app.create_modal {
                                modal.focus_next();
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
            } else if app.confirm_delete.is_some() {
                // Handle confirmation dialog
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
                    KeyCode::Char('c') => app.open_create_modal(),
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
