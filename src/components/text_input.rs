use crossterm::event::KeyCode;
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

const BRAND_COLOR: Color = Color::Rgb(212, 160, 32);

pub struct TextInput {
    pub value: String,
    pub cursor: usize,
}

impl TextInput {
    pub fn new(initial: &str) -> Self {
        Self {
            value: initial.to_string(),
            cursor: initial.len(),
        }
    }

    /// Handle a key event. Returns true if Enter was pressed (submit).
    pub fn handle_input(&mut self, key: KeyCode) -> bool {
        match key {
            KeyCode::Char(c) => {
                self.value.insert(self.cursor, c);
                self.cursor += 1;
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.value.remove(self.cursor);
                }
            }
            KeyCode::Delete => {
                if self.cursor < self.value.len() {
                    self.value.remove(self.cursor);
                }
            }
            KeyCode::Left => {
                self.cursor = self.cursor.saturating_sub(1);
            }
            KeyCode::Right => {
                self.cursor = (self.cursor + 1).min(self.value.len());
            }
            KeyCode::Home => {
                self.cursor = 0;
            }
            KeyCode::End => {
                self.cursor = self.value.len();
            }
            KeyCode::Enter => return true,
            _ => {}
        }
        false
    }

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let display = if self.value.is_empty() {
            Line::from(Span::styled("_", Style::default().fg(BRAND_COLOR)))
        } else {
            let before = &self.value[..self.cursor];
            let cursor_char = if self.cursor < self.value.len() {
                &self.value[self.cursor..self.cursor + 1]
            } else {
                " "
            };
            let after = if self.cursor < self.value.len() {
                &self.value[self.cursor + 1..]
            } else {
                ""
            };

            Line::from(vec![
                Span::raw(before),
                Span::styled(cursor_char, Style::default().bg(BRAND_COLOR).fg(Color::Black)),
                Span::raw(after),
            ])
        };

        let text = Paragraph::new(display);
        frame.render_widget(text, area);
    }
}
