use std::collections::HashSet;

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};

use crate::tui::types::{StorageFocus, StorageState, VolumeSelection};
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
    volume_selection: &Option<VolumeSelection>,
    template_table_state: &mut TableState,
    focus: StorageFocus,
    status_message: Option<&str>,
    confirm_delete_volume: Option<&str>,
    confirm_delete_template: Option<&str>,
    confirm_delete_snapshot: Option<(&str, &str)>,
    confirm_rollback_snapshot: Option<(&str, &str)>,
    last_refresh: Option<chrono::DateTime<chrono::Local>>,
    expanded_volumes: &HashSet<String>,
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
        volume_selection,
        focus == StorageFocus::Volumes,
        expanded_volumes,
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
            Span::styled(" ↵", Style::default().fg(Color::Cyan).bold()),
            Span::styled(":detail ", Style::default().fg(Color::DarkGray)),
            Span::styled("␣", Style::default().fg(Color::Cyan).bold()),
            Span::styled(":expand ", Style::default().fg(Color::DarkGray)),
            Span::styled("n", Style::default().fg(Color::Green).bold()),
            Span::styled(":new ", Style::default().fg(Color::DarkGray)),
            Span::styled("s", Style::default().fg(Color::Cyan).bold()),
            Span::styled(":snap ", Style::default().fg(Color::DarkGray)),
            Span::styled("t", Style::default().fg(Color::Green).bold()),
            Span::styled(":tmpl ", Style::default().fg(Color::DarkGray)),
            Span::styled("d", Style::default().fg(Color::Red).bold()),
            Span::styled(":del ", Style::default().fg(Color::DarkGray)),
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
    } else if let Some((vol, snap)) = confirm_delete_snapshot {
        let confirm_line = Line::from(vec![
            Span::styled(" \u{26a0} ", Style::default().fg(Color::Red)),
            Span::styled(
                format!("Delete snapshot {}@{}? ", vol, snap),
                Style::default().fg(Color::Red).bold(),
            ),
            Span::styled("[y]", Style::default().fg(Color::Green).bold()),
            Span::styled("es / ", Style::default().fg(Color::DarkGray)),
            Span::styled("[n]", Style::default().fg(Color::Red).bold()),
            Span::styled("o", Style::default().fg(Color::DarkGray)),
        ]);
        frame.render_widget(Paragraph::new(confirm_line), chunks[4]);
    } else if let Some((vol, snap)) = confirm_rollback_snapshot {
        let confirm_line = Line::from(vec![
            Span::styled(" \u{26a0} ", Style::default().fg(Color::Yellow)),
            Span::styled(
                format!("Rollback to {}@{}? ", vol, snap),
                Style::default().fg(Color::Yellow).bold(),
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
    tabs.push(Span::styled(
        " [5:System]",
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
    volume_selection: &Option<VolumeSelection>,
    is_focused: bool,
    expanded_volumes: &HashSet<String>,
) {
    let border_color = if is_focused {
        Color::Cyan
    } else {
        Color::DarkGray
    };

    let header = Row::new(vec![
        Cell::from("").style(Style::default().fg(Color::Cyan)),
        Cell::from("ID").style(Style::default().fg(Color::Cyan)),
        Cell::from("NAME").style(Style::default().fg(Color::Cyan)),
        Cell::from("SIZE").style(Style::default().fg(Color::Cyan)),
        Cell::from("USED").style(Style::default().fg(Color::Cyan)),
        Cell::from("CREATED").style(Style::default().fg(Color::Cyan)),
    ])
    .style(Style::default().bold())
    .bottom_margin(1);

    // Build rows from volumes (with snapshots) and active import jobs
    let mut rows: Vec<Row> = Vec::new();
    let mut selected_row: Option<usize> = None;

    // Add completed volumes
    for (vol_idx, vol) in storage.volumes.iter().enumerate() {
        let is_volume_selected = is_focused
            && matches!(volume_selection, Some(VolumeSelection::Volume(i)) if *i == vol_idx);
        let bg = if is_volume_selected {
            Color::Indexed(236)
        } else {
            Color::Reset
        };

        let is_expanded = expanded_volumes.contains(&vol.id);
        let has_snapshots = !vol.snapshots.is_empty();

        // Expand/collapse indicator
        let expand_indicator = if has_snapshots {
            if is_expanded { "▼" } else { "▶" }
        } else {
            " "
        };

        let created = vol.created_at.split('T').next().unwrap_or(&vol.created_at);
        let short_id = format!("{}\u{2026}", &vol.id[..8]);

        if is_volume_selected {
            selected_row = Some(rows.len());
        }

        rows.push(Row::new(vec![
            Cell::from(Span::styled(
                expand_indicator,
                Style::default().fg(Color::DarkGray).bg(bg),
            )),
            Cell::from(Span::styled(
                short_id,
                Style::default().fg(Color::DarkGray).bg(bg),
            )),
            Cell::from(Span::styled(
                vol.name.clone(),
                Style::default()
                    .fg(if is_volume_selected {
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
                created.to_string(),
                Style::default().fg(Color::DarkGray).bg(bg),
            )),
        ]));

        // Add snapshot rows if expanded
        if is_expanded {
            for (snap_idx, snap) in vol.snapshots.iter().enumerate() {
                let is_snap_selected = is_focused
                    && matches!(volume_selection, Some(VolumeSelection::Snapshot(vi, si)) if *vi == vol_idx && *si == snap_idx);
                let snap_bg = if is_snap_selected {
                    Color::Indexed(236)
                } else {
                    Color::Reset
                };

                if is_snap_selected {
                    selected_row = Some(rows.len());
                }

                let snap_short_id = format!("{}\u{2026}", &snap.id[..8.min(snap.id.len())]);

                rows.push(Row::new(vec![
                    Cell::from(Span::styled("", Style::default().bg(snap_bg))),
                    Cell::from(Span::styled(
                        snap_short_id,
                        Style::default().fg(Color::DarkGray).bg(snap_bg),
                    )),
                    Cell::from(Span::styled(
                        format!("  └─ @{}", snap.name),
                        Style::default()
                            .fg(if is_snap_selected {
                                Color::White
                            } else {
                                Color::DarkGray
                            })
                            .bg(snap_bg),
                    )),
                    Cell::from(Span::styled("", Style::default().bg(snap_bg))),
                    Cell::from(Span::styled(
                        format_size(snap.used_bytes),
                        Style::default().fg(Color::DarkGray).bg(snap_bg),
                    )),
                    Cell::from(Span::styled(
                        snap.created_at.split('T').next().unwrap_or("-"),
                        Style::default().fg(Color::DarkGray).bg(snap_bg),
                    )),
                ]));
            }
        }
    }

    // Set up display state for the table
    let mut display_state = TableState::default();
    display_state.select(selected_row);

    let table = Table::new(
        rows,
        [
            Constraint::Length(2),
            Constraint::Length(11),
            Constraint::Min(15),
            Constraint::Length(10),
            Constraint::Length(10),
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

    frame.render_stateful_widget(table, area, &mut display_state);
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
        Cell::from("ID").style(Style::default().fg(Color::Cyan)),
        Cell::from("NAME").style(Style::default().fg(Color::Cyan)),
        Cell::from("SIZE").style(Style::default().fg(Color::Cyan)),
        Cell::from("STATUS").style(Style::default().fg(Color::Cyan)),
    ])
    .style(Style::default().bold())
    .bottom_margin(1);

    let selected_idx = table_state.selected();
    let mut rows: Vec<Row> = Vec::new();

    // Add active import jobs first
    for job in &storage.import_jobs {
        let state = ImportJobState::try_from(job.state).unwrap_or(ImportJobState::Unspecified);
        if matches!(
            state,
            ImportJobState::Pending
                | ImportJobState::Downloading
                | ImportJobState::Converting
                | ImportJobState::Writing
        ) {
            let status_str = match state {
                ImportJobState::Pending => "pending",
                ImportJobState::Downloading => "downloading",
                ImportJobState::Converting => "converting",
                ImportJobState::Writing => "writing",
                _ => "...",
            };

            let short_id = format!("{}\u{2026}", &job.id[..8]);

            rows.push(Row::new(vec![
                Cell::from(Span::styled(short_id, Style::default().fg(Color::DarkGray))),
                Cell::from(Span::styled(
                    job.template_name.clone(),
                    Style::default().fg(Color::Yellow),
                )),
                Cell::from(Span::styled(
                    if job.total_bytes > 0 {
                        format_size(job.total_bytes)
                    } else {
                        "-".to_string()
                    },
                    Style::default().fg(Color::DarkGray),
                )),
                Cell::from(Span::styled(status_str, Style::default().fg(Color::Yellow))),
            ]));
        }
    }

    // Add existing templates
    let import_count = rows.len();
    for (idx, tpl) in storage.templates.iter().enumerate() {
        let is_selected = is_focused && selected_idx == Some(idx + import_count);
        let bg = if is_selected {
            Color::Indexed(236)
        } else {
            Color::Reset
        };

        let short_id = format!("{}\u{2026}", &tpl.id[..8]);

        rows.push(Row::new(vec![
            Cell::from(Span::styled(
                short_id,
                Style::default().fg(Color::DarkGray).bg(bg),
            )),
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
                "ready",
                Style::default().fg(Color::Green).bg(bg),
            )),
        ]));
    }

    let table = Table::new(
        rows,
        [
            Constraint::Length(11),
            Constraint::Min(15),
            Constraint::Length(10),
            Constraint::Length(16),
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
