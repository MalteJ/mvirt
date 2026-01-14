//! Modal for creating a new network

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

#[derive(Default)]
pub struct NetworkCreateModal {
    pub name: String,
    pub ipv4_subnet: String,
    pub ipv6_prefix: String,
    pub focused_field: usize,
}

impl NetworkCreateModal {
    pub fn new() -> Self {
        Self {
            ipv4_subnet: "10.0.0.0/24".to_string(),
            ipv6_prefix: String::new(),
            ..Default::default()
        }
    }

    pub fn field_count() -> usize {
        4 // name, ipv4_subnet, ipv6_prefix, submit
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
            1 => Some(&mut self.ipv4_subnet),
            2 => Some(&mut self.ipv6_prefix),
            _ => None,
        }
    }

    pub fn is_name_field(&self) -> bool {
        self.focused_field == 0
    }

    pub fn is_submit_field(&self) -> bool {
        self.focused_field == 3
    }

    /// Returns (name, ipv4_subnet or None, ipv6_prefix or None)
    pub fn validate(&self) -> Result<(String, Option<String>, Option<String>), String> {
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

        let ipv4 = if self.ipv4_subnet.is_empty() {
            None
        } else {
            // Basic CIDR validation
            if !self.ipv4_subnet.contains('/') {
                return Err("IPv4 subnet must be in CIDR format (e.g., 10.0.0.0/24)".to_string());
            }
            Some(self.ipv4_subnet.clone())
        };

        let ipv6 = if self.ipv6_prefix.is_empty() {
            None
        } else {
            // Basic CIDR validation
            if !self.ipv6_prefix.contains('/') {
                return Err("IPv6 prefix must be in CIDR format (e.g., fd00::/64)".to_string());
            }
            Some(self.ipv6_prefix.clone())
        };

        if ipv4.is_none() && ipv6.is_none() {
            return Err("At least one of IPv4 or IPv6 must be configured".to_string());
        }

        Ok((self.name.clone(), ipv4, ipv6))
    }
}

pub fn draw(frame: &mut Frame, modal: &NetworkCreateModal) {
    let area = centered_rect(60, 14, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Create Network ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    frame.render_widget(block.clone(), area);

    let inner = block.inner(area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(2), // Name
            Constraint::Length(2), // IPv4 Subnet
            Constraint::Length(2), // IPv6 Prefix
            Constraint::Length(1), // Spacing
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

    // IPv4 Subnet field
    let ipv4_style = if modal.focused_field == 1 {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::White)
    };
    let ipv4_line = Line::from(vec![
        Span::styled(" IPv4 Subnet: ", Style::default().fg(Color::Cyan)),
        Span::styled(&modal.ipv4_subnet, ipv4_style),
        if modal.focused_field == 1 {
            Span::styled("_", Style::default().fg(Color::Yellow))
        } else {
            Span::raw("")
        },
    ]);
    frame.render_widget(Paragraph::new(ipv4_line), chunks[1]);

    // IPv6 Prefix field
    let ipv6_style = if modal.focused_field == 2 {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::White)
    };
    let ipv6_line = Line::from(vec![
        Span::styled(" IPv6 Prefix: ", Style::default().fg(Color::Cyan)),
        Span::styled(&modal.ipv6_prefix, ipv6_style),
        if modal.focused_field == 2 {
            Span::styled("_", Style::default().fg(Color::Yellow))
        } else {
            Span::raw("")
        },
        if modal.ipv6_prefix.is_empty() && modal.focused_field != 2 {
            Span::styled(" (optional)", Style::default().fg(Color::DarkGray))
        } else {
            Span::raw("")
        },
    ]);
    frame.render_widget(Paragraph::new(ipv6_line), chunks[2]);

    // Submit button
    let submit_style = if modal.focused_field == 3 {
        Style::default().fg(Color::Black).bg(Color::Cyan)
    } else {
        Style::default().fg(Color::Cyan)
    };
    frame.render_widget(
        Paragraph::new(Span::styled(" [ Create ] ", submit_style)).alignment(Alignment::Center),
        chunks[4],
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
