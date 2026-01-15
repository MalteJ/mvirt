use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};

use crate::net_proto::NicState;
use crate::tui::types::{NetworkFocus, NetworkState};

#[allow(clippy::too_many_arguments)]
pub fn draw(
    frame: &mut Frame,
    network: &NetworkState,
    networks_table_state: &mut TableState,
    nics_table_state: &mut TableState,
    focus: NetworkFocus,
    status_message: Option<&str>,
    confirm_delete_network: Option<&str>,
    confirm_delete_nic: Option<&str>,
    last_refresh: Option<chrono::DateTime<chrono::Local>>,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Title bar
            Constraint::Min(8),     // Networks table
            Constraint::Length(10), // NICs table
            Constraint::Length(1),  // Legend
            Constraint::Length(1),  // Status
        ])
        .split(frame.area());

    // Title bar
    draw_title_bar(frame, chunks[0]);

    // Networks table
    draw_networks_table(
        frame,
        chunks[1],
        network,
        networks_table_state,
        focus == NetworkFocus::Networks,
    );

    // NICs table
    draw_nics_table(
        frame,
        chunks[2],
        network,
        nics_table_state,
        focus == NetworkFocus::Nics,
    );

    // Legend - context-sensitive based on focus
    let legend_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(12)])
        .split(chunks[3]);

    let legend = match focus {
        NetworkFocus::Networks => Line::from(vec![
            Span::styled(" n", Style::default().fg(Color::Green).bold()),
            Span::styled(":new ", Style::default().fg(Color::DarkGray)),
            Span::styled("d", Style::default().fg(Color::Red).bold()),
            Span::styled(":delete ", Style::default().fg(Color::DarkGray)),
            Span::styled("Enter", Style::default().fg(Color::Cyan).bold()),
            Span::styled(":select ", Style::default().fg(Color::DarkGray)),
            Span::styled("S-Tab", Style::default().fg(Color::Magenta).bold()),
            Span::styled(":NICs ", Style::default().fg(Color::DarkGray)),
            Span::styled("q", Style::default().fg(Color::Magenta).bold()),
            Span::styled(":quit", Style::default().fg(Color::DarkGray)),
        ]),
        NetworkFocus::Nics => Line::from(vec![
            Span::styled(" c", Style::default().fg(Color::Green).bold()),
            Span::styled(":create ", Style::default().fg(Color::DarkGray)),
            Span::styled("d", Style::default().fg(Color::Red).bold()),
            Span::styled(":delete ", Style::default().fg(Color::DarkGray)),
            Span::styled("S-Tab", Style::default().fg(Color::Magenta).bold()),
            Span::styled(":Networks ", Style::default().fg(Color::DarkGray)),
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
    if let Some(name) = confirm_delete_network {
        let confirm_line = Line::from(vec![
            Span::styled(" \u{26a0} ", Style::default().fg(Color::Red)),
            Span::styled(
                format!("Delete network {}? ", name),
                Style::default().fg(Color::Red).bold(),
            ),
            Span::styled("[y]", Style::default().fg(Color::Green).bold()),
            Span::styled("es / ", Style::default().fg(Color::DarkGray)),
            Span::styled("[n]", Style::default().fg(Color::Red).bold()),
            Span::styled("o", Style::default().fg(Color::DarkGray)),
        ]);
        frame.render_widget(Paragraph::new(confirm_line), chunks[4]);
    } else if let Some(name) = confirm_delete_nic {
        let confirm_line = Line::from(vec![
            Span::styled(" \u{26a0} ", Style::default().fg(Color::Red)),
            Span::styled(
                format!("Delete NIC {}? ", name),
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

fn draw_title_bar(frame: &mut Frame, area: Rect) {
    let mut tabs = vec![
        Span::styled(" mvirt ", Style::default().fg(Color::Cyan).bold()),
        Span::styled("[1:VMs]", Style::default().fg(Color::DarkGray)),
        Span::raw(" "),
        Span::styled("[2:Storage]", Style::default().fg(Color::DarkGray)),
    ];
    tabs.push(Span::raw(" "));
    tabs.push(Span::styled("[3:", Style::default().fg(Color::DarkGray)));
    tabs.push(Span::styled(
        "Networks",
        Style::default().fg(Color::White).bold(),
    ));
    tabs.push(Span::styled("]", Style::default().fg(Color::DarkGray)));
    tabs.push(Span::styled(
        " [4:Logs]",
        Style::default().fg(Color::DarkGray),
    ));
    let title = Line::from(tabs);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    frame.render_widget(block.clone(), area);
    let inner = block.inner(area);
    frame.render_widget(Paragraph::new(title), inner);
}

fn draw_networks_table(
    frame: &mut Frame,
    area: Rect,
    network: &NetworkState,
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
        Cell::from("IPv4 SUBNET").style(Style::default().fg(Color::Cyan)),
        Cell::from("IPv6 PREFIX").style(Style::default().fg(Color::Cyan)),
        Cell::from("NICs").style(Style::default().fg(Color::Cyan)),
    ])
    .style(Style::default().bold())
    .bottom_margin(1);

    let selected_idx = table_state.selected();
    let rows: Vec<Row> = network
        .networks
        .iter()
        .enumerate()
        .map(|(idx, net)| {
            let is_selected = is_focused && selected_idx == Some(idx);
            let bg = if is_selected {
                Color::Indexed(236)
            } else {
                Color::Reset
            };

            let short_id = format!("{}\u{2026}", &net.id[..8]);

            let ipv4 = if net.ipv4_enabled && !net.ipv4_subnet.is_empty() {
                net.ipv4_subnet.clone()
            } else {
                "-".to_string()
            };

            let ipv6 = if net.ipv6_enabled && !net.ipv6_prefix.is_empty() {
                net.ipv6_prefix.clone()
            } else {
                "-".to_string()
            };

            Row::new(vec![
                Cell::from(Span::styled(
                    short_id,
                    Style::default().fg(Color::DarkGray).bg(bg),
                )),
                Cell::from(Span::styled(
                    net.name.clone(),
                    Style::default()
                        .fg(if is_selected {
                            Color::White
                        } else {
                            Color::Reset
                        })
                        .bg(bg),
                )),
                Cell::from(Span::styled(
                    ipv4,
                    Style::default().fg(Color::DarkGray).bg(bg),
                )),
                Cell::from(Span::styled(
                    ipv6,
                    Style::default().fg(Color::DarkGray).bg(bg),
                )),
                Cell::from(Span::styled(
                    net.nic_count.to_string(),
                    Style::default().fg(Color::DarkGray).bg(bg),
                )),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(11),
            Constraint::Min(15),
            Constraint::Length(18),
            Constraint::Length(20),
            Constraint::Length(6),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .title(" Networks ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color)),
    )
    .row_highlight_style(Style::default().bg(Color::Indexed(236)));

    frame.render_stateful_widget(table, area, table_state);
}

fn draw_nics_table(
    frame: &mut Frame,
    area: Rect,
    network: &NetworkState,
    table_state: &mut TableState,
    is_focused: bool,
) {
    let border_color = if is_focused {
        Color::Cyan
    } else {
        Color::DarkGray
    };

    let title = if let Some(ref net_id) = network.selected_network_id {
        // Find network name
        let name = network
            .networks
            .iter()
            .find(|n| n.id == *net_id)
            .map(|n| n.name.as_str())
            .unwrap_or("?");
        format!(" NICs in \"{}\" ", name)
    } else {
        " NICs (select a network) ".to_string()
    };

    let header = Row::new(vec![
        Cell::from("ID").style(Style::default().fg(Color::Cyan)),
        Cell::from("NAME").style(Style::default().fg(Color::Cyan)),
        Cell::from("MAC").style(Style::default().fg(Color::Cyan)),
        Cell::from("IPv4").style(Style::default().fg(Color::Cyan)),
        Cell::from("IPv6").style(Style::default().fg(Color::Cyan)),
        Cell::from("ST").style(Style::default().fg(Color::Cyan)),
    ])
    .style(Style::default().bold())
    .bottom_margin(1);

    let selected_idx = table_state.selected();
    let rows: Vec<Row> = network
        .nics
        .iter()
        .enumerate()
        .map(|(idx, nic)| {
            let is_selected = is_focused && selected_idx == Some(idx);
            let bg = if is_selected {
                Color::Indexed(236)
            } else {
                Color::Reset
            };

            let short_id = format!("{}\u{2026}", &nic.id[..8]);

            let state = NicState::try_from(nic.state).unwrap_or(NicState::Unspecified);
            let state_indicator = match state {
                NicState::Active => Span::styled("\u{25cf}", Style::default().fg(Color::Green)),
                NicState::Created => Span::styled("\u{25cb}", Style::default().fg(Color::Yellow)),
                NicState::Error => Span::styled("\u{25cf}", Style::default().fg(Color::Red)),
                NicState::Unspecified => Span::styled("?", Style::default().fg(Color::DarkGray)),
            };

            let name = if nic.name.is_empty() {
                "-".to_string()
            } else {
                nic.name.clone()
            };

            let ipv4 = if nic.ipv4_address.is_empty() {
                "-".to_string()
            } else {
                nic.ipv4_address.clone()
            };

            let ipv6 = if nic.ipv6_address.is_empty() {
                "-".to_string()
            } else {
                // Truncate long IPv6 addresses
                if nic.ipv6_address.len() > 16 {
                    format!("{}...", &nic.ipv6_address[..13])
                } else {
                    nic.ipv6_address.clone()
                }
            };

            Row::new(vec![
                Cell::from(Span::styled(
                    short_id,
                    Style::default().fg(Color::DarkGray).bg(bg),
                )),
                Cell::from(Span::styled(
                    name,
                    Style::default()
                        .fg(if is_selected {
                            Color::White
                        } else {
                            Color::Reset
                        })
                        .bg(bg),
                )),
                Cell::from(Span::styled(
                    nic.mac_address.clone(),
                    Style::default().fg(Color::DarkGray).bg(bg),
                )),
                Cell::from(Span::styled(
                    ipv4,
                    Style::default().fg(Color::DarkGray).bg(bg),
                )),
                Cell::from(Span::styled(
                    ipv6,
                    Style::default().fg(Color::DarkGray).bg(bg),
                )),
                Cell::from(state_indicator),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(11),
            Constraint::Min(10),
            Constraint::Length(19),
            Constraint::Length(15),
            Constraint::Length(17),
            Constraint::Length(3),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color)),
    )
    .row_highlight_style(Style::default().bg(Color::Indexed(236)));

    frame.render_stateful_widget(table, area, table_state);
}
