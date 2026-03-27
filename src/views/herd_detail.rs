use crossterm::event::KeyCode;
use ratatui::prelude::*;

use crate::components::herd_header;
use crate::components::list::{self, ListItem, ListState, ItemStatus};
use crate::components::panel::Panel;
use crate::types::*;

#[derive(Debug, Clone, Copy, PartialEq)]
enum FocusedPanel {
    Livestock,
    Critters,
}

pub enum HerdAction {
    None,
    SelectLivestock(usize),
    SelectCritter(usize),
}

pub struct HerdDetailView {
    focused_panel: FocusedPanel,
    livestock_state: ListState,
    critters_state: ListState,
}

impl HerdDetailView {
    pub fn new() -> Self {
        Self {
            focused_panel: FocusedPanel::Livestock,
            livestock_state: ListState::new(),
            critters_state: ListState::new(),
        }
    }

    pub fn handle_input(
        &mut self,
        key: KeyCode,
        _project: &Project,
        herd: &Herd,
    ) -> HerdAction {
        if key == KeyCode::Tab {
            self.focused_panel = match self.focused_panel {
                FocusedPanel::Livestock => FocusedPanel::Critters,
                FocusedPanel::Critters => FocusedPanel::Livestock,
            };
            return HerdAction::None;
        }

        match self.focused_panel {
            FocusedPanel::Livestock => {
                let count = herd.livestock.len();
                match key {
                    KeyCode::Char('j') | KeyCode::Down => self.livestock_state.select_next(count),
                    KeyCode::Char('k') | KeyCode::Up => self.livestock_state.select_prev(),
                    KeyCode::Enter => return HerdAction::SelectLivestock(self.livestock_state.selected),
                    _ => {}
                }
            }
            FocusedPanel::Critters => {
                let count = herd.critters.len();
                match key {
                    KeyCode::Char('j') | KeyCode::Down => self.critters_state.select_next(count),
                    KeyCode::Char('k') | KeyCode::Up => self.critters_state.select_prev(),
                    KeyCode::Enter => return HerdAction::SelectCritter(self.critters_state.selected),
                    _ => {}
                }
            }
        }

        HerdAction::None
    }

    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        project: &Project,
        herd: &Herd,
        _barns: &[Barn],
    ) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(10), // ASCII header with metadata
                Constraint::Min(1),     // content
            ])
            .split(area);

        herd_header::render_herd_header(frame, chunks[0], herd, &project.name);

        // Two panels
        let panels = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(50),
                Constraint::Percentage(50),
            ])
            .margin(1)
            .split(chunks[1]);

        // Livestock panel
        let livestock_panel = Panel {
            title: "Livestock",
            focused: self.focused_panel == FocusedPanel::Livestock,
            hints: Some("[n] add  [d] remove"),
        };
        let livestock_inner = livestock_panel.render(frame, panels[0]);

        let livestock_items: Vec<ListItem> = herd.livestock.iter().map(|ls_name| {
            let ls = project.livestock.iter().find(|l| l.name == *ls_name);
            let meta = ls.map(|l| l.barn.as_deref().unwrap_or("local").to_string());
            ListItem {
                id: ls_name.clone(),
                label: ls_name.clone(),
                status: Some(ItemStatus::Active),
                meta,
                actions: vec![],
            }
        }).collect();

        list::render_list(
            frame,
            livestock_inner,
            &livestock_items,
            &mut self.livestock_state,
            self.focused_panel == FocusedPanel::Livestock,
            Some(10),
        );

        // Critters panel
        let critters_panel = Panel {
            title: "Critters",
            focused: self.focused_panel == FocusedPanel::Critters,
            hints: Some("[n] add  [d] remove"),
        };
        let critters_inner = critters_panel.render(frame, panels[1]);

        let critter_items: Vec<ListItem> = herd.critters.iter().map(|cr| {
            ListItem {
                id: format!("{}:{}", cr.barn, cr.critter),
                label: cr.critter.clone(),
                status: Some(ItemStatus::Active),
                meta: Some(cr.barn.clone()),
                actions: vec![],
            }
        }).collect();

        list::render_list(
            frame,
            critters_inner,
            &critter_items,
            &mut self.critters_state,
            self.focused_panel == FocusedPanel::Critters,
            Some(10),
        );
    }
}
