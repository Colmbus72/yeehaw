use crossterm::event::KeyCode;
use ratatui::prelude::*;

use crate::app::WormAction;
use crate::components::list::{self, ListItem, ListState, ItemStatus};
use crate::components::panel::Panel;
use crate::components::worm_header;
use crate::types::*;

pub struct WormDetailView {
    runs_state: ListState,
}

impl WormDetailView {
    pub fn new() -> Self {
        Self {
            runs_state: ListState::new(),
        }
    }

    pub fn handle_input(&mut self, key: KeyCode, _worm: &Worm) -> WormAction {
        match key {
            KeyCode::Char('r') => return WormAction::RunNow,
            KeyCode::Char('t') => return WormAction::Toggle,
            KeyCode::Char('d') => return WormAction::Delete,
            KeyCode::Char('e') => return WormAction::EditCommand,
            KeyCode::Char('j') | KeyCode::Down => self.runs_state.select_next(10), // approximate
            KeyCode::Char('k') | KeyCode::Up => self.runs_state.select_prev(),
            KeyCode::Enter => return WormAction::SelectRun(self.runs_state.selected),
            _ => {}
        }
        WormAction::None
    }

    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        worm: &Worm,
        runs: &[WormRun],
    ) {
        // Layout: Header (ASCII art + metadata) + Runs
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(14), // ASCII header with metadata
                Constraint::Min(1),     // runs
            ])
            .split(area);

        worm_header::render_worm_header(frame, chunks[0], worm);

        // Runs panel
        let runs_panel = Panel {
            title: "Run History",
            focused: true,
            hints: Some("[Enter] view log"),
        };
        let runs_inner = runs_panel.render(frame, chunks[1]);

        let run_items: Vec<ListItem> = runs.iter().map(|r| {
            let status_str = match r.status.as_deref() {
                Some("running") => "running",
                Some("completed") => "completed",
                Some("skipped") => "skipped",
                _ => "unknown",
            };
            let exit_info = r.exit_code.map(|c| format!("exit {}", c)).unwrap_or_default();
            let meta = format!("{} {} {}", r.trigger, status_str, exit_info);

            ListItem {
                id: r.started_at.clone(),
                label: r.started_at.clone(),
                status: Some(match r.exit_code {
                    Some(0) => ItemStatus::Active,
                    Some(_) => ItemStatus::Error,
                    None => ItemStatus::Inactive,
                }),
                meta: Some(meta),
                actions: vec![],
            }
        }).collect();

        list::render_list(
            frame, runs_inner, &run_items,
            &mut self.runs_state,
            true,
            Some(15),
        );
    }
}
