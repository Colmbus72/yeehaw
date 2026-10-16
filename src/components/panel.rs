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

    /// Render the panel block and return the inner area for content.
    ///
    /// Hints sit in the bottom border, right-aligned, while focused — never on
    /// a content row. Terminals are wider than they are tall, and a hints row
    /// inside the panel cost every list its last line.
    pub fn render(&self, frame: &mut Frame, area: Rect) -> Rect {
        let mut block = self.block();
        if self.focused {
            if let Some(hints) = self.hints {
                let mut spans = vec![Span::raw(" ")];
                spans.extend(render_hints(hints));
                spans.push(Span::raw(" "));
                block = block.title_bottom(Line::from(spans).right_aligned());
            }
        }
        let inner = block.inner(area);
        frame.render_widget(block, area);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Hints ride in the bottom border, so a focused panel gives its content
    /// every inner row.
    #[test]
    fn hints_sit_in_the_bottom_border_and_cost_no_content_row() {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(40, 6)).expect("test terminal");
        let mut inner = Rect::default();
        terminal
            .draw(|f| {
                let panel = Panel { title: "Projects", focused: true, hints: Some("[n] new  [d] delete") };
                inner = panel.render(f, f.area());
            })
            .expect("draw");
        assert_eq!(inner.height, 4, "both borders only — no row kept back for hints");
        let buf = terminal.backend().buffer().clone();
        let bottom: String = (0..40).map(|x| buf.cell((x, 5)).expect("a cell").symbol()).collect();
        assert!(bottom.contains("[n] new  [d] delete"), "bottom border: {:?}", bottom);
    }
}
