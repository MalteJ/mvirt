use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::tui::types::{
    CreateVmParams, CreateVmTab, DataDisk, DiskSourceType, NetworkItem, SshKeySource,
    SshKeysConfig, UserDataMode,
};
use crate::zfs_proto::{Template, Volume};

/// Disk selection item (either a volume or template)
#[derive(Clone)]
pub struct DiskItem {
    pub name: String,
    pub size_bytes: u64,
    pub source_type: DiskSourceType,
}

pub struct CreateModal {
    // Tab navigation
    pub current_tab: CreateVmTab,
    pub focused_field: usize, // Field index within current tab

    // General tab fields
    pub name: String,
    pub vcpus: String,
    pub memory_mb: String,
    pub nested_virt: bool,

    // Storage tab fields
    pub disk_source_type: DiskSourceType,
    pub disk_items: Vec<DiskItem>, // Combined list of templates and volumes
    pub selected_disk: usize,
    pub volume_size_gb: String, // Size for new volume when cloning from template
    // Data disks (additional storage)
    pub data_disks: Vec<DataDisk>,
    pub selected_data_disk: usize,
    pub adding_data_disk: bool,
    pub new_disk_name: String,
    pub new_disk_size_gb: String,

    // Network tab fields
    pub network_items: Vec<NetworkItem>, // Available networks
    pub selected_network: Option<usize>, // None = no network, Some(idx) = selected network

    // Cloud-Init tab fields
    pub user_data_mode: UserDataMode,
    pub user_data_file: String,
    // SSH Keys fields (inline, no separate modal)
    pub ssh_username: String,
    pub ssh_source: SshKeySource,
    pub ssh_github_user: String,
    pub ssh_local_path: String,
    pub ssh_password: String,
}

impl Default for CreateModal {
    fn default() -> Self {
        Self::new()
    }
}

impl CreateModal {
    pub fn new() -> Self {
        let default_ssh_path = dirs::home_dir()
            .map(|p| p.join(".ssh/id_rsa.pub").to_string_lossy().to_string())
            .unwrap_or_else(|| "~/.ssh/id_rsa.pub".to_string());

        Self {
            current_tab: CreateVmTab::General,
            focused_field: 0,
            name: String::new(),
            vcpus: "1".to_string(),
            memory_mb: "512".to_string(),
            nested_virt: false,
            disk_source_type: DiskSourceType::Template,
            disk_items: Vec::new(),
            selected_disk: 0,
            volume_size_gb: String::new(),
            data_disks: Vec::new(),
            selected_data_disk: 0,
            adding_data_disk: false,
            new_disk_name: String::new(),
            new_disk_size_gb: String::new(),
            network_items: Vec::new(),
            selected_network: None,
            user_data_mode: UserDataMode::None,
            user_data_file: String::new(),
            ssh_username: String::new(),
            ssh_source: SshKeySource::GitHub,
            ssh_github_user: String::new(),
            ssh_local_path: default_ssh_path,
            ssh_password: String::new(),
        }
    }

    /// Create modal with pre-populated disk items (no networks)
    #[allow(dead_code)]
    pub fn with_storage(templates: &[Template], volumes: &[Volume]) -> Self {
        Self::with_storage_and_networks(templates, volumes, &[])
    }

    /// Create modal with pre-populated disk and network items
    pub fn with_storage_and_networks(
        templates: &[Template],
        volumes: &[Volume],
        networks: &[NetworkItem],
    ) -> Self {
        let mut modal = Self::new();

        // Add templates first (default selection)
        for tpl in templates {
            modal.disk_items.push(DiskItem {
                name: tpl.name.clone(),
                size_bytes: tpl.size_bytes,
                source_type: DiskSourceType::Template,
            });
        }

        // Then add volumes
        for vol in volumes {
            modal.disk_items.push(DiskItem {
                name: vol.name.clone(),
                size_bytes: vol.volsize_bytes,
                source_type: DiskSourceType::Volume,
            });
        }

        // Add networks
        modal.network_items = networks.to_vec();

        modal
    }

    /// Number of fields in each tab (dynamic based on mode)
    fn field_count_for_tab(&self) -> usize {
        match self.current_tab {
            CreateVmTab::General => 4, // name, vcpus, memory, nested_virt
            CreateVmTab::Storage => {
                if self.adding_data_disk {
                    4 // disk, volume_size, new_disk_name, new_disk_size
                } else {
                    3 // disk, volume_size, data_disks_list
                }
            }
            CreateVmTab::Network => 1, // network
            CreateVmTab::CloudInit => match self.user_data_mode {
                UserDataMode::None => 1,    // just mode selector
                UserDataMode::SshKeys => 5, // mode, username, source, github/path, password
                UserDataMode::File => 2,    // mode, file path
            },
        }
    }

    /// Switch to a specific tab
    pub fn switch_tab(&mut self, tab: CreateVmTab) {
        self.current_tab = tab;
        self.focused_field = 0;
    }

    /// Switch to tab by number (1-4)
    pub fn switch_tab_by_number(&mut self, num: u8) {
        let tab = match num {
            1 => CreateVmTab::General,
            2 => CreateVmTab::Storage,
            3 => CreateVmTab::Network,
            4 => CreateVmTab::CloudInit,
            _ => return,
        };
        self.switch_tab(tab);
    }

    pub fn focus_next(&mut self) {
        let count = self.field_count_for_tab();
        self.focused_field = (self.focused_field + 1) % count;
    }

    pub fn focus_prev(&mut self) {
        let count = self.field_count_for_tab();
        self.focused_field = if self.focused_field == 0 {
            count - 1
        } else {
            self.focused_field - 1
        };
    }

    pub fn current_input(&mut self) -> Option<&mut String> {
        match self.current_tab {
            CreateVmTab::General => match self.focused_field {
                0 => Some(&mut self.name),
                1 => Some(&mut self.vcpus),
                2 => Some(&mut self.memory_mb),
                _ => None,
            },
            CreateVmTab::Storage => {
                if self.adding_data_disk {
                    match self.focused_field {
                        1 => Some(&mut self.volume_size_gb),
                        2 => Some(&mut self.new_disk_name),
                        3 => Some(&mut self.new_disk_size_gb),
                        _ => None,
                    }
                } else {
                    match self.focused_field {
                        1 => Some(&mut self.volume_size_gb),
                        _ => None,
                    }
                }
            }
            CreateVmTab::Network => None,
            CreateVmTab::CloudInit => match self.user_data_mode {
                UserDataMode::None => None,
                UserDataMode::SshKeys => match self.focused_field {
                    1 => Some(&mut self.ssh_username),
                    3 => match self.ssh_source {
                        SshKeySource::GitHub => Some(&mut self.ssh_github_user),
                        SshKeySource::Local => Some(&mut self.ssh_local_path),
                    },
                    4 => Some(&mut self.ssh_password),
                    _ => None, // 0=mode, 2=source toggle
                },
                UserDataMode::File => match self.focused_field {
                    1 => Some(&mut self.user_data_file),
                    _ => None,
                },
            },
        }
    }

    pub fn is_name_field(&self) -> bool {
        self.current_tab == CreateVmTab::General && self.focused_field == 0
    }

    pub fn is_numeric_field(&self) -> bool {
        match self.current_tab {
            CreateVmTab::General => matches!(self.focused_field, 1 | 2), // vcpus, memory
            CreateVmTab::Storage => {
                if self.adding_data_disk {
                    matches!(self.focused_field, 1 | 3) // volume_size, new_disk_size
                } else {
                    self.focused_field == 1 // volume_size
                }
            }
            _ => false,
        }
    }

    pub fn is_nested_virt_field(&self) -> bool {
        self.current_tab == CreateVmTab::General && self.focused_field == 3
    }

    pub fn toggle_nested_virt(&mut self) {
        self.nested_virt = !self.nested_virt;
    }

    pub fn is_disk_field(&self) -> bool {
        self.current_tab == CreateVmTab::Storage && self.focused_field == 0
    }

    pub fn is_network_field(&self) -> bool {
        self.current_tab == CreateVmTab::Network && self.focused_field == 0
    }

    pub fn network_select_next(&mut self) {
        if self.network_items.is_empty() {
            self.selected_network = None;
        } else {
            self.selected_network = match self.selected_network {
                None => Some(0),
                Some(idx) if idx >= self.network_items.len() - 1 => None,
                Some(idx) => Some(idx + 1),
            };
        }
    }

    pub fn network_select_prev(&mut self) {
        if self.network_items.is_empty() {
            self.selected_network = None;
        } else {
            self.selected_network = match self.selected_network {
                None => Some(self.network_items.len() - 1),
                Some(0) => None,
                Some(idx) => Some(idx - 1),
            };
        }
    }

    pub fn is_user_data_mode_field(&self) -> bool {
        self.current_tab == CreateVmTab::CloudInit && self.focused_field == 0
    }

    pub fn cycle_user_data_mode_next(&mut self) {
        self.user_data_mode = match self.user_data_mode {
            UserDataMode::None => UserDataMode::SshKeys,
            UserDataMode::SshKeys => UserDataMode::File,
            UserDataMode::File => UserDataMode::None,
        };
    }

    pub fn cycle_user_data_mode_prev(&mut self) {
        self.user_data_mode = match self.user_data_mode {
            UserDataMode::None => UserDataMode::File,
            UserDataMode::SshKeys => UserDataMode::None,
            UserDataMode::File => UserDataMode::SshKeys,
        };
    }

    pub fn is_ssh_source_field(&self) -> bool {
        self.current_tab == CreateVmTab::CloudInit
            && self.user_data_mode == UserDataMode::SshKeys
            && self.focused_field == 2
    }

    pub fn is_user_data_file_field(&self) -> bool {
        self.current_tab == CreateVmTab::CloudInit
            && self.user_data_mode == UserDataMode::File
            && self.focused_field == 1
    }

    pub fn toggle_ssh_source(&mut self) {
        self.ssh_source = match self.ssh_source {
            SshKeySource::GitHub => SshKeySource::Local,
            SshKeySource::Local => SshKeySource::GitHub,
        };
    }

    pub fn disk_select_next(&mut self) {
        if !self.disk_items.is_empty() {
            self.selected_disk = (self.selected_disk + 1) % self.disk_items.len();
            if let Some(item) = self.disk_items.get(self.selected_disk) {
                self.disk_source_type = item.source_type;
            }
        }
    }

    pub fn disk_select_prev(&mut self) {
        if !self.disk_items.is_empty() {
            self.selected_disk = if self.selected_disk == 0 {
                self.disk_items.len() - 1
            } else {
                self.selected_disk - 1
            };
            if let Some(item) = self.disk_items.get(self.selected_disk) {
                self.disk_source_type = item.source_type;
            }
        }
    }

    /// Toggle between showing templates or volumes only
    pub fn toggle_disk_source_type(&mut self) {
        self.disk_source_type = match self.disk_source_type {
            DiskSourceType::Template => DiskSourceType::Volume,
            DiskSourceType::Volume => DiskSourceType::Template,
        };
        // Find first item of the new type
        for (idx, item) in self.disk_items.iter().enumerate() {
            if item.source_type == self.disk_source_type {
                self.selected_disk = idx;
                break;
            }
        }
    }

    // Data disk methods

    pub fn is_data_disks_field(&self) -> bool {
        self.current_tab == CreateVmTab::Storage
            && self.focused_field == 2
            && !self.adding_data_disk
    }

    pub fn is_new_disk_name_field(&self) -> bool {
        self.current_tab == CreateVmTab::Storage && self.focused_field == 2 && self.adding_data_disk
    }

    pub fn start_adding_data_disk(&mut self) {
        self.adding_data_disk = true;
        self.new_disk_name.clear();
        self.new_disk_size_gb.clear();
        self.focused_field = 2; // Focus on name field
    }

    pub fn cancel_adding_data_disk(&mut self) {
        self.adding_data_disk = false;
        self.new_disk_name.clear();
        self.new_disk_size_gb.clear();
    }

    pub fn confirm_add_data_disk(&mut self) -> Result<(), &'static str> {
        if self.new_disk_name.is_empty() {
            return Err("Disk name is required");
        }
        let size_gb: u64 = self.new_disk_size_gb.parse().map_err(|_| "Invalid size")?;
        if size_gb == 0 {
            return Err("Size must be greater than 0");
        }
        self.data_disks.push(DataDisk {
            name: self.new_disk_name.clone(),
            size_gb,
        });
        self.adding_data_disk = false;
        self.new_disk_name.clear();
        self.new_disk_size_gb.clear();
        self.selected_data_disk = self.data_disks.len().saturating_sub(1);
        Ok(())
    }

    pub fn delete_selected_data_disk(&mut self) {
        if !self.data_disks.is_empty() {
            self.data_disks.remove(self.selected_data_disk);
            if self.selected_data_disk >= self.data_disks.len() && !self.data_disks.is_empty() {
                self.selected_data_disk = self.data_disks.len() - 1;
            }
        }
    }

    pub fn data_disk_select_next(&mut self) {
        if !self.data_disks.is_empty() {
            self.selected_data_disk = (self.selected_data_disk + 1) % self.data_disks.len();
        }
    }

    pub fn data_disk_select_prev(&mut self) {
        if !self.data_disks.is_empty() {
            self.selected_data_disk = if self.selected_data_disk == 0 {
                self.data_disks.len() - 1
            } else {
                self.selected_data_disk - 1
            };
        }
    }

    pub fn is_valid_name_char(c: char) -> bool {
        c.is_ascii_alphanumeric() || c == '-' || c == '_'
    }

    pub fn set_user_data_file(&mut self, path: String) {
        self.user_data_file = path;
    }

    #[allow(dead_code)]
    pub fn selected_disk_item(&self) -> Option<&DiskItem> {
        self.disk_items.get(self.selected_disk)
    }

    pub fn validate(&self) -> Result<CreateVmParams, &'static str> {
        let disk_item = self
            .disk_items
            .get(self.selected_disk)
            .ok_or("No disk selected")?;

        match self.user_data_mode {
            UserDataMode::None => {}
            UserDataMode::SshKeys => {
                if self.ssh_username.is_empty() {
                    return Err("SSH username is required");
                }
                match self.ssh_source {
                    SshKeySource::GitHub => {
                        if self.ssh_github_user.is_empty() {
                            return Err("GitHub username is required");
                        }
                    }
                    SshKeySource::Local => {
                        if self.ssh_local_path.is_empty() {
                            return Err("SSH key file path is required");
                        }
                    }
                }
            }
            UserDataMode::File => {
                if self.user_data_file.is_empty() {
                    return Err("User-data file path is required");
                }
            }
        }

        let vcpus: u32 = self.vcpus.parse().map_err(|_| "Invalid vcpus")?;
        let memory_mb: u64 = self.memory_mb.parse().map_err(|_| "Invalid memory")?;

        // Parse volume size (only used when cloning from template)
        let volume_size_bytes = if disk_item.source_type == DiskSourceType::Template
            && !self.volume_size_gb.is_empty()
        {
            let size_gb: u64 = self
                .volume_size_gb
                .parse()
                .map_err(|_| "Invalid volume size")?;
            let size_bytes = size_gb * 1024 * 1024 * 1024;
            if size_bytes < disk_item.size_bytes {
                return Err("Volume size must be at least template size");
            }
            Some(size_bytes)
        } else {
            None
        };

        // Get selected network ID if any
        let network_id = self
            .selected_network
            .and_then(|idx| self.network_items.get(idx))
            .map(|n| n.id.clone());

        // Build SSH keys config from inline fields if mode is SshKeys
        let ssh_keys_config = if self.user_data_mode == UserDataMode::SshKeys {
            Some(SshKeysConfig {
                username: self.ssh_username.clone(),
                source: self.ssh_source,
                github_user: self.ssh_github_user.clone(),
                local_path: self.ssh_local_path.clone(),
                root_password: self.ssh_password.clone(),
            })
        } else {
            None
        };

        Ok(CreateVmParams {
            name: if self.name.is_empty() {
                None
            } else {
                Some(self.name.clone())
            },
            disk_source_type: disk_item.source_type,
            disk_name: disk_item.name.clone(),
            volume_size_bytes,
            vcpus,
            memory_mb,
            nested_virt: self.nested_virt,
            user_data_mode: self.user_data_mode,
            user_data_file: if self.user_data_file.is_empty() {
                None
            } else {
                Some(self.user_data_file.clone())
            },
            ssh_keys_config,
            network_id,
            data_disks: self.data_disks.clone(),
        })
    }
}

/// Format bytes to human-readable size
fn format_size(bytes: u64) -> String {
    const GB: u64 = 1024 * 1024 * 1024;
    const TB: u64 = GB * 1024;

    if bytes >= TB {
        format!("{:.1}T", bytes as f64 / TB as f64)
    } else {
        format!("{:.1}G", bytes as f64 / GB as f64)
    }
}

pub fn draw(frame: &mut Frame, modal: &CreateModal) {
    let area = frame.area();
    let modal_width = 70.min(area.width.saturating_sub(4));
    let modal_height = 18.min(area.height.saturating_sub(4));

    let modal_area = Rect {
        x: (area.width - modal_width) / 2,
        y: (area.height - modal_height) / 2,
        width: modal_width,
        height: modal_height,
    };

    frame.render_widget(Clear, modal_area);

    let title = Line::from(vec![
        Span::styled(" Create VM ", Style::default().fg(Color::Cyan).bold()),
        Span::styled("|", Style::default().fg(Color::DarkGray)),
        Span::styled(" Tab", Style::default().fg(Color::Yellow)),
        Span::styled(": next ", Style::default().fg(Color::DarkGray)),
        Span::styled("Enter", Style::default().fg(Color::Green)),
        Span::styled(": create ", Style::default().fg(Color::DarkGray)),
        Span::styled("Esc", Style::default().fg(Color::Red)),
        Span::styled(": cancel ", Style::default().fg(Color::DarkGray)),
    ]);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(title);
    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    // Main layout: tab bar + content + create button
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // Tab bar
            Constraint::Min(8),    // Tab content
            Constraint::Length(2), // Create button
        ])
        .split(inner);

    // Draw tab bar
    draw_tab_bar(frame, main_chunks[0], modal.current_tab);

    // Draw tab content
    match modal.current_tab {
        CreateVmTab::General => draw_general_tab(frame, main_chunks[1], modal),
        CreateVmTab::Storage => draw_storage_tab(frame, main_chunks[1], modal),
        CreateVmTab::Network => draw_network_tab(frame, main_chunks[1], modal),
        CreateVmTab::CloudInit => draw_cloud_init_tab(frame, main_chunks[1], modal),
    }

    // Create button (always visible)
    let submit_style = Style::default().fg(Color::Green);
    let submit = Paragraph::new(Line::from(vec![Span::styled(
        "  \u{25b6} Create VM  ",
        submit_style,
    )]))
    .alignment(Alignment::Center);
    frame.render_widget(submit, main_chunks[2]);
}

fn draw_tab_bar(frame: &mut Frame, area: Rect, current_tab: CreateVmTab) {
    let tabs = [
        (CreateVmTab::General, "1:General"),
        (CreateVmTab::Storage, "2:Storage"),
        (CreateVmTab::Network, "3:Network"),
        (CreateVmTab::CloudInit, "4:Cloud-Init"),
    ];

    let mut spans = vec![Span::raw(" ")];
    for (i, (tab, label)) in tabs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" ", Style::default()));
        }
        let style = if *tab == current_tab {
            Style::default().fg(Color::Cyan).bold()
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let bracket_style = if *tab == current_tab {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        spans.push(Span::styled("[", bracket_style));
        spans.push(Span::styled(*label, style));
        spans.push(Span::styled("]", bracket_style));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_general_tab(frame: &mut Frame, area: Rect, modal: &CreateModal) {
    let label_focused = Style::default().fg(Color::Cyan).bold();
    let label_normal = Style::default().fg(Color::DarkGray);
    let value_focused = Style::default().fg(Color::White);
    let value_normal = Style::default().fg(Color::Gray);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Padding
            Constraint::Length(2), // Name
            Constraint::Length(2), // VCPUs
            Constraint::Length(2), // Memory
            Constraint::Length(2), // Nested virt
            Constraint::Min(0),    // Spacer
        ])
        .split(area);

    // Name field (field 0)
    let name_focused = modal.current_tab == CreateVmTab::General && modal.focused_field == 0;
    let cursor = if name_focused { "\u{258c}" } else { "" };
    let name_line = if modal.name.is_empty() && !name_focused {
        Line::from(vec![
            Span::styled(" Name:       ", label_normal),
            Span::styled("optional", Style::default().fg(Color::DarkGray)),
        ])
    } else {
        Line::from(vec![
            Span::styled(
                " Name:       ",
                if name_focused {
                    label_focused
                } else {
                    label_normal
                },
            ),
            Span::styled(
                format!("{}{}", modal.name, cursor),
                if name_focused {
                    value_focused
                } else {
                    value_normal
                },
            ),
        ])
    };
    frame.render_widget(Paragraph::new(name_line), chunks[1]);

    // VCPUs (field 1)
    let vcpus_focused = modal.current_tab == CreateVmTab::General && modal.focused_field == 1;
    let vcpus_cursor = if vcpus_focused { "\u{258c}" } else { "" };
    let vcpus_line = Line::from(vec![
        Span::styled(
            " VCPUs:      ",
            if vcpus_focused {
                label_focused
            } else {
                label_normal
            },
        ),
        Span::styled(
            format!("{}{}", modal.vcpus, vcpus_cursor),
            if vcpus_focused {
                value_focused
            } else {
                value_normal
            },
        ),
    ]);
    frame.render_widget(Paragraph::new(vcpus_line), chunks[2]);

    // Memory (field 2)
    let memory_focused = modal.current_tab == CreateVmTab::General && modal.focused_field == 2;
    let memory_cursor = if memory_focused { "\u{258c}" } else { "" };
    let memory_line = Line::from(vec![
        Span::styled(
            " Memory:     ",
            if memory_focused {
                label_focused
            } else {
                label_normal
            },
        ),
        Span::styled(
            format!("{}{}", modal.memory_mb, memory_cursor),
            if memory_focused {
                value_focused
            } else {
                value_normal
            },
        ),
        Span::styled(" MB", Style::default().fg(Color::DarkGray)),
    ]);
    frame.render_widget(Paragraph::new(memory_line), chunks[3]);

    // Nested Virt (field 3)
    let nested_focused = modal.current_tab == CreateVmTab::General && modal.focused_field == 3;
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
    frame.render_widget(Paragraph::new(nested_line), chunks[4]);
}

fn draw_storage_tab(frame: &mut Frame, area: Rect, modal: &CreateModal) {
    let label_focused = Style::default().fg(Color::Cyan).bold();
    let label_normal = Style::default().fg(Color::DarkGray);
    let value_focused = Style::default().fg(Color::White);
    let value_normal = Style::default().fg(Color::Gray);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Padding
            Constraint::Length(4), // Boot disk selector
            Constraint::Length(2), // Volume size
            Constraint::Min(4),    // Data disks section
        ])
        .split(area);

    // Boot Disk selector (field 0)
    let disk_focused = modal.current_tab == CreateVmTab::Storage && modal.focused_field == 0;
    let type_str = match modal.disk_source_type {
        DiskSourceType::Template => "Template",
        DiskSourceType::Volume => "Volume",
    };

    let disk_header = Line::from(vec![
        Span::styled(
            " Boot Disk:  ",
            if disk_focused {
                label_focused
            } else {
                label_normal
            },
        ),
        Span::styled(
            format!("[{}]", type_str),
            if disk_focused {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::DarkGray)
            },
        ),
        if disk_focused {
            Span::styled(
                " [Space: switch type, ↑↓: select]",
                Style::default().fg(Color::Yellow),
            )
        } else {
            Span::raw("")
        },
    ]);
    frame.render_widget(Paragraph::new(disk_header), chunks[1]);

    // Show disk list
    let disk_list_area = Rect {
        x: chunks[1].x + 13,
        y: chunks[1].y + 1,
        width: chunks[1].width.saturating_sub(14),
        height: 3,
    };

    if modal.disk_items.is_empty() {
        let no_disks = Line::from(vec![Span::styled(
            "No templates or volumes available",
            Style::default().fg(Color::Red),
        )]);
        frame.render_widget(Paragraph::new(no_disks), disk_list_area);
    } else {
        let filtered_items: Vec<(usize, &DiskItem)> = modal
            .disk_items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.source_type == modal.disk_source_type)
            .collect();

        let selected_in_filtered = filtered_items
            .iter()
            .position(|(idx, _)| *idx == modal.selected_disk)
            .unwrap_or(0);

        let start = selected_in_filtered.saturating_sub(1);
        let visible: Vec<_> = filtered_items.iter().skip(start).take(3).collect();

        let mut lines = Vec::new();
        for (orig_idx, item) in visible.iter() {
            let is_selected = *orig_idx == modal.selected_disk;
            let prefix = if is_selected { "▶ " } else { "  " };
            let style = if is_selected && disk_focused {
                Style::default().fg(Color::White).bold()
            } else if is_selected {
                Style::default().fg(Color::Gray)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            lines.push(Line::from(vec![
                Span::styled(prefix, style),
                Span::styled(format!("{:<20}", item.name), style),
                Span::styled(format!(" {}", format_size(item.size_bytes)), style),
            ]));
        }

        while lines.len() < 3 {
            lines.push(Line::from(""));
        }

        frame.render_widget(Paragraph::new(lines), disk_list_area);
    }

    // Volume Size (field 1)
    let size_focused = modal.current_tab == CreateVmTab::Storage && modal.focused_field == 1;
    if modal.disk_source_type == DiskSourceType::Template {
        let size_cursor = if size_focused { "\u{258c}" } else { "" };
        let min_size_gb = modal
            .disk_items
            .get(modal.selected_disk)
            .map(|d| d.size_bytes / (1024 * 1024 * 1024))
            .unwrap_or(0);
        let size_line = Line::from(vec![
            Span::styled(
                " Vol Size:   ",
                if size_focused {
                    label_focused
                } else {
                    label_normal
                },
            ),
            Span::styled(
                format!("{}{}", modal.volume_size_gb, size_cursor),
                if size_focused {
                    value_focused
                } else {
                    value_normal
                },
            ),
            Span::styled(
                format!(" GB (min: {})", min_size_gb),
                Style::default().fg(Color::DarkGray),
            ),
        ]);
        frame.render_widget(Paragraph::new(size_line), chunks[2]);
    } else {
        let size_line = Line::from(vec![
            Span::styled(" Vol Size:   ", label_normal),
            Span::styled(
                "(uses existing volume)",
                Style::default().fg(Color::DarkGray),
            ),
        ]);
        frame.render_widget(Paragraph::new(size_line), chunks[2]);
    }

    // Data Disks section (field 2, or fields 2-3 when adding)
    draw_data_disks_section(
        frame,
        chunks[3],
        modal,
        label_focused,
        label_normal,
        value_focused,
    );
}

fn draw_data_disks_section(
    frame: &mut Frame,
    area: Rect,
    modal: &CreateModal,
    label_focused: Style,
    label_normal: Style,
    value_focused: Style,
) {
    if modal.adding_data_disk {
        // Adding new disk mode
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // Header
                Constraint::Length(1), // Name input
                Constraint::Length(1), // Size input
                Constraint::Min(0),    // Spacer
            ])
            .split(area);

        // Header
        let header = Line::from(vec![
            Span::styled(" Data Disks: ", label_focused),
            Span::styled("Adding new disk...", Style::default().fg(Color::Yellow)),
        ]);
        frame.render_widget(Paragraph::new(header), chunks[0]);

        // Name input (field 2)
        let name_focused = modal.focused_field == 2;
        let cursor = if name_focused { "\u{258c}" } else { "" };
        let name_line = Line::from(vec![
            Span::styled(
                "   Name:     ",
                if name_focused {
                    label_focused
                } else {
                    label_normal
                },
            ),
            Span::styled(
                format!("{}{}", modal.new_disk_name, cursor),
                if name_focused {
                    value_focused
                } else {
                    Style::default().fg(Color::Gray)
                },
            ),
        ]);
        frame.render_widget(Paragraph::new(name_line), chunks[1]);

        // Size input (field 3)
        let size_focused = modal.focused_field == 3;
        let cursor = if size_focused { "\u{258c}" } else { "" };
        let size_line = Line::from(vec![
            Span::styled(
                "   Size:     ",
                if size_focused {
                    label_focused
                } else {
                    label_normal
                },
            ),
            Span::styled(
                format!("{}{}", modal.new_disk_size_gb, cursor),
                if size_focused {
                    value_focused
                } else {
                    Style::default().fg(Color::Gray)
                },
            ),
            Span::styled(" GB", Style::default().fg(Color::DarkGray)),
            Span::styled(
                " [Enter: add, Esc: cancel]",
                Style::default().fg(Color::Yellow),
            ),
        ]);
        frame.render_widget(Paragraph::new(size_line), chunks[2]);
    } else {
        // Normal mode: show list and add button
        let data_focused = modal.current_tab == CreateVmTab::Storage && modal.focused_field == 2;

        let header = Line::from(vec![
            Span::styled(
                " Data Disks: ",
                if data_focused {
                    label_focused
                } else {
                    label_normal
                },
            ),
            if modal.data_disks.is_empty() {
                Span::styled("none", Style::default().fg(Color::DarkGray))
            } else {
                Span::styled(
                    format!("{} disk(s)", modal.data_disks.len()),
                    Style::default().fg(Color::Gray),
                )
            },
            if data_focused {
                Span::styled(
                    " [a: add, d: delete, ↑↓: select]",
                    Style::default().fg(Color::Yellow),
                )
            } else {
                Span::raw("")
            },
        ]);
        frame.render_widget(Paragraph::new(header), area);

        // Show data disks list below header
        if !modal.data_disks.is_empty() {
            let list_area = Rect {
                x: area.x + 13,
                y: area.y + 1,
                width: area.width.saturating_sub(14),
                height: area.height.saturating_sub(1),
            };

            let mut lines = Vec::new();
            for (idx, disk) in modal.data_disks.iter().enumerate() {
                let is_selected = idx == modal.selected_data_disk;
                let prefix = if is_selected { "▶ " } else { "  " };
                let style = if is_selected && data_focused {
                    Style::default().fg(Color::White).bold()
                } else if is_selected {
                    Style::default().fg(Color::Gray)
                } else {
                    Style::default().fg(Color::DarkGray)
                };

                lines.push(Line::from(vec![
                    Span::styled(prefix, style),
                    Span::styled(format!("{:<20}", disk.name), style),
                    Span::styled(format!(" {}G", disk.size_gb), style),
                ]));
            }

            frame.render_widget(Paragraph::new(lines), list_area);
        }
    }
}

fn draw_network_tab(frame: &mut Frame, area: Rect, modal: &CreateModal) {
    let label_focused = Style::default().fg(Color::Cyan).bold();
    let label_normal = Style::default().fg(Color::DarkGray);
    let value_focused = Style::default().fg(Color::White);
    let value_normal = Style::default().fg(Color::Gray);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Padding
            Constraint::Length(2), // Network
            Constraint::Min(0),    // Spacer
        ])
        .split(area);

    // Network selector (field 0)
    let network_focused = modal.current_tab == CreateVmTab::Network && modal.focused_field == 0;
    let network_line = match modal.selected_network {
        None => Line::from(vec![
            Span::styled(
                " Network:    ",
                if network_focused {
                    label_focused
                } else {
                    label_normal
                },
            ),
            Span::styled("none", Style::default().fg(Color::DarkGray)),
            if network_focused && !modal.network_items.is_empty() {
                Span::styled(" [↑↓: select]", Style::default().fg(Color::Yellow))
            } else {
                Span::raw("")
            },
        ]),
        Some(idx) => {
            let name = modal
                .network_items
                .get(idx)
                .map(|n| n.name.clone())
                .unwrap_or_else(|| "Unknown".to_string());
            Line::from(vec![
                Span::styled(
                    " Network:    ",
                    if network_focused {
                        label_focused
                    } else {
                        label_normal
                    },
                ),
                Span::styled(
                    name,
                    if network_focused {
                        value_focused
                    } else {
                        value_normal
                    },
                ),
                if network_focused {
                    Span::styled(" [↑↓: select]", Style::default().fg(Color::Yellow))
                } else {
                    Span::raw("")
                },
            ])
        }
    };
    frame.render_widget(Paragraph::new(network_line), chunks[1]);
}

fn draw_cloud_init_tab(frame: &mut Frame, area: Rect, modal: &CreateModal) {
    let label_focused = Style::default().fg(Color::Cyan).bold();
    let label_normal = Style::default().fg(Color::DarkGray);
    let value_focused = Style::default().fg(Color::White);
    let value_normal = Style::default().fg(Color::Gray);

    match modal.user_data_mode {
        UserDataMode::None => {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1), // Padding
                    Constraint::Length(2), // Mode
                    Constraint::Min(0),    // Spacer
                ])
                .split(area);

            let mode_focused = modal.focused_field == 0;
            let mode_line = Line::from(vec![
                Span::styled(
                    " Mode:       ",
                    if mode_focused {
                        label_focused
                    } else {
                        label_normal
                    },
                ),
                Span::styled("none", Style::default().fg(Color::DarkGray)),
                if mode_focused {
                    Span::styled(" [↑↓: select]", Style::default().fg(Color::Yellow))
                } else {
                    Span::raw("")
                },
            ]);
            frame.render_widget(Paragraph::new(mode_line), chunks[1]);
        }
        UserDataMode::SshKeys => {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1), // Padding
                    Constraint::Length(2), // Mode
                    Constraint::Length(2), // Username
                    Constraint::Length(2), // Source
                    Constraint::Length(2), // GitHub user / Local path
                    Constraint::Length(2), // Password
                    Constraint::Min(0),    // Spacer
                ])
                .split(area);

            // Mode (field 0)
            let mode_focused = modal.focused_field == 0;
            let mode_line = Line::from(vec![
                Span::styled(
                    " Mode:       ",
                    if mode_focused {
                        label_focused
                    } else {
                        label_normal
                    },
                ),
                Span::styled(
                    "SSH Keys",
                    if mode_focused {
                        value_focused
                    } else {
                        value_normal
                    },
                ),
                if mode_focused {
                    Span::styled(" [↑↓: select]", Style::default().fg(Color::Yellow))
                } else {
                    Span::raw("")
                },
            ]);
            frame.render_widget(Paragraph::new(mode_line), chunks[1]);

            // Username (field 1)
            let username_focused = modal.focused_field == 1;
            let cursor = if username_focused { "\u{258c}" } else { "" };
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
                    format!("{}{}", modal.ssh_username, cursor),
                    if username_focused {
                        value_focused
                    } else {
                        value_normal
                    },
                ),
            ]);
            frame.render_widget(Paragraph::new(username_line), chunks[2]);

            // Source (field 2)
            let source_focused = modal.focused_field == 2;
            let source_str = match modal.ssh_source {
                SshKeySource::GitHub => "(\u{25cf}) GitHub  ( ) Local",
                SshKeySource::Local => "( ) GitHub  (\u{25cf}) Local",
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
            frame.render_widget(Paragraph::new(source_line), chunks[3]);

            // GitHub user or Local path (field 3)
            let value_focused_field = modal.focused_field == 3;
            let cursor = if value_focused_field { "\u{258c}" } else { "" };
            let (label, value) = match modal.ssh_source {
                SshKeySource::GitHub => ("GitHub User:", &modal.ssh_github_user),
                SshKeySource::Local => ("Key File:", &modal.ssh_local_path),
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
            frame.render_widget(Paragraph::new(value_line), chunks[4]);

            // Password (field 4)
            let password_focused = modal.focused_field == 4;
            let cursor = if password_focused { "\u{258c}" } else { "" };
            let password_line = if modal.ssh_password.is_empty() {
                Line::from(vec![
                    Span::styled(
                        " Password:   ",
                        if password_focused {
                            label_focused
                        } else {
                            label_normal
                        },
                    ),
                    if password_focused {
                        Span::styled(cursor, value_focused)
                    } else {
                        Span::styled("optional", Style::default().fg(Color::DarkGray))
                    },
                ])
            } else {
                let password_display = "*".repeat(modal.ssh_password.len());
                Line::from(vec![
                    Span::styled(
                        " Password:   ",
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
                ])
            };
            frame.render_widget(Paragraph::new(password_line), chunks[5]);
        }
        UserDataMode::File => {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1), // Padding
                    Constraint::Length(2), // Mode
                    Constraint::Length(2), // File path
                    Constraint::Min(0),    // Spacer
                ])
                .split(area);

            // Mode (field 0)
            let mode_focused = modal.focused_field == 0;
            let mode_line = Line::from(vec![
                Span::styled(
                    " Mode:       ",
                    if mode_focused {
                        label_focused
                    } else {
                        label_normal
                    },
                ),
                Span::styled(
                    "File",
                    if mode_focused {
                        value_focused
                    } else {
                        value_normal
                    },
                ),
                if mode_focused {
                    Span::styled(" [↑↓: select]", Style::default().fg(Color::Yellow))
                } else {
                    Span::raw("")
                },
            ]);
            frame.render_widget(Paragraph::new(mode_line), chunks[1]);

            // File path (field 1)
            let file_focused = modal.focused_field == 1;
            let cursor = if file_focused { "\u{258c}" } else { "" };
            let display_path = if modal.user_data_file.is_empty() && !file_focused {
                "not selected".to_string()
            } else {
                format!("{}{}", modal.user_data_file, cursor)
            };
            let file_line = Line::from(vec![
                Span::styled(
                    " File:       ",
                    if file_focused {
                        label_focused
                    } else {
                        label_normal
                    },
                ),
                Span::styled(
                    display_path,
                    if file_focused {
                        value_focused
                    } else if modal.user_data_file.is_empty() {
                        Style::default().fg(Color::DarkGray)
                    } else {
                        value_normal
                    },
                ),
                if file_focused {
                    Span::styled(" [Enter: browse]", Style::default().fg(Color::Yellow))
                } else {
                    Span::raw("")
                },
            ]);
            frame.render_widget(Paragraph::new(file_line), chunks[2]);
        }
    }
}
