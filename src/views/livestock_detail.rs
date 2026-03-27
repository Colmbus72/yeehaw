use crossterm::event::KeyCode;
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::components::header;
use crate::components::list::{self, ListItem, ListState, ItemStatus};
use crate::components::livestock_header;
use crate::components::panel::Panel;
use crate::components::text_input::TextInput;
use crate::tmux::{self, TmuxWindow};
use crate::types::*;

const BRAND_COLOR: Color = Color::Rgb(212, 160, 32);

#[derive(Debug, Clone, Copy, PartialEq)]
enum FocusedPanel {
    Sessions,
    Trails,
}

pub enum LivestockAction {
    None,
    OpenLogs,
    OpenClaude,
    OpenShell,
    OpenTrail(usize),
    SelectWindow(usize),
    UpdateLivestock(Livestock),
    UnlinkTrail(usize),
    SaveNewTrail(crate::trails::Trail),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum EditMode {
    Normal,
    EditName,
    EditPath,
    EditRepo,
    EditBranch,
    EditLogPath,
    EditEnvPath,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum WizardMode {
    Inactive,
    TrailName,
    TrailDescription,
    TriggerOnPush,
    TriggerBranches,
    StepName,
    StepCommand,
    StepTimeout,
}

pub struct LivestockDetailView {
    focused_panel: FocusedPanel,
    sessions_state: ListState,
    trails_state: ListState,
    edit_mode: EditMode,
    text_input: TextInput,
    // Accumulated edit state
    edit_name: String,
    edit_path: String,
    edit_repo: String,
    edit_branch: String,
    edit_log_path: String,
    // Trail creation wizard
    wizard_mode: WizardMode,
    new_trail_name: String,
    new_trail_description: String,
    new_trail_trigger_push: bool,
    new_trail_branches: String,
    new_trail_steps: Vec<crate::trails::TrailStep>,
    new_step_name: String,
    new_step_command: String,
}

impl LivestockDetailView {
    pub fn new() -> Self {
        Self {
            focused_panel: FocusedPanel::Sessions,
            sessions_state: ListState::new(),
            trails_state: ListState::new(),
            edit_mode: EditMode::Normal,
            text_input: TextInput::new(""),
            edit_name: String::new(),
            edit_path: String::new(),
            edit_repo: String::new(),
            edit_branch: String::new(),
            edit_log_path: String::new(),
            wizard_mode: WizardMode::Inactive,
            new_trail_name: String::new(),
            new_trail_description: String::new(),
            new_trail_trigger_push: false,
            new_trail_branches: String::new(),
            new_trail_steps: Vec::new(),
            new_step_name: String::new(),
            new_step_command: String::new(),
        }
    }

    pub fn is_editing(&self) -> bool {
        self.edit_mode != EditMode::Normal
    }

    pub fn is_in_wizard(&self) -> bool {
        self.wizard_mode != WizardMode::Inactive
    }

    fn start_edit(&mut self, livestock: &Livestock) {
        self.edit_name = livestock.name.clone();
        self.edit_path = livestock.path.clone();
        self.edit_repo = livestock.repo.clone().unwrap_or_default();
        self.edit_branch = livestock.branch.clone().unwrap_or_default();
        self.edit_log_path = livestock.log_path.clone().unwrap_or_default();
        self.edit_mode = EditMode::EditName;
        self.text_input = TextInput::new(&self.edit_name);
    }

    fn cancel_edit(&mut self) {
        self.edit_mode = EditMode::Normal;
    }

    fn advance_field(&mut self) -> Option<()> {
        let value = self.text_input.value.trim().to_string();

        match self.edit_mode {
            EditMode::EditName => {
                if !value.is_empty() { self.edit_name = value; }
                self.edit_mode = EditMode::EditPath;
                self.text_input = TextInput::new(&self.edit_path);
            }
            EditMode::EditPath => {
                if !value.is_empty() { self.edit_path = value; }
                self.edit_mode = EditMode::EditRepo;
                self.text_input = TextInput::new(&self.edit_repo);
            }
            EditMode::EditRepo => {
                self.edit_repo = value;
                self.edit_mode = EditMode::EditBranch;
                self.text_input = TextInput::new(&self.edit_branch);
            }
            EditMode::EditBranch => {
                self.edit_branch = value;
                self.edit_mode = EditMode::EditLogPath;
                self.text_input = TextInput::new(&self.edit_log_path);
            }
            EditMode::EditLogPath => {
                self.edit_log_path = value;
                self.edit_mode = EditMode::EditEnvPath;
                self.text_input = TextInput::new("");
            }
            EditMode::EditEnvPath => {
                // Last field — signal save
                return None;
            }
            EditMode::Normal => {}
        }
        Some(())
    }

    fn build_updated(&self, original: &Livestock) -> Livestock {
        let env_path_val = self.text_input.value.trim().to_string();
        Livestock {
            name: self.edit_name.clone(),
            path: self.edit_path.clone(),
            barn: original.barn.clone(),
            repo: if self.edit_repo.is_empty() { None } else { Some(self.edit_repo.clone()) },
            branch: if self.edit_branch.is_empty() { None } else { Some(self.edit_branch.clone()) },
            log_path: if self.edit_log_path.is_empty() { None } else { Some(self.edit_log_path.clone()) },
            env_path: if env_path_val.is_empty() { None } else { Some(env_path_val) },
            source: original.source.clone(),
            k8s_metadata: original.k8s_metadata.clone(),
            trails: original.trails.clone(),
        }
    }

    pub fn handle_input(
        &mut self,
        key: KeyCode,
        _project: &Project,
        livestock: &Livestock,
        session_count: usize,
        trails_count: usize,
    ) -> LivestockAction {
        if self.edit_mode != EditMode::Normal {
            if key == KeyCode::Esc {
                self.cancel_edit();
                return LivestockAction::None;
            }
            let submitted = self.text_input.handle_input(key);
            if submitted {
                if self.advance_field().is_none() {
                    // Last field submitted — save
                    let updated = self.build_updated(livestock);
                    self.edit_mode = EditMode::Normal;
                    return LivestockAction::UpdateLivestock(updated);
                }
            }
            return LivestockAction::None;
        }

        if self.wizard_mode != WizardMode::Inactive {
            return self.handle_wizard_input(key);
        }

        // Tab switching between panels
        if key == KeyCode::Tab {
            self.focused_panel = match self.focused_panel {
                FocusedPanel::Sessions => FocusedPanel::Trails,
                FocusedPanel::Trails => FocusedPanel::Sessions,
            };
            return LivestockAction::None;
        }

        // Page-level keys (work regardless of panel)
        match key {
            KeyCode::Char('e') => {
                self.start_edit(livestock);
                return LivestockAction::None;
            }
            KeyCode::Char('l') => return LivestockAction::OpenLogs,
            _ => {}
        }

        // Panel-specific keys
        match self.focused_panel {
            FocusedPanel::Sessions => {
                match key {
                    KeyCode::Char('c') => LivestockAction::OpenClaude,
                    KeyCode::Char('s') => LivestockAction::OpenShell,
                    KeyCode::Char('j') | KeyCode::Down => {
                        self.sessions_state.select_next(session_count);
                        LivestockAction::None
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        self.sessions_state.select_prev();
                        LivestockAction::None
                    }
                    KeyCode::Enter => LivestockAction::SelectWindow(self.sessions_state.selected),
                    _ => LivestockAction::None,
                }
            }
            FocusedPanel::Trails => {
                match key {
                    KeyCode::Char('n') => {
                        self.start_trail_wizard();
                        LivestockAction::None
                    }
                    KeyCode::Char('d') => {
                        if trails_count > 0 {
                            LivestockAction::UnlinkTrail(self.trails_state.selected)
                        } else {
                            LivestockAction::None
                        }
                    }
                    KeyCode::Char('j') | KeyCode::Down => {
                        self.trails_state.select_next(trails_count);
                        LivestockAction::None
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        self.trails_state.select_prev();
                        LivestockAction::None
                    }
                    KeyCode::Enter => {
                        if trails_count > 0 {
                            LivestockAction::OpenTrail(self.trails_state.selected)
                        } else {
                            LivestockAction::None
                        }
                    }
                    _ => LivestockAction::None,
                }
            }
        }
    }

    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        project: &Project,
        livestock: &Livestock,
        windows: &[TmuxWindow],
        trails: &[crate::trails::Trail],
        trail_runs: &[(String, Option<crate::trails::TrailRun>)],
    ) {
        if self.edit_mode != EditMode::Normal {
            self.render_edit_form(frame, area, project, livestock);
            return;
        }

        if self.wizard_mode != WizardMode::Inactive {
            self.render_trail_wizard(frame, area, project, livestock);
            return;
        }

        // Filter windows for this livestock
        let pattern = format!("{}-{}", project.name, livestock.name);
        let session_windows: Vec<_> = windows
            .iter()
            .filter(|w| w.index > 0 && w.name.contains(&pattern))
            .collect();

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(14), // ASCII header with metadata
                Constraint::Min(1),     // content
            ])
            .split(area);

        livestock_header::render_livestock_header(frame, chunks[0], livestock, &project.name);

        // Two panels side by side
        let panels = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(50),
                Constraint::Percentage(50),
            ])
            .margin(1)
            .split(chunks[1]);

        // Sessions panel (left)
        let sessions_panel = Panel {
            title: "Sessions",
            focused: self.focused_panel == FocusedPanel::Sessions,
            hints: Some("[c] claude  [s] shell"),
        };
        let sessions_inner = sessions_panel.render(frame, panels[0]);

        let session_items: Vec<ListItem> = session_windows
            .iter()
            .enumerate()
            .map(|(i, w)| {
                let status_info = tmux::get_window_status(w);
                ListItem {
                    id: w.index.to_string(),
                    label: format!("[{}] {}", i + 1, w.name),
                    status: Some(if w.active {
                        ItemStatus::Active
                    } else {
                        ItemStatus::Inactive
                    }),
                    meta: Some(status_info.text),
                    actions: vec![],
                }
            })
            .collect();

        list::render_list(
            frame,
            sessions_inner,
            &session_items,
            &mut self.sessions_state,
            self.focused_panel == FocusedPanel::Sessions,
            Some(10),
        );

        // Trails panel (right)
        let trails_panel = Panel {
            title: "Trails",
            focused: self.focused_panel == FocusedPanel::Trails,
            hints: Some("[n] add  [d] unlink"),
        };
        let trails_inner = trails_panel.render(frame, panels[1]);

        let trail_items: Vec<ListItem> = trails
            .iter()
            .map(|trail| {
                // Find latest run for this trail
                let latest_run = trail_runs
                    .iter()
                    .find(|(name, _)| name == &trail.name)
                    .and_then(|(_, run)| run.as_ref());

                let meta = if let Some(run) = latest_run {
                    // Build step status icons
                    let step_icons: String = run.steps.iter().map(|s| {
                        match s.status.as_str() {
                            "success" => '\u{2713}', // checkmark
                            "failed" => '\u{2717}',  // x mark
                            "running" => '\u{25CB}',  // circle
                            _ => '\u{00B7}',          // middle dot (pending)
                        }
                    }).collect();
                    let ago = format_time_ago(&run.started_at);
                    Some(format!("{} {}", step_icons, ago))
                } else {
                    Some("no runs".to_string())
                };

                let status = latest_run.map(|run| {
                    match run.status.as_str() {
                        "success" => ItemStatus::Active,
                        "failed" => ItemStatus::Error,
                        "running" => ItemStatus::Active,
                        _ => ItemStatus::Inactive,
                    }
                });

                ListItem {
                    id: trail.name.clone(),
                    label: trail.name.clone(),
                    status,
                    meta,
                    actions: vec![],
                }
            })
            .collect();

        list::render_list(
            frame,
            trails_inner,
            &trail_items,
            &mut self.trails_state,
            self.focused_panel == FocusedPanel::Trails,
            Some(10),
        );
    }

    fn start_trail_wizard(&mut self) {
        self.wizard_mode = WizardMode::TrailName;
        self.new_trail_name = String::new();
        self.new_trail_description = String::new();
        self.new_trail_trigger_push = false;
        self.new_trail_branches = String::new();
        self.new_trail_steps = Vec::new();
        self.new_step_name = String::new();
        self.new_step_command = String::new();
        self.text_input = TextInput::new("");
    }

    fn handle_wizard_input(&mut self, key: KeyCode) -> LivestockAction {
        if key == KeyCode::Esc {
            // Esc during StepName with existing steps = save trail
            if self.wizard_mode == WizardMode::StepName && !self.new_trail_steps.is_empty() {
                return self.finish_trail_wizard();
            }
            // Esc at any other point = cancel
            self.wizard_mode = WizardMode::Inactive;
            return LivestockAction::None;
        }

        let submitted = self.text_input.handle_input(key);
        if submitted {
            let value = self.text_input.value.trim().to_string();
            match self.wizard_mode {
                WizardMode::TrailName => {
                    if value.is_empty() {
                        return LivestockAction::None;
                    }
                    self.new_trail_name = value;
                    self.wizard_mode = WizardMode::TrailDescription;
                    self.text_input = TextInput::new("");
                }
                WizardMode::TrailDescription => {
                    self.new_trail_description = value;
                    self.wizard_mode = WizardMode::TriggerOnPush;
                    self.text_input = TextInput::new("n");
                }
                WizardMode::TriggerOnPush => {
                    let val = value.to_lowercase();
                    self.new_trail_trigger_push = val == "y" || val == "yes";
                    if self.new_trail_trigger_push {
                        self.wizard_mode = WizardMode::TriggerBranches;
                        self.text_input = TextInput::new("main");
                    } else {
                        self.wizard_mode = WizardMode::StepName;
                        self.text_input = TextInput::new("");
                    }
                }
                WizardMode::TriggerBranches => {
                    self.new_trail_branches = value;
                    self.wizard_mode = WizardMode::StepName;
                    self.text_input = TextInput::new("");
                }
                WizardMode::StepName => {
                    if value.is_empty() {
                        // Empty step name with existing steps = save
                        if !self.new_trail_steps.is_empty() {
                            return self.finish_trail_wizard();
                        }
                        return LivestockAction::None;
                    }
                    self.new_step_name = value;
                    self.wizard_mode = WizardMode::StepCommand;
                    self.text_input = TextInput::new("");
                }
                WizardMode::StepCommand => {
                    if value.is_empty() {
                        return LivestockAction::None;
                    }
                    self.new_step_command = value;
                    self.wizard_mode = WizardMode::StepTimeout;
                    self.text_input = TextInput::new("1");
                }
                WizardMode::StepTimeout => {
                    let timeout_minutes: u64 = value.parse().unwrap_or(1);
                    self.new_trail_steps.push(crate::trails::TrailStep {
                        name: self.new_step_name.clone(),
                        run: self.new_step_command.clone(),
                        env: None,
                        timeout_minutes: Some(timeout_minutes),
                    });
                    self.new_step_name = String::new();
                    self.new_step_command = String::new();
                    self.wizard_mode = WizardMode::StepName;
                    self.text_input = TextInput::new("");
                }
                WizardMode::Inactive => {}
            }
        }
        LivestockAction::None
    }

    fn finish_trail_wizard(&mut self) -> LivestockAction {
        use std::collections::BTreeMap;

        let steps: Vec<crate::trails::TrailStep> = self.new_trail_steps.clone();

        let job = crate::trails::TrailJob {
            runs_on: "native".to_string(),
            env: None,
            steps,
        };

        let on = if self.new_trail_trigger_push {
            let branches = if self.new_trail_branches.is_empty() {
                None
            } else {
                Some(
                    self.new_trail_branches
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .collect(),
                )
            };
            Some(crate::trails::TrailTrigger {
                push: Some(crate::trails::PushTrigger {
                    branches,
                    poll_interval: None,
                }),
            })
        } else {
            None
        };

        let mut jobs = BTreeMap::new();
        jobs.insert(self.new_trail_name.clone(), job);

        let trail = crate::trails::Trail {
            name: self.new_trail_name.clone(),
            on,
            env: None,
            jobs,
        };

        self.wizard_mode = WizardMode::Inactive;
        LivestockAction::SaveNewTrail(trail)
    }

    fn render_trail_wizard(
        &self,
        frame: &mut Frame,
        area: Rect,
        project: &Project,
        livestock: &Livestock,
    ) {
        let step_num = self.new_trail_steps.len() + 1;
        let step_title = format!("New Trail \u{2014} Step {}", step_num);
        let step_cmd_title = format!("New Trail \u{2014} Step {} command", step_num);
        let step_timeout_title = format!("New Trail \u{2014} Step {} timeout", step_num);
        let (label, title_text): (&str, &str) = match self.wizard_mode {
            WizardMode::TrailName => ("Trail name:", "New Trail (Step 1)"),
            WizardMode::TrailDescription => ("Description (optional):", "New Trail (Step 2)"),
            WizardMode::TriggerOnPush => ("Trigger on push? (y/n):", "New Trail \u{2014} Trigger"),
            WizardMode::TriggerBranches => ("Branches (comma-separated):", "New Trail \u{2014} Trigger"),
            WizardMode::StepName => ("Step name (Enter to add, Esc to save):", &step_title),
            WizardMode::StepCommand => ("Command:", &step_cmd_title),
            WizardMode::StepTimeout => ("Timeout (minutes):", &step_timeout_title),
            WizardMode::Inactive => return,
        };

        // Calculate completed fields for sizing
        let mut completed_lines = 0u16;
        if self.wizard_mode as u8 > WizardMode::TrailName as u8 {
            completed_lines += 1; // trail name
        }
        if self.wizard_mode as u8 > WizardMode::TrailDescription as u8 {
            completed_lines += 1; // description
        }
        if self.wizard_mode as u8 > WizardMode::TriggerOnPush as u8 {
            completed_lines += 1; // trigger on push
        }
        if self.wizard_mode as u8 > WizardMode::TriggerBranches as u8 && self.new_trail_trigger_push {
            completed_lines += 1; // trigger branches
        }
        completed_lines += self.new_trail_steps.len() as u16; // each completed step
        if matches!(self.wizard_mode, WizardMode::StepCommand | WizardMode::StepTimeout) {
            completed_lines += 1; // current step name in progress
        }

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),                          // header
                Constraint::Length(2),                          // title
                Constraint::Length(completed_lines.max(1) + 1), // completed fields
                Constraint::Length(1),                          // label
                Constraint::Length(1),                          // input
                Constraint::Length(2),                          // hints
                Constraint::Min(1),
            ])
            .split(area);

        header::render_simple_header(
            frame,
            chunks[0],
            &format!("{} / {} / New Trail", project.name, livestock.name),
            Some("creating"),
        );

        let title = Paragraph::new(format!("  {}", title_text))
            .style(Style::default().fg(BRAND_COLOR).add_modifier(Modifier::BOLD));
        frame.render_widget(title, chunks[1]);

        // Show completed fields
        let mut completed: Vec<Line> = Vec::new();
        if self.wizard_mode as u8 > WizardMode::TrailName as u8 {
            completed.push(Line::from(vec![
                Span::styled("  Name: ", Style::default().fg(Color::DarkGray)),
                Span::raw(&self.new_trail_name),
            ]));
        }
        if self.wizard_mode as u8 > WizardMode::TrailDescription as u8 {
            let desc_display = if self.new_trail_description.is_empty() { "\u{2014}" } else { &self.new_trail_description };
            completed.push(Line::from(vec![
                Span::styled("  Description: ", Style::default().fg(Color::DarkGray)),
                Span::raw(desc_display),
            ]));
        }
        if self.wizard_mode as u8 > WizardMode::TriggerOnPush as u8 {
            let trigger_display = if self.new_trail_trigger_push { "yes" } else { "no" };
            completed.push(Line::from(vec![
                Span::styled("  On push: ", Style::default().fg(Color::DarkGray)),
                Span::raw(trigger_display),
            ]));
        }
        if self.wizard_mode as u8 > WizardMode::TriggerBranches as u8 && self.new_trail_trigger_push {
            let branches_display = if self.new_trail_branches.is_empty() { "all" } else { &self.new_trail_branches };
            completed.push(Line::from(vec![
                Span::styled("  Branches: ", Style::default().fg(Color::DarkGray)),
                Span::raw(branches_display),
            ]));
        }
        for (i, step) in self.new_trail_steps.iter().enumerate() {
            let timeout = step.timeout_minutes.unwrap_or(1);
            completed.push(Line::from(vec![
                Span::styled(format!("  Step {}: ", i + 1), Style::default().fg(Color::DarkGray)),
                Span::raw(format!("{} \u{2014} {} ({}m)", step.name, step.run, timeout)),
            ]));
        }
        if matches!(self.wizard_mode, WizardMode::StepCommand | WizardMode::StepTimeout) {
            completed.push(Line::from(vec![
                Span::styled(format!("  Step {}: ", step_num), Style::default().fg(Color::DarkGray)),
                Span::raw(&self.new_step_name),
            ]));
        }
        if !completed.is_empty() {
            frame.render_widget(Paragraph::new(completed), chunks[2]);
        }

        let label_text = Paragraph::new(format!("  {}", label))
            .style(Style::default().fg(Color::White));
        frame.render_widget(label_text, chunks[3]);

        let input_area = Rect {
            x: chunks[4].x + 4,
            y: chunks[4].y,
            width: chunks[4].width.saturating_sub(4),
            height: 1,
        };
        self.text_input.render(frame, input_area);

        let hint_text = if self.wizard_mode == WizardMode::StepName && !self.new_trail_steps.is_empty() {
            "  Enter: add step  Esc: save trail"
        } else {
            "  Enter: next field  Esc: cancel"
        };
        let hints = Paragraph::new(hint_text)
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(hints, chunks[5]);
    }

    fn render_edit_form(
        &self,
        frame: &mut Frame,
        area: Rect,
        project: &Project,
        livestock: &Livestock,
    ) {
        let (label, step) = match self.edit_mode {
            EditMode::EditName => ("Name:", "1/6"),
            EditMode::EditPath => ("Path:", "2/6"),
            EditMode::EditRepo => ("Repo (optional):", "3/6"),
            EditMode::EditBranch => ("Branch (optional):", "4/6"),
            EditMode::EditLogPath => ("Log path (optional):", "5/6"),
            EditMode::EditEnvPath => ("Env path (optional):", "6/6"),
            EditMode::Normal => return,
        };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // header
                Constraint::Length(2), // title
                Constraint::Length(3), // completed fields
                Constraint::Length(1), // label
                Constraint::Length(1), // input
                Constraint::Length(2), // hints
                Constraint::Min(1),
            ])
            .split(area);

        header::render_simple_header(
            frame,
            chunks[0],
            &format!("{} / {}", project.name, livestock.name),
            Some("editing"),
        );

        let title = Paragraph::new(format!("  Edit Livestock (Step {})", step))
            .style(Style::default().fg(BRAND_COLOR).add_modifier(Modifier::BOLD));
        frame.render_widget(title, chunks[1]);

        // Show completed fields
        let mut completed: Vec<Line> = Vec::new();
        let fields: Vec<(&str, &str, EditMode)> = vec![
            ("Name", &self.edit_name, EditMode::EditName),
            ("Path", &self.edit_path, EditMode::EditPath),
            ("Repo", &self.edit_repo, EditMode::EditRepo),
            ("Branch", &self.edit_branch, EditMode::EditBranch),
            ("Log path", &self.edit_log_path, EditMode::EditLogPath),
        ];
        for (fname, fval, fmode) in &fields {
            if *fmode as u8 >= self.edit_mode as u8 {
                break;
            }
            completed.push(Line::from(vec![
                Span::styled(format!("  {}: ", fname), Style::default().fg(Color::DarkGray)),
                Span::raw(if fval.is_empty() { "—" } else { fval }),
            ]));
        }
        if !completed.is_empty() {
            frame.render_widget(Paragraph::new(completed), chunks[2]);
        }

        let label_text = Paragraph::new(format!("  {}", label))
            .style(Style::default().fg(Color::White));
        frame.render_widget(label_text, chunks[3]);

        let input_area = Rect {
            x: chunks[4].x + 4,
            y: chunks[4].y,
            width: chunks[4].width.saturating_sub(4),
            height: 1,
        };
        self.text_input.render(frame, input_area);

        let hints = Paragraph::new("  Enter: next field  Esc: cancel")
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(hints, chunks[5]);
    }
}

fn format_time_ago(timestamp: &str) -> String {
    // Parse ISO 8601 timestamp and compute relative time
    // Expected format: "2025-01-15T10:30:00Z" or similar
    use std::time::SystemTime;

    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // Try to parse the timestamp - simple ISO 8601 parser
    let ts_secs = parse_iso_timestamp(timestamp).unwrap_or(0);
    if ts_secs == 0 {
        return timestamp.to_string();
    }

    let diff = now.saturating_sub(ts_secs);

    if diff < 60 {
        "just now".to_string()
    } else if diff < 3600 {
        let mins = diff / 60;
        format!("{}m ago", mins)
    } else if diff < 86400 {
        let hours = diff / 3600;
        format!("{}h ago", hours)
    } else {
        let days = diff / 86400;
        format!("{}d ago", days)
    }
}

fn parse_iso_timestamp(s: &str) -> Option<u64> {
    // Parse "YYYY-MM-DDTHH:MM:SSZ" or "YYYY-MM-DDTHH:MM:SS+00:00"
    let s = s.trim();
    if s.len() < 19 {
        return None;
    }

    let year: u64 = s.get(0..4)?.parse().ok()?;
    let month: u64 = s.get(5..7)?.parse().ok()?;
    let day: u64 = s.get(8..10)?.parse().ok()?;
    let hour: u64 = s.get(11..13)?.parse().ok()?;
    let min: u64 = s.get(14..16)?.parse().ok()?;
    let sec: u64 = s.get(17..19)?.parse().ok()?;

    // Approximate conversion to unix timestamp
    // Days from year
    let mut days = 0u64;
    for y in 1970..year {
        days += if is_leap_year(y) { 366 } else { 365 };
    }
    // Days from month
    let month_days = [31, 28 + if is_leap_year(year) { 1 } else { 0 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    for m in 0..(month.saturating_sub(1) as usize) {
        if m < 12 {
            days += month_days[m];
        }
    }
    days += day.saturating_sub(1);

    Some(days * 86400 + hour * 3600 + min * 60 + sec)
}

fn is_leap_year(y: u64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0)
}
