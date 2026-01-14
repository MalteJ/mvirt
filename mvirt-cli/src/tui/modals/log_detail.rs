//! Modal for displaying full log entry details

use chrono::{DateTime, Local};
use mvirt_log::{LogEntry, LogLevel};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

/// Format nanosecond timestamp to human-readable time
fn format_timestamp(timestamp_ns: i64) -> String {
    let secs = timestamp_ns / 1_000_000_000;
    let nanos = (timestamp_ns % 1_000_000_000) as u32;
    if let Some(dt) = DateTime::from_timestamp(secs, nanos) {
        let local: DateTime<Local> = dt.into();
        local.format("%Y-%m-%d %H:%M:%S%.3f").to_string()
    } else {
        "Invalid time".to_string()
    }
}

/// Get display string and color for log level
fn level_style(level: i32) -> (&'static str, Style) {
    match LogLevel::try_from(level) {
        Ok(LogLevel::Info) => ("INFO", Style::default().fg(Color::Green).bold()),
        Ok(LogLevel::Warn) => ("WARN", Style::default().fg(Color::Yellow).bold()),
        Ok(LogLevel::Error) => ("ERROR", Style::default().fg(Color::Red).bold()),
        Ok(LogLevel::Debug) => ("DEBUG", Style::default().fg(Color::DarkGray).bold()),
        Ok(LogLevel::Audit) => ("AUDIT", Style::default().fg(Color::Cyan).bold()),
        Err(_) => ("???", Style::default().fg(Color::White)),
    }
}

pub fn draw(frame: &mut Frame, entry: &LogEntry) {
    let area = frame.area();
    let modal_width = 80.min(area.width.saturating_sub(4));
    let modal_height = 20.min(area.height.saturating_sub(4));

    let modal_area = Rect {
        x: (area.width - modal_width) / 2,
        y: (area.height - modal_height) / 2,
        width: modal_width,
        height: modal_height,
    };

    frame.render_widget(Clear, modal_area);

    let (level_str, level_style) = level_style(entry.level);
    let title = Line::from(vec![
        Span::styled(" Log Entry ", Style::default().fg(Color::Cyan).bold()),
        Span::styled("|", Style::default().fg(Color::DarkGray)),
        Span::styled(format!(" {} ", level_str), level_style),
        Span::styled("|", Style::default().fg(Color::DarkGray)),
        Span::styled(" Esc", Style::default().fg(Color::Yellow)),
        Span::styled(": close ", Style::default().fg(Color::DarkGray)),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(title);
    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    let label_style = Style::default().fg(Color::DarkGray);
    let value_style = Style::default().fg(Color::White);

    let mut lines: Vec<Line> = Vec::new();

    // Timestamp
    lines.push(Line::from(vec![
        Span::styled(" Time:      ", label_style),
        Span::styled(format_timestamp(entry.timestamp_ns), value_style),
    ]));

    // Component
    lines.push(Line::from(vec![
        Span::styled(" Component: ", label_style),
        Span::styled(&entry.component, Style::default().fg(Color::Cyan)),
    ]));

    // Related objects
    if !entry.related_object_ids.is_empty() {
        let objects = entry.related_object_ids.join(", ");
        lines.push(Line::from(vec![
            Span::styled(" Objects:   ", label_style),
            Span::styled(objects, Style::default().fg(Color::Magenta)),
        ]));
    }

    // ID
    lines.push(Line::from(vec![
        Span::styled(" ID:        ", label_style),
        Span::styled(&entry.id, Style::default().fg(Color::DarkGray)),
    ]));

    // Separator
    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(" Message:", label_style)]));
    lines.push(Line::from(""));

    // Message - split by newlines and wrap
    for line in entry.message.lines() {
        lines.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(line, value_style),
        ]));
    }

    // Split inner area for header and message
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(7), Constraint::Min(1)])
        .split(inner);

    // Render header lines
    let header_text = Text::from(lines[..7.min(lines.len())].to_vec());
    frame.render_widget(Paragraph::new(header_text), chunks[0]);

    // Render message with wrapping
    if lines.len() > 7 {
        let message_lines: Vec<Line> = lines[7..].to_vec();
        let message_text = Text::from(message_lines);
        frame.render_widget(
            Paragraph::new(message_text).wrap(Wrap { trim: false }),
            chunks[1],
        );
    }
}
