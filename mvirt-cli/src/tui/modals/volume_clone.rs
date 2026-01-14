//! Modal for cloning a volume from a template

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

#[derive(Default)]
pub struct VolumeCloneModal {
    pub template_name: String,
    pub template_size_bytes: u64,
    pub new_volume_name: String,
    pub size_input: String,
    pub focused_field: usize,
}

impl VolumeCloneModal {
    pub fn new(template_name: String, template_size_bytes: u64) -> Self {
        Self {
            template_name,
            template_size_bytes,
            new_volume_name: String::new(),
            size_input: String::new(), // Empty = use template size
            focused_field: 0,
        }
    }

    pub fn field_count() -> usize {
        3 // name, size, submit
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
            1 => Some(&mut self.size_input),
            _ => None,
        }
    }

    pub fn is_name_field(&self) -> bool {
        self.focused_field == 0
    }

    pub fn is_submit_field(&self) -> bool {
        self.focused_field == 2
    }

    /// Validate and return (name, size_bytes)
    /// size_bytes is None if empty (use template size) or Some(parsed_size)
    pub fn validate(&self) -> Result<(String, Option<u64>), String> {
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

        let size_bytes = if self.size_input.is_empty() {
            None
        } else {
            let size = parse_size(&self.size_input)?;
            if size < self.template_size_bytes {
                return Err(format!(
                    "Size must be >= template size ({})",
                    format_size(self.template_size_bytes)
                ));
            }
            Some(size)
        };

        Ok((self.new_volume_name.clone(), size_bytes))
    }
}

/// Parse size string like "10G", "1T", "500M" to bytes
fn parse_size(s: &str) -> Result<u64, String> {
    let s = s.trim().to_uppercase();
    if s.is_empty() {
        return Err("Size cannot be empty".to_string());
    }

    let (num_str, multiplier) = if s.ends_with('T') {
        (&s[..s.len() - 1], 1024u64 * 1024 * 1024 * 1024)
    } else if s.ends_with('G') {
        (&s[..s.len() - 1], 1024u64 * 1024 * 1024)
    } else if s.ends_with('M') {
        (&s[..s.len() - 1], 1024u64 * 1024)
    } else {
        // Assume bytes if no suffix
        (s.as_str(), 1u64)
    };

    let num: f64 = num_str
        .parse()
        .map_err(|_| format!("Invalid number: {}", num_str))?;

    Ok((num * multiplier as f64) as u64)
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
    let area = centered_rect(50, 14, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(format!(" New Volume from: {} ", modal.template_name))
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
            Constraint::Length(2), // Size
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
        Span::styled(" Volume Name: ", Style::default().fg(Color::Cyan)),
        Span::styled(&modal.new_volume_name, name_style),
        if modal.focused_field == 0 {
            Span::styled("_", Style::default().fg(Color::Yellow))
        } else {
            Span::raw("")
        },
    ]);
    frame.render_widget(Paragraph::new(name_line), chunks[1]);

    // Size field
    let size_style = if modal.focused_field == 1 {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::White)
    };
    let size_display = if modal.size_input.is_empty() {
        format_size(modal.template_size_bytes)
    } else {
        modal.size_input.clone()
    };
    let size_line = Line::from(vec![
        Span::styled(" Size: ", Style::default().fg(Color::Cyan)),
        Span::styled(&size_display, size_style),
        if modal.focused_field == 1 {
            Span::styled("_", Style::default().fg(Color::Yellow))
        } else {
            Span::raw("")
        },
        Span::styled(" (e.g. 10G, 1T)", Style::default().fg(Color::DarkGray)),
    ]);
    frame.render_widget(Paragraph::new(size_line), chunks[2]);

    // Submit button
    let submit_style = if modal.focused_field == 2 {
        Style::default().fg(Color::Black).bg(Color::Cyan)
    } else {
        Style::default().fg(Color::Cyan)
    };
    frame.render_widget(
        Paragraph::new(Span::styled(" [ Create Volume ] ", submit_style))
            .alignment(Alignment::Center),
        chunks[3],
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
