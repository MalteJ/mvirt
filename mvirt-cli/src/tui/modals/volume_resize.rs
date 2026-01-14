//! Modal for resizing a volume

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

#[derive(Clone, Copy, PartialEq, Default)]
pub enum SizeUnit {
    #[default]
    GB,
    TB,
}

#[derive(Default)]
pub struct VolumeResizeModal {
    pub volume_name: String,
    pub current_size_bytes: u64,
    pub new_size: String,
    pub size_unit: SizeUnit,
    pub focused_field: usize,
}

impl VolumeResizeModal {
    pub fn new(volume_name: String, current_size_bytes: u64) -> Self {
        // Default to current size in GB
        let current_gb = current_size_bytes / (1024 * 1024 * 1024);
        Self {
            volume_name,
            current_size_bytes,
            new_size: current_gb.to_string(),
            size_unit: SizeUnit::GB,
            focused_field: 0,
        }
    }

    pub fn field_count() -> usize {
        3 // size, unit, submit
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
            0 => Some(&mut self.new_size),
            _ => None,
        }
    }

    pub fn is_size_field(&self) -> bool {
        self.focused_field == 0
    }

    pub fn is_unit_field(&self) -> bool {
        self.focused_field == 1
    }

    pub fn is_submit_field(&self) -> bool {
        self.focused_field == 2
    }

    pub fn toggle_unit(&mut self) {
        self.size_unit = match self.size_unit {
            SizeUnit::GB => SizeUnit::TB,
            SizeUnit::TB => SizeUnit::GB,
        };
    }

    pub fn validate(&self) -> Result<u64, String> {
        let size: u64 = self
            .new_size
            .parse()
            .map_err(|_| "Invalid size".to_string())?;
        if size == 0 {
            return Err("Size must be > 0".to_string());
        }
        let size_bytes = match self.size_unit {
            SizeUnit::GB => size * 1024 * 1024 * 1024,
            SizeUnit::TB => size * 1024 * 1024 * 1024 * 1024,
        };
        if size_bytes <= self.current_size_bytes {
            return Err("New size must be larger than current size".to_string());
        }
        Ok(size_bytes)
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

pub fn draw(frame: &mut Frame, modal: &VolumeResizeModal) {
    let area = centered_rect(50, 12, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(format!(" Resize Volume: {} ", modal.volume_name))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    frame.render_widget(block.clone(), area);

    let inner = block.inner(area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(2), // Current size info
            Constraint::Length(2), // New size + unit
            Constraint::Length(2), // Submit
        ])
        .split(inner);

    // Current size info
    let current_line = Line::from(vec![
        Span::styled(" Current: ", Style::default().fg(Color::Cyan)),
        Span::styled(
            format_size(modal.current_size_bytes),
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    frame.render_widget(Paragraph::new(current_line), chunks[0]);

    // New size field
    let size_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(22), Constraint::Min(10)])
        .split(chunks[1]);

    let size_style = if modal.focused_field == 0 {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::White)
    };
    let size_line = Line::from(vec![
        Span::styled(" New Size: ", Style::default().fg(Color::Cyan)),
        Span::styled(&modal.new_size, size_style),
        if modal.focused_field == 0 {
            Span::styled("_", Style::default().fg(Color::Yellow))
        } else {
            Span::raw("")
        },
    ]);
    frame.render_widget(Paragraph::new(size_line), size_chunks[0]);

    let unit_style = if modal.focused_field == 1 {
        Style::default().fg(Color::Yellow).bold()
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let unit_str = match modal.size_unit {
        SizeUnit::GB => "[GB] TB",
        SizeUnit::TB => "GB [TB]",
    };
    frame.render_widget(
        Paragraph::new(Span::styled(unit_str, unit_style)),
        size_chunks[1],
    );

    // Submit button
    let submit_style = if modal.focused_field == 2 {
        Style::default().fg(Color::Black).bg(Color::Cyan)
    } else {
        Style::default().fg(Color::Cyan)
    };
    frame.render_widget(
        Paragraph::new(Span::styled(" [ Resize ] ", submit_style)).alignment(Alignment::Center),
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
