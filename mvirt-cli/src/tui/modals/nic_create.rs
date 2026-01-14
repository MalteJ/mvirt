//! Modal for creating a new NIC

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

#[derive(Default)]
pub struct NicCreateModal {
    pub network_id: String,
    pub network_name: String,
    pub name: String,
    pub focused_field: usize,
}

impl NicCreateModal {
    pub fn new(network_id: String, network_name: String) -> Self {
        Self {
            network_id,
            network_name,
            name: String::new(),
            focused_field: 0,
        }
    }

    pub fn field_count() -> usize {
        2 // name, submit (network is readonly)
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
            _ => None,
        }
    }

    pub fn is_name_field(&self) -> bool {
        self.focused_field == 0
    }

    pub fn is_submit_field(&self) -> bool {
        self.focused_field == 1
    }

    /// Returns (network_id, name or None)
    pub fn validate(&self) -> Result<(String, Option<String>), String> {
        if self.network_id.is_empty() {
            return Err("Network ID is required".to_string());
        }

        let name = if self.name.is_empty() {
            None
        } else {
            if !self
                .name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                return Err("Name must be alphanumeric with - or _".to_string());
            }
            Some(self.name.clone())
        };

        Ok((self.network_id.clone(), name))
    }
}

pub fn draw(frame: &mut Frame, modal: &NicCreateModal) {
    let area = centered_rect(50, 12, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Create NIC ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    frame.render_widget(block.clone(), area);

    let inner = block.inner(area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(2), // Network (readonly)
            Constraint::Length(2), // Name
            Constraint::Length(1), // Spacing
            Constraint::Length(2), // Submit
        ])
        .split(inner);

    // Network field (readonly)
    let network_line = Line::from(vec![
        Span::styled(" Network: ", Style::default().fg(Color::Cyan)),
        Span::styled(&modal.network_name, Style::default().fg(Color::DarkGray)),
    ]);
    frame.render_widget(Paragraph::new(network_line), chunks[0]);

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
        if modal.name.is_empty() && modal.focused_field != 0 {
            Span::styled(" (optional)", Style::default().fg(Color::DarkGray))
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
        Paragraph::new(Span::styled(" [ Create ] ", submit_style)).alignment(Alignment::Center),
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
