use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::prelude::*;
use ratatui::DefaultTerminal;

use crate::components::claude_splash::{self, ClaudeSplashState, ClaudeSplashAction};
use crate::components::confirm_dialog::{ConfirmDialog, ConfirmAction};
use crate::components::help_overlay;
use crate::config;
use crate::context;
use crate::crontab;
use crate::editor;
use crate::tmux;
use crate::types::*;
use crate::watcher::{self, WatchEvent};
use crate::views::global_dashboard::GlobalDashboard;
use crate::views::project_context::ProjectContextView;
use crate::views::barn_context::BarnContextView;
use crate::views::worm_detail::WormDetailView;
use crate::views::worm_run_log::WormRunLogView;
use crate::views::livestock_detail::LivestockDetailView;
use crate::views::logs_view::LogsView;
use crate::views::critter_detail::CritterDetailView;
use crate::views::critter_logs::CritterLogsView;
use crate::views::wiki_view::WikiView;
use crate::views::herd_detail::HerdDetailView;
use crate::views::night_sky::NightSkyView;
use crate::views::issues_view::{IssuesView, IssuesAction};
use crate::views::ranchhand_detail::{RanchHandDetailView, RanchHandAction};
use crate::views::trail_view::{TrailView, TrailViewAction};
use crate::views::vault_view::{VaultView, VaultAction, VaultMode};
use crate::vault::crypto;
use crate::trails::provider::TrailProvider;
use crate::slack::{self, SlackStatus, SlackEvent};

// ============================================================================
// App State
// ============================================================================

pub struct App {
    pub view: AppView,
    pub previous_view: Option<AppView>,
    pub projects: Vec<Project>,
    pub barns: Vec<Barn>,
    pub worms: Vec<Worm>,
    pub windows: Vec<tmux::TmuxWindow>,
    pub should_quit: bool,
    pub error: Option<String>,
    pub show_help: bool,
    pub confirm_dialog: Option<ConfirmDialog>,

    // Sub-view states
    pub global_dashboard: GlobalDashboard,
    pub project_view: ProjectContextView,
    pub barn_view: BarnContextView,
    pub worm_view: WormDetailView,
    pub worm_run_log_view: Option<WormRunLogView>,
    pub livestock_view: LivestockDetailView,
    pub logs_view: Option<LogsView>,
    pub critter_view: CritterDetailView,
    pub critter_logs_view: Option<CritterLogsView>,
    pub wiki_view: WikiView,
    pub herd_view: HerdDetailView,
    pub night_sky_view: NightSkyView,
    pub issues_view: IssuesView,
    pub ranchhand_view: RanchHandDetailView,
    pub vault_view: VaultView,

    // Trail execution state
    pub trail_view: TrailView,
    pub trail_provider: Option<crate::trails::native::NativeProvider>,
    pub trail_run_receiver: Option<tokio::sync::mpsc::Receiver<crate::trails::provider::StepUpdate>>,
    pub trail_run_dir: Option<std::path::PathBuf>,

    // Slack integration
    pub slack_rx: Option<std::sync::mpsc::Receiver<SlackEvent>>,
    pub slack_status: SlackStatus,

    // Editor request: (content, callback_id) — handled by main loop
    pub pending_editor: Option<PendingEditor>,

    // Claude splash screen state
    pub claude_splash: Option<ClaudeSplashState>,
    pub claude_splash_window: Option<u32>,
    pub claude_splash_prompt: Option<String>,
    pub claude_splash_tools: Option<Vec<String>>,
    pub claude_splash_tick: Option<std::time::Instant>,
}

pub struct PendingEditor {
    pub content: String,
    pub filename: String,
    pub callback: EditorCallback,
}

pub enum EditorCallback {
    UpdateWormCommand(Worm),
}

impl App {
    pub fn new() -> Self {
        let projects = config::load_projects();
        let barns = config::load_barns();
        let worms = config::load_worms();
        let windows = tmux::list_yeehaw_windows();

        Self {
            view: AppView::Global,
            previous_view: None,
            projects,
            barns,
            worms,
            windows,
            should_quit: false,
            error: None,
            show_help: false,
            confirm_dialog: None,
            global_dashboard: GlobalDashboard::new(),
            project_view: ProjectContextView::new(),
            barn_view: BarnContextView::new(),
            worm_view: WormDetailView::new(),
            worm_run_log_view: None,
            livestock_view: LivestockDetailView::new(),
            logs_view: None,
            critter_view: CritterDetailView::new(),
            critter_logs_view: None,
            wiki_view: WikiView::new(),
            herd_view: HerdDetailView::new(),
            night_sky_view: NightSkyView::new(None),
            issues_view: IssuesView::new(),
            ranchhand_view: RanchHandDetailView::new(),
            vault_view: VaultView::new(),
            trail_view: TrailView::new(),
            trail_provider: None,
            trail_run_receiver: None,
            trail_run_dir: None,
            slack_rx: None,
            slack_status: SlackStatus::default(),
            pending_editor: None,
            claude_splash: None,
            claude_splash_window: None,
            claude_splash_prompt: None,
            claude_splash_tools: None,
            claude_splash_tick: None,
        }
    }

    pub fn start_slack(&mut self) {
        let cfg = config::load_config();
        if cfg.slack.as_ref().is_some_and(|s| s.enabled) {
            self.slack_status.enabled = true;
        }
        self.slack_rx = slack::start_slack_listener();
    }

    pub fn reload(&mut self) {
        self.projects = config::load_projects();
        self.barns = config::load_barns();
        self.worms = config::load_worms();
        self.windows = tmux::list_yeehaw_windows();
    }

    pub fn refresh_windows(&mut self) {
        self.windows = tmux::list_yeehaw_windows();
    }

    pub fn show_claude_splash(&mut self, window_index: u32, system_prompt: String, tools: Vec<String>) {
        self.claude_splash = Some(ClaudeSplashState::new());
        self.claude_splash_window = Some(window_index);
        self.claude_splash_prompt = Some(system_prompt);
        self.claude_splash_tools = Some(tools);
        self.claude_splash_tick = Some(std::time::Instant::now());
    }

    pub fn dismiss_claude_splash(&mut self) {
        self.claude_splash = None;
        self.claude_splash_window = None;
        self.claude_splash_prompt = None;
        self.claude_splash_tools = None;
        self.claude_splash_tick = None;
    }

    /// Navigate to a new view
    pub fn navigate(&mut self, view: AppView) {
        match &view {
            AppView::Global => {
                tmux::update_status_bar(None);
                tmux::ensure_correct_status_bar();
            }
            AppView::Project { project } => {
                tmux::update_status_bar(Some(&project.name));
            }
            AppView::Barn { barn } => {
                tmux::update_status_bar(Some(&format!("Barn: {}", barn.name)));
            }
            AppView::Worm { worm } => {
                tmux::update_status_bar(Some(&format!("Worm: {}", worm.name)));
            }
            AppView::Trail { ref trail, .. } => {
                tmux::update_status_bar(Some(&format!("Trail: {}", trail.name)));
            }
            AppView::Vault { .. } => {
                tmux::update_status_bar(Some("Vault"));
            }
            _ => {}
        }
        self.view = view;
    }

    /// Navigate back
    pub fn go_back(&mut self) {
        match &self.view {
            AppView::Global => {}
            AppView::Project { .. } | AppView::Barn { .. } | AppView::Worm { .. } => {
                self.navigate(AppView::Global);
            }
            AppView::Wiki { project } | AppView::Issues { project } => {
                let project = project.clone();
                self.navigate(AppView::Project { project });
            }
            AppView::Livestock { project, source, source_barn, .. } => {
                if source == "barn" {
                    if let Some(barn) = source_barn.clone() {
                        self.navigate(AppView::Barn { barn });
                    } else {
                        let project = project.clone();
                        self.navigate(AppView::Project { project });
                    }
                } else {
                    let project = project.clone();
                    self.navigate(AppView::Project { project });
                }
            }
            AppView::Logs { project, livestock, source, source_barn } => {
                let view = AppView::Livestock {
                    project: project.clone(),
                    livestock: livestock.clone(),
                    source: source.clone(),
                    source_barn: source_barn.clone(),
                };
                self.navigate(view);
            }
            AppView::Critter { barn, .. } => {
                let barn = barn.clone();
                self.navigate(AppView::Barn { barn });
            }
            AppView::CritterLogs { barn, critter } => {
                let view = AppView::Critter {
                    barn: barn.clone(),
                    critter: critter.clone(),
                };
                self.navigate(view);
            }
            AppView::Herd { project, .. } => {
                let project = project.clone();
                self.navigate(AppView::Project { project });
            }
            AppView::RanchHand { project, .. } => {
                let project = project.clone();
                self.navigate(AppView::Project { project });
            }
            AppView::WormRunLog { worm, .. } => {
                let worm = worm.clone();
                self.navigate(AppView::Worm { worm });
            }
            AppView::Trail { project, livestock, source, source_barn, .. } => {
                let view = AppView::Livestock {
                    project: project.clone(),
                    livestock: livestock.clone(),
                    source: source.clone(),
                    source_barn: source_barn.clone(),
                };
                self.navigate(view);
            }
            AppView::NightSky => {
                if let Some(prev) = self.previous_view.take() {
                    self.navigate(prev);
                } else {
                    self.navigate(AppView::Global);
                }
            }
            AppView::Vault { .. } => {
                self.vault_view.enter_locked();
                if let Some(prev) = self.previous_view.take() {
                    self.navigate(prev);
                } else {
                    self.navigate(AppView::Global);
                }
            }
        }
    }
}

// ============================================================================
// Main Event Loop
// ============================================================================

pub fn run(terminal: &mut DefaultTerminal) -> Result<()> {
    let mut app = App::new();

    // Start file watcher
    let watch_rx = watcher::start_watcher(&config::yeehaw_dir());

    // Start Slack listener
    app.start_slack();

    loop {
        // Process file watcher events (non-blocking)
        if let Some(ref rx) = watch_rx {
            while let Ok(event) = rx.try_recv() {
                match event {
                    WatchEvent::ConfigChanged => {
                        app.reload();
                    }
                    WatchEvent::WormTrigger(filename) => {
                        handle_worm_trigger(&mut app, &filename);
                    }
                }
            }
        }

        // Process Slack events (non-blocking)
        {
            let mut slack_events = Vec::new();
            if let Some(ref rx) = app.slack_rx {
                while let Ok(event) = rx.try_recv() {
                    slack_events.push(event);
                }
            }
            for event in slack_events {
                handle_slack_event(&mut app, event);
            }
        }

        // Handle pending editor (needs terminal access)
        if let Some(pending) = app.pending_editor.take() {
            // Restore terminal for the editor
            ratatui::restore();
            let result = editor::edit_in_editor(&pending.content, &pending.filename);
            // Re-init terminal
            *terminal = ratatui::init();

            if let Some(new_content) = result {
                match pending.callback {
                    EditorCallback::UpdateWormCommand(mut worm) => {
                        worm.command = new_content;
                        if config::save_worm(&worm).is_ok() {
                            let _ = crontab::sync_crontab();
                            app.reload();
                            app.navigate(AppView::Worm { worm });
                        }
                    }
                }
            }
            continue;
        }

        // Claude splash countdown tick (every second)
        if let Some(ref mut splash) = app.claude_splash {
            if let Some(ref tick_time) = app.claude_splash_tick {
                if tick_time.elapsed() >= std::time::Duration::from_secs(1) {
                    app.claude_splash_tick = Some(std::time::Instant::now());
                    match splash.tick() {
                        ClaudeSplashAction::Launch => {
                            if let Some(idx) = app.claude_splash_window {
                                tmux::switch_to_window(idx);
                            }
                            app.dismiss_claude_splash();
                            app.refresh_windows();
                            continue;
                        }
                        _ => {}
                    }
                }
            }
        }

        // Check for vault trigger file (from Ctrl+P tmux keybinding)
        {
            let trigger_path = config::vault_trigger_file();
            if trigger_path.exists() {
                let source_pane = std::fs::read_to_string(&trigger_path)
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                let _ = std::fs::remove_file(&trigger_path);

                if !matches!(app.view, AppView::Vault { .. }) {
                    app.previous_view = Some(app.view.clone());
                    app.vault_view = VaultView::new();

                    if crypto::vault_exists(&config::vault_file()) {
                        app.vault_view.enter_locked();
                    } else {
                        app.vault_view.enter_creating();
                    }

                    app.navigate(AppView::Vault { source_pane });
                }
            }
        }

        // Draw
        terminal.draw(|frame| draw(frame, &mut app))?;

        // Poll for events
        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }

                // Clear error on any input
                if app.error.is_some() {
                    app.error = None;
                }

                // Confirm dialog handling
                if app.confirm_dialog.is_some() {
                    match key.code {
                        KeyCode::Char('y') | KeyCode::Char('Y') => {
                            let dialog = app.confirm_dialog.take().unwrap();
                            handle_confirm_action(&mut app, dialog.on_confirm);
                        }
                        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                            app.confirm_dialog = None;
                        }
                        _ => {}
                    }
                    continue;
                }

                // Claude splash handling
                if app.claude_splash.is_some() {
                    let splash = app.claude_splash.as_mut().unwrap();
                    match claude_splash::handle_key(splash, key) {
                        ClaudeSplashAction::Launch => {
                            if let Some(idx) = app.claude_splash_window {
                                tmux::switch_to_window(idx);
                            }
                            app.dismiss_claude_splash();
                            app.refresh_windows();
                        }
                        ClaudeSplashAction::Cancel => {
                            if let Some(idx) = app.claude_splash_window {
                                tmux::kill_window(idx);
                            }
                            app.dismiss_claude_splash();
                            app.refresh_windows();
                        }
                        ClaudeSplashAction::None | ClaudeSplashAction::Tick => {}
                    }
                    continue;
                }

                // When any view is in edit/input mode, skip global keybinds
                let in_input_mode = (matches!(app.view, AppView::Global) && app.global_dashboard.is_input_mode())
                    || (matches!(app.view, AppView::Project { .. }) && app.project_view.is_input_mode())
                    || (matches!(app.view, AppView::Barn { .. }) && app.barn_view.is_editing())
                    || (matches!(app.view, AppView::Livestock { .. }) && app.livestock_view.is_editing())
                    || (matches!(app.view, AppView::Critter { .. }) && app.critter_view.is_editing())
                    || matches!(app.view, AppView::Vault { .. });

                if !in_input_mode {
                    // Help toggle
                    if key.code == KeyCode::Char('?') {
                        app.show_help = !app.show_help;
                        continue;
                    }

                    if app.show_help {
                        if key.code == KeyCode::Esc {
                            app.show_help = false;
                        }
                        continue;
                    }

                    // Ctrl-R: restart
                    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('r') {
                        tmux::restart_yeehaw();
                        continue;
                    }
                }

                // Global keybinds based on current view
                match &app.view {
                    AppView::Global => {
                        if !in_input_mode {
                            match key.code {
                                KeyCode::Char('q') => {
                                    tmux::detach_from_session();
                                    continue;
                                }
                                KeyCode::Char('Q') => {
                                    tmux::kill_yeehaw_session();
                                    app.should_quit = true;
                                }
                                KeyCode::Char('v') => {
                                    app.previous_view = Some(app.view.clone());
                                    app.night_sky_view = NightSkyView::new(None);
                                    app.navigate(AppView::NightSky);
                                    continue;
                                }
                                _ => {}
                            }
                        }
                        handle_global_dashboard_input(&mut app, key.code);
                    }
                    AppView::Project { .. } => {
                        if !app.project_view.is_input_mode() {
                            match key.code {
                                KeyCode::Esc => { app.go_back(); continue; }
                                KeyCode::Char('v') => {
                                    app.previous_view = Some(app.view.clone());
                                    if let AppView::Project { ref project } = app.view {
                                        app.night_sky_view = NightSkyView::new(Some(&project.name));
                                    }
                                    app.navigate(AppView::NightSky);
                                    continue;
                                }
                                _ => {}
                            }
                        }
                        handle_project_context_input(&mut app, key.code);
                    }
                    AppView::Barn { .. } => {
                        if !app.barn_view.is_editing() {
                            match key.code {
                                KeyCode::Esc => { app.go_back(); continue; }
                                KeyCode::Char('v') => {
                                    app.previous_view = Some(app.view.clone());
                                    if let AppView::Barn { ref barn } = app.view {
                                        app.night_sky_view = NightSkyView::new(Some(&barn.name));
                                    }
                                    app.navigate(AppView::NightSky);
                                    continue;
                                }
                                _ => {}
                            }
                        }
                        handle_barn_context_input(&mut app, key.code);
                    }
                    AppView::Worm { .. } => {
                        match key.code {
                            KeyCode::Esc => { app.go_back(); continue; }
                            _ => {}
                        }
                        handle_worm_detail_input(&mut app, key.code);
                    }
                    AppView::WormRunLog { .. } => {
                        handle_worm_run_log_input(&mut app, key.code);
                    }
                    AppView::Livestock { .. } => {
                        if !app.livestock_view.is_editing() && !app.livestock_view.is_in_wizard() {
                            if key.code == KeyCode::Esc { app.go_back(); continue; }
                        }
                        handle_livestock_detail_input(&mut app, key.code);
                    }
                    AppView::Logs { .. } => {
                        handle_logs_view_input(&mut app, key.code);
                    }
                    AppView::Critter { .. } => {
                        if !app.critter_view.is_editing() {
                            if key.code == KeyCode::Esc { app.go_back(); continue; }
                        }
                        handle_critter_detail_input(&mut app, key.code);
                    }
                    AppView::CritterLogs { .. } => {
                        handle_critter_logs_input(&mut app, key.code);
                    }
                    AppView::Wiki { .. } => {
                        handle_wiki_input(&mut app, key.code);
                    }
                    AppView::Herd { .. } => {
                        match key.code {
                            KeyCode::Esc => { app.go_back(); continue; }
                            _ => {}
                        }
                        handle_herd_detail_input(&mut app, key.code);
                    }
                    AppView::NightSky => {
                        handle_night_sky_input(&mut app, key.code);
                    }
                    AppView::Issues { ref project } => {
                        let project = project.clone();
                        match app.issues_view.handle_input(key.code, &project) {
                            IssuesAction::Back => { app.go_back(); continue; }
                            IssuesAction::OpenClaude(ctx) => {
                                let working_dir = expand_path(&project.path);
                                let window_name = format!("{}-issue-claude", project.name);
                                match tmux::create_claude_window_with_context(&working_dir, &window_name, &ctx) {
                                    Ok(idx) => {
                                        let tools: Vec<String> = tmux::YEEHAW_MCP_TOOLS.iter()
                                            .map(|t| t.strip_prefix("mcp__yeehaw__").unwrap_or(t).to_string())
                                            .collect();
                                        app.show_claude_splash(idx, ctx.clone(), tools);
                                    }
                                    Err(e) => { app.error = Some(e.to_string()); }
                                }
                                app.refresh_windows();
                            }
                            IssuesAction::None => {}
                        }
                    }
                    AppView::RanchHand { ref ranchhand, .. } => {
                        let ranchhand = ranchhand.clone();
                        match app.ranchhand_view.handle_input(key.code, &ranchhand) {
                            RanchHandAction::Back => { app.go_back(); continue; }
                            RanchHandAction::None => {}
                        }
                    }
                    AppView::Trail { .. } => {
                        handle_trail_input(&mut app, key.code);
                    }
                    AppView::Vault { ref source_pane } => {
                        let source_pane = source_pane.clone();
                        let action = app.vault_view.handle_input(key.code, key.modifiers);
                        handle_vault_action(&mut app, action, source_pane);
                    }
                }
            }
        } else {
            // Tick: refresh windows periodically, animate night sky
            app.refresh_windows();
            if matches!(app.view, AppView::NightSky) {
                app.night_sky_view.tick();
            }

            // Vault idle timeout
            if matches!(app.view, AppView::Vault { .. }) && app.vault_view.is_idle_expired() {
                app.vault_view.enter_locked();
                app.go_back();
            }

            // Poll trail execution updates
            {
                let mut trail_finished = false;
                if let Some(ref mut rx) = app.trail_run_receiver {
                    while let Ok(update) = rx.try_recv() {
                        let is_terminal = matches!(
                            update.status,
                            crate::trails::provider::StepStatus::Success |
                            crate::trails::provider::StepStatus::Failed { .. }
                        );
                        app.trail_view.apply_update(update.step_index, update.status, update.output_line);

                        if is_terminal {
                            let all_done = app.trail_view.step_statuses.iter().any(|s| {
                                matches!(s, crate::trails::provider::StepStatus::Failed { .. })
                            }) || app.trail_view.step_statuses.iter().all(|s| {
                                matches!(s, crate::trails::provider::StepStatus::Success)
                            });

                            if all_done {
                                // Save final run state
                                if let Some(ref run_dir) = app.trail_run_dir {
                                    if let AppView::Trail { ref livestock, ref trail, .. } = app.view {
                                        let final_status = if app.trail_view.step_statuses.iter().all(|s| {
                                            matches!(s, crate::trails::provider::StepStatus::Success)
                                        }) {
                                            "success"
                                        } else {
                                            "failed"
                                        };

                                        let steps = trail.first_job()
                                            .map(|(_, job)| &job.steps[..])
                                            .unwrap_or(&[]);
                                        // Read original started_at from the initial run.json
                                        let original_started_at = std::fs::read_to_string(run_dir.join("run.json")).ok()
                                            .and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok())
                                            .and_then(|v| v["started_at"].as_str().map(|s| s.to_string()))
                                            .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());
                                        let run = crate::trails::TrailRun {
                                            livestock: livestock.name.clone(),
                                            trail: trail.name.clone(),
                                            started_at: original_started_at,
                                            finished_at: Some(chrono::Utc::now().to_rfc3339()),
                                            status: final_status.to_string(),
                                            steps: steps.iter().enumerate().map(|(i, s)| {
                                                crate::trails::TrailStepRun {
                                                    name: s.name.clone(),
                                                    status: match app.trail_view.step_statuses.get(i) {
                                                        Some(crate::trails::provider::StepStatus::Success) => "success".to_string(),
                                                        Some(crate::trails::provider::StepStatus::Failed { .. }) => "failed".to_string(),
                                                        _ => "pending".to_string(),
                                                    },
                                                    exit_code: match app.trail_view.step_statuses.get(i) {
                                                        Some(crate::trails::provider::StepStatus::Failed { exit_code }) => Some(*exit_code),
                                                        Some(crate::trails::provider::StepStatus::Success) => Some(0),
                                                        _ => None,
                                                    },
                                                    started_at: None,
                                                    duration_ms: None,
                                                }
                                            }).collect(),
                                        };
                                        let _ = config::save_trail_run(&run, run_dir);
                                    }
                                }
                                // Refresh the runs list in trail view
                                if let AppView::Trail { ref livestock, ref trail, .. } = app.view {
                                    app.trail_view.finish_run(&livestock.name, &trail.name);
                                }
                                trail_finished = true;
                            }
                        }
                    }
                }
                if trail_finished {
                    app.trail_run_receiver = None;
                    app.trail_provider = None;
                    app.trail_run_dir = None;
                }
            }

            // Tick trail view animation
            app.trail_view.tick();
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

// ============================================================================
// Input Handlers
// ============================================================================

fn handle_global_dashboard_input(app: &mut App, key: KeyCode) {
    let action = app.global_dashboard.handle_input(key, &app.projects, &app.barns, &app.worms, &app.windows);
    match action {
        DashboardAction::None => {}
        DashboardAction::SelectProject(idx) => {
            if let Some(project) = app.projects.get(idx).cloned() {
                app.project_view = ProjectContextView::new();
                app.navigate(AppView::Project { project });
            }
        }
        DashboardAction::SelectBarn(idx) => {
            if let Some(barn) = app.barns.get(idx).cloned() {
                app.barn_view = BarnContextView::new();
                app.navigate(AppView::Barn { barn });
            }
        }
        DashboardAction::SelectWorm(idx) => {
            if let Some(worm) = app.worms.get(idx).cloned() {
                app.worm_view = WormDetailView::new();
                app.navigate(AppView::Worm { worm });
            }
        }
        DashboardAction::SelectWindow(idx) => {
            let session_windows: Vec<_> = app.windows.iter().filter(|w| w.index > 0).collect();
            if let Some(window) = session_windows.get(idx) {
                tmux::switch_to_window(window.index);
            }
        }
        DashboardAction::NewClaude(project_idx) => {
            if let Some(project) = app.projects.get(project_idx) {
                let working_dir = expand_path(&project.path);
                let window_name = format!("{}-claude", project.name);
                let ctx = context::build_project_context(project);
                match tmux::create_claude_window_with_context(&working_dir, &window_name, &ctx) {
                    Ok(idx) => {
                        let tools: Vec<String> = tmux::YEEHAW_MCP_TOOLS.iter()
                            .map(|t| t.strip_prefix("mcp__yeehaw__").unwrap_or(t).to_string())
                            .collect();
                        app.show_claude_splash(idx, ctx.clone(), tools);
                    }
                    Err(e) => { app.error = Some(format!("Failed to create Claude: {}", e)); }
                }
            }
        }
        DashboardAction::SshToBarn(barn_idx) => {
            if let Some(barn) = app.barns.get(barn_idx) {
                if config::is_local_barn(barn) {
                    let home = dirs::home_dir().unwrap_or_default();
                    let window_name = format!("barn-{}", barn.name);
                    if let Ok(idx) = tmux::create_shell_window(home.to_str().unwrap_or("~"), &window_name) {
                        tmux::switch_to_window(idx);
                    }
                } else if let (Some(host), Some(user), Some(port), Some(key)) =
                    (&barn.host, &barn.user, barn.port, &barn.identity_file) {
                    let window_name = format!("barn-{}", barn.name);
                    if let Ok(idx) = tmux::create_ssh_window(&window_name, host, user, port, key, "~") {
                        tmux::switch_to_window(idx);
                    }
                }
            }
        }
        DashboardAction::CreateProject(name, path) => {
            let project = Project {
                name,
                path,
                summary: None,
                color: None,
                gradient_spread: None,
                gradient_inverted: None,
                livestock: vec![],
                herds: vec![],
                wiki: vec![],
                issue_provider: None,
                wiki_provider: None,
            };
            match config::save_project(&project) {
                Ok(()) => {
                    app.reload();
                    app.project_view = ProjectContextView::new();
                    app.navigate(AppView::Project { project });
                }
                Err(e) => { app.error = Some(format!("Failed to create project: {}", e)); }
            }
        }
        DashboardAction::CreateBarn(name, host, user, port, identity_file) => {
            let barn = Barn {
                name,
                host: Some(host),
                user: Some(user),
                port: Some(port),
                identity_file,
                critters: vec![],
                source: None,
                connection_type: None,
                connection_config: None,
                connectable: None,
            };
            match config::save_barn(&barn) {
                Ok(()) => {
                    app.reload();
                    app.barn_view = BarnContextView::new();
                    app.navigate(AppView::Barn { barn });
                }
                Err(e) => { app.error = Some(format!("Failed to create barn: {}", e)); }
            }
        }
        DashboardAction::CreateWorm(name, command, schedule) => {
            let worm = Worm {
                name,
                command,
                schedule,
                worm_type: "shell".to_string(),
                enabled: true,
                project: None,
                working_dir: None,
            };
            match config::save_worm(&worm) {
                Ok(()) => {
                    let _ = crontab::sync_crontab();
                    app.reload();
                    app.worm_view = WormDetailView::new();
                    app.navigate(AppView::Worm { worm });
                }
                Err(e) => { app.error = Some(format!("Failed to create worm: {}", e)); }
            }
        }
        DashboardAction::RequestDeleteProject(idx) => {
            if let Some(project) = app.projects.get(idx) {
                app.confirm_dialog = Some(ConfirmDialog::delete_project(&project.name));
            }
        }
        DashboardAction::RequestDeleteBarn(idx) => {
            if let Some(barn) = app.barns.get(idx) {
                if !config::is_local_barn(barn) {
                    app.confirm_dialog = Some(ConfirmDialog::delete_barn(&barn.name));
                }
            }
        }
        DashboardAction::RequestDeleteWorm(idx) => {
            if let Some(worm) = app.worms.get(idx) {
                app.confirm_dialog = Some(ConfirmDialog::delete_worm(&worm.name));
            }
        }
    }
}

fn handle_project_context_input(app: &mut App, key: KeyCode) {
    if let AppView::Project { ref project } = app.view {
        let project = project.clone();
        let action = app.project_view.handle_input(key, &project, &app.barns);
        match action {
            ProjectAction::None => {}
            ProjectAction::SelectLivestock(idx) => {
                if let Some(ls) = project.livestock.get(idx).cloned() {
                    app.livestock_view = LivestockDetailView::new();
                    app.navigate(AppView::Livestock {
                        project: project.clone(),
                        livestock: ls,
                        source: "project".to_string(),
                        source_barn: None,
                    });
                }
            }
            ProjectAction::SelectHerd(idx) => {
                if let Some(herd) = project.herds.get(idx).cloned() {
                    app.herd_view = HerdDetailView::new();
                    app.navigate(AppView::Herd {
                        project: project.clone(),
                        herd,
                    });
                }
            }
            ProjectAction::OpenWiki => {
                app.wiki_view = WikiView::new();
                app.navigate(AppView::Wiki { project: project.clone() });
            }
            ProjectAction::OpenIssues => {
                app.issues_view = IssuesView::new();
                app.issues_view.enter(&project);
                app.navigate(AppView::Issues { project: project.clone() });
            }
            ProjectAction::NewClaude(ls_idx) => {
                if let Some(ls) = project.livestock.get(ls_idx) {
                    let working_dir = expand_path(&ls.path);
                    let window_name = format!("{}-{}-claude", project.name, ls.name);
                    let ctx = context::build_livestock_context(&project, &ls.name);
                    match tmux::create_claude_window_with_context(&working_dir, &window_name, &ctx) {
                        Ok(idx) => {
                            let tools: Vec<String> = tmux::YEEHAW_MCP_TOOLS.iter()
                                .map(|t| t.strip_prefix("mcp__yeehaw__").unwrap_or(t).to_string())
                                .collect();
                            app.show_claude_splash(idx, ctx.clone(), tools);
                        }
                        Err(e) => { app.error = Some(format!("Failed: {}", e)); }
                    }
                }
            }
            ProjectAction::OpenShell(ls_idx) => {
                if let Some(ls) = project.livestock.get(ls_idx) {
                    let barn = ls.barn.as_ref().and_then(|bn| app.barns.iter().find(|b| b.name == *bn));
                    let window_name = format!("{}-{}", project.name, ls.name);
                    if let Some(barn) = barn {
                        if !config::is_local_barn(barn) {
                            if let (Some(host), Some(user), Some(port), Some(key)) =
                                (&barn.host, &barn.user, barn.port, &barn.identity_file) {
                                if let Ok(idx) = tmux::create_ssh_window(&window_name, host, user, port, key, &ls.path) {
                                    tmux::switch_to_window(idx);
                                }
                            }
                            return;
                        }
                    }
                    let working_dir = expand_path(&ls.path);
                    if let Ok(idx) = tmux::create_shell_window(&working_dir, &window_name) {
                        tmux::switch_to_window(idx);
                    }
                }
            }
            ProjectAction::CreateLivestock(name, path, barn, repo, branch) => {
                let livestock = Livestock {
                    name,
                    path,
                    barn,
                    repo,
                    branch,
                    log_path: None,
                    env_path: None,
                    source: None,
                    k8s_metadata: None,
                    trails: vec![],
                };
                match config::add_livestock_to_project(&project.name, &livestock) {
                    Ok(()) => {
                        app.reload();
                        // Navigate to the new livestock detail
                        let refreshed_project = app.projects.iter().find(|p| p.name == project.name).cloned().unwrap_or(project);
                        app.livestock_view = LivestockDetailView::new();
                        app.navigate(AppView::Livestock {
                            project: refreshed_project,
                            livestock,
                            source: "project".to_string(),
                            source_barn: None,
                        });
                    }
                    Err(e) => {
                        app.error = Some(format!("Failed to create livestock: {}", e));
                    }
                }
            }
            ProjectAction::SelectRanchHand(rh_name) => {
                let ranchhands = config::load_ranchhands_for_project(&project.name);
                if let Some(rh) = ranchhands.into_iter().find(|r| r.name == rh_name) {
                    app.ranchhand_view = RanchHandDetailView::new();
                    app.ranchhand_view.enter(&rh);
                    app.navigate(AppView::RanchHand {
                        project: project.clone(),
                        ranchhand: rh,
                    });
                }
            }
            ProjectAction::CreateHerd(name) => {
                let mut updated_project = project.clone();
                updated_project.herds.push(Herd {
                    name,
                    livestock: vec![],
                    critters: vec![],
                    connections: vec![],
                });
                match config::save_project(&updated_project) {
                    Ok(()) => {
                        app.reload();
                        if let Some(refreshed) = app.projects.iter().find(|p| p.name == updated_project.name).cloned() {
                            app.view = AppView::Project { project: refreshed };
                        }
                    }
                    Err(e) => {
                        app.error = Some(format!("Failed to create herd: {}", e));
                    }
                }
            }
            ProjectAction::CreateRanchHand { name, rh_type, herd } => {
                let rh = RanchHand {
                    name,
                    project: project.name.clone(),
                    rh_type,
                    config: serde_yaml::Value::Mapping(serde_yaml::Mapping::new()),
                    sync_settings: RanchHandSyncSettings {
                        auto_sync: false,
                        interval_minutes: None,
                    },
                    herd,
                    resource_mappings: vec![],
                    last_sync: None,
                };
                match config::save_ranchhand(&rh) {
                    Ok(()) => { app.reload(); }
                    Err(e) => { app.error = Some(format!("Failed to create ranchhand: {}", e)); }
                }
            }
            ProjectAction::UpdateProject(updated) => {
                match config::save_project(&updated) {
                    Ok(()) => {
                        app.reload();
                        if let Some(refreshed) = app.projects.iter().find(|p| p.name == updated.name).cloned() {
                            app.view = AppView::Project { project: refreshed };
                        }
                    }
                    Err(e) => {
                        app.error = Some(format!("Failed to update project: {}", e));
                    }
                }
            }
        }
    }
}

fn handle_barn_context_input(app: &mut App, key: KeyCode) {
    if let AppView::Barn { ref barn } = app.view {
        let barn = barn.clone();
        let livestock = config::get_livestock_for_barn(&barn.name);
        let action = app.barn_view.handle_input(key, &barn, livestock.len());
        match action {
            BarnAction::None => {}
            BarnAction::SelectLivestock(idx) => {
                if let Some((project, ls)) = livestock.get(idx).cloned() {
                    app.livestock_view = LivestockDetailView::new();
                    app.navigate(AppView::Livestock {
                        project,
                        livestock: ls,
                        source: "barn".to_string(),
                        source_barn: Some(barn.clone()),
                    });
                }
            }
            BarnAction::SelectCritter(idx) => {
                if let Some(critter) = barn.critters.get(idx).cloned() {
                    app.critter_view = CritterDetailView::new();
                    app.navigate(AppView::Critter {
                        barn: barn.clone(),
                        critter,
                    });
                }
            }
            BarnAction::CreateCritter(name, service) => {
                let mut updated_barn = barn.clone();
                updated_barn.critters.push(Critter {
                    name: name.clone(),
                    service,
                    service_path: None,
                    config_path: None,
                    log_path: None,
                    use_journald: Some(true),
                    source: None,
                    endpoint: None,
                    port: None,
                    k8s_metadata: None,
                    tf_metadata: None,
                });
                match config::save_barn(&updated_barn) {
                    Ok(()) => {
                        app.reload();
                        let refreshed = app.barns.iter().find(|b| b.name == updated_barn.name).cloned().unwrap_or(updated_barn);
                        app.navigate(AppView::Barn { barn: refreshed });
                    }
                    Err(e) => {
                        app.error = Some(format!("Failed to add critter: {}", e));
                    }
                }
            }
            BarnAction::SshToBarn => {
                if config::is_local_barn(&barn) {
                    let home = dirs::home_dir().unwrap_or_default();
                    let window_name = format!("barn-{}", barn.name);
                    if let Ok(idx) = tmux::create_shell_window(home.to_str().unwrap_or("~"), &window_name) {
                        tmux::switch_to_window(idx);
                    }
                } else if let (Some(host), Some(user), Some(port), Some(key_file)) =
                    (&barn.host, &barn.user, barn.port, &barn.identity_file) {
                    let window_name = format!("barn-{}", barn.name);
                    if let Ok(idx) = tmux::create_ssh_window(&window_name, host, user, port, key_file, "~") {
                        tmux::switch_to_window(idx);
                    }
                }
            }
            BarnAction::UpdateBarn(updated) => {
                match config::save_barn(&updated) {
                    Ok(()) => {
                        app.reload();
                        let refreshed_barn = app.barns.iter().find(|b| b.name == updated.name).cloned().unwrap_or(updated);
                        app.navigate(AppView::Barn { barn: refreshed_barn });
                    }
                    Err(e) => {
                        app.error = Some(format!("Failed to update barn: {}", e));
                    }
                }
            }
        }
    }
}

fn handle_worm_detail_input(app: &mut App, key: KeyCode) {
    if let AppView::Worm { ref worm } = app.view {
        let worm = worm.clone();
        let action = app.worm_view.handle_input(key, &worm);
        match action {
            WormAction::None => {}
            WormAction::Toggle => {
                let mut updated = worm.clone();
                updated.enabled = !updated.enabled;
                if config::save_worm(&updated).is_ok() {
                    let _ = crontab::sync_crontab();
                    app.reload();
                    app.navigate(AppView::Worm { worm: updated });
                }
            }
            WormAction::RunNow => {
                trigger_worm(&mut app.error, &worm);
            }
            WormAction::SelectRun(idx) => {
                let runs = config::load_worm_runs(&worm.name);
                if let Some(run) = runs.get(idx).cloned() {
                    app.worm_run_log_view = Some(WormRunLogView::new(&worm, &run));
                    app.navigate(AppView::WormRunLog { worm: worm.clone(), run });
                }
            }
            WormAction::Delete => {
                app.confirm_dialog = Some(ConfirmDialog::delete_worm(&worm.name));
            }
            WormAction::EditCommand => {
                app.pending_editor = Some(PendingEditor {
                    content: worm.command.clone(),
                    filename: format!("worm-{}.sh", worm.name),
                    callback: EditorCallback::UpdateWormCommand(worm.clone()),
                });
            }
        }
    }
}

fn handle_worm_run_log_input(app: &mut App, key: KeyCode) {
    if let Some(ref mut view) = app.worm_run_log_view {
        if view.handle_input(key) {
            app.go_back();
        }
    }
}

fn handle_trail_input(app: &mut App, key: KeyCode) {
    if let AppView::Trail {
        ref project, ref livestock, ref trail, ref source_barn, ..
    } = app.view {
        let project = project.clone();
        let trail = trail.clone();
        let livestock = livestock.clone();
        let source_barn = source_barn.clone();

        let action = app.trail_view.handle_input(key, &trail);
        match action {
            TrailViewAction::None => {}
            TrailViewAction::Back => {
                app.go_back();
            }
            TrailViewAction::RunTrail => {
                // Find the barn for this livestock (fall back to local barn)
                let barn = source_barn.as_ref().or_else(|| {
                    livestock.barn.as_ref().and_then(|bn| app.barns.iter().find(|b| b.name == *bn))
                }).cloned().unwrap_or_else(config::local_barn);

                match crate::trails::runner::start_trail(&trail, &livestock, &barn, Some(&project.name)) {
                    Ok((run_dir, rx, provider)) => {
                        app.trail_view.start_run(&trail);
                        app.trail_run_receiver = Some(rx);
                        app.trail_provider = Some(provider);
                        app.trail_run_dir = Some(run_dir);
                    }
                    Err(e) => {
                        app.error = Some(format!("Failed to start trail: {}", e));
                    }
                }
            }
            TrailViewAction::CancelTrail => {
                if let Some(ref provider) = app.trail_provider {
                    let _ = provider.cancel();
                }
            }
        }
    }
}

fn handle_livestock_detail_input(app: &mut App, key: KeyCode) {
    if let AppView::Livestock { ref project, ref livestock, ref source, ref source_barn } = app.view {
        let project = project.clone();
        let livestock = livestock.clone();
        let source = source.clone();
        let source_barn = source_barn.clone();

        // Count sessions for this livestock
        let pattern = format!("{}-{}", project.name, livestock.name);
        let session_count = app.windows.iter().filter(|w| w.index > 0 && w.name.contains(&pattern)).count();

        // Load trails and count
        let trails = config::load_trails_for_livestock(&livestock);
        let trails_count = trails.len();

        let action = app.livestock_view.handle_input(key, &project, &livestock, session_count, trails_count);
        match action {
            LivestockAction::None => {}
            LivestockAction::OpenLogs => {
                let barn = source_barn.as_ref().or_else(|| {
                    livestock.barn.as_ref().and_then(|bn| app.barns.iter().find(|b| b.name == *bn))
                }).cloned();
                app.logs_view = Some(LogsView::new(&project, &livestock, barn.as_ref()));
                app.navigate(AppView::Logs {
                    project: project.clone(),
                    livestock: livestock.clone(),
                    source: source.clone(),
                    source_barn: source_barn.clone(),
                });
            }
            LivestockAction::OpenClaude => {
                let working_dir = expand_path(&livestock.path);
                let window_name = format!("{}-{}-claude", project.name, livestock.name);
                let ctx = context::build_livestock_context(&project, &livestock.name);
                match tmux::create_claude_window_with_context(&working_dir, &window_name, &ctx) {
                    Ok(idx) => {
                        let tools: Vec<String> = tmux::YEEHAW_MCP_TOOLS.iter()
                            .map(|t| t.strip_prefix("mcp__yeehaw__").unwrap_or(t).to_string())
                            .collect();
                        app.show_claude_splash(idx, ctx.clone(), tools);
                    }
                    Err(e) => { app.error = Some(format!("Failed: {}", e)); }
                }
            }
            LivestockAction::OpenShell => {
                let barn = source_barn.as_ref().or_else(|| {
                    livestock.barn.as_ref().and_then(|bn| app.barns.iter().find(|b| b.name == *bn))
                });
                let window_name = format!("{}-{}", project.name, livestock.name);
                if let Some(barn) = barn {
                    if !config::is_local_barn(barn) {
                        if let (Some(host), Some(user), Some(port), Some(key)) =
                            (&barn.host, &barn.user, barn.port, &barn.identity_file) {
                            if let Ok(idx) = tmux::create_ssh_window(&window_name, host, user, port, key, &livestock.path) {
                                tmux::switch_to_window(idx);
                            }
                        }
                        return;
                    }
                }
                let working_dir = expand_path(&livestock.path);
                if let Ok(idx) = tmux::create_shell_window(&working_dir, &window_name) {
                    tmux::switch_to_window(idx);
                }
            }
            LivestockAction::SelectWindow(idx) => {
                let pattern = format!("{}-{}", project.name, livestock.name);
                let session_windows: Vec<_> = app.windows.iter().filter(|w| w.index > 0 && w.name.contains(&pattern)).collect();
                if let Some(window) = session_windows.get(idx) {
                    tmux::switch_to_window(window.index);
                }
            }
            LivestockAction::OpenTrail(idx) => {
                let trails = config::load_trails_for_livestock(&livestock);
                if let Some(trail) = trails.get(idx) {
                    app.trail_view = TrailView::new();
                    app.trail_view.enter(trail, &livestock);
                    app.navigate(AppView::Trail {
                        project: project.clone(),
                        livestock: livestock.clone(),
                        trail: trail.clone(),
                        source: source.clone(),
                        source_barn: source_barn.clone(),
                    });
                } else {
                    app.error = Some("Trail not found".to_string());
                }
            }
            LivestockAction::UpdateLivestock(updated) => {
                let original_name = livestock.name.clone();
                match config::update_livestock_in_project(&project.name, &original_name, &updated) {
                    Ok(()) => {
                        app.reload();
                        // Re-navigate with updated livestock
                        app.navigate(AppView::Livestock {
                            project: app.projects.iter().find(|p| p.name == project.name).cloned().unwrap_or(project),
                            livestock: updated,
                            source: source.clone(),
                            source_barn: source_barn.clone(),
                        });
                    }
                    Err(e) => {
                        app.error = Some(format!("Failed to update livestock: {}", e));
                    }
                }
            }
            LivestockAction::UnlinkTrail(idx) => {
                let trails = config::load_trails_for_livestock(&livestock);
                if let Some(trail) = trails.get(idx) {
                    match config::unlink_trail_from_livestock(&project.name, &livestock.name, &trail.name) {
                        Ok(()) => {
                            app.reload();
                            // Re-navigate with updated livestock
                            let updated_project = app.projects.iter().find(|p| p.name == project.name).cloned().unwrap_or(project.clone());
                            let updated_livestock = updated_project.livestock.iter().find(|l| l.name == livestock.name).cloned().unwrap_or(livestock);
                            app.navigate(AppView::Livestock {
                                project: updated_project,
                                livestock: updated_livestock,
                                source: source.clone(),
                                source_barn: source_barn.clone(),
                            });
                        }
                        Err(e) => {
                            app.error = Some(format!("Failed to unlink trail: {}", e));
                        }
                    }
                }
            }
            LivestockAction::SaveNewTrail(trail) => {
                match config::save_trail(&trail) {
                    Ok(()) => {
                        match config::link_trail_to_livestock(&project.name, &livestock.name, &trail.name) {
                            Ok(()) => {
                                app.reload();
                                let updated_project = app.projects.iter().find(|p| p.name == project.name).cloned().unwrap_or(project.clone());
                                let updated_livestock = updated_project.livestock.iter().find(|l| l.name == livestock.name).cloned().unwrap_or(livestock);
                                app.navigate(AppView::Livestock {
                                    project: updated_project,
                                    livestock: updated_livestock,
                                    source: source.clone(),
                                    source_barn: source_barn.clone(),
                                });
                            }
                            Err(e) => {
                                app.error = Some(format!("Failed to link trail: {}", e));
                            }
                        }
                    }
                    Err(e) => {
                        app.error = Some(format!("Failed to save trail: {}", e));
                    }
                }
            }
        }
    }
}

fn handle_logs_view_input(app: &mut App, key: KeyCode) {
    if let Some(ref mut view) = app.logs_view {
        if view.handle_input(key) {
            app.go_back();
        }
    }
}

fn handle_critter_detail_input(app: &mut App, key: KeyCode) {
    if let AppView::Critter { ref barn, ref critter } = app.view {
        let barn = barn.clone();
        let critter = critter.clone();
        let action = app.critter_view.handle_input(key, &barn, &critter);
        match action {
            CritterAction::None => {}
            CritterAction::OpenLogs => {
                app.critter_logs_view = Some(CritterLogsView::new(&barn, &critter));
                app.navigate(AppView::CritterLogs {
                    barn: barn.clone(),
                    critter: critter.clone(),
                });
            }
            CritterAction::UpdateCritter(updated) => {
                let original_name = critter.name.clone();
                match config::update_critter_in_barn(&barn.name, &original_name, &updated) {
                    Ok(()) => {
                        app.reload();
                        // Re-navigate with updated critter and refreshed barn
                        let refreshed_barn = app.barns.iter().find(|b| b.name == barn.name).cloned().unwrap_or(barn);
                        app.navigate(AppView::Critter {
                            barn: refreshed_barn,
                            critter: updated,
                        });
                    }
                    Err(e) => {
                        app.error = Some(format!("Failed to update critter: {}", e));
                    }
                }
            }
        }
    }
}

fn handle_critter_logs_input(app: &mut App, key: KeyCode) {
    if let Some(ref mut view) = app.critter_logs_view {
        if view.handle_input(key) {
            app.go_back();
        }
    }
}

fn handle_wiki_input(app: &mut App, key: KeyCode) {
    if let AppView::Wiki { ref project } = app.view {
        let project = project.clone();
        if app.wiki_view.handle_input(key, &project) {
            app.go_back();
        }
    }
}

fn handle_herd_detail_input(app: &mut App, key: KeyCode) {
    if let AppView::Herd { ref project, ref herd } = app.view {
        let project = project.clone();
        let herd = herd.clone();
        let action = app.herd_view.handle_input(key, &project, &herd);
        match action {
            HerdAction::None => {}
            HerdAction::SelectLivestock(idx) => {
                if let Some(ls_name) = herd.livestock.get(idx) {
                    if let Some(ls) = project.livestock.iter().find(|l| l.name == *ls_name).cloned() {
                        app.livestock_view = LivestockDetailView::new();
                        app.navigate(AppView::Livestock {
                            project: project.clone(),
                            livestock: ls,
                            source: "herd".to_string(),
                            source_barn: None,
                        });
                    }
                }
            }
            HerdAction::SelectCritter(idx) => {
                if let Some(cr_ref) = herd.critters.get(idx) {
                    if let Some(barn) = app.barns.iter().find(|b| b.name == cr_ref.barn).cloned() {
                        if let Some(critter) = barn.critters.iter().find(|c| c.name == cr_ref.critter).cloned() {
                            app.critter_view = CritterDetailView::new();
                            app.navigate(AppView::Critter { barn, critter });
                        }
                    }
                }
            }
        }
    }
}

fn handle_night_sky_input(app: &mut App, key: KeyCode) {
    if app.night_sky_view.handle_input(key) {
        app.go_back();
    }
}

fn handle_vault_action(app: &mut App, action: VaultAction, source_pane: Option<String>) {
    match action {
        VaultAction::None => {}
        VaultAction::Close => {
            app.vault_view.enter_locked();
            if let Some(ref pane) = source_pane {
                let _ = std::process::Command::new("tmux")
                    .args(["select-pane", "-t", pane])
                    .output();
                if let Ok(output) = std::process::Command::new("tmux")
                    .args(["display-message", "-t", pane, "-p", "#{window_index}"])
                    .output()
                {
                    let idx_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if let Ok(idx) = idx_str.parse::<u32>() {
                        tmux::switch_to_window(idx);
                    }
                }
            }
            app.go_back();
        }
        VaultAction::CreateVault(password) => {
            let path = config::vault_file();
            match crypto::create_vault(&path, &password) {
                Ok(()) => {
                    app.vault_view.master_password = Some(password);
                    app.vault_view.enter_unlocked(vec![]);
                }
                Err(e) => {
                    app.vault_view.error = Some(format!("Failed to create vault: {}", e));
                }
            }
        }
        VaultAction::Unlock(password) => {
            let path = config::vault_file();
            match crypto::unlock_vault(&path, &password) {
                Ok(vault) => {
                    app.vault_view.master_password = Some(password);
                    app.vault_view.enter_unlocked(vault.entries);
                }
                Err(e) => {
                    app.vault_view.error = Some(e.to_string());
                    app.vault_view.master_input = crate::components::text_input::TextInput::new("");
                }
            }
        }
        VaultAction::SaveEntry { name, username, password, notes, edit_index } => {
            let now = chrono::Utc::now().to_rfc3339();
            match edit_index {
                Some(idx) => {
                    if let Some(entry) = app.vault_view.entries.get_mut(idx) {
                        entry.name = name;
                        entry.username = username;
                        entry.password = password;
                        entry.notes = notes;
                        entry.updated_at = now;
                    }
                }
                None => {
                    app.vault_view.entries.push(VaultEntry {
                        id: uuid::Uuid::new_v4().to_string(),
                        name,
                        username,
                        password,
                        notes,
                        created_at: now.clone(),
                        updated_at: now,
                    });
                }
            }
            app.vault_view.mode = VaultMode::Unlocked;
            save_vault_to_disk(app);
        }
        VaultAction::DeleteEntry(idx) => {
            if idx < app.vault_view.entries.len() {
                app.vault_view.entries.remove(idx);
                save_vault_to_disk(app);
            }
        }
        VaultAction::InjectPassword(password) => {
            if let Some(ref pane) = source_pane {
                let _ = std::process::Command::new("tmux")
                    .args(["send-keys", "-t", pane, "-l", &password])
                    .output();

                app.vault_view.enter_locked();

                let _ = std::process::Command::new("tmux")
                    .args(["select-pane", "-t", pane])
                    .output();
                if let Ok(output) = std::process::Command::new("tmux")
                    .args(["display-message", "-t", pane, "-p", "#{window_index}"])
                    .output()
                {
                    let idx_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if let Ok(idx) = idx_str.parse::<u32>() {
                        tmux::switch_to_window(idx);
                    }
                }
                app.go_back();
            } else {
                app.vault_view.error = Some("No source pane — use [c] to copy instead".to_string());
            }
        }
        VaultAction::CopyPassword(password) => {
            let result = std::process::Command::new("pbcopy")
                .stdin(std::process::Stdio::piped())
                .spawn()
                .and_then(|mut child| {
                    use std::io::Write;
                    if let Some(ref mut stdin) = child.stdin {
                        stdin.write_all(password.as_bytes())?;
                    }
                    child.wait()
                });

            match result {
                Ok(_) => {
                    app.vault_view.error = Some("Copied! (clipboard clears in 30s)".to_string());

                    std::thread::spawn(|| {
                        std::thread::sleep(std::time::Duration::from_secs(30));
                        let _ = std::process::Command::new("pbcopy")
                            .stdin(std::process::Stdio::piped())
                            .spawn()
                            .and_then(|mut child| {
                                use std::io::Write;
                                if let Some(ref mut stdin) = child.stdin {
                                    stdin.write_all(b"")?;
                                }
                                child.wait()
                            });
                    });
                }
                Err(_) => {
                    app.vault_view.error = Some("Failed to copy to clipboard".to_string());
                }
            }
        }
    }
}

fn save_vault_to_disk(app: &mut App) {
    if let Some(ref master_pw) = app.vault_view.master_password {
        let vault = Vault {
            entries: app.vault_view.entries.clone(),
        };
        let path = config::vault_file();
        if let Err(e) = crypto::save_vault(&path, &vault, master_pw) {
            app.vault_view.error = Some(format!("Failed to save vault: {}", e));
        }
    }
}

fn handle_confirm_action(app: &mut App, action: ConfirmAction) {
    match action {
        ConfirmAction::DeleteProject(name) => {
            match config::delete_project(&name) {
                Ok(true) => {
                    app.reload();
                    app.navigate(AppView::Global);
                }
                Ok(false) => {
                    app.error = Some(format!("Project '{}' not found", name));
                }
                Err(e) => {
                    app.error = Some(format!("Failed to delete project: {}", e));
                }
            }
        }
        ConfirmAction::DeleteBarn(name) => {
            match config::delete_barn(&name) {
                Ok(true) => {
                    app.reload();
                    app.navigate(AppView::Global);
                }
                Ok(false) => {
                    app.error = Some(format!("Barn '{}' not found or is local", name));
                }
                Err(e) => {
                    app.error = Some(format!("Failed to delete barn: {}", e));
                }
            }
        }
        ConfirmAction::DeleteWorm(name) => {
            match config::delete_worm(&name) {
                Ok(true) => {
                    let _ = crontab::sync_crontab();
                    app.reload();
                    app.navigate(AppView::Global);
                }
                Ok(false) => {
                    app.error = Some(format!("Worm '{}' not found", name));
                }
                Err(e) => {
                    app.error = Some(format!("Failed to delete worm: {}", e));
                }
            }
        }
    }
}

// ============================================================================
// Actions returned by sub-views
// ============================================================================

pub enum DashboardAction {
    None,
    SelectProject(usize),
    SelectBarn(usize),
    SelectWorm(usize),
    SelectWindow(usize),
    NewClaude(usize),
    SshToBarn(usize),
    CreateProject(String, String),
    CreateBarn(String, String, String, u16, Option<String>),
    CreateWorm(String, String, String),
    RequestDeleteProject(usize),
    RequestDeleteBarn(usize),
    RequestDeleteWorm(usize),
}

pub enum ProjectAction {
    None,
    SelectLivestock(usize),
    SelectHerd(usize),
    OpenWiki,
    OpenIssues,
    NewClaude(usize),
    OpenShell(usize),
    CreateLivestock(String, String, Option<String>, Option<String>, Option<String>),
    CreateHerd(String), // herd name
    UpdateProject(Project),
    SelectRanchHand(String), // ranchhand name
    CreateRanchHand { name: String, rh_type: String, herd: String },
}

pub enum BarnAction {
    None,
    SelectLivestock(usize),
    SelectCritter(usize),
    CreateCritter(String, String), // name, service
    SshToBarn,
    UpdateBarn(Barn),
}

pub enum WormAction {
    None,
    Toggle,
    RunNow,
    SelectRun(usize),
    Delete,
    EditCommand,
}

pub use crate::views::livestock_detail::LivestockAction;
pub use crate::views::critter_detail::CritterAction;
pub use crate::views::herd_detail::HerdAction;

// ============================================================================
// Drawing
// ============================================================================

fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    // Layout: main content + bottom bar
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),     // main content
            Constraint::Length(1),  // bottom bar
        ])
        .split(area);

    let main_area = chunks[0];
    let bottom_area = chunks[1];

    // Error bar
    if let Some(ref error) = app.error {
        let err_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
            ])
            .split(main_area);

        let error_text = ratatui::widgets::Paragraph::new(format!("Error: {}", error))
            .style(Style::default().fg(Color::Red));
        frame.render_widget(error_text, err_chunks[0]);

        render_view(frame, app, err_chunks[1]);
    } else {
        render_view(frame, app, main_area);
    }

    // Bottom bar
    render_bottom_bar(frame, app, bottom_area);

    // Help overlay (renders on top of everything)
    if app.show_help {
        let scope = match &app.view {
            AppView::Global => "global",
            AppView::Project { .. } => "project",
            AppView::Barn { .. } => "barn",
            AppView::Worm { .. } => "worm",
            AppView::Livestock { .. } => "livestock",
            AppView::Vault { .. } => "vault",
            _ => "general",
        };
        help_overlay::render_help_overlay(frame, area, scope);
    }

    // Confirm dialog (renders on top of everything, including help)
    if let Some(ref dialog) = app.confirm_dialog {
        dialog.render(frame, area);
    }

    // Claude splash overlay (renders on top of everything)
    if let Some(ref splash) = app.claude_splash {
        let prompt = app.claude_splash_prompt.as_deref().unwrap_or("");
        let tools = app.claude_splash_tools.as_deref().unwrap_or(&[]);
        claude_splash::render(frame, area, splash, prompt, tools);
    }
}

fn render_view(frame: &mut Frame, app: &mut App, area: Rect) {
    match &app.view {
        AppView::Global => {
            app.global_dashboard.render(frame, area, &app.projects, &app.barns, &app.worms, &app.windows);
        }
        AppView::Project { project } => {
            let project = project.clone();
            app.project_view.render(frame, area, &project, &app.barns, &app.windows);
        }
        AppView::Barn { barn } => {
            let barn = barn.clone();
            let livestock = config::get_livestock_for_barn(&barn.name);
            app.barn_view.render(frame, area, &barn, &livestock);
        }
        AppView::Worm { worm } => {
            let worm = worm.clone();
            let runs = config::load_worm_runs(&worm.name);
            app.worm_view.render(frame, area, &worm, &runs);
        }
        AppView::WormRunLog { worm, run } => {
            let worm = worm.clone();
            let run = run.clone();
            if let Some(ref view) = app.worm_run_log_view {
                view.render(frame, area, &worm, &run);
            }
        }
        AppView::Livestock { project, livestock, .. } => {
            let project = project.clone();
            let livestock = livestock.clone();
            let trails = config::load_trails_for_livestock(&livestock);
            let trail_runs: Vec<(String, Option<crate::trails::TrailRun>)> = trails.iter().map(|t| {
                let runs = config::load_trail_runs(&livestock.name, &t.name);
                let latest = runs.into_iter().next();
                (t.name.clone(), latest)
            }).collect();
            app.livestock_view.render(frame, area, &project, &livestock, &app.windows, &trails, &trail_runs);
        }
        AppView::Logs { project, livestock, .. } => {
            let project = project.clone();
            let livestock = livestock.clone();
            if let Some(ref view) = app.logs_view {
                view.render(frame, area, &project, &livestock);
            }
        }
        AppView::Critter { barn, critter } => {
            let barn = barn.clone();
            let critter = critter.clone();
            app.critter_view.render(frame, area, &barn, &critter);
        }
        AppView::CritterLogs { barn, critter } => {
            let barn = barn.clone();
            let critter = critter.clone();
            if let Some(ref view) = app.critter_logs_view {
                view.render(frame, area, &barn, &critter);
            }
        }
        AppView::Wiki { project } => {
            let project = project.clone();
            app.wiki_view.render(frame, area, &project);
        }
        AppView::Herd { project, herd } => {
            let project = project.clone();
            let herd = herd.clone();
            app.herd_view.render(frame, area, &project, &herd, &app.barns);
        }
        AppView::NightSky => {
            app.night_sky_view.render(frame, area);
        }
        AppView::Issues { project } => {
            let project = project.clone();
            app.issues_view.render(frame, area, &project);
        }
        AppView::RanchHand { ranchhand, .. } => {
            let ranchhand = ranchhand.clone();
            app.ranchhand_view.render(frame, area, &ranchhand);
        }
        AppView::Trail { ref trail, ref livestock, .. } => {
            let trail = trail.clone();
            let livestock = livestock.clone();
            app.trail_view.render(frame, area, &trail, &livestock);
        }
        AppView::Vault { .. } => {
            app.vault_view.render(frame, area);
        }
    }
}

fn render_bottom_bar(frame: &mut Frame, app: &App, area: Rect) {
    let items = get_bottom_bar_items(&app.view);
    let mut spans: Vec<Span> = items
        .iter()
        .flat_map(|(key, label)| {
            vec![
                Span::styled(
                    format!(" {} ", key),
                    Style::default().fg(Color::Rgb(212, 160, 32)).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{} ", label),
                    Style::default().fg(Color::DarkGray),
                ),
            ]
        })
        .collect();

    // Slack status indicator (right-aligned)
    if app.slack_status.enabled {
        let slack_text = if app.slack_status.connected {
            if app.slack_status.active_runs > 0 {
                format!(" Slack: {} active ", app.slack_status.active_runs)
            } else {
                " Slack: connected ".to_string()
            }
        } else {
            " Slack: disconnected ".to_string()
        };
        let slack_color = if app.slack_status.connected {
            if app.slack_status.active_runs > 0 {
                Color::Yellow
            } else {
                Color::Green
            }
        } else {
            Color::Red
        };
        spans.push(Span::styled(slack_text, Style::default().fg(slack_color)));
    }

    let bar = ratatui::widgets::Paragraph::new(Line::from(spans))
        .style(Style::default().bg(Color::Rgb(30, 30, 30)));
    frame.render_widget(bar, area);
}

fn get_bottom_bar_items(view: &AppView) -> Vec<(&'static str, &'static str)> {
    match view {
        AppView::Global => vec![
            ("v", "visualizer"),
            ("q", "detach"),
            ("Q", "quit"),
            ("Tab", ""),
            ("?", "help"),
        ],
        AppView::Project { .. } => vec![
            ("v", "visualizer"),
            ("w", "wiki"),
            ("i", "issues"),
            ("e", "edit"),
            ("Esc", "back"),
            ("?", "help"),
        ],
        AppView::Barn { .. } => vec![
            ("v", "visualizer"),
            ("s", "ssh"),
            ("e", "edit"),
            ("Esc", "back"),
            ("?", "help"),
        ],
        AppView::Worm { .. } => vec![
            ("r", "run now"),
            ("t", "toggle"),
            ("e", "edit"),
            ("d", "delete"),
            ("Esc", "back"),
            ("?", "help"),
        ],
        AppView::WormRunLog { .. } => vec![
            ("j/k", "scroll"),
            ("g/G", "top/bottom"),
            ("Esc", "back"),
        ],
        AppView::Livestock { .. } => vec![
            ("l", "logs"),
            ("e", "edit"),
            ("Tab", "switch"),
            ("Esc", "back"),
            ("?", "help"),
        ],
        AppView::Logs { .. } | AppView::CritterLogs { .. } => vec![
            ("j/k", "scroll"),
            ("g/G", "top/bottom"),
            ("r", "refresh"),
            ("Esc", "back"),
        ],
        AppView::Critter { .. } => vec![
            ("l", "logs"),
            ("e", "edit"),
            ("Esc", "back"),
            ("?", "help"),
        ],
        AppView::Wiki { .. } => vec![
            ("Tab", "switch panel"),
            ("j/k", "navigate"),
            ("Esc", "back"),
        ],
        AppView::Herd { .. } => vec![
            ("Tab", "switch panel"),
            ("n", "add"),
            ("d", "remove"),
            ("Esc", "back"),
        ],
        AppView::NightSky => vec![
            ("r", "randomize"),
            ("Space", "pause"),
            ("Esc", "exit"),
        ],
        AppView::Trail { .. } => vec![
            ("r", "run"),
            ("x", "cancel"),
            ("Tab", "switch panel"),
            ("Esc", "back"),
        ],
        AppView::Vault { .. } => vec![
            ("Esc", "lock & close"),
        ],
        _ => vec![
            ("Esc", "back"),
            ("?", "help"),
        ],
    }
}

/// Write a trigger file to ~/.yeehaw/worm-triggers/ to manually run a worm
fn trigger_worm(error: &mut Option<String>, worm: &Worm) {
    let now = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%S").to_string();
    let filename = format!("{}-{}.json", worm.name, now);
    let trigger_path = config::worm_triggers_dir().join(&filename);

    let trigger = serde_json::json!({
        "worm": worm.name,
        "triggered_at": chrono::Utc::now().to_rfc3339(),
        "trigger": "manual"
    });

    match std::fs::write(&trigger_path, trigger.to_string()) {
        Ok(()) => {}
        Err(e) => {
            *error = Some(format!("Failed to trigger worm: {}", e));
        }
    }
}

/// Process a worm trigger file detected by the watcher
fn handle_worm_trigger(app: &mut App, filename: &str) {
    let trigger_path = config::worm_triggers_dir().join(filename);

    // Read and parse trigger
    let content = match std::fs::read_to_string(&trigger_path) {
        Ok(c) => c,
        Err(_) => return,
    };

    // Delete trigger file immediately
    let _ = std::fs::remove_file(&trigger_path);

    let trigger: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return,
    };

    let worm_name = match trigger.get("worm").and_then(|v| v.as_str()) {
        Some(n) => n.to_string(),
        None => return,
    };

    let trigger_type = trigger
        .get("trigger")
        .and_then(|v| v.as_str())
        .unwrap_or("manual")
        .to_string();

    // Check if this is a trail trigger (from poll or MCP)
    if trigger_type == "poll" || trigger_type == "mcp" {
        if let (Some(livestock_name), Some(trail_name)) = (
            trigger.get("livestock").and_then(|v| v.as_str()),
            trigger.get("trail").and_then(|v| v.as_str()),
        ) {
            // Check if a trail is already running (skip policy)
            if app.trail_run_receiver.is_some() {
                // Trail already running, skip this trigger
                return;
            }

            let project_name_str = trigger.get("project").and_then(|v| v.as_str()).map(|s| s.to_string());

            // Find the livestock, trail, and barn across all projects
            let projects = config::load_projects();
            for project in &projects {
                if let Some(ls) = project.livestock.iter().find(|l| l.name == livestock_name) {
                    if let Some(trail) = config::load_trail(trail_name) {
                        let barn = ls.barn.as_ref()
                            .and_then(|bn| config::load_barns().into_iter().find(|b| &b.name == bn))
                            .unwrap_or_else(config::local_barn);
                        let proj_name = project_name_str.as_deref().unwrap_or(&project.name);
                        match crate::trails::runner::start_trail(&trail, ls, &barn, Some(proj_name)) {
                            Ok((run_dir, rx, provider)) => {
                                app.trail_view = TrailView::new();
                                app.trail_view.enter(&trail, ls);
                                app.trail_view.start_run(&trail);
                                app.trail_run_receiver = Some(rx);
                                app.trail_provider = Some(provider);
                                app.trail_run_dir = Some(run_dir);
                                app.navigate(AppView::Trail {
                                    project: project.clone(),
                                    livestock: ls.clone(),
                                    trail: trail.clone(),
                                    source: "project".to_string(),
                                    source_barn: Some(barn.clone()),
                                });
                            }
                            Err(e) => {
                                app.error = Some(format!("Failed to start trail: {}", e));
                            }
                        }
                        return;
                    }
                }
            }
            return;
        }
    }

    // Find the worm
    let worm = match app.worms.iter().find(|w| w.name == worm_name) {
        Some(w) => w.clone(),
        None => return,
    };

    // Create the worm run record
    let now = chrono::Utc::now();
    let log_filename = format!("{}.log", now.format("%Y-%m-%dT%H-%M-%S"));
    let run = crate::types::WormRun {
        worm: worm.name.clone(),
        started_at: now.to_rfc3339(),
        finished_at: None,
        exit_code: None,
        log_file: log_filename,
        trigger: trigger_type,
        status: Some("running".to_string()),
        skip_reason: None,
    };

    let _ = config::save_worm_run(&worm.name, &run);

    // Execute based on type
    if worm.worm_type == "shell" {
        let working_dir = worm.working_dir.as_deref().unwrap_or("~");
        let window_name = format!("worm-{}", worm.name);
        match tmux::create_worm_window(&window_name, &worm.command, working_dir) {
            Ok(idx) => {
                tmux::switch_to_window(idx);
            }
            Err(e) => {
                app.error = Some(format!("Failed to run worm: {}", e));
            }
        }
    } else if worm.worm_type == "claude" {
        // Claude worm: open a Claude session with the command as prompt
        let working_dir = worm.working_dir.as_deref().unwrap_or("~");
        let working_dir = expand_path(working_dir);
        let window_name = format!("worm-{}", worm.name);
        match tmux::create_claude_worm_window(&window_name, &worm.command, &working_dir) {
            Ok(idx) => {
                tmux::switch_to_window(idx);
            }
            Err(e) => {
                app.error = Some(format!("Failed to run claude worm: {}", e));
            }
        }
    }
}

fn handle_slack_event(app: &mut App, event: SlackEvent) {
    match event {
        SlackEvent::Connected => {
            app.slack_status.connected = true;
            app.slack_status.last_error = None;
        }
        SlackEvent::Disconnected => {
            app.slack_status.connected = false;
        }
        SlackEvent::RunStarted { .. } => {
            app.slack_status.active_runs += 1;
        }
        SlackEvent::RunCompleted { .. } => {
            app.slack_status.active_runs = app.slack_status.active_runs.saturating_sub(1);
            // Refresh windows since slack creates tmux windows
            app.refresh_windows();
        }
        SlackEvent::Error(msg) => {
            app.slack_status.last_error = Some(msg);
        }
    }
}

fn expand_path(path: &str) -> String {
    if path.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(&path[2..]).to_string_lossy().to_string();
        }
    }
    path.to_string()
}
