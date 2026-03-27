use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Padding};

const BRAND_COLOR: Color = Color::Rgb(212, 160, 32);

pub struct Panel<'a> {
    pub title: &'a str,
    pub focused: bool,
    pub hints: Option<&'a str>,
}

impl<'a> Panel<'a> {
    pub fn block(&self) -> Block<'a> {
        let border_color = if self.focused { BRAND_COLOR } else { Color::DarkGray };
        let title_style = if self.focused {
            Style::default().fg(BRAND_COLOR).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color))
            .title(Span::styled(self.title, title_style))
            .padding(Padding::horizontal(1))
    }

    /// Returns the inner area of the panel (after borders/padding)
    pub fn inner(&self, area: Rect) -> Rect {
        self.block().inner(area)
    }

    /// Render the panel block and return the inner area for content
    pub fn render(&self, frame: &mut Frame, area: Rect) -> Rect {
        let block = self.block();
        let inner = block.inner(area);
        frame.render_widget(block, area);

        // If we have hints and are focused, render them at the bottom of inner area
        if self.focused {
            if let Some(hints) = self.hints {
                if inner.height > 1 {
                    let hints_area = Rect {
                        x: inner.x,
                        y: inner.y + inner.height - 1,
                        width: inner.width,
                        height: 1,
                    };
                    let content_area = Rect {
                        x: inner.x,
                        y: inner.y,
                        width: inner.width,
                        height: inner.height - 1,
                    };

                    let hint_spans = render_hints(hints);
                    let hint_line = ratatui::widgets::Paragraph::new(Line::from(hint_spans))
                        .alignment(Alignment::Right);
                    frame.render_widget(hint_line, hints_area);

                    return content_area;
                }
            }
        }

        inner
    }
}

fn render_hints(hints: &str) -> Vec<Span<'_>> {
    let mut spans = Vec::new();
    let mut chars = hints.chars().peekable();
    let mut current = String::new();
    let mut in_bracket = false;

    while let Some(ch) = chars.next() {
        if ch == '[' {
            if !current.is_empty() {
                spans.push(Span::styled(current.clone(), Style::default().fg(Color::DarkGray)));
                current.clear();
            }
            in_bracket = true;
            current.push(ch);
        } else if ch == ']' && in_bracket {
            current.push(ch);
            spans.push(Span::styled(current.clone(), Style::default().fg(BRAND_COLOR)));
            current.clear();
            in_bracket = false;
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        spans.push(Span::styled(current, Style::default().fg(Color::DarkGray)));
    }
    spans
}
