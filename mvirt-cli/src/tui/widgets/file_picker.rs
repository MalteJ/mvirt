use std::path::PathBuf;

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

pub struct FilePicker {
    pub current_path: PathBuf,
    pub entries: Vec<PathBuf>,
    pub selected: usize,
    pub scroll_offset: usize,
    pub target_field: usize,
}

impl FilePicker {
    pub fn new(start_path: PathBuf, target_field: usize) -> Self {
        let mut picker = Self {
            current_path: start_path,
            entries: Vec::new(),
            selected: 0,
            scroll_offset: 0,
            target_field,
        };
        picker.refresh_entries();
        picker
    }

    pub fn refresh_entries(&mut self) {
        self.entries.clear();

        if self.current_path.parent().is_some() {
            self.entries.push(PathBuf::from(".."));
        }

        if let Ok(read_dir) = std::fs::read_dir(&self.current_path) {
            let mut dirs: Vec<PathBuf> = Vec::new();
            let mut files: Vec<PathBuf> = Vec::new();

            for entry in read_dir.flatten() {
                let path = entry.path();
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                if name.starts_with('.') {
                    continue;
                }
                if path.is_dir() {
                    dirs.push(path);
                } else {
                    files.push(path);
                }
            }

            dirs.sort();
            files.sort();

            self.entries.extend(dirs);
            self.entries.extend(files);
        }

        self.selected = 0;
        self.scroll_offset = 0;
    }

    pub fn select_next(&mut self) {
        if !self.entries.is_empty() {
            self.selected = (self.selected + 1) % self.entries.len();
        }
    }

    pub fn select_prev(&mut self) {
        if !self.entries.is_empty() {
            self.selected = if self.selected == 0 {
                self.entries.len() - 1
            } else {
                self.selected - 1
            };
        }
    }

    pub fn enter_selected(&mut self) -> Option<PathBuf> {
        let entry = self.entries.get(self.selected)?;

        if entry == &PathBuf::from("..") {
            if let Some(parent) = self.current_path.parent() {
                self.current_path = parent.to_path_buf();
                self.refresh_entries();
            }
            None
        } else if entry.is_dir() {
            self.current_path = entry.clone();
            self.refresh_entries();
            None
        } else {
            Some(entry.clone())
        }
    }
}

pub fn draw(frame: &mut Frame, picker: &FilePicker) {
    let area = frame.area();
    let modal_width = 60.min(area.width.saturating_sub(6));
    let modal_height = 20.min(area.height.saturating_sub(6));

    let modal_area = Rect {
        x: (area.width - modal_width) / 2,
        y: (area.height - modal_height) / 2,
        width: modal_width,
        height: modal_height,
    };

    frame.render_widget(Clear, modal_area);

    let title = format!(
        " {} (Enter: select, Esc: cancel) ",
        picker.current_path.display()
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .style(Style::default().bg(Color::Black));
    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    let visible_height = inner.height as usize;

    let scroll_offset = if picker.selected >= visible_height {
        picker.selected - visible_height + 1
    } else {
        0
    };

    for (i, entry) in picker
        .entries
        .iter()
        .skip(scroll_offset)
        .take(visible_height)
        .enumerate()
    {
        let actual_index = i + scroll_offset;
        let is_selected = actual_index == picker.selected;

        let (name, style) = if entry == &PathBuf::from("..") {
            (
                "..".to_string(),
                if is_selected {
                    Style::default().fg(Color::Cyan).bold().reversed()
                } else {
                    Style::default().fg(Color::Cyan)
                },
            )
        } else if entry.is_dir() {
            let name = entry
                .file_name()
                .map(|n| format!("{}/", n.to_string_lossy()))
                .unwrap_or_else(|| "???/".to_string());
            (
                name,
                if is_selected {
                    Style::default().fg(Color::Blue).bold().reversed()
                } else {
                    Style::default().fg(Color::Blue)
                },
            )
        } else {
            let name = entry
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "???".to_string());
            (
                name,
                if is_selected {
                    Style::default().reversed()
                } else {
                    Style::default()
                },
            )
        };

        let line_area = Rect {
            x: inner.x,
            y: inner.y + i as u16,
            width: inner.width,
            height: 1,
        };
        frame.render_widget(Paragraph::new(Span::styled(name, style)), line_area);
    }
}
