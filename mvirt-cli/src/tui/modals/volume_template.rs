//! Modal for promoting a snapshot to a template

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

#[derive(Default)]
pub struct VolumeTemplateModal {
    pub volume_name: String,
    pub snapshot_name: String,
    pub template_name: String,
    pub focused_field: usize,
}

impl VolumeTemplateModal {
    #[allow(dead_code)]
    pub fn new(volume_name: String, snapshot_name: String) -> Self {
        // Default template name based on snapshot name
        let template_name = format!("{}-template", snapshot_name);
        Self {
            volume_name,
            snapshot_name,
            template_name,
            focused_field: 0,
        }
    }

    /// Create modal with snapshot name to be entered by user
    pub fn new_for_volume(volume_name: String) -> Self {
        Self {
            volume_name,
            snapshot_name: String::new(),
            template_name: String::new(),
            focused_field: 0,
        }
    }

    pub fn field_count() -> usize {
        3 // snapshot name, template name, submit
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
            1 => Some(&mut self.template_name),
            _ => None,
        }
    }

    #[allow(dead_code)]
    pub fn is_snapshot_field(&self) -> bool {
        self.focused_field == 0
    }

    pub fn is_name_field(&self) -> bool {
        self.focused_field == 1
    }

    pub fn is_submit_field(&self) -> bool {
        self.focused_field == 2
    }

    pub fn validate(&self) -> Result<(String, String), String> {
        if self.snapshot_name.is_empty() {
            return Err("Snapshot name is required".to_string());
        }
        if self.template_name.is_empty() {
            return Err("Template name is required".to_string());
        }
        if !self
            .template_name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err("Name must be alphanumeric with - or _".to_string());
        }
        Ok((self.snapshot_name.clone(), self.template_name.clone()))
    }
}

pub fn draw(frame: &mut Frame, modal: &VolumeTemplateModal) {
    let area = centered_rect(55, 14, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(format!(
            " Promote Snapshot to Template: {} ",
            modal.volume_name
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    frame.render_widget(block.clone(), area);

    let inner = block.inner(area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(2), // Snapshot name
            Constraint::Length(2), // Template name
            Constraint::Length(2), // Submit
        ])
        .split(inner);

    // Snapshot name field
    let snap_style = if modal.focused_field == 0 {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::White)
    };
    let snap_line = Line::from(vec![
        Span::styled(" Snapshot Name: ", Style::default().fg(Color::Cyan)),
        Span::styled(&modal.snapshot_name, snap_style),
        if modal.focused_field == 0 {
            Span::styled("_", Style::default().fg(Color::Yellow))
        } else {
            Span::raw("")
        },
    ]);
    frame.render_widget(Paragraph::new(snap_line), chunks[0]);

    // Template name field
    let name_style = if modal.focused_field == 1 {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::White)
    };
    let name_line = Line::from(vec![
        Span::styled(" Template Name: ", Style::default().fg(Color::Cyan)),
        Span::styled(&modal.template_name, name_style),
        if modal.focused_field == 1 {
            Span::styled("_", Style::default().fg(Color::Yellow))
        } else {
            Span::raw("")
        },
    ]);
    frame.render_widget(Paragraph::new(name_line), chunks[1]);

    // Submit button
    let submit_style = if modal.focused_field == 2 {
        Style::default().fg(Color::Black).bg(Color::Cyan)
    } else {
        Style::default().fg(Color::Cyan)
    };
    frame.render_widget(
        Paragraph::new(Span::styled(" [ Promote to Template ] ", submit_style))
            .alignment(Alignment::Center),
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
