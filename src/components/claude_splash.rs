use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph, Wrap};

const BRAND_COLOR: Color = Color::Rgb(212, 160, 32);

#[derive(Debug, Clone, PartialEq)]
pub enum ClaudeSplashAction {
    None,
    Launch,
    Cancel,
    Tick,
}

#[derive(Debug, Clone)]
pub struct ClaudeSplashState {
    pub countdown: u8,
    pub paused: bool,
    pub focused_panel: usize, // 0 = left (system prompt), 1 = right (tools)
    pub left_scroll: usize,
    pub right_scroll: usize,
}

impl ClaudeSplashState {
    pub fn new() -> Self {
        Self {
            countdown: 3,
            paused: false,
            focused_panel: 0,
            left_scroll: 0,
            right_scroll: 0,
        }
    }

    /// Called once per second by the app tick handler.
    pub fn tick(&mut self) -> ClaudeSplashAction {
        if self.paused {
            return ClaudeSplashAction::None;
        }
        if self.countdown > 1 {
            self.countdown -= 1;
            ClaudeSplashAction::Tick
        } else {
            ClaudeSplashAction::Launch
        }
    }
}

pub fn handle_key(state: &mut ClaudeSplashState, key: KeyEvent) -> ClaudeSplashAction {
    match key.code {
        KeyCode::Esc => ClaudeSplashAction::Cancel,
        KeyCode::Enter => ClaudeSplashAction::Launch,
        KeyCode::Char(' ') => {
            state.paused = !state.paused;
            ClaudeSplashAction::None
        }
        KeyCode::Tab | KeyCode::BackTab => {
            state.focused_panel = if state.focused_panel == 0 { 1 } else { 0 };
            ClaudeSplashAction::None
        }
        KeyCode::Char('j') | KeyCode::Down => {
            if state.focused_panel == 0 {
                state.left_scroll += 1;
            } else {
                state.right_scroll += 1;
            }
            ClaudeSplashAction::None
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if state.focused_panel == 0 {
                state.left_scroll = state.left_scroll.saturating_sub(1);
            } else {
                state.right_scroll = state.right_scroll.saturating_sub(1);
            }
            ClaudeSplashAction::None
        }
        _ => ClaudeSplashAction::None,
    }
}

pub fn render(
    frame: &mut Frame,
    area: Rect,
    state: &ClaudeSplashState,
    system_prompt: &str,
    tools: &[String],
) {
    // Clear the entire area so underlying views don't bleed through
    frame.render_widget(Clear, area);

    // Layout: header (2 lines) + body + footer (2 lines)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(4),
            Constraint::Length(2),
        ])
        .split(area);

    // Header: countdown
    let status = if state.paused {
        format!("  PAUSED ({}s) - press Space to resume", state.countdown)
    } else {
        format!("  Launching Claude in {}s...", state.countdown)
    };
    let header = Paragraph::new(Line::from(vec![
        Span::styled(
            " Claude Session ",
            Style::default()
                .fg(BRAND_COLOR)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(status, Style::default().fg(Color::DarkGray)),
    ]));
    frame.render_widget(header, chunks[0]);

    // Body: two-panel split (65% / 35%)
    let panels = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
        .split(chunks[1]);

    // Left panel: system prompt
    let left_focused = state.focused_panel == 0;
    let left_border_color = if left_focused { BRAND_COLOR } else { Color::DarkGray };
    let left_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(left_border_color))
        .title(Span::styled(
            " System Prompt ",
            Style::default().fg(if left_focused { BRAND_COLOR } else { Color::DarkGray }),
        ))
        .padding(Padding::horizontal(1));

    let left_inner = left_block.inner(panels[0]);
    frame.render_widget(left_block, panels[0]);

    let prompt_lines: Vec<Line> = system_prompt.lines().map(|l| Line::from(l.to_string())).collect();
    let prompt_paragraph = Paragraph::new(prompt_lines)
        .scroll((state.left_scroll as u16, 0))
        .wrap(Wrap { trim: false });
    frame.render_widget(prompt_paragraph, left_inner);

    // Right panel: MCP tools list
    let right_focused = state.focused_panel == 1;
    let right_border_color = if right_focused { BRAND_COLOR } else { Color::DarkGray };
    let right_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(right_border_color))
        .title(Span::styled(
            format!(" MCP Tools ({}) ", tools.len()),
            Style::default().fg(if right_focused { BRAND_COLOR } else { Color::DarkGray }),
        ))
        .padding(Padding::horizontal(1));

    let right_inner = right_block.inner(panels[1]);
    frame.render_widget(right_block, panels[1]);

    let tool_lines: Vec<Line> = tools
        .iter()
        .enumerate()
        .map(|(i, tool)| {
            Line::from(vec![
                Span::styled(
                    format!("{:>3}. ", i + 1),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(tool.to_string(), Style::default().fg(Color::White)),
            ])
        })
        .collect();
    let tools_paragraph = Paragraph::new(tool_lines)
        .scroll((state.right_scroll as u16, 0));
    frame.render_widget(tools_paragraph, right_inner);

    // Footer: hints
    let hints = Line::from(vec![
        Span::styled("[Enter]", Style::default().fg(BRAND_COLOR)),
        Span::styled(" launch  ", Style::default().fg(Color::DarkGray)),
        Span::styled("[Space]", Style::default().fg(BRAND_COLOR)),
        Span::styled(" pause  ", Style::default().fg(Color::DarkGray)),
        Span::styled("[Esc]", Style::default().fg(BRAND_COLOR)),
        Span::styled(" cancel  ", Style::default().fg(Color::DarkGray)),
        Span::styled("[Tab]", Style::default().fg(BRAND_COLOR)),
        Span::styled(" switch panel  ", Style::default().fg(Color::DarkGray)),
        Span::styled("[j/k]", Style::default().fg(BRAND_COLOR)),
        Span::styled(" scroll", Style::default().fg(Color::DarkGray)),
    ]);
    let footer = Paragraph::new(hints).alignment(Alignment::Center);
    frame.render_widget(footer, chunks[2]);
}
