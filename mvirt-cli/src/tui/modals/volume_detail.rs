//! Modal for displaying volume details with logs

use chrono::{DateTime, Local};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::zfs_proto::Volume;
use mvirt_log::{LogEntry, LogLevel};

fn format_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}

fn format_log_timestamp(timestamp_ns: i64) -> String {
    let secs = timestamp_ns / 1_000_000_000;
    let nanos = (timestamp_ns % 1_000_000_000) as u32;
    if let Some(dt) = DateTime::from_timestamp(secs, nanos) {
        let local: DateTime<Local> = dt.into();
        local.format("%m-%d %H:%M:%S").to_string()
    } else {
        "??".to_string()
    }
}

fn level_style(level: i32) -> (String, Color) {
    match LogLevel::try_from(level) {
        Ok(LogLevel::Info) => ("I".to_string(), Color::Green),
        Ok(LogLevel::Warn) => ("W".to_string(), Color::Yellow),
        Ok(LogLevel::Error) => ("E".to_string(), Color::Red),
        Ok(LogLevel::Debug) => ("D".to_string(), Color::DarkGray),
        Ok(LogLevel::Audit) => ("A".to_string(), Color::Cyan),
        Err(_) | Ok(_) => ("?".to_string(), Color::White),
    }
}

pub fn draw(frame: &mut Frame, volume: &Volume, logs: &[LogEntry]) {
    let area = frame.area();
    let modal_width = 80.min(area.width.saturating_sub(4));
    let modal_height = 26.min(area.height.saturating_sub(4));

    let modal_area = Rect {
        x: (area.width - modal_width) / 2,
        y: (area.height - modal_height) / 2,
        width: modal_width,
        height: modal_height,
    };

    frame.render_widget(Clear, modal_area);

    let title = Line::from(vec![
        Span::styled(
            format!(" {} ", volume.name),
            Style::default().fg(Color::Cyan).bold(),
        ),
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

    // Split inner area: top for volume details, bottom for logs
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(8),     // Volume details
            Constraint::Length(10), // Logs section
        ])
        .split(inner);

    let label_style = Style::default().fg(Color::DarkGray);
    let value_style = Style::default().fg(Color::White);

    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled(" Name:        ", label_style),
            Span::styled(&volume.name, value_style),
        ]),
        Line::from(vec![
            Span::styled(" Size:        ", label_style),
            Span::styled(format_size(volume.volsize_bytes), value_style),
        ]),
        Line::from(vec![
            Span::styled(" Used:        ", label_style),
            Span::styled(format_size(volume.used_bytes), value_style),
        ]),
        Line::from(vec![
            Span::styled(" Path:        ", label_style),
            Span::styled(&volume.path, value_style),
        ]),
    ];

    // Snapshots
    if !volume.snapshots.is_empty() {
        lines.push(Line::from(vec![Span::raw("")]));
        lines.push(Line::from(vec![
            Span::styled(" Snapshots:   ", label_style),
            Span::styled(
                format!("{} snapshot(s)", volume.snapshots.len()),
                value_style,
            ),
        ]));
        for snap in volume.snapshots.iter().take(3) {
            lines.push(Line::from(vec![
                Span::styled("              ", label_style),
                Span::styled(
                    format!("@{} ({})", snap.name, format_size(snap.used_bytes)),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }
        if volume.snapshots.len() > 3 {
            lines.push(Line::from(vec![
                Span::styled("              ", label_style),
                Span::styled(
                    format!("... and {} more", volume.snapshots.len() - 3),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }
    }

    let text = Text::from(lines);
    frame.render_widget(Paragraph::new(text), chunks[0]);

    // Logs section
    let logs_block = Block::default()
        .title(" Logs ")
        .borders(Borders::TOP)
        .border_style(Style::default().fg(Color::DarkGray));
    let logs_inner = logs_block.inner(chunks[1]);
    frame.render_widget(logs_block, chunks[1]);

    if logs.is_empty() {
        let no_logs = Paragraph::new(Span::styled(
            " No logs found",
            Style::default().fg(Color::DarkGray),
        ));
        frame.render_widget(no_logs, logs_inner);
    } else {
        let log_lines: Vec<Line> = logs
            .iter()
            .take(logs_inner.height as usize)
            .map(|entry| {
                let (level_char, level_color) = level_style(entry.level);
                let timestamp = format_log_timestamp(entry.timestamp_ns);
                let max_msg_len = logs_inner.width.saturating_sub(20) as usize;
                let message = if entry.message.len() > max_msg_len {
                    format!("{}\u{2026}", &entry.message[..max_msg_len])
                } else {
                    entry.message.clone()
                };
                Line::from(vec![
                    Span::styled(
                        format!(" {} ", level_char),
                        Style::default().fg(level_color),
                    ),
                    Span::styled(
                        format!("{} ", timestamp),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::styled(message, Style::default().fg(Color::White)),
                ])
            })
            .collect();
        frame.render_widget(Paragraph::new(log_lines), logs_inner);
    }
}
