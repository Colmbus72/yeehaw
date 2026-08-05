use std::collections::HashSet;
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
use crate::remote_grid::{self, RemoteEvent, RemoteStream, RemoteStreams};
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
use crate::views::session_grid::{GridAction, GridScope, Origin, SessionGridView};
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
    /// tmux session names of barns we currently hold a local session onto.
    /// Bare names as `list-sessions` reports them, so membership is tested with
    /// [`tmux::barn_session_name`] — never `barn_session_target`, whose `=`
    /// prefix is for `-t` arguments only and would never match here.
    /// Refreshed by `refresh_windows` on the idle tick, so no render path has to
    /// shell out to tmux to know this.
    pub connected_barns: HashSet<String>,
    /// Live frame streams from connected barns, one ssh channel each.
    ///
    /// Owned for the whole life of the process but only ever *populated* while
    /// [`AppView::SessionGrid`] is on screen — see [`streams_wanted`]. Every
    /// route off the grid runs through [`App::navigate`], which is what closes
    /// them.
    pub remote_grid: RemoteStreams,
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
    pub session_grid_view: SessionGridView,
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
        let connected_barns = tmux::connected_barn_sessions(&tmux::list_session_names());

        Self {
            view: AppView::Global,
            previous_view: None,
            projects,
            barns,
            worms,
            windows,
            connected_barns,
            remote_grid: RemoteStreams::new(),
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
            session_grid_view: SessionGridView::new(GridScope::All),
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
        // Hoisted here rather than into the render path: this runs on the 250ms
        // idle tick, render runs every frame and must stay free of subprocesses.
        self.connected_barns = tmux::connected_barn_sessions(&tmux::list_session_names());
    }

    /// Open the live session grid, scoped to wherever `v` was pressed.
    pub fn open_session_grid(&mut self, scope: GridScope) {
        self.previous_view = Some(self.view.clone());
        self.session_grid_view = SessionGridView::new(scope);
        // Populate immediately so the first frame is not blank.
        let windows = self.windows.clone();
        self.session_grid_view.tick(&windows);
        self.navigate(AppView::SessionGrid);
        // Open the ssh channels now rather than waiting on the first idle tick,
        // so a barn is already logging in while the local cells paint. After
        // `navigate`, not before: the view is what decides whether a stream may
        // exist at all.
        tick_remote_streams(
            &mut self.remote_grid,
            &self.view,
            &self.barns,
            &self.connected_barns,
        );
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
        // The one place `self.view` is ever assigned, so the one place that can
        // see every route off the session grid — `go_back`, the vault trigger
        // file, a worm trigger. See [`sync_streams_for_view`].
        sync_streams_for_view(&mut self.remote_grid, &self.view, &view);
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
            AppView::SessionGrid => {
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
// Remote stream lifecycle
// ============================================================================

/// Whether the remote frame streams should be running while `view` is on screen.
///
/// Exactly one view wants them, and that is the entire cost argument for the
/// feature: a stream is a live ssh channel to a barn, emitting a rendered frame
/// every second. A dashboard parked anywhere else with streams open is paying
/// that for a grid nobody is looking at.
///
/// Written as an exhaustive `match` rather than `matches!`, deliberately. A new
/// `AppView` variant then fails to compile here instead of silently inheriting
/// whichever answer a `_` arm happened to give.
pub(crate) fn streams_wanted(view: &AppView) -> bool {
    match view {
        AppView::SessionGrid => true,
        AppView::Global
        | AppView::Project { .. }
        | AppView::Barn { .. }
        | AppView::Wiki { .. }
        | AppView::Issues { .. }
        | AppView::Livestock { .. }
        | AppView::Logs { .. }
        | AppView::Critter { .. }
        | AppView::CritterLogs { .. }
        | AppView::Herd { .. }
        | AppView::RanchHand { .. }
        | AppView::Worm { .. }
        | AppView::WormRunLog { .. }
        | AppView::Trail { .. }
        | AppView::Vault { .. } => false,
    }
}

/// Bring the streams in line with a view change.
///
/// Called from [`App::navigate`], which is the **only** place `App.view` is
/// ever assigned — and that is why the hook lives there rather than in
/// [`App::go_back`]. `go_back` is one of three routes off the grid:
///
/// - `Esc`/`v` on the grid → `go_back`.
/// - The vault trigger file, read straight from the main loop, navigates to
///   `Vault` from whatever view is up.
/// - A worm or poll trigger from the file watcher navigates to `Trail` the
///   same way.
///
/// Neither of the last two goes near `go_back`, and either one would otherwise
/// leave an ssh channel per connected barn open behind a view that is not the
/// grid.
///
/// Grid → grid changes nothing: tearing the streams down and reopening them
/// while the grid is still on screen would cost a handshake per barn and blank
/// every remote cell until the next frame landed.
pub(crate) fn sync_streams_for_view(streams: &mut RemoteStreams, from: &AppView, to: &AppView) {
    if streams_wanted(from) && !streams_wanted(to) {
        streams.shutdown();
    }
}

/// The idle tick's remote-grid work: match the running streams to the barns
/// that are connected *right now*, then take whatever frames arrived.
///
/// `reconcile` runs here and not only on open, so connecting to a barn from
/// another window while the grid is up brings its sessions in without
/// reopening. It is idempotent — a live stream is left alone — which it has to
/// be at four ticks a second.
///
/// Being in `connected` means a `yh-barn-*` tmux session exists, never that ssh
/// works: `tmux::connect_to_barn` creates that session before any ssh succeeds
/// and `connect::run` renders "unreachable" without exiting, so a barn parked
/// on its own error screen is in the set looking exactly like a healthy one.
/// The registry's anti-respawn guard is what makes that survivable; nothing
/// here may work around it.
fn tick_remote_streams(
    streams: &mut RemoteStreams,
    view: &AppView,
    barns: &[Barn],
    connected: &HashSet<String>,
) {
    tick_remote_streams_with(streams, view, barns, connected, RemoteStream::spawn)
}

/// [`tick_remote_streams`] with the spawn injected — the same seam
/// `RemoteStreams::reconcile_with` opens one level down.
///
/// The guard is here rather than at the call site so that "no stream exists off
/// the grid" is a property of this function, testable against a recording
/// spawner that reports the *attempt*. "Did not spawn" and "spawned and threw
/// it away" look identical from outside, and the difference between them is an
/// ssh handshake per barn per tick.
pub(crate) fn tick_remote_streams_with<F>(
    streams: &mut RemoteStreams,
    view: &AppView,
    barns: &[Barn],
    connected: &HashSet<String>,
    spawn: F,
) where
    F: Fn(&Barn, std::sync::mpsc::Sender<RemoteEvent>) -> Result<RemoteStream>,
{
    if !streams_wanted(view) {
        return;
    }
    streams.reconcile_with(barns, connected, spawn);
    streams.drain();
}

/// The quit teardown, in the one order that is safe.
///
/// **Streams first.** Each one reads its barn's tmux over an ssh channel that
/// the barn's own connection is multiplexing, so killing the `yh-barn-*`
/// sessions first pulls the transport out from under a reader mid-frame.
/// `shutdown` is a kill and a reap per stream, synchronously, so by the time
/// `kill_barn_sessions` runs there is nothing left reading through anything.
///
/// **yeehaw's own session last.** It takes down the session this process is
/// running in, so anything after it may never execute — which is also why
/// `should_quit` is the caller's to set.
fn quit_teardown(
    streams: &mut RemoteStreams,
    kill_barn_sessions: impl FnOnce(),
    kill_yeehaw_session: impl FnOnce(),
) {
    streams.shutdown();
    kill_barn_sessions();
    kill_yeehaw_session();
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
                            // The `continue` below skips the loop's own
                            // should_quit check, so a confirmed quit has to
                            // leave here. Outside tmux — a dev run — the
                            // kill-session is a no-op and this is the only exit.
                            if app.should_quit {
                                break;
                            }
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
                        // Reachable from the grid, and `respawn-window -k`
                        // replaces this process rather than returning, so no
                        // `Drop` runs. Measured on tmux 3.6a: the kill does
                        // reach the pane's whole process group, so the ssh
                        // children die with us either way — but leaning on
                        // that is leaning on tmux internals for a teardown,
                        // and the design is explicit that the backstop is
                        // never the primary. Kill and reap here, now.
                        app.remote_grid.shutdown();
                        tmux::restart_yeehaw();
                        continue;
                    }
                }

                // Global keybinds based on current view
                match &app.view {
                    AppView::Global => {
                        // Ctrl-D: disconnect the selected barn. Handled here and
                        // not in `handle_input` because that takes a bare
                        // `KeyCode` and never sees the modifier. `d` alone is
                        // already delete-barn, so disconnect must not share it.
                        if !in_input_mode
                            && key.modifiers.contains(KeyModifiers::CONTROL)
                            && key.code == KeyCode::Char('d')
                        {
                            let name = app
                                .global_dashboard
                                .focused_barn_index()
                                .and_then(|idx| app.barns.get(idx))
                                .map(|barn| barn.name.clone());
                            if let Some(name) = name {
                                tmux::disconnect_barn(&name);
                                // Clear the connected dot now instead of
                                // waiting on the 250ms idle tick. Skipped when
                                // nothing was disconnected so a stray C-d on
                                // another panel spawns no subprocesses.
                                app.refresh_windows();
                            }
                            continue;
                        }
                        if !in_input_mode {
                            match key.code {
                                KeyCode::Char('q') => {
                                    tmux::detach_from_session();
                                    continue;
                                }
                                KeyCode::Char('Q') => {
                                    // Barn sessions are siblings of the yeehaw
                                    // session, so killing yeehaw alone strands
                                    // every one of them: still running, no
                                    // dashboard left to reach them from. Read
                                    // the live set rather than trusting the
                                    // idle tick — a barn connected in the last
                                    // 250ms must not be quietly orphaned.
                                    app.connected_barns = tmux::connected_barn_sessions(
                                        &tmux::list_session_names(),
                                    );
                                    // The prompt is driven by the session count,
                                    // not the name list: a barn deleted while
                                    // connected leaves a session nothing can
                                    // name, and quitting past it silently is
                                    // the orphan this prompt exists to stop.
                                    let open = connected_barn_names(&app.barns, &app.connected_barns);
                                    if app.connected_barns.is_empty() {
                                        // Nothing to close in the middle step —
                                        // no barn sessions, so no streams
                                        // either — but the order lives in one
                                        // place and this path keeps to it.
                                        quit_teardown(
                                            &mut app.remote_grid,
                                            || {},
                                            tmux::kill_yeehaw_session,
                                        );
                                        app.should_quit = true;
                                    } else {
                                        app.confirm_dialog = Some(
                                            ConfirmDialog::quit_with_barn_sessions(
                                                &open,
                                                app.connected_barns.len(),
                                            ),
                                        );
                                        continue;
                                    }
                                }
                                KeyCode::Char('v') => {
                                    app.open_session_grid(GridScope::All);
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
                                    if let AppView::Project { ref project } = app.view {
                                        let scope = GridScope::Project(project.name.clone());
                                        app.open_session_grid(scope);
                                    }
                                    continue;
                                }
                                _ => {}
                            }
                        }
                        handle_project_context_input(&mut app, key.code);
                    }
                    AppView::Barn { .. } => {
                        // Ctrl-D: disconnect this barn. Same reasoning as the
                        // global arm; here the barn is already in hand.
                        if !in_input_mode
                            && key.modifiers.contains(KeyModifiers::CONTROL)
                            && key.code == KeyCode::Char('d')
                        {
                            if let AppView::Barn { ref barn } = app.view {
                                tmux::disconnect_barn(&barn.name);
                            }
                            app.refresh_windows();
                            continue;
                        }
                        if !app.barn_view.is_editing() {
                            match key.code {
                                KeyCode::Esc => { app.go_back(); continue; }
                                KeyCode::Char('v') => {
                                    if let AppView::Barn { ref barn } = app.view {
                                        let scope = GridScope::Barn(barn.name.clone());
                                        app.open_session_grid(scope);
                                    }
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
                    AppView::SessionGrid => {
                        handle_session_grid_input(&mut app, key.code);
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
                                        tmux::set_window_scope(idx, &project.name, None);
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
            // Tick: refresh windows periodically, stream the session grid
            app.refresh_windows();
            // Guarded inside on the view, so this is a no-op off the grid. It
            // runs before the local tick because both paint the same frame and
            // the remote half is the one with a network behind it — `drain` is
            // non-blocking, `reconcile` only acts when the connected set moved.
            tick_remote_streams(
                &mut app.remote_grid,
                &app.view,
                &app.barns,
                &app.connected_barns,
            );
            if matches!(app.view, AppView::SessionGrid) {
                let windows = app.windows.clone();
                app.session_grid_view.tick(&windows);
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
                        tmux::set_window_scope(idx, &project.name, None);
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
            if let Some(barn) = app.barns.get(barn_idx).cloned() {
                let barn = &barn;
                if config::is_local_barn(barn) {
                    let home = dirs::home_dir().unwrap_or_default();
                    let window_name = format!("barn-{}", barn.name);
                    if let Ok(idx) = tmux::create_shell_window(home.to_str().unwrap_or("~"), &window_name) {
                        tmux::set_window_scope(idx, "", Some(&barn.name));
                        tmux::switch_to_window(idx);
                    }
                } else {
                    let window_name = format!("barn-{}", barn.name);
                    match tmux::create_ssh_window(&window_name, barn, "~") {
                        Ok(idx) => {
                            tmux::set_window_scope(idx, "", Some(&barn.name));
                            tmux::switch_to_window(idx);
                        }
                        Err(e) => app.error = Some(format!("SSH failed: {}", e)),
                    }
                }
            }
        }
        DashboardAction::ConnectBarn(barn_idx) => {
            // `.cloned()`: `connect_barn` writes `app.error`, so the borrow of
            // `app.barns` must not still be live.
            if let Some(barn) = app.barns.get(barn_idx).cloned() {
                connect_barn(app, &barn);
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
                            tmux::set_window_scope(idx, &project.name, ls.barn.as_deref());
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
                    let barn = ls.barn.as_ref()
                        .and_then(|bn| app.barns.iter().find(|b| b.name == *bn))
                        .cloned();
                    let window_name = format!("{}-{}", project.name, ls.name);
                    if let Some(barn) = barn {
                        if !config::is_local_barn(&barn) {
                            match tmux::create_ssh_window(&window_name, &barn, &ls.path) {
                                Ok(idx) => {
                                    tmux::set_window_scope(idx, &project.name, ls.barn.as_deref());
                                    tmux::switch_to_window(idx);
                                }
                                Err(e) => app.error = Some(format!("SSH failed: {}", e)),
                            }
                            return;
                        }
                    }
                    let working_dir = expand_path(&ls.path);
                    if let Ok(idx) = tmux::create_shell_window(&working_dir, &window_name) {
                        tmux::set_window_scope(idx, &project.name, ls.barn.as_deref());
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

/// Shared by the `c` binding on the dashboard's barns panel and on the barn
/// view. Rejects exactly the barns `connect::run` rejects, using its wording
/// verbatim — the same barn must not produce two different messages depending
/// on which side noticed. `connect::run` re-checks both: it is the entry point
/// for `yeehaw connect` from a shell, where the TUI never ran.
fn connect_barn(app: &mut App, barn: &Barn) {
    if config::is_local_barn(barn) {
        app.error = Some(format!(
            "'{}' is the local barn — not connectable, just run yeehaw",
            barn.name
        ));
    } else if barn.connectable == Some(false) {
        app.error = Some(format!("barn '{}' is not connectable over SSH", barn.name));
    } else if let Err(e) = tmux::connect_to_barn(barn) {
        // `{:#}`, not `{}`: connect failures are context chains ("failed to write
        // ~/.yeehaw/tmux.conf: permission denied"), and plain Display shows only
        // the outermost layer. The layer that says what actually went wrong is
        // underneath, and this banner is the only place the user ever sees it.
        app.error = Some(format!("Connect failed: {:#}", e));
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
                        tmux::set_window_scope(idx, "", Some(&barn.name));
                        tmux::switch_to_window(idx);
                    }
                } else {
                    let window_name = format!("barn-{}", barn.name);
                    match tmux::create_ssh_window(&window_name, &barn, "~") {
                        Ok(idx) => {
                            tmux::set_window_scope(idx, "", Some(&barn.name));
                            tmux::switch_to_window(idx);
                        }
                        Err(e) => app.error = Some(format!("SSH failed: {}", e)),
                    }
                }
            }
            BarnAction::ConnectBarn => {
                connect_barn(app, &barn);
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
                        tmux::set_window_scope(idx, &project.name, livestock.barn.as_deref());
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
                }).cloned();
                let window_name = format!("{}-{}", project.name, livestock.name);
                if let Some(barn) = barn {
                    if !config::is_local_barn(&barn) {
                        match tmux::create_ssh_window(&window_name, &barn, &livestock.path) {
                            Ok(idx) => {
                                tmux::set_window_scope(idx, &project.name, livestock.barn.as_deref());
                                tmux::switch_to_window(idx);
                            }
                            Err(e) => app.error = Some(format!("SSH failed: {}", e)),
                        }
                        return;
                    }
                }
                let working_dir = expand_path(&livestock.path);
                if let Ok(idx) = tmux::create_shell_window(&working_dir, &window_name) {
                    tmux::set_window_scope(idx, &project.name, livestock.barn.as_deref());
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

fn handle_session_grid_input(app: &mut App, key: KeyCode) {
    let windows = app.windows.clone();
    // The frames go in so a number key over a barn's cell resolves to that
    // barn's session rather than falling off the end of the local list.
    let action = app
        .session_grid_view
        .handle_input(key, &windows, app.remote_grid.frames());
    match action {
        GridAction::Back => app.go_back(),
        GridAction::Jump { origin, window_index } => {
            // Stay on the grid rather than going back, so Ctrl+Y from the
            // session lands straight back here instead of the dashboard. `C-q`
            // out of a barn is `switch-client -t =yeehaw`, which lands here too.
            jump_to_cell(
                app,
                origin,
                window_index,
                tmux::switch_to_window,
                remote_grid::select_window,
                tmux::connect_to_barn,
            );
        }
        GridAction::None => {}
    }
}

/// Land the user on the session behind a number key.
///
/// `Local` is the `select-window` it always was.
///
/// `Barn` selects the window **on the barn first**, then switches into that
/// barn's local session. The other order attaches to whatever window the barn
/// happened to have selected and only then corrects it, so the user watches the
/// wrong session for the length of an ssh round trip. There is no attach race
/// to worry about in exchange: the grid only shows barns already in
/// `connected_barns`, so the `yh-barn-*` session and its ssh attach both exist
/// before any of this is reachable.
///
/// The remote half is a **blocking** ssh exec, ~70–180 ms over a warm
/// `ControlMaster`. That is affordable because it is user-initiated, on a
/// keypress. It must never end up on the 250 ms idle tick. Note the warm
/// figure is the good case only — see [`remote_grid::select_window`] for what a
/// jump to a barn whose master has died costs.
///
/// **A stale barn is jumped to without the select.** That good case does not
/// apply to a barn whose stream just died: `ConnectTimeout` is 10 s, ssh has no
/// way to report progress from behind a full-screen TUI, and a stale cell is by
/// definition the cell whose channel has already failed. Ten seconds of frozen
/// terminal on a keypress is not a trade worth landing on the right *window*
/// for, so the jump drops the select and keeps the part that cannot block:
/// `connect` is local tmux work, and it lands the user in the barn's session —
/// live if it recovered, on `yeehaw connect`'s own "unreachable" screen, retry
/// prompt and all, if it did not. The user was told: the cell says STALE.
///
/// Rejected, for the record: doing the select on a background thread and
/// switching immediately. It would correct the window ~10 s after arrival in
/// the case that matters, which is a session changing under the user long after
/// they stopped expecting it.
///
/// Both effects are injected for the same reason [`quit_teardown`]'s are: one
/// reaches the network and the other creates a tmux session, and the order
/// between them is the property worth guarding.
fn jump_to_cell(
    app: &mut App,
    origin: Origin,
    window_index: u32,
    switch_local: impl FnOnce(u32),
    select_remote: impl FnOnce(&Barn, u32) -> Result<()>,
    connect: impl FnOnce(&Barn) -> Result<()>,
) {
    let name = match origin {
        Origin::Local => {
            switch_local(window_index);
            return;
        }
        Origin::Barn(name) => name,
    };

    // A cell can outlive its barn's config entry: the grid holds the last frame
    // from a barn deleted from the ranch a moment ago, still numbered. Connect
    // to *something* rather than nothing and the number under the user's finger
    // has just sent them to a different host.
    let Some(barn) = app.barns.iter().find(|b| b.name == name).cloned() else {
        app.error = Some(format!("barn '{}' is no longer on the ranch", name));
        return;
    };

    // Read here rather than passed in from `handle_session_grid_input`: it is
    // the same map the cell was drawn from, so the badge the user pressed and
    // the route taken cannot disagree about which barns are stale.
    if !app.remote_grid.stale().contains(name.as_str()) {
        if let Err(e) = select_remote(&barn, window_index) {
            app.error = Some(format!("Jump failed: {:#}", e));
            return;
        }
    }
    // `{:#}`, like `connect_barn`: these are context chains and plain Display
    // shows only the outermost layer, never the one that says what went wrong.
    if let Err(e) = connect(&barn) {
        app.error = Some(format!("Connect failed: {:#}", e));
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

/// Names of the barns we currently hold a local session onto, in dashboard
/// order — what the quit dialog lists.
///
/// The mapping only runs forward: `barn_session_name(barn)` is looked up in the
/// set of live session names. It cannot run backward. A session name is a lossy
/// slug (`camera pi` and `camera-pi` share one) plus an FNV hash of the
/// original, so nothing recovers a barn name from `yh-barn-camera-pi-1a2b3c4d`.
///
/// A live session whose barn has since been deleted from the ranch therefore
/// has no name to show and is left out of the list. It is still closed:
/// [`tmux::kill_all_barn_sessions`] works off tmux's own session list, not off
/// this one.
fn connected_barn_names(barns: &[Barn], connected: &HashSet<String>) -> Vec<String> {
    barns
        .iter()
        .filter(|b| connected.contains(&tmux::barn_session_name(&b.name)))
        .map(|b| b.name.clone())
        .collect()
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
        ConfirmAction::QuitClosingBarnSessions => {
            // Streams, then barns, then yeehaw — see `quit_teardown` for why
            // that order is the only safe one. kill_all_barn_sessions re-reads
            // tmux, so a barn connected between the prompt and the `y` is
            // closed too, listed or not.
            quit_teardown(
                &mut app.remote_grid,
                tmux::kill_all_barn_sessions,
                tmux::kill_yeehaw_session,
            );
            app.should_quit = true;
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
    ConnectBarn(usize),
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
    ConnectBarn,
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
            // The grid answers a set of keys no other view does and none of the
            // generic navigation ones. Falling through to "general" here left
            // `l` — and `?` itself — named nowhere in the program.
            AppView::SessionGrid => "sessiongrid",
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
            app.global_dashboard.render(frame, area, &app.projects, &app.barns, &app.worms, &app.windows, &app.connected_barns);
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
        AppView::SessionGrid => {
            let windows = app.windows.clone();
            // Whatever the streams last delivered. Empty off the grid and empty
            // with no barns connected, which is the overwhelmingly common case —
            // nothing here shells out or blocks.
            //
            // `stale` alongside it, because a barn's cells outlive its stream:
            // the last frame stays on the grid, dimmed and badged, rather than
            // vanishing and renumbering everything after it.
            let stale = app.remote_grid.stale();
            app.session_grid_view
                .render(frame, area, &windows, app.remote_grid.frames(), &stale);
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
        AppView::SessionGrid => vec![
            ("1-9", "jump"),
            ("c", "claude only"),
            ("l", "local only"),
            ("f", "filter"),
            ("Esc", "back"),
            ("?", "help"),
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

#[cfg(test)]
mod tests {
    use super::*;
    // Real streams over local children — no ssh, no barn, no network. The
    // process-group discipline comes from `remote_grid`'s own helpers rather
    // than being rebuilt here: a stream child's forked subshells hold the same
    // pipe and survive a kill aimed at the child alone, which is how RG-2 left
    // `capture-pane` loops running on this machine twice.
    use crate::remote_grid::tests::{
        child_pid, failed_barns, guard_all, mark_failed, named_barn, process_state,
        recording_spawner, sessions_for, silent,
    };
    use std::cell::RefCell;

    fn barn(name: &str) -> Barn {
        Barn {
            name: name.into(),
            host: Some("172.233.141.59".into()),
            user: Some("forge".into()),
            port: None,
            identity_file: None,
            critters: vec![],
            source: None,
            connection_type: None,
            connection_config: None,
            connectable: None,
        }
    }

    /// The set is built the way the app builds it: from what `list-sessions`
    /// prints, filtered by `connected_barn_sessions`.
    fn connected(session_names: &[String]) -> HashSet<String> {
        tmux::connected_barn_sessions(session_names)
    }

    #[test]
    fn quit_lists_only_the_barns_with_an_open_session() {
        let barns = [barn("guided"), barn("camera pi"), barn("BIG UPS")];
        let set = connected(&[
            "yeehaw".into(),
            tmux::barn_session_name("guided"),
            tmux::barn_session_name("BIG UPS"),
        ]);

        assert_eq!(connected_barn_names(&barns, &set), vec!["guided", "BIG UPS"]);
    }

    #[test]
    fn quit_lists_nothing_when_no_barn_is_connected() {
        let barns = [barn("guided"), barn("camera pi")];

        assert!(connected_barn_names(&barns, &connected(&["yeehaw".into()])).is_empty());
        assert!(connected_barn_names(&barns, &HashSet::new()).is_empty());
    }

    #[test]
    fn quit_lists_the_barn_name_not_the_session_name() {
        // The dialog is read by a person: "camera pi", not
        // "yh-barn-camera-pi-6f2a10bd".
        let barns = [barn("camera pi")];
        let set = connected(&[tmux::barn_session_name("camera pi")]);

        assert_eq!(connected_barn_names(&barns, &set), vec!["camera pi"]);
    }

    #[test]
    fn a_connected_barn_never_lends_its_dot_to_a_name_that_merely_starts_the_same() {
        // Session lookup is exact-membership, not prefix. Were it prefix-based,
        // connecting to `guided` would list `guided-2` as open and quitting
        // would report closing a production host nobody touched.
        let barns = [barn("guided"), barn("guided-2")];
        let set = connected(&[tmux::barn_session_name("guided-2")]);

        assert_eq!(connected_barn_names(&barns, &set), vec!["guided-2"]);
    }

    #[test]
    fn barns_that_slugify_alike_are_told_apart() {
        // `camera pi` and `camera-pi` share a slug and differ only in the hash.
        let barns = [barn("camera pi"), barn("camera-pi")];
        let set = connected(&[tmux::barn_session_name("camera-pi")]);

        assert_eq!(connected_barn_names(&barns, &set), vec!["camera-pi"]);
    }

    #[test]
    fn a_session_for_a_deleted_barn_is_unnameable_but_still_gets_closed() {
        // No barn record, so nothing to print — the list is what the dialog can
        // name, not what the kill covers. tmux's own session list drives the
        // kill, and it still holds this session.
        let barns = [barn("guided")];
        let sessions = vec![
            tmux::barn_session_name("guided"),
            tmux::barn_session_name("deleted last week"),
        ];
        let set = connected(&sessions);

        assert_eq!(connected_barn_names(&barns, &set), vec!["guided"]);
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn the_quit_dialog_names_every_barn_the_lookup_found() {
        let barns = [barn("guided"), barn("camera pi")];
        let set = connected(&[
            tmux::barn_session_name("guided"),
            tmux::barn_session_name("camera pi"),
        ]);

        let open = connected_barn_names(&barns, &set);
        let dialog = ConfirmDialog::quit_with_barn_sessions(&open, set.len());

        assert!(dialog.message.contains("guided"), "{}", dialog.message);
        assert!(dialog.message.contains("camera pi"), "{}", dialog.message);
        assert!(matches!(dialog.on_confirm, ConfirmAction::QuitClosingBarnSessions));
    }

    #[test]
    fn the_quit_dialog_counts_a_session_it_cannot_name() {
        // Barn deleted from the ranch while connected: one session to close,
        // one name to print. The header counts sessions, so the prompt never
        // promises fewer closures than it performs.
        let barns = [barn("guided")];
        let set = connected(&[
            tmux::barn_session_name("guided"),
            tmux::barn_session_name("deleted last week"),
        ]);

        let open = connected_barn_names(&barns, &set);
        let dialog = ConfirmDialog::quit_with_barn_sessions(&open, set.len());

        assert!(dialog.message.contains("Close 2 barn connections"), "{}", dialog.message);
        assert!(dialog.message.contains("guided"), "{}", dialog.message);
        assert!(dialog.message.contains("and 1 more"), "{}", dialog.message);
    }

    // === remote stream lifecycle ==========================================
    //
    // The streams are ssh channels, so the cost argument for the whole feature
    // is that they exist while the session grid is on screen and at no other
    // moment. These drive that through the real registry with real children,
    // and never through a terminal.

    /// Build a fixture from JSON so a five-field view payload does not need a
    /// twenty-line literal. Every one of these types is `Deserialize`.
    fn fixture<T: serde::de::DeserializeOwned>(json: &str) -> T {
        serde_json::from_str(json).expect("fixture should deserialize")
    }

    /// Every view the grid is actually left *for*.
    ///
    /// `go_back` is only one of the three routes. The vault trigger file and a
    /// worm/poll trigger both navigate straight out of `SessionGrid` from the
    /// main loop without going anywhere near `go_back`, so `Vault` and `Trail`
    /// are in this list on purpose and not for symmetry.
    fn views_off_the_grid() -> Vec<AppView> {
        let project: Project = fixture(r#"{"name":"proj","path":"/tmp/proj"}"#);
        vec![
            // `go_back` with no previous view.
            AppView::Global,
            // `go_back` to wherever `v` was pressed.
            AppView::Project { project: project.clone() },
            AppView::Barn { barn: barn("guided") },
            // The vault trigger file, from the main loop.
            AppView::Vault { source_pane: None },
            // A worm or poll trigger, from the file watcher.
            AppView::Trail {
                project,
                livestock: fixture(r#"{"name":"api","path":"/srv/api"}"#),
                trail: fixture(r#"{"name":"deploy","jobs":{}}"#),
                source: "project".to_string(),
                source_barn: None,
            },
        ]
    }

    /// A registry with a live stream per named barn, plus the group guards that
    /// take the children with them however the test ends.
    fn streaming(
        barns: &[Barn],
        log: &RefCell<Vec<String>>,
    ) -> (RemoteStreams, Vec<crate::remote_grid::tests::GroupGuard>) {
        let names: Vec<&str> = barns.iter().map(|b| b.name.as_str()).collect();
        let mut streams = RemoteStreams::new();
        streams.reconcile_with(barns, &sessions_for(&names), recording_spawner(log, silent));
        let guards = guard_all(&streams);
        for name in &names {
            assert!(
                child_pid(&streams, name).is_some(),
                "control: '{name}' has to be streaming before the test starts"
            );
        }
        (streams, guards)
    }

    #[test]
    fn only_the_session_grid_wants_streams() {
        assert!(streams_wanted(&AppView::SessionGrid));
        for view in views_off_the_grid() {
            assert!(!streams_wanted(&view), "{view:?} wants ssh channels open");
        }
    }

    #[test]
    fn leaving_the_grid_shuts_every_stream_down() {
        // Whichever view the grid is left for, and however it is left. Every
        // one of these routes runs through `navigate`, so this is the decision
        // `navigate` makes, driven against real children.
        for to in views_off_the_grid() {
            let barns = [named_barn("guided"), named_barn("smash-mac")];
            let log = RefCell::new(Vec::new());
            let (mut streams, _guards) = streaming(&barns, &log);
            let pids: Vec<i32> = barns
                .iter()
                .map(|b| child_pid(&streams, &b.name).expect("streaming"))
                .collect();

            sync_streams_for_view(&mut streams, &AppView::SessionGrid, &to);

            for (b, pid) in barns.iter().zip(&pids) {
                assert!(
                    child_pid(&streams, &b.name).is_none(),
                    "leaving the grid for {to:?} kept '{}' streaming",
                    b.name
                );
                // Killed *and* reaped, synchronously. A `Z` here is a defunct
                // ssh per barn per grid open for the rest of the TUI's life.
                assert_eq!(
                    process_state(*pid),
                    None,
                    "'{}' outlived the grid as {:?} on the way to {to:?}",
                    b.name,
                    process_state(*pid)
                );
            }
        }
    }

    #[test]
    fn navigating_off_the_grid_shuts_the_streams_down_through_the_real_app() {
        // The seam above is only worth anything if `navigate` calls it, and
        // `navigate` is the one place `App.view` is ever assigned — which is
        // what makes it cover the two routes that never touch `go_back`.
        //
        // `Wiki` as the destination because `navigate`'s own match has no arm
        // for it, so this test changes nothing about the tmux session it is
        // running inside.
        let barns = [named_barn("guided")];
        let log = RefCell::new(Vec::new());
        let (streams, _guards) = streaming(&barns, &log);
        let pid = child_pid(&streams, "guided").expect("streaming");

        let mut app = App::new();
        app.remote_grid = streams;
        app.view = AppView::SessionGrid;

        app.navigate(AppView::Wiki { project: fixture(r#"{"name":"proj","path":"/tmp/proj"}"#) });

        assert!(child_pid(&app.remote_grid, "guided").is_none(), "the stream survived navigate");
        assert_eq!(process_state(pid), None, "the child survived navigate");
    }

    #[test]
    fn moving_around_inside_the_grid_leaves_the_streams_alone() {
        // Guards the shape of the condition. "Shut down whenever the view
        // changes" and "shut down unless we are arriving at the grid" both
        // pass the test above and both tear the streams down under the user
        // while the grid is still on screen.
        let barns = [named_barn("guided")];
        let log = RefCell::new(Vec::new());
        let (mut streams, _guards) = streaming(&barns, &log);
        let pid = child_pid(&streams, "guided").expect("streaming");

        sync_streams_for_view(&mut streams, &AppView::SessionGrid, &AppView::SessionGrid);
        assert_eq!(child_pid(&streams, "guided"), Some(pid), "a stream was replaced mid-grid");

        // And arriving at the grid must not clear what is already running.
        sync_streams_for_view(&mut streams, &AppView::Global, &AppView::SessionGrid);
        assert_eq!(child_pid(&streams, "guided"), Some(pid), "opening the grid killed its streams");

        streams.shutdown();
    }

    #[test]
    fn quitting_shuts_streams_down_before_killing_barn_sessions() {
        // Reversed, the streams race the very sessions they read through.
        let barns = [named_barn("guided")];
        let log = RefCell::new(Vec::new());
        let (mut streams, _guards) = streaming(&barns, &log);
        let pid = child_pid(&streams, "guided").expect("streaming");
        assert!(
            process_state(pid).is_some(),
            "control: the child is alive right up to the quit"
        );

        let order = RefCell::new(Vec::new());
        quit_teardown(
            &mut streams,
            || {
                // The whole test. By the time the barn sessions go, nothing is
                // reading through one.
                assert_eq!(
                    process_state(pid),
                    None,
                    "a barn session was killed while a stream was still reading through it"
                );
                order.borrow_mut().push("barn sessions");
            },
            || order.borrow_mut().push("yeehaw session"),
        );

        assert_eq!(
            *order.borrow(),
            ["barn sessions", "yeehaw session"],
            "yeehaw's own session has to go last — it takes this process with it"
        );
        assert!(child_pid(&streams, "guided").is_none(), "the quit left a stream behind");
    }

    #[test]
    fn no_streams_are_spawned_while_the_grid_is_closed() {
        // The entire cost argument. A stream is a live ssh channel per barn, so
        // a closed dashboard that keeps ticking must not open one — and the log
        // records the *attempt*, because "did not spawn" and "spawned and threw
        // it away" are indistinguishable from the outside.
        let barns = [named_barn("guided"), named_barn("smash-mac")];
        let connected = sessions_for(&["guided", "smash-mac"]);

        for view in views_off_the_grid() {
            let log = RefCell::new(Vec::new());
            let mut streams = RemoteStreams::new();
            for _tick in 0..8 {
                tick_remote_streams_with(
                    &mut streams,
                    &view,
                    &barns,
                    &connected,
                    recording_spawner(&log, silent),
                );
            }
            let _guards = guard_all(&streams);
            assert!(
                log.borrow().is_empty(),
                "two seconds of ticks on {view:?} opened {:?}",
                log.borrow()
            );
            streams.shutdown();
        }

        // Control: the same barns and the same ticks on the grid itself. Without
        // it, a tick that did nothing anywhere would pass.
        let log = RefCell::new(Vec::new());
        let mut streams = RemoteStreams::new();
        tick_remote_streams_with(
            &mut streams,
            &AppView::SessionGrid,
            &barns,
            &connected,
            recording_spawner(&log, silent),
        );
        let _guards = guard_all(&streams);
        assert_eq!(log.borrow().len(), 2, "the grid itself spawned nothing");
        streams.shutdown();
    }

    #[test]
    fn opening_the_grid_reconciles_without_waiting_for_the_first_tick() {
        // Otherwise every remote cell is a quarter second late for nothing, and
        // the slow part — ssh connecting and `bash -l` sourcing a profile — has
        // not even begun.
        //
        // The barn has no host, so the real `RemoteStream::spawn` cannot build
        // an ssh argv and records the failure instead of starting anything.
        // That runs the real `open_session_grid`, real spawn included, with
        // nothing on the network and no child anywhere.
        let mut app = App::new();
        app.windows = vec![];
        app.barns = vec![Barn { host: None, ..named_barn("ghost") }];
        app.connected_barns = sessions_for(&["ghost"]);

        app.open_session_grid(GridScope::All);

        assert!(matches!(app.view, AppView::SessionGrid), "the grid did not open");
        assert_eq!(
            failed_barns(&app.remote_grid),
            ["ghost"],
            "opening the grid never reconciled, so every remote cell waits a tick it need not"
        );
    }

    #[test]
    fn a_barn_connected_while_the_grid_is_open_gains_a_stream_without_reopening() {
        // Why reconcile is on the tick and not only on open. `connect_to_barn`
        // from another window is a session appearing in `connected_barns` on a
        // later tick, with nothing reopening the grid.
        let barns = [named_barn("guided"), named_barn("smash-mac")];
        let log = RefCell::new(Vec::new());
        let mut streams = RemoteStreams::new();

        tick_remote_streams_with(
            &mut streams,
            &AppView::SessionGrid,
            &barns,
            &sessions_for(&["guided"]),
            recording_spawner(&log, silent),
        );
        let mut guards = guard_all(&streams);
        assert_eq!(*log.borrow(), ["guided"], "control: one barn connected, one stream");

        tick_remote_streams_with(
            &mut streams,
            &AppView::SessionGrid,
            &barns,
            &sessions_for(&["guided", "smash-mac"]),
            recording_spawner(&log, silent),
        );
        guards.extend(guard_all(&streams));

        assert_eq!(
            *log.borrow(),
            ["guided", "smash-mac"],
            "a barn connected from elsewhere never reached the open grid"
        );
        assert!(child_pid(&streams, "guided").is_some(), "the first stream was disturbed");

        streams.shutdown();
    }

    // === the jump ==========================================================
    //
    // Both halves of a jump leave the process — one ssh exec and one tmux
    // session — so both are injected, exactly as `quit_teardown`'s are. Nothing
    // here spawns anything: `tmux::switch_to_window` against this machine would
    // move the window of the yeehaw session the developer is sitting in.

    /// Run a jump with every effect recorded instead of performed, and hand
    /// back what it did, in order.
    fn jump_log(
        app: &mut App,
        origin: Origin,
        window_index: u32,
        select: Result<()>,
        connect: Result<()>,
    ) -> Vec<String> {
        let log = RefCell::new(Vec::new());
        jump_to_cell(
            app,
            origin,
            window_index,
            |i| log.borrow_mut().push(format!("switch {i}")),
            |b, i| {
                log.borrow_mut().push(format!("select {} {i}", b.name));
                select
            },
            |b| {
                log.borrow_mut().push(format!("connect {}", b.name));
                connect
            },
        );
        log.into_inner()
    }

    /// An app whose ranch holds exactly `names`, and nothing else that matters.
    fn ranch(names: &[&str]) -> App {
        let mut app = App::new();
        app.barns = names.iter().map(|n| barn(n)).collect();
        app.error = None;
        app
    }

    #[test]
    fn a_jump_to_a_barn_selects_on_the_barn_before_switching_locally() {
        // The order *is* the design. Switching first attaches to whatever window
        // the barn had selected and corrects it an ssh round trip later, so the
        // user watches the wrong session for 70-180ms every single jump.
        let mut app = ranch(&["guided"]);
        let log = jump_log(&mut app, Origin::Barn("guided".into()), 4, Ok(()), Ok(()));

        assert_eq!(log, ["select guided 4", "connect guided"]);
        assert_eq!(app.error, None);
    }

    #[test]
    fn a_local_jump_switches_locally_and_never_reaches_a_barn() {
        // The unchanged path, asserted with a barn on the ranch so a jump that
        // wandered into the remote branch has somewhere to wander to.
        let mut app = ranch(&["guided"]);
        let log = jump_log(&mut app, Origin::Local, 7, Ok(()), Ok(()));

        assert_eq!(log, ["switch 7"], "a local jump went over the network");
        assert_eq!(app.error, None);
    }

    #[test]
    fn a_jump_to_a_barn_that_vanished_reports_an_error_rather_than_connecting() {
        // A frame outlives the config entry it came from: delete a barn while
        // its cells are on screen and the numbers stay drawn. Connecting to
        // *something* here means the number under the user's finger silently
        // became a different host.
        let mut app = ranch(&["guided"]);
        let log = jump_log(&mut app, Origin::Barn("ghost".into()), 4, Ok(()), Ok(()));

        assert!(log.is_empty(), "a vanished barn still reached the world: {log:?}");
        let msg = app.error.expect("a jump to a barn that is gone must say so");
        assert!(msg.contains("ghost"), "the error does not name the barn: {msg}");
        assert!(
            !msg.contains("guided"),
            "the error names a barn the user did not press: {msg}"
        );
    }

    #[test]
    fn a_failed_remote_select_reports_the_error_and_does_not_connect() {
        // Connecting anyway lands the user on whatever the barn had selected —
        // the flash of the wrong session this ordering exists to prevent, made
        // permanent and silent.
        let mut app = ranch(&["guided"]);
        let log = jump_log(
            &mut app,
            Origin::Barn("guided".into()),
            4,
            Err(anyhow::anyhow!("can't find window: 4")),
            Ok(()),
        );

        assert_eq!(log, ["select guided 4"], "it connected anyway: {log:?}");
        let msg = app.error.expect("a failed remote select must say so");
        assert!(msg.contains("can't find window: 4"), "{msg}");
    }

    #[test]
    fn a_failed_connect_after_a_good_select_surfaces_the_whole_context_chain() {
        // `{:#}`, the same as `connect_barn`. Plain Display prints only the
        // outermost layer, and the layer that says what actually went wrong is
        // underneath it — this banner is the only place the user ever sees it.
        let mut app = ranch(&["guided"]);
        let cause = anyhow::anyhow!("permission denied")
            .context("failed to write ~/.yeehaw/tmux.conf");
        let log = jump_log(&mut app, Origin::Barn("guided".into()), 4, Ok(()), Err(cause));

        assert_eq!(log, ["select guided 4", "connect guided"]);
        let msg = app.error.expect("a failed connect must say so");
        assert!(msg.contains("permission denied"), "the cause was swallowed: {msg}");
    }

    #[test]
    fn the_grid_is_drawn_with_the_registry_s_stale_barns_not_an_empty_set() {
        // The one seam between the registry that knows a stream died and the
        // view that draws it. Everything either side of this call is covered —
        // `stale()` by the registry's tests, the badge and the header note by
        // the view's — and an `&HashSet::new()` here would leave both sets of
        // tests green while the running TUI never dimmed a single cell.
        let mut app = ranch(&["guided"]);
        app.view = AppView::SessionGrid;
        mark_failed(&mut app.remote_grid, "guided", "the stream to 'guided' ended");

        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40))
            .expect("test terminal");
        terminal
            .draw(|f| {
                let area = f.area();
                render_view(f, &mut app, area);
            })
            .expect("draw");

        let buf = terminal.backend().buffer().clone();
        let header: String = (0..120u16)
            .map(|x| buf.cell((x, 0)).map(|c| c.symbol().to_string()).unwrap_or_default())
            .collect();
        assert!(
            header.contains("stale: guided"),
            "the view was drawn as if every barn were live: {header:?}"
        );
    }

    #[test]
    fn a_jump_to_a_stale_barn_skips_the_blocking_remote_select() {
        // The whole reason a stale cell keeps its number, and the one thing that
        // makes keeping it affordable. `select_remote` is a blocking ssh exec:
        // ~70-180ms over a warm ControlMaster, but ssh's full ConnectTimeout —
        // ten seconds, TUI frozen, no spinner, no escape — when the master is
        // gone. A barn is marked stale precisely because its channel died, so
        // this is the one cell on the grid where that cost is not the exception.
        //
        // Skipping it lands the user in the barn's own session instead, which is
        // local tmux work and returns at once: live if the barn recovered, on
        // `yeehaw connect`'s "unreachable" screen if it did not. That screen is
        // where a dead barn is *supposed* to be reported, and it can retry.
        let mut app = ranch(&["guided"]);
        mark_failed(&mut app.remote_grid, "guided", "the stream to 'guided' ended");

        let log = jump_log(&mut app, Origin::Barn("guided".into()), 4, Ok(()), Ok(()));

        assert_eq!(
            log,
            ["connect guided"],
            "a jump to a stale barn went over the network anyway: {log:?}"
        );
        assert_eq!(app.error, None, "a stale jump reported a failure that did not happen");
    }

    #[test]
    fn a_stale_jump_is_not_the_silent_no_op_it_would_be_easiest_to_ship() {
        // The alternative — ignore the key — leaves a cell wearing a number that
        // does nothing, which is the failure that pulled the remote jump forward
        // a task. A stale number still takes you to that barn; only the remote
        // select is dropped.
        let mut app = ranch(&["guided"]);
        mark_failed(&mut app.remote_grid, "guided", "the stream to 'guided' ended");

        let log = jump_log(&mut app, Origin::Barn("guided".into()), 4, Ok(()), Ok(()));
        assert!(!log.is_empty(), "the number on a stale cell did nothing at all");

        // And a failure to reach the barn is still reported, rather than the
        // jump quietly deciding a stale barn cannot fail.
        let mut app = ranch(&["guided"]);
        mark_failed(&mut app.remote_grid, "guided", "the stream to 'guided' ended");
        let log = jump_log(
            &mut app,
            Origin::Barn("guided".into()),
            4,
            Ok(()),
            Err(anyhow::anyhow!("no route to host")),
        );
        assert_eq!(log, ["connect guided"]);
        let msg = app.error.expect("a failed connect must still say so");
        assert!(msg.contains("no route to host"), "{msg}");
    }

    #[test]
    fn only_the_stale_barn_loses_its_remote_select() {
        // Staleness is per barn, like everything else about the merge. One dead
        // barn must not cost the healthy one beside it the window it was aimed
        // at.
        let mut app = ranch(&["guided", "smash-mac"]);
        mark_failed(&mut app.remote_grid, "guided", "the stream to 'guided' ended");

        let log = jump_log(&mut app, Origin::Barn("smash-mac".into()), 2, Ok(()), Ok(()));
        assert_eq!(
            log,
            ["select smash-mac 2", "connect smash-mac"],
            "a live barn lost its remote select to its neighbour dying: {log:?}"
        );
    }

    #[test]
    fn a_local_jump_is_untouched_by_a_stale_barn() {
        let mut app = ranch(&["guided"]);
        mark_failed(&mut app.remote_grid, "guided", "the stream to 'guided' ended");

        let log = jump_log(&mut app, Origin::Local, 7, Ok(()), Ok(()));
        assert_eq!(log, ["switch 7"]);
    }

    #[test]
    fn a_jump_finds_its_barn_by_exact_name_not_by_prefix() {
        // `guided` and `guided-2` are different production hosts, and the same
        // hazard the `=` in every tmux target guards. A `starts_with` here sends
        // the user to whichever one the config listed first.
        let mut app = ranch(&["guided-2", "guided"]);
        let log = jump_log(&mut app, Origin::Barn("guided".into()), 1, Ok(()), Ok(()));

        assert_eq!(log, ["select guided 1", "connect guided"]);
    }

    // === help on the grid ==================================================

    /// Draw the whole app — overlay, bottom bar and all — and read the screen
    /// back as one string.
    fn whole_screen(app: &mut App, w: u16, h: u16) -> String {
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h))
            .expect("test terminal");
        terminal.draw(|f| draw(f, app)).expect("draw");
        let buf = terminal.backend().buffer().clone();
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| {
                        buf.cell((x, y))
                            .map(|c| c.symbol().to_string())
                            .unwrap_or_default()
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn help_on_the_grid_is_the_grids_own_help_and_not_the_generic_one() {
        // `?` opens the overlay from the grid — it is not an input-mode view —
        // and the grid used to fall through this match to "general", which lists
        // `Esc` under a navigation block none of whose keys the grid answers.
        //
        // Driven through `draw` rather than by calling the scope match directly,
        // because a scope name is worth nothing until it reaches the overlay:
        // "vault" is already mapped here and has no section on the other side.
        let mut app = ranch(&["guided"]);
        app.view = AppView::SessionGrid;
        app.show_help = true;

        let screen = whole_screen(&mut app, 120, 40);
        assert!(
            screen.contains("Local sessions only"),
            "`l` is bound on the grid and the grid's help never mentions it:\n{screen}"
        );
        assert!(
            screen.contains("Jump to that session"),
            "the grid's help is not the grid's:\n{screen}"
        );
        assert!(
            !screen.contains("Go to bottom"),
            "the grid was handed the generic navigation block, whose keys it ignores:\n{screen}"
        );
    }

    #[test]
    fn the_grids_bottom_bar_names_the_source_filter_and_the_help_key() {
        // The other place a key goes to be discovered, and the one a user sees
        // without pressing anything. `l` and `?` were both live on the grid and
        // named in neither this bar nor the overlay.
        let mut app = ranch(&[]);
        app.view = AppView::SessionGrid;

        let screen = whole_screen(&mut app, 120, 40);
        let bar = screen.lines().last().unwrap_or_default().to_string();
        assert!(bar.contains("local"), "the bottom bar hides `l`: {bar:?}");
        assert!(bar.contains("help"), "the bottom bar hides `?`: {bar:?}");
        assert!(bar.contains("jump"), "the bottom bar lost `1-9`: {bar:?}");
    }
}
