use std::collections::{HashMap, HashSet};

use crossterm::event::KeyCode;
use ratatui::prelude::*;
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use unicode_width::UnicodeWidthChar;

use crate::remote_grid::RemoteFrame;
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

/// Where a session physically lives.
///
/// The derived `Ord` is load-bearing: `Local` sorts before every `Barn(_)` and
/// barns sort alphabetically, which *is* the cell ordering for the grid. Do not
/// reorder the variants.
///
/// **Not `GridScope::Barn`.** `GridScope::Barn(b)` selects *local* windows whose
/// work targets barn `b` (an ssh window into it, a log tail). `Origin::Barn(b)`
/// means the session is running **on** that barn. A local ssh window into
/// `guided` is `Origin::Local` with `barn == "guided"`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Origin {
    Local,
    Barn(String),
}

/// How an origin is named in the filter panel and the header.
fn origin_label(origin: &Origin) -> String {
    match origin {
        Origin::Local => "local".to_string(),
        Origin::Barn(barn) => barn.clone(),
    }
}

/// One line of the filter panel.
///
/// Types and sources are different questions asked by the same cursor: a type
/// row toggles `active_types`, a source row toggles `hidden_origins`. Keeping
/// them in one list is what lets `j`/`k`/`space` stay a single code path across
/// the separator.
#[derive(Debug, Clone, PartialEq)]
enum FilterRow {
    Type(String),
    Origin(Origin),
}

/// One tile of the grid: a window and the host it lives on.
///
/// The window is borrowed from whichever side supplied it — `App.windows` for
/// local, a [`RemoteFrame`] for a barn — so a cell never owns a copy of state
/// that is refreshed four times a second.
#[derive(Debug)]
pub struct GridCell<'a> {
    pub origin: Origin,
    pub window: &'a TmuxWindow,
}

/// Barn red, the same `#8b1a1a` a connected barn's status bar wears.
///
/// Remote cells take it **unconditionally**. Remoteness is a property of the
/// cell, not a state, so the border stops being the status channel there — which
/// is why [`SessionGridView::render_cell`] keeps the status colour on the number
/// chip and the badge. Lose those two and WAITING is invisible on a barn.
const BARN_RED: Color = Color::Rgb(0x8b, 0x1a, 0x1a);

/// A stale cell's border: barn red with the life taken out of it.
///
/// Dimmed rather than recoloured, because the cell has not stopped being a
/// barn's — it has stopped being *current*. A `Modifier::DIM` would say the same
/// thing on terminals that honour it and nothing at all on the ones that do not,
/// and this is the difference between "these sessions are live" and "these
/// sessions are a photograph".
const STALE_BORDER: Color = Color::Rgb(0x4e, 0x1e, 0x1e);

/// A stale cell's number chip, title and badge.
///
/// Grey, and not the status colours, because a frozen frame's status is frozen
/// too: remote freshness is measured against the barn's own clock, which stopped
/// arriving with the last frame, so a session that was WAITING when the stream
/// died would go on saying WAITING in amber for as long as the grid stayed open.
const STALE_FG: Color = Color::Rgb(120, 120, 120);

/// The header's `· stale: <barn>` note. Not amber — that is the "only N fit"
/// warning's colour, and these are different problems.
const STALE_NOTE: Color = Color::Rgb(190, 90, 80);

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

/// The keys the header offers, in the order it draws them.
///
/// Every one is answered by [`SessionGridView::handle_input`]. `[l] local` is
/// the newest and was live, drawn on no line anywhere, for a whole task.
const HINTS: &[&str] = &["[1-9] jump", "[c] claude", "[l] local", "[f] filter", "[esc] back"];

/// Drawn last and dropped last.
///
/// The header has been over budget since before remote cells existed — a full
/// type note plus a sources note plus the stale note is already past 120
/// columns — and the span order is the priority order, so the hints are what a
/// crowded header cuts. Six hints do not fit beside a title and a count on an
/// 80-column terminal either. So they fall off the right as room runs out, and
/// this one is pinned to the end of whatever is left: it is the only hint whose
/// absence costs the user a key they cannot then find, because it is the one
/// that lists the rest.
const HELP_HINT: &str = "[?] help";

/// Between the last header note and the first hint, and between hints.
const HINT_LEAD: &str = "   ";
const HINT_GAP: &str = "  ";

/// As much of the hint line as fits in `budget` columns, always ending at a
/// hint boundary.
///
/// Never a truncation. A `Paragraph` cutting this line mid-hint gives
/// `[esc] bac`, which reads as a typo rather than as a terminal that is too
/// narrow, and RG-9 measured exactly that happening inside `[c] claude` on a
/// crowded 120-column header.
fn hint_line(budget: usize) -> String {
    let mut shown = HINTS.len();
    loop {
        let mut parts: Vec<&str> = HINTS[..shown].to_vec();
        parts.push(HELP_HINT);
        let line = format!("{}{}", HINT_LEAD, parts.join(HINT_GAP));
        if display_width(&line) <= budget {
            return line;
        }
        if shown == 0 {
            // Not even `[?] help` fits. The bottom bar still carries it.
            return String::new();
        }
        shown -= 1;
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

/// Display columns a string occupies. Barn names are user-supplied and nothing
/// stops them holding a wide glyph.
fn display_width(s: &str) -> usize {
    s.chars().map(|c| c.width().unwrap_or(0)).sum()
}

/// Truncate to `max` display columns, marking the cut with `…`.
///
/// Never splits a wide glyph, for the same reason [`ansi_spans`] does not: half
/// a character is a rendering artefact tmux would then have to redraw around.
fn truncate_cols(s: &str, max: usize) -> String {
    if display_width(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for c in s.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > max - 1 {
            break;
        }
        used += w;
        out.push(c);
    }
    out.push('…');
    out
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
    /// Captured screens of local panes, keyed by `pane_id`.
    ///
    /// Keyed rather than parallel to `visible()`: `tick` and `render` each call
    /// `visible()` independently, so any change between them — a filter keypress,
    /// a window opening or closing — used to slide every screen into a
    /// neighbour's cell. Looking up by pane id means a change can only ever show
    /// *fewer* cells, never wrong ones.
    ///
    /// Local only. Pane ids collide across hosts (`%1` exists on every machine),
    /// so remote captures must stay partitioned per barn rather than merge here.
    local_captures: HashMap<String, Vec<String>>,
    /// Sources currently switched **off**, by `l` or by a row in the filter
    /// panel.
    ///
    /// Hidden rather than active, and that is not a detail. Barns come and go as
    /// you connect and disconnect, so an active set would have to learn about
    /// each new barn in order to show it — and the barn you just connected to
    /// would arrive invisible, with nothing on screen to say why. A hidden set
    /// defaults every origin it has never heard of to visible.
    hidden_origins: HashSet<Origin>,
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
            local_captures: HashMap::new(),
            hidden_origins: HashSet::new(),
        }
    }

    /// Does this window survive the filters, on the host it lives on?
    ///
    /// The one place any filter is answered. Numbering, key resolution and the
    /// screen all read the grid through here, so a cell can never be drawn
    /// under a number that resolves to something else, and `l` can never mean
    /// one thing to `cells()` and another to the remote merge.
    ///
    /// `origin` and `scope` are separate questions and both are asked. `scope`
    /// reads the window's own `@yeehaw_barn` tag, which is where its *work*
    /// points; `origin` is the machine it runs on. A local `ssh` window into
    /// `guided` is `Origin::Local` tagged `guided`, and `GridScope::Barn`
    /// keeps it while a local-only filter still would too.
    fn shows(&self, w: &TmuxWindow, origin: &Origin) -> bool {
        !self.hidden_origins.contains(origin)
            && w.index > 0
            && self.scope.matches(w)
            && self.active_types.contains(type_key(w))
    }

    /// Local windows in this scope passing the filters, in tmux window order so
    /// number keys stay put.
    pub fn visible<'a>(&self, windows: &'a [TmuxWindow]) -> Vec<&'a TmuxWindow> {
        let mut v: Vec<&TmuxWindow> = windows
            .iter()
            .filter(|w| self.shows(w, &Origin::Local))
            .collect();
        v.sort_by_key(|w| w.index);
        v
    }

    /// Every tile on the grid, in display order: local first, then barns
    /// alphabetically, then window index.
    ///
    /// Ordering is the whole contract. `remote` is a `HashMap`, so its iteration
    /// order is arbitrary from one frame to the next — the sort is not a
    /// tidy-up, it is what makes the grid deterministic at all. And local
    /// sorting ahead of every barn is what keeps a barn connecting or dropping
    /// from renumbering the cells under the user's fingers.
    pub fn cells<'a>(
        &self,
        local: &'a [TmuxWindow],
        remote: &'a HashMap<String, RemoteFrame>,
    ) -> Vec<GridCell<'a>> {
        let mut cells: Vec<GridCell<'a>> = self
            .visible(local)
            .into_iter()
            .map(|window| GridCell { origin: Origin::Local, window })
            .collect();

        for (barn, frame) in remote {
            let origin = Origin::Barn(barn.clone());
            cells.extend(
                frame
                    .windows
                    .iter()
                    .filter(|w| self.shows(w, &origin))
                    .map(|window| GridCell { origin: origin.clone(), window }),
            );
        }

        // Derived `Ord` on `Origin` gives local-first then barns alphabetically;
        // see the note on the enum.
        cells.sort_by(|a, b| (&a.origin, a.window.index).cmp(&(&b.origin, b.window.index)));
        cells
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

    /// Every source the grid could be showing right now: local, then each barn
    /// with a frame, in `Origin` order.
    ///
    /// Built from what is **present**, not from what survived the filters. A
    /// barn switched off by its own row has to keep that row or there is
    /// nothing left to switch it back on with, and a barn that disconnected has
    /// to lose its row or the panel offers a toggle for something that is not
    /// there.
    fn present_origins(&self, remote: &HashMap<String, RemoteFrame>) -> Vec<Origin> {
        let mut origins: Vec<Origin> = remote.keys().map(|b| Origin::Barn(b.clone())).collect();
        origins.sort();
        let mut all = vec![Origin::Local];
        all.append(&mut origins);
        all
    }

    /// Local showing and every connected barn hidden — the state `l` toggles.
    ///
    /// Derived from `hidden_origins` rather than kept as its own flag, for the
    /// same reason `claude_only` is derived from `active_types`: a second copy
    /// of the answer is a header that eventually describes a grid nobody is
    /// looking at. With no barns connected there is nothing to be "only" of, so
    /// this is false and the header stays quiet.
    fn local_only(&self, remote: &HashMap<String, RemoteFrame>) -> bool {
        !remote.is_empty()
            && !self.hidden_origins.contains(&Origin::Local)
            && remote
                .keys()
                .all(|b| self.hidden_origins.contains(&Origin::Barn(b.clone())))
    }

    /// `l`. The mirror of `c`: on, then off again.
    ///
    /// Only *present* barns are touched. A barn that is not connected has no
    /// cells to hide, so hiding it would be invisible now and surprising later,
    /// when connecting to it produced nothing.
    fn toggle_local_only(&mut self, remote: &HashMap<String, RemoteFrame>) {
        if self.local_only(remote) {
            for barn in remote.keys() {
                self.hidden_origins.remove(&Origin::Barn(barn.clone()));
            }
        } else {
            // "Local only" has to mean local is on screen, whatever the panel
            // did to it earlier — otherwise `l` can leave an empty grid.
            self.hidden_origins.remove(&Origin::Local);
            for barn in remote.keys() {
                self.hidden_origins.insert(Origin::Barn(barn.clone()));
            }
        }
    }

    /// The filter panel's rows: the window types, then the sources.
    ///
    /// The separator between the two sections is drawn by `render_filter` and
    /// is deliberately **not** a row — it is not selectable, so counting it
    /// here would put a dead stop in the middle of `j`.
    fn filter_rows(&self, remote: &HashMap<String, RemoteFrame>) -> Vec<FilterRow> {
        let mut rows: Vec<FilterRow> = WINDOW_TYPES
            .iter()
            .map(|t| FilterRow::Type(t.to_string()))
            .collect();
        rows.push(FilterRow::Type(UNTAGGED.to_string()));
        rows.extend(
            self.present_origins(remote)
                .into_iter()
                .map(FilterRow::Origin),
        );
        rows
    }

    /// Pull fresh screens for the visible cells. One batched tmux call.
    pub fn tick(&mut self, windows: &[TmuxWindow]) {
        self.frame_count = self.frame_count.wrapping_add(1);

        let visible = self.visible(windows);
        let pane_ids: Vec<String> = visible.iter().map(|w| w.pane_id.clone()).collect();
        let screens = crate::tmux::capture_panes(&pane_ids);
        self.store_captures(pane_ids, screens);
    }

    /// Everything `tick` does with the batched capture once tmux has answered.
    ///
    /// `capture_panes` contracts to return exactly one entry per requested pane,
    /// in order, so the zip is total.
    fn store_captures(&mut self, pane_ids: Vec<String>, screens: Vec<Vec<String>>) {
        self.local_captures = pane_ids.into_iter().zip(screens).collect();
    }

    /// One keypress on the grid.
    ///
    /// `remote` is what makes a number key over a barn's cell mean anything.
    /// Cell *numbering* has counted remote cells since the merge, but this
    /// resolved the key against `visible()` — local only — so every press past
    /// the last local cell hit `None` and did nothing at all, under a number
    /// the grid had drawn on the cell.
    pub fn handle_input(
        &mut self,
        key: KeyCode,
        windows: &[TmuxWindow],
        remote: &HashMap<String, RemoteFrame>,
    ) -> GridAction {
        if self.filter_open {
            return self.handle_filter_input(key, remote);
        }

        match key {
            KeyCode::Esc | KeyCode::Char('v') => GridAction::Back,
            KeyCode::Char('c') => {
                self.toggle_claude_only();
                GridAction::None
            }
            KeyCode::Char('l') => {
                self.toggle_local_only(remote);
                GridAction::None
            }
            KeyCode::Char('f') => {
                self.filter_open = true;
                self.filter_cursor = 0;
                GridAction::None
            }
            KeyCode::Char(ch @ '1'..='9') => {
                let slot = ch.to_digit(10).unwrap_or(0) as usize - 1;
                // `cells`, never `visible`: this must resolve against the exact
                // list, in the exact order, that `render_cells` numbered. Any
                // second source of truth here is a number drawn on one session
                // that jumps to another.
                match self.cells(windows, remote).get(slot) {
                    Some(cell) => GridAction::Jump {
                        origin: cell.origin.clone(),
                        window_index: cell.window.index,
                    },
                    None => GridAction::None,
                }
            }
            _ => GridAction::None,
        }
    }

    fn handle_filter_input(
        &mut self,
        key: KeyCode,
        remote: &HashMap<String, RemoteFrame>,
    ) -> GridAction {
        let rows = self.filter_rows(remote);
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
            KeyCode::Char(' ') => match rows.get(self.filter_cursor) {
                Some(FilterRow::Type(t)) => {
                    if !self.active_types.remove(t) {
                        self.active_types.insert(t.clone());
                    }
                }
                // The set stores what is *off*, so removing is showing.
                Some(FilterRow::Origin(o)) => {
                    if !self.hidden_origins.remove(o) {
                        self.hidden_origins.insert(o.clone());
                    }
                }
                None => {}
            },
            _ => {}
        }
        GridAction::None
    }

    /// `stale` names the barns whose stream has died. Their cells stay exactly
    /// where they were, drawn from the last frame — dropping them would
    /// renumber every cell after them under the user's fingers, and the number
    /// keys are muscle memory.
    pub fn render(
        &self,
        frame: &mut Frame,
        area: Rect,
        windows: &[TmuxWindow],
        remote: &HashMap<String, RemoteFrame>,
        stale: &HashSet<&str>,
    ) {
        let cells = self.cells(windows, remote);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .split(area);

        let body = chunks[1];
        self.render_stars(frame, body);

        let shown = if cells.is_empty() {
            self.render_empty(frame, body);
            0
        } else {
            self.render_cells(frame, body, &cells, remote, stale)
        };
        self.render_header(frame, chunks[0], cells.len(), shown, remote, stale);

        if self.filter_open {
            self.render_filter(frame, area, remote);
        }
    }

    /// What the header says about the source filter.
    ///
    /// A hidden cell takes its number with it and leaves nothing behind, so the
    /// header has to name what is being held back — the job `· claude only` has
    /// done since the grid shipped. Naming the barns individually rather than
    /// always saying "local only" matters: with one barn of two hidden, "local
    /// only" would be a plain lie about what is on screen.
    fn source_note(&self, remote: &HashMap<String, RemoteFrame>) -> String {
        if self.local_only(remote) {
            return " · local only".to_string();
        }
        let hidden: Vec<String> = self
            .present_origins(remote)
            .iter()
            .filter(|o| self.hidden_origins.contains(o))
            .map(origin_label)
            .collect();
        if hidden.is_empty() {
            String::new()
        } else {
            format!(" · hiding {}", hidden.join("/"))
        }
    }

    fn render_header(
        &self,
        frame: &mut Frame,
        area: Rect,
        count: usize,
        shown: usize,
        remote: &HashMap<String, RemoteFrame>,
        stale: &HashSet<&str>,
    ) {
        let filter_note = if self.claude_only() {
            " · claude only".to_string()
        } else if self.active_types.len() < WINDOW_TYPES.len() + 1 {
            let mut t: Vec<&str> = self.active_types.iter().map(|s| s.as_str()).collect();
            t.sort();
            format!(" · {}", t.join("/"))
        } else {
            String::new()
        };

        let mut spans = vec![
            Span::styled(
                format!(" {} ", self.scope.title()),
                Style::default()
                    .fg(Color::Rgb(26, 26, 26))
                    .bg(Color::Rgb(184, 134, 11))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    " {} session{}{}{}",
                    count,
                    if count == 1 { "" } else { "s" },
                    filter_note,
                    self.source_note(remote)
                ),
                Style::default().fg(Color::Rgb(184, 134, 11)),
            ),
            // A dead barn does not change the count — its cells are still there —
            // so the header is where the grid says which of them are a
            // photograph. Named individually: with two barns up and one gone, a
            // bare "stale" would leave the user guessing which half of the
            // screen to believe.
            //
            // Sorted, and from `stale` rather than from the frames, so a barn
            // that died before it ever produced one — the `connect`-parked-on-
            // unreachable case, which has no cells to badge — is still named.
            Span::styled(
                if stale.is_empty() {
                    String::new()
                } else {
                    let mut names: Vec<&str> = stale.iter().copied().collect();
                    names.sort_unstable();
                    format!(" · stale: {}", names.join("/"))
                },
                Style::default().fg(STALE_NOTE).add_modifier(Modifier::BOLD),
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
        ];

        // The hints are drawn last and dropped last, so they get whatever the
        // notes did not spend rather than a fixed allowance. Measuring here —
        // instead of assuming a width — is what keeps `hint_line`'s guarantee
        // true: it can only end at a hint boundary if it knows the real budget.
        //
        // `Paragraph` would otherwise cut this line wherever the column ran out,
        // which lands mid-hint and renders `[esc] bac` — a typo, not a signal
        // that the terminal is too narrow.
        let spent: usize = spans.iter().map(|s| display_width(&s.content)).sum();
        let hints = hint_line((area.width as usize).saturating_sub(spent));
        if !hints.is_empty() {
            spans.push(Span::styled(hints, Style::default().fg(Color::DarkGray)));
        }

        frame.render_widget(Paragraph::new(Line::from(spans)), area);
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
    fn render_cells(
        &self,
        frame: &mut Frame,
        area: Rect,
        cells: &[GridCell],
        remote: &HashMap<String, RemoteFrame>,
        stale: &HashSet<&str>,
    ) -> usize {
        let (cols, rows) = grid_dims(cells.len(), area);
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

        let shown = cells.len().min(capacity);
        for (slot, cell) in cells.iter().enumerate().take(capacity) {
            let cx = slot % cols;
            let cy = slot / cols;
            let rect = Rect {
                x: inner.x + cx as u16 * cell_w,
                y: inner.y + cy as u16 * cell_h,
                width: cell_w.saturating_sub(1),
                height: cell_h.saturating_sub(1),
            };
            self.render_cell(frame, rect, slot, cell, remote, stale);
        }
        shown
    }

    fn render_cell(
        &self,
        frame: &mut Frame,
        rect: Rect,
        slot: usize,
        cell: &GridCell,
        remote: &HashMap<String, RemoteFrame>,
        stale: &HashSet<&str>,
    ) {
        let win = cell.window;
        // A cell is stale when the barn it lives on has no stream any more. The
        // cell stays exactly where it was — dropping it would renumber every
        // cell after it, which is the one thing this feature may never do — and
        // says so instead.
        let is_stale = match &cell.origin {
            Origin::Local => false,
            Origin::Barn(barn) => stale.contains(barn.as_str()),
        };
        let status = match &cell.origin {
            Origin::Local => signals::read_signal(&win.pane_id).map(|s| s.status),
            // `fresh_signal`, never `read_signal` and never a hand-rolled map
            // lookup: it sanitizes the pane id and measures age against the
            // *barn's* clock in one call. Against ours, a barn five minutes
            // behind renders a wall of statusless cells with nothing to say why.
            Origin::Barn(barn) => remote
                .get(barn)
                .and_then(|f| f.fresh_signal(&win.pane_id))
                .map(|s| s.status.clone()),
        };
        let (color, badge) = match status {
            _ if is_stale => (STALE_FG, "STALE"),
            Some(SessionStatus::Waiting) => (Color::Rgb(224, 168, 40), "WAITING"),
            Some(SessionStatus::Error) => (Color::Rgb(200, 70, 60), "ERROR"),
            Some(SessionStatus::Working) => (Color::Rgb(110, 170, 100), "working"),
            Some(SessionStatus::Idle) => (Color::Rgb(110, 110, 110), "idle"),
            None => (Color::Rgb(90, 90, 90), ""),
        };
        // A stale cell never shouts. The status it would shout with came from a
        // frame that stopped arriving, and a thick border demanding attention
        // for a session nobody can reach is worse than no border at all.
        let needs_attention = !is_stale
            && matches!(
                status,
                Some(SessionStatus::Waiting) | Some(SessionStatus::Error)
            );

        let number = if slot < MAX_NUMBERED {
            format!(" {} ", slot + 1)
        } else {
            " · ".to_string()
        };
        let badge_text = if badge.is_empty() {
            String::new()
        } else {
            format!("{} ", badge)
        };

        // ` <barn> · <name> ` for a barn, ` <name> ` at home. The barn name is
        // the only part that gives: at MIN_CELL_W there is barely room for the
        // chip, the name and the badge, and the badge is the part a WAITING
        // session cannot lose.
        let label = match &cell.origin {
            Origin::Local => format!(" {} ", win.name),
            Origin::Barn(barn) => {
                // Two columns go to the block's corners; the rest is the title.
                // Everything but the barn name is fixed: chip, badge, the two
                // padding spaces and the ` · ` separator.
                let fixed = display_width(&number)
                    + display_width(&badge_text)
                    + display_width(&win.name)
                    + 5;
                let budget = (rect.width as usize).saturating_sub(2).saturating_sub(fixed);
                match truncate_cols(barn, budget) {
                    // Not one column to spare for the barn name. Spend none on
                    // the separator either: a bare ` · ` naming nothing costs
                    // four columns the badge needs, and losing the badge is how
                    // a WAITING session on a barn goes unnoticed. Falling back
                    // to exactly the local title keeps a remote cell from ever
                    // being worse off than the same cell would be at home — the
                    // red border still says where it lives.
                    b if b.is_empty() => format!(" {} ", win.name),
                    b => format!(" {} · {} ", b, win.name),
                }
            }
        };

        let title = Line::from(vec![
            Span::styled(
                number,
                Style::default()
                    .fg(Color::Rgb(26, 26, 26))
                    .bg(color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(label, Style::default().fg(color)),
            Span::styled(
                badge_text,
                Style::default().fg(color).add_modifier(if needs_attention {
                    Modifier::BOLD
                } else {
                    Modifier::DIM
                }),
            ),
        ]);

        // Remote cells are red whatever they are doing; local cells keep the
        // border as their status channel. See [`BARN_RED`]. A dead barn's red is
        // dimmed — the sessions are still its, they are just no longer news.
        let border_color = match &cell.origin {
            Origin::Local => color,
            Origin::Barn(_) if is_stale => STALE_BORDER,
            Origin::Barn(_) => BARN_RED,
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(if needs_attention {
                BorderType::Thick
            } else {
                BorderType::Plain
            })
            .border_style(Style::default().fg(border_color))
            .title(title);

        let text_area = block.inner(rect);
        // Cells paint over the starfield rather than blending with it.
        frame.render_widget(ratatui::widgets::Clear, rect);
        frame.render_widget(block, rect);

        if text_area.width == 0 || text_area.height == 0 {
            return;
        }

        // By pane id, never by slot: the cell list this came from may already be
        // a frame newer than the captures.
        //
        // And per origin, never merged: `%1` exists on every machine, so one map
        // holding both hosts would paint a barn's screen into a local cell.
        // Looked up rather than zipped, too — `tmux::capture_panes` contracts to
        // one entry per pane and a remote frame's map promises nothing of the
        // kind, so a window with no capture renders empty.
        let empty = Vec::new();
        let screen = match &cell.origin {
            Origin::Local => self.local_captures.get(&win.pane_id).unwrap_or(&empty),
            Origin::Barn(barn) => remote
                .get(barn)
                .and_then(|f| f.captures.get(&win.pane_id))
                .unwrap_or(&empty),
        };
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

    fn render_filter(&self, frame: &mut Frame, area: Rect, remote: &HashMap<String, RemoteFrame>) {
        let rows = self.filter_rows(remote);
        let width = 30u16.min(area.width);
        // One line per row, plus the sources separator, the blank and the hint,
        // between the two border lines.
        let height = (rows.len() as u16 + 5).min(area.height);
        let rect = Rect {
            x: area.x + (area.width.saturating_sub(width)) / 2,
            y: area.y + (area.height.saturating_sub(height)) / 2,
            width,
            height,
        };

        let mut lines: Vec<Line> = Vec::new();
        let mut sources_started = false;
        for (i, row) in rows.iter().enumerate() {
            let (label, on) = match row {
                FilterRow::Type(t) => (t.clone(), self.active_types.contains(t)),
                FilterRow::Origin(o) => {
                    if !sources_started {
                        sources_started = true;
                        lines.push(Line::from(Span::styled(
                            "  ── sources ─────────────",
                            Style::default().fg(Color::Rgb(120, 120, 120)),
                        )));
                    }
                    (origin_label(o), !self.hidden_origins.contains(o))
                }
            };
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
                    label,
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
    /// Land on the session under a number key.
    ///
    /// `origin` is not decoration. A local jump is one `select-window`; a
    /// remote one has to select the window **on the barn** before switching
    /// into that barn's local session, and the origin is the only thing that
    /// says which barn. `window_index` is the index on whichever host that is —
    /// a barn's window 4 has nothing to do with ours.
    Jump { origin: Origin, window_index: u32 },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(w: u16, h: u16) -> Rect {
        Rect { x: 0, y: 0, width: w, height: h }
    }

    /// No barns connected — the state the grid is in almost all the time.
    fn no_barns() -> HashMap<String, RemoteFrame> {
        HashMap::new()
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
        match v.handle_input(KeyCode::Char('2'), &windows, &no_barns()) {
            GridAction::Jump { origin, window_index } => {
                assert_eq!(origin, Origin::Local);
                assert_eq!(window_index, 5);
            }
            _ => panic!("expected a jump"),
        }
    }

    #[test]
    fn number_key_past_the_last_cell_does_nothing() {
        let mut v = SessionGridView::new(GridScope::All);
        let windows = vec![win(2, "claude", "P", "local")];
        assert!(matches!(
            v.handle_input(KeyCode::Char('7'), &windows, &no_barns()),
            GridAction::None
        ));
    }

    #[test]
    fn filter_panel_swallows_keys_while_open() {
        let mut v = SessionGridView::new(GridScope::All);
        let windows = vec![win(2, "claude", "P", "local")];

        v.handle_input(KeyCode::Char('f'), &windows, &no_barns());
        assert!(v.filter_open);

        // '1' would jump if the panel were closed.
        assert!(matches!(
            v.handle_input(KeyCode::Char('1'), &windows, &no_barns()),
            GridAction::None
        ));

        v.handle_input(KeyCode::Esc, &windows, &no_barns());
        assert!(!v.filter_open);
    }

    #[test]
    fn space_toggles_a_type_off_and_on_in_the_filter() {
        let mut v = SessionGridView::new(GridScope::All);
        let windows = vec![win(1, "claude", "P", "local")];

        v.handle_input(KeyCode::Char('f'), &windows, &no_barns());
        // Cursor starts on the first row, which is "claude".
        v.handle_input(KeyCode::Char(' '), &windows, &no_barns());
        assert!(!v.active_types.contains(CLAUDE));
        assert_eq!(v.visible(&windows).len(), 0);

        v.handle_input(KeyCode::Char(' '), &windows, &no_barns());
        assert!(v.active_types.contains(CLAUDE));
        assert_eq!(v.visible(&windows).len(), 1);
    }

    #[test]
    fn esc_leaves_the_grid_when_the_filter_is_closed() {
        let mut v = SessionGridView::new(GridScope::All);
        assert!(matches!(v.handle_input(KeyCode::Esc, &[], &no_barns()), GridAction::Back));
        assert!(matches!(
            v.handle_input(KeyCode::Char('v'), &[], &no_barns()),
            GridAction::Back
        ));
    }

    // ---- Origin ordering -------------------------------------------------
    //
    // The derived Ord is the cell ordering for the whole feature, so assert it
    // directly: a later reordering of the enum variants has to fail loudly here
    // rather than silently renumbering everyone's grid.

    #[test]
    fn origin_sorts_local_before_barns_and_barns_alphabetically() {
        assert!(Origin::Local < Origin::Barn("aaa".into()));
        assert!(Origin::Barn("aaa".into()) < Origin::Barn("bbb".into()));
    }

    #[test]
    fn sorting_a_mixed_list_of_origins_puts_local_first_then_barns_by_name() {
        let mut origins = vec![
            Origin::Barn("zulu".into()),
            Origin::Barn("alpha".into()),
            Origin::Local,
            Origin::Barn("mike".into()),
        ];
        origins.sort();
        assert_eq!(
            origins,
            vec![
                Origin::Local,
                Origin::Barn("alpha".into()),
                Origin::Barn("mike".into()),
                Origin::Barn("zulu".into()),
            ]
        );
    }

    #[test]
    fn an_origin_is_a_usable_map_key() {
        // Captures and the hidden-origins filter both key on this.
        let mut seen: HashSet<Origin> = HashSet::new();
        assert!(seen.insert(Origin::Local));
        assert!(!seen.insert(Origin::Local));
        assert!(seen.insert(Origin::Barn("guided".into())));
        assert!(!seen.insert(Origin::Barn("guided".into())));
        assert!(seen.insert(Origin::Barn("smash-mac".into())));
        assert_eq!(seen.len(), 3);
    }

    // ---- Captures follow their pane, not their slot ----------------------
    //
    // These drive the real `render` through a real ratatui buffer, because the
    // bug lives in the lookup `render_cell` performs, not in any in-memory
    // bookkeeping. Seeding goes through `store_captures` — the exact call
    // `tick` makes once tmux has answered — so only the shell-out is stubbed.

    /// Draw the grid into a test buffer and hand back the buffer itself, so a
    /// test can read colours as well as glyphs.
    fn render_buffer(
        v: &SessionGridView,
        windows: &[TmuxWindow],
        remote: &HashMap<String, RemoteFrame>,
        w: u16,
        h: u16,
    ) -> ratatui::buffer::Buffer {
        render_buffer_stale(v, windows, remote, &HashSet::new(), w, h)
    }

    /// The same, with some of the barns' streams dead.
    fn render_buffer_stale(
        v: &SessionGridView,
        windows: &[TmuxWindow],
        remote: &HashMap<String, RemoteFrame>,
        stale: &HashSet<&str>,
        w: u16,
        h: u16,
    ) -> ratatui::buffer::Buffer {
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h))
            .expect("test terminal");
        terminal
            .draw(|f| {
                let area = f.area();
                v.render(f, area, windows, remote, stale);
            })
            .expect("draw");
        terminal.backend().buffer().clone()
    }

    /// Barn names as `render` takes them.
    fn stale_set<'a>(names: &[&'a str]) -> HashSet<&'a str> {
        names.iter().copied().collect()
    }

    fn symbols(buf: &ratatui::buffer::Buffer, w: u16, h: u16) -> Vec<Vec<String>> {
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| {
                        buf.cell((x, y))
                            .map(|c| c.symbol().to_string())
                            .unwrap_or_default()
                    })
                    .collect()
            })
            .collect()
    }

    /// Render the grid into a test buffer and return it as a grid of symbols.
    fn render_grid(
        v: &SessionGridView,
        windows: &[TmuxWindow],
        w: u16,
        h: u16,
    ) -> Vec<Vec<String>> {
        let none = HashMap::new();
        symbols(&render_buffer(v, windows, &none, w, h), w, h)
    }

    /// The same, with barns connected.
    fn render_grid_with(
        v: &SessionGridView,
        windows: &[TmuxWindow],
        remote: &HashMap<String, RemoteFrame>,
        w: u16,
        h: u16,
    ) -> Vec<Vec<String>> {
        symbols(&render_buffer(v, windows, remote, w, h), w, h)
    }

    /// Flatten a column range of the rendered buffer into searchable text.
    fn region(grid: &[Vec<String>], x0: usize, x1: usize) -> String {
        grid.iter()
            .map(|row| row[x0.min(row.len())..x1.min(row.len())].concat())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// A 120x40 terminal tiles two cells side by side; this is the column that
    /// separates them. Derived the same way `render_cells` derives it:
    /// body 120x39 -> 2 cols, pad_x 2, inner width 116, cell_w 58, so the left
    /// cell spans x 2..58 and the right starts at x 60.
    const SPLIT: usize = 59;

    // ---- barns in the grid -----------------------------------------------

    /// The barn's own clock, deliberately years behind this machine's.
    ///
    /// Every remote signal in these tests is timestamped against it, so any
    /// lookup that judged freshness against the *local* clock would discard the
    /// lot and the tests that expect a status would go red.
    const BARN_NOW: u64 = 1_700_000_000;

    /// A window living **on** a barn. Its pane id is the caller's to choose
    /// because the point of several of these is that `%1` exists on every host.
    fn rwin(index: u32, name: &str, pane: &str, ty: &str) -> TmuxWindow {
        TmuxWindow {
            index,
            name: name.to_string(),
            active: false,
            pane_id: pane.to_string(),
            pane_title: String::new(),
            pane_current_command: String::new(),
            window_activity: 0,
            window_type: ty.to_string(),
            project: "P".to_string(),
            barn: "local".to_string(),
        }
    }

    /// One barn's frame. Captures start empty and tests fill in only the panes
    /// they care about — a remote capture map carries no guarantee that every
    /// window is in it.
    fn frame_of(barn: &str, windows: Vec<TmuxWindow>) -> RemoteFrame {
        RemoteFrame {
            barn: barn.to_string(),
            windows,
            captures: HashMap::new(),
            signals: HashMap::new(),
            barn_now: BARN_NOW,
        }
    }

    fn barns(frames: Vec<RemoteFrame>) -> HashMap<String, RemoteFrame> {
        frames.into_iter().map(|f| (f.barn.clone(), f)).collect()
    }

    /// Give a remote pane a screen.
    fn remote_capture(f: &mut RemoteFrame, pane: &str, screen: &str) {
        f.captures.insert(pane.to_string(), vec![screen.to_string()]);
    }

    /// Give a remote pane a signal, `age` seconds old **on the barn's clock**.
    /// Files on the barn are named by sanitized pane id, so that is the key.
    fn remote_signal(f: &mut RemoteFrame, pane: &str, status: SessionStatus, age: u64) {
        f.signals.insert(
            crate::signals::sanitize_pane_id(pane),
            crate::signals::SessionSignal { status, updated: BARN_NOW - age },
        );
    }

    /// The text inside the cell `render_cells` puts in `slot`, for `n` cells on
    /// a `w x h` terminal.
    ///
    /// Mirrors `render_cells`' arithmetic rather than hardcoding a column the
    /// way `SPLIT` does, so these keep pointing at the right cell when the
    /// tiling changes under them.
    fn cell_text(grid: &[Vec<String>], n: usize, slot: usize, w: u16, h: u16) -> String {
        let body = Rect { x: 0, y: 1, width: w, height: h - 1 };
        let (cols, rows) = grid_dims(n, body);
        assert!(slot < cols * rows, "slot {slot} does not fit a {cols}x{rows} grid");
        let pad_x = if body.width > 20 { 2 } else { 0 };
        let pad_y = if body.height > 12 { 1 } else { 0 };
        let inner = Rect {
            x: body.x + pad_x,
            y: body.y + pad_y,
            width: body.width - pad_x * 2,
            height: body.height - pad_y * 2,
        };
        let cw = inner.width / cols as u16;
        let ch = inner.height / rows as u16;
        let x0 = (inner.x + (slot % cols) as u16 * cw) as usize;
        let y0 = (inner.y + (slot / cols) as u16 * ch) as usize;
        let x1 = x0 + cw.saturating_sub(1) as usize;
        let y1 = y0 + ch.saturating_sub(1) as usize;
        grid[y0..y1.min(grid.len())]
            .iter()
            .map(|row| row[x0.min(row.len())..x1.min(row.len())].concat())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Column at which `needle` starts on row `y`, panicking with the row if it
    /// is not there. Columns, not byte offsets — titles carry a `·`.
    fn col_of(buf: &ratatui::buffer::Buffer, y: u16, w: u16, needle: &str) -> u16 {
        let sym = |x: u16| {
            buf.cell((x, y))
                .map(|c| c.symbol().to_string())
                .unwrap_or_default()
        };
        for x in 0..w {
            let tail: String = (x..w).map(sym).collect();
            if tail.starts_with(needle) {
                return x;
            }
        }
        let row: String = (0..w).map(sym).collect();
        panic!("{needle:?} is not on row {y}: {row:?}");
    }

    fn fg_at(buf: &ratatui::buffer::Buffer, x: u16, y: u16) -> Color {
        buf.cell((x, y)).expect("a cell").fg
    }

    fn bg_at(buf: &ratatui::buffer::Buffer, x: u16, y: u16) -> Color {
        buf.cell((x, y)).expect("a cell").bg
    }

    const AMBER: Color = Color::Rgb(224, 168, 40);

    /// Seed captures exactly as `tick` would have, for the windows visible at
    /// the time of that tick, with tmux stubbed out.
    fn seed_captures(v: &mut SessionGridView, at_tick: &[TmuxWindow], screens: &[&str]) {
        let ids: Vec<String> = v.visible(at_tick).iter().map(|w| w.pane_id.clone()).collect();
        assert_eq!(ids.len(), screens.len(), "one screen per visible pane");
        let screens: Vec<Vec<String>> = screens.iter().map(|s| vec![s.to_string()]).collect();
        v.store_captures(ids, screens);
    }

    #[test]
    fn a_filter_change_between_tick_and_render_never_shows_the_wrong_pane() {
        // `handle_input` mutates the filter and returns; the event loop redraws
        // immediately, so `render` computes a new `visible()` against captures
        // taken before the change. Slot-indexed captures misalign here.
        let mut v = SessionGridView::new(GridScope::All);
        let windows = vec![
            win(1, "shell", "P", "local"),
            win(2, "claude", "P", "local"),
            win(3, "claude", "P", "local"),
        ];
        seed_captures(&mut v, &windows, &["SHELL-ONE", "CLAUDE-TWO", "CLAUDE-THREE"]);

        // Press `c`. No tick happens before the next frame.
        v.handle_input(KeyCode::Char('c'), &windows, &no_barns());

        let grid = render_grid(&v, &windows, 120, 40);
        let left = region(&grid, 0, SPLIT);
        let right = region(&grid, SPLIT, 120);
        let all = region(&grid, 0, 120);

        assert!(left.contains("w2"), "left cell should belong to window 2");
        assert!(right.contains("w3"), "right cell should belong to window 3");
        assert!(
            left.contains("CLAUDE-TWO"),
            "window 2's cell must show window 2's screen, got:\n{left}"
        );
        assert!(
            right.contains("CLAUDE-THREE"),
            "window 3's cell must show window 3's screen, got:\n{right}"
        );
        assert!(
            !all.contains("SHELL-ONE"),
            "a filtered-out pane's screen leaked into the grid:\n{all}"
        );
    }

    #[test]
    fn a_pane_with_no_capture_yet_renders_empty_rather_than_borrowing_a_neighbours() {
        // Window 2 was created after the last tick, so it has no capture. It
        // must render blank — and it must not inherit window 3's screen just
        // because it now occupies the slot window 3 held.
        let mut v = SessionGridView::new(GridScope::All);
        let at_tick = vec![win(3, "claude", "P", "local")];
        seed_captures(&mut v, &at_tick, &["THREE-SCREEN"]);

        let now = vec![win(2, "claude", "P", "local"), win(3, "claude", "P", "local")];
        let grid = render_grid(&v, &now, 120, 40);
        let left = region(&grid, 0, SPLIT);
        let right = region(&grid, SPLIT, 120);

        assert!(left.contains("w2"), "left cell should belong to window 2");
        assert!(
            !left.contains("THREE-SCREEN"),
            "the new window borrowed window 3's screen:\n{left}"
        );
        assert!(
            right.contains("THREE-SCREEN"),
            "window 3 lost its own screen to the new window:\n{right}"
        );
    }

    #[test]
    fn a_closed_window_does_not_donate_its_screen_to_the_cell_that_replaces_it() {
        // Same misalignment reached without a keypress: tmux state changed
        // between the tick and the redraw.
        let mut v = SessionGridView::new(GridScope::All);
        let at_tick = vec![
            win(1, "claude", "P", "local"),
            win(2, "claude", "P", "local"),
            win(3, "claude", "P", "local"),
        ];
        seed_captures(&mut v, &at_tick, &["ONE-SCREEN", "TWO-SCREEN", "THREE-SCREEN"]);

        // Window 1 closed. Windows 2 and 3 shift down a slot each.
        let now = vec![win(2, "claude", "P", "local"), win(3, "claude", "P", "local")];
        let grid = render_grid(&v, &now, 120, 40);
        let left = region(&grid, 0, SPLIT);
        let right = region(&grid, SPLIT, 120);
        let all = region(&grid, 0, 120);

        assert!(left.contains("TWO-SCREEN"), "window 2 lost its screen:\n{left}");
        assert!(right.contains("THREE-SCREEN"), "window 3 lost its screen:\n{right}");
        assert!(
            !all.contains("ONE-SCREEN"),
            "the closed window's screen is still on the grid:\n{all}"
        );
    }

    #[test]
    fn a_tick_keys_every_capture_by_its_own_pane_id() {
        let mut v = SessionGridView::new(GridScope::All);
        let windows = vec![
            win(4, "claude", "P", "local"),
            win(9, "claude", "P", "local"),
        ];
        seed_captures(&mut v, &windows, &["FOUR", "NINE"]);

        assert_eq!(v.local_captures.get("%4"), Some(&vec!["FOUR".to_string()]));
        assert_eq!(v.local_captures.get("%9"), Some(&vec!["NINE".to_string()]));
        assert_eq!(v.local_captures.len(), 2);
    }

    // ---- remote cells ----------------------------------------------------
    //
    // Anything these claim about what reaches the screen is asserted off a real
    // `TestBackend` buffer, because the claims *are* about pixels: which cell a
    // screen landed in, what colour a badge came out. Assertions about
    // `cells()` alone would be a weaker witness for all of them.

    #[test]
    fn remote_windows_appear_as_cells_alongside_local_ones() {
        let mut v = SessionGridView::new(GridScope::All);
        let local = vec![win(1, "claude", "P", "local")];
        seed_captures(&mut v, &local, &["LOCAL-SCREEN"]);

        let mut f = frame_of("guided", vec![rwin(4, "api", "%12", "claude")]);
        remote_capture(&mut f, "%12", "REMOTE-SCREEN");
        let remote = barns(vec![f]);

        let cells = v.cells(&local, &remote);
        assert_eq!(cells.len(), 2, "one local and one remote: {cells:?}");
        assert_eq!(cells[0].origin, Origin::Local);
        assert_eq!(cells[1].origin, Origin::Barn("guided".to_string()));
        assert_eq!(cells[1].window.name, "api");

        let grid = render_grid_with(&v, &local, &remote, 120, 40);
        let left = region(&grid, 0, SPLIT);
        let right = region(&grid, SPLIT, 120);
        assert!(left.contains("LOCAL-SCREEN"), "the local cell went missing:\n{left}");
        assert!(
            right.contains("REMOTE-SCREEN"),
            "the barn's session never reached the grid:\n{right}"
        );
        assert!(
            right.contains("guided"),
            "a remote cell must name its barn:\n{right}"
        );
    }

    #[test]
    fn local_cells_always_number_before_remote_ones() {
        // Remote window indices are the barn's own and say nothing about ours:
        // index 1 on a barn must still sort after index 9 here.
        let v = SessionGridView::new(GridScope::All);
        let local = vec![win(9, "claude", "P", "local")];
        let remote = barns(vec![
            frame_of("zulu", vec![rwin(1, "z-api", "%1", "claude")]),
            frame_of("alpha", vec![rwin(1, "a-api", "%1", "claude")]),
        ]);

        let cells = v.cells(&local, &remote);
        let order: Vec<(Origin, &str)> = cells
            .iter()
            .map(|c| (c.origin.clone(), c.window.name.as_str()))
            .collect();
        assert_eq!(
            order,
            vec![
                (Origin::Local, "w9"),
                (Origin::Barn("alpha".to_string()), "a-api"),
                (Origin::Barn("zulu".to_string()), "z-api"),
            ],
            "local first, then barns alphabetically"
        );

        // And on screen: chip 1 belongs to the local window, 3 to zulu.
        let grid = render_grid_with(&v, &local, &remote, 120, 40);
        let first = cell_text(&grid, 3, 0, 120, 40);
        let third = cell_text(&grid, 3, 2, 120, 40);
        assert!(first.contains(" 1  w9"), "cell 1 is not the local window:\n{first}");
        assert!(third.contains(" 3  zulu"), "cell 3 is not zulu:\n{third}");
    }

    #[test]
    fn a_barn_appearing_does_not_renumber_the_local_cells() {
        // Number keys are muscle memory. If connecting a barn slides cell 2 from
        // one project to another, the feature is actively dangerous.
        let mut v = SessionGridView::new(GridScope::All);
        let local = vec![
            win(1, "claude", "P", "local"),
            win(2, "claude", "P", "local"),
            win(3, "claude", "P", "local"),
        ];
        let none = HashMap::new();
        let before: Vec<u32> = v.cells(&local, &none).iter().map(|c| c.window.index).collect();

        // A barn whose name sorts before everything, holding low window indices —
        // every excuse an ordering bug could want to jump the queue.
        let remote = barns(vec![frame_of(
            "aaa",
            vec![rwin(1, "api", "%1", "claude"), rwin(2, "web", "%2", "claude")],
        )]);
        let after = v.cells(&local, &remote);

        assert_eq!(after.len(), 5);
        let after_local: Vec<u32> = after[..3].iter().map(|c| c.window.index).collect();
        assert_eq!(after_local, before, "the barn renumbered the local cells");
        assert!(after[..3].iter().all(|c| c.origin == Origin::Local));

        // The keypress that numbering exists for still lands where it did.
        match v.handle_input(KeyCode::Char('2'), &local, &remote) {
            GridAction::Jump { origin, window_index } => {
                assert_eq!(origin, Origin::Local, "'2' stopped meaning a local window");
                assert_eq!(window_index, 2, "'2' stopped meaning window 2");
            }
            _ => panic!("expected a jump"),
        }
    }

    #[test]
    fn identical_pane_ids_on_two_hosts_do_not_share_a_capture() {
        // `%1` exists on every machine. THE collision test.
        let mut v = SessionGridView::new(GridScope::All);
        let local = vec![win(1, "claude", "P", "local")]; // pane %1
        seed_captures(&mut v, &local, &["LOCAL-ONE"]);

        let mut f = frame_of("guided", vec![rwin(1, "api", "%1", "claude")]); // also %1
        remote_capture(&mut f, "%1", "REMOTE-ONE");
        let remote = barns(vec![f]);

        let grid = render_grid_with(&v, &local, &remote, 120, 40);
        let left = region(&grid, 0, SPLIT);
        let right = region(&grid, SPLIT, 120);

        assert!(left.contains("LOCAL-ONE"), "the local cell lost its screen:\n{left}");
        assert!(
            !left.contains("REMOTE-ONE"),
            "the barn's screen leaked into the local cell:\n{left}"
        );
        assert!(right.contains("REMOTE-ONE"), "the remote cell lost its screen:\n{right}");
        assert!(
            !right.contains("LOCAL-ONE"),
            "the local screen leaked into the barn's cell:\n{right}"
        );
    }

    #[test]
    fn a_remote_signal_is_judged_against_the_barns_clock_not_ours() {
        // BARN_NOW is years behind this machine. Against the local clock every
        // one of these is ancient, so the barn renders statusless with nothing
        // anywhere to explain it.
        let v = SessionGridView::new(GridScope::All);
        let mut f = frame_of(
            "guided",
            vec![rwin(1, "fresh", "%1", "claude"), rwin(2, "old", "%2", "claude")],
        );
        remote_signal(&mut f, "%1", SessionStatus::Waiting, 10);
        remote_signal(&mut f, "%2", SessionStatus::Waiting, 9_999);
        let remote = barns(vec![f]);

        let grid = render_grid_with(&v, &[], &remote, 120, 40);
        let fresh = cell_text(&grid, 2, 0, 120, 40);
        let old = cell_text(&grid, 2, 1, 120, 40);

        assert!(
            fresh.contains("WAITING"),
            "10s old on the barn's clock is fresh however far off ours is:\n{fresh}"
        );
        assert!(
            !old.contains("WAITING"),
            "genuinely old on the barn's own clock must still be dropped:\n{old}"
        );
    }

    #[test]
    fn a_remote_cell_keeps_its_status_colour_in_the_chip_and_badge() {
        // The border gave that job up to mark remoteness, so if status does not
        // survive in the other two channels WAITING is invisible on a barn.
        let v = SessionGridView::new(GridScope::All);
        let mut f = frame_of("guided", vec![rwin(1, "api", "%1", "claude")]);
        remote_signal(&mut f, "%1", SessionStatus::Waiting, 10);
        let remote = barns(vec![f]);

        let buf = render_buffer(&v, &[], &remote, 120, 40);
        // One cell: body 120x39, pad 2/1, so the block's corner is at (2, 2) and
        // its title starts one column in.
        let title_y = 2;
        let chip = col_of(&buf, title_y, 120, " 1 ");
        assert_eq!(
            bg_at(&buf, chip + 1, title_y),
            AMBER,
            "the number chip lost the status colour"
        );
        let badge = col_of(&buf, title_y, 120, "WAITING");
        assert_eq!(
            fg_at(&buf, badge, title_y),
            AMBER,
            "the badge lost the status colour"
        );

        // ...and the border is red regardless, which is what forced the other
        // two to carry it.
        assert_eq!(fg_at(&buf, 2, title_y), BARN_RED, "the corner is not barn red");
        assert_ne!(fg_at(&buf, 2, title_y), AMBER);
    }

    #[test]
    fn a_remote_cell_wears_barn_red_whatever_its_status_while_local_cells_do_not() {
        let v = SessionGridView::new(GridScope::All);

        // No signal at all: still red, because remoteness is not a state.
        let quiet = barns(vec![frame_of("guided", vec![rwin(1, "api", "%1", "claude")])]);
        let buf = render_buffer(&v, &[], &quiet, 120, 40);
        assert_eq!(fg_at(&buf, 2, 2), BARN_RED, "a statusless remote cell is not red");

        // An error on a barn is still red, not the error colour.
        let mut f = frame_of("guided", vec![rwin(1, "api", "%1", "claude")]);
        remote_signal(&mut f, "%1", SessionStatus::Error, 5);
        let buf = render_buffer(&v, &[], &barns(vec![f]), 120, 40);
        assert_eq!(fg_at(&buf, 2, 2), BARN_RED);

        // Local cells never turn red — the border is still their status channel.
        let none = HashMap::new();
        let local = vec![win(1, "claude", "P", "local")];
        let buf = render_buffer(&v, &local, &none, 120, 40);
        assert_ne!(fg_at(&buf, 2, 2), BARN_RED, "barn red leaked onto a local cell");
    }

    #[test]
    fn grid_scope_barn_still_filters_by_the_yeehaw_barn_tag_not_by_origin() {
        // `GridScope::Barn(b)` selects windows whose *work targets* b. A local
        // ssh window into `guided` is Origin::Local with barn == "guided" and
        // belongs; a session living on guided but tagged for somewhere else does
        // not, however much the two words look alike.
        let v = SessionGridView::new(GridScope::Barn("guided".to_string()));
        let local = vec![
            win(1, "ssh", "P", "guided"),
            win(2, "claude", "P", "somewhere-else"),
        ];

        let mut elsewhere = rwin(1, "not-tagged-guided", "%1", "claude");
        elsewhere.barn = "somewhere-else".to_string();
        let mut tagged = rwin(2, "tagged-guided", "%2", "claude");
        tagged.barn = "guided".to_string();
        let remote = barns(vec![frame_of("guided", vec![elsewhere, tagged])]);

        let cells = v.cells(&local, &remote);
        let names: Vec<&str> = cells.iter().map(|c| c.window.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["w1", "tagged-guided"],
            "the scope must read the @yeehaw_barn tag, never the origin"
        );
        assert_eq!(cells[0].origin, Origin::Local, "the local ssh window into guided");
        assert_eq!(cells[1].origin, Origin::Barn("guided".to_string()));
    }

    #[test]
    fn a_remote_window_with_no_capture_renders_empty_rather_than_borrowing_one() {
        // Remote captures arrive in a HashMap with no promise that every window
        // is in it — unlike `tmux::capture_panes`, which contracts to one entry
        // per pane. A missing screen is a blank cell, never a neighbour's.
        let v = SessionGridView::new(GridScope::All);
        let mut f = frame_of(
            "guided",
            vec![rwin(1, "api", "%1", "claude"), rwin(2, "web", "%2", "claude")],
        );
        remote_capture(&mut f, "%1", "API-SCREEN");
        let remote = barns(vec![f]);

        let grid = render_grid_with(&v, &[], &remote, 120, 40);
        let first = cell_text(&grid, 2, 0, 120, 40);
        let second = cell_text(&grid, 2, 1, 120, 40);

        assert!(first.contains("API-SCREEN"), "window 1 lost its screen:\n{first}");
        assert!(second.contains("web"), "cell 2 is not window 2:\n{second}");
        assert!(
            !second.contains("API-SCREEN"),
            "the capture-less window borrowed its neighbour's screen:\n{second}"
        );
    }

    #[test]
    fn a_long_barn_name_is_truncated_so_the_title_still_fits() {
        // 30 columns is a 26-wide cell, barely over MIN_CELL_W. An untruncated
        // barn name pushes the badge straight off the end of the title, which is
        // the one thing a remote cell cannot afford to lose.
        let v = SessionGridView::new(GridScope::All);
        let long = "a-very-long-barn-name-indeed";
        let mut f = frame_of(long, vec![rwin(1, "w1", "%1", "claude")]);
        remote_signal(&mut f, "%1", SessionStatus::Waiting, 10);
        let remote = barns(vec![f]);

        let grid = render_grid_with(&v, &[], &remote, 30, 20);
        let cell = cell_text(&grid, 1, 0, 30, 20);

        assert!(
            cell.contains("WAITING"),
            "the barn name crowded the badge out of the title:\n{cell}"
        );
        assert!(
            !cell.contains(long),
            "the barn name was not truncated to fit:\n{cell}"
        );
        assert!(
            cell.contains("a-"),
            "truncation ate the barn name entirely:\n{cell}"
        );
    }

    #[test]
    fn a_remote_cell_is_never_worse_off_for_title_room_than_the_same_cell_at_home() {
        // Truncating the barn name to nothing still leaves ` · ` naming nothing,
        // and those four columns are exactly what pushes the badge off a narrow
        // cell. 30 columns fits a local `WAITING` title to the column; a remote
        // one has to fit too, or a barn's WAITING is invisible on any terminal
        // this size.
        let v = SessionGridView::new(GridScope::All);
        let mut f = frame_of("a-very-long-barn-name-indeed", vec![rwin(1, "api-server", "%1", "claude")]);
        remote_signal(&mut f, "%1", SessionStatus::Waiting, 10);
        let remote = barns(vec![f]);

        let cell = cell_text(&render_grid_with(&v, &[], &remote, 30, 20), 1, 0, 30, 20);
        assert!(
            cell.contains("api-server"),
            "the window's own name is the one thing the title cannot drop:\n{cell}"
        );
        assert!(
            cell.contains("WAITING"),
            "an unfittable barn name still crowded out the badge:\n{cell}"
        );
    }

    #[test]
    fn the_header_counts_remote_cells_too() {
        let v = SessionGridView::new(GridScope::All);
        let local = vec![win(1, "claude", "P", "local")];
        let remote = barns(vec![frame_of(
            "guided",
            vec![rwin(1, "api", "%1", "claude"), rwin(2, "web", "%2", "claude")],
        )]);
        let grid = render_grid_with(&v, &local, &remote, 120, 40);
        assert!(
            grid[0].concat().contains("3 sessions"),
            "the header ignored the barn: {:?}",
            grid[0].concat()
        );
    }

    // ---- jumping to a remote cell ----------------------------------------
    //
    // RG-6 numbered remote cells; until now `handle_input` resolved the number
    // against `visible()`, which is local only. Every key past the last local
    // cell was a labelled button that did nothing.

    #[test]
    fn a_number_key_on_a_remote_cell_yields_a_barn_scoped_jump() {
        // Cell 2 is the barn's, and the index it carries is the *barn's* window
        // index — 4 here precisely because it is nothing like the slot.
        let mut v = SessionGridView::new(GridScope::All);
        let local = vec![win(1, "claude", "P", "local")];
        let remote = barns(vec![frame_of("guided", vec![rwin(4, "api", "%12", "claude")])]);

        match v.handle_input(KeyCode::Char('2'), &local, &remote) {
            GridAction::Jump { origin, window_index } => {
                assert_eq!(
                    origin,
                    Origin::Barn("guided".to_string()),
                    "the jump lost the barn, so the app cannot tell where to send it"
                );
                assert_eq!(window_index, 4, "the jump carried the slot, not the window");
            }
            _ => panic!("the number under a remote cell did nothing"),
        }
    }

    #[test]
    fn a_number_key_on_a_local_cell_still_yields_a_local_jump() {
        // With a barn connected, and one whose name sorts first at that: the
        // local cells keep their numbers and their origin.
        let mut v = SessionGridView::new(GridScope::All);
        let local = vec![
            win(2, "claude", "P", "local"),
            win(5, "claude", "P", "local"),
        ];
        let remote = barns(vec![frame_of("aaa", vec![rwin(1, "api", "%1", "claude")])]);

        match v.handle_input(KeyCode::Char('2'), &local, &remote) {
            GridAction::Jump { origin, window_index } => {
                assert_eq!(origin, Origin::Local, "a local cell jumped to a barn");
                assert_eq!(window_index, 5);
            }
            _ => panic!("expected a jump"),
        }
    }

    #[test]
    fn the_number_a_cell_wears_is_the_number_that_jumps_to_it() {
        // Numbering lives in `render_cell` (slot + 1) and resolution lives in
        // `handle_input`. Two readings of the same list, and nothing but this
        // ties them together — a key that resolves against a differently
        // ordered list is a grid whose labels lie.
        let mut v = SessionGridView::new(GridScope::All);
        let local = vec![win(3, "claude", "P", "local")];
        let remote = barns(vec![
            frame_of("zulu", vec![rwin(7, "z-api", "%1", "claude")]),
            frame_of("alpha", vec![rwin(8, "a-api", "%1", "claude")]),
        ]);

        let expected: Vec<(Origin, u32)> = v
            .cells(&local, &remote)
            .iter()
            .map(|c| (c.origin.clone(), c.window.index))
            .collect();
        assert_eq!(expected.len(), 3, "control: three cells to number");

        for (slot, want) in expected.iter().enumerate() {
            let key = KeyCode::Char(char::from_digit(slot as u32 + 1, 10).expect("1-9"));
            match v.handle_input(key, &local, &remote) {
                GridAction::Jump { origin, window_index } => assert_eq!(
                    &(origin, window_index),
                    want,
                    "cell {} is drawn as one session and jumps to another",
                    slot + 1
                ),
                _ => panic!("cell {} is numbered but does nothing", slot + 1),
            }
        }
    }

    #[test]
    fn a_number_past_the_last_remote_cell_still_does_nothing() {
        let mut v = SessionGridView::new(GridScope::All);
        let local = vec![win(1, "claude", "P", "local")];
        let remote = barns(vec![frame_of("guided", vec![rwin(4, "api", "%12", "claude")])]);

        assert!(matches!(
            v.handle_input(KeyCode::Char('3'), &local, &remote),
            GridAction::None
        ));
    }

    #[test]
    fn a_hidden_remote_cell_is_not_reachable_by_its_old_number() {
        // The filter decides what is on screen, so it has to decide what the
        // numbers mean too. `c` here leaves one local claude cell and drops the
        // barn's shell — pressing 2 must not still reach the barn.
        let mut v = SessionGridView::new(GridScope::All);
        let local = vec![win(1, "claude", "P", "local")];
        let remote = barns(vec![frame_of("guided", vec![rwin(4, "api", "%12", "shell")])]);

        // Control: before the filter, 2 is the barn's.
        assert!(matches!(
            v.handle_input(KeyCode::Char('2'), &local, &remote),
            GridAction::Jump { origin: Origin::Barn(_), .. }
        ));

        v.handle_input(KeyCode::Char('c'), &local, &remote);
        assert!(
            matches!(v.handle_input(KeyCode::Char('2'), &local, &remote), GridAction::None),
            "a filtered-out barn session was still reachable by number"
        );
    }

    // ---- `l`, and the sources section of the filter -----------------------
    //
    // Everything here that claims a cell is gone claims it off the rendered
    // buffer, for the same reason the remote-cell tests do: "the filter drops
    // it from `cells()`" and "it is not on the screen" are two statements, and
    // only the second is the feature.

    /// The filter panel as text, cut out of the rendered buffer.
    ///
    /// Mirrors `render_filter`'s own geometry rather than hardcoding a rect —
    /// 30 columns, centred, one line per row plus the sources separator, the
    /// blank and the hint line inside the border — and sanity-checks the cut by
    /// insisting the panel's title landed inside it. Reading the whole buffer
    /// instead would be no test at all: barn names are painted on the cells
    /// behind the panel too.
    fn filter_panel(buf: &ratatui::buffer::Buffer, barns: usize, w: u16, h: u16) -> String {
        // types + untagged + local + one per barn.
        let rows = WINDOW_TYPES.len() + 1 + 1 + barns;
        let width = 30u16.min(w);
        let height = (rows as u16 + 5).min(h);
        let x0 = w.saturating_sub(width) / 2;
        let y0 = h.saturating_sub(height) / 2;
        let text = (y0..y0 + height)
            .map(|y| {
                (x0..x0 + width)
                    .map(|x| {
                        buf.cell((x, y))
                            .map(|c| c.symbol().to_string())
                            .unwrap_or_default()
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            text.contains("filter"),
            "the filter panel is not where this helper looked:\n{text}"
        );
        text
    }

    /// Everything painted on the grid, cells and header alike.
    fn screen(
        v: &SessionGridView,
        local: &[TmuxWindow],
        remote: &HashMap<String, RemoteFrame>,
    ) -> String {
        region(&render_grid_with(v, local, remote, 120, 40), 0, 120)
    }

    /// Two barns, each with one claude session carrying a screen nothing else
    /// on the grid could be mistaken for.
    fn two_barns() -> HashMap<String, RemoteFrame> {
        let mut alpha = frame_of("alpha", vec![rwin(1, "a-api", "%1", "claude")]);
        remote_capture(&mut alpha, "%1", "ALPHA-SCREEN");
        let mut zulu = frame_of("zulu", vec![rwin(1, "z-api", "%1", "claude")]);
        remote_capture(&mut zulu, "%1", "ZULU-SCREEN");
        barns(vec![alpha, zulu])
    }

    #[test]
    fn l_hides_every_remote_cell_and_restores_them() {
        let mut v = SessionGridView::new(GridScope::All);
        let local = vec![win(1, "claude", "P", "local")];
        seed_captures(&mut v, &local, &["LOCAL-SCREEN"]);
        let remote = two_barns();

        let before = screen(&v, &local, &remote);
        assert!(
            before.contains("ALPHA-SCREEN") && before.contains("ZULU-SCREEN"),
            "control: both barns start on the grid:\n{before}"
        );

        v.handle_input(KeyCode::Char('l'), &local, &remote);
        let hidden = screen(&v, &local, &remote);
        assert!(
            hidden.contains("LOCAL-SCREEN"),
            "`l` is local-only, not nothing-at-all:\n{hidden}"
        );
        assert!(
            !hidden.contains("ALPHA-SCREEN"),
            "`l` left a barn's session on the grid:\n{hidden}"
        );
        assert!(
            !hidden.contains("ZULU-SCREEN"),
            "`l` hid one barn and not the other:\n{hidden}"
        );

        v.handle_input(KeyCode::Char('l'), &local, &remote);
        let back = screen(&v, &local, &remote);
        assert!(
            back.contains("ALPHA-SCREEN") && back.contains("ZULU-SCREEN"),
            "`l` is a one-way door rather than the toggle `c` is:\n{back}"
        );
        assert!(back.contains("LOCAL-SCREEN"), "the local cell went missing:\n{back}");
    }

    #[test]
    fn l_and_c_compose_rather_than_overriding_each_other() {
        // Local-only plus claude-only is local claude sessions. Either key
        // resetting the other's state — the obvious way to write one of them —
        // leaves a grid the header cannot describe.
        let local = vec![
            win(1, "claude", "P", "local"),
            win(2, "shell", "P", "local"),
        ];
        let mut f = frame_of(
            "alpha",
            vec![
                rwin(1, "a-claude", "%1", "claude"),
                rwin(2, "a-shell", "%2", "shell"),
            ],
        );
        remote_capture(&mut f, "%1", "BARN-CLAUDE");
        remote_capture(&mut f, "%2", "BARN-SHELL");
        let remote = barns(vec![f]);

        // Both orders, because "composes" is a claim about both.
        for keys in [['l', 'c'], ['c', 'l']] {
            let mut v = SessionGridView::new(GridScope::All);
            seed_captures(&mut v, &local, &["LOCAL-CLAUDE", "LOCAL-SHELL"]);
            for k in keys {
                v.handle_input(KeyCode::Char(k), &local, &remote);
            }

            let s = screen(&v, &local, &remote);
            let order = format!("{keys:?}");
            assert!(
                s.contains("LOCAL-CLAUDE"),
                "{order}: the one session that survives both filters is gone:\n{s}"
            );
            assert!(
                !s.contains("LOCAL-SHELL"),
                "{order}: `l` overrode the type filter:\n{s}"
            );
            assert!(
                !s.contains("BARN-CLAUDE"),
                "{order}: `c` overrode the source filter:\n{s}"
            );
            assert!(!s.contains("BARN-SHELL"), "{order}: neither filter applied:\n{s}");
        }
    }

    #[test]
    fn a_newly_connected_barn_defaults_to_visible() {
        // The whole reason the set stores what is *hidden*. An active set would
        // have to learn about each new barn to show it, so the barn you just
        // connected to would arrive invisible with no hint that it had.
        let mut v = SessionGridView::new(GridScope::All);
        let local: Vec<TmuxWindow> = Vec::new();

        let mut alpha = frame_of("alpha", vec![rwin(1, "a-api", "%1", "claude")]);
        remote_capture(&mut alpha, "%1", "ALPHA-SCREEN");
        let only_alpha = barns(vec![alpha]);

        v.handle_input(KeyCode::Char('l'), &local, &only_alpha);
        let s = screen(&v, &local, &only_alpha);
        assert!(!s.contains("ALPHA-SCREEN"), "control: `l` hid the connected barn:\n{s}");

        // zulu connects while the grid is up and local-only is on.
        let both = two_barns();
        let s = screen(&v, &local, &both);
        assert!(
            s.contains("ZULU-SCREEN"),
            "a barn connected after the filter was set arrived invisible:\n{s}"
        );
        assert!(
            !s.contains("ALPHA-SCREEN"),
            "the barn that was hidden came back on its own:\n{s}"
        );
    }

    #[test]
    fn toggling_one_barn_off_leaves_the_others_showing() {
        let mut v = SessionGridView::new(GridScope::All);
        let local = vec![win(1, "claude", "P", "local")];
        seed_captures(&mut v, &local, &["LOCAL-SCREEN"]);
        let remote = two_barns();

        // Rows are the six types, then `local`, then the barns alphabetically,
        // so zulu is the ninth. Which row the cursor actually landed on is
        // asserted off the panel rather than counted on.
        v.handle_input(KeyCode::Char('f'), &local, &remote);
        for _ in 0..8 {
            v.handle_input(KeyCode::Char('j'), &local, &remote);
        }
        let panel = filter_panel(&render_buffer(&v, &local, &remote, 120, 40), 2, 120, 40);
        let on_zulu = panel
            .lines()
            .any(|l| l.contains('❯') && l.contains("zulu"));
        assert!(on_zulu, "the cursor is not on zulu's row:\n{panel}");

        v.handle_input(KeyCode::Char(' '), &local, &remote);
        v.handle_input(KeyCode::Esc, &local, &remote);

        let s = screen(&v, &local, &remote);
        assert!(!s.contains("ZULU-SCREEN"), "space did not hide zulu:\n{s}");
        assert!(
            s.contains("ALPHA-SCREEN"),
            "hiding one barn took the other with it:\n{s}"
        );
        assert!(s.contains("LOCAL-SCREEN"), "hiding a barn hid the local cells:\n{s}");
    }

    #[test]
    fn hiding_local_from_the_panel_leaves_the_barns_showing() {
        // `local` is a source row like any other, and the mirror of the case
        // above: the two sides of the split have to be independently toggleable
        // or the section is decoration.
        let mut v = SessionGridView::new(GridScope::All);
        let local = vec![win(1, "claude", "P", "local")];
        seed_captures(&mut v, &local, &["LOCAL-SCREEN"]);
        let remote = two_barns();

        v.handle_input(KeyCode::Char('f'), &local, &remote);
        for _ in 0..6 {
            v.handle_input(KeyCode::Char('j'), &local, &remote);
        }
        let panel = filter_panel(&render_buffer(&v, &local, &remote, 120, 40), 2, 120, 40);
        assert!(
            panel.lines().any(|l| l.contains('❯') && l.contains("local")),
            "the cursor is not on the local row:\n{panel}"
        );

        v.handle_input(KeyCode::Char(' '), &local, &remote);
        v.handle_input(KeyCode::Esc, &local, &remote);

        let s = screen(&v, &local, &remote);
        assert!(!s.contains("LOCAL-SCREEN"), "the local source could not be hidden:\n{s}");
        assert!(s.contains("ALPHA-SCREEN"), "hiding local hid the barns too:\n{s}");
        assert!(s.contains("ZULU-SCREEN"), "hiding local hid the barns too:\n{s}");
    }

    #[test]
    fn the_filter_panel_is_tall_enough_for_the_sources_section_and_its_hint() {
        // The panel's height was a hardcoded "one line per row, plus 4". The
        // separator makes that one short, and a Paragraph in a Block clips
        // silently: the row count still looks right and the hint at the bottom
        // — the only thing on the panel that says `space` toggles anything —
        // just stops being drawn. Nothing else here would notice, because
        // every other panel assertion is about a row near the top.
        let mut v = SessionGridView::new(GridScope::All);
        let local = vec![win(1, "claude", "P", "local")];
        let remote = two_barns();

        v.handle_input(KeyCode::Char('f'), &local, &remote);
        let panel = filter_panel(&render_buffer(&v, &local, &remote, 120, 40), 2, 120, 40);
        assert!(
            panel.contains("space toggle"),
            "the panel clipped its own hint line:\n{panel}"
        );
        assert!(
            panel.contains("zulu"),
            "the panel clipped the last source row:\n{panel}"
        );
    }

    #[test]
    fn a_barn_switched_off_from_the_panel_keeps_its_row_so_it_can_come_back() {
        // Source rows come from the barns that are *present*, never from the
        // cells that survived the filter. Built from what is on screen, hiding
        // a barn would take its own row away with it and the panel would be a
        // one-way door — nothing left to press to bring the barn back, and no
        // hint that there ever was a barn there.
        let mut v = SessionGridView::new(GridScope::All);
        let local = vec![win(1, "claude", "P", "local")];
        seed_captures(&mut v, &local, &["LOCAL-SCREEN"]);
        let remote = two_barns();

        v.handle_input(KeyCode::Char('f'), &local, &remote);
        for _ in 0..8 {
            v.handle_input(KeyCode::Char('j'), &local, &remote);
        }
        let panel = filter_panel(&render_buffer(&v, &local, &remote, 120, 40), 2, 120, 40);
        assert!(
            panel.lines().any(|l| l.contains('❯') && l.contains("zulu")),
            "control: the cursor is not on zulu's row:\n{panel}"
        );

        v.handle_input(KeyCode::Char(' '), &local, &remote);
        let panel = filter_panel(&render_buffer(&v, &local, &remote, 120, 40), 2, 120, 40);
        let zulu = panel
            .lines()
            .find(|l| l.contains("zulu"))
            .unwrap_or_else(|| panic!("the barn lost its row the moment it was hidden:\n{panel}"))
            .to_string();
        assert!(
            zulu.contains("[ ]"),
            "zulu's row does not show that it is switched off:\n{panel}"
        );

        // And the row still does something: space again brings the barn back.
        v.handle_input(KeyCode::Char(' '), &local, &remote);
        v.handle_input(KeyCode::Esc, &local, &remote);
        let s = screen(&v, &local, &remote);
        assert!(
            s.contains("ZULU-SCREEN"),
            "the barn could not be switched back on from its own row:\n{s}"
        );
    }

    #[test]
    fn l_brings_the_local_cells_back_if_the_panel_had_hidden_them() {
        // "Local only" is a claim about what you can see. With local switched
        // off from the panel, an `l` that touched nothing but the barns leaves
        // an empty grid — under a header describing it as local only.
        let mut v = SessionGridView::new(GridScope::All);
        let local = vec![win(1, "claude", "P", "local")];
        seed_captures(&mut v, &local, &["LOCAL-SCREEN"]);
        let remote = two_barns();

        v.handle_input(KeyCode::Char('f'), &local, &remote);
        for _ in 0..6 {
            v.handle_input(KeyCode::Char('j'), &local, &remote);
        }
        v.handle_input(KeyCode::Char(' '), &local, &remote);
        v.handle_input(KeyCode::Esc, &local, &remote);
        let s = screen(&v, &local, &remote);
        assert!(!s.contains("LOCAL-SCREEN"), "control: the panel did not hide local:\n{s}");

        v.handle_input(KeyCode::Char('l'), &local, &remote);
        let s = screen(&v, &local, &remote);
        assert!(s.contains("LOCAL-SCREEN"), "`l` left the grid with nothing on it:\n{s}");
        assert!(!s.contains("ALPHA-SCREEN"), "`l` did not hide the barns:\n{s}");
    }

    #[test]
    fn the_filter_panel_lists_only_barns_that_are_actually_present() {
        let mut v = SessionGridView::new(GridScope::All);
        let local = vec![win(1, "claude", "P", "local")];
        let remote = two_barns();

        v.handle_input(KeyCode::Char('f'), &local, &remote);
        let panel = filter_panel(&render_buffer(&v, &local, &remote, 120, 40), 2, 120, 40);
        assert!(panel.contains("claude"), "the type rows went missing:\n{panel}");
        assert!(panel.contains("local"), "the panel cannot toggle local:\n{panel}");
        assert!(panel.contains("alpha"), "a connected barn has no row:\n{panel}");
        assert!(panel.contains("zulu"), "a connected barn has no row:\n{panel}");
        assert!(
            panel.contains("sources"),
            "nothing separates the type rows from the source rows:\n{panel}"
        );

        // zulu disconnects. Its row must go with it, or a barn you can no longer
        // see keeps a toggle that does nothing.
        let mut alpha = frame_of("alpha", vec![rwin(1, "a-api", "%1", "claude")]);
        remote_capture(&mut alpha, "%1", "ALPHA-SCREEN");
        let one = barns(vec![alpha]);
        let panel = filter_panel(&render_buffer(&v, &local, &one, 120, 40), 1, 120, 40);
        assert!(panel.contains("alpha"), "the surviving barn lost its row:\n{panel}");
        assert!(
            !panel.contains("zulu"),
            "a disconnected barn is still occupying a row:\n{panel}"
        );
    }

    #[test]
    fn the_filter_panel_still_swallows_number_keys_while_open() {
        // Existing guarantee. The source rows push the panel taller and give the
        // cursor further to travel; none of that may hand a number key back to
        // the grid underneath.
        let mut v = SessionGridView::new(GridScope::All);
        let local = vec![win(1, "claude", "P", "local")];
        let remote = two_barns();

        assert!(
            matches!(
                v.handle_input(KeyCode::Char('2'), &local, &remote),
                GridAction::Jump { .. }
            ),
            "control: 2 reaches a barn's cell with the panel closed"
        );

        v.handle_input(KeyCode::Char('f'), &local, &remote);
        assert!(v.filter_open);
        for key in ['1', '2', '3'] {
            assert!(
                matches!(
                    v.handle_input(KeyCode::Char(key), &local, &remote),
                    GridAction::None
                ),
                "{key} jumped out from under the open filter panel"
            );
        }

        // `l` belongs to the grid, not the panel: toggling the sources out from
        // under a cursor that is sitting on one of them is not a thing to do.
        let before = screen(&v, &local, &remote);
        v.handle_input(KeyCode::Char('l'), &local, &remote);
        assert_eq!(
            screen(&v, &local, &remote),
            before,
            "`l` reached the grid through the open panel"
        );

        v.handle_input(KeyCode::Esc, &local, &remote);
        assert!(!v.filter_open);
    }

    #[test]
    fn a_barn_hidden_by_l_is_not_reachable_by_its_old_number() {
        // The sibling of `a_hidden_remote_cell_is_not_reachable_by_its_old_number`
        // for the source filter. Numbering and key resolution both run through
        // `cells()`, so a hidden cell must be unnumbered *and* unreachable — a
        // filter that only removed the cell from the screen would leave `2`
        // jumping to a barn the user just told it to forget.
        let mut v = SessionGridView::new(GridScope::All);
        let local = vec![win(1, "claude", "P", "local")];
        let remote = barns(vec![frame_of("guided", vec![rwin(4, "api", "%12", "claude")])]);

        assert!(
            matches!(
                v.handle_input(KeyCode::Char('2'), &local, &remote),
                GridAction::Jump { origin: Origin::Barn(_), .. }
            ),
            "control: 2 is the barn's before the filter"
        );

        v.handle_input(KeyCode::Char('l'), &local, &remote);
        assert!(
            matches!(
                v.handle_input(KeyCode::Char('2'), &local, &remote),
                GridAction::None
            ),
            "a barn hidden by `l` was still reachable by number"
        );
        assert!(
            matches!(
                v.handle_input(KeyCode::Char('1'), &local, &remote),
                GridAction::Jump { origin: Origin::Local, window_index: 1 }
            ),
            "hiding the barn renumbered the local cell"
        );
    }

    #[test]
    fn the_header_says_which_sources_it_is_hiding() {
        // A hidden cell leaves the count and nothing else. `c` has said
        // "claude only" since the grid shipped for exactly this reason: a grid
        // showing fewer sessions than exist has to say so.
        let mut v = SessionGridView::new(GridScope::All);
        let local = vec![win(1, "claude", "P", "local")];
        let remote = two_barns();

        let header = |v: &SessionGridView, r: &HashMap<String, RemoteFrame>| {
            render_grid_with(v, &local, r, 120, 40)[0].concat()
        };
        assert!(
            !header(&v, &remote).contains("local only"),
            "the header claims a filter nobody set"
        );

        v.handle_input(KeyCode::Char('l'), &local, &remote);
        assert!(
            header(&v, &remote).contains("local only"),
            "the header does not say the barns are hidden: {:?}",
            header(&v, &remote)
        );

        // One barn back, one still hidden: "local only" would now be a lie.
        v.hidden_origins.remove(&Origin::Barn("alpha".to_string()));
        let h = header(&v, &remote);
        assert!(!h.contains("local only"), "the header still claims local only: {h:?}");
        assert!(h.contains("zulu"), "the header does not name the hidden barn: {h:?}");
    }

    #[test]
    fn l_with_no_barns_connected_changes_nothing() {
        // The state the grid is in almost all the time. `l` there is a no-op,
        // and must not label the header with a filter that is doing nothing.
        let mut v = SessionGridView::new(GridScope::All);
        let local = vec![win(1, "claude", "P", "local")];
        seed_captures(&mut v, &local, &["LOCAL-SCREEN"]);
        let none = HashMap::new();

        let before = screen(&v, &local, &none);
        v.handle_input(KeyCode::Char('l'), &local, &none);
        let after = screen(&v, &local, &none);
        assert_eq!(after, before, "`l` changed a grid with nothing remote on it");
        assert!(after.contains("LOCAL-SCREEN"));
        // Asserted outright rather than left to the equality above, which a
        // header claiming "local only" *before* the keypress as well as after
        // would satisfy — and "local only" is vacuously true of a grid with no
        // barns, so that is exactly the shape the bug takes. It would then be
        // on screen for the state the grid is in almost all the time.
        assert!(
            !after.contains("local only"),
            "the header labels a filter that is filtering nothing:\n{after}"
        );
    }

    #[test]
    fn with_no_barns_connected_the_grid_is_exactly_what_it_was() {
        // The regression guard for everything above: the remote map is empty
        // almost all the time, and that path must not have moved.
        let mut v = SessionGridView::new(GridScope::All);
        let local = vec![win(1, "claude", "P", "local"), win(2, "shell", "P", "local")];
        seed_captures(&mut v, &local, &["ONE", "TWO"]);
        let none = HashMap::new();

        let cells = v.cells(&local, &none);
        assert_eq!(cells.len(), 2);
        assert!(cells.iter().all(|c| c.origin == Origin::Local));

        let grid = render_grid_with(&v, &local, &none, 120, 40);
        assert!(region(&grid, 0, SPLIT).contains("ONE"));
        assert!(region(&grid, SPLIT, 120).contains("TWO"));
    }

    // ---- stale barns ------------------------------------------------------
    //
    // A barn whose stream died keeps every cell it had. Dropping them instead
    // would renumber everything after them under the user's fingers, and the
    // number keys are muscle memory — that is the failure this whole feature
    // exists to avoid.

    /// Two barns, `alpha` with a screen worth keeping and `zulu` alongside it,
    /// so "the stale one kept its place" is a claim about the others too.
    fn alpha_and_zulu() -> HashMap<String, RemoteFrame> {
        let mut a = frame_of("alpha", vec![rwin(1, "api", "%1", "claude")]);
        remote_capture(&mut a, "%1", "LAST-WORDS");
        let mut z = frame_of("zulu", vec![rwin(1, "web", "%1", "claude")]);
        remote_capture(&mut z, "%1", "STILL-LIVE");
        barns(vec![a, z])
    }

    #[test]
    fn a_failed_barn_renders_stale_rather_than_vanishing() {
        let v = SessionGridView::new(GridScope::All);
        let local = vec![win(1, "claude", "P", "local")];
        let remote = alpha_and_zulu();

        let buf = render_buffer_stale(&v, &local, &remote, &stale_set(&["alpha"]), 120, 40);
        let grid = symbols(&buf, 120, 40);

        assert_eq!(v.cells(&local, &remote).len(), 3, "control: three cells on the grid");
        let dead = cell_text(&grid, 3, 1, 120, 40);
        assert!(
            dead.contains("STALE"),
            "a barn whose stream died is not badged:\n{dead}"
        );
        assert!(
            dead.contains("LAST-WORDS"),
            "the last frame went with the stream:\n{dead}"
        );
        assert!(dead.contains("api"), "the cell lost its window:\n{dead}");

        let live = cell_text(&grid, 3, 2, 120, 40);
        assert!(
            !live.contains("STALE"),
            "one barn dying badged the other one too:\n{live}"
        );
        assert!(live.contains("STILL-LIVE"), "the live barn lost its screen:\n{live}");
    }

    #[test]
    fn stale_cells_keep_their_numbers() {
        // The whole reason a dead barn's cells stay. `alpha` sorts before
        // `zulu`, so dropping it would move zulu from 3 to 2 and local stays put
        // either way — the renumbering is invisible unless you look past the
        // first cell.
        let v = SessionGridView::new(GridScope::All);
        let local = vec![win(1, "claude", "P", "local")];
        let remote = alpha_and_zulu();

        let numbered = |stale: &HashSet<&str>| -> Vec<String> {
            let grid = symbols(&render_buffer_stale(&v, &local, &remote, stale, 120, 40), 120, 40);
            (0..3)
                .map(|slot| {
                    let text = cell_text(&grid, 3, slot, 120, 40);
                    let title = text.lines().next().unwrap_or_default().to_string();
                    title
                })
                .collect()
        };

        let healthy = numbered(&HashSet::new());
        let dead = numbered(&stale_set(&["alpha"]));

        for (slot, (before, after)) in healthy.iter().zip(&dead).enumerate() {
            let n = format!(" {} ", slot + 1);
            assert!(
                before.contains(&n) && after.contains(&n),
                "slot {slot} does not wear {n:?}: {before:?} then {after:?}"
            );
        }
        assert!(dead[1].contains("api"), "cell 2 is no longer alpha's: {:?}", dead[1]);
        assert!(dead[2].contains("web"), "zulu was renumbered by alpha dying: {:?}", dead[2]);
    }

    #[test]
    fn a_stale_cell_dims_its_border_rather_than_wearing_live_barn_red() {
        // Barn red is "this session lives on a barn", and it means nothing about
        // whether the barn is answering. A cell drawn from a frame minutes old
        // in exactly the same colours as a live one is a lie the user cannot see
        // through.
        let v = SessionGridView::new(GridScope::All);
        let remote = alpha_and_zulu();

        let live = render_buffer(&v, &[], &remote, 120, 40);
        assert_eq!(fg_at(&live, 2, 2), BARN_RED, "control: a live barn cell is red");

        let dead = render_buffer_stale(&v, &[], &remote, &stale_set(&["alpha"]), 120, 40);
        assert_eq!(
            fg_at(&dead, 2, 2),
            STALE_BORDER,
            "a stale cell kept the live barn border"
        );
        assert_ne!(fg_at(&dead, 2, 2), BARN_RED);
        // The barn beside it is untouched: the split column is where the second
        // cell's border starts.
        assert_eq!(
            fg_at(&dead, SPLIT as u16 + 1, 2),
            BARN_RED,
            "one barn dying dimmed the other one's border"
        );
    }

    #[test]
    fn a_stale_cell_stops_claiming_a_live_status() {
        // A remote status is judged against the *barn's* clock, frozen in the
        // last frame — so a session that was WAITING when the stream died stays
        // WAITING for as long as the grid is open, thick border and all,
        // shouting for attention nobody can give it.
        let v = SessionGridView::new(GridScope::All);
        let mut f = frame_of("alpha", vec![rwin(1, "api", "%1", "claude")]);
        remote_signal(&mut f, "%1", SessionStatus::Waiting, 10);
        let remote = barns(vec![f]);

        let live = render_buffer(&v, &[], &remote, 120, 40);
        let title_y = 2;
        assert_eq!(
            bg_at(&live, col_of(&live, title_y, 120, " 1 ") + 1, title_y),
            AMBER,
            "control: a live WAITING cell wears amber"
        );

        let buf = render_buffer_stale(&v, &[], &remote, &stale_set(&["alpha"]), 120, 40);
        let row: String = (0..120u16)
            .map(|x| {
                buf.cell((x, title_y))
                    .map(|c| c.symbol().to_string())
                    .unwrap_or_default()
            })
            .collect();
        assert!(row.contains("STALE"), "the stale badge is missing: {row:?}");
        assert!(
            !row.contains("WAITING"),
            "a dead barn is still advertising a status it cannot know: {row:?}"
        );

        let chip = col_of(&buf, title_y, 120, " 1 ");
        assert_ne!(
            bg_at(&buf, chip + 1, title_y),
            AMBER,
            "the number chip still carries the frozen status colour"
        );
        // Thick borders are the attention channel. `┏` is the thick corner and
        // `┌` the plain one.
        assert_eq!(
            buf.cell((2, title_y)).expect("a corner").symbol(),
            "┌",
            "a stale cell still draws the attention border"
        );
    }

    #[test]
    fn the_header_names_the_barn_that_went_stale() {
        // The count does not move when a barn dies — its cells are still there —
        // so without a note the only signal is a badge inside a cell that may
        // not have fitted on screen at all.
        let v = SessionGridView::new(GridScope::All);
        let local = vec![win(1, "claude", "P", "local")];
        let remote = alpha_and_zulu();

        let header = |stale: &HashSet<&str>| -> String {
            symbols(&render_buffer_stale(&v, &local, &remote, stale, 120, 40), 120, 40)[0].concat()
        };

        let healthy = header(&HashSet::new());
        assert!(
            !healthy.contains("stale"),
            "the header calls a live grid stale: {healthy:?}"
        );

        let dead = header(&stale_set(&["alpha"]));
        assert!(
            dead.contains("stale: alpha"),
            "the header does not name the dead barn: {dead:?}"
        );
        assert!(
            !dead.contains("zulu"),
            "the header names a barn that is streaming fine: {dead:?}"
        );

        let both = header(&stale_set(&["alpha", "zulu"]));
        assert!(
            both.contains("stale: alpha/zulu"),
            "two dead barns are not both named, in order: {both:?}"
        );
    }

    #[test]
    fn a_barn_that_died_before_its_first_frame_is_still_named_in_the_header() {
        // The `connect`-parked-on-unreachable case: a `yh-barn-*` session exists,
        // so the barn counts as connected and gets a stream, and the stream never
        // produces anything. There are no cells to badge, and the header note is
        // then the only thing on the screen that explains where the sessions the
        // user came looking for went.
        let v = SessionGridView::new(GridScope::All);
        let local = vec![win(1, "claude", "P", "local")];
        let none = HashMap::new();

        let grid = symbols(
            &render_buffer_stale(&v, &local, &none, &stale_set(&["ghost"]), 120, 40),
            120,
            40,
        );
        let header = grid[0].concat();
        assert!(
            header.contains("stale: ghost"),
            "a barn that never streamed went unmentioned: {header:?}"
        );
        assert!(
            header.contains("1 session"),
            "a frameless barn was counted as a cell: {header:?}"
        );
    }

    #[test]
    fn the_stale_badge_survives_the_narrowest_cell_a_long_barn_name_can_crowd() {
        // Same trap `a_long_barn_name_is_truncated_so_the_title_still_fits`
        // caught for WAITING, and the badge is even more load-bearing here: on a
        // stale cell it is the *only* thing in the title saying the screen is a
        // photograph, since the dimmed border is a colour and colours are what a
        // 26-column cell has already spent.
        let v = SessionGridView::new(GridScope::All);
        let long = "a-very-long-barn-name-indeed";
        let mut f = frame_of(long, vec![rwin(1, "w1", "%1", "claude")]);
        remote_signal(&mut f, "%1", SessionStatus::Waiting, 10);
        let remote = barns(vec![f]);

        let grid = symbols(
            &render_buffer_stale(&v, &[], &remote, &stale_set(&[long]), 30, 20),
            30,
            20,
        );
        let cell = cell_text(&grid, 1, 0, 30, 20);
        assert!(
            cell.contains("STALE"),
            "the barn name crowded the stale badge out of the title:\n{cell}"
        );
        assert!(
            !cell.contains("WAITING"),
            "a narrow stale cell is still advertising a frozen status:\n{cell}"
        );
    }

    #[test]
    fn a_local_cell_is_never_stale_however_many_barns_are() {
        // Staleness is a property of the *stream*, and local windows do not
        // arrive over one — `tmux::list_yeehaw_windows` is read directly, every
        // tick, on this machine. A local cell dimmed and badged because some
        // barn stopped answering would be the grid lying about a session the
        // user can see with their own eyes.
        let v = SessionGridView::new(GridScope::All);
        let local = vec![win(1, "claude", "P", "local")];
        let remote = alpha_and_zulu();

        let buf = render_buffer_stale(&v, &local, &remote, &stale_set(&["alpha", "zulu"]), 120, 40);
        let grid = symbols(&buf, 120, 40);
        let home = cell_text(&grid, 3, 0, 120, 40);
        assert!(home.contains("w1"), "control: slot 1 is the local cell:\n{home}");
        assert!(
            !home.contains("STALE"),
            "a barn dying badged a local session:\n{home}"
        );
        assert_ne!(
            fg_at(&buf, 2, 2),
            STALE_BORDER,
            "a barn dying dimmed a local cell's border"
        );
    }

    #[test]
    fn a_number_key_on_a_stale_cell_still_resolves_to_that_cell() {
        // The alternative RG-6 already rejected once: a numbered cell that does
        // nothing. Staleness changes how the jump is *performed* — `app`'s
        // `jump_to_cell` skips the blocking remote select — never whether the
        // key resolves.
        let mut v = SessionGridView::new(GridScope::All);
        let local = vec![win(1, "claude", "P", "local")];
        let remote = alpha_and_zulu();

        assert!(
            matches!(
                v.handle_input(KeyCode::Char('2'), &local, &remote),
                GridAction::Jump { origin: Origin::Barn(ref b), window_index: 1 } if b == "alpha"
            ),
            "the number drawn on a stale cell stopped meaning anything"
        );
    }

    // ---- the header's hint line -------------------------------------------
    //
    // The header has been out of columns since before this feature: RG-7
    // measured 131 columns of content on a 120-column terminal, and RG-9 put
    // the stale note deliberately ahead of the hints, so the hints are what
    // gets cut. Adding `[l] local` and `[?] help` to a line already overflowing
    // does not make them discoverable, it makes `[esc] back` disappear.
    //
    // So the line is fitted to the room actually left. These tests are about
    // that fitting, and every one of them reads the real rendered row rather
    // than the string that was handed to the `Span` — a `Paragraph` clips
    // silently, which is the whole reason this is a problem nobody noticed.

    /// Every hint the header can carry, in the order it draws them, with the
    /// pinned one last.
    ///
    /// Deliberately literals rather than a borrow of the production constants:
    /// a rename that dropped a hint from the line would rename it here too and
    /// the check would go on passing.
    const HINT_TEXTS: &[&str] = &[
        "[1-9] jump",
        "[c] claude",
        "[l] local",
        "[f] filter",
        "[esc] back",
        "[?] help",
    ];

    /// The header row of a rendered grid.
    fn header_row(
        v: &SessionGridView,
        local: &[TmuxWindow],
        remote: &HashMap<String, RemoteFrame>,
        stale: &HashSet<&str>,
        w: u16,
    ) -> String {
        symbols(&render_buffer_stale(v, local, remote, stale, w, 40), w, 40)[0].concat()
    }

    /// Fails if the row ends part-way through a hint.
    ///
    /// The failure mode a width-aware hint line can still have: fitting the
    /// count wrong by a column or two turns `[esc] back` into `[esc] bac`,
    /// which looks like a typo rather than a layout bug and survives review.
    fn no_hint_is_half_drawn(row: &str) {
        let trimmed = row.trim_end();
        for hint in HINT_TEXTS {
            if trimmed.ends_with(hint) {
                continue;
            }
            for cut in 1..hint.chars().count() {
                let partial: String = hint.chars().take(cut).collect();
                assert!(
                    !trimmed.ends_with(&partial),
                    "the header was cut inside {hint:?}: {trimmed:?}"
                );
            }
        }
    }

    /// Which hints made it onto the row, in display order.
    fn hints_on(row: &str) -> Vec<&'static str> {
        HINT_TEXTS.iter().filter(|h| row.contains(**h)).copied().collect()
    }

    #[test]
    fn the_header_advertises_the_source_filter_and_the_help_key() {
        // `l` has been bound since the sources filter shipped and the hint line
        // still read `[1-9] jump [c] claude [f] filter [esc] back`. A key with
        // no name anywhere on the screen is a key nobody has.
        let v = SessionGridView::new(GridScope::All);
        let local = vec![
            win(1, "claude", "P", "local"),
            win(2, "shell", "P", "local"),
            win(3, "shell", "P", "local"),
        ];

        let row = header_row(&v, &local, &no_barns(), &HashSet::new(), 120);
        assert!(row.contains("[l] local"), "the header never mentions `l`: {row:?}");
        assert!(row.contains("[?] help"), "the header never mentions `?`: {row:?}");
        assert_eq!(
            hints_on(&row),
            HINT_TEXTS,
            "a roomy header dropped a hint it had space for: {row:?}"
        );
        no_hint_is_half_drawn(&row);
    }

    #[test]
    fn a_narrow_header_drops_whole_hints_from_the_right_and_keeps_the_help_one() {
        // 80 columns is the terminal the grid has to survive, and the full line
        // is 70 columns on its own — it does not fit beside the title and the
        // count, let alone beside a note.
        //
        // Hints fall off the right as room runs out, except `[?] help`, which
        // is pinned: it is the doorway to every key that just left the screen,
        // so it is the one hint whose absence costs the user something they
        // cannot get back.
        let v = SessionGridView::new(GridScope::All);
        let local = vec![
            win(1, "claude", "P", "local"),
            win(2, "shell", "P", "local"),
            win(3, "shell", "P", "local"),
        ];

        let row = header_row(&v, &local, &no_barns(), &HashSet::new(), 80);
        no_hint_is_half_drawn(&row);
        let shown = hints_on(&row);
        assert!(
            shown.contains(&"[?] help"),
            "a narrow header dropped the only hint that leads to the others: {row:?}"
        );
        assert!(
            shown.contains(&"[l] local"),
            "80 columns has room for `l` and did not show it: {row:?}"
        );
        assert!(
            shown.len() < HINT_TEXTS.len(),
            "control: 80 columns cannot hold the whole line, so something must \
             have been dropped: {row:?}"
        );
    }

    #[test]
    fn the_hints_that_survive_are_always_a_prefix_of_the_line_plus_the_pinned_one() {
        // The invariant behind "drops from the right", asserted across every
        // width the grid can be drawn at rather than at one chosen one. A drop
        // loop that removed the wrong end, or removed a hole out of the middle,
        // would still satisfy any single-width test.
        let v = SessionGridView::new(GridScope::All);
        let local = vec![win(1, "claude", "P", "local")];

        for w in 20..=160u16 {
            let row = header_row(&v, &local, &no_barns(), &HashSet::new(), w);
            no_hint_is_half_drawn(&row);

            let shown = hints_on(&row);
            let (help, rest): (Vec<&str>, Vec<&str>) =
                shown.iter().partition(|h| **h == "[?] help");
            assert_eq!(
                rest,
                HINT_TEXTS[..rest.len()],
                "at {w} columns the header dropped hints out of the middle: {row:?}"
            );
            assert!(
                rest.is_empty() || help.len() == 1,
                "at {w} columns hints were drawn without the help one: {row:?}"
            );
        }
    }

    #[test]
    fn a_header_with_no_room_left_drops_the_hints_rather_than_the_notes() {
        // RG-9's crowded case, and the reason the fix is not "shorten the stale
        // note": the notes describe *this* grid — which sessions are missing and
        // which are a photograph — and are unrecoverable once cut. The hints
        // describe keys that are the same on every grid and are also on the
        // bottom bar, which no note can crowd.
        let mut v = SessionGridView::new(GridScope::Project("desert-silo-web".into()));
        let local = vec![win(1, "claude", "desert-silo-web", "local")];
        let mut alpha = frame_of("alpha", vec![rwin(1, "a-api", "%1", "claude")]);
        alpha.windows[0].project = "desert-silo-web".into();
        let mut zulu = frame_of("zulu", vec![rwin(1, "z-api", "%1", "claude")]);
        zulu.windows[0].project = "desert-silo-web".into();
        let remote = barns(vec![alpha, zulu]);

        // A type list, a hidden barn, and a dead one: every note the header has.
        // `shell` is the type dropped rather than `claude`, so the cells the
        // notes are describing are still on the grid underneath them.
        v.handle_input(KeyCode::Char('f'), &local, &remote);
        v.handle_input(KeyCode::Char('j'), &local, &remote);
        v.handle_input(KeyCode::Char(' '), &local, &remote);
        v.handle_input(KeyCode::Char('f'), &local, &remote);
        v.hidden_origins.insert(Origin::Barn("zulu".into()));

        let row = header_row(&v, &local, &remote, &stale_set(&["alpha"]), 120);
        no_hint_is_half_drawn(&row);
        assert!(
            row.contains("hiding zulu"),
            "the header dropped a note to keep a hint: {row:?}"
        );
        assert!(
            row.contains("stale: alpha"),
            "the header dropped the stale note to keep a hint: {row:?}"
        );
    }
}
