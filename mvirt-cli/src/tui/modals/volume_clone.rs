//! Modal for cloning a volume from a template

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

#[derive(Default)]
pub struct VolumeCloneModal {
    pub template_name: String,
    pub template_size_bytes: u64,
    pub new_volume_name: String,
    pub focused_field: usize,
}

impl VolumeCloneModal {
    pub fn new(template_name: String, template_size_bytes: u64) -> Self {
        Self {
            template_name,
            template_size_bytes,
            new_volume_name: String::new(),
            focused_field: 0,
        }
    }

    pub fn field_count() -> usize {
        2 // name, submit
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
            0 => Some(&mut self.new_volume_name),
            _ => None,
        }
    }

    pub fn is_name_field(&self) -> bool {
        self.focused_field == 0
    }

    pub fn is_submit_field(&self) -> bool {
        self.focused_field == 1
    }

    pub fn validate(&self) -> Result<String, String> {
        if self.new_volume_name.is_empty() {
            return Err("Volume name is required".to_string());
        }
        if !self
            .new_volume_name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err("Name must be alphanumeric with - or _".to_string());
        }
        Ok(self.new_volume_name.clone())
    }
}

fn format_size(bytes: u64) -> String {
    const GB: u64 = 1024 * 1024 * 1024;
    const TB: u64 = GB * 1024;

    if bytes >= TB {
        format!("{:.1} TB", bytes as f64 / TB as f64)
    } else {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    }
}

pub fn draw(frame: &mut Frame, modal: &VolumeCloneModal) {
    let area = centered_rect(50, 12, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(format!(" Clone from: {} ", modal.template_name))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    frame.render_widget(block.clone(), area);

    let inner = block.inner(area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(2), // Template info
            Constraint::Length(2), // Name
            Constraint::Length(2), // Submit
        ])
        .split(inner);

    // Template info
    let info_line = Line::from(vec![
        Span::styled(" Template Size: ", Style::default().fg(Color::Cyan)),
        Span::styled(
            format_size(modal.template_size_bytes),
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    frame.render_widget(Paragraph::new(info_line), chunks[0]);

    // Name field
    let name_style = if modal.focused_field == 0 {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::White)
    };
    let name_line = Line::from(vec![
        Span::styled(" New Volume Name: ", Style::default().fg(Color::Cyan)),
        Span::styled(&modal.new_volume_name, name_style),
        if modal.focused_field == 0 {
            Span::styled("_", Style::default().fg(Color::Yellow))
        } else {
            Span::raw("")
        },
    ]);
    frame.render_widget(Paragraph::new(name_line), chunks[1]);

    // Submit button
    let submit_style = if modal.focused_field == 1 {
        Style::default().fg(Color::Black).bg(Color::Cyan)
    } else {
        Style::default().fg(Color::Cyan)
    };
    frame.render_widget(
        Paragraph::new(Span::styled(" [ Clone ] ", submit_style)).alignment(Alignment::Center),
        chunks[2],
    );
}

fn centered_rect(percent_x: u16, height: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - height.min(100)) / 2),
            Constraint::Length(height),
            Constraint::Percentage((100 - height.min(100)) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
