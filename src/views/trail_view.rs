use crossterm::event::KeyCode;
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::components::list::{self, ListItem, ListState, ItemStatus};
use crate::components::panel::Panel;
use crate::components::trail_header;
use crate::components::trail_steps::{self, StepItem, StepListState};
use crate::config;
use crate::trails::{Trail, TrailRun};
use crate::trails::provider::StepStatus;
use crate::types::Livestock;

// ============================================================================
// Enums
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq)]
enum FocusedPanel {
    Runs,
    Steps,
    Output,
}

pub enum TrailViewAction {
    None,
    Back,
    RunTrail,
    CancelTrail,
}

// ============================================================================
// TrailView
// ============================================================================

pub struct TrailView {
    focused_panel: FocusedPanel,
    runs_state: ListState,
    step_state: StepListState,
    output_scroll: usize,
    tick: u64,

    /// Cached run history (most recent first).
    pub runs: Vec<TrailRun>,
    /// Index into the logical run list. When running, 0 = active run, 1..N = historical.
    /// When not running, 0..N-1 = historical.
    pub selected_run: usize,

    /// Live step statuses for the active (running) trail.
    pub step_statuses: Vec<StepStatus>,
    /// Live output lines per step for the active trail.
    pub step_outputs: Vec<Vec<String>>,
    /// The `run:` command for each step, cached from the trail definition.
    pub step_commands: Vec<String>,
    /// Whether a trail is currently running.
    pub running: bool,
    /// Auto-follow: track the currently running step.
    pub auto_follow: bool,
}

impl TrailView {
    pub fn new() -> Self {
        Self {
            focused_panel: FocusedPanel::Runs,
            runs_state: ListState::new(),
            step_state: StepListState::new(),
            output_scroll: 0,
            tick: 0,
            runs: vec![],
            selected_run: 0,
            step_statuses: vec![],
            step_outputs: vec![],
            step_commands: vec![],
            running: false,
            auto_follow: true,
        }
    }

    /// Initialize for a trail on a specific livestock. Loads run history and resets state.
    pub fn enter(&mut self, trail: &Trail, livestock: &Livestock) {
        let step_count = trail.first_job().map(|(_, j)| j.steps.len()).unwrap_or(0);
        self.runs = config::load_trail_runs(&livestock.name, &trail.name);
        self.selected_run = 0;
        self.runs_state = ListState::new();
        self.step_state = StepListState::new();
        self.output_scroll = 0;
        self.step_statuses = vec![StepStatus::Pending; step_count];
        self.step_outputs = vec![vec![]; step_count];
        self.step_commands = trail.first_job()
            .map(|(_, j)| j.steps.iter().map(|s| s.run.clone()).collect())
            .unwrap_or_default();
        self.running = false;
        self.auto_follow = true;
        self.focused_panel = FocusedPanel::Runs;
    }

    /// Called when the user triggers [r] to start a new run.
    /// Resets live statuses/outputs and sets running = true.
    pub fn start_run(&mut self, trail: &Trail) {
        let step_count = trail.first_job().map(|(_, j)| j.steps.len()).unwrap_or(0);
        self.step_statuses = vec![StepStatus::Pending; step_count];
        self.step_outputs = vec![vec![]; step_count];
        self.step_commands = trail.first_job()
            .map(|(_, j)| j.steps.iter().map(|s| s.run.clone()).collect())
            .unwrap_or_default();
        self.running = true;
        self.auto_follow = true;
        self.selected_run = 0;
        self.runs_state.selected = 0;
        self.step_state = StepListState::new();
        self.output_scroll = 0;
    }

    /// Called when a run completes. Reloads history.
    pub fn finish_run(&mut self, livestock_name: &str, trail_name: &str) {
        self.running = false;
        self.runs = config::load_trail_runs(livestock_name, trail_name);
    }

    /// Advance animation tick and auto-follow the running step.
    pub fn tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);

        if self.auto_follow && self.running {
            for (i, status) in self.step_statuses.iter().enumerate() {
                if *status == StepStatus::Running {
                    self.step_state.selected = i;
                    if let Some(lines) = self.step_outputs.get(i) {
                        self.output_scroll = lines.len().saturating_sub(20);
                    }
                    break;
                }
            }
        }
    }

    /// Process a step update from the execution receiver.
    pub fn apply_update(&mut self, step_index: usize, status: StepStatus, output_line: Option<String>) {
        if step_index < self.step_statuses.len() {
            self.step_statuses[step_index] = status;
        }
        if let Some(line) = output_line {
            if step_index < self.step_outputs.len() {
                self.step_outputs[step_index].push(line);
            }
        }
    }

    /// Handle keyboard input. Returns an action for the app to process.
    pub fn handle_input(&mut self, key: KeyCode, trail: &Trail) -> TrailViewAction {
        // Global keys
        match key {
            KeyCode::Esc | KeyCode::Char('q') => return TrailViewAction::Back,
            KeyCode::Char('r') if !self.running => return TrailViewAction::RunTrail,
            KeyCode::Char('x') if self.running => return TrailViewAction::CancelTrail,
            KeyCode::Tab => {
                self.focused_panel = match self.focused_panel {
                    FocusedPanel::Runs => FocusedPanel::Steps,
                    FocusedPanel::Steps => FocusedPanel::Output,
                    FocusedPanel::Output => FocusedPanel::Runs,
                };
                self.auto_follow = false;
            }
            _ => {}
        }

        // Panel-specific keys
        match self.focused_panel {
            FocusedPanel::Runs => {
                let run_count = self.logical_run_count();
                match key {
                    KeyCode::Char('j') | KeyCode::Down => {
                        if run_count > 0 {
                            self.selected_run = (self.selected_run + 1).min(run_count - 1);
                            self.runs_state.selected = self.selected_run;
                            self.step_state = StepListState::new();
                            self.output_scroll = 0;
                            self.auto_follow = false;
                        }
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        self.selected_run = self.selected_run.saturating_sub(1);
                        self.runs_state.selected = self.selected_run;
                        self.step_state = StepListState::new();
                        self.output_scroll = 0;
                        self.auto_follow = false;
                    }
                    _ => {}
                }
            }
            FocusedPanel::Steps => {
                let step_count = self.get_selected_steps(trail).len();
                match key {
                    KeyCode::Char('j') | KeyCode::Down => {
                        self.step_state.select_next(step_count);
                        self.output_scroll = 0;
                        self.auto_follow = false;
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        self.step_state.select_prev();
                        self.output_scroll = 0;
                        self.auto_follow = false;
                    }
                    _ => {}
                }
            }
            FocusedPanel::Output => {
                match key {
                    KeyCode::Char('j') | KeyCode::Down => {
                        self.output_scroll += 1;
                        self.auto_follow = false;
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        self.output_scroll = self.output_scroll.saturating_sub(1);
                        self.auto_follow = false;
                    }
                    KeyCode::Char('g') => {
                        self.output_scroll = 0;
                        self.auto_follow = false;
                    }
                    KeyCode::Char('G') => {
                        let lines = self.get_selected_output();
                        self.output_scroll = lines.len().saturating_sub(20);
                        self.auto_follow = false;
                    }
                    _ => {}
                }
            }
        }

        TrailViewAction::None
    }

    /// Render the 3-panel layout.
    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        trail: &Trail,
        livestock: &Livestock,
    ) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(10), // header
                Constraint::Min(1),    // panels
            ])
            .split(area);

        // Header
        trail_header::render_trail_header(frame, chunks[0], trail, livestock);

        // 3-panel split: Runs (25%) | Steps (25%) | Output (50%)
        let panels = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(25),
                Constraint::Percentage(25),
                Constraint::Percentage(50),
            ])
            .margin(1)
            .split(chunks[1]);

        self.render_runs_panel(frame, panels[0], trail);
        self.render_steps_panel(frame, panels[1], trail);
        self.render_output_panel(frame, panels[2]);
    }

    // ========================================================================
    // Panel rendering (private)
    // ========================================================================

    fn render_runs_panel(&mut self, frame: &mut Frame, area: Rect, trail: &Trail) {
        let hints = if self.focused_panel == FocusedPanel::Runs {
            if self.running {
                Some("[x] cancel  [Tab] next")
            } else {
                Some("[r] run  [Tab] next")
            }
        } else {
            None
        };

        let panel = Panel {
            title: "Runs",
            focused: self.focused_panel == FocusedPanel::Runs,
            hints,
        };
        let inner = panel.render(frame, area);

        let mut items: Vec<ListItem> = Vec::new();

        // Active run (if running)
        if self.running {
            let step_icons = self.build_step_icons_live();
            items.push(ListItem {
                id: "__active__".to_string(),
                label: format!("\u{2592} Running  {}", step_icons),
                status: Some(ItemStatus::Inactive),
                meta: None,
                actions: vec![],
            });
        }

        // Historical runs
        for run in &self.runs {
            let status_icon = match run.status.as_str() {
                "success" => "\u{2713}",
                "failed" | "cancelled" => "\u{2717}",
                "running" => "\u{2592}",
                _ => "?",
            };

            let step_icons = build_step_icons_from_run(run, trail);
            let date_display = format_run_timestamp(&run.started_at);

            items.push(ListItem {
                id: run.started_at.clone(),
                label: format!("{} {}  {}", status_icon, date_display, step_icons),
                status: Some(match run.status.as_str() {
                    "success" => ItemStatus::Active,
                    "failed" | "cancelled" => ItemStatus::Error,
                    _ => ItemStatus::Inactive,
                }),
                meta: None,
                actions: vec![],
            });
        }

        list::render_list(
            frame,
            inner,
            &items,
            &mut self.runs_state,
            self.focused_panel == FocusedPanel::Runs,
            Some(20),
        );
    }

    fn render_steps_panel(&self, frame: &mut Frame, area: Rect, trail: &Trail) {
        let panel = Panel {
            title: "Steps",
            focused: self.focused_panel == FocusedPanel::Steps,
            hints: None,
        };
        let inner = panel.render(frame, area);

        let step_items = self.get_selected_steps(trail);

        trail_steps::render_step_list(
            frame,
            inner,
            &step_items,
            &self.step_state,
            self.focused_panel == FocusedPanel::Steps,
            self.tick,
        );
    }

    fn render_output_panel(&self, frame: &mut Frame, area: Rect) {
        let panel = Panel {
            title: "Output",
            focused: self.focused_panel == FocusedPanel::Output,
            hints: if self.focused_panel == FocusedPanel::Output {
                Some("[j/k] scroll  [g/G] top/bottom")
            } else {
                None
            },
        };
        let inner = panel.render(frame, area);

        // Build combined lines: command header + separator + output
        let mut all_lines: Vec<Line> = Vec::new();

        // Pinned command at top
        if let Some(cmd) = self.step_commands.get(self.step_state.selected) {
            let prefix = "  $ ";
            let max_width = inner.width as usize;
            let available = max_width.saturating_sub(prefix.len());

            if available > 0 {
                let mut remaining = cmd.as_str();
                let mut first = true;
                while !remaining.is_empty() {
                    // Find a safe char boundary for splitting
                    let split_at = if remaining.len() > available {
                        // Walk backwards from `available` to find a valid char boundary
                        let mut pos = available;
                        while pos > 0 && !remaining.is_char_boundary(pos) {
                            pos -= 1;
                        }
                        if pos == 0 {
                            // Edge case: first char is wider than available space
                            remaining.chars().next().map(|c| c.len_utf8()).unwrap_or(0)
                        } else {
                            pos
                        }
                    } else {
                        remaining.len()
                    };
                    let (chunk, rest) = remaining.split_at(split_at);
                    let line_prefix = if first { "  $ " } else { "    " };
                    all_lines.push(Line::from(Span::styled(
                        format!("{}{}", line_prefix, chunk),
                        Style::default().fg(Color::DarkGray),
                    )));
                    remaining = rest;
                    first = false;
                }
            }

            // Separator
            let sep = "\u{2500}".repeat(max_width.min(60));
            all_lines.push(Line::from(Span::styled(
                format!("  {}", sep),
                Style::default().fg(Color::DarkGray),
            )));
        }

        let output_lines = self.get_selected_output();

        if output_lines.is_empty() {
            let status_text = if self.is_viewing_active_run() {
                match self.step_statuses.get(self.step_state.selected) {
                    Some(StepStatus::Pending) => "Waiting...",
                    Some(StepStatus::Running) => "Running...",
                    Some(StepStatus::Success) => "Completed successfully.",
                    Some(StepStatus::Failed { .. }) => "Failed.",
                    Some(StepStatus::Skipped) => "Skipped.",
                    None => "No data.",
                }
            } else if self.runs.is_empty() && !self.running {
                "No runs yet. Press [r] to run."
            } else {
                "No output."
            };
            all_lines.push(Line::from(Span::styled(
                format!("  {}", status_text),
                Style::default().fg(Color::DarkGray),
            )));
        } else {
            for line in &output_lines {
                all_lines.push(Line::from(format!("  {}", line)));
            }
        }

        let visible_height = inner.height as usize;
        let max_scroll = all_lines.len().saturating_sub(visible_height);
        let scroll = self.output_scroll.min(max_scroll);

        let visible_lines: Vec<Line> = all_lines
            .into_iter()
            .skip(scroll)
            .take(visible_height)
            .collect();
        let p = Paragraph::new(visible_lines);
        frame.render_widget(p, inner);
    }

    // ========================================================================
    // Helpers
    // ========================================================================

    /// Total number of logical runs (active + historical).
    fn logical_run_count(&self) -> usize {
        let historical = self.runs.len();
        if self.running { historical + 1 } else { historical }
    }

    /// Whether the currently selected run is the active (live) run.
    fn is_viewing_active_run(&self) -> bool {
        self.running && self.selected_run == 0
    }

    /// Get the historical run at the given logical index.
    /// Returns None if viewing the active run or index is out of range.
    fn get_historical_run(&self) -> Option<&TrailRun> {
        if self.running {
            if self.selected_run == 0 {
                None // active run
            } else {
                self.runs.get(self.selected_run - 1)
            }
        } else {
            self.runs.get(self.selected_run)
        }
    }

    /// Build StepItems for the selected run.
    /// If viewing the active run, uses live step_statuses.
    /// If viewing a historical run, builds from the run's step data.
    fn get_selected_steps(&self, trail: &Trail) -> Vec<StepItem> {
        let steps = trail.first_job()
            .map(|(_, job)| &job.steps[..])
            .unwrap_or(&[]);

        if self.is_viewing_active_run() {
            // Live data from the active run
            steps.iter().enumerate().map(|(i, s)| {
                StepItem {
                    name: s.name.clone(),
                    status: self.step_statuses.get(i).cloned().unwrap_or(StepStatus::Pending),
                }
            }).collect()
        } else if let Some(run) = self.get_historical_run() {
            // Historical data
            run.steps.iter().map(|s| {
                let status = match s.status.as_str() {
                    "success" => StepStatus::Success,
                    "failed" => StepStatus::Failed { exit_code: s.exit_code.unwrap_or(1) },
                    "running" => StepStatus::Running,
                    "skipped" => StepStatus::Skipped,
                    _ => StepStatus::Pending,
                };
                StepItem {
                    name: s.name.clone(),
                    status,
                }
            }).collect()
        } else {
            // No run selected (e.g., empty history, not running)
            steps.iter().map(|s| {
                StepItem {
                    name: s.name.clone(),
                    status: StepStatus::Pending,
                }
            }).collect()
        }
    }

    /// Get output lines for the selected step in the selected run.
    fn get_selected_output(&self) -> Vec<String> {
        let step_idx = self.step_state.selected;

        if self.is_viewing_active_run() {
            // Live output
            self.step_outputs
                .get(step_idx)
                .cloned()
                .unwrap_or_default()
        } else if let Some(run) = self.get_historical_run() {
            // Load from log file on disk
            let run_dir = self.run_dir_for_historical(run);
            config::load_trail_step_log(&run_dir, step_idx)
                .map(|content| content.lines().map(|l| l.to_string()).collect())
                .unwrap_or_default()
        } else {
            vec![]
        }
    }

    /// Construct the run directory path for a historical run.
    ///
    /// The runner creates run dirs using `chrono::Utc::now().format("%Y-%m-%dT%H-%M-%S")`,
    /// but TrailRun.started_at stores the RFC3339 timestamp (e.g., "2024-01-15T10:30:00+00:00").
    /// We need to convert the RFC3339 to the directory format.
    fn run_dir_for_historical(&self, run: &TrailRun) -> std::path::PathBuf {
        // Parse the RFC3339 started_at and reformat to match the directory name format
        let dir_timestamp = if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&run.started_at) {
            dt.format("%Y-%m-%dT%H-%M-%S").to_string()
        } else {
            // Fallback: replace colons with hyphens (handles edge cases)
            run.started_at.replace(':', "-")
        };
        config::trail_run_dir_for(&run.livestock, &run.trail, &dir_timestamp)
    }

    /// Build step status icons string from live step_statuses.
    fn build_step_icons_live(&self) -> String {
        self.step_statuses.iter().map(|s| {
            match s {
                StepStatus::Pending => '\u{2591}',   // light shade
                StepStatus::Running => '\u{2592}',   // medium shade
                StepStatus::Success => '\u{2713}',   // checkmark
                StepStatus::Failed { .. } => '\u{2717}', // ballot x
                StepStatus::Skipped => '\u{2591}',   // light shade
            }
        }).collect()
    }
}

/// Build step status icons from a historical TrailRun.
fn build_step_icons_from_run(run: &TrailRun, _trail: &Trail) -> String {
    run.steps.iter().map(|s| {
        match s.status.as_str() {
            "success" => '\u{2713}',
            "failed" => '\u{2717}',
            "running" => '\u{2592}',
            _ => '\u{2591}',
        }
    }).collect()
}

/// Format a run timestamp for display in the runs list.
/// Takes an RFC3339 timestamp and returns a shortened display form.
fn format_run_timestamp(started_at: &str) -> String {
    if started_at.len() >= 16 {
        // Show "YYYY-MM-DD HH:MM" from the RFC3339 string
        started_at[..16].replace('T', " ")
    } else {
        started_at.to_string()
    }
}
