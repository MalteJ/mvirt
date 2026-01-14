use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};

use crate::proto::{SystemInfo, Vm};
use crate::tui::modals::vm_detail::{format_state, state_style};

#[allow(clippy::too_many_arguments)]
pub fn draw(
    frame: &mut Frame,
    vms: &[Vm],
    table_state: &mut TableState,
    system_info: Option<&SystemInfo>,
    status_message: Option<&str>,
    confirm_delete: Option<&str>,
    confirm_kill: Option<&str>,
    last_refresh: Option<chrono::DateTime<chrono::Local>>,
    zfs_available: bool,
) {
    let get_vm_by_id = |id: &str| vms.iter().find(|vm| vm.id == id);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(frame.area());

    // Title bar with system resource info and tabs
    let tabs = if zfs_available {
        vec![
            Span::styled("[", Style::default().fg(Color::DarkGray)),
            Span::styled("VMs", Style::default().fg(Color::White).bold()),
            Span::styled("]", Style::default().fg(Color::DarkGray)),
            Span::styled(" Storage ", Style::default().fg(Color::DarkGray)),
        ]
    } else {
        vec![
            Span::styled("[", Style::default().fg(Color::DarkGray)),
            Span::styled("VMs", Style::default().fg(Color::White).bold()),
            Span::styled("]", Style::default().fg(Color::DarkGray)),
        ]
    };

    // Title (left side): mvirt [VMs] Storage
    let mut title_spans = vec![Span::styled(
        " mvirt ",
        Style::default().fg(Color::Cyan).bold(),
    )];
    title_spans.extend(tabs);
    let title = Line::from(title_spans);

    // Stats (right side): CPU and RAM info
    let stats_text = if let Some(info) = system_info {
        let cpu_color = if info.allocated_cpus > info.total_cpus * 8 / 10 {
            Color::Red
        } else if info.allocated_cpus > info.total_cpus / 2 {
            Color::Yellow
        } else {
            Color::Green
        };
        let mem_color = if info.allocated_memory_mb > info.total_memory_mb * 8 / 10 {
            Color::Red
        } else if info.allocated_memory_mb > info.total_memory_mb / 2 {
            Color::Yellow
        } else {
            Color::Green
        };
        let total_mem_gib = info.total_memory_mb as f64 / 1024.0;
        let alloc_mem_gib = info.allocated_memory_mb as f64 / 1024.0;

        Line::from(vec![
            Span::styled(
                format!("Load {:.2} ", info.load_1),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled("| CPU ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{}", info.allocated_cpus),
                Style::default().fg(cpu_color).bold(),
            ),
            Span::styled(
                format!("/{} ", info.total_cpus),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled("| RAM ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:.1}", alloc_mem_gib),
                Style::default().fg(mem_color).bold(),
            ),
            Span::styled(
                format!("/{:.1} GiB ", total_mem_gib),
                Style::default().fg(Color::DarkGray),
            ),
        ])
    } else {
        Line::from(vec![Span::styled(
            "loading... ",
            Style::default().fg(Color::DarkGray),
        )])
    };

    let title_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    frame.render_widget(title_block.clone(), chunks[0]);
    let title_inner = title_block.inner(chunks[0]);
    let title_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(50)])
        .split(title_inner);
    frame.render_widget(Paragraph::new(title), title_chunks[0]);
    frame.render_widget(
        Paragraph::new(stats_text).alignment(ratatui::prelude::Alignment::Right),
        title_chunks[1],
    );

    // VM Table
    let header = Row::new(vec![
        Cell::from("ID").style(Style::default().fg(Color::Cyan)),
        Cell::from("NAME").style(Style::default().fg(Color::Cyan)),
        Cell::from("STATE").style(Style::default().fg(Color::Cyan)),
        Cell::from("CPU").style(Style::default().fg(Color::Cyan)),
        Cell::from("MEM").style(Style::default().fg(Color::Cyan)),
    ])
    .style(Style::default().bold())
    .bottom_margin(1);

    let selected_idx = table_state.selected();
    let rows: Vec<Row> = vms
        .iter()
        .enumerate()
        .map(|(idx, vm)| {
            let config = vm.config.as_ref();
            let state = vm.state;
            let is_selected = selected_idx == Some(idx);
            let bg = if is_selected {
                Color::Indexed(236)
            } else {
                Color::Reset
            };

            Row::new(vec![
                Cell::from(Span::styled(
                    format!("{}\u{2026}", &vm.id[..8]),
                    Style::default().fg(Color::DarkGray).bg(bg),
                )),
                Cell::from(Span::styled(
                    vm.name.clone().unwrap_or_else(|| "-".to_string()),
                    Style::default()
                        .fg(if is_selected {
                            Color::White
                        } else {
                            Color::Reset
                        })
                        .bg(bg),
                )),
                Cell::from(Span::styled(format_state(state), state_style(state).bg(bg))),
                Cell::from(Span::styled(
                    config.map(|c| c.vcpus.to_string()).unwrap_or_default(),
                    Style::default()
                        .fg(if is_selected {
                            Color::White
                        } else {
                            Color::Reset
                        })
                        .bg(bg),
                )),
                Cell::from(Span::styled(
                    config
                        .map(|c| format!("{} MB", c.memory_mb))
                        .unwrap_or_default(),
                    Style::default()
                        .fg(if is_selected {
                            Color::White
                        } else {
                            Color::Reset
                        })
                        .bg(bg),
                )),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(11),
            Constraint::Min(15),
            Constraint::Length(12),
            Constraint::Length(5),
            Constraint::Length(10),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    )
    .row_highlight_style(Style::default().bg(Color::Indexed(236)));

    frame.render_stateful_widget(table, chunks[1], table_state);

    // Hotkey legend with refresh time
    let legend_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(12)])
        .split(chunks[2]);

    let legend = Line::from(vec![
        Span::styled(" \u{21b5}", Style::default().fg(Color::White).bold()),
        Span::styled(" Details ", Style::default().fg(Color::DarkGray)),
        Span::styled("n", Style::default().fg(Color::Cyan).bold()),
        Span::styled(" New ", Style::default().fg(Color::DarkGray)),
        Span::styled("c", Style::default().fg(Color::Cyan).bold()),
        Span::styled(" Console ", Style::default().fg(Color::DarkGray)),
        Span::styled("s", Style::default().fg(Color::Green).bold()),
        Span::styled(" Start ", Style::default().fg(Color::DarkGray)),
        Span::styled("S", Style::default().fg(Color::Yellow).bold()),
        Span::styled(" Stop ", Style::default().fg(Color::DarkGray)),
        Span::styled("k", Style::default().fg(Color::Red).bold()),
        Span::styled(" Kill ", Style::default().fg(Color::DarkGray)),
        Span::styled("d", Style::default().fg(Color::Red).bold()),
        Span::styled(" Delete ", Style::default().fg(Color::DarkGray)),
        Span::styled("q", Style::default().fg(Color::Magenta).bold()),
        Span::styled(" Quit", Style::default().fg(Color::DarkGray)),
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
        Paragraph::new(refresh_text).alignment(ratatui::prelude::Alignment::Right),
        legend_chunks[1],
    );

    // Status bar / Confirmation
    if let Some(id) = confirm_kill {
        let vm_display = if let Some(vm) = get_vm_by_id(id) {
            format!(
                "{} ({}\u{2026})",
                vm.name.as_deref().unwrap_or(&id[..8]),
                &id[..8]
            )
        } else {
            format!("{}\u{2026}", &id[..8])
        };
        let confirm_line = Line::from(vec![
            Span::styled(" \u{26a0} ", Style::default().fg(Color::Red)),
            Span::styled(
                format!("Kill VM {}? ", vm_display),
                Style::default().fg(Color::Red).bold(),
            ),
            Span::styled("[y]", Style::default().fg(Color::Green).bold()),
            Span::styled("es / ", Style::default().fg(Color::DarkGray)),
            Span::styled("[n]", Style::default().fg(Color::Red).bold()),
            Span::styled("o", Style::default().fg(Color::DarkGray)),
        ]);
        frame.render_widget(Paragraph::new(confirm_line), chunks[3]);
    } else if let Some(id) = confirm_delete {
        let vm_display = if let Some(vm) = get_vm_by_id(id) {
            format!(
                "{} ({}\u{2026})",
                vm.name.as_deref().unwrap_or(&id[..8]),
                &id[..8]
            )
        } else {
            format!("{}\u{2026}", &id[..8])
        };
        let confirm_line = Line::from(vec![
            Span::styled(" \u{26a0} ", Style::default().fg(Color::Red)),
            Span::styled(
                format!("Delete VM {}? ", vm_display),
                Style::default().fg(Color::Red).bold(),
            ),
            Span::styled("[y]", Style::default().fg(Color::Green).bold()),
            Span::styled("es / ", Style::default().fg(Color::DarkGray)),
            Span::styled("[n]", Style::default().fg(Color::Red).bold()),
            Span::styled("o", Style::default().fg(Color::DarkGray)),
        ]);
        frame.render_widget(Paragraph::new(confirm_line), chunks[3]);
    } else if let Some(status) = status_message {
        let status_line = Line::from(vec![Span::styled(
            format!(" {}", status),
            Style::default().fg(Color::Yellow),
        )]);
        frame.render_widget(Paragraph::new(status_line), chunks[3]);
    }
}
