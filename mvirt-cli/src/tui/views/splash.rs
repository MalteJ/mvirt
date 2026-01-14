use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

pub fn draw(frame: &mut Frame) {
    let area = frame.area();

    // Each line padded to same width for consistent centering
    let ascii_art: &[&str] = &[
        "                              ███             █████   ",
        "                             ░░░             ░░███    ",
        " █████████████   █████ █████ ████  ████████  ███████  ",
        "░░███░░███░░███ ░░███ ░░███ ░░███ ░░███░░███░░░███░   ",
        " ░███ ░███ ░███  ░███  ░███  ░███  ░███ ░░░   ░███    ",
        " ░███ ░███ ░███  ░░███ ███   ░███  ░███       ░███ ███",
        " █████░███ █████  ░░█████    █████ █████      ░░█████ ",
        "░░░░░ ░░░ ░░░░░    ░░░░░    ░░░░░ ░░░░░        ░░░░░  ",
    ];

    let art_height = ascii_art.len() as u16;
    let start_y = (area.height.saturating_sub(art_height)) / 2;

    let lines: Vec<Line> = ascii_art
        .iter()
        .map(|line| Line::from(Span::styled(*line, Style::default().fg(Color::Cyan))))
        .collect();

    let splash_area = Rect {
        x: area.x,
        y: start_y,
        width: area.width,
        height: art_height.min(area.height),
    };

    frame.render_widget(
        Paragraph::new(lines).alignment(ratatui::prelude::Alignment::Center),
        splash_area,
    );

    // Rainbow pride bar at the bottom
    let rainbow_colors: [Color; 6] = [
        Color::Indexed(196), // Red
        Color::Indexed(208), // Orange
        Color::Indexed(226), // Yellow
        Color::Indexed(46),  // Green
        Color::Indexed(21),  // Blue
        Color::Indexed(129), // Purple
    ];
    let section_width = area.width as f32 / rainbow_colors.len() as f32;
    let rainbow_line = Line::from(
        (0..area.width)
            .map(|i| {
                let color_idx = (i as f32 / section_width) as usize;
                let color = rainbow_colors[color_idx.min(rainbow_colors.len() - 1)];
                Span::styled("█", Style::default().fg(color))
            })
            .collect::<Vec<_>>(),
    );

    let bottom_row = Rect {
        x: area.x,
        y: area.height.saturating_sub(1),
        width: area.width,
        height: 1,
    };
    frame.render_widget(Paragraph::new(rainbow_line), bottom_row);
}
