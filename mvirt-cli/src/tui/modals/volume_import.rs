//! Modal for importing a volume from file or URL

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

#[derive(Default)]
pub struct VolumeImportModal {
    pub name: String,
    pub source: String, // URL to import from
    pub size: String,   // Optional, in GB
    pub focused_field: usize,
}

impl VolumeImportModal {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn field_count() -> usize {
        4 // name, source, size, submit
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
            1 => Some(&mut self.source),
            2 => Some(&mut self.size),
            _ => None,
        }
    }

    pub fn is_name_field(&self) -> bool {
        self.focused_field == 0
    }

    #[allow(dead_code)]
    pub fn is_source_field(&self) -> bool {
        self.focused_field == 1
    }

    pub fn is_size_field(&self) -> bool {
        self.focused_field == 2
    }

    pub fn is_submit_field(&self) -> bool {
        self.focused_field == 3
    }

    pub fn validate(&self) -> Result<(String, String, Option<u64>), String> {
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
        if self.source.is_empty() {
            return Err("Source is required".to_string());
        }

        let size_bytes = if self.size.is_empty() {
            None
        } else {
            let size: u64 = self.size.parse().map_err(|_| "Invalid size".to_string())?;
            Some(size * 1024 * 1024 * 1024) // GB to bytes
        };

        Ok((self.name.clone(), self.source.clone(), size_bytes))
    }
}

pub fn draw(frame: &mut Frame, modal: &VolumeImportModal) {
    let area = centered_rect(60, 12, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Import Volume ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    frame.render_widget(block.clone(), area);

    let inner = block.inner(area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(2), // Name
            Constraint::Length(2), // Source
            Constraint::Length(2), // Size
            Constraint::Length(2), // Submit
        ])
        .split(inner);

    // Name field
    let name_style = field_style(modal.focused_field == 0);
    let name_line = Line::from(vec![
        Span::styled(" Name: ", Style::default().fg(Color::Cyan)),
        Span::styled(&modal.name, name_style),
        cursor_span(modal.focused_field == 0),
    ]);
    frame.render_widget(Paragraph::new(name_line), chunks[0]);

    // URL field
    let source_style = field_style(modal.focused_field == 1);
    let source_line = Line::from(vec![
        Span::styled(" URL: ", Style::default().fg(Color::Cyan)),
        Span::styled(&modal.source, source_style),
        cursor_span(modal.focused_field == 1),
    ]);
    frame.render_widget(Paragraph::new(source_line), chunks[1]);

    // Size field (optional)
    let size_style = field_style(modal.focused_field == 2);
    let size_line = Line::from(vec![
        Span::styled(" Size (GB, optional): ", Style::default().fg(Color::Cyan)),
        Span::styled(&modal.size, size_style),
        cursor_span(modal.focused_field == 2),
    ]);
    frame.render_widget(Paragraph::new(size_line), chunks[2]);

    // Submit button
    let submit_style = if modal.focused_field == 3 {
        Style::default().fg(Color::Black).bg(Color::Cyan)
    } else {
        Style::default().fg(Color::Cyan)
    };
    frame.render_widget(
        Paragraph::new(Span::styled(" [ Import ] ", submit_style)).alignment(Alignment::Center),
        chunks[3],
    );
}

fn field_style(focused: bool) -> Style {
    if focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::White)
    }
}

fn cursor_span(show: bool) -> Span<'static> {
    if show {
        Span::styled("_", Style::default().fg(Color::Yellow))
    } else {
        Span::raw("")
    }
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
