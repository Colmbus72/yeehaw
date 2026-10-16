//! The sync view: every connected barn, and under each, the entities that are
//! not the same here and there. Nothing moves without a keypress.
//!
//! Fetching and transfers run on a background thread (the injected spawner);
//! results come back over a channel that [`SyncView::tick`] drains, so the UI
//! never blocks on ssh.

use std::cell::Cell;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::components::list::{self, ListItem, ListState, RowAction, RowStyle};
use crate::components::panel::Panel;
use crate::config;
use crate::sync::canonical;
use crate::sync::engine::{self, Newer, Remote, Row, Status};
use crate::types::Barn;

/// `p`/`l` are not repeated here: the selected row shows its own `[p] push`
/// `[l] pull`, and at 80 columns the list pane has room for little else.
const LIST_HINTS: &str = "[r] refresh  [Tab] diff";
const DIFF_HINTS: &str = "[j/k] scroll  [PgUp/PgDn] page  [Tab] list";

/// Unchanged lines kept on each side of a change; longer unchanged runs fold
/// into one "… N unchanged" line.
const CONTEXT: usize = 2;
/// A line only this machine has: pushing sends it. Matches the list's `↑ push`.
const HERE_MARK: &str = "↑ ";
/// A line only the barn has: pulling brings it. Matches the list's `↓ pull`.
const THERE_MARK: &str = "↓ ";
const HERE_COLOR: Color = Color::Green;
const THERE_COLOR: Color = Color::Cyan;

pub enum SyncAction {
    None,
    Back,
}

/// A unit of background work.
pub type Job = Box<dyn FnOnce() + Send + 'static>;
/// Runs a [`Job`] somewhere. Production: a fresh thread. Tests: inline, or
/// deferred, so they control when "the network" answers.
pub type Spawn = Box<dyn Fn(Job)>;

enum BarnState {
    Loading,
    Ready(Vec<Row>),
    Failed(String),
}

enum Msg {
    /// A barn's diff, or why there isn't one, tagged with the generation of
    /// the fetch that produced it (see [`SyncView::generations`]).
    Diff(usize, u64, Result<Vec<Row>, String>),
    /// A push or pull finished: what to say about it. Not generation-tagged:
    /// it reports something that happened, never stale, and it starts a fresh
    /// fetch of its barn (a new generation) anyway.
    Done(usize, Result<String, String>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    List,
    Diff,
}

#[derive(Clone, Copy)]
enum Direction {
    Push,
    Pull,
}

pub struct SyncView {
    barns: Vec<Barn>,
    states: Vec<BarnState>,
    /// Per barn, bumped every time a fetch starts. A [`Msg::Diff`] carrying an
    /// older number answers a fetch that has since been superseded (`r` while
    /// one was in flight) and is dropped, so a slow old answer cannot
    /// overwrite a newer one.
    generations: Vec<u64>,
    /// Index into [`Self::items`].
    selected: usize,
    /// One line under the list: (text, is_error).
    status: Option<(String, bool)>,
    /// Which pane `j`/`k`/`g`/`G` move. `p`, `l`, `r`, `J`/`K` and the page
    /// keys work from either.
    focus: Focus,
    /// First line of the diff pane on screen. Back to 0 whenever the selected
    /// row changes.
    diff_scroll: usize,
    /// The diff pane's height at the last render, for paging and for clamping
    /// the scroll to the last screenful. `render` takes `&self`, hence `Cell`.
    diff_height: Cell<usize>,
    /// This machine's barn name, for the diff header.
    here_name: String,
    remote: Arc<dyn Remote>,
    spawn: Spawn,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
}

impl SyncView {
    /// The sync view over `barns` (the connected ones), fetching each over ssh
    /// on its own thread.
    pub fn new(barns: Vec<Barn>) -> Self {
        Self::with_deps(
            barns,
            Arc::new(engine::Ssh),
            Box::new(|job| {
                std::thread::spawn(job);
            }),
        )
    }

    pub fn with_deps(barns: Vec<Barn>, remote: Arc<dyn Remote>, spawn: Spawn) -> Self {
        let (tx, rx) = channel();
        let states = barns.iter().map(|_| BarnState::Loading).collect();
        let generations = vec![0; barns.len()];
        let mut view = SyncView {
            barns,
            states,
            generations,
            selected: 0,
            status: None,
            focus: Focus::List,
            diff_scroll: 0,
            diff_height: Cell::new(0),
            here_name: config::this_barn_name().unwrap_or_else(|| "this machine".into()),
            remote,
            spawn,
            tx,
            rx,
        };
        view.refresh_all();
        view
    }

    fn refresh_all(&mut self) {
        for i in 0..self.barns.len() {
            self.refresh(i);
        }
    }

    /// Re-fetches one barn's diff in the background.
    fn refresh(&mut self, i: usize) {
        self.states[i] = BarnState::Loading;
        self.generations[i] += 1;
        let generation = self.generations[i];
        let remote = Arc::clone(&self.remote);
        let barn = self.barns[i].clone();
        let tx = self.tx.clone();
        let job: Job = Box::new(move || {
            let result = engine::fetch(&*remote, &barn)
                .and_then(|there| engine::diff(&engine::local_snapshot(), &there))
                .map_err(|e| e.to_string());
            let _ = tx.send(Msg::Diff(i, generation, result));
        });
        (self.spawn)(job);
    }

    /// Applies whatever the background has finished. Call every frame.
    pub fn tick(&mut self) {
        let before = self.selected_key();
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Diff(i, generation, _) if generation != self.generations[i] => {}
                Msg::Diff(i, _, Ok(rows)) => self.states[i] = BarnState::Ready(rows),
                Msg::Diff(i, _, Err(e)) => self.states[i] = BarnState::Failed(e),
                Msg::Done(i, result) => {
                    self.status = Some(match result {
                        Ok(text) => (text, false),
                        Err(e) => (e, true),
                    });
                    self.refresh(i);
                }
            }
        }
        // Rows come and go under the cursor; keep it on one, so what `p`/`l`
        // act on is what `render` shows as selected.
        let mut state = ListState { selected: self.selected, scroll_offset: 0 };
        state.settle_on_selectable(&self.items());
        self.selected = state.selected;
        if self.selected_key() != before {
            self.diff_scroll = 0;
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> SyncAction {
        let before = self.selected_key();
        let items = self.items();
        let mut state = ListState { selected: self.selected, scroll_offset: 0 };
        let page = self.diff_height.get().max(1);
        let list = self.focus == Focus::List;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return SyncAction::Back,
            KeyCode::Tab | KeyCode::BackTab => {
                self.focus = if list { Focus::Diff } else { Focus::List };
            }
            KeyCode::Char('j') | KeyCode::Down if list => state.select_next_selectable(&items),
            KeyCode::Char('k') | KeyCode::Up if list => state.select_prev_selectable(&items),
            KeyCode::Char('g') if list => state.select_first_selectable(&items),
            KeyCode::Char('G') if list => state.select_last_selectable(&items),
            KeyCode::Char('j') | KeyCode::Down | KeyCode::Char('J') => self.scroll_diff(1),
            KeyCode::Char('k') | KeyCode::Up | KeyCode::Char('K') => self.scroll_diff(-1),
            KeyCode::Char('g') => self.diff_scroll = 0,
            KeyCode::Char('G') => self.scroll_diff(isize::MAX),
            KeyCode::PageDown => self.scroll_diff(page as isize),
            KeyCode::PageUp => self.scroll_diff(-(page as isize)),
            KeyCode::Char('r') => {
                self.status = Some(("refreshing…".into(), false));
                self.refresh_all();
            }
            KeyCode::Char('p') => self.transfer(Direction::Push),
            KeyCode::Char('l') => self.transfer(Direction::Pull),
            _ => {}
        }
        self.selected = state.selected;
        if self.selected_key() != before {
            self.diff_scroll = 0;
        }
        SyncAction::None
    }

    /// Moves the diff pane by `by` lines, never past its last screenful.
    fn scroll_diff(&mut self, by: isize) {
        let max = self.max_diff_scroll();
        let from = self.diff_scroll.min(max);
        self.diff_scroll = if by < 0 {
            from.saturating_sub(by.unsigned_abs())
        } else {
            from.saturating_add(by as usize).min(max)
        };
    }

    /// The farthest the diff pane can scroll: its last line at the bottom.
    fn max_diff_scroll(&self) -> usize {
        let total = self.selected_row().map_or(0, |(i, row)| self.detail(i, &row).len());
        total.saturating_sub(self.diff_height.get().max(1))
    }

    /// Which row the cursor is on, as identity rather than position: a refresh
    /// can put a different row under the same index.
    fn selected_key(&self) -> Option<(usize, engine::Kind, String)> {
        self.selected_row().map(|(i, row)| (i, row.kind, row.name))
    }

    fn detail(&self, barn: usize, row: &Row) -> Vec<Line<'static>> {
        detail_lines(row, &self.here_name, &self.barns[barn].name)
    }

    fn transfer(&mut self, direction: Direction) {
        let Some((i, row)) = self.selected_row() else { return };
        let barn = self.barns[i].clone();
        let what = format!("{} {}", row.kind.as_str(), row.name);
        match (direction, row.status) {
            (Direction::Push, Status::OnlyThere) => {
                self.status = Some((format!("{} is only on {} — nothing to push", what, barn.name), true));
                return;
            }
            (Direction::Pull, Status::OnlyHere) => {
                self.status = Some((format!("{} isn't on {} — nothing to pull", what, barn.name), true));
                return;
            }
            _ => {}
        }
        let remote = Arc::clone(&self.remote);
        let tx = self.tx.clone();
        self.status = Some((
            match direction {
                Direction::Push => format!("pushing {} to {}…", what, barn.name),
                Direction::Pull => format!("pulling {} from {}…", what, barn.name),
            },
            false,
        ));
        (self.spawn)(Box::new(move || {
            let result = match direction {
                Direction::Push => engine::push(&*remote, &barn, &row)
                    .map(|()| format!("pushed {} to {}", what, barn.name)),
                Direction::Pull => {
                    engine::pull(&row).map(|()| format!("pulled {} from {}", what, barn.name))
                }
            };
            let _ = tx.send(Msg::Done(i, result.map_err(|e| e.to_string())));
        }));
    }

    /// The `(barn index, row)` under the cursor, if the cursor is on a row.
    fn selected_row(&self) -> Option<(usize, Row)> {
        let mut index = 0;
        for (i, state) in self.states.iter().enumerate() {
            index += 1; // heading
            if let BarnState::Ready(rows) = state {
                if self.selected >= index && self.selected < index + rows.len() {
                    return Some((i, rows[self.selected - index].clone()));
                }
                index += rows.len();
            }
        }
        None
    }

    /// The list as drawn: a heading per barn, then its rows.
    fn items(&self) -> Vec<ListItem> {
        let mut items = Vec::new();
        for (barn, state) in self.barns.iter().zip(&self.states) {
            let label = match state {
                BarnState::Loading => format!("{} — fetching…", barn.name),
                BarnState::Failed(e) => format!("{} — {}", barn.name, e),
                BarnState::Ready(rows) if rows.is_empty() => format!("{} — in sync", barn.name),
                BarnState::Ready(_) => barn.name.clone(),
            };
            items.push(ListItem {
                id: barn.name.clone(),
                label,
                style: RowStyle::Heading,
                ..Default::default()
            });
            if let BarnState::Ready(rows) = state {
                items.extend(rows.iter().map(row_item));
            }
        }
        items
    }

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(ratatui::layout::Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(area);

        // List ~40%, diff the rest — but never so narrow the list's rows
        // (`⇅ differs  ranchhand  name`) are cut off at their start.
        let list_width = (chunks[0].width * 2 / 5).max(34.min(chunks[0].width));
        let panes = Layout::default()
            .direction(ratatui::layout::Direction::Horizontal)
            .constraints([Constraint::Length(list_width), Constraint::Min(0)])
            .split(chunks[0]);

        let list_panel = Panel {
            title: " Sync ",
            focused: self.focus == Focus::List,
            hints: Some(LIST_HINTS),
        };
        let inner = list_panel.render(frame, panes[0]);
        if self.barns.is_empty() {
            frame.render_widget(
                Paragraph::new("No connected barns. Connect to one with [c] first.")
                    .wrap(ratatui::widgets::Wrap { trim: true })
                    .style(Style::default().fg(Color::DarkGray)),
                inner,
            );
        } else {
            let items = self.items();
            let mut state = ListState { selected: self.selected, scroll_offset: 0 };
            // Always drawn focused: the cursor says which row the diff is of.
            list::render_list(frame, inner, &items, &mut state, true, None);
        }

        let diff_panel = Panel {
            title: " Diff ",
            focused: self.focus == Focus::Diff,
            hints: Some(DIFF_HINTS),
        };
        let inner = diff_panel.render(frame, panes[1]);
        self.diff_height.set(inner.height as usize);
        match self.selected_row() {
            Some((i, row)) => {
                let lines = self.detail(i, &row);
                let max = lines.len().saturating_sub(inner.height as usize);
                let scroll = self.diff_scroll.min(max);
                frame.render_widget(
                    Paragraph::new(lines.into_iter().skip(scroll).collect::<Vec<_>>()),
                    inner,
                );
            }
            None => frame.render_widget(
                Paragraph::new("Nothing selected").style(Style::default().fg(Color::DarkGray)),
                inner,
            ),
        }

        if let Some((text, is_error)) = &self.status {
            let color = if *is_error { Color::Red } else { Color::DarkGray };
            frame.render_widget(
                Paragraph::new(format!(" {}", text)).style(Style::default().fg(color)),
                chunks[1],
            );
        }
    }
}

/// The right pane for `row`: whose side is whose, when each side last
/// changed, then the entity as the comparison sees it, line by line.
///
/// Lines only this machine has are marked `↑` (a push sends them), lines only
/// the barn has `↓` (a pull brings them), and unchanged lines are dimmed, with
/// long runs of them folded. A row on one side only is all `↑` or all `↓`.
fn detail_lines(row: &Row, here: &str, there: &str) -> Vec<Line<'static>> {
    let dim = Style::default().fg(Color::DarkGray);
    let here_style = Style::default().fg(HERE_COLOR);
    let there_style = Style::default().fg(THERE_COLOR);
    let newer = match row.status {
        Status::Differs(newer) => newer,
        _ => Newer::Unknown,
    };
    let mut out = vec![
        Line::from(vec![
            Span::styled(format!("here ({here})"), here_style.add_modifier(Modifier::BOLD)),
            Span::styled(" → ", dim),
            Span::styled(format!("there ({there})"), there_style.add_modifier(Modifier::BOLD)),
        ]),
        stamp_line(HERE_MARK, "here ", here_style, row.here.as_ref(), newer == Newer::Here, "this machine"),
        stamp_line(THERE_MARK, "there", there_style, row.there.as_ref(), newer == Newer::There, there),
        Line::default(),
    ];

    let (here_lines, there_lines) =
        match (yaml_lines(row, row.here.as_ref()), yaml_lines(row, row.there.as_ref())) {
            (Ok(h), Ok(t)) => (h, t),
            (Err(e), _) | (_, Err(e)) => {
                out.push(Line::styled(format!("can't show this entity: {e}"), Style::default().fg(Color::Red)));
                return out;
            }
        };

    let ops = line_diff(here_lines, there_lines);
    let changed: Vec<bool> = ops.iter().map(|op| !matches!(op, Op::Same(_))).collect();
    let mut i = 0;
    while i < ops.len() {
        match &ops[i] {
            Op::Here(line) => out.push(Line::styled(format!("{HERE_MARK}{line}"), here_style)),
            Op::There(line) => out.push(Line::styled(format!("{THERE_MARK}{line}"), there_style)),
            Op::Same(_) => {
                // The whole unchanged run, folded down to CONTEXT lines on each
                // side that touches a change.
                let end = (i..ops.len()).find(|&j| changed[j]).unwrap_or(ops.len());
                let head = if changed[..i].iter().any(|&c| c) { CONTEXT } else { 0 };
                let tail = if end < ops.len() { CONTEXT } else { 0 };
                let run = end - i;
                let same = |op: &Op| match op {
                    Op::Same(line) => Line::styled(format!("  {line}"), dim),
                    _ => unreachable!("an unchanged run holds only unchanged lines"),
                };
                if run > head + tail + 1 && changed.iter().any(|&c| c) {
                    out.extend(ops[i..i + head].iter().map(same));
                    out.push(Line::styled(
                        format!("  … {} unchanged", run - head - tail),
                        dim.add_modifier(Modifier::ITALIC),
                    ));
                    out.extend(ops[end - tail..end].iter().map(same));
                } else {
                    out.extend(ops[i..end].iter().map(same));
                }
                i = end;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// "↑ here   newer  2026-…", or that the side hasn't got it. "newer" comes
/// before the timestamp so a narrow pane cuts the timestamp, not the verdict.
fn stamp_line(
    mark: &str,
    side: &str,
    style: Style,
    value: Option<&serde_json::Value>,
    is_newer: bool,
    holder: &str,
) -> Line<'static> {
    let dim = Style::default().fg(Color::DarkGray);
    let mut spans = vec![Span::styled(format!("{mark}{side}  "), style)];
    match value {
        None => spans.push(Span::styled(format!("not on {holder}"), dim)),
        Some(value) => {
            let stamp = value.get("updated_at").and_then(|v| v.as_str()).unwrap_or("never stamped");
            if is_newer {
                spans.push(Span::styled("newer  ", style.add_modifier(Modifier::BOLD)));
            } else {
                spans.push(Span::styled("updated ", dim));
            }
            spans.push(Span::styled(stamp.to_string(), dim));
        }
    }
    Line::from(spans)
}

/// One side as YAML lines of exactly what `canonical::hash_entity` compares:
/// no identity, no machine-local fields, no nulls, keys sorted.
fn yaml_lines(row: &Row, value: Option<&serde_json::Value>) -> anyhow::Result<Vec<String>> {
    let Some(value) = value else { return Ok(Vec::new()) };
    let comparable = canonical::comparable_value(row.kind.as_str(), value)?;
    Ok(serde_yaml::to_string(&comparable)?.lines().map(str::to_string).collect())
}

#[derive(Debug, PartialEq)]
enum Op {
    Same(String),
    Here(String),
    There(String),
}

/// A line diff by longest common subsequence. The common head and tail are
/// peeled off first, so an edit to one field of a long entity costs a table the
/// size of the edit, not of the entity.
fn line_diff(here: Vec<String>, there: Vec<String>) -> Vec<Op> {
    let prefix = here.iter().zip(&there).take_while(|(a, b)| a == b).count();
    let suffix = here[prefix..]
        .iter()
        .rev()
        .zip(there[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let (h, t) = (&here[prefix..here.len() - suffix], &there[prefix..there.len() - suffix]);

    // lcs[i][j]: the LCS length of h[i..] and t[j..].
    let mut lcs = vec![vec![0u32; t.len() + 1]; h.len() + 1];
    for i in (0..h.len()).rev() {
        for j in (0..t.len()).rev() {
            lcs[i][j] = if h[i] == t[j] { lcs[i + 1][j + 1] + 1 } else { lcs[i + 1][j].max(lcs[i][j + 1]) };
        }
    }

    let mut ops: Vec<Op> = here[..prefix].iter().cloned().map(Op::Same).collect();
    let (mut i, mut j) = (0, 0);
    while i < h.len() || j < t.len() {
        if i < h.len() && j < t.len() && h[i] == t[j] {
            ops.push(Op::Same(h[i].clone()));
            i += 1;
            j += 1;
        } else if j == t.len() || (i < h.len() && lcs[i + 1][j] >= lcs[i][j + 1]) {
            ops.push(Op::Here(h[i].clone()));
            i += 1;
        } else {
            ops.push(Op::There(t[j].clone()));
            j += 1;
        }
    }
    ops.extend(here[here.len() - suffix..].iter().cloned().map(Op::Same));
    ops
}

fn row_item(row: &Row) -> ListItem {
    let (verb, meta, actions) = match row.status {
        Status::OnlyHere => ("↑ push   ", None, vec![action("p", "push")]),
        Status::OnlyThere => ("↓ pull   ", None, vec![action("l", "pull")]),
        Status::Differs(newer) => (
            "⇅ differs",
            match newer {
                Newer::Here => Some("(here newer)".to_string()),
                Newer::There => Some("(there newer)".to_string()),
                Newer::Unknown => None,
            },
            vec![action("p", "push"), action("l", "pull")],
        ),
    };
    ListItem {
        id: format!("{}:{}", row.kind.as_str(), row.name),
        label: format!("{}  {:<9}  {}", verb, row.kind.as_str(), row.name),
        meta,
        actions,
        ..Default::default()
    }
}

fn action(key: &str, label: &str) -> RowAction {
    RowAction { key: key.into(), label: label.into() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;
    use crate::sync::engine::fake::FakeRemote;
    use crate::sync::engine::Snapshot;
    use crate::testing;
    use crate::types::Project;
    use crossterm::event::KeyModifiers;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn barn(name: &str) -> Barn {
        Barn { name: name.into(), host: Some(format!("{name}.ts.net")), ..Default::default() }
    }

    fn project(name: &str, summary: &str) -> Project {
        serde_yaml::from_str(&format!("name: {name}\npath: /src/{name}\nsummary: {summary}\n")).unwrap()
    }

    /// What barn `name` dumps: branded `name`, holding `projects`.
    fn snapshot(name: &str, projects: Vec<Project>) -> Snapshot {
        Snapshot { this_barn: Some(name.into()), projects, ..Default::default() }
    }

    /// A temp ranch branded `imac`, as `yeehaw brand imac` leaves one.
    fn branded_ranch() -> testing::TempRanch {
        let ranch = testing::temp_ranch();
        config::ensure_config_dirs();
        let mut cfg = config::load_config();
        cfg.this_barn = Some("imac".into());
        config::save_config(&cfg).unwrap();
        ranch
    }

    fn inline() -> Spawn {
        Box::new(|job| job())
    }

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    fn labels(view: &SyncView) -> Vec<String> {
        view.items().into_iter().map(|i| i.label).collect()
    }

    fn put_calls(fake: &FakeRemote) -> Vec<(String, String, Option<String>)> {
        fake.calls().into_iter().filter(|c| c.1.contains("sync put")).collect()
    }

    fn dump_calls(fake: &FakeRemote) -> usize {
        fake.calls().iter().filter(|c| c.1 == engine::DUMP_CMD).count()
    }

    #[test]
    fn shows_a_heading_per_barn_with_the_rows_that_differ() {
        let _ranch = branded_ranch();
        config::save_project(&mut project("a", "x")).unwrap();
        let fake = Arc::new(FakeRemote::with_dump("pi", &snapshot("pi", vec![project("b", "y")])));
        fake.set_dump("mini", Ok(serde_json::to_string(&snapshot("mini", vec![project("a", "x")])).unwrap()));
        let mut view = SyncView::with_deps(vec![barn("pi"), barn("mini")], fake.clone(), inline());
        view.tick();

        assert_eq!(
            labels(&view),
            vec![
                "pi".to_string(),
                "↑ push     project    a".into(),
                "↓ pull     project    b".into(),
                "mini — in sync".into(),
            ]
        );

        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(70, 10)).unwrap();
        terminal.draw(|f| view.render(f, f.area())).unwrap();
        let screen: String = terminal.backend().buffer().content().iter().map(|c| c.symbol()).collect();
        assert!(screen.contains("↓ pull     project    b"), "{screen}");
        assert!(screen.contains("mini — in sync"), "{screen}");
    }

    #[test]
    fn differing_rows_say_which_side_is_newer() {
        let _ranch = branded_ranch();
        let mut mine = project("a", "mine");
        config::save_project(&mut mine).unwrap(); // stamped now
        let mut theirs = project("a", "theirs");
        theirs.updated_at = Some("2020-01-01T00:00:00Z".into());
        let fake = Arc::new(FakeRemote::with_dump("pi", &snapshot("pi", vec![theirs])));
        let mut view = SyncView::with_deps(vec![barn("pi")], fake, inline());
        view.tick();
        let item = view.items().remove(1);
        assert_eq!(item.label, "⇅ differs  project    a");
        assert_eq!(item.meta.as_deref(), Some("(here newer)"));
    }

    #[test]
    fn fetching_runs_on_the_spawner_and_shows_loading_until_tick() {
        let _ranch = branded_ranch();
        let fake = Arc::new(FakeRemote::with_dump("pi", &snapshot("pi", vec![project("b", "y")])));
        let jobs: Rc<RefCell<Vec<Job>>> = Rc::default();
        let queue = Rc::clone(&jobs);
        let mut view = SyncView::with_deps(
            vec![barn("pi")],
            fake.clone(),
            Box::new(move |job| queue.borrow_mut().push(job)),
        );

        assert_eq!(dump_calls(&fake), 0, "fetched on the UI thread");
        assert_eq!(labels(&view), vec!["pi — fetching…".to_string()]);

        let pending: Vec<Job> = jobs.borrow_mut().drain(..).collect();
        for job in pending {
            job();
        }
        view.tick();
        assert_eq!(labels(&view), vec!["pi".to_string(), "↓ pull     project    b".into()]);
    }

    /// Two fetches of one barn in flight (open, then `r`): whichever answers
    /// last must not win if it was started first.
    #[test]
    fn a_stale_fetch_answering_late_does_not_overwrite_a_newer_one() {
        let _ranch = branded_ranch();
        let fake = Arc::new(FakeRemote::default());
        let jobs: Rc<RefCell<Vec<Job>>> = Rc::default();
        let queue = Rc::clone(&jobs);
        let mut view = SyncView::with_deps(
            vec![barn("pi")],
            fake.clone(),
            Box::new(move |job| queue.borrow_mut().push(job)),
        );
        view.handle_key(key(KeyCode::Char('r')));
        let mut pending: Vec<Job> = jobs.borrow_mut().drain(..).collect();
        assert_eq!(pending.len(), 2);
        let first = pending.remove(0);
        let second = pending.remove(0);

        fake.set_dump("pi", Ok(serde_json::to_string(&snapshot("pi", vec![project("new", "y")])).unwrap()));
        second();
        view.tick();
        fake.set_dump("pi", Ok(serde_json::to_string(&snapshot("pi", vec![project("old", "y")])).unwrap()));
        first();
        view.tick();

        assert_eq!(labels(&view), vec!["pi".to_string(), "↓ pull     project    new".into()]);
    }

    #[test]
    fn an_unbranded_barn_or_machine_says_to_brand_it() {
        let _ranch = branded_ranch();
        let fake = Arc::new(FakeRemote::with_dump("pi", &Snapshot::default()));
        let mut view = SyncView::with_deps(vec![barn("pi")], fake, inline());
        view.tick();
        assert_eq!(
            labels(&view),
            vec!["pi — barn pi has no barn name — run `yeehaw brand <name>` on it".to_string()]
        );

        let _ranch = testing::temp_ranch();
        let fake = Arc::new(FakeRemote::with_dump("pi", &snapshot("pi", vec![])));
        let mut view = SyncView::with_deps(vec![barn("pi")], fake, inline());
        view.tick();
        assert_eq!(
            labels(&view),
            vec!["pi — this machine has no barn name — run `yeehaw brand <name>`".to_string()]
        );
    }

    #[test]
    fn a_failed_fetch_shows_on_its_barns_heading() {
        let _ranch = branded_ranch();
        let fake = Arc::new(FakeRemote::default());
        fake.set_dump("pi", Err("barn pi's yeehaw doesn't understand `sync` — update it".into()));
        let mut view = SyncView::with_deps(vec![barn("pi")], fake, inline());
        view.tick();
        assert_eq!(
            labels(&view),
            vec!["pi — barn pi's yeehaw doesn't understand `sync` — update it".to_string()]
        );
    }

    #[test]
    fn p_pushes_the_selected_row_then_refreshes_that_barn() {
        let _ranch = branded_ranch();
        config::save_project(&mut project("a", "x")).unwrap();
        let fake = Arc::new(FakeRemote::with_dump("pi", &snapshot("pi", vec![])));
        let mut view = SyncView::with_deps(vec![barn("pi")], fake.clone(), inline());
        view.tick();
        view.handle_key(key(KeyCode::Char('j')));
        view.handle_key(key(KeyCode::Char('p')));
        view.tick();

        let puts = put_calls(&fake);
        assert_eq!(puts.len(), 1);
        assert_eq!(puts[0].1, "bash -lc 'yeehaw sync put project'");
        assert!(puts[0].2.as_deref().unwrap().contains("\"name\":\"a\""));
        assert_eq!(dump_calls(&fake), 2, "no refresh after the push");
        assert_eq!(view.status, Some(("pushed project a to pi".to_string(), false)));
    }

    #[test]
    fn l_pulls_the_selected_row_here_and_it_drops_out_of_the_diff() {
        let _ranch = branded_ranch();
        let fake = Arc::new(FakeRemote::with_dump("pi", &snapshot("pi", vec![project("b", "y")])));
        let mut view = SyncView::with_deps(vec![barn("pi")], fake.clone(), inline());
        view.tick();
        view.handle_key(key(KeyCode::Down));
        view.handle_key(key(KeyCode::Char('l')));
        view.tick();
        view.tick();

        assert!(put_calls(&fake).is_empty(), "pull went over the wire");
        let pulled = config::load_projects();
        assert_eq!(pulled.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), vec!["b"]);
        assert_eq!(pulled[0].path, "", "another machine's path came along");
        assert_eq!(labels(&view), vec!["pi — in sync".to_string()]);
    }

    #[test]
    fn a_transfer_with_nothing_to_send_says_so_and_sends_nothing() {
        let _ranch = branded_ranch();
        let fake = Arc::new(FakeRemote::with_dump("pi", &snapshot("pi", vec![project("b", "y")])));
        let mut view = SyncView::with_deps(vec![barn("pi")], fake.clone(), inline());
        view.tick();
        view.handle_key(key(KeyCode::Char('j')));
        view.handle_key(key(KeyCode::Char('p')));
        view.tick();
        assert!(put_calls(&fake).is_empty());
        assert_eq!(
            view.status,
            Some(("project b is only on pi — nothing to push".to_string(), true))
        );
    }

    #[test]
    fn a_failed_push_shows_its_error() {
        let _ranch = branded_ranch();
        config::save_project(&mut project("a", "x")).unwrap();
        let fake = Arc::new(FakeRemote::with_dump("pi", &snapshot("pi", vec![])));
        *fake.put_error.lock().unwrap() = Some("barn pi: Error: disk full".into());
        let mut view = SyncView::with_deps(vec![barn("pi")], fake, inline());
        view.tick();
        view.handle_key(key(KeyCode::Char('j')));
        view.handle_key(key(KeyCode::Char('p')));
        view.tick();
        assert_eq!(view.status, Some(("barn pi: Error: disk full".to_string(), true)));
    }

    #[test]
    fn j_and_k_step_over_barn_headings() {
        let _ranch = branded_ranch();
        let fake = Arc::new(FakeRemote::with_dump("pi", &snapshot("pi", vec![project("b", "y")])));
        fake.set_dump("mini", Ok(serde_json::to_string(&snapshot("mini", vec![project("c", "z")])).unwrap()));
        let mut view = SyncView::with_deps(vec![barn("pi"), barn("mini")], fake, inline());
        view.tick();
        // The cursor starts on the first row, not on the heading above it.
        assert_eq!(view.selected_row().map(|(i, r)| (i, r.name)), Some((0, "b".into())));
        view.handle_key(key(KeyCode::Char('j')));
        assert_eq!(view.selected_row().map(|(i, r)| (i, r.name)), Some((1, "c".into())));
        view.handle_key(key(KeyCode::Char('k')));
        assert_eq!(view.selected_row().map(|(i, r)| (i, r.name)), Some((0, "b".into())));
    }

    /// Every screen row as text. Everything drawn here is one cell wide, so a
    /// char index in a row is its x.
    fn draw(view: &SyncView, width: u16, height: u16) -> (Vec<String>, ratatui::buffer::Buffer) {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| view.render(f, f.area())).unwrap();
        let buf = terminal.backend().buffer().clone();
        let rows = (0..height)
            .map(|y| (0..width).map(|x| buf.cell((x, y)).unwrap().symbol()).collect())
            .collect();
        (rows, buf)
    }

    /// The colour of the first cell of `needle` on screen.
    fn colour_of(rows: &[String], buf: &ratatui::buffer::Buffer, needle: &str) -> Color {
        for (y, row) in rows.iter().enumerate() {
            if let Some(byte) = row.find(needle) {
                let x = row[..byte].chars().count();
                return buf.cell((x as u16, y as u16)).unwrap().fg;
            }
        }
        panic!("{needle:?} not on screen:\n{}", rows.join("\n"));
    }

    fn on_screen(rows: &[String], needle: &str) -> bool {
        rows.iter().any(|r| r.contains(needle))
    }

    fn with_wiki(mut p: Project, sections: usize) -> Project {
        p.wiki = (0..sections)
            .map(|i| crate::types::WikiSection { title: format!("t{i}"), content: format!("c{i}") })
            .collect();
        p
    }

    #[test]
    fn a_differing_row_shows_what_differs_and_not_what_the_comparison_ignores() {
        let _ranch = branded_ranch();
        let mut mine = project("a", "mine");
        config::save_project(&mut mine).unwrap(); // stamped: id, created_at, updated_at
        let mut theirs = project("a", "theirs");
        theirs.path = "/home/pi/elsewhere".into();
        theirs.id = Some("uuid-over-there".into());
        theirs.created_at = Some("2019-05-05T00:00:00Z".into());
        theirs.updated_at = Some("2020-01-01T00:00:00Z".into());
        let fake = Arc::new(FakeRemote::with_dump("pi", &snapshot("pi", vec![theirs])));
        let mut view = SyncView::with_deps(vec![barn("pi")], fake, inline());
        view.tick();

        let (rows, buf) = draw(&view, 110, 24);
        let screen = rows.join("\n");
        assert!(on_screen(&rows, "here (imac)"), "{screen}");
        assert!(on_screen(&rows, "there (pi)"), "{screen}");
        // When each side last changed — what "here newer" is judged on.
        assert!(on_screen(&rows, mine.updated_at.as_deref().unwrap()), "{screen}");
        assert!(on_screen(&rows, "2020-01-01T00:00:00Z"), "{screen}");
        // The verdict leads, so an 80-column pane cuts the timestamp, not it.
        let (narrow, _) = draw(&view, 80, 24);
        assert!(on_screen(&narrow, "↑ here   newer"), "{}", narrow.join("\n"));
        assert!(on_screen(&narrow, "↓ there  updated 2020-01-01"), "{}", narrow.join("\n"));

        assert_eq!(colour_of(&rows, &buf, "↑ summary: mine"), Color::Green, "{screen}");
        assert_eq!(colour_of(&rows, &buf, "↓ summary: theirs"), Color::Cyan, "{screen}");
        assert_eq!(colour_of(&rows, &buf, "name: a"), Color::DarkGray, "{screen}");
        assert!(!on_screen(&rows, "↑ name: a") && !on_screen(&rows, "↓ name: a"), "{screen}");

        // Identity and the machine-local path are not content: not in the diff.
        for ignored in ["path:", "/src/a", "/home/pi/elsewhere", "uuid-over-there", "2019-05-05", "id:", "created_at"] {
            assert!(!on_screen(&rows, ignored), "{ignored:?} shown:\n{screen}");
        }
        assert!(!on_screen(&rows, mine.id.as_deref().unwrap()), "{screen}");
    }

    #[test]
    fn a_row_only_here_shows_the_whole_entity_as_new_on_the_barn() {
        let _ranch = branded_ranch();
        config::save_project(&mut with_wiki(project("a", "x"), 1)).unwrap();
        let fake = Arc::new(FakeRemote::with_dump("pi", &snapshot("pi", vec![])));
        let mut view = SyncView::with_deps(vec![barn("pi")], fake, inline());
        view.tick();

        let (rows, buf) = draw(&view, 110, 24);
        let screen = rows.join("\n");
        for line in ["↑ name: a", "↑ summary: x", "↑ wiki:", "↑ - content: c0", "↑   title: t0"] {
            assert_eq!(colour_of(&rows, &buf, line), Color::Green, "{line:?}\n{screen}");
        }
        // Nothing marked as the barn's but the barn's own stamp line.
        assert!(on_screen(&rows, "↓ there  not on pi"), "{screen}");
        assert!(!rows.iter().any(|r| r.contains("↓ ") && !r.contains("↓ there")), "{screen}");
        assert!(!on_screen(&rows, "unchanged"), "{screen}");
        assert!(!on_screen(&rows, "path:"), "{screen}");
    }

    #[test]
    fn moving_the_cursor_shows_the_newly_selected_rows_diff() {
        let _ranch = branded_ranch();
        let fake = Arc::new(FakeRemote::with_dump(
            "pi",
            &snapshot("pi", vec![project("b", "bee"), project("c", "sea")]),
        ));
        let mut view = SyncView::with_deps(vec![barn("pi")], fake, inline());
        view.tick();

        let (rows, _) = draw(&view, 110, 24);
        assert!(on_screen(&rows, "↓ summary: bee"), "{}", rows.join("\n"));
        assert!(!on_screen(&rows, "summary: sea"), "{}", rows.join("\n"));

        view.handle_key(key(KeyCode::Char('j')));
        let (rows, _) = draw(&view, 110, 24);
        assert!(on_screen(&rows, "↓ summary: sea"), "{}", rows.join("\n"));
        assert!(!on_screen(&rows, "summary: bee"), "{}", rows.join("\n"));
    }

    #[test]
    fn long_unchanged_runs_fold_to_a_little_context() {
        let _ranch = branded_ranch();
        config::save_project(&mut with_wiki(project("a", "mine"), 20)).unwrap();
        let fake = Arc::new(FakeRemote::with_dump("pi", &snapshot("pi", vec![with_wiki(project("a", "theirs"), 20)])));
        let mut view = SyncView::with_deps(vec![barn("pi")], fake, inline());
        view.tick();

        let (rows, _) = draw(&view, 110, 60);
        let screen = rows.join("\n");
        assert!(on_screen(&rows, "↑ summary: mine"), "{screen}");
        assert!(on_screen(&rows, "↓ summary: theirs"), "{screen}");
        // Two lines of context after the change, then the fold.
        assert!(on_screen(&rows, "  wiki:"), "{screen}");
        assert!(on_screen(&rows, "  - content: c0"), "{screen}");
        assert!(!on_screen(&rows, "title: t0"), "{screen}");
        assert!(!on_screen(&rows, "content: c10"), "{screen}");
        assert!(on_screen(&rows, "… 39 unchanged"), "{screen}");
    }

    #[test]
    fn the_diff_pane_scrolls_without_moving_the_cursor() {
        let _ranch = branded_ranch();
        config::save_project(&mut with_wiki(project("a", "x"), 30)).unwrap();
        let fake = Arc::new(FakeRemote::with_dump("pi", &snapshot("pi", vec![project("b", "y")])));
        let mut view = SyncView::with_deps(vec![barn("pi")], fake, inline());
        view.tick();
        let first = |view: &SyncView| -> String {
            let (rows, _) = draw(view, 110, 20);
            rows.join("\n")
        };

        let top = first(&view);
        assert!(top.contains("↑ herds: []"), "{top}");
        assert!(!top.contains("title: t29"), "{top}");

        // J scrolls by a line: the top of the pane changes, the cursor stays.
        view.handle_key(key(KeyCode::Char('J')));
        assert_ne!(first(&view), top);
        assert_eq!(view.selected_row().map(|(_, r)| r.name), Some("a".into()));

        // Paging reaches the end, and no further than the last screenful.
        for _ in 0..20 {
            view.handle_key(key(KeyCode::PageDown));
        }
        let bottom = first(&view);
        assert!(bottom.contains("↑   title: t29"), "{bottom}");
        assert!(!bottom.contains("↑ herds: []"), "{bottom}");
        view.handle_key(key(KeyCode::Char('K')));
        assert_ne!(first(&view), bottom, "K at the end did nothing: scrolled past the last screenful");

        for _ in 0..20 {
            view.handle_key(key(KeyCode::PageUp));
        }
        assert_eq!(first(&view), top);

        // Tab hands j/k to the diff pane; p and l still act on the row.
        view.handle_key(key(KeyCode::Tab));
        view.handle_key(key(KeyCode::Char('j')));
        assert_ne!(first(&view), top);
        assert_eq!(view.selected_row().map(|(_, r)| r.name), Some("a".into()));

        // A different row starts at its top.
        view.handle_key(key(KeyCode::Tab));
        view.handle_key(key(KeyCode::Char('j')));
        assert_eq!(view.selected_row().map(|(_, r)| r.name), Some("b".into()));
        assert!(first(&view).contains("↓ herds: []"));
    }

    #[test]
    fn esc_and_q_go_back() {
        let _ranch = branded_ranch();
        let mut view = SyncView::with_deps(vec![], Arc::new(FakeRemote::default()), inline());
        assert!(matches!(view.handle_key(key(KeyCode::Esc)), SyncAction::Back));
        assert!(matches!(view.handle_key(key(KeyCode::Char('q'))), SyncAction::Back));
        assert!(matches!(view.handle_key(key(KeyCode::Char('x'))), SyncAction::None));
    }
}
