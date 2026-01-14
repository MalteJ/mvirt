//! Modal for creating a new empty volume

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

#[derive(Clone, Copy, PartialEq, Default)]
pub enum SizeUnit {
    #[default]
    GB,
    TB,
}

#[derive(Default)]
pub struct VolumeCreateModal {
    pub name: String,
    pub size: String,
    pub size_unit: SizeUnit,
    pub focused_field: usize,
}

impl VolumeCreateModal {
    pub fn new() -> Self {
        Self {
            size: "10".to_string(),
            size_unit: SizeUnit::GB,
            ..Default::default()
        }
    }

    pub fn field_count() -> usize {
        4 // name, size, unit, submit
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
            1 => Some(&mut self.size),
            _ => None,
        }
    }

    pub fn is_name_field(&self) -> bool {
        self.focused_field == 0
    }

    pub fn is_size_field(&self) -> bool {
        self.focused_field == 1
    }

    pub fn is_unit_field(&self) -> bool {
        self.focused_field == 2
    }

    pub fn is_submit_field(&self) -> bool {
        self.focused_field == 3
    }

    pub fn toggle_unit(&mut self) {
        self.size_unit = match self.size_unit {
            SizeUnit::GB => SizeUnit::TB,
            SizeUnit::TB => SizeUnit::GB,
        };
    }

    pub fn validate(&self) -> Result<(String, u64), String> {
        if self.name.is_empty() {
            return Err("Name is required".to_string());
        }
        if !self
            .name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err("Name must be alphanumeric with - or _".to_string());
        }
        let size: u64 = self.size.parse().map_err(|_| "Invalid size".to_string())?;
        if size == 0 {
            return Err("Size must be > 0".to_string());
        }
        let size_bytes = match self.size_unit {
            SizeUnit::GB => size * 1024 * 1024 * 1024,
            SizeUnit::TB => size * 1024 * 1024 * 1024 * 1024,
        };
        Ok((self.name.clone(), size_bytes))
    }
}

pub fn draw(frame: &mut Frame, modal: &VolumeCreateModal) {
    let area = centered_rect(50, 12, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Create Volume ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    frame.render_widget(block.clone(), area);

    let inner = block.inner(area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(2), // Name
            Constraint::Length(2), // Size + Unit
            Constraint::Length(2), // Submit
        ])
        .split(inner);

    // Name field
    let name_style = if modal.focused_field == 0 {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::White)
    };
    let name_line = Line::from(vec![
        Span::styled(" Name: ", Style::default().fg(Color::Cyan)),
        Span::styled(&modal.name, name_style),
        if modal.focused_field == 0 {
            Span::styled("_", Style::default().fg(Color::Yellow))
        } else {
            Span::raw("")
        },
    ]);
    frame.render_widget(Paragraph::new(name_line), chunks[0]);

    // Size field
    let size_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(20), Constraint::Min(10)])
        .split(chunks[1]);

    let size_style = if modal.focused_field == 1 {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::White)
    };
    let size_line = Line::from(vec![
        Span::styled(" Size: ", Style::default().fg(Color::Cyan)),
        Span::styled(&modal.size, size_style),
        if modal.focused_field == 1 {
            Span::styled("_", Style::default().fg(Color::Yellow))
        } else {
            Span::raw("")
        },
    ]);
    frame.render_widget(Paragraph::new(size_line), size_chunks[0]);

    let unit_style = if modal.focused_field == 2 {
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
    let submit_style = if modal.focused_field == 3 {
        Style::default().fg(Color::Black).bg(Color::Cyan)
    } else {
        Style::default().fg(Color::Cyan)
    };
    frame.render_widget(
        Paragraph::new(Span::styled(" [ Create ] ", submit_style)).alignment(Alignment::Center),
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
