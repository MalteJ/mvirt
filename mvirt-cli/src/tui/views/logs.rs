use chrono::{DateTime, Local};
use mvirt_log::{LogEntry, LogLevel};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};

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
fn level_style(level: i32) -> (String, Color) {
    match LogLevel::try_from(level) {
        Ok(LogLevel::Info) => ("INFO".to_string(), Color::Green),
        Ok(LogLevel::Warn) => ("WARN".to_string(), Color::Yellow),
        Ok(LogLevel::Error) => ("ERROR".to_string(), Color::Red),
        Ok(LogLevel::Debug) => ("DEBUG".to_string(), Color::DarkGray),
        Ok(LogLevel::Audit) => ("AUDIT".to_string(), Color::Cyan),
        Err(_) => ("???".to_string(), Color::White),
    }
}

pub fn draw(
    frame: &mut Frame,
    logs: &[LogEntry],
    table_state: &mut TableState,
    status_message: Option<&str>,
    last_refresh: Option<chrono::DateTime<chrono::Local>>,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Title
            Constraint::Min(5),    // Logs table
            Constraint::Length(1), // Legend
            Constraint::Length(1), // Status
        ])
        .split(frame.area());

    // Title bar
    draw_title(frame, chunks[0], logs.len());

    // Logs table
    draw_logs_table(frame, chunks[1], logs, table_state);

    // Legend
    let legend = Line::from(vec![
        Span::styled(" Enter", Style::default().fg(Color::Cyan).bold()),
        Span::styled(":details ", Style::default().fg(Color::DarkGray)),
        Span::styled("r", Style::default().fg(Color::Green).bold()),
        Span::styled(":refresh ", Style::default().fg(Color::DarkGray)),
        Span::styled("Tab", Style::default().fg(Color::Magenta).bold()),
        Span::styled(":switch view ", Style::default().fg(Color::DarkGray)),
        Span::styled("q", Style::default().fg(Color::Magenta).bold()),
        Span::styled(":quit", Style::default().fg(Color::DarkGray)),
    ]);
    frame.render_widget(Paragraph::new(legend), chunks[2]);

    // Status bar
    draw_status_bar(frame, chunks[3], status_message, last_refresh);
}

fn draw_title(frame: &mut Frame, area: Rect, log_count: usize) {
    // Left: Title with tabs
    let mut tabs = vec![
        Span::styled(" mvirt ", Style::default().fg(Color::Cyan).bold()),
        Span::styled("[1:VMs]", Style::default().fg(Color::DarkGray)),
    ];
    tabs.push(Span::styled(
        " [2:Storage]",
        Style::default().fg(Color::DarkGray),
    ));
    tabs.push(Span::styled(
        " [3:Networks]",
        Style::default().fg(Color::DarkGray),
    ));
    tabs.push(Span::raw(" "));
    tabs.push(Span::styled("[4:", Style::default().fg(Color::DarkGray)));
    tabs.push(Span::styled(
        "Logs",
        Style::default().fg(Color::White).bold(),
    ));
    tabs.push(Span::styled("]", Style::default().fg(Color::DarkGray)));
    let title = Line::from(tabs);

    // Right: Log count
    let stats = Line::from(vec![
        Span::styled("Entries: ", Style::default().fg(Color::DarkGray)),
        Span::styled(format!("{}", log_count), Style::default().fg(Color::White)),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    frame.render_widget(block.clone(), area);
    let inner = block.inner(area);
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(30)])
        .split(inner);
    frame.render_widget(Paragraph::new(title), chunks[0]);
    frame.render_widget(Paragraph::new(stats).alignment(Alignment::Right), chunks[1]);
}

fn draw_logs_table(frame: &mut Frame, area: Rect, logs: &[LogEntry], table_state: &mut TableState) {
    let header = Row::new(vec![
        Cell::from("Time").style(Style::default().fg(Color::Yellow)),
        Cell::from("Level").style(Style::default().fg(Color::Yellow)),
        Cell::from("Component").style(Style::default().fg(Color::Yellow)),
        Cell::from("Message").style(Style::default().fg(Color::Yellow)),
    ])
    .height(1);

    let rows: Vec<Row> = logs
        .iter()
        .map(|entry| {
            let (level_str, level_color) = level_style(entry.level);
            Row::new(vec![
                Cell::from(format_timestamp(entry.timestamp_ns)),
                Cell::from(level_str).style(Style::default().fg(level_color)),
                Cell::from(entry.component.clone()).style(Style::default().fg(Color::Cyan)),
                Cell::from(entry.message.clone()),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(19), // Time: "2024-01-15 12:34:56"
        Constraint::Length(5),  // Level: "ERROR"
        Constraint::Length(10), // Component
        Constraint::Min(20),    // Message
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .title(" Logs ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::White)),
        )
        .row_highlight_style(Style::default().bg(Color::DarkGray))
        .highlight_symbol("> ");

    frame.render_stateful_widget(table, area, table_state);
}

fn draw_status_bar(
    frame: &mut Frame,
    area: Rect,
    status_message: Option<&str>,
    last_refresh: Option<chrono::DateTime<chrono::Local>>,
) {
    let status_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(20)])
        .split(area);

    // Left: Status message
    let status = if let Some(msg) = status_message {
        Paragraph::new(format!(" {}", msg)).style(Style::default().fg(Color::Yellow))
    } else {
        Paragraph::new(" Ready").style(Style::default().fg(Color::DarkGray))
    };
    frame.render_widget(status, status_chunks[0]);

    // Right: Last refresh time
    if let Some(time) = last_refresh {
        let refresh = Paragraph::new(format!("Updated: {} ", time.format("%H:%M:%S")))
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Right);
        frame.render_widget(refresh, status_chunks[1]);
    }
}
