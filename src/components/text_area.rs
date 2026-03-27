use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

const BRAND_COLOR: Color = Color::Rgb(212, 160, 32);

#[derive(Debug, Clone, PartialEq)]
pub enum TextAreaAction {
    None,
    Submit,
    Cancel,
}

#[derive(Debug, Clone)]
pub struct TextAreaState {
    pub lines: Vec<String>,
    pub cursor_row: usize,
    pub cursor_col: usize,
    pub scroll_offset: usize,
}

impl TextAreaState {
    pub fn new(initial_content: &str) -> Self {
        let lines: Vec<String> = if initial_content.is_empty() {
            vec![String::new()]
        } else {
            initial_content.lines().map(String::from).collect()
        };
        let cursor_row = lines.len().saturating_sub(1);
        let cursor_col = lines.last().map_or(0, |l| l.len());
        Self {
            lines,
            cursor_row,
            cursor_col,
            scroll_offset: 0,
        }
    }

    pub fn content(&self) -> String {
        self.lines.join("\n")
    }

    fn clamp_cursor(&mut self) {
        if self.cursor_row >= self.lines.len() {
            self.cursor_row = self.lines.len().saturating_sub(1);
        }
        let line_len = self.lines[self.cursor_row].len();
        if self.cursor_col > line_len {
            self.cursor_col = line_len;
        }
    }

    fn ensure_visible(&mut self, visible_height: usize) {
        if visible_height == 0 {
            return;
        }
        if self.cursor_row < self.scroll_offset {
            self.scroll_offset = self.cursor_row;
        }
        if self.cursor_row >= self.scroll_offset + visible_height {
            self.scroll_offset = self.cursor_row - visible_height + 1;
        }
    }
}

pub fn handle_key(state: &mut TextAreaState, key: KeyEvent) -> TextAreaAction {
    let modifiers = key.modifiers;
    let code = key.code;

    // Ctrl+S or Ctrl+D -> Submit
    if modifiers.contains(KeyModifiers::CONTROL) {
        match code {
            KeyCode::Char('s') | KeyCode::Char('d') => return TextAreaAction::Submit,
            _ => {}
        }
    }

    match code {
        KeyCode::Esc => TextAreaAction::Cancel,
        KeyCode::Enter => {
            let current_line = &state.lines[state.cursor_row];
            let remainder = current_line[state.cursor_col..].to_string();
            state.lines[state.cursor_row] = current_line[..state.cursor_col].to_string();
            state.cursor_row += 1;
            state.lines.insert(state.cursor_row, remainder);
            state.cursor_col = 0;
            TextAreaAction::None
        }
        KeyCode::Backspace => {
            if state.cursor_col > 0 {
                state.lines[state.cursor_row].remove(state.cursor_col - 1);
                state.cursor_col -= 1;
            } else if state.cursor_row > 0 {
                let current_line = state.lines.remove(state.cursor_row);
                state.cursor_row -= 1;
                state.cursor_col = state.lines[state.cursor_row].len();
                state.lines[state.cursor_row].push_str(&current_line);
            }
            TextAreaAction::None
        }
        KeyCode::Delete => {
            let line_len = state.lines[state.cursor_row].len();
            if state.cursor_col < line_len {
                state.lines[state.cursor_row].remove(state.cursor_col);
            } else if state.cursor_row + 1 < state.lines.len() {
                let next_line = state.lines.remove(state.cursor_row + 1);
                state.lines[state.cursor_row].push_str(&next_line);
            }
            TextAreaAction::None
        }
        KeyCode::Left => {
            if state.cursor_col > 0 {
                state.cursor_col -= 1;
            } else if state.cursor_row > 0 {
                state.cursor_row -= 1;
                state.cursor_col = state.lines[state.cursor_row].len();
            }
            TextAreaAction::None
        }
        KeyCode::Right => {
            let line_len = state.lines[state.cursor_row].len();
            if state.cursor_col < line_len {
                state.cursor_col += 1;
            } else if state.cursor_row + 1 < state.lines.len() {
                state.cursor_row += 1;
                state.cursor_col = 0;
            }
            TextAreaAction::None
        }
        KeyCode::Up => {
            if state.cursor_row > 0 {
                state.cursor_row -= 1;
                state.clamp_cursor();
            }
            TextAreaAction::None
        }
        KeyCode::Down => {
            if state.cursor_row + 1 < state.lines.len() {
                state.cursor_row += 1;
                state.clamp_cursor();
            }
            TextAreaAction::None
        }
        KeyCode::Home => {
            state.cursor_col = 0;
            TextAreaAction::None
        }
        KeyCode::End => {
            state.cursor_col = state.lines[state.cursor_row].len();
            TextAreaAction::None
        }
        KeyCode::PageUp => {
            state.cursor_row = state.cursor_row.saturating_sub(10);
            state.clamp_cursor();
            TextAreaAction::None
        }
        KeyCode::PageDown => {
            state.cursor_row = (state.cursor_row + 10).min(state.lines.len().saturating_sub(1));
            state.clamp_cursor();
            TextAreaAction::None
        }
        KeyCode::Char(c) => {
            state.lines[state.cursor_row].insert(state.cursor_col, c);
            state.cursor_col += 1;
            TextAreaAction::None
        }
        _ => TextAreaAction::None,
    }
}

pub fn render(frame: &mut Frame, area: Rect, state: &mut TextAreaState, focused: bool) {
    let visible_height = area.height as usize;
    state.ensure_visible(visible_height);

    // Line number gutter width: at least 3 chars + separator
    let total_lines = state.lines.len();
    let gutter_width = format!("{}", total_lines).len().max(3) + 1;
    let gutter_w = (gutter_width as u16).min(area.width);
    let content_w = area.width.saturating_sub(gutter_w);

    for (vi, row_idx) in (state.scroll_offset..).take(visible_height).enumerate() {
        let y = area.y + vi as u16;
        if y >= area.y + area.height {
            break;
        }

        // Gutter
        let gutter_area = Rect::new(area.x, y, gutter_w, 1);
        let line_num = if row_idx < total_lines {
            format!("{:>width$} ", row_idx + 1, width = gutter_width - 1)
        } else {
            " ".repeat(gutter_width)
        };
        let gutter_style = if focused && row_idx == state.cursor_row {
            Style::default().fg(BRAND_COLOR)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        frame.render_widget(
            Paragraph::new(Span::styled(line_num, gutter_style)),
            gutter_area,
        );

        // Content
        let content_area = Rect::new(area.x + gutter_w, y, content_w, 1);
        if row_idx < total_lines {
            let line = &state.lines[row_idx];
            if focused && row_idx == state.cursor_row {
                // Render with cursor highlight
                let col = state.cursor_col.min(line.len());
                let before = &line[..col];
                let cursor_char = if col < line.len() {
                    &line[col..col + 1]
                } else {
                    " "
                };
                let after = if col < line.len() {
                    &line[col + 1..]
                } else {
                    ""
                };
                let spans = vec![
                    Span::raw(before),
                    Span::styled(
                        cursor_char,
                        Style::default().bg(BRAND_COLOR).fg(Color::Black),
                    ),
                    Span::raw(after),
                ];
                frame.render_widget(Paragraph::new(Line::from(spans)), content_area);
            } else {
                frame.render_widget(Paragraph::new(line.as_str()), content_area);
            }
        }
    }
}
