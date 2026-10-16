//! The sync view: every connected barn, and under each, the entities that are
//! not the same here and there. Nothing moves without a keypress.
//!
//! Fetching and transfers run on a background thread (the injected spawner);
//! results come back over a channel that [`SyncView::tick`] drains, so the UI
//! never blocks on ssh.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::components::list::{self, ListItem, ListState, RowAction, RowStyle};
use crate::components::panel::Panel;
use crate::sync::engine::{self, Newer, Remote, Row, Status};
use crate::types::Barn;

const HINTS: &str = "[p] push  [l] pull  [r] refresh  [Esc] back";

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
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> SyncAction {
        let items = self.items();
        let mut state = ListState { selected: self.selected, scroll_offset: 0 };
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return SyncAction::Back,
            KeyCode::Char('j') | KeyCode::Down => state.select_next_selectable(&items),
            KeyCode::Char('k') | KeyCode::Up => state.select_prev_selectable(&items),
            KeyCode::Char('g') => state.select_first_selectable(&items),
            KeyCode::Char('G') => state.select_last_selectable(&items),
            KeyCode::Char('r') => {
                self.status = Some(("refreshing…".into(), false));
                self.refresh_all();
            }
            KeyCode::Char('p') => self.transfer(Direction::Push),
            KeyCode::Char('l') => self.transfer(Direction::Pull),
            _ => {}
        }
        self.selected = state.selected;
        SyncAction::None
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

        let panel = Panel { title: " Sync ", focused: true, hints: Some(HINTS) };
        let inner = panel.render(frame, chunks[0]);
        if self.barns.is_empty() {
            frame.render_widget(
                Paragraph::new("No connected barns. Connect to one with [c] first.")
                    .style(Style::default().fg(Color::DarkGray)),
                inner,
            );
        } else {
            let items = self.items();
            let mut state = ListState { selected: self.selected, scroll_offset: 0 };
            list::render_list(frame, inner, &items, &mut state, true, None);
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

    #[test]
    fn esc_and_q_go_back() {
        let _ranch = branded_ranch();
        let mut view = SyncView::with_deps(vec![], Arc::new(FakeRemote::default()), inline());
        assert!(matches!(view.handle_key(key(KeyCode::Esc)), SyncAction::Back));
        assert!(matches!(view.handle_key(key(KeyCode::Char('q'))), SyncAction::Back));
        assert!(matches!(view.handle_key(key(KeyCode::Char('x'))), SyncAction::None));
    }
}
