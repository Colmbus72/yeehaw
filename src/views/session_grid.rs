use std::collections::HashSet;

use crossterm::event::KeyCode;
use ratatui::prelude::*;
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use unicode_width::UnicodeWidthChar;

use crate::signals::{self, SessionStatus};
use crate::tmux::TmuxWindow;

/// Which sessions the grid is showing, decided by where `v` was pressed.
#[derive(Debug, Clone, PartialEq)]
pub enum GridScope {
    All,
    Project(String),
    Barn(String),
}

impl GridScope {
    fn title(&self) -> String {
        match self {
            GridScope::All => "ALL SESSIONS".to_string(),
            GridScope::Project(p) => format!("PROJECT · {}", p),
            GridScope::Barn(b) => format!("BARN · {}", b),
        }
    }

    /// Does this window belong to the scope? Untagged windows only ever show in
    /// `All`, since we cannot know where they came from.
    fn matches(&self, w: &TmuxWindow) -> bool {
        match self {
            GridScope::All => true,
            GridScope::Project(p) => &w.project == p,
            GridScope::Barn(b) => &w.barn == b,
        }
    }
}

/// The window types the filter can toggle. Order here is the order shown in the
/// filter panel.
pub const WINDOW_TYPES: &[&str] = &["claude", "shell", "ssh", "worm", "slack"];

const CLAUDE: &str = "claude";
/// Bucket for windows created before scope tagging existed.
const UNTAGGED: &str = "untagged";

fn type_key(w: &TmuxWindow) -> &str {
    if w.window_type.is_empty() {
        UNTAGGED
    } else {
        &w.window_type
    }
}

/// Minimum cell size worth rendering. Below this the preview is confetti.
const MIN_CELL_W: u16 = 24;
const MIN_CELL_H: u16 = 5;

/// Number keys only reach the first nine cells.
const MAX_NUMBERED: usize = 9;

/// Choose a tiling for `n` sessions inside `area`.
///
/// Returns `(cols, rows)`. Deliberately leaves margin around the cells — that
/// space is where the background art lives.
pub fn grid_dims(n: usize, area: Rect) -> (usize, usize) {
    if n == 0 {
        return (0, 0);
    }
    let (cols, rows) = match n {
        1 => (1, 1),
        2 => (2, 1),
        3..=4 => (2, 2),
        5..=6 => (3, 2),
        7..=9 => (3, 3),
        10..=12 => (4, 3),
        _ => (4, 4),
    };

    // Shrink the tiling until cells clear the legibility floor, so a small
    // terminal shows fewer, readable cells instead of many useless ones.
    let mut cols = cols;
    let mut rows = rows;
    while cols > 1 && area.width / (cols as u16) < MIN_CELL_W {
        cols -= 1;
    }
    while rows > 1 && area.height / (rows as u16) < MIN_CELL_H {
        rows -= 1;
    }
    (cols, rows)
}

/// Apply one SGR parameter run (the numbers in `ESC [ ... m`) to a style.
fn apply_sgr(style: Style, params: &[u8]) -> Style {
    let mut style = style;
    let mut i = 0;
    while i < params.len() {
        match params[i] {
            0 => style = Style::default(),
            1 => style = style.add_modifier(Modifier::BOLD),
            2 => style = style.add_modifier(Modifier::DIM),
            3 => style = style.add_modifier(Modifier::ITALIC),
            4 => style = style.add_modifier(Modifier::UNDERLINED),
            22 => style = style.remove_modifier(Modifier::BOLD | Modifier::DIM),
            23 => style = style.remove_modifier(Modifier::ITALIC),
            24 => style = style.remove_modifier(Modifier::UNDERLINED),
            // Extended colour: 38/48 ; 5 ; n  or  38/48 ; 2 ; r ; g ; b
            n @ (38 | 48) => {
                let (color, consumed) = match params.get(i + 1) {
                    Some(5) => (
                        params.get(i + 2).map(|&c| Color::Indexed(c)),
                        3,
                    ),
                    Some(2) => (
                        match (params.get(i + 2), params.get(i + 3), params.get(i + 4)) {
                            (Some(&r), Some(&g), Some(&b)) => Some(Color::Rgb(r, g, b)),
                            _ => None,
                        },
                        5,
                    ),
                    _ => (None, 1),
                };
                if let Some(c) = color {
                    style = if n == 38 { style.fg(c) } else { style.bg(c) };
                }
                i += consumed;
                continue;
            }
            39 => style = style.fg(Color::Reset),
            49 => style = style.bg(Color::Reset),
            n @ 30..=37 => style = style.fg(Color::Indexed(n - 30)),
            n @ 40..=47 => style = style.bg(Color::Indexed(n - 40)),
            n @ 90..=97 => style = style.fg(Color::Indexed(n - 90 + 8)),
            n @ 100..=107 => style = style.bg(Color::Indexed(n - 100 + 8)),
            _ => {}
        }
        i += 1;
    }
    style
}

/// Parse a captured line containing ANSI escapes into styled spans, truncated to
/// `max_cols` display columns.
///
/// Truncation happens on decoded text, so a cut can never land inside an escape
/// sequence and leak styling into the rest of the grid.
pub fn ansi_spans(s: &str, max_cols: usize, base: Style) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut style = base;
    let mut run = String::new();
    let mut used = 0usize;

    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            // Only CSI sequences carry styling; skip anything else entirely.
            if chars.peek() == Some(&'[') {
                chars.next();
                let mut buf = String::new();
                let mut final_byte = None;
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        final_byte = Some(c);
                        break;
                    }
                    buf.push(c);
                }
                if final_byte == Some('m') {
                    if !run.is_empty() {
                        spans.push(Span::styled(std::mem::take(&mut run), style));
                    }
                    let params: Vec<u8> = buf
                        .split(';')
                        .map(|p| p.trim().parse::<u8>().unwrap_or(0))
                        .collect();
                    style = apply_sgr(style, &params);
                }
            } else {
                // Non-CSI escape: drop the introducer and let the next char go.
                chars.next();
            }
            continue;
        }

        let w = ch.width().unwrap_or(0);
        if used + w > max_cols {
            break;
        }
        used += w;
        run.push(ch);
    }

    if !run.is_empty() {
        spans.push(Span::styled(run, style));
    }
    spans
}

struct Star {
    x: u16,
    y: u16,
    phase: f64,
    bright: u8,
}

pub struct SessionGridView {
    pub scope: GridScope,
    /// Window types currently shown. Shared by `c` and `f` so they can never
    /// disagree about what is on screen.
    active_types: HashSet<String>,
    filter_open: bool,
    filter_cursor: usize,
    frame_count: u32,
    stars: Vec<Star>,
    /// Captured screens, parallel to the visible window list.
    captures: Vec<Vec<String>>,
}

impl SessionGridView {
    pub fn new(scope: GridScope) -> Self {
        let mut active_types: HashSet<String> =
            WINDOW_TYPES.iter().map(|t| t.to_string()).collect();
        active_types.insert(UNTAGGED.to_string());

        // Deterministic scatter, same LCG the night sky used.
        let mut stars = Vec::new();
        let mut seed: u64 = 42;
        for _ in 0..140 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let x = ((seed >> 16) % 400) as u16;
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let y = ((seed >> 16) % 200) as u16;
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let phase = ((seed >> 16) % 628) as f64 / 100.0;
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let bright = ((seed >> 16) % 130 + 60) as u8;
            stars.push(Star { x, y, phase, bright });
        }

        Self {
            scope,
            active_types,
            filter_open: false,
            filter_cursor: 0,
            frame_count: 0,
            stars,
            captures: Vec::new(),
        }
    }

    /// Windows in this scope passing the type filter, in tmux window order so
    /// number keys stay put.
    pub fn visible<'a>(&self, windows: &'a [TmuxWindow]) -> Vec<&'a TmuxWindow> {
        let mut v: Vec<&TmuxWindow> = windows
            .iter()
            .filter(|w| w.index > 0)
            .filter(|w| self.scope.matches(w))
            .filter(|w| self.active_types.contains(type_key(w)))
            .collect();
        v.sort_by_key(|w| w.index);
        v
    }

    fn claude_only(&self) -> bool {
        self.active_types.len() == 1 && self.active_types.contains(CLAUDE)
    }

    fn toggle_claude_only(&mut self) {
        if self.claude_only() {
            self.active_types = WINDOW_TYPES.iter().map(|t| t.to_string()).collect();
            self.active_types.insert(UNTAGGED.to_string());
        } else {
            self.active_types.clear();
            self.active_types.insert(CLAUDE.to_string());
        }
    }

    fn filter_rows() -> Vec<String> {
        let mut rows: Vec<String> = WINDOW_TYPES.iter().map(|t| t.to_string()).collect();
        rows.push(UNTAGGED.to_string());
        rows
    }

    /// Pull fresh screens for the visible cells. One batched tmux call.
    pub fn tick(&mut self, windows: &[TmuxWindow]) {
        self.frame_count = self.frame_count.wrapping_add(1);

        let visible = self.visible(windows);
        let pane_ids: Vec<String> = visible.iter().map(|w| w.pane_id.clone()).collect();
        self.captures = crate::tmux::capture_panes(&pane_ids);
    }

    /// Returns `Some(window_index)` to jump to, or `None`.
    pub fn handle_input(&mut self, key: KeyCode, windows: &[TmuxWindow]) -> GridAction {
        if self.filter_open {
            return self.handle_filter_input(key);
        }

        match key {
            KeyCode::Esc | KeyCode::Char('v') => GridAction::Back,
            KeyCode::Char('c') => {
                self.toggle_claude_only();
                GridAction::None
            }
            KeyCode::Char('f') => {
                self.filter_open = true;
                self.filter_cursor = 0;
                GridAction::None
            }
            KeyCode::Char(ch @ '1'..='9') => {
                let slot = ch.to_digit(10).unwrap_or(0) as usize - 1;
                let visible = self.visible(windows);
                match visible.get(slot) {
                    Some(w) => GridAction::Jump(w.index),
                    None => GridAction::None,
                }
            }
            _ => GridAction::None,
        }
    }

    fn handle_filter_input(&mut self, key: KeyCode) -> GridAction {
        let rows = Self::filter_rows();
        match key {
            KeyCode::Esc | KeyCode::Char('f') | KeyCode::Enter => {
                self.filter_open = false;
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                self.filter_cursor = (self.filter_cursor + 1) % rows.len();
            }
            KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => {
                self.filter_cursor = if self.filter_cursor == 0 {
                    rows.len() - 1
                } else {
                    self.filter_cursor - 1
                };
            }
            KeyCode::Char(' ') => {
                if let Some(t) = rows.get(self.filter_cursor) {
                    if !self.active_types.remove(t) {
                        self.active_types.insert(t.clone());
                    }
                }
            }
            _ => {}
        }
        GridAction::None
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, windows: &[TmuxWindow]) {
        let visible = self.visible(windows);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .split(area);

        let body = chunks[1];
        self.render_stars(frame, body);

        let shown = if visible.is_empty() {
            self.render_empty(frame, body);
            0
        } else {
            self.render_cells(frame, body, &visible)
        };
        self.render_header(frame, chunks[0], visible.len(), shown);

        if self.filter_open {
            self.render_filter(frame, area);
        }
    }

    fn render_header(&self, frame: &mut Frame, area: Rect, count: usize, shown: usize) {
        let filter_note = if self.claude_only() {
            " · claude only".to_string()
        } else if self.active_types.len() < WINDOW_TYPES.len() + 1 {
            let mut t: Vec<&str> = self.active_types.iter().map(|s| s.as_str()).collect();
            t.sort();
            format!(" · {}", t.join("/"))
        } else {
            String::new()
        };

        let line = Line::from(vec![
            Span::styled(
                format!(" {} ", self.scope.title()),
                Style::default()
                    .fg(Color::Rgb(26, 26, 26))
                    .bg(Color::Rgb(184, 134, 11))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" {} session{}{}", count, if count == 1 { "" } else { "s" }, filter_note),
                Style::default().fg(Color::Rgb(184, 134, 11)),
            ),
            // Never let the grid quietly show fewer sessions than it counted.
            Span::styled(
                if shown < count {
                    format!("  ⚠ only {} fit — resize or filter", shown)
                } else {
                    String::new()
                },
                Style::default().fg(Color::Rgb(224, 168, 40)).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "   [1-9] jump  [c] claude  [f] filter  [esc] back",
                Style::default().fg(Color::DarkGray),
            ),
        ]);
        frame.render_widget(Paragraph::new(line), area);
    }

    /// Twinkling stars across the whole body. Cells paint over them, so the art
    /// only ever shows in space the grid did not claim.
    fn render_stars(&self, frame: &mut Frame, area: Rect) {
        let time = self.frame_count as f64 * 0.05;
        let buf = frame.buffer_mut();

        for star in &self.stars {
            if area.width == 0 || area.height == 0 {
                return;
            }
            let x = area.x + (star.x % area.width);
            let y = area.y + (star.y % area.height);

            let pulse = (time * 0.4 + star.phase).sin() * 0.35 + 0.65;
            let b = (star.bright as f64 * pulse).clamp(18.0, 255.0) as u8;
            let glyph = if b > 130 { "*" } else { "." };

            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_symbol(glyph);
                cell.set_fg(Color::Rgb(b, b, (b as u16 * 3 / 4) as u8));
            }
        }

        self.render_ground(frame, area);
    }

    /// Cow and cactus on the bottom strip, only when there is room to spare.
    fn render_ground(&self, frame: &mut Frame, area: Rect) {
        if area.height < 8 || area.width < 40 {
            return;
        }
        let art = [
            r"    (__)          ,|,",
            r"    (oo)         \|||/",
            r"----/\/\-----------|-----",
        ];
        let y0 = area.y + area.height.saturating_sub(art.len() as u16);
        let x0 = area.x + area.width.saturating_sub(30).min(area.width / 2);

        for (i, row) in art.iter().enumerate() {
            let y = y0 + i as u16;
            if y >= area.y + area.height {
                break;
            }
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    *row,
                    Style::default().fg(Color::Rgb(90, 70, 30)),
                ))),
                Rect {
                    x: x0,
                    y,
                    width: (area.x + area.width).saturating_sub(x0),
                    height: 1,
                },
            );
        }
    }

    fn render_empty(&self, frame: &mut Frame, area: Rect) {
        let msg = vec![
            Line::from(""),
            Line::from(Span::styled(
                "no sessions match",
                Style::default().fg(Color::Rgb(184, 134, 11)).add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                "[f] widen the filter   [c] toggle claude-only",
                Style::default().fg(Color::DarkGray),
            )),
        ];
        let h = msg.len() as u16;
        let rect = Rect {
            x: area.x,
            y: area.y + area.height.saturating_sub(h) / 2,
            width: area.width,
            height: h.min(area.height),
        };
        frame.render_widget(Paragraph::new(msg).alignment(Alignment::Center), rect);
    }

    /// Draws the cells and returns how many actually fit, so the header can
    /// admit to any shortfall.
    fn render_cells(&self, frame: &mut Frame, area: Rect, visible: &[&TmuxWindow]) -> usize {
        let (cols, rows) = grid_dims(visible.len(), area);
        if cols == 0 || rows == 0 {
            return 0;
        }
        let capacity = cols * rows;

        // Leave a margin so the art has somewhere to live.
        let pad_x = if area.width > 20 { 2 } else { 0 };
        let pad_y = if area.height > 12 { 1 } else { 0 };
        let inner = Rect {
            x: area.x + pad_x,
            y: area.y + pad_y,
            width: area.width.saturating_sub(pad_x * 2),
            height: area.height.saturating_sub(pad_y * 2),
        };

        let cell_w = inner.width / cols as u16;
        let cell_h = inner.height / rows as u16;
        if cell_w < MIN_CELL_W || cell_h < MIN_CELL_H {
            return 0;
        }

        let shown = visible.len().min(capacity);
        for (slot, win) in visible.iter().enumerate().take(capacity) {
            let cx = slot % cols;
            let cy = slot / cols;
            let rect = Rect {
                x: inner.x + cx as u16 * cell_w,
                y: inner.y + cy as u16 * cell_h,
                width: cell_w.saturating_sub(1),
                height: cell_h.saturating_sub(1),
            };
            self.render_cell(frame, rect, slot, win);
        }
        shown
    }

    fn render_cell(&self, frame: &mut Frame, rect: Rect, slot: usize, win: &TmuxWindow) {
        let status = signals::read_signal(&win.pane_id).map(|s| s.status);
        let (color, badge) = match status {
            Some(SessionStatus::Waiting) => (Color::Rgb(224, 168, 40), "WAITING"),
            Some(SessionStatus::Error) => (Color::Rgb(200, 70, 60), "ERROR"),
            Some(SessionStatus::Working) => (Color::Rgb(110, 170, 100), "working"),
            Some(SessionStatus::Idle) => (Color::Rgb(110, 110, 110), "idle"),
            None => (Color::Rgb(90, 90, 90), ""),
        };
        let needs_attention = matches!(
            status,
            Some(SessionStatus::Waiting) | Some(SessionStatus::Error)
        );

        let number = if slot < MAX_NUMBERED {
            format!(" {} ", slot + 1)
        } else {
            " · ".to_string()
        };
        let title = Line::from(vec![
            Span::styled(
                number,
                Style::default()
                    .fg(Color::Rgb(26, 26, 26))
                    .bg(color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" {} ", win.name), Style::default().fg(color)),
            Span::styled(
                if badge.is_empty() { String::new() } else { format!("{} ", badge) },
                Style::default().fg(color).add_modifier(if needs_attention {
                    Modifier::BOLD
                } else {
                    Modifier::DIM
                }),
            ),
        ]);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(if needs_attention {
                BorderType::Thick
            } else {
                BorderType::Plain
            })
            .border_style(Style::default().fg(color))
            .title(title);

        let text_area = block.inner(rect);
        // Cells paint over the starfield rather than blending with it.
        frame.render_widget(ratatui::widgets::Clear, rect);
        frame.render_widget(block, rect);

        if text_area.width == 0 || text_area.height == 0 {
            return;
        }

        let empty = Vec::new();
        let screen = self.captures.get(slot).unwrap_or(&empty);
        let h = text_area.height as usize;
        let w = text_area.width as usize;

        // Trailing blank lines are dead space; drop them so the tail of the
        // conversation sits against the bottom of the cell.
        let end = screen
            .iter()
            .rposition(|l| !l.trim().is_empty())
            .map(|i| i + 1)
            .unwrap_or(0);
        let start = end.saturating_sub(h);

        let base = Style::default().fg(Color::Rgb(170, 170, 170));
        let lines: Vec<Line> = screen[start..end]
            .iter()
            .map(|l| Line::from(ansi_spans(l, w, base)))
            .collect();

        frame.render_widget(Paragraph::new(lines), text_area);
    }

    fn render_filter(&self, frame: &mut Frame, area: Rect) {
        let rows = Self::filter_rows();
        let width = 30u16.min(area.width);
        let height = (rows.len() as u16 + 4).min(area.height);
        let rect = Rect {
            x: area.x + (area.width.saturating_sub(width)) / 2,
            y: area.y + (area.height.saturating_sub(height)) / 2,
            width,
            height,
        };

        let mut lines: Vec<Line> = Vec::new();
        for (i, t) in rows.iter().enumerate() {
            let on = self.active_types.contains(t);
            let cursor = i == self.filter_cursor;
            lines.push(Line::from(vec![
                Span::styled(
                    if cursor { " ❯ " } else { "   " },
                    Style::default().fg(Color::Rgb(218, 165, 32)),
                ),
                Span::styled(
                    if on { "[x] " } else { "[ ] " },
                    Style::default().fg(if on {
                        Color::Rgb(218, 165, 32)
                    } else {
                        Color::DarkGray
                    }),
                ),
                Span::styled(
                    t.clone(),
                    Style::default().fg(if on { Color::White } else { Color::DarkGray }),
                ),
            ]));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  space toggle · esc close",
            Style::default().fg(Color::DarkGray),
        )));

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Rgb(184, 134, 11)))
            .title(Span::styled(
                " filter ",
                Style::default()
                    .fg(Color::Rgb(218, 165, 32))
                    .add_modifier(Modifier::BOLD),
            ));

        frame.render_widget(ratatui::widgets::Clear, rect);
        frame.render_widget(Paragraph::new(lines).block(block), rect);
    }
}

pub enum GridAction {
    None,
    Back,
    Jump(u32),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(w: u16, h: u16) -> Rect {
        Rect { x: 0, y: 0, width: w, height: h }
    }

    fn win(index: u32, ty: &str, project: &str, barn: &str) -> TmuxWindow {
        TmuxWindow {
            index,
            name: format!("w{}", index),
            active: false,
            pane_id: format!("%{}", index),
            pane_title: String::new(),
            pane_current_command: String::new(),
            window_activity: 0,
            window_type: ty.to_string(),
            project: project.to_string(),
            barn: barn.to_string(),
        }
    }

    #[test]
    fn tiling_grows_with_session_count_on_a_roomy_terminal() {
        let a = area(160, 43);
        assert_eq!(grid_dims(1, a), (1, 1));
        assert_eq!(grid_dims(2, a), (2, 1));
        assert_eq!(grid_dims(4, a), (2, 2));
        assert_eq!(grid_dims(6, a), (3, 2));
        assert_eq!(grid_dims(9, a), (3, 3));
    }

    #[test]
    fn no_sessions_means_no_tiling() {
        assert_eq!(grid_dims(0, area(160, 43)), (0, 0));
    }

    #[test]
    fn tiling_shrinks_rather_than_rendering_unreadable_cells() {
        // 60 cols only fits two columns at the 24-col floor.
        let (cols, _) = grid_dims(9, area(60, 43));
        assert_eq!(cols, 2);

        // A short terminal loses rows before it loses columns.
        let (_, rows) = grid_dims(9, area(160, 11));
        assert_eq!(rows, 2);
    }

    #[test]
    fn tiling_has_room_for_every_session_up_to_the_numbered_limit() {
        // If capacity ever drops below the count on a normal terminal, cells
        // vanish with no indication. The header warns, but the common cases
        // should never need the warning.
        let a = area(160, 43);
        for n in 1..=MAX_NUMBERED {
            let (cols, rows) = grid_dims(n, a);
            assert!(
                cols * rows >= n,
                "{n} sessions only got {cols}x{rows} = {} cells",
                cols * rows
            );
        }
    }

    fn plain(spans: &[Span]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn ansi_parser_strips_escapes_and_keeps_the_text() {
        let s = "\u{1b}[38;5;65mhello\u{1b}[39m world";
        let spans = ansi_spans(s, 80, Style::default());
        assert_eq!(plain(&spans), "hello world");
    }

    #[test]
    fn ansi_parser_applies_256_colour_and_bold() {
        let s = "\u{1b}[1m\u{1b}[38;5;105mX";
        let spans = ansi_spans(s, 80, Style::default());
        let last = spans.last().expect("a span");
        assert_eq!(last.style.fg, Some(Color::Indexed(105)));
        assert!(last.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn ansi_parser_handles_truecolour() {
        let s = "\u{1b}[38;2;10;20;30mX";
        let spans = ansi_spans(s, 80, Style::default());
        assert_eq!(spans.last().unwrap().style.fg, Some(Color::Rgb(10, 20, 30)));
    }

    #[test]
    fn ansi_reset_returns_to_default() {
        let s = "\u{1b}[38;5;9mred\u{1b}[0mplain";
        let spans = ansi_spans(s, 80, Style::default());
        assert_eq!(spans[0].style.fg, Some(Color::Indexed(9)));
        assert_eq!(spans[1].style.fg, None);
    }

    #[test]
    fn escape_sequences_do_not_consume_the_width_budget() {
        // The escapes are invisible, so all six visible chars must survive.
        let s = "\u{1b}[38;5;65mabc\u{1b}[39m\u{1b}[1mdef\u{1b}[0m";
        assert_eq!(plain(&ansi_spans(s, 6, Style::default())), "abcdef");
    }

    #[test]
    fn ansi_truncation_cuts_visible_text_not_escapes() {
        let s = "\u{1b}[38;5;65mabcdefghij\u{1b}[39m";
        assert_eq!(plain(&ansi_spans(s, 4, Style::default())), "abcd");
    }

    #[test]
    fn ansi_parser_never_emits_raw_escape_bytes() {
        // Whatever we fail to understand must still not reach the screen.
        let nasty = "\u{1b}[38;5;65mok\u{1b}[2J\u{1b}[10;20Hmore\u{1b}]0;title\u{7}end";
        let spans = ansi_spans(nasty, 80, Style::default());
        let text = plain(&spans);
        assert!(!text.contains('\u{1b}'), "escape leaked: {text:?}");
    }

    #[test]
    fn ansi_parser_handles_wide_glyphs_under_truncation() {
        let s = "\u{1b}[1m日本語\u{1b}[0m";
        assert_eq!(plain(&ansi_spans(s, 5, Style::default())), "日本");
    }

    #[test]
    fn bare_escape_m_is_treated_as_a_reset() {
        let s = "\u{1b}[38;5;9mred\u{1b}[mplain";
        let spans = ansi_spans(s, 80, Style::default());
        assert_eq!(spans[1].style.fg, None);
    }

    #[test]
    fn truncation_counts_display_columns_not_chars() {
        let st = Style::default();
        assert_eq!(plain(&ansi_spans("hello world", 5, st)), "hello");
        assert_eq!(plain(&ansi_spans("hi", 10, st)), "hi");
        assert_eq!(plain(&ansi_spans("", 5, st)), "");
    }

    #[test]
    fn truncation_never_splits_a_wide_glyph() {
        // Each CJK char is two columns wide; an odd budget must not emit a half.
        let st = Style::default();
        assert_eq!(plain(&ansi_spans("日本語", 4, st)), "日本");
        assert_eq!(plain(&ansi_spans("日本語", 5, st)), "日本");
        assert_eq!(plain(&ansi_spans("日本語", 6, st)), "日本語");
        assert_eq!(plain(&ansi_spans("日本語", 1, st)), "");
    }

    #[test]
    fn truncation_handles_box_drawing_used_by_claude_output() {
        let spans = ansi_spans("──────────", 3, Style::default());
        assert_eq!(plain(&spans).chars().count(), 3);
    }

    #[test]
    fn claude_toggle_narrows_then_restores() {
        let mut v = SessionGridView::new(GridScope::All);
        assert!(!v.claude_only());

        v.toggle_claude_only();
        assert!(v.claude_only());

        v.toggle_claude_only();
        assert!(!v.claude_only());
        assert!(v.active_types.contains("shell"));
        assert!(v.active_types.contains("ssh"));
    }

    #[test]
    fn window_zero_is_never_a_cell() {
        let v = SessionGridView::new(GridScope::All);
        let windows = vec![
            win(0, "", "", ""),
            win(1, "claude", "P", "local"),
        ];
        let visible = v.visible(&windows);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].index, 1);
    }

    #[test]
    fn project_scope_uses_exact_tags_not_name_prefixes() {
        let v = SessionGridView::new(GridScope::Project("Kill".to_string()));
        let windows = vec![
            win(1, "claude", "Kill", "local"),
            win(2, "claude", "Kill Switch", "local"),
        ];
        let visible = v.visible(&windows);
        assert_eq!(visible.len(), 1, "\"Kill Switch\" must not fall into \"Kill\"");
        assert_eq!(visible[0].index, 1);
    }

    #[test]
    fn barn_scope_filters_by_barn_tag() {
        let v = SessionGridView::new(GridScope::Barn("local".to_string()));
        let windows = vec![
            win(1, "claude", "P", "local"),
            win(2, "ssh", "P", "prod"),
        ];
        let visible = v.visible(&windows);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].index, 1);
    }

    #[test]
    fn claude_only_filter_hides_other_types() {
        let mut v = SessionGridView::new(GridScope::All);
        let windows = vec![
            win(1, "claude", "P", "local"),
            win(2, "shell", "P", "local"),
            win(3, "ssh", "P", "local"),
        ];
        assert_eq!(v.visible(&windows).len(), 3);

        v.toggle_claude_only();
        let visible = v.visible(&windows);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].window_type, "claude");
    }

    #[test]
    fn untagged_windows_show_in_all_scope_but_not_a_project_scope() {
        let legacy = vec![win(1, "", "", "")];

        let all = SessionGridView::new(GridScope::All);
        assert_eq!(all.visible(&legacy).len(), 1, "legacy windows must not vanish");

        let scoped = SessionGridView::new(GridScope::Project("P".to_string()));
        assert_eq!(scoped.visible(&legacy).len(), 0);
    }

    #[test]
    fn cells_stay_in_window_order_so_number_keys_do_not_move() {
        let v = SessionGridView::new(GridScope::All);
        let windows = vec![
            win(8, "claude", "P", "local"),
            win(2, "claude", "P", "local"),
            win(5, "claude", "P", "local"),
        ];
        let idx: Vec<u32> = v.visible(&windows).iter().map(|w| w.index).collect();
        assert_eq!(idx, vec![2, 5, 8]);
    }

    #[test]
    fn number_key_jumps_to_the_window_in_that_slot() {
        let mut v = SessionGridView::new(GridScope::All);
        let windows = vec![
            win(2, "claude", "P", "local"),
            win(5, "claude", "P", "local"),
        ];
        match v.handle_input(KeyCode::Char('2'), &windows) {
            GridAction::Jump(i) => assert_eq!(i, 5),
            _ => panic!("expected a jump"),
        }
    }

    #[test]
    fn number_key_past_the_last_cell_does_nothing() {
        let mut v = SessionGridView::new(GridScope::All);
        let windows = vec![win(2, "claude", "P", "local")];
        assert!(matches!(
            v.handle_input(KeyCode::Char('7'), &windows),
            GridAction::None
        ));
    }

    #[test]
    fn filter_panel_swallows_keys_while_open() {
        let mut v = SessionGridView::new(GridScope::All);
        let windows = vec![win(2, "claude", "P", "local")];

        v.handle_input(KeyCode::Char('f'), &windows);
        assert!(v.filter_open);

        // '1' would jump if the panel were closed.
        assert!(matches!(
            v.handle_input(KeyCode::Char('1'), &windows),
            GridAction::None
        ));

        v.handle_input(KeyCode::Esc, &windows);
        assert!(!v.filter_open);
    }

    #[test]
    fn space_toggles_a_type_off_and_on_in_the_filter() {
        let mut v = SessionGridView::new(GridScope::All);
        let windows = vec![win(1, "claude", "P", "local")];

        v.handle_input(KeyCode::Char('f'), &windows);
        // Cursor starts on the first row, which is "claude".
        v.handle_input(KeyCode::Char(' '), &windows);
        assert!(!v.active_types.contains(CLAUDE));
        assert_eq!(v.visible(&windows).len(), 0);

        v.handle_input(KeyCode::Char(' '), &windows);
        assert!(v.active_types.contains(CLAUDE));
        assert_eq!(v.visible(&windows).len(), 1);
    }

    #[test]
    fn esc_leaves_the_grid_when_the_filter_is_closed() {
        let mut v = SessionGridView::new(GridScope::All);
        assert!(matches!(v.handle_input(KeyCode::Esc, &[]), GridAction::Back));
        assert!(matches!(v.handle_input(KeyCode::Char('v'), &[]), GridAction::Back));
    }
}
