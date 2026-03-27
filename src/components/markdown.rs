use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

const BRAND_COLOR: Color = Color::Rgb(212, 160, 32);

#[derive(Debug, Clone)]
pub struct MarkdownState {
    pub scroll_offset: usize,
    pub total_lines: usize,
}

impl MarkdownState {
    pub fn new() -> Self {
        Self {
            scroll_offset: 0,
            total_lines: 0,
        }
    }
}

pub fn handle_key(state: &mut MarkdownState, key: KeyEvent, visible_height: usize) {
    let max_scroll = state.total_lines.saturating_sub(visible_height);
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            state.scroll_offset = (state.scroll_offset + 1).min(max_scroll);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            state.scroll_offset = state.scroll_offset.saturating_sub(1);
        }
        KeyCode::Char('g') => {
            state.scroll_offset = 0;
        }
        KeyCode::Char('G') => {
            state.scroll_offset = max_scroll;
        }
        KeyCode::PageDown => {
            state.scroll_offset = (state.scroll_offset + visible_height).min(max_scroll);
        }
        KeyCode::PageUp => {
            state.scroll_offset = state.scroll_offset.saturating_sub(visible_height);
        }
        _ => {}
    }
}

pub fn render_markdown(frame: &mut Frame, area: Rect, content: &str, state: &mut MarkdownState) {
    let lines = parse_markdown(content);
    state.total_lines = lines.len();

    let visible: Vec<Line> = lines
        .into_iter()
        .skip(state.scroll_offset)
        .take(area.height as usize)
        .collect();

    let paragraph = Paragraph::new(visible);
    frame.render_widget(paragraph, area);
}

/// Parse markdown content into styled `Line` objects.
fn parse_markdown(content: &str) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut in_code_block = false;

    for raw_line in content.lines() {
        if raw_line.starts_with("```") {
            in_code_block = !in_code_block;
            if in_code_block {
                lines.push(Line::from(Span::styled(
                    "---".to_string(),
                    Style::default().fg(Color::DarkGray),
                )));
            } else {
                lines.push(Line::from(Span::styled(
                    "---".to_string(),
                    Style::default().fg(Color::DarkGray),
                )));
            }
            continue;
        }

        if in_code_block {
            lines.push(Line::from(Span::styled(
                format!("  {}", raw_line),
                Style::default().fg(Color::Green),
            )));
            continue;
        }

        // Horizontal rule
        if raw_line.trim() == "---" || raw_line.trim() == "***" || raw_line.trim() == "___" {
            lines.push(Line::from(Span::styled(
                "\u{2500}".repeat(40),
                Style::default().fg(Color::DarkGray),
            )));
            continue;
        }

        // Headers
        if let Some(stripped) = raw_line.strip_prefix("### ") {
            lines.push(Line::from(Span::styled(
                format!("   {}", stripped),
                Style::default()
                    .fg(BRAND_COLOR)
                    .add_modifier(Modifier::BOLD),
            )));
            continue;
        }
        if let Some(stripped) = raw_line.strip_prefix("## ") {
            lines.push(Line::from(Span::styled(
                format!("  {}", stripped),
                Style::default()
                    .fg(BRAND_COLOR)
                    .add_modifier(Modifier::BOLD),
            )));
            continue;
        }
        if let Some(stripped) = raw_line.strip_prefix("# ") {
            lines.push(Line::from(Span::styled(
                stripped.to_string(),
                Style::default()
                    .fg(BRAND_COLOR)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            )));
            continue;
        }

        // List items
        if let Some(stripped) = raw_line.strip_prefix("- ") {
            let mut spans = vec![Span::styled(
                "  \u{2022} ".to_string(),
                Style::default().fg(BRAND_COLOR),
            )];
            spans.extend(parse_inline(stripped));
            lines.push(Line::from(spans));
            continue;
        }
        if let Some(stripped) = raw_line.strip_prefix("* ") {
            let mut spans = vec![Span::styled(
                "  \u{2022} ".to_string(),
                Style::default().fg(BRAND_COLOR),
            )];
            spans.extend(parse_inline(stripped));
            lines.push(Line::from(spans));
            continue;
        }

        // Regular paragraph line with inline formatting
        if raw_line.is_empty() {
            lines.push(Line::from(""));
        } else {
            lines.push(Line::from(parse_inline(raw_line)));
        }
    }

    lines
}

/// Parse inline markdown formatting: **bold**, *italic*, `code`.
fn parse_inline(text: &str) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        // Bold: **text**
        if let Some(start) = remaining.find("**") {
            if start > 0 {
                spans.push(Span::raw(remaining[..start].to_string()));
            }
            let after_start = &remaining[start + 2..];
            if let Some(end) = after_start.find("**") {
                spans.push(Span::styled(
                    after_start[..end].to_string(),
                    Style::default().add_modifier(Modifier::BOLD),
                ));
                remaining = &after_start[end + 2..];
                continue;
            } else {
                spans.push(Span::raw(remaining[start..].to_string()));
                break;
            }
        }

        // Italic: *text*
        if let Some(start) = remaining.find('*') {
            if start > 0 {
                spans.push(Span::raw(remaining[..start].to_string()));
            }
            let after_start = &remaining[start + 1..];
            if let Some(end) = after_start.find('*') {
                spans.push(Span::styled(
                    after_start[..end].to_string(),
                    Style::default().add_modifier(Modifier::ITALIC),
                ));
                remaining = &after_start[end + 1..];
                continue;
            } else {
                spans.push(Span::raw(remaining[start..].to_string()));
                break;
            }
        }

        // Inline code: `text`
        if let Some(start) = remaining.find('`') {
            if start > 0 {
                spans.push(Span::raw(remaining[..start].to_string()));
            }
            let after_start = &remaining[start + 1..];
            if let Some(end) = after_start.find('`') {
                spans.push(Span::styled(
                    after_start[..end].to_string(),
                    Style::default().fg(Color::Green),
                ));
                remaining = &after_start[end + 1..];
                continue;
            } else {
                spans.push(Span::raw(remaining[start..].to_string()));
                break;
            }
        }

        // No more inline formatting found
        spans.push(Span::raw(remaining.to_string()));
        break;
    }

    spans
}
