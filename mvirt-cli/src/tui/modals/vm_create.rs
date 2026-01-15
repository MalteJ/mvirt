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
    pub volume_size_gb: String, // Size for new volume when cloning from template
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
            volume_size_gb: String::new(),
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
        9 // name, disk, volume_size, vcpus, memory, nested_virt, network, user_data, submit
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
            2 => Some(&mut self.volume_size_gb),
            3 => Some(&mut self.vcpus),
            4 => Some(&mut self.memory_mb),
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
        matches!(self.focused_field, 2..=4) // volume_size, vcpus, memory
    }

    pub fn is_nested_virt_field(&self) -> bool {
        self.focused_field == 5
    }

    pub fn toggle_nested_virt(&mut self) {
        self.nested_virt = !self.nested_virt;
    }

    pub fn is_network_field(&self) -> bool {
        self.focused_field == 6
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
        self.focused_field == 7
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

    pub fn is_submit_field(&self) -> bool {
        self.focused_field == 8
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
    let modal_height = 22.min(area.height.saturating_sub(4));

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
            Constraint::Length(2), // [3] Volume size (only for templates)
            Constraint::Length(2), // [4] VCPUs
            Constraint::Length(2), // [5] Memory
            Constraint::Length(2), // [6] Nested virt
            Constraint::Length(2), // [7] Network
            Constraint::Length(2), // [8] User-data
            Constraint::Length(2), // [9] Submit
        ])
        .split(inner);

    // Name field
    let name_focused = modal.focused_field == 0;
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

    // Volume Size (only shown when template is selected)
    let size_focused = modal.focused_field == 2;
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
        frame.render_widget(Paragraph::new(size_line), field_chunks[3]);
    } else {
        // Show placeholder when volume is selected (size is fixed)
        let size_line = Line::from(vec![
            Span::styled(" Vol Size:   ", label_normal),
            Span::styled(
                "(uses existing volume)",
                Style::default().fg(Color::DarkGray),
            ),
        ]);
        frame.render_widget(Paragraph::new(size_line), field_chunks[3]);
    }

    // VCPUs
    let vcpus_focused = modal.focused_field == 3;
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
    frame.render_widget(Paragraph::new(vcpus_line), field_chunks[4]);

    // Memory
    let memory_focused = modal.focused_field == 4;
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
    frame.render_widget(Paragraph::new(memory_line), field_chunks[5]);

    // Nested Virt
    let nested_focused = modal.focused_field == 5;
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
    frame.render_widget(Paragraph::new(nested_line), field_chunks[6]);

    // Network selector
    let network_focused = modal.focused_field == 6;
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
    frame.render_widget(Paragraph::new(network_line), field_chunks[7]);

    // User-Data
    let user_data_focused = modal.focused_field == 7;
    let user_data_line = match modal.user_data_mode {
        UserDataMode::None => Line::from(vec![
            Span::styled(
                " User-Data:  ",
                if user_data_focused {
                    label_focused
                } else {
                    label_normal
                },
            ),
            Span::styled("none", Style::default().fg(Color::DarkGray)),
            if user_data_focused {
                Span::styled(" [↑↓: select]", Style::default().fg(Color::Yellow))
            } else {
                Span::raw("")
            },
        ]),
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
                String::new()
            };
            let mut spans = vec![
                Span::styled(
                    " User-Data:  ",
                    if user_data_focused {
                        label_focused
                    } else {
                        label_normal
                    },
                ),
                Span::styled(
                    "SSH Keys",
                    if user_data_focused {
                        value_focused
                    } else {
                        value_normal
                    },
                ),
            ];
            if !value.is_empty() {
                spans.push(Span::styled(
                    format!(" {}", value),
                    Style::default().fg(Color::DarkGray),
                ));
            } else if user_data_focused {
                spans.push(Span::styled(
                    " not configured",
                    Style::default().fg(Color::DarkGray),
                ));
            }
            if user_data_focused {
                spans.push(Span::styled(
                    " [↑↓: select, Enter: configure]",
                    Style::default().fg(Color::Yellow),
                ));
            }
            Line::from(spans)
        }
        UserDataMode::File => {
            let mut spans = vec![
                Span::styled(
                    " User-Data:  ",
                    if user_data_focused {
                        label_focused
                    } else {
                        label_normal
                    },
                ),
                Span::styled(
                    "File",
                    if user_data_focused {
                        value_focused
                    } else {
                        value_normal
                    },
                ),
            ];
            if !modal.user_data_file.is_empty() {
                spans.push(Span::styled(
                    format!(" {}", modal.user_data_file),
                    Style::default().fg(Color::DarkGray),
                ));
            } else if user_data_focused {
                spans.push(Span::styled(
                    " not selected",
                    Style::default().fg(Color::DarkGray),
                ));
            }
            if user_data_focused {
                spans.push(Span::styled(
                    " [↑↓: select, Enter: browse]",
                    Style::default().fg(Color::Yellow),
                ));
            }
            Line::from(spans)
        }
    };
    frame.render_widget(Paragraph::new(user_data_line), field_chunks[8]);

    // Submit button
    let submit_style = if modal.focused_field == 8 {
        Style::default().fg(Color::Black).bg(Color::Green).bold()
    } else {
        Style::default().fg(Color::Green)
    };
    let submit = Paragraph::new(Line::from(vec![Span::styled(
        "  \u{25b6} Create VM  ",
        submit_style,
    )]))
    .alignment(Alignment::Center);
    frame.render_widget(submit, field_chunks[9]);
}
