//! Modal for creating a snapshot of a volume

use chrono::Local;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

#[derive(Default)]
pub struct VolumeSnapshotModal {
    pub volume_name: String,
    pub snapshot_name: String,
    pub focused_field: usize,
}

impl VolumeSnapshotModal {
    pub fn new(volume_name: String) -> Self {
        let timestamp = Local::now().format("%Y%m%d-%H%M%S");
        let snapshot_name = format!("{}-snap-{}", volume_name, timestamp);
        Self {
            volume_name,
            snapshot_name,
            focused_field: 1, // Start on submit button
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
            0 => Some(&mut self.snapshot_name),
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
        if self.snapshot_name.is_empty() {
            return Err("Snapshot name is required".to_string());
        }
        if !self
            .snapshot_name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err("Name must be alphanumeric with - or _".to_string());
        }
        Ok(self.snapshot_name.clone())
    }
}

pub fn draw(frame: &mut Frame, modal: &VolumeSnapshotModal) {
    let area = centered_rect(50, 10, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(format!(" Snapshot: {} ", modal.volume_name))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    frame.render_widget(block.clone(), area);

    let inner = block.inner(area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(2), // Name
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
        Span::styled(" Snapshot Name: ", Style::default().fg(Color::Cyan)),
        Span::styled(&modal.snapshot_name, name_style),
        if modal.focused_field == 0 {
            Span::styled("_", Style::default().fg(Color::Yellow))
        } else {
            Span::raw("")
        },
    ]);
    frame.render_widget(Paragraph::new(name_line), chunks[0]);

    // Submit button
    let submit_style = if modal.focused_field == 1 {
        Style::default().fg(Color::Black).bg(Color::Cyan)
    } else {
        Style::default().fg(Color::Cyan)
    };
    frame.render_widget(
        Paragraph::new(Span::styled(" [ Create Snapshot ] ", submit_style))
            .alignment(Alignment::Center),
        chunks[1],
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
