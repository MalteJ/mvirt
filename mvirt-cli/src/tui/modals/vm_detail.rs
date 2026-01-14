use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::proto::{BootMode, Vm, VmState};

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

pub fn draw(frame: &mut Frame, vm: &Vm) {
    let area = frame.area();
    let modal_width = 70.min(area.width.saturating_sub(4));
    let modal_height = 22.min(area.height.saturating_sub(4));

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
    frame.render_widget(Paragraph::new(text), inner);
}
