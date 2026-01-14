use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::tui::types::{SshKeySource, SshKeysConfig};

pub struct SshKeysModal {
    pub config: SshKeysConfig,
    pub focused_field: usize, // 0=username, 1=source, 2=github/path, 3=root_password, 4=add, 5=cancel
}

impl SshKeysModal {
    pub fn new() -> Self {
        Self {
            config: SshKeysConfig::new(),
            focused_field: 0,
        }
    }

    pub fn field_count(&self) -> usize {
        6
    }

    pub fn focus_next(&mut self) {
        self.focused_field = (self.focused_field + 1) % self.field_count();
    }

    pub fn focus_prev(&mut self) {
        self.focused_field = if self.focused_field == 0 {
            self.field_count() - 1
        } else {
            self.focused_field - 1
        };
    }

    pub fn toggle_source(&mut self) {
        self.config.source = match self.config.source {
            SshKeySource::GitHub => SshKeySource::Local,
            SshKeySource::Local => SshKeySource::GitHub,
        };
    }

    pub fn current_input(&mut self) -> Option<&mut String> {
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

    pub fn is_source_field(&self) -> bool {
        self.focused_field == 1
    }

    pub fn is_add_button(&self) -> bool {
        self.focused_field == 4
    }

    pub fn is_cancel_button(&self) -> bool {
        self.focused_field == 5
    }

    pub fn validate(&self) -> Result<(), &'static str> {
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

pub fn draw(frame: &mut Frame, modal: &SshKeysModal) {
    let area = frame.area();
    let modal_width = 60.min(area.width.saturating_sub(6));
    let modal_height = 15.min(area.height.saturating_sub(6));

    let modal_area = Rect {
        x: (area.width - modal_width) / 2,
        y: (area.height - modal_height) / 2,
        width: modal_width,
        height: modal_height,
    };

    frame.render_widget(Clear, modal_area);

    let title = Line::from(vec![
        Span::styled(" SSH Keys ", Style::default().fg(Color::Cyan).bold()),
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
            Constraint::Length(1), // Top padding
            Constraint::Length(2), // Username
            Constraint::Length(2), // Source
            Constraint::Length(2), // GitHub user / Local path
            Constraint::Length(2), // Root password
            Constraint::Length(1), // Spacer
            Constraint::Length(2), // Buttons
        ])
        .split(inner);

    // Username field
    let username_focused = modal.focused_field == 0;
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
    frame.render_widget(Paragraph::new(source_line), field_chunks[2]);

    // GitHub username or Local path
    let value_focused_field = modal.focused_field == 2;
    let cursor = if value_focused_field { "\u{258c}" } else { "" };
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
    let cursor = if password_focused { "\u{258c}" } else { "" };
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
        Paragraph::new(Span::styled("  \u{25b6} Add  ", add_style))
            .alignment(ratatui::prelude::Alignment::Center),
        button_chunks[0],
    );
    frame.render_widget(
        Paragraph::new(Span::styled("  \u{2715} Cancel  ", cancel_style))
            .alignment(ratatui::prelude::Alignment::Center),
        button_chunks[1],
    );
}
