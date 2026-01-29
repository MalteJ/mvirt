use chrono::{DateTime, Local};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::proto::{BootMode, Vm, VmState};
use mvirt_log::{LogEntry, LogLevel};

pub fn format_state(state: i32) -> &'static str {
    match VmState::try_from(state).unwrap_or(VmState::Unspecified) {
        VmState::Unspecified => "\u{25cb} unknown",
        VmState::Stopped => "\u{25cb} stopped",
        VmState::Starting => "\u{25d0} starting",
        VmState::Running => "\u{25cf} running",
        VmState::Stopping => "\u{25d1} stopping",
    }
}

pub fn state_style(state: i32) -> Style {
    match VmState::try_from(state).unwrap_or(VmState::Unspecified) {
        VmState::Running => Style::default().fg(Color::Green).bold(),
        VmState::Stopped => Style::default().fg(Color::DarkGray),
        VmState::Starting | VmState::Stopping => Style::default().fg(Color::Yellow),
        VmState::Unspecified => Style::default().fg(Color::DarkGray),
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

pub fn draw(frame: &mut Frame, vm: &Vm, logs: &[LogEntry]) {
    let area = frame.area();
    let modal_width = 80.min(area.width.saturating_sub(4));
    let modal_height = 30.min(area.height.saturating_sub(4));

    let modal_area = Rect {
        x: (area.width - modal_width) / 2,
        y: (area.height - modal_height) / 2,
        width: modal_width,
        height: modal_height,
    };

    frame.render_widget(Clear, modal_area);

    let name_str = vm.name.clone().unwrap_or_else(|| "unnamed".to_string());
    let title = Line::from(vec![
        Span::styled(
            format!(" {} ", name_str),
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

    // Split inner area: top for VM details, bottom for logs
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(10),    // VM details
            Constraint::Length(10), // Logs section
        ])
        .split(inner);

    let config = vm.config.as_ref();
    let label_style = Style::default().fg(Color::DarkGray);
    let value_style = Style::default().fg(Color::White);

    let mut lines: Vec<Line> = Vec::new();

    lines.push(Line::from(vec![
        Span::styled(" ID:          ", label_style),
        Span::styled(&vm.id, value_style),
    ]));

    let state_text = format_state(vm.state);
    lines.push(Line::from(vec![
        Span::styled(" State:       ", label_style),
        Span::styled(state_text, state_style(vm.state)),
    ]));

    if let Some(cfg) = config {
        lines.push(Line::from(vec![
            Span::styled(" VCPUs:       ", label_style),
            Span::styled(cfg.vcpus.to_string(), value_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled(" Memory:      ", label_style),
            Span::styled(format!("{} MB", cfg.memory_mb), value_style),
        ]));

        let boot_mode_str = match BootMode::try_from(cfg.boot_mode) {
            Ok(BootMode::Disk) | Ok(BootMode::Unspecified) | Err(_) => "Disk (UEFI)",
            Ok(BootMode::Kernel) => "Kernel (Direct)",
        };
        lines.push(Line::from(vec![
            Span::styled(" Boot Mode:   ", label_style),
            Span::styled(boot_mode_str, value_style),
        ]));

        if let Some(ref kernel) = cfg.kernel {
            lines.push(Line::from(vec![
                Span::styled(" Kernel:      ", label_style),
                Span::styled(kernel, value_style),
            ]));
        }

        if let Some(ref initramfs) = cfg.initramfs {
            lines.push(Line::from(vec![
                Span::styled(" Initramfs:   ", label_style),
                Span::styled(initramfs, value_style),
            ]));
        }

        if let Some(ref cmdline) = cfg.cmdline {
            let display_cmdline = if cmdline.len() > 50 {
                format!("{}\u{2026}", &cmdline[..50])
            } else {
                cmdline.clone()
            };
            lines.push(Line::from(vec![
                Span::styled(" Cmdline:     ", label_style),
                Span::styled(display_cmdline, value_style),
            ]));
        }

        for (i, disk) in cfg.disks.iter().enumerate() {
            let label = if i == 0 {
                " Disks:       "
            } else {
                "              "
            };
            let readonly = if disk.readonly { " (ro)" } else { "" };
            lines.push(Line::from(vec![
                Span::styled(label, label_style),
                Span::styled(format!("{}{}", disk.path, readonly), value_style),
            ]));
        }

        for (i, nic) in cfg.nics.iter().enumerate() {
            let label = if i == 0 {
                " NICs:        "
            } else {
                "              "
            };
            let nic_info = match (&nic.tap, &nic.mac) {
                (Some(tap), Some(mac)) => format!("{} ({})", tap, mac),
                (Some(tap), None) => tap.clone(),
                (None, Some(mac)) => format!("({})", mac),
                (None, None) => "default".to_string(),
            };
            lines.push(Line::from(vec![
                Span::styled(label, label_style),
                Span::styled(nic_info, value_style),
            ]));
        }

        if cfg.user_data.is_some() {
            lines.push(Line::from(vec![
                Span::styled(" User-Data:   ", label_style),
                Span::styled("configured", Style::default().fg(Color::Green)),
            ]));
        }
    }

    lines.push(Line::from(vec![Span::raw("")]));

    let created_time = chrono::DateTime::from_timestamp(vm.created_at, 0)
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| "-".to_string());
    lines.push(Line::from(vec![
        Span::styled(" Created:     ", label_style),
        Span::styled(created_time, value_style),
    ]));

    if let Some(started_at) = vm.started_at {
        let started_time = chrono::DateTime::from_timestamp(started_at, 0)
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| "-".to_string());
        lines.push(Line::from(vec![
            Span::styled(" Started:     ", label_style),
            Span::styled(started_time, value_style),
        ]));
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
