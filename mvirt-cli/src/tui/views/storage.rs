use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};

use crate::tui::types::{StorageFocus, StorageState};
use crate::zfs_proto::ImportJobState;

/// Format bytes to human-readable size
fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if bytes >= TB {
        format!("{:.1} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

#[allow(clippy::too_many_arguments)]
pub fn draw(
    frame: &mut Frame,
    storage: &StorageState,
    volume_table_state: &mut TableState,
    template_table_state: &mut TableState,
    focus: StorageFocus,
    status_message: Option<&str>,
    confirm_delete_volume: Option<&str>,
    confirm_delete_template: Option<&str>,
    last_refresh: Option<chrono::DateTime<chrono::Local>>,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Title / Pool stats
            Constraint::Min(8),    // Volumes table
            Constraint::Length(8), // Templates table
            Constraint::Length(1), // Legend
            Constraint::Length(1), // Status
        ])
        .split(frame.area());

    // Title bar with pool stats
    draw_pool_stats(frame, chunks[0], storage);

    // Volumes table
    draw_volumes_table(
        frame,
        chunks[1],
        storage,
        volume_table_state,
        focus == StorageFocus::Volumes,
    );

    // Templates table
    draw_templates_table(
        frame,
        chunks[2],
        storage,
        template_table_state,
        focus == StorageFocus::Templates,
    );

    // Legend - context-sensitive based on focus
    let legend_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(12)])
        .split(chunks[3]);

    let legend = match focus {
        StorageFocus::Volumes => Line::from(vec![
            Span::styled(" n", Style::default().fg(Color::Green).bold()),
            Span::styled(":new ", Style::default().fg(Color::DarkGray)),
            Span::styled("i", Style::default().fg(Color::Green).bold()),
            Span::styled(":import ", Style::default().fg(Color::DarkGray)),
            Span::styled("r", Style::default().fg(Color::Yellow).bold()),
            Span::styled(":resize ", Style::default().fg(Color::DarkGray)),
            Span::styled("s", Style::default().fg(Color::Cyan).bold()),
            Span::styled(":snap ", Style::default().fg(Color::DarkGray)),
            Span::styled("t", Style::default().fg(Color::Cyan).bold()),
            Span::styled(":template ", Style::default().fg(Color::DarkGray)),
            Span::styled("d", Style::default().fg(Color::Red).bold()),
            Span::styled(":delete ", Style::default().fg(Color::DarkGray)),
            Span::styled("S-Tab", Style::default().fg(Color::Magenta).bold()),
            Span::styled(":templates ", Style::default().fg(Color::DarkGray)),
            Span::styled("q", Style::default().fg(Color::Magenta).bold()),
            Span::styled(":quit", Style::default().fg(Color::DarkGray)),
        ]),
        StorageFocus::Templates => Line::from(vec![
            Span::styled(" c", Style::default().fg(Color::Green).bold()),
            Span::styled(":clone ", Style::default().fg(Color::DarkGray)),
            Span::styled("d", Style::default().fg(Color::Red).bold()),
            Span::styled(":delete ", Style::default().fg(Color::DarkGray)),
            Span::styled("n", Style::default().fg(Color::Green).bold()),
            Span::styled(":new ", Style::default().fg(Color::DarkGray)),
            Span::styled("i", Style::default().fg(Color::Green).bold()),
            Span::styled(":import ", Style::default().fg(Color::DarkGray)),
            Span::styled("S-Tab", Style::default().fg(Color::Magenta).bold()),
            Span::styled(":volumes ", Style::default().fg(Color::DarkGray)),
            Span::styled("q", Style::default().fg(Color::Magenta).bold()),
            Span::styled(":quit", Style::default().fg(Color::DarkGray)),
        ]),
    };
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

    // Status bar / Confirmation
    if let Some(name) = confirm_delete_volume {
        let confirm_line = Line::from(vec![
            Span::styled(" \u{26a0} ", Style::default().fg(Color::Red)),
            Span::styled(
                format!("Delete volume {}? ", name),
                Style::default().fg(Color::Red).bold(),
            ),
            Span::styled("[y]", Style::default().fg(Color::Green).bold()),
            Span::styled("es / ", Style::default().fg(Color::DarkGray)),
            Span::styled("[n]", Style::default().fg(Color::Red).bold()),
            Span::styled("o", Style::default().fg(Color::DarkGray)),
        ]);
        frame.render_widget(Paragraph::new(confirm_line), chunks[4]);
    } else if let Some(name) = confirm_delete_template {
        let confirm_line = Line::from(vec![
            Span::styled(" \u{26a0} ", Style::default().fg(Color::Red)),
            Span::styled(
                format!("Delete template {}? ", name),
                Style::default().fg(Color::Red).bold(),
            ),
            Span::styled("[y]", Style::default().fg(Color::Green).bold()),
            Span::styled("es / ", Style::default().fg(Color::DarkGray)),
            Span::styled("[n]", Style::default().fg(Color::Red).bold()),
            Span::styled("o", Style::default().fg(Color::DarkGray)),
        ]);
        frame.render_widget(Paragraph::new(confirm_line), chunks[4]);
    } else if let Some(status) = status_message {
        let color = if status.starts_with("Loading") {
            Color::DarkGray
        } else {
            Color::Yellow
        };
        let status_line = Line::from(vec![Span::styled(
            format!(" {}", status),
            Style::default().fg(color),
        )]);
        frame.render_widget(Paragraph::new(status_line), chunks[4]);
    }
}

fn draw_pool_stats(frame: &mut Frame, area: Rect, storage: &StorageState) {
    // Title (left side): mvirt [1:VMs] [2:Storage] [3:Networks] [4:Logs]
    let mut tabs = vec![
        Span::styled(" mvirt ", Style::default().fg(Color::Cyan).bold()),
        Span::styled("[1:VMs]", Style::default().fg(Color::DarkGray)),
        Span::raw(" "),
        Span::styled("[2:", Style::default().fg(Color::DarkGray)),
        Span::styled("Storage", Style::default().fg(Color::White).bold()),
        Span::styled("]", Style::default().fg(Color::DarkGray)),
    ];
    tabs.push(Span::styled(
        " [3:Networks]",
        Style::default().fg(Color::DarkGray),
    ));
    tabs.push(Span::styled(
        " [4:Logs]",
        Style::default().fg(Color::DarkGray),
    ));
    let title = Line::from(tabs);

    // Stats (right side): pool info
    let stats = if let Some(ref pool) = storage.pool {
        let used_pct = if pool.total_bytes > 0 {
            (pool.used_bytes as f64 / pool.total_bytes as f64 * 100.0) as u64
        } else {
            0
        };
        let usage_color = if used_pct > 80 {
            Color::Red
        } else if used_pct > 50 {
            Color::Yellow
        } else {
            Color::Green
        };

        Line::from(vec![
            Span::styled(format!("{} ", pool.name), Style::default().fg(Color::Cyan)),
            Span::styled(
                format_size(pool.used_bytes),
                Style::default().fg(usage_color).bold(),
            ),
            Span::styled(
                format!("/{} ({}%) ", format_size(pool.total_bytes), used_pct),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled("| Compress ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:.2}x ", pool.compression_ratio),
                Style::default().fg(Color::Green),
            ),
        ])
    } else {
        Line::from(vec![Span::styled(
            "loading... ",
            Style::default().fg(Color::DarkGray),
        )])
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    frame.render_widget(block.clone(), area);
    let inner = block.inner(area);
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(50)])
        .split(inner);
    frame.render_widget(Paragraph::new(title), chunks[0]);
    frame.render_widget(Paragraph::new(stats).alignment(Alignment::Right), chunks[1]);
}

fn draw_volumes_table(
    frame: &mut Frame,
    area: Rect,
    storage: &StorageState,
    table_state: &mut TableState,
    is_focused: bool,
) {
    let border_color = if is_focused {
        Color::Cyan
    } else {
        Color::DarkGray
    };

    let header = Row::new(vec![
        Cell::from("NAME").style(Style::default().fg(Color::Cyan)),
        Cell::from("SIZE").style(Style::default().fg(Color::Cyan)),
        Cell::from("USED").style(Style::default().fg(Color::Cyan)),
        Cell::from("SOURCE").style(Style::default().fg(Color::Cyan)),
        Cell::from("CREATED").style(Style::default().fg(Color::Cyan)),
    ])
    .style(Style::default().bold())
    .bottom_margin(1);

    let selected_idx = table_state.selected();

    // Build rows from volumes and active import jobs
    let mut rows: Vec<Row> = Vec::new();

    // Add completed volumes
    for (idx, vol) in storage.volumes.iter().enumerate() {
        let is_selected = is_focused && selected_idx == Some(idx);
        let bg = if is_selected {
            Color::Indexed(236)
        } else {
            Color::Reset
        };

        let created = vol.created_at.split('T').next().unwrap_or(&vol.created_at);

        rows.push(Row::new(vec![
            Cell::from(Span::styled(
                vol.name.clone(),
                Style::default()
                    .fg(if is_selected {
                        Color::White
                    } else {
                        Color::Reset
                    })
                    .bg(bg),
            )),
            Cell::from(Span::styled(
                format_size(vol.volsize_bytes),
                Style::default().fg(Color::DarkGray).bg(bg),
            )),
            Cell::from(Span::styled(
                format_size(vol.used_bytes),
                Style::default().fg(Color::DarkGray).bg(bg),
            )),
            Cell::from(Span::styled(
                "-",
                Style::default().fg(Color::DarkGray).bg(bg),
            )),
            Cell::from(Span::styled(
                created.to_string(),
                Style::default().fg(Color::DarkGray).bg(bg),
            )),
        ]));
    }

    // Add active import jobs
    for job in &storage.import_jobs {
        let state = ImportJobState::try_from(job.state).unwrap_or(ImportJobState::Unspecified);
        if matches!(
            state,
            ImportJobState::Pending
                | ImportJobState::Downloading
                | ImportJobState::Converting
                | ImportJobState::Writing
        ) {
            let progress = if job.total_bytes > 0 {
                (job.bytes_written as f64 / job.total_bytes as f64 * 100.0) as u64
            } else {
                0
            };

            let state_str = match state {
                ImportJobState::Pending => "pending",
                ImportJobState::Downloading => "download",
                ImportJobState::Converting => "convert",
                ImportJobState::Writing => "writing",
                _ => "...",
            };

            // Progress bar
            let bar_width = 10;
            let filled = (progress as usize * bar_width / 100).min(bar_width);
            let empty = bar_width - filled;
            let progress_bar = format!(
                "[{}{}] {}%",
                "\u{2588}".repeat(filled),
                "\u{2591}".repeat(empty),
                progress
            );

            rows.push(Row::new(vec![
                Cell::from(Span::styled(
                    format!("[{}] {}", state_str, job.template_name),
                    Style::default().fg(Color::Yellow),
                )),
                Cell::from(Span::styled(
                    format_size(job.total_bytes),
                    Style::default().fg(Color::DarkGray),
                )),
                Cell::from(Span::styled(progress_bar, Style::default().fg(Color::Cyan))),
                Cell::from(Span::styled(
                    truncate_source(&job.source),
                    Style::default().fg(Color::DarkGray),
                )),
                Cell::from(Span::styled("-", Style::default().fg(Color::DarkGray))),
            ]));
        }
    }

    let table = Table::new(
        rows,
        [
            Constraint::Min(15),
            Constraint::Length(10),
            Constraint::Length(20),
            Constraint::Length(20),
            Constraint::Length(12),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .title(" Volumes ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color)),
    )
    .row_highlight_style(Style::default().bg(Color::Indexed(236)));

    frame.render_stateful_widget(table, area, table_state);
}

fn draw_templates_table(
    frame: &mut Frame,
    area: Rect,
    storage: &StorageState,
    table_state: &mut TableState,
    is_focused: bool,
) {
    let border_color = if is_focused {
        Color::Cyan
    } else {
        Color::DarkGray
    };

    let header = Row::new(vec![
        Cell::from("NAME").style(Style::default().fg(Color::Cyan)),
        Cell::from("SIZE").style(Style::default().fg(Color::Cyan)),
        Cell::from("CLONES").style(Style::default().fg(Color::Cyan)),
        Cell::from("CREATED").style(Style::default().fg(Color::Cyan)),
    ])
    .style(Style::default().bold())
    .bottom_margin(1);

    let selected_idx = table_state.selected();
    let rows: Vec<Row> = storage
        .templates
        .iter()
        .enumerate()
        .map(|(idx, tpl)| {
            let is_selected = is_focused && selected_idx == Some(idx);
            let bg = if is_selected {
                Color::Indexed(236)
            } else {
                Color::Reset
            };

            let created = tpl.created_at.split('T').next().unwrap_or(&tpl.created_at);

            Row::new(vec![
                Cell::from(Span::styled(
                    tpl.name.clone(),
                    Style::default()
                        .fg(if is_selected {
                            Color::White
                        } else {
                            Color::Reset
                        })
                        .bg(bg),
                )),
                Cell::from(Span::styled(
                    format_size(tpl.size_bytes),
                    Style::default().fg(Color::DarkGray).bg(bg),
                )),
                Cell::from(Span::styled(
                    tpl.clone_count.to_string(),
                    Style::default().fg(Color::DarkGray).bg(bg),
                )),
                Cell::from(Span::styled(
                    created.to_string(),
                    Style::default().fg(Color::DarkGray).bg(bg),
                )),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Min(20),
            Constraint::Length(10),
            Constraint::Length(8),
            Constraint::Length(12),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .title(" Templates ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color)),
    )
    .row_highlight_style(Style::default().bg(Color::Indexed(236)));

    frame.render_stateful_widget(table, area, table_state);
}

fn truncate_source(source: &str) -> String {
    if source.len() > 18 {
        format!("{}...", &source[..15])
    } else {
        source.to_string()
    }
}
