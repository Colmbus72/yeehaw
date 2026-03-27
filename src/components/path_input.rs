use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;
use std::path::{Path, PathBuf};

const BRAND_COLOR: Color = Color::Rgb(212, 160, 32);

#[derive(Debug, Clone, PartialEq)]
pub enum PathInputAction {
    None,
    Submit(String),
    Cancel,
}

#[derive(Debug, Clone)]
pub struct PathInputState {
    pub input: String,
    pub cursor: usize,
    pub completions: Vec<String>,
    pub selected_completion: usize,
    pub show_completions: bool,
}

impl PathInputState {
    pub fn new(initial: &str) -> Self {
        Self {
            input: initial.to_string(),
            cursor: initial.len(),
            completions: Vec::new(),
            selected_completion: 0,
            show_completions: false,
        }
    }

    /// Expand `~/` to the user's home directory.
    fn expand_tilde(path: &str) -> String {
        if path.starts_with("~/") || path == "~" {
            if let Some(home) = dirs::home_dir() {
                return path.replacen('~', &home.to_string_lossy(), 1);
            }
        }
        path.to_string()
    }

    /// Compute completions for the current input.
    fn compute_completions(&mut self) {
        let expanded = Self::expand_tilde(&self.input);
        let path = Path::new(&expanded);

        let (dir, prefix) = if expanded.ends_with('/') || expanded.ends_with(std::path::MAIN_SEPARATOR) {
            (PathBuf::from(&expanded), String::new())
        } else if let Some(parent) = path.parent() {
            let file_prefix = path
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_default();
            (parent.to_path_buf(), file_prefix)
        } else {
            (PathBuf::from("."), expanded.clone())
        };

        let mut matches = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with(&prefix) {
                    let full = dir.join(&name);
                    let display = if full.is_dir() {
                        format!("{}/", name)
                    } else {
                        name
                    };
                    matches.push(display);
                }
            }
        }
        matches.sort();
        self.completions = matches;
        self.selected_completion = 0;
        self.show_completions = !self.completions.is_empty();
    }

    /// Apply the common prefix of all completions, or cycle if only one match.
    fn apply_completion(&mut self) {
        if self.completions.is_empty() {
            return;
        }

        if self.completions.len() == 1 {
            // Single match: replace the file portion with the completion
            self.apply_selected_completion();
            self.show_completions = false;
            return;
        }

        // Multiple matches: insert the longest common prefix
        let common = longest_common_prefix(&self.completions);
        if !common.is_empty() {
            let expanded = Self::expand_tilde(&self.input);
            let path = Path::new(&expanded);

            let base_dir = if expanded.ends_with('/') {
                expanded.clone()
            } else if let Some(parent) = path.parent() {
                let mut p = parent.to_string_lossy().to_string();
                if !p.ends_with('/') {
                    p.push('/');
                }
                p
            } else {
                String::new()
            };

            // Re-collapse home dir if the original input used tilde
            let new_input = if self.input.starts_with("~/") || self.input == "~" {
                if let Some(home) = dirs::home_dir() {
                    let home_str = home.to_string_lossy().to_string();
                    let full = format!("{}{}", base_dir, common);
                    full.replacen(&home_str, "~", 1)
                } else {
                    format!("{}{}", base_dir, common)
                }
            } else {
                format!("{}{}", base_dir, common)
            };

            self.input = new_input;
            self.cursor = self.input.len();
        }
    }

    fn apply_selected_completion(&mut self) {
        if self.completions.is_empty() {
            return;
        }
        let completion = &self.completions[self.selected_completion].clone();
        let expanded = Self::expand_tilde(&self.input);
        let path = Path::new(&expanded);

        let base_dir = if expanded.ends_with('/') {
            expanded.clone()
        } else if let Some(parent) = path.parent() {
            let mut p = parent.to_string_lossy().to_string();
            if !p.ends_with('/') {
                p.push('/');
            }
            p
        } else {
            String::new()
        };

        let new_input = if self.input.starts_with("~/") || self.input == "~" {
            if let Some(home) = dirs::home_dir() {
                let home_str = home.to_string_lossy().to_string();
                let full = format!("{}{}", base_dir, completion);
                full.replacen(&home_str, "~", 1)
            } else {
                format!("{}{}", base_dir, completion)
            }
        } else {
            format!("{}{}", base_dir, completion)
        };

        self.input = new_input;
        self.cursor = self.input.len();
    }
}

fn longest_common_prefix(strings: &[String]) -> String {
    if strings.is_empty() {
        return String::new();
    }
    let first = &strings[0];
    let mut len = first.len();
    for s in &strings[1..] {
        len = len.min(s.len());
        for (i, (a, b)) in first.chars().zip(s.chars()).enumerate() {
            if a != b {
                len = len.min(i);
                break;
            }
        }
    }
    first[..len].to_string()
}

pub fn handle_key(state: &mut PathInputState, key: KeyEvent) -> PathInputAction {
    match key.code {
        KeyCode::Esc => PathInputAction::Cancel,
        KeyCode::Enter => {
            state.show_completions = false;
            let expanded = PathInputState::expand_tilde(&state.input);
            PathInputAction::Submit(expanded)
        }
        KeyCode::Tab => {
            if state.show_completions && state.completions.len() > 1 {
                // Cycle through completions
                state.selected_completion =
                    (state.selected_completion + 1) % state.completions.len();
                state.apply_selected_completion();
                // Recompute for the new path
                state.compute_completions();
            } else {
                state.compute_completions();
                state.apply_completion();
            }
            PathInputAction::None
        }
        KeyCode::BackTab => {
            if state.show_completions && !state.completions.is_empty() {
                if state.selected_completion == 0 {
                    state.selected_completion = state.completions.len() - 1;
                } else {
                    state.selected_completion -= 1;
                }
                state.apply_selected_completion();
                state.compute_completions();
            }
            PathInputAction::None
        }
        KeyCode::Char(c) => {
            state.input.insert(state.cursor, c);
            state.cursor += 1;
            state.show_completions = false;
            PathInputAction::None
        }
        KeyCode::Backspace => {
            if state.cursor > 0 {
                state.cursor -= 1;
                state.input.remove(state.cursor);
            }
            state.show_completions = false;
            PathInputAction::None
        }
        KeyCode::Delete => {
            if state.cursor < state.input.len() {
                state.input.remove(state.cursor);
            }
            state.show_completions = false;
            PathInputAction::None
        }
        KeyCode::Left => {
            state.cursor = state.cursor.saturating_sub(1);
            PathInputAction::None
        }
        KeyCode::Right => {
            state.cursor = (state.cursor + 1).min(state.input.len());
            PathInputAction::None
        }
        KeyCode::Home => {
            state.cursor = 0;
            PathInputAction::None
        }
        KeyCode::End => {
            state.cursor = state.input.len();
            PathInputAction::None
        }
        _ => PathInputAction::None,
    }
}

pub fn render(frame: &mut Frame, area: Rect, state: &PathInputState) {
    // Row 1: the input line with cursor
    let input_area = Rect::new(area.x, area.y, area.width, 1);
    let input = &state.input;
    let cursor = state.cursor.min(input.len());

    let before = &input[..cursor];
    let cursor_char = if cursor < input.len() {
        &input[cursor..cursor + 1]
    } else {
        " "
    };
    let after = if cursor < input.len() {
        &input[cursor + 1..]
    } else {
        ""
    };

    let spans = vec![
        Span::raw(before),
        Span::styled(cursor_char, Style::default().bg(BRAND_COLOR).fg(Color::Black)),
        Span::raw(after),
    ];
    frame.render_widget(Paragraph::new(Line::from(spans)), input_area);

    // Rows 2+: completion list (if visible)
    if state.show_completions && !state.completions.is_empty() {
        let max_rows = (area.height.saturating_sub(1) as usize).min(8);
        for (i, completion) in state.completions.iter().take(max_rows).enumerate() {
            let y = area.y + 1 + i as u16;
            if y >= area.y + area.height {
                break;
            }
            let row_area = Rect::new(area.x, y, area.width, 1);
            let style = if i == state.selected_completion {
                Style::default().fg(Color::Black).bg(BRAND_COLOR)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let prefix = if i == state.selected_completion { "> " } else { "  " };
            frame.render_widget(
                Paragraph::new(Span::styled(format!("{}{}", prefix, completion), style)),
                row_area,
            );
        }
        // If there are more completions than shown
        if state.completions.len() > max_rows {
            let y = area.y + 1 + max_rows as u16;
            if y < area.y + area.height {
                let remaining = state.completions.len() - max_rows;
                frame.render_widget(
                    Paragraph::new(Span::styled(
                        format!("  ... {} more", remaining),
                        Style::default().fg(Color::DarkGray),
                    )),
                    Rect::new(area.x, y, area.width, 1),
                );
            }
        }
    }
}
