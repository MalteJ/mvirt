//! System view - displays detailed host system information

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};

use crate::proto::SystemInfo;
use crate::tui::types::{ServiceVersions, SystemFocus};

#[allow(clippy::too_many_arguments)]
pub fn draw(
    frame: &mut Frame,
    system_info: Option<&SystemInfo>,
    service_versions: &ServiceVersions,
    focus: SystemFocus,
    cores_scroll: usize,
    disks_table_state: &mut TableState,
    nics_table_state: &mut TableState,
    status_message: Option<&str>,
    last_refresh: Option<chrono::DateTime<chrono::Local>>,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Title bar
            Constraint::Length(4), // Summary bar (host, cpu, memory)
            Constraint::Length(2), // Section tabs
            Constraint::Min(8),    // Main content area
            Constraint::Length(1), // Legend
            Constraint::Length(1), // Status
        ])
        .split(frame.area());

    // Title bar
    draw_title_bar(frame, chunks[0], system_info);

    // Summary bar
    draw_summary_bar(frame, chunks[1], system_info);

    // Section tabs
    draw_section_tabs(frame, chunks[2], focus);

    // Main content based on focus
    match focus {
        SystemFocus::Overview => draw_overview(frame, chunks[3], system_info, service_versions),
        SystemFocus::Cores => draw_cores_table(frame, chunks[3], system_info, cores_scroll),
        SystemFocus::Numa => draw_numa_table(frame, chunks[3], system_info),
        SystemFocus::Disks => draw_disks_table(frame, chunks[3], system_info, disks_table_state),
        SystemFocus::Nics => draw_nics_table(frame, chunks[3], system_info, nics_table_state),
    }

    // Legend
    let legend_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(12)])
        .split(chunks[4]);

    let legend = Line::from(vec![
        Span::styled(" Tab", Style::default().fg(Color::Magenta).bold()),
        Span::styled(":section ", Style::default().fg(Color::DarkGray)),
        Span::styled("\u{2191}\u{2193}", Style::default().fg(Color::Cyan).bold()),
        Span::styled(":scroll ", Style::default().fg(Color::DarkGray)),
        Span::styled("r", Style::default().fg(Color::Green).bold()),
        Span::styled(":refresh ", Style::default().fg(Color::DarkGray)),
        Span::styled("q", Style::default().fg(Color::Magenta).bold()),
        Span::styled(":quit", Style::default().fg(Color::DarkGray)),
    ]);
    frame.render_widget(Paragraph::new(legend), legend_chunks[0]);

    let refresh_time = last_refresh
        .map(|t| t.format("%H:%M:%S").to_string())
        .unwrap_or_else(|| "--:--:--".to_string());
    let refresh_text = Line::from(vec![Span::styled(
        format!("{} ", refresh_time),
        Style::default().fg(Color::DarkGray),
    )]);
    frame.render_widget(
        Paragraph::new(refresh_text).alignment(Alignment::Right),
        legend_chunks[1],
    );

    // Status bar
    if let Some(status) = status_message {
        let color = if status.starts_with("Loading") {
            Color::DarkGray
        } else {
            Color::Yellow
        };
        let status_line = Line::from(vec![Span::styled(
            format!(" {}", status),
            Style::default().fg(color),
        )]);
        frame.render_widget(Paragraph::new(status_line), chunks[5]);
    }
}

fn draw_title_bar(frame: &mut Frame, area: Rect, info: Option<&SystemInfo>) {
    let mut tabs = vec![
        Span::styled(" mvirt ", Style::default().fg(Color::Cyan).bold()),
        Span::styled("[1:VMs]", Style::default().fg(Color::DarkGray)),
        Span::raw(" "),
        Span::styled("[2:Storage]", Style::default().fg(Color::DarkGray)),
        Span::raw(" "),
        Span::styled("[3:Networks]", Style::default().fg(Color::DarkGray)),
        Span::raw(" "),
        Span::styled("[4:Logs]", Style::default().fg(Color::DarkGray)),
        Span::raw(" "),
        Span::styled("[5:", Style::default().fg(Color::DarkGray)),
        Span::styled("System", Style::default().fg(Color::White).bold()),
        Span::styled("]", Style::default().fg(Color::DarkGray)),
    ];

    // Right side: hostname
    if let Some(info) = info
        && let Some(host) = &info.host
    {
        tabs.push(Span::styled(
            format!("  {}", host.hostname),
            Style::default().fg(Color::Cyan),
        ));
    }

    let title = Line::from(tabs);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(title), inner);
}

fn draw_summary_bar(frame: &mut Frame, area: Rect, info: Option<&SystemInfo>) {
    let Some(info) = info else {
        frame.render_widget(
            Paragraph::new(" Loading system information...").fg(Color::DarkGray),
            area,
        );
        return;
    };

    // Line 1: Host info
    let host_line = if let Some(host) = &info.host {
        let uptime_str = format_uptime(host.uptime_seconds);
        Line::from(vec![
            Span::styled(" Host: ", Style::default().fg(Color::DarkGray)),
            Span::styled(&host.hostname, Style::default().fg(Color::White).bold()),
            Span::styled(" | Kernel: ", Style::default().fg(Color::DarkGray)),
            Span::styled(&host.kernel_version, Style::default().fg(Color::Cyan)),
            Span::styled(" | Up: ", Style::default().fg(Color::DarkGray)),
            Span::styled(uptime_str, Style::default().fg(Color::Green)),
        ])
    } else {
        Line::from(" Host: unknown")
    };

    // Line 2: CPU info
    let cpu_line = if let Some(cpu) = &info.cpu {
        let load_color = if info.load_1 > (cpu.logical_cores as f32 * 0.8) {
            Color::Red
        } else if info.load_1 > (cpu.logical_cores as f32 * 0.5) {
            Color::Yellow
        } else {
            Color::Green
        };
        Line::from(vec![
            Span::styled(" CPU: ", Style::default().fg(Color::DarkGray)),
            Span::styled(&cpu.model, Style::default().fg(Color::White)),
            Span::styled(
                format!(" ({}c/{}t)", cpu.physical_cores, cpu.logical_cores),
                Style::default().fg(Color::Cyan),
            ),
            Span::styled(" | Load: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:.2} {:.2} {:.2}", info.load_1, info.load_5, info.load_15),
                Style::default().fg(load_color),
            ),
        ])
    } else {
        Line::from(format!(
            " CPU: {} cores | Load: {:.2} {:.2} {:.2}",
            info.total_cpus, info.load_1, info.load_5, info.load_15
        ))
    };

    // Line 3: Memory info
    let mem_line = if let Some(mem) = &info.memory {
        let used = mem.total_bytes - mem.available_bytes;
        let pct = (used as f64 / mem.total_bytes as f64 * 100.0) as u8;
        let mem_color = if pct > 80 {
            Color::Red
        } else if pct > 50 {
            Color::Yellow
        } else {
            Color::Green
        };
        let swap_used = mem.swap_used_bytes;
        let swap_total = mem.swap_total_bytes;
        Line::from(vec![
            Span::styled(" RAM: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!(
                    "{}/{} GiB ({}%)",
                    format_gib(used),
                    format_gib(mem.total_bytes),
                    pct
                ),
                Style::default().fg(mem_color),
            ),
            Span::styled(" | Swap: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{}/{} GiB", format_gib(swap_used), format_gib(swap_total)),
                Style::default().fg(Color::Cyan),
            ),
            Span::styled(" | HugePages: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{}/{}", mem.hugepages_free, mem.hugepages_total),
                Style::default().fg(Color::Cyan),
            ),
        ])
    } else {
        let used = info.total_memory_mb - (info.total_memory_mb / 4); // Rough estimate
        Line::from(format!(" RAM: ~{}/{} MiB", used, info.total_memory_mb))
    };

    let text = vec![host_line, cpu_line, mem_line];
    frame.render_widget(Paragraph::new(text), area);
}

fn draw_section_tabs(frame: &mut Frame, area: Rect, focus: SystemFocus) {
    let tabs = [
        ("Overview", SystemFocus::Overview),
        ("Cores", SystemFocus::Cores),
        ("NUMA", SystemFocus::Numa),
        ("Disks", SystemFocus::Disks),
        ("NICs", SystemFocus::Nics),
    ];

    let mut spans = vec![Span::raw(" ")];
    for (i, (name, tab_focus)) in tabs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" "));
        }
        if *tab_focus == focus {
            spans.push(Span::styled(
                format!("[{}]", name),
                Style::default().fg(Color::Cyan).bold(),
            ));
        } else {
            spans.push(Span::styled(
                format!("[{}]", name),
                Style::default().fg(Color::DarkGray),
            ));
        }
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_overview(
    frame: &mut Frame,
    area: Rect,
    info: Option<&SystemInfo>,
    versions: &ServiceVersions,
) {
    let Some(info) = info else {
        frame.render_widget(Paragraph::new(" Loading...").fg(Color::DarkGray), area);
        return;
    };

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    // Left: CPU flags and NUMA summary
    let mut left_lines = vec![];

    if let Some(cpu) = &info.cpu {
        left_lines.push(Line::from(vec![
            Span::styled(" CPU Flags: ", Style::default().fg(Color::DarkGray)),
            Span::styled(cpu.flags.join(" "), Style::default().fg(Color::Cyan)),
        ]));
        left_lines.push(Line::from(format!(
            " Sockets: {} | Physical: {} | Logical: {}",
            cpu.sockets, cpu.physical_cores, cpu.logical_cores
        )));
    }

    left_lines.push(Line::raw(""));
    left_lines.push(Line::styled(
        " NUMA Topology:",
        Style::default().fg(Color::White).bold(),
    ));

    if info.numa_nodes.is_empty() {
        left_lines.push(Line::styled(
            "   N/A (single-socket system)",
            Style::default().fg(Color::DarkGray),
        ));
    } else {
        for node in &info.numa_nodes {
            left_lines.push(Line::from(format!(
                "   Node {}: CPUs {}-{} | {}/{} GiB free",
                node.id,
                node.cpu_ids.first().unwrap_or(&0),
                node.cpu_ids.last().unwrap_or(&0),
                format_gib(node.free_memory_bytes),
                format_gib(node.total_memory_bytes),
            )));
        }
    }

    frame.render_widget(Paragraph::new(left_lines), chunks[0]);

    // Right: Disk and NIC summary
    let mut right_lines = vec![];

    right_lines.push(Line::styled(
        " Disks:",
        Style::default().fg(Color::White).bold(),
    ));
    if info.disks.is_empty() {
        right_lines.push(Line::styled(
            "   No disks found",
            Style::default().fg(Color::DarkGray),
        ));
    } else {
        for disk in info.disks.iter().take(4) {
            let health_icon = if disk.smart_available {
                if disk.smart_healthy {
                    Span::styled("\u{2713}", Style::default().fg(Color::Green))
                } else {
                    Span::styled("\u{2717}", Style::default().fg(Color::Red))
                }
            } else {
                Span::styled("?", Style::default().fg(Color::DarkGray))
            };
            right_lines.push(Line::from(vec![
                Span::raw("   "),
                health_icon,
                Span::raw(" "),
                Span::styled(&disk.device, Style::default().fg(Color::Cyan)),
                Span::raw(" "),
                Span::styled(
                    format!("{} GiB", format_gib(disk.size_bytes)),
                    Style::default().fg(Color::White),
                ),
                Span::raw(" "),
                Span::styled(
                    disk.model.chars().take(20).collect::<String>(),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }
        if info.disks.len() > 4 {
            right_lines.push(Line::styled(
                format!("   ... and {} more", info.disks.len() - 4),
                Style::default().fg(Color::DarkGray),
            ));
        }
    }

    right_lines.push(Line::raw(""));
    right_lines.push(Line::styled(
        " Network Interfaces:",
        Style::default().fg(Color::White).bold(),
    ));
    if info.nics.is_empty() {
        right_lines.push(Line::styled(
            "   No NICs found",
            Style::default().fg(Color::DarkGray),
        ));
    } else {
        for nic in info.nics.iter().take(4) {
            let status_icon = if nic.is_up {
                Span::styled("\u{25cf}", Style::default().fg(Color::Green))
            } else {
                Span::styled("\u{25cb}", Style::default().fg(Color::DarkGray))
            };
            let speed = nic
                .speed_mbps
                .map(|s| format!("{}Mbps", s))
                .unwrap_or_else(|| "-".to_string());
            right_lines.push(Line::from(vec![
                Span::raw("   "),
                status_icon,
                Span::raw(" "),
                Span::styled(&nic.name, Style::default().fg(Color::Cyan)),
                Span::raw(" "),
                Span::styled(speed, Style::default().fg(Color::White)),
                Span::raw(" "),
                Span::styled(&nic.mac, Style::default().fg(Color::DarkGray)),
            ]));
        }
        if info.nics.len() > 4 {
            right_lines.push(Line::styled(
                format!("   ... and {} more", info.nics.len() - 4),
                Style::default().fg(Color::DarkGray),
            ));
        }
    }

    // Services section with versions
    right_lines.push(Line::raw(""));
    right_lines.push(Line::styled(
        " Services:",
        Style::default().fg(Color::White).bold(),
    ));

    let version_or_dash =
        |v: &Option<String>| v.as_ref().map(|s| s.as_str()).unwrap_or("-").to_string();

    right_lines.push(Line::from(vec![
        Span::raw("   "),
        Span::styled("mvirt-vmm ", Style::default().fg(Color::Cyan)),
        Span::styled(
            version_or_dash(&versions.vmm),
            Style::default().fg(Color::White),
        ),
    ]));
    right_lines.push(Line::from(vec![
        Span::raw("   "),
        Span::styled("mvirt-zfs ", Style::default().fg(Color::Cyan)),
        Span::styled(
            version_or_dash(&versions.zfs),
            Style::default().fg(Color::White),
        ),
    ]));
    right_lines.push(Line::from(vec![
        Span::raw("   "),
        Span::styled("mvirt-net ", Style::default().fg(Color::Cyan)),
        Span::styled(
            version_or_dash(&versions.net),
            Style::default().fg(Color::White),
        ),
    ]));
    right_lines.push(Line::from(vec![
        Span::raw("   "),
        Span::styled("mvirt-log ", Style::default().fg(Color::Cyan)),
        Span::styled(
            version_or_dash(&versions.log),
            Style::default().fg(Color::White),
        ),
    ]));

    frame.render_widget(Paragraph::new(right_lines), chunks[1]);
}

fn draw_cores_table(frame: &mut Frame, area: Rect, info: Option<&SystemInfo>, scroll: usize) {
    let Some(info) = info else {
        frame.render_widget(Paragraph::new(" Loading...").fg(Color::DarkGray), area);
        return;
    };

    let Some(cpu) = &info.cpu else {
        frame.render_widget(
            Paragraph::new(" CPU info not available").fg(Color::DarkGray),
            area,
        );
        return;
    };

    let header = Row::new(vec![
        Cell::from("Core").style(Style::default().fg(Color::Blue).bold()),
        Cell::from("Freq (MHz)").style(Style::default().fg(Color::Blue).bold()),
        Cell::from("Usage").style(Style::default().fg(Color::Blue).bold()),
        Cell::from("NUMA").style(Style::default().fg(Color::Blue).bold()),
    ])
    .height(1);

    let visible_cores: Vec<_> = cpu
        .cores
        .iter()
        .skip(scroll)
        .take(area.height as usize - 3)
        .collect();

    let rows: Vec<Row> = visible_cores
        .iter()
        .map(|core| {
            let usage_color = if core.usage_percent > 80.0 {
                Color::Red
            } else if core.usage_percent > 50.0 {
                Color::Yellow
            } else {
                Color::Green
            };
            Row::new(vec![
                Cell::from(format!("{:4}", core.id)),
                Cell::from(format!("{:6}", core.frequency_mhz)),
                Cell::from(format!("{:5.1}%", core.usage_percent))
                    .style(Style::default().fg(usage_color)),
                Cell::from(format!("{:4}", core.numa_node)),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(6),
        Constraint::Length(12),
        Constraint::Length(8),
        Constraint::Length(6),
    ];

    let title = format!(" CPU Cores ({}/{}) ", scroll + 1, cpu.cores.len());

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .row_highlight_style(Style::default().bg(Color::DarkGray));

    frame.render_widget(table, area);
}

fn draw_numa_table(frame: &mut Frame, area: Rect, info: Option<&SystemInfo>) {
    let Some(info) = info else {
        frame.render_widget(Paragraph::new(" Loading...").fg(Color::DarkGray), area);
        return;
    };

    if info.numa_nodes.is_empty() {
        let block = Block::default()
            .title(" NUMA Topology ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        frame.render_widget(
            Paragraph::new(" NUMA not available (single-socket system)").fg(Color::DarkGray),
            inner,
        );
        return;
    }

    let header = Row::new(vec![
        Cell::from("Node").style(Style::default().fg(Color::Blue).bold()),
        Cell::from("CPUs").style(Style::default().fg(Color::Blue).bold()),
        Cell::from("Total Memory").style(Style::default().fg(Color::Blue).bold()),
        Cell::from("Free Memory").style(Style::default().fg(Color::Blue).bold()),
        Cell::from("Used %").style(Style::default().fg(Color::Blue).bold()),
    ])
    .height(1);

    let rows: Vec<Row> = info
        .numa_nodes
        .iter()
        .map(|node| {
            let cpu_range = if node.cpu_ids.is_empty() {
                "-".to_string()
            } else {
                format!(
                    "{}-{}",
                    node.cpu_ids.first().unwrap(),
                    node.cpu_ids.last().unwrap()
                )
            };
            let used_pct = if node.total_memory_bytes > 0 {
                ((node.total_memory_bytes - node.free_memory_bytes) as f64
                    / node.total_memory_bytes as f64
                    * 100.0) as u8
            } else {
                0
            };
            let pct_color = if used_pct > 80 {
                Color::Red
            } else if used_pct > 50 {
                Color::Yellow
            } else {
                Color::Green
            };
            Row::new(vec![
                Cell::from(format!("{}", node.id)),
                Cell::from(cpu_range),
                Cell::from(format!("{} GiB", format_gib(node.total_memory_bytes))),
                Cell::from(format!("{} GiB", format_gib(node.free_memory_bytes))),
                Cell::from(format!("{}%", used_pct)).style(Style::default().fg(pct_color)),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(6),
        Constraint::Length(12),
        Constraint::Length(14),
        Constraint::Length(14),
        Constraint::Length(8),
    ];

    let table = Table::new(rows, widths).header(header).block(
        Block::default()
            .title(" NUMA Topology ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)),
    );

    frame.render_widget(table, area);
}

fn draw_disks_table(
    frame: &mut Frame,
    area: Rect,
    info: Option<&SystemInfo>,
    table_state: &mut TableState,
) {
    let Some(info) = info else {
        frame.render_widget(Paragraph::new(" Loading...").fg(Color::DarkGray), area);
        return;
    };

    let header = Row::new(vec![
        Cell::from("Device").style(Style::default().fg(Color::Blue).bold()),
        Cell::from("Model").style(Style::default().fg(Color::Blue).bold()),
        Cell::from("Size").style(Style::default().fg(Color::Blue).bold()),
        Cell::from("SMART").style(Style::default().fg(Color::Blue).bold()),
        Cell::from("Temp").style(Style::default().fg(Color::Blue).bold()),
        Cell::from("Hours").style(Style::default().fg(Color::Blue).bold()),
    ])
    .height(1);

    let rows: Vec<Row> = info
        .disks
        .iter()
        .map(|disk| {
            let smart_status = if disk.smart_available {
                if disk.smart_healthy {
                    Cell::from("OK").style(Style::default().fg(Color::Green))
                } else {
                    Cell::from("FAIL").style(Style::default().fg(Color::Red).bold())
                }
            } else {
                Cell::from("N/A").style(Style::default().fg(Color::DarkGray))
            };

            let temp = disk
                .temperature_celsius
                .map(|t| {
                    let color = if t > 50 {
                        Color::Red
                    } else if t > 40 {
                        Color::Yellow
                    } else {
                        Color::Green
                    };
                    Cell::from(format!("{}C", t)).style(Style::default().fg(color))
                })
                .unwrap_or_else(|| Cell::from("-").style(Style::default().fg(Color::DarkGray)));

            let hours = disk
                .power_on_hours
                .map(|h| Cell::from(format!("{}", h)))
                .unwrap_or_else(|| Cell::from("-").style(Style::default().fg(Color::DarkGray)));

            Row::new(vec![
                Cell::from(disk.device.clone()),
                Cell::from(disk.model.chars().take(25).collect::<String>()),
                Cell::from(format!("{} GiB", format_gib(disk.size_bytes))),
                smart_status,
                temp,
                hours,
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(12),
        Constraint::Min(20),
        Constraint::Length(10),
        Constraint::Length(6),
        Constraint::Length(6),
        Constraint::Length(8),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .title(format!(" Disks ({}) ", info.disks.len()))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .row_highlight_style(Style::default().bg(Color::DarkGray));

    frame.render_stateful_widget(table, area, table_state);
}

fn draw_nics_table(
    frame: &mut Frame,
    area: Rect,
    info: Option<&SystemInfo>,
    table_state: &mut TableState,
) {
    let Some(info) = info else {
        frame.render_widget(Paragraph::new(" Loading...").fg(Color::DarkGray), area);
        return;
    };

    let header = Row::new(vec![
        Cell::from("Interface").style(Style::default().fg(Color::Blue).bold()),
        Cell::from("MAC").style(Style::default().fg(Color::Blue).bold()),
        Cell::from("Status").style(Style::default().fg(Color::Blue).bold()),
        Cell::from("Speed").style(Style::default().fg(Color::Blue).bold()),
        Cell::from("IPv4").style(Style::default().fg(Color::Blue).bold()),
        Cell::from("RX/TX").style(Style::default().fg(Color::Blue).bold()),
        Cell::from("Driver").style(Style::default().fg(Color::Blue).bold()),
    ])
    .height(1);

    let rows: Vec<Row> = info
        .nics
        .iter()
        .map(|nic| {
            let status = if nic.is_up {
                Cell::from("up").style(Style::default().fg(Color::Green))
            } else {
                Cell::from("down").style(Style::default().fg(Color::Red))
            };

            let speed = nic
                .speed_mbps
                .map(|s| format!("{}M", s))
                .unwrap_or_else(|| "-".to_string());

            let ipv4 = nic.ipv4.first().cloned().unwrap_or_else(|| "-".to_string());

            let rx_tx = format!(
                "{}/{}",
                format_bytes(nic.rx_bytes),
                format_bytes(nic.tx_bytes)
            );

            Row::new(vec![
                Cell::from(nic.name.clone()),
                Cell::from(nic.mac.clone()),
                status,
                Cell::from(speed),
                Cell::from(ipv4),
                Cell::from(rx_tx),
                Cell::from(nic.driver.clone()),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(12),
        Constraint::Length(18),
        Constraint::Length(6),
        Constraint::Length(8),
        Constraint::Length(16),
        Constraint::Length(14),
        Constraint::Min(10),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .title(format!(" Network Interfaces ({}) ", info.nics.len()))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .row_highlight_style(Style::default().bg(Color::DarkGray));

    frame.render_stateful_widget(table, area, table_state);
}

// Helper functions

fn format_uptime(seconds: u64) -> String {
    let days = seconds / 86400;
    let hours = (seconds % 86400) / 3600;
    let minutes = (seconds % 3600) / 60;

    if days > 0 {
        format!("{}d {}h", days, hours)
    } else if hours > 0 {
        format!("{}h {}m", hours, minutes)
    } else {
        format!("{}m", minutes)
    }
}

fn format_gib(bytes: u64) -> String {
    let gib = bytes as f64 / 1024.0 / 1024.0 / 1024.0;
    if gib >= 100.0 {
        format!("{:.0}", gib)
    } else if gib >= 10.0 {
        format!("{:.1}", gib)
    } else {
        format!("{:.2}", gib)
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_000_000_000_000 {
        format!("{:.1}T", bytes as f64 / 1_000_000_000_000.0)
    } else if bytes >= 1_000_000_000 {
        format!("{:.1}G", bytes as f64 / 1_000_000_000.0)
    } else if bytes >= 1_000_000 {
        format!("{:.1}M", bytes as f64 / 1_000_000.0)
    } else if bytes >= 1_000 {
        format!("{:.1}K", bytes as f64 / 1_000.0)
    } else {
        format!("{}B", bytes)
    }
}
