use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::tui::types::{
    CreateBootMode, CreateVmParams, SshKeySource, SshKeysConfig, UserDataMode,
};

#[derive(Default)]
pub struct CreateModal {
    pub name: String,
    pub boot_mode: CreateBootMode,
    pub kernel: String,
    pub initramfs: String,
    pub cmdline: String,
    pub disk: String,
    pub vcpus: String,
    pub memory_mb: String,
    pub nested_virt: bool,
    pub user_data_mode: UserDataMode,
    pub user_data_file: String,
    pub ssh_keys_config: Option<SshKeysConfig>,
    pub focused_field: usize,
}

impl CreateModal {
    pub fn new() -> Self {
        Self {
            vcpus: "1".to_string(),
            memory_mb: "512".to_string(),
            boot_mode: CreateBootMode::Disk,
            user_data_mode: UserDataMode::None,
            ..Default::default()
        }
    }

    pub fn field_count() -> usize {
        11
    }

    pub fn focus_next(&mut self) {
        loop {
            self.focused_field = (self.focused_field + 1) % Self::field_count();
            if self.is_field_visible(self.focused_field) {
                break;
            }
        }
    }

    pub fn focus_prev(&mut self) {
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

    pub fn is_field_visible(&self, field: usize) -> bool {
        match field {
            2..=4 => self.boot_mode == CreateBootMode::Kernel,
            _ => true,
        }
    }

    pub fn current_input(&mut self) -> Option<&mut String> {
        match self.focused_field {
            0 => Some(&mut self.name),
            2 => Some(&mut self.kernel),
            3 => Some(&mut self.initramfs),
            4 => Some(&mut self.cmdline),
            5 => Some(&mut self.disk),
            6 => Some(&mut self.vcpus),
            7 => Some(&mut self.memory_mb),
            _ => None,
        }
    }

    pub fn is_name_field(&self) -> bool {
        self.focused_field == 0
    }

    pub fn is_boot_mode_field(&self) -> bool {
        self.focused_field == 1
    }

    pub fn is_file_field(&self) -> bool {
        match self.focused_field {
            2 | 3 => self.boot_mode == CreateBootMode::Kernel,
            5 => true,
            _ => false,
        }
    }

    pub fn is_nested_virt_field(&self) -> bool {
        self.focused_field == 8
    }

    pub fn toggle_nested_virt(&mut self) {
        self.nested_virt = !self.nested_virt;
    }

    pub fn is_user_data_mode_field(&self) -> bool {
        self.focused_field == 9
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

    pub fn is_numeric_field(&self) -> bool {
        matches!(self.focused_field, 6 | 7)
    }

    pub fn is_valid_name_char(c: char) -> bool {
        c.is_ascii_alphanumeric() || c == '-' || c == '_'
    }

    pub fn set_field(&mut self, field: usize, value: String) {
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

    pub fn set_user_data_file(&mut self, path: String) {
        self.user_data_file = path;
    }

    pub fn set_ssh_keys_config(&mut self, config: SshKeysConfig) {
        self.ssh_keys_config = Some(config);
    }

    pub fn toggle_boot_mode(&mut self) {
        self.boot_mode = match self.boot_mode {
            CreateBootMode::Disk => CreateBootMode::Kernel,
            CreateBootMode::Kernel => CreateBootMode::Disk,
        };
    }

    pub fn validate(&self) -> Result<CreateVmParams, &'static str> {
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

pub fn draw(frame: &mut Frame, modal: &CreateModal) {
    let area = frame.area();
    let modal_width = 70.min(area.width.saturating_sub(4));

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

    let constraints: Vec<Constraint> = if modal.boot_mode == CreateBootMode::Kernel {
        vec![
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(2),
        ]
    } else {
        vec![
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(2),
        ]
    };

    let field_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    let render_field =
        |frame: &mut Frame, area: Rect, label: &str, value: &str, focused: bool, hint: &str| {
            let cursor = if focused { "\u{258c}" } else { "" };
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

    let mut row = 1;

    render_field(
        frame,
        field_chunks[row],
        "Name:",
        &modal.name,
        modal.focused_field == 0,
        "",
    );
    row += 1;

    let boot_str = match modal.boot_mode {
        CreateBootMode::Disk => "(\u{25cf}) Disk  ( ) Kernel",
        CreateBootMode::Kernel => "( ) Disk  (\u{25cf}) Kernel",
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

    let submit_style = if modal.focused_field == 10 {
        Style::default().fg(Color::Black).bg(Color::Green).bold()
    } else {
        Style::default().fg(Color::Green)
    };
    let submit = Paragraph::new(Line::from(vec![Span::styled(
        "  \u{25b6} Create VM  ",
        submit_style,
    )]))
    .alignment(ratatui::prelude::Alignment::Center);
    frame.render_widget(submit, field_chunks[row]);
}
