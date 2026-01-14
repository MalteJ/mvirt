use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use tokio::sync::mpsc;

use crate::tui::types::EscapeState;

pub struct ConsoleSession {
    pub vm_id: String,
    pub vm_name: Option<String>,
    pub parser: vt100::Parser,
    pub escape_state: EscapeState,
    pub input_tx: mpsc::UnboundedSender<Vec<u8>>,
}

impl ConsoleSession {
    pub fn new(
        vm_id: String,
        vm_name: Option<String>,
        input_tx: mpsc::UnboundedSender<Vec<u8>>,
    ) -> Self {
        Self {
            vm_id,
            vm_name,
            parser: vt100::Parser::new(24, 80, 10000),
            escape_state: EscapeState::Normal,
            input_tx,
        }
    }

    pub fn process_output(&mut self, data: &[u8]) {
        self.parser.process(data);
    }

    pub fn handle_key(&mut self, key_code: KeyCode, modifiers: KeyModifiers) -> bool {
        match self.escape_state {
            EscapeState::Normal => {
                if modifiers.contains(KeyModifiers::CONTROL)
                    && let KeyCode::Char('a') | KeyCode::Char('A') = key_code
                {
                    self.escape_state = EscapeState::SawCtrlA;
                    return false;
                }
            }
            EscapeState::SawCtrlA => {
                self.escape_state = EscapeState::Normal;
                if let KeyCode::Char('t') | KeyCode::Char('T') = key_code {
                    return true; // Signal to close console
                }
                let _ = self.input_tx.send(vec![0x01]);
            }
        }

        let data: Option<Vec<u8>> = match key_code {
            KeyCode::Char(c) => {
                if modifiers.contains(KeyModifiers::CONTROL) {
                    let ctrl_char = (c.to_ascii_lowercase() as u8)
                        .wrapping_sub(b'a')
                        .wrapping_add(1);
                    Some(vec![ctrl_char])
                } else {
                    let mut buf = [0u8; 4];
                    let s = c.encode_utf8(&mut buf);
                    Some(s.as_bytes().to_vec())
                }
            }
            KeyCode::Enter => Some(vec![b'\r']),
            KeyCode::Backspace => Some(vec![0x7f]),
            KeyCode::Tab => Some(vec![b'\t']),
            KeyCode::Esc => Some(vec![0x1b]),
            KeyCode::Up => Some(b"\x1b[A".to_vec()),
            KeyCode::Down => Some(b"\x1b[B".to_vec()),
            KeyCode::Right => Some(b"\x1b[C".to_vec()),
            KeyCode::Left => Some(b"\x1b[D".to_vec()),
            KeyCode::Home => Some(b"\x1b[H".to_vec()),
            KeyCode::End => Some(b"\x1b[F".to_vec()),
            KeyCode::PageUp => Some(b"\x1b[5~".to_vec()),
            KeyCode::PageDown => Some(b"\x1b[6~".to_vec()),
            KeyCode::Delete => Some(b"\x1b[3~".to_vec()),
            KeyCode::Insert => Some(b"\x1b[2~".to_vec()),
            KeyCode::F(n) => {
                let seq = match n {
                    1 => b"\x1bOP".to_vec(),
                    2 => b"\x1bOQ".to_vec(),
                    3 => b"\x1bOR".to_vec(),
                    4 => b"\x1bOS".to_vec(),
                    5 => b"\x1b[15~".to_vec(),
                    6 => b"\x1b[17~".to_vec(),
                    7 => b"\x1b[18~".to_vec(),
                    8 => b"\x1b[19~".to_vec(),
                    9 => b"\x1b[20~".to_vec(),
                    10 => b"\x1b[21~".to_vec(),
                    11 => b"\x1b[23~".to_vec(),
                    12 => b"\x1b[24~".to_vec(),
                    _ => return false,
                };
                Some(seq)
            }
            _ => None,
        };

        if let Some(bytes) = data {
            let _ = self.input_tx.send(bytes);
        }

        false
    }
}

fn vt100_color_to_ratatui(color: vt100::Color) -> Color {
    match color {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

pub fn draw(frame: &mut Frame, session: &mut ConsoleSession) {
    let area = frame.area();

    frame.render_widget(Clear, area);

    let title = Line::from(vec![
        Span::styled(" Console: ", Style::default().fg(Color::Cyan).bold()),
        Span::styled(
            session
                .vm_name
                .as_deref()
                .unwrap_or(&session.vm_id[..8.min(session.vm_id.len())]),
            Style::default().fg(Color::White),
        ),
        Span::styled(" | ", Style::default().fg(Color::DarkGray)),
        Span::styled("Ctrl+A t", Style::default().fg(Color::Yellow)),
        Span::styled(": exit", Style::default().fg(Color::DarkGray)),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(title);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let view_height = inner.height as usize;
    let view_width = inner.width as usize;

    let (current_rows, current_cols) = session.parser.screen().size();
    if current_rows as usize != view_height || current_cols as usize != view_width {
        session
            .parser
            .set_size(view_height as u16, view_width as u16);
    }

    let screen = session.parser.screen();
    let (cursor_row, cursor_col) = screen.cursor_position();

    let mut lines: Vec<Line> = Vec::with_capacity(view_height);

    for row in 0..view_height {
        let mut spans: Vec<Span> = Vec::new();
        let mut current_text = String::new();
        let mut current_style = Style::default();

        for col in 0..view_width {
            let cell = screen.cell(row as u16, col as u16);
            let (ch, cell_fg, cell_bg, bold) = if let Some(cell) = cell {
                (
                    cell.contents(),
                    vt100_color_to_ratatui(cell.fgcolor()),
                    vt100_color_to_ratatui(cell.bgcolor()),
                    cell.bold(),
                )
            } else {
                (" ".to_string(), Color::Reset, Color::Reset, false)
            };

            let is_cursor = row == cursor_row as usize && col == cursor_col as usize;

            let cell_style = if is_cursor {
                Style::default().fg(Color::Black).bg(Color::White)
            } else {
                let mut s = Style::default().fg(cell_fg).bg(cell_bg);
                if bold {
                    s = s.bold();
                }
                s
            };

            if cell_style != current_style && !current_text.is_empty() {
                spans.push(Span::styled(
                    std::mem::take(&mut current_text),
                    current_style,
                ));
            }
            current_style = cell_style;

            if ch.is_empty() {
                current_text.push(' ');
            } else {
                current_text.push_str(&ch);
            }
        }

        if !current_text.is_empty() {
            spans.push(Span::styled(current_text, current_style));
        }

        lines.push(Line::from(spans));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}
