use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::tui::types::{
    CreateVmParams, DiskSourceType, NetworkItem, SshKeySource, SshKeysConfig, UserDataMode,
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
    pub name: String,
    pub disk_source_type: DiskSourceType,
    pub disk_items: Vec<DiskItem>, // Combined list of templates and volumes
    pub selected_disk: usize,
    pub vcpus: String,
    pub memory_mb: String,
    pub nested_virt: bool,
    pub user_data_mode: UserDataMode,
    pub user_data_file: String,
    pub ssh_keys_config: Option<SshKeysConfig>,
    pub network_items: Vec<NetworkItem>, // Available networks
    pub selected_network: Option<usize>, // None = no network, Some(idx) = selected network
    pub focused_field: usize,
}

impl Default for CreateModal {
    fn default() -> Self {
        Self::new()
    }
}

impl CreateModal {
    pub fn new() -> Self {
        Self {
            name: String::new(),
            disk_source_type: DiskSourceType::Template,
            disk_items: Vec::new(),
            selected_disk: 0,
            vcpus: "1".to_string(),
            memory_mb: "512".to_string(),
            nested_virt: false,
            user_data_mode: UserDataMode::None,
            user_data_file: String::new(),
            ssh_keys_config: None,
            network_items: Vec::new(),
            selected_network: None,
            focused_field: 0,
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

    pub fn field_count() -> usize {
        8 // name, disk, vcpus, memory, nested_virt, network, user_data, submit
    }

    pub fn focus_next(&mut self) {
        self.focused_field = (self.focused_field + 1) % Self::field_count();
    }

    pub fn focus_prev(&mut self) {
        self.focused_field = if self.focused_field == 0 {
            Self::field_count() - 1
        } else {
            self.focused_field - 1
        };
    }

    pub fn current_input(&mut self) -> Option<&mut String> {
        match self.focused_field {
            0 => Some(&mut self.name),
            2 => Some(&mut self.vcpus),
            3 => Some(&mut self.memory_mb),
            _ => None,
        }
    }

    pub fn is_name_field(&self) -> bool {
        self.focused_field == 0
    }

    pub fn is_disk_field(&self) -> bool {
        self.focused_field == 1
    }

    pub fn is_numeric_field(&self) -> bool {
        matches!(self.focused_field, 2 | 3)
    }

    pub fn is_nested_virt_field(&self) -> bool {
        self.focused_field == 4
    }

    pub fn toggle_nested_virt(&mut self) {
        self.nested_virt = !self.nested_virt;
    }

    pub fn is_network_field(&self) -> bool {
        self.focused_field == 5
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
        self.focused_field == 6
    }

    pub fn cycle_user_data_mode(&mut self) {
        self.user_data_mode = match self.user_data_mode {
            UserDataMode::None => UserDataMode::SshKeys,
            UserDataMode::SshKeys => UserDataMode::File,
            UserDataMode::File => UserDataMode::None,
        };
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

    pub fn is_submit_field(&self) -> bool {
        self.focused_field == 7
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

    pub fn is_valid_name_char(c: char) -> bool {
        c.is_ascii_alphanumeric() || c == '-' || c == '_'
    }

    pub fn set_user_data_file(&mut self, path: String) {
        self.user_data_file = path;
    }

    pub fn set_ssh_keys_config(&mut self, config: SshKeysConfig) {
        self.ssh_keys_config = Some(config);
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

        // Get selected network ID if any
        let network_id = self
            .selected_network
            .and_then(|idx| self.network_items.get(idx))
            .map(|n| n.id.clone());

        Ok(CreateVmParams {
            name: if self.name.is_empty() {
                None
            } else {
                Some(self.name.clone())
            },
            disk_source_type: disk_item.source_type,
            disk_name: disk_item.name.clone(),
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
            network_id,
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
    let modal_height = 20.min(area.height.saturating_sub(4));

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
            Constraint::Length(1), // [0] Padding
            Constraint::Length(2), // [1] Name
            Constraint::Length(4), // [2] Disk selector (expanded)
            Constraint::Length(2), // [3] VCPUs
            Constraint::Length(2), // [4] Memory
            Constraint::Length(2), // [5] Nested virt
            Constraint::Length(2), // [6] Network
            Constraint::Length(2), // [7] User-data
            Constraint::Length(2), // [8] Submit
        ])
        .split(inner);

    // Name field
    let name_focused = modal.focused_field == 0;
    let cursor = if name_focused { "\u{258c}" } else { "" };
    let name_line = Line::from(vec![
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
    ]);
    frame.render_widget(Paragraph::new(name_line), field_chunks[1]);

    // Disk selector
    let disk_focused = modal.focused_field == 1;
    let type_str = match modal.disk_source_type {
        DiskSourceType::Template => "Template",
        DiskSourceType::Volume => "Volume",
    };

    // Show selected disk with navigation hints
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
    frame.render_widget(Paragraph::new(disk_header), field_chunks[2]);

    // Show disk list (3 items visible with current selection in middle)
    let disk_list_area = Rect {
        x: field_chunks[2].x + 13,
        y: field_chunks[2].y + 1,
        width: field_chunks[2].width.saturating_sub(14),
        height: 3,
    };

    if modal.disk_items.is_empty() {
        let no_disks = Line::from(vec![Span::styled(
            "No templates or volumes available",
            Style::default().fg(Color::Red),
        )]);
        frame.render_widget(Paragraph::new(no_disks), disk_list_area);
    } else {
        // Filter items by current source type
        let filtered_items: Vec<(usize, &DiskItem)> = modal
            .disk_items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.source_type == modal.disk_source_type)
            .collect();

        // Find position of selected item in filtered list
        let selected_in_filtered = filtered_items
            .iter()
            .position(|(idx, _)| *idx == modal.selected_disk)
            .unwrap_or(0);

        // Show up to 3 items centered around selection
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

        // Pad if less than 3 items
        while lines.len() < 3 {
            lines.push(Line::from(""));
        }

        frame.render_widget(Paragraph::new(lines), disk_list_area);
    }

    // VCPUs
    let vcpus_focused = modal.focused_field == 2;
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
    frame.render_widget(Paragraph::new(vcpus_line), field_chunks[3]);

    // Memory
    let memory_focused = modal.focused_field == 3;
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
    frame.render_widget(Paragraph::new(memory_line), field_chunks[4]);

    // Nested Virt
    let nested_focused = modal.focused_field == 4;
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
    frame.render_widget(Paragraph::new(nested_line), field_chunks[5]);

    // Network selector
    let network_focused = modal.focused_field == 5;
    let network_display = match modal.selected_network {
        None => "None".to_string(),
        Some(idx) => modal
            .network_items
            .get(idx)
            .map(|n| n.name.clone())
            .unwrap_or_else(|| "Unknown".to_string()),
    };
    let network_line = Line::from(vec![
        Span::styled(
            " Network:    ",
            if network_focused {
                label_focused
            } else {
                label_normal
            },
        ),
        Span::styled(
            format!("[{}]", network_display),
            if network_focused {
                value_focused
            } else {
                value_normal
            },
        ),
        if network_focused {
            if modal.network_items.is_empty() {
                Span::styled(
                    " (no networks available)",
                    Style::default().fg(Color::DarkGray),
                )
            } else {
                Span::styled(" [↑↓: select]", Style::default().fg(Color::Yellow))
            }
        } else {
            Span::raw("")
        },
    ]);
    frame.render_widget(Paragraph::new(network_line), field_chunks[6]);

    // User-Data
    let user_data_focused = modal.focused_field == 6;
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
    frame.render_widget(Paragraph::new(user_data_line), field_chunks[7]);

    // Submit button
    let submit_style = if modal.focused_field == 7 {
        Style::default().fg(Color::Black).bg(Color::Green).bold()
    } else {
        Style::default().fg(Color::Green)
    };
    let submit = Paragraph::new(Line::from(vec![Span::styled(
        "  \u{25b6} Create VM  ",
        submit_style,
    )]))
    .alignment(Alignment::Center);
    frame.render_widget(submit, field_chunks[8]);
}
