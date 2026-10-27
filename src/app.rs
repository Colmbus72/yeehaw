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
use crate::views::sync_view::{SyncAction, SyncView};
use crate::views::issues_view::{IssuesView, IssuesAction};
use crate::views::ranchhand_detail::{RanchHandDetailView, RanchHandAction};
use crate::views::trail_view::{TrailView, TrailViewAction};
use crate::views::vault_view::{VaultView, VaultAction, VaultMode};
use crate::vault::crypto;

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
    /// The Ctrl-S sync view. Built fresh on each open, so its fetches are
    /// against whatever is connected at that moment.
    pub sync_view: Option<SyncView>,
    pub issues_view: IssuesView,
    pub ranchhand_view: RanchHandDetailView,
    pub vault_view: VaultView,

    // Trail view. Runs themselves live in their own tmux windows (`jobs`);
    // the view follows them through the records they write.
    pub trail_view: TrailView,

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
        let worms = config::load_worms();
        let windows = tmux::list_yeehaw_windows();
        let connected_barns = tmux::connected_barn_sessions(&tmux::list_session_names());
        let mut barns = config::load_barns();
        order_barns_for_display(&mut barns, &connected_barns, config::this_barn_name().as_deref());

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
            sync_view: None,
            issues_view: IssuesView::new(),
            ranchhand_view: RanchHandDetailView::new(),
            vault_view: VaultView::new(),
            trail_view: TrailView::new(),
            pending_editor: None,
            claude_splash: None,
            claude_splash_window: None,
            claude_splash_prompt: None,
            claude_splash_tools: None,
            claude_splash_tick: None,
        }
    }

    pub fn reload(&mut self) {
        self.projects = config::load_projects();
        self.worms = config::load_worms();
        self.windows = tmux::list_yeehaw_windows();
        self.replace_barns(config::load_barns());
    }

    pub fn refresh_windows(&mut self) {
        self.windows = tmux::list_yeehaw_windows();
        // Hoisted here rather than into the render path: this runs on the 250ms
        // idle tick, render runs every frame and must stay free of subprocesses.
        let connected = tmux::connected_barn_sessions(&tmux::list_session_names());
        self.set_connected_barns(connected);
    }

    /// Take a new connected set, re-sorting the barns only when it changed —
    /// the sort reads `config.yaml` for this machine's name, and this runs four
    /// times a second.
    pub fn set_connected_barns(&mut self, connected: HashSet<String>) {
        if connected != self.connected_barns {
            self.connected_barns = connected;
            self.reorder_barns();
        }
    }

    /// Put `self.barns` in display order, keeping the barns panel's cursor on
    /// the barn it was on.
    ///
    /// `self.barns` *is* the display order: every barns-panel action carries an
    /// index into it (`s`, `c`, `Enter`, `d`, `C-d`), so sorting anywhere else —
    /// in the panel's render, say — would have those keys act on a different
    /// barn than the one under the cursor. And the cursor follows its barn,
    /// because a barn that connects moves up the list while the user is inside
    /// it, and the row they come back to should still be that barn.
    fn reorder_barns(&mut self) {
        let name = self.selected_barn_name();
        self.sort_barns_keeping(name);
    }

    /// Install `barns` (in any order) as `self.barns`, in display order, with
    /// the barns panel's cursor on the barn it was on before.
    ///
    /// The selected barn's *name* is read from the list being replaced, before
    /// it is replaced: read after, the cursor's index points into the new,
    /// unsorted list, and the cursor lands on whichever barn happens to be
    /// there.
    fn replace_barns(&mut self, barns: Vec<Barn>) {
        let name = self.selected_barn_name();
        self.barns = barns;
        self.sort_barns_keeping(name);
    }

    fn selected_barn_name(&self) -> Option<String> {
        self.barns.get(self.global_dashboard.barn_cursor(&self.barns)).map(|b| b.name.clone())
    }

    /// Sort `self.barns` for display and put the cursor back on `name`.
    fn sort_barns_keeping(&mut self, name: Option<String>) {
        order_barns_for_display(
            &mut self.barns,
            &self.connected_barns,
            config::this_barn_name().as_deref(),
        );
        if let Some(index) = name.and_then(|n| self.barns.iter().position(|b| b.name == n)) {
            self.global_dashboard.set_barn_cursor(index, &self.barns);
        }
    }

    /// Open the live session grid, scoped to wherever `v` was pressed.
    /// Ctrl-S: diff this machine against every barn it is connected to.
    pub fn open_sync_view(&mut self) {
        let barns: Vec<Barn> = self
            .barns
            .iter()
            .filter(|b| self.connected_barns.contains(&tmux::barn_session_name(&b.name)))
            .cloned()
            .collect();
        self.sync_view = Some(SyncView::new(barns));
        self.navigate(AppView::Sync);
    }

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
        tick_remote_streams(&mut self.remote_grid, &self.view, &self.barns, &self.connected_barns);
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
            AppView::Project { .. } | AppView::Barn { .. } | AppView::Worm { .. } | AppView::Sync => {
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
/// Two views want them, and every other one is still off — that is the cost
/// argument for the feature: a stream is a live ssh channel to a barn, emitting
/// a rendered frame every second. A view parked somewhere else with streams
/// open is paying that for sessions nobody is looking at.
///
/// `Global` is the second one because the dashboard's sessions panel is a list
/// of the *whole ranch*, not of this machine — each connected barn's windows
/// sit under a heading of their own, and those rows come out of the same frames
/// the grid draws its cells from. The cost stays proportional to what the user
/// asked for: a barn streams only while it is connected, and a ranch with
/// nothing connected opens no channel at all.
///
/// Written as an exhaustive `match` rather than `matches!`, deliberately. A new
/// `AppView` variant then fails to compile here instead of silently inheriting
/// whichever answer a `_` arm happened to give.
pub(crate) fn streams_wanted(view: &AppView) -> bool {
    match view {
        AppView::SessionGrid | AppView::Global => true,
        AppView::Sync
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
/// the dashboard brings its sessions in without reopening the grid, and
/// disconnecting drops them. It is idempotent — a live stream is left alone —
/// which it has to be at four ticks a second.
///
/// Being in `connected` means a `yh-barn-*` tmux session exists, never that ssh
/// works: `tmux::connect_to_barn` creates that session before any ssh succeeds
/// and `connect::run` renders "unreachable" without exiting, so a barn parked
/// on its own error screen is in the set looking exactly like a healthy one.
/// The registry's backoff is what makes that survivable; nothing here may work
/// around it.
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
    streams.reconcile_with(&connected_barns_of(barns, connected), spawn);
    streams.drain();
}

/// The barns with a local connect session, in `barns` order.
///
/// `connected` holds tmux **session** names, straight out of `list-sessions`.
/// Barn names are not session names — barn `camera pi` lives in session
/// `yh-barn-camera-pi-<hash>` — so the mapping runs forwards through
/// [`tmux::barn_session_name`], the direction that is not lossy. Never
/// [`tmux::barn_session_target`]: its `=` prefix exists for `-t` arguments and
/// appears in nothing `list-sessions` prints. Both mistakes fail the same way —
/// nothing matches, no stream is ever spawned, and the panel simply stays empty.
fn connected_barns_of(barns: &[Barn], connected: &HashSet<String>) -> Vec<Barn> {
    barns
        .iter()
        .filter(|b| is_connected(b, connected))
        .cloned()
        .collect()
}

fn is_connected(barn: &Barn, connected: &HashSet<String>) -> bool {
    connected.contains(&tmux::barn_session_name(&barn.name))
}

/// The barns panel's order: this machine's own row, then the connected barns,
/// then everything else — by name, ignoring case, within each.
///
/// A connected barn is one the user is working in right now, with its
/// sessions keyed `A`-`Z` in the sessions panel, so its row is one they keep
/// coming back to. This needs live tmux state, which is why it is here rather
/// than in `config::load_barns_checked` (which does the this-machine half on
/// its own, for every other caller).
fn order_barns_for_display(barns: &mut [Barn], connected: &HashSet<String>, this_barn: Option<&str>) {
    barns.sort_by_key(|b| {
        let rank = if config::is_local_barn(b) || this_barn == Some(b.name.as_str()) {
            0
        } else if is_connected(b, connected) {
            1
        } else {
            2
        };
        (rank, b.name.to_lowercase())
    });
}

/// The quit teardown, in the one order that is safe.
///
/// It cannot be left to `Drop`: `kill_yeehaw_session` ends the session this
/// process runs in without unwinding, so nothing's destructor is guaranteed to
/// run.
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
                        if config::save_worm(&mut worm).is_ok() {
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
                    || (matches!(app.view, AppView::Barn { .. }) && (app.barn_view.is_editing() || app.barn_view.is_filter_typing()))
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
                        if !in_input_mode
                            && key.modifiers.contains(KeyModifiers::CONTROL)
                            && key.code == KeyCode::Char('s')
                        {
                            app.open_sync_view();
                            continue;
                        }
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
                                .focused_barn_index(&app.barns)
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
                                    app.set_connected_barns(tmux::connected_barn_sessions(
                                        &tmux::list_session_names(),
                                    ));
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
                        // A `/` filter owns `Esc` until it is cleared.
                        if !app.project_view.is_input_mode() {
                            match key.code {
                                KeyCode::Esc if !app.project_view.has_filter() => { app.go_back(); continue; }
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
                        if !app.barn_view.is_editing() && !app.barn_view.is_filter_typing() {
                            match key.code {
                                KeyCode::Esc if !app.barn_view.has_filter() => { app.go_back(); continue; }
                                KeyCode::Char('v') => {
                                    if let AppView::Barn { ref barn } = app.view {
                                        let scope = GridScope::for_barn(&barn.name);
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
                    AppView::Sync => {
                        let action = match app.sync_view.as_mut() {
                            Some(view) => view.handle_key(key),
                            None => SyncAction::Back,
                        };
                        if let SyncAction::Back = action {
                            app.sync_view = None;
                            app.go_back();
                            // A pull may have written entities; show them.
                            app.reload();
                            continue;
                        }
                    }
                    AppView::Issues { ref project } => {
                        let project = project.clone();
                        match app.issues_view.handle_input(key.code, &project) {
                            IssuesAction::Back => { app.go_back(); continue; }
                            IssuesAction::OpenClaude(ctx) => open_issue_claude(&mut app, &project, ctx),
                            IssuesAction::ProjectUpdated(updated) => {
                                apply_issues_project_update(&mut app, updated);
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
            tick_remote_streams(&mut app.remote_grid, &app.view, &app.barns, &app.connected_barns);
            if matches!(app.view, AppView::SessionGrid) {
                let windows = app.windows.clone();
                app.session_grid_view.tick(&windows);
            }
            if let Some(view) = app.sync_view.as_mut() {
                view.tick();
            }

            // Vault idle timeout
            if matches!(app.view, AppView::Vault { .. }) && app.vault_view.is_idle_expired() {
                app.vault_view.enter_locked();
                app.go_back();
            }

            // Follow trail runs through their records on disk.
            if let AppView::Trail { ref livestock, ref trail, .. } = app.view {
                let (livestock, trail) = (livestock.clone(), trail.clone());
                app.trail_view.sync(&trail, &livestock);
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
    // The frames and the stale set go in for the same reason they go into the
    // grid's handler: the sessions panel lists every connected barn's
    // windows, so a capital-letter key has to resolve against the exact list,
    // in the exact order, that `render` drew.
    let action = {
        let stale = app.remote_grid.stale();
        app.global_dashboard.handle_input(
            key,
            &app.projects,
            &app.barns,
            &app.worms,
            &app.windows,
            app.remote_grid.frames(),
            &stale,
        )
    };
    apply_dashboard_action(app, action);
}

/// The dashboard's side of the loop, split from the keystroke that produced
/// the action so the effects — which write to the ranch — can be driven
/// directly from a test.
/// The directory a process scoped to `project` starts in.
///
/// A project pulled from another machine arrives with `path: ""` (the
/// sender's checkout path means nothing here), and an empty working directory
/// would start the process wherever yeehaw itself happens to be running.
fn project_working_dir(project: &Project) -> Result<String, String> {
    if project.path.trim().is_empty() {
        return Err(format!(
            "project '{}' has no path on this machine — set it with e (edit) first",
            project.name
        ));
    }
    Ok(expand_path(&project.path))
}

/// Issues view `Enter`: a Claude session in the project, primed with an issue.
fn open_issue_claude(app: &mut App, project: &Project, ctx: String) {
    let working_dir = match project_working_dir(project) {
        Ok(dir) => dir,
        Err(e) => {
            app.error = Some(e);
            return;
        }
    };
    let window_name = format!("{}-issue-claude", project.name);
    match tmux::create_claude_window_with_context(&working_dir, &window_name, &ctx) {
        Ok(idx) => {
            tmux::set_window_scope(idx, &project.name, None);
            let tools: Vec<String> = tmux::YEEHAW_MCP_TOOLS.iter()
                .map(|t| t.strip_prefix("mcp__yeehaw__").unwrap_or(t).to_string())
                .collect();
            app.show_claude_splash(idx, ctx, tools);
        }
        Err(e) => { app.error = Some(e.to_string()); }
    }
    app.refresh_windows();
}

fn apply_dashboard_action(app: &mut App, action: DashboardAction) {
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
        DashboardAction::SelectWindow(window_index) => {
            tmux::switch_to_window(window_index);
        }
        DashboardAction::JumpToBarnSession { barn, window_index } => {
            // `jump_to_cell`, the same route a letter on the grid takes: it is
            // the one place that knows a barn deleted out from under a
            // still-drawn row must be reported rather than half-followed, and
            // which barns get the remote `select-window` first.
            jump_to_cell(
                app,
                Origin::Barn(barn),
                window_index,
                tmux::switch_to_window,
                select_remote_window,
                tmux::connect_to_barn,
            );
        }
        DashboardAction::NewClaude(project_idx) => {
            if let Some(project) = app.projects.get(project_idx) {
                let working_dir = match project_working_dir(project) {
                    Ok(dir) => dir,
                    Err(e) => {
                        app.error = Some(e);
                        return;
                    }
                };
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
                // "Is this the machine I am running on?" — a plain shell here,
                // or ssh to somewhere else. After adoption that machine's row is
                // its real record, which has no host to dial, so asking only
                // about the synthetic `local` turned `s` on the user's own row
                // into a failed ssh.
                if config::barn_is_this_machine(barn) {
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
            let mut project = Project {
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
                id: None,
                created_at: None,
                updated_at: None,
            };
            // `create_*`, not `save_*`: a plain save writes over whatever is
            // at that name, and this struct's empty livestock/herds/wiki plus
            // a fresh uuid would replace the existing entity outright.
            match config::create_project(&mut project) {
                Ok(()) => {
                    app.reload();
                    app.project_view = ProjectContextView::new();
                    app.navigate(AppView::Project { project });
                }
                Err(e) => { app.error = Some(format!("Failed to create project: {}", e)); }
            }
        }
        DashboardAction::CreateBarn(name, host, user, port, identity_file) => {
            let mut barn = Barn {
                name,
                host: Some(host),
                user: Some(user),
                port: Some(port),
                identity_file,
                critters: vec![],
                ..Default::default()
            };
            match config::create_barn(&mut barn) {
                Ok(()) => {
                    app.reload();
                    app.barn_view = BarnContextView::new();
                    app.navigate(AppView::Barn { barn });
                }
                Err(e) => { app.error = Some(format!("Failed to create barn: {}", e)); }
            }
        }
        DashboardAction::CreateWorm(name, command, schedule) => {
            let mut worm = Worm {
                name,
                command,
                schedule,
                worm_type: "shell".to_string(),
                enabled: true,
                project: None,
                working_dir: None,
                id: None,
                created_at: None,
                updated_at: None,
            };
            match config::create_worm(&mut worm) {
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
                // "Is this the machine I am running on?" — it is the one row
                // that is never a barn the user can drop. The synthetic `local`
                // has no file to delete (`config::delete_barn` refuses the name
                // outright); the adopted self-barn does, and deleting it is
                // worse than a no-op — it strands `config.this_barn` on a
                // record that no longer exists.
                if !config::barn_is_this_machine(barn) {
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
        let action = app.project_view.handle_input(key, &project, &app.barns, &app.windows);
        apply_project_action(app, project, action);
    }
}

/// Replaces the Issues view's project snapshot with the copy the view just
/// saved.
///
/// The view is handed a `&Project`, so it has to clone it to save it, and
/// `save_project` stamps the clone. Without this the view keeps the pre-save
/// snapshot — `id: None` on a project being stamped for the first time — and
/// `go_back` carries that stale copy to the project view, where the next save
/// mints a *second* uuid over the first.
fn apply_issues_project_update(app: &mut App, updated: Project) {
    app.reload();
    let refreshed = app
        .projects
        .iter()
        .find(|p| p.name == updated.name)
        .cloned()
        .unwrap_or(updated);
    app.view = AppView::Issues { project: refreshed };
}

/// The barn a livestock's work runs on, or `None` when that is this machine.
///
/// The single lookup behind a shell, a log tail, a trail run and a worm-driven
/// trail, so all four answer adoption the same way. `None` is what every one of
/// them turns into the local path — an unpinned livestock has always produced
/// it — which is exactly why the resolved form has to keep producing it once
/// `adopt_this_machine` has rewritten `barn: None` to this machine's real name.
///
/// `source_barn` — the barn the user reached the livestock *through* — still
/// wins, because that is how a livestock opened from a barn's own list gets
/// ssh'd into. The one exception is this machine: adoption makes the self-barn
/// a browsable record with no host, and reaching a local livestock through it
/// must not turn a local shell into an ssh to nowhere.
fn barn_for_livestock(
    livestock: &Livestock,
    barns: &[Barn],
    source_barn: Option<&Barn>,
) -> Option<Barn> {
    source_barn
        .filter(|b| !config::barn_is_this_machine(b))
        .or_else(|| {
            config::resolve_livestock_barn(livestock)
                .and_then(|name| barns.iter().find(|b| b.name == name))
        })
        .cloned()
}

/// The project view's side of the loop, split from the keystroke that produced
/// the action so the effects — which write to the ranch — can be driven
/// directly from a test.
///
/// `project` is the snapshot the view was editing, so its name is the name the
/// file still carries on disk. `UpdateProject` needs both to move a rename.
fn apply_project_action(app: &mut App, project: Project, action: ProjectAction) {
    {
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
            ProjectAction::SelectWindow(window_index) => {
                tmux::switch_to_window(window_index);
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
                if let Some(ls) = project.livestock.get(ls_idx).filter(|ls| config::livestock_is_on_this_machine(ls)) {
                    let working_dir = expand_path(&ls.path);
                    let window_name = format!("{}-{}-claude", project.name, ls.name);
                    let ctx = context::build_livestock_context(&project, &ls.name);
                    match tmux::create_claude_window_with_context(&working_dir, &window_name, &ctx) {
                        Ok(idx) => {
                            tmux::set_window_scope(idx, &project.name, config::resolve_livestock_barn(ls));
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
                    let barn = barn_for_livestock(ls, &app.barns, None);
                    let window_name = format!("{}-{}", project.name, ls.name);
                    if let Some(barn) = barn {
                        // "Is this the machine I am running on?" — the same
                        // question `barn_for_livestock` already answered on the
                        // way in, and it has to be answered the same way here or
                        // the two disagree the moment the machine is adopted.
                        if !config::barn_is_this_machine(&barn) {
                            match tmux::create_ssh_window(&window_name, &barn, &ls.path) {
                                Ok(idx) => {
                                    tmux::set_window_scope(idx, &project.name, config::resolve_livestock_barn(ls));
                                    tmux::switch_to_window(idx);
                                }
                                Err(e) => app.error = Some(format!("SSH failed: {}", e)),
                            }
                            return;
                        }
                    }
                    let working_dir = expand_path(&ls.path);
                    if let Ok(idx) = tmux::create_shell_window(&working_dir, &window_name) {
                        tmux::set_window_scope(idx, &project.name, config::resolve_livestock_barn(ls));
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
                match config::save_project(&mut updated_project) {
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
                let mut rh = RanchHand {
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
                    id: None,
                    created_at: None,
                    updated_at: None,
                };
                match config::create_ranchhand(&mut rh) {
                    Ok(()) => { app.reload(); }
                    Err(e) => { app.error = Some(format!("Failed to create ranchhand: {}", e)); }
                }
            }
            ProjectAction::UpdateProject(mut updated) => {
                // `project` is the snapshot the view was editing, so its name
                // is the name the file still carries. `save_project` alone
                // would leave that file in place beside the renamed one, both
                // holding the same uuid.
                match config::rename_project(&project.name, &mut updated) {
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
/// view. Rejects exactly the barns `connect::run` rejects, by calling the same
/// function — the same barn must not produce two different messages depending
/// on which side noticed. `connect::run` re-checks: it is the entry point for
/// `yeehaw connect` from a shell, where the TUI never ran.
fn connect_barn(app: &mut App, barn: &Barn) {
    if let Some(why) = crate::connect::refusal(barn) {
        app.error = Some(why);
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
        let action = app.barn_view.handle_input(key, &barn, &livestock);
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
                match config::save_barn(&mut updated_barn) {
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
                // "Is this the machine I am running on?" — the barn view's copy
                // of the dashboard's `s`, and it has to answer identically.
                if config::barn_is_this_machine(&barn) {
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
            BarnAction::UpdateBarn(mut updated) => {
                match config::save_barn(&mut updated) {
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
                if config::save_worm(&mut updated).is_ok() {
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
                let barn = barn_for_livestock(&livestock, &app.barns, source_barn.as_ref())
                    .unwrap_or_else(config::local_barn);

                // The window opens in the background; the view picks the run
                // up from its record within a tick, and `Esc` leaves it running.
                match crate::jobs::launch_trail(
                    &livestock.name,
                    &trail.name,
                    Some(&project.name),
                    Some(&barn.name),
                    "manual",
                ) {
                    Ok(crate::jobs::Launch::Started) => app.trail_view.expect_run(),
                    Ok(crate::jobs::Launch::AlreadyRunning) => {
                        app.error = Some("This trail is already running".to_string());
                    }
                    Ok(crate::jobs::Launch::NoSession) => {
                        app.error = Some("Trails run in the yeehaw tmux session, which is not running".to_string());
                    }
                    Err(e) => {
                        app.error = Some(format!("Failed to start trail: {}", e));
                    }
                }
            }
            TrailViewAction::CancelTrail => {
                if let Some(run) = app.trail_view.active_run() {
                    if !crate::jobs::cancel_trail_run(run) {
                        app.error = Some("Could not reach the run to cancel it".to_string());
                    }
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
        apply_livestock_action(app, project, livestock, source, source_barn, action);
    }
}

/// The livestock view's side of the loop, split from the keystroke that
/// produced the action so the effects — which write to the ranch — can be
/// driven directly from a test.
fn apply_livestock_action(
    app: &mut App,
    project: Project,
    livestock: Livestock,
    source: String,
    source_barn: Option<Barn>,
    action: LivestockAction,
) {
    {
        match action {
            LivestockAction::None => {}
            LivestockAction::OpenLogs => {
                let barn = barn_for_livestock(&livestock, &app.barns, source_barn.as_ref());
                app.logs_view = Some(LogsView::new(&project, &livestock, barn.as_ref()));
                app.navigate(AppView::Logs {
                    project: project.clone(),
                    livestock: livestock.clone(),
                    source: source.clone(),
                    source_barn: source_barn.clone(),
                });
            }
            LivestockAction::OpenClaude if config::livestock_is_on_this_machine(&livestock) => {
                let working_dir = expand_path(&livestock.path);
                let window_name = format!("{}-{}-claude", project.name, livestock.name);
                let ctx = context::build_livestock_context(&project, &livestock.name);
                match tmux::create_claude_window_with_context(&working_dir, &window_name, &ctx) {
                    Ok(idx) => {
                        tmux::set_window_scope(idx, &project.name, config::resolve_livestock_barn(&livestock));
                        let tools: Vec<String> = tmux::YEEHAW_MCP_TOOLS.iter()
                            .map(|t| t.strip_prefix("mcp__yeehaw__").unwrap_or(t).to_string())
                            .collect();
                        app.show_claude_splash(idx, ctx.clone(), tools);
                    }
                    Err(e) => { app.error = Some(format!("Failed: {}", e)); }
                }
            }
            // Not offered for livestock on another barn; the key does nothing.
            LivestockAction::OpenClaude => {}
            LivestockAction::OpenShell => {
                let barn = barn_for_livestock(&livestock, &app.barns, source_barn.as_ref());
                let window_name = format!("{}-{}", project.name, livestock.name);
                if let Some(barn) = barn {
                    // "Is this the machine I am running on?" — and here it can
                    // genuinely be: `source_barn` is the barn the user reached
                    // this livestock *through*, so browsing into the adopted
                    // self-barn and opening a shell arrived with a real record
                    // for this machine in hand.
                    if !config::barn_is_this_machine(&barn) {
                        match tmux::create_ssh_window(&window_name, &barn, &livestock.path) {
                            Ok(idx) => {
                                tmux::set_window_scope(idx, &project.name, config::resolve_livestock_barn(&livestock));
                                tmux::switch_to_window(idx);
                            }
                            Err(e) => app.error = Some(format!("SSH failed: {}", e)),
                        }
                        return;
                    }
                }
                let working_dir = expand_path(&livestock.path);
                if let Ok(idx) = tmux::create_shell_window(&working_dir, &window_name) {
                    tmux::set_window_scope(idx, &project.name, config::resolve_livestock_barn(&livestock));
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
            LivestockAction::SaveNewTrail(mut trail) => {
                // `create_trail`, not `save_trail`: the name comes straight
                // from a text field, and this `Trail` was built fresh with no
                // id. A plain save on a name already in use replaces that trail
                // outright and mints a new uuid over its old one.
                match config::create_trail(&mut trail) {
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
            // Stay on the grid rather than going back, so Ctrl+Y from a local
            // session lands straight back here instead of the dashboard.
            jump_to_cell(
                app,
                origin,
                window_index,
                tmux::switch_to_window,
                select_remote_window,
                tmux::connect_to_barn,
            );
        }
        GridAction::None => {}
    }
}

/// Land the user on the session behind a key.
///
/// `Local` is the `select-window` it always was.
///
/// `Barn` lands in that barn's **connect session** — the same local tmux
/// session `c` makes, holding `ssh -t <host> bash -lc yeehaw` — with the barn's
/// own tmux first pointed at the window that was pressed. Ctrl-Q comes back
/// out, as from any connect. No window onto the barn is ever opened in the
/// local session.
///
/// **The select on the barn runs first**, and only when the barn is
/// connected. Switching first would show whatever window the barn had selected
/// and correct it an ssh round trip later. Its failure is reported in the
/// banner, with tmux's own message, and the switch does not happen: landing on
/// the wrong session silently is worse than not landing. It is bounded by
/// [`remote_grid::SELECT_TIMEOUT`] so the 250 ms loop is never held for ssh's
/// `ConnectTimeout`; past the bound the switch goes ahead regardless.
///
/// **A barn with no connect session falls back to `c`.** Its sessions are
/// still drawn from the last frame for a tick after a disconnect, and the
/// barn's yeehaw may not be running at all, so there is nothing to select —
/// the connect path builds the session and the user picks from the barn's own
/// dashboard.
///
/// **A stale barn is jumped to without the select.** Its stream just died, so
/// the select is the one call most likely to sit out the whole timeout.
///
/// The effects are injected for the same reason [`quit_teardown`]'s are: one
/// reaches the network and the other moves the user's tmux client, and the
/// order between them is the property worth guarding.
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

    // A row can outlive its barn's config entry: the panel holds the last frame
    // from a barn deleted from the ranch a moment ago, still keyed. Connecting
    // to *something* rather than nothing means the key under the user's finger
    // has just sent them to a different host.
    let Some(barn) = app.barns.iter().find(|b| b.name == name).cloned() else {
        app.error = Some(format!("barn '{}' is no longer on the ranch", name));
        return;
    };

    if is_connected(&barn, &app.connected_barns) {
        // Read here rather than passed in: it is the same map the row was drawn
        // from, so the badge the user pressed and the route taken cannot
        // disagree about which barns are stale.
        if !app.remote_grid.stale().contains(name.as_str()) {
            if let Err(e) = select_remote(&barn, window_index) {
                app.error = Some(format!("Jump failed: {:#}", e));
                return;
            }
        }
    } else if let Some(why) = crate::connect::refusal(&barn) {
        app.error = Some(why);
        return;
    }
    // `{:#}`, like `connect_barn`: these are context chains and plain Display
    // shows only the outermost layer, never the one that says what went wrong.
    if let Err(e) = connect(&barn) {
        app.error = Some(format!("Connect failed: {:#}", e));
    }
}

/// [`remote_grid::select_window`], with the timeout read as "go anyway".
fn select_remote_window(barn: &Barn, window_index: u32) -> Result<()> {
    remote_grid::select_window(barn, window_index).map(|_| ())
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
    /// A window of this machine's yeehaw session, by **tmux window index** —
    /// never a position in the sessions list, which skips window 0.
    SelectWindow(u32),
    /// A window on a barn: select it in the barn's yeehaw, then switch to the
    /// barn's connect session — the same path a letter on the session grid
    /// takes.
    JumpToBarnSession { barn: String, window_index: u32 },
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
    /// Switch to a session window, by **tmux window index**.
    SelectWindow(u32),
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
            let stale = app.remote_grid.stale();
            app.global_dashboard.render(
                frame,
                area,
                &app.projects,
                &app.barns,
                &app.worms,
                &app.windows,
                &app.connected_barns,
                app.remote_grid.frames(),
                &stale,
            );
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
        AppView::Sync => {
            if let Some(view) = app.sync_view.as_ref() {
                view.render(frame, area);
            }
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
    let spans: Vec<Span> = items
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

    let bar = ratatui::widgets::Paragraph::new(Line::from(spans))
        .style(Style::default().bg(Color::Rgb(30, 30, 30)));
    frame.render_widget(bar, area);
}

fn get_bottom_bar_items(view: &AppView) -> Vec<(&'static str, &'static str)> {
    match view {
        AppView::Global => vec![
            ("v", "visualizer"),
            ("C-s", "sync"),
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

/// Run a worm now, in a background window.
fn trigger_worm(error: &mut Option<String>, worm: &Worm) {
    match crate::jobs::launch_worm(&worm.name, "manual") {
        Ok(crate::jobs::Launch::Started) | Ok(crate::jobs::Launch::AlreadyRunning) => {}
        Ok(crate::jobs::Launch::NoSession) => {
            *error = Some("Worms run in the yeehaw tmux session, which is not running".to_string());
        }
        Err(e) => *error = Some(format!("Failed to run worm: {}", e)),
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

    // Trigger files are the old transport — everything now launches its
    // window directly — but one may still be written by a binary from before
    // that (an MCP server Claude has had running for days, say). Honour it the
    // new way: a background window, never a jump to it.
    let result = if trigger_type == "poll" || trigger_type == "mcp" {
        match (
            trigger.get("livestock").and_then(|v| v.as_str()),
            trigger.get("trail").and_then(|v| v.as_str()),
        ) {
            (Some(livestock), Some(trail)) => {
                let project = trigger.get("project").and_then(|v| v.as_str());
                crate::jobs::launch_trail(livestock, trail, project, None, &trigger_type)
            }
            _ => return,
        }
    } else {
        crate::jobs::launch_worm(&worm_name, &trigger_type)
    };
    if let Err(e) = result {
        app.error = Some(format!("Failed to run '{}': {}", worm_name, e));
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
        recording_spawner, silent,
    };
    use std::cell::RefCell;

    // `App::new` loads the ranch — `ensure_config_dirs` creates directories and
    // `load_projects` reads them — so tests that build one open a temp ranch
    // (`let _ranch = ...`) instead of touching the developer's real `~/.yeehaw`.
    // The guard has to be a binding: it holds only until it drops.

    fn barn(name: &str) -> Barn {
        Barn {
            name: name.into(),
            host: Some("172.233.141.59".into()),
            user: Some("forge".into()),
            port: None,
            identity_file: None,
            critters: vec![],
            ..Default::default()
        }
    }

    /// The set is built the way the app builds it: from what `list-sessions`
    /// prints, filtered by `connected_barn_sessions`.
    fn connected(session_names: &[String]) -> HashSet<String> {
        tmux::connected_barn_sessions(session_names)
    }

    /// The connected set for these barns: their connect sessions, named as
    /// `list-sessions` prints them.
    fn sessions(barns: &[&str]) -> HashSet<String> {
        connected(&barns.iter().map(|n| tmux::barn_session_name(n)).collect::<Vec<_>>())
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

    /// Every view the grid is left *for* that has no use for a stream.
    ///
    /// `go_back` is only one of the three routes. The vault trigger file and a
    /// worm/poll trigger both navigate straight out of `SessionGrid` from the
    /// main loop without going anywhere near `go_back`, so `Vault` and `Trail`
    /// are in this list on purpose and not for symmetry.
    ///
    /// `Global` is deliberately **not** here any more: the dashboard's sessions
    /// panel is now a view of the whole ranch, so it reads the same frames the
    /// grid does. `the_dashboard_keeps_the_streams_the_grid_opened` covers that
    /// route instead.
    fn views_off_the_grid() -> Vec<AppView> {
        let project: Project = fixture(r#"{"name":"proj","path":"/tmp/proj"}"#);
        vec![
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
        streams.reconcile_with(barns, recording_spawner(log, silent));
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
    fn only_the_grid_and_the_dashboard_want_streams() {
        assert!(streams_wanted(&AppView::SessionGrid));
        // The dashboard's sessions panel lists every connected barn's windows,
        // so it needs frames exactly as much as the grid does. Nothing else
        // renders one.
        assert!(
            streams_wanted(&AppView::Global),
            "the dashboard's sessions panel has no frames to list"
        );
        for view in views_off_the_grid() {
            assert!(!streams_wanted(&view), "{view:?} wants ssh channels open");
        }
    }

    /// The grid and the dashboard are one route apart in both directions — `v`
    /// and `Esc` — and both read frames. Tearing the streams down between them
    /// would cost a handshake per barn and blank the sessions panel until the
    /// next frame landed, every single time.
    #[test]
    fn the_dashboard_keeps_the_streams_the_grid_opened() {
        let barns = [named_barn("guided")];
        let log = RefCell::new(Vec::new());
        let (mut streams, _guards) = streaming(&barns, &log);
        let pid = child_pid(&streams, "guided").expect("streaming");

        sync_streams_for_view(&mut streams, &AppView::SessionGrid, &AppView::Global);
        assert_eq!(
            child_pid(&streams, "guided"),
            Some(pid),
            "Esc off the grid killed the streams the dashboard is about to list"
        );

        sync_streams_for_view(&mut streams, &AppView::Global, &AppView::SessionGrid);
        assert_eq!(child_pid(&streams, "guided"), Some(pid), "and `v` killed them coming back");

        streams.shutdown();
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
        let _ranch = crate::testing::temp_ranch();
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
        let both = sessions(&["guided", "smash-mac"]);

        for view in views_off_the_grid() {
            let log = RefCell::new(Vec::new());
            let mut streams = RemoteStreams::new();
            for _tick in 0..8 {
                tick_remote_streams_with(
                    &mut streams,
                    &view,
                    &barns,
                    &both,
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
            &both,
            recording_spawner(&log, silent),
        );
        let _guards = guard_all(&streams);
        assert_eq!(log.borrow().len(), 2, "the grid itself spawned nothing");
        streams.shutdown();
    }

    #[test]
    fn opening_the_grid_reconciles_without_waiting_for_the_first_tick() {
        let _ranch = crate::testing::temp_ranch();
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
        app.connected_barns = sessions(&["ghost"]);
        // CONTROL: the real `RemoteStream::spawn` below would open a real ssh to
        // whatever the fixture named. Nothing here may be reachable.
        assert!(
            crate::ssh::dial_host(&app.barns[0]).is_none(),
            "the fixture is dialable, so this test would ssh somewhere real"
        );

        app.open_session_grid(GridScope::All);

        assert!(matches!(app.view, AppView::SessionGrid), "the grid did not open");
        assert_eq!(
            failed_barns(&app.remote_grid),
            ["ghost"],
            "opening the grid never reconciled, so every remote cell waits a tick it need not"
        );
    }

    /// Connecting is what puts a barn's sessions on screen and disconnecting is
    /// what takes them off, both while the grid or dashboard stays open — which
    /// is why reconcile is on the tick and not only on open.
    #[test]
    fn a_barns_stream_follows_its_connect_session_while_the_grid_is_open() {
        let barns = [named_barn("guided"), named_barn("smash-mac")];
        let log = RefCell::new(Vec::new());
        let mut streams = RemoteStreams::new();
        let tick = |streams: &mut RemoteStreams, connected: &HashSet<String>| {
            tick_remote_streams_with(
                streams,
                &AppView::SessionGrid,
                &barns,
                connected,
                recording_spawner(&log, silent),
            )
        };

        tick(&mut streams, &sessions(&["guided"]));
        let mut guards = guard_all(&streams);
        assert_eq!(*log.borrow(), ["guided"], "a barn with no connect session got a stream");

        // smash-mac connects.
        tick(&mut streams, &sessions(&["guided", "smash-mac"]));
        guards.extend(guard_all(&streams));
        assert_eq!(
            *log.borrow(),
            ["guided", "smash-mac"],
            "a barn connected while the grid was open never reached it"
        );
        let guided = child_pid(&streams, "guided").expect("guided is streaming");

        // smash-mac disconnects.
        let gone = child_pid(&streams, "smash-mac").expect("smash-mac is streaming");
        tick(&mut streams, &sessions(&["guided"]));
        assert!(child_pid(&streams, "smash-mac").is_none(), "a disconnected barn kept its stream");
        assert_eq!(process_state(gone), None, "its child outlived the disconnect");
        assert_eq!(child_pid(&streams, "guided"), Some(guided), "the first stream was disturbed");

        streams.shutdown();
    }

    // === the jump ==========================================================
    //
    // Both halves of a jump leave the process — one ssh exec and one tmux
    // client switch — so both are injected, exactly as `quit_teardown`'s are.
    // Nothing here spawns anything: `tmux::switch_to_window` against this
    // machine would move the window of the yeehaw session the developer is
    // sitting in.

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
        app.connected_barns = HashSet::new();
        app.error = None;
        app
    }

    /// [`ranch`], with every named barn holding a connect session.
    fn connected_ranch(names: &[&str]) -> App {
        let mut app = ranch(names);
        app.connected_barns = sessions(names);
        app
    }

    /// A capital letter on the dashboard and a number key on the grid are the
    /// same act and must take the same route. Driven through a barn the ranch
    /// does not have, which is the one branch of that route that returns before
    /// anything leaves the process.
    #[test]
    fn a_barn_row_on_the_dashboard_goes_through_the_same_jump_the_grid_uses() {
        let _ranch = crate::testing::temp_ranch();
        let mut app = ranch(&[]);

        apply_dashboard_action(
            &mut app,
            DashboardAction::JumpToBarnSession { barn: "guided".into(), window_index: 4 },
        );

        assert_eq!(
            app.error.as_deref(),
            Some("barn 'guided' is no longer on the ranch"),
            "the dashboard's barn row does not go through `jump_to_cell`"
        );
    }

    /// The order *is* the design. Switching first shows whatever window the
    /// barn had selected and corrects it an ssh round trip later.
    #[test]
    fn a_jump_to_a_connected_barn_selects_on_the_barn_then_switches_to_its_session() {
        let _ranch = crate::testing::temp_ranch();
        let mut app = connected_ranch(&["guided"]);
        let log = jump_log(&mut app, Origin::Barn("guided".into()), 4, Ok(()), Ok(()));

        assert_eq!(log, ["select guided 4", "connect guided"]);
        assert_eq!(app.error, None);
    }

    /// No connect session, no yeehaw on the far end to point anywhere: the jump
    /// is the plain `c` connect, and the user picks from the barn's dashboard.
    #[test]
    fn a_jump_to_a_barn_with_no_connect_session_falls_back_to_connect() {
        let _ranch = crate::testing::temp_ranch();
        let mut app = ranch(&["guided"]);
        let log = jump_log(&mut app, Origin::Barn("guided".into()), 4, Ok(()), Ok(()));

        assert_eq!(log, ["connect guided"], "a barn with no connect session was selected on");
        assert_eq!(app.error, None);
    }

    /// The fallback is the `c` path, refusals included: a barn marked not
    /// connectable is refused with the same words `c` uses.
    #[test]
    fn the_fallback_refuses_what_c_refuses() {
        let _ranch = crate::testing::temp_ranch();
        let mut app = ranch(&["guided"]);
        app.barns[0].connectable = Some(false);
        let log = jump_log(&mut app, Origin::Barn("guided".into()), 4, Ok(()), Ok(()));

        assert!(log.is_empty(), "a refused barn was reached anyway: {log:?}");
        assert_eq!(app.error, crate::connect::refusal(&app.barns[0]));
    }

    #[test]
    fn a_local_jump_switches_locally_and_never_reaches_a_barn() {
        let _ranch = crate::testing::temp_ranch();
        // The unchanged path, asserted with a connected barn on the ranch so a
        // jump that wandered into the remote branch has somewhere to wander to.
        let mut app = connected_ranch(&["guided"]);
        let log = jump_log(&mut app, Origin::Local, 7, Ok(()), Ok(()));

        assert_eq!(log, ["switch 7"], "a local jump went over the network");
        assert_eq!(app.error, None);
    }

    #[test]
    fn a_jump_to_a_barn_that_vanished_reports_an_error_rather_than_connecting() {
        let _ranch = crate::testing::temp_ranch();
        // A frame outlives the config entry it came from: delete a barn while
        // its rows are on screen and the keys stay drawn. Connecting to
        // *something* here means the key under the user's finger silently
        // became a different host.
        let mut app = connected_ranch(&["guided"]);
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
    fn a_failed_remote_select_reports_the_error_and_does_not_switch() {
        let _ranch = crate::testing::temp_ranch();
        // Switching anyway lands the user on whatever the barn had selected —
        // the wrong session, made permanent and silent.
        let mut app = connected_ranch(&["guided"]);
        let log = jump_log(
            &mut app,
            Origin::Barn("guided".into()),
            4,
            Err(anyhow::anyhow!("can't find window: 4")),
            Ok(()),
        );

        assert_eq!(log, ["select guided 4"], "it switched anyway: {log:?}");
        let msg = app.error.expect("a failed remote select must say so");
        assert!(msg.contains("can't find window: 4"), "{msg}");
    }

    #[test]
    fn a_failed_connect_after_a_good_select_surfaces_the_whole_context_chain() {
        let _ranch = crate::testing::temp_ranch();
        // `{:#}`, the same as `connect_barn`. Plain Display prints only the
        // outermost layer, and the layer that says what actually went wrong is
        // underneath it — this banner is the only place the user ever sees it.
        let mut app = connected_ranch(&["guided"]);
        let cause = anyhow::anyhow!("permission denied")
            .context("failed to switch to the barn session");
        let log = jump_log(&mut app, Origin::Barn("guided".into()), 4, Ok(()), Err(cause));

        assert_eq!(log, ["select guided 4", "connect guided"]);
        let msg = app.error.expect("a failed connect must say so");
        assert!(msg.contains("permission denied"), "the cause was swallowed: {msg}");
    }

    #[test]
    fn the_grid_is_drawn_with_the_registry_s_stale_barns_not_an_empty_set() {
        let _ranch = crate::testing::temp_ranch();
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

    /// A barn is marked stale because its stream died, which makes its select
    /// the one most likely to sit out the whole timeout. The jump still lands.
    #[test]
    fn a_jump_to_a_stale_barn_skips_the_remote_select_but_still_lands() {
        let _ranch = crate::testing::temp_ranch();
        let mut app = connected_ranch(&["guided"]);
        mark_failed(&mut app.remote_grid, "guided", "the stream to 'guided' ended");

        let log = jump_log(&mut app, Origin::Barn("guided".into()), 4, Ok(()), Ok(()));

        assert_eq!(log, ["connect guided"], "a stale barn went over the network: {log:?}");
        assert_eq!(app.error, None, "a stale jump reported a failure that did not happen");
    }

    #[test]
    fn only_the_stale_barn_loses_its_remote_select() {
        let _ranch = crate::testing::temp_ranch();
        // Staleness is per barn. One dead barn must not cost the healthy one
        // beside it the window it was aimed at.
        let mut app = connected_ranch(&["guided", "smash-mac"]);
        mark_failed(&mut app.remote_grid, "guided", "the stream to 'guided' ended");

        let log = jump_log(&mut app, Origin::Barn("smash-mac".into()), 2, Ok(()), Ok(()));
        assert_eq!(
            log,
            ["select smash-mac 2", "connect smash-mac"],
            "a live barn lost its remote select to its neighbour dying: {log:?}"
        );
    }

    #[test]
    fn a_jump_finds_its_barn_by_exact_name_not_by_prefix() {
        let _ranch = crate::testing::temp_ranch();
        // `guided` and `guided-2` are different production hosts, and the same
        // hazard the `=` in every tmux target guards. A `starts_with` here sends
        // the user to whichever one the config listed first.
        let mut app = connected_ranch(&["guided-2", "guided"]);
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
        let _ranch = crate::testing::temp_ranch();
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

    // === identity across renames and creates ==============================

    fn livestock_on(barn: Option<&str>) -> Livestock {
        Livestock {
            name: "web".into(),
            path: "/tmp/web".into(),
            barn: barn.map(str::to_string),
            repo: None,
            branch: None,
            log_path: None,
            env_path: None,
            source: None,
            k8s_metadata: None,
            trails: vec![],
        }
    }

    /// Every barn lookup behind a shell, a log tail, a trail run and a worm
    /// trigger goes through this. `None` is what routes the work to the local
    /// path, so an adopted machine answering `Some(self-barn)` here is an ssh
    /// attempt at a barn record with no host.
    #[test]
    fn a_livestock_on_this_machine_has_no_barn_before_or_after_adoption() {
        let _ranch = crate::testing::temp_ranch();
        let mut imac = barn("imac");
        imac.host = None;
        let barns = vec![config::local_barn(), imac, barn("pi")];

        assert!(barn_for_livestock(&livestock_on(None), &barns, None).is_none());
        assert!(barn_for_livestock(&livestock_on(Some("local")), &barns, None).is_none());
        assert_eq!(
            barn_for_livestock(&livestock_on(Some("pi")), &barns, None).map(|b| b.name),
            Some("pi".to_string())
        );

        crate::migrate::adopt_this_machine("imac").unwrap();

        assert!(
            barn_for_livestock(&livestock_on(Some("imac")), &barns, None).is_none(),
            "adoption renamed this machine's livestock; it did not move it"
        );
        assert!(barn_for_livestock(&livestock_on(None), &barns, None).is_none());
        assert!(barn_for_livestock(&livestock_on(Some("local")), &barns, None).is_none());
        assert_eq!(
            barn_for_livestock(&livestock_on(Some("pi")), &barns, None).map(|b| b.name),
            Some("pi".to_string()),
            "a real remote barn is untouched by adoption"
        );
    }

    /// The barn the user reached the livestock *through* still wins — that is
    /// how a livestock opened from a barn's own list gets ssh'd into. But
    /// adoption creates a browsable barn record for this machine, and reaching
    /// a local livestock through it must not turn a local shell into an ssh to
    /// a hostless barn.
    #[test]
    fn the_barn_a_livestock_was_reached_through_wins_unless_it_is_this_machine() {
        let _ranch = crate::testing::temp_ranch();
        let mut imac = barn("imac");
        imac.host = None;
        let barns = vec![config::local_barn(), imac.clone(), barn("pi")];

        assert_eq!(
            barn_for_livestock(&livestock_on(None), &barns, Some(&barn("pi"))).map(|b| b.name),
            Some("pi".to_string()),
            "the source barn wins over the livestock's own"
        );
        assert!(
            barn_for_livestock(&livestock_on(None), &barns, Some(&config::local_barn())).is_none(),
            "the synthetic local barn is this machine, not a destination"
        );

        crate::migrate::adopt_this_machine("imac").unwrap();

        assert!(
            barn_for_livestock(&livestock_on(Some("imac")), &barns, Some(&imac)).is_none(),
            "the adopted self-barn is this machine too"
        );
        assert_eq!(
            barn_for_livestock(&livestock_on(Some("imac")), &barns, Some(&barn("pi"))).map(|b| b.name),
            Some("pi".to_string()),
            "a real barn reached through still wins"
        );
    }

    fn test_project(name: &str) -> Project {
        Project {
            name: name.into(),
            path: format!("/tmp/{name}"),
            summary: None,
            color: None,
            gradient_spread: None,
            gradient_inverted: None,
            livestock: vec![],
            herds: vec![],
            wiki: vec![],
            issue_provider: None,
            wiki_provider: None,
            id: None,
            created_at: None,
            updated_at: None,
        }
    }

    fn project_file_names() -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(config::projects_dir())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.ends_with(".yaml"))
            .collect();
        names.sort();
        names
    }

    /// `save_project` writes to `projects/<name>.yaml`. Renaming a project in
    /// the project view and saving it therefore left the *old* file in place
    /// beside the new one, both carrying the same uuid — one identity on two
    /// files, which no uuid-keyed merge can resolve.
    #[test]
    fn renaming_a_project_in_the_project_view_removes_the_old_file() {
        let _ranch = crate::testing::temp_ranch();

        let mut original = test_project("api");
        config::save_project(&mut original).unwrap();
        let id = original.id.clone().unwrap();

        let mut app = App::new();
        app.view = AppView::Project { project: original.clone() };

        let mut renamed = original.clone();
        renamed.name = "gateway".into();
        apply_project_action(&mut app, original.clone(), ProjectAction::UpdateProject(renamed));

        assert_eq!(app.error, None, "the rename must succeed");
        assert_eq!(
            project_file_names(),
            vec!["gateway.yaml".to_string()],
            "the old file must not survive the rename"
        );
        let loaded = config::load_projects();
        assert_eq!(loaded.len(), 1);
        assert_eq!(
            loaded[0].id.as_ref(),
            Some(&id),
            "a rename keeps the uuid — that is what distinguishes it from a delete plus a create"
        );
    }

    /// The rename must not be able to swallow a different project that happens
    /// to hold the destination name.
    #[test]
    fn renaming_a_project_onto_an_existing_one_is_refused_in_the_view() {
        let _ranch = crate::testing::temp_ranch();

        let mut api = test_project("api");
        config::save_project(&mut api).unwrap();
        let mut gateway = test_project("gateway");
        gateway.summary = Some("the real gateway".into());
        config::save_project(&mut gateway).unwrap();
        let gateway_id = gateway.id.clone().unwrap();

        let mut app = App::new();
        app.view = AppView::Project { project: api.clone() };

        let mut renamed = api.clone();
        renamed.name = "gateway".into();
        apply_project_action(&mut app, api.clone(), ProjectAction::UpdateProject(renamed));

        assert!(app.error.is_some(), "the collision must be reported, not swallowed");
        assert_eq!(
            project_file_names(),
            vec!["api.yaml".to_string(), "gateway.yaml".to_string()],
            "both projects must survive"
        );
        let survivor = config::load_projects().into_iter().find(|p| p.name == "gateway").unwrap();
        assert_eq!(survivor.id.as_ref(), Some(&gateway_id));
        assert_eq!(survivor.summary.as_deref(), Some("the real gateway"));
    }

    /// An edit that does not touch the name is still an ordinary save, and must
    /// not delete the file it just wrote.
    #[test]
    fn editing_a_project_without_renaming_it_keeps_the_file() {
        let _ranch = crate::testing::temp_ranch();

        let mut original = test_project("api");
        config::save_project(&mut original).unwrap();
        let id = original.id.clone().unwrap();

        let mut app = App::new();
        app.view = AppView::Project { project: original.clone() };

        let mut edited = original.clone();
        edited.summary = Some("edited".into());
        apply_project_action(&mut app, original.clone(), ProjectAction::UpdateProject(edited));

        assert_eq!(app.error, None);
        assert_eq!(project_file_names(), vec!["api.yaml".to_string()]);
        let loaded = config::load_projects();
        assert_eq!(loaded[0].summary.as_deref(), Some("edited"));
        assert_eq!(loaded[0].id.as_ref(), Some(&id));
    }

    /// The dashboard's create forms took a name and wrote it, with no check
    /// that the name was free. A project built by that form has empty
    /// livestock, herds and wiki and no id, so landing it on an existing entity
    /// destroyed the content and minted a new uuid over the old one.
    #[test]
    fn creating_a_project_that_already_exists_is_refused_from_the_dashboard() {
        let _ranch = crate::testing::temp_ranch();

        let mut existing = test_project("api");
        existing.summary = Some("the real one".into());
        existing.livestock.push(Livestock {
            name: "web".into(),
            path: "/tmp/web".into(),
            barn: None,
            repo: None,
            branch: None,
            log_path: None,
            env_path: None,
            source: None,
            k8s_metadata: None,
            trails: vec![],
        });
        config::save_project(&mut existing).unwrap();
        let id = existing.id.clone().unwrap();

        let mut app = App::new();
        apply_dashboard_action(
            &mut app,
            DashboardAction::CreateProject("api".into(), "/tmp/elsewhere".into()),
        );

        assert!(app.error.is_some(), "the collision must be reported");
        let loaded = config::load_projects();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id.as_ref(), Some(&id), "the uuid must not be re-minted");
        assert_eq!(loaded[0].livestock.len(), 1, "the livestock must survive");
        assert_eq!(loaded[0].summary.as_deref(), Some("the real one"));
    }

    /// The same for the other two dashboard forms. Each writes its own file.
    ///
    /// Only the refusal is driven here: the worm form's success path calls
    /// `crontab::sync_crontab()`, which rewrites the developer's real crontab —
    /// no temp ranch covers that.
    #[test]
    fn creating_a_barn_or_worm_that_already_exists_is_refused_from_the_dashboard() {
        let _ranch = crate::testing::temp_ranch();

        let mut existing_barn = barn("pi");
        config::save_barn(&mut existing_barn).unwrap();
        let barn_id = existing_barn.id.clone().unwrap();

        let mut existing_worm = Worm {
            name: "nightly".into(),
            command: "echo original".into(),
            schedule: "* * * * *".into(),
            worm_type: "shell".into(),
            enabled: true,
            project: None,
            working_dir: None,
            id: None,
            created_at: None,
            updated_at: None,
        };
        config::save_worm(&mut existing_worm).unwrap();
        let worm_id = existing_worm.id.clone().unwrap();

        let mut app = App::new();

        apply_dashboard_action(
            &mut app,
            DashboardAction::CreateBarn("pi".into(), "192.168.0.9".into(), "root".into(), 2222, None),
        );
        assert!(app.error.is_some(), "the barn collision must be reported");
        let reloaded = config::load_barns().into_iter().find(|b| b.name == "pi").unwrap();
        assert_eq!(reloaded.id.as_ref(), Some(&barn_id), "barn uuid must not be re-minted");
        assert_eq!(reloaded.host.as_deref(), Some("172.233.141.59"), "barn must not be rewritten");

        app.error = None;
        apply_dashboard_action(
            &mut app,
            DashboardAction::CreateWorm("nightly".into(), "rm -rf /".into(), "0 0 * * *".into()),
        );
        assert!(app.error.is_some(), "the worm collision must be reported");
        let reloaded = config::load_worms().into_iter().find(|w| w.name == "nightly").unwrap();
        assert_eq!(reloaded.id.as_ref(), Some(&worm_id), "worm uuid must not be re-minted");
        assert_eq!(reloaded.command, "echo original", "worm must not be rewritten");
    }

    // === the barns panel's order ===========================================
    //
    // This machine first, then the connected barns, then the rest. The order
    // needs live tmux state, so it is kept in `app.barns` itself — the list
    // every barns-panel action indexes into.

    #[test]
    fn connected_barns_sort_under_this_machine_and_above_the_rest() {
        let mut barns: Vec<Barn> =
            ["zulu", "Alpha", "beta", "smashed-air", "yankee"].iter().map(|n| barn(n)).collect();

        order_barns_for_display(&mut barns, &sessions(&["zulu", "beta"]), Some("smashed-air"));

        let names: Vec<&str> = barns.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(
            names,
            ["smashed-air", "beta", "zulu", "Alpha", "yankee"],
            "this machine, the connected ones by name, then the rest by name"
        );
    }

    /// The synthetic `local` row is this machine too, and a connected barn
    /// never overtakes it.
    #[test]
    fn the_synthetic_row_stays_first_however_much_is_connected() {
        let _ranch = crate::testing::temp_ranch();
        let mut barns = vec![barn("alpha"), config::local_barn(), barn("beta")];

        order_barns_for_display(&mut barns, &sessions(&["alpha", "beta"]), None);

        assert!(config::is_local_barn(&barns[0]), "{:?}", barns.iter().map(|b| &b.name).collect::<Vec<_>>());
    }

    /// The barn names as the barns panel draws them, top to bottom.
    fn barn_rows_on_screen(app: &mut App, names: &[&str]) -> Vec<String> {
        let screen = whole_screen(app, 160, 60);
        let mut rows: Vec<(usize, String)> = Vec::new();
        for name in names {
            let line = screen
                .lines()
                .position(|l| l.contains(name))
                .unwrap_or_else(|| panic!("'{name}' is not on screen:\n{screen}"));
            rows.push((line, name.to_string()));
        }
        rows.sort();
        rows.into_iter().map(|(_, n)| n).collect()
    }

    /// The property the order exists to keep: `c` and `Enter` act on the barn
    /// *drawn* on the row under the cursor. A panel that sorted on its own,
    /// over a list that did not, would connect to its neighbour.
    #[test]
    fn c_and_enter_act_on_the_barn_drawn_on_the_selected_row() {
        let _ranch = crate::testing::temp_ranch();
        for name in ["barn-alpha", "barn-beta", "barn-zulu"] {
            let mut b = barn(name);
            config::save_barn(&mut b).unwrap();
        }
        let mut app = App::new();
        app.windows = vec![];
        app.set_connected_barns(sessions(&["barn-zulu"]));

        let drawn = barn_rows_on_screen(&mut app, &["barn-alpha", "barn-beta", "barn-zulu"]);
        assert_eq!(drawn, ["barn-zulu", "barn-alpha", "barn-beta"], "control: zulu is connected");

        // Projects -> Sessions -> Barns, then down past the `local` row onto
        // the first barn drawn under it.
        let mut press = |key| {
            app.global_dashboard.handle_input(
                key,
                &app.projects,
                &app.barns,
                &app.worms,
                &app.windows,
                app.remote_grid.frames(),
                &HashSet::new(),
            )
        };
        press(KeyCode::Tab);
        press(KeyCode::Tab);
        press(KeyCode::Char('j'));
        let connect = press(KeyCode::Char('c'));
        let open = press(KeyCode::Enter);

        let DashboardAction::ConnectBarn(idx) = connect else {
            panic!("`c` on the barns panel did not connect")
        };
        assert_eq!(app.barns[idx].name, drawn[0], "`c` connected a different barn than the row shows");
        let DashboardAction::SelectBarn(idx) = open else {
            panic!("`Enter` on the barns panel did not open")
        };
        assert_eq!(app.barns[idx].name, drawn[0], "`Enter` opened a different barn than the row shows");
    }

    /// A barn moves up the list when it connects — usually while the user is
    /// inside it. The row they come back to has to still be that barn.
    #[test]
    fn the_barns_cursor_follows_its_barn_when_the_order_changes() {
        let _ranch = crate::testing::temp_ranch();
        for name in ["barn-alpha", "barn-beta", "barn-zulu"] {
            let mut b = barn(name);
            config::save_barn(&mut b).unwrap();
        }
        let mut app = App::new();
        app.set_connected_barns(HashSet::new());
        let zulu = app.barns.iter().position(|b| b.name == "barn-zulu").unwrap();
        app.global_dashboard.set_barn_cursor(zulu, &app.barns);

        app.set_connected_barns(sessions(&["barn-zulu"]));

        let moved_to = app.barns.iter().position(|b| b.name == "barn-zulu").unwrap();
        assert_ne!(moved_to, zulu, "control: connecting moved the barn");
        let at = app.global_dashboard.barn_cursor(&app.barns);
        assert_eq!(app.barns[at].name, "barn-zulu", "the cursor stayed on the row, not the barn");
    }

    fn pathless_project() -> Project {
        serde_yaml::from_str("name: api\npath: ''\n").unwrap()
    }

    const NO_PATH: &str = "project 'api' has no path on this machine — set it with e (edit) first";

    /// A project pulled from another machine has `path: ""`. Starting Claude
    /// "in" it would start Claude wherever yeehaw is running.
    #[test]
    fn new_claude_on_a_project_with_no_path_is_refused() {
        let _ranch = crate::testing::temp_ranch();
        let mut app = App::new();
        app.projects = vec![pathless_project()];
        app.error = None;
        apply_dashboard_action(&mut app, DashboardAction::NewClaude(0));
        assert_eq!(app.error.as_deref(), Some(NO_PATH));
    }

    #[test]
    fn issue_claude_on_a_project_with_no_path_is_refused() {
        let _ranch = crate::testing::temp_ranch();
        let mut app = App::new();
        app.error = None;
        open_issue_claude(&mut app, &pathless_project(), "ctx".into());
        assert_eq!(app.error.as_deref(), Some(NO_PATH));
    }

    #[test]
    fn a_blank_project_path_has_no_working_dir() {
        let mut p = pathless_project();
        assert_eq!(project_working_dir(&p), Err(NO_PATH.to_string()));
        p.path = "   ".into();
        assert_eq!(project_working_dir(&p), Err(NO_PATH.to_string()));
        p.path = "/srv/api".into();
        assert_eq!(project_working_dir(&p), Ok("/srv/api".to_string()));
    }

    /// `reload` replaces `self.barns` with the unsorted on-disk list. The
    /// cursor has to follow the barn it was on, not the index.
    #[test]
    fn the_barns_cursor_follows_its_barn_across_a_reload() {
        let _ranch = crate::testing::temp_ranch();
        for name in ["barn-alpha", "barn-beta", "barn-zulu"] {
            let mut b = barn(name);
            config::save_barn(&mut b).unwrap();
        }
        let mut app = App::new();
        app.set_connected_barns(sessions(&["barn-zulu"]));
        let zulu = app.barns.iter().position(|b| b.name == "barn-zulu").unwrap();
        app.global_dashboard.set_barn_cursor(zulu, &app.barns);
        assert_ne!(
            config::load_barns().iter().position(|b| b.name == "barn-zulu"),
            Some(zulu),
            "control: the on-disk order puts zulu somewhere else"
        );

        app.reload();

        let at = app.global_dashboard.barn_cursor(&app.barns);
        assert_eq!(app.barns[at].name, "barn-zulu", "the cursor jumped to another barn on reload");
    }

    /// The livestock view's "new trail" form takes a name from a text field and
    /// wrote it with no check that the name was free — the same clobber as the
    /// dashboard forms, on a path the create audit had to go looking for.
    #[test]
    fn saving_a_new_trail_over_an_existing_one_is_refused_from_the_livestock_view() {
        let _ranch = crate::testing::temp_ranch();

        let mut existing = crate::trails::Trail {
            name: "deploy".into(),
            on: None,
            env: Some(std::collections::HashMap::from([(
                "STAGE".to_string(),
                "prod".to_string(),
            )])),
            jobs: Default::default(),
            id: None,
            created_at: None,
            updated_at: None,
        };
        config::save_trail(&mut existing).unwrap();
        let id = existing.id.clone().unwrap();

        let project = test_project("api");
        let livestock = Livestock {
            name: "web".into(),
            path: "/tmp/web".into(),
            barn: None,
            repo: None,
            branch: None,
            log_path: None,
            env_path: None,
            source: None,
            k8s_metadata: None,
            trails: vec![],
        };

        let intruder = crate::trails::Trail {
            name: "deploy".into(),
            on: None,
            env: None,
            jobs: Default::default(),
            id: None,
            created_at: None,
            updated_at: None,
        };

        let mut app = App::new();
        apply_livestock_action(
            &mut app,
            project,
            livestock,
            "project".to_string(),
            None,
            LivestockAction::SaveNewTrail(intruder),
        );

        assert!(app.error.is_some(), "the collision must be reported");
        let reloaded = config::load_trail("deploy").unwrap();
        assert_eq!(reloaded.id.as_ref(), Some(&id), "trail uuid must not be re-minted");
        assert!(reloaded.env.is_some(), "the existing trail must not be rewritten");
    }

    /// The other half of the issues-view fix. `go_back` navigates out of
    /// `Issues` with whatever project the view is holding, so refreshing that
    /// snapshot is what stops the stale copy from reaching the project view and
    /// minting a second uuid over the one already on disk.
    #[test]
    fn an_issues_project_update_refreshes_the_snapshot_go_back_will_carry() {
        let _ranch = crate::testing::temp_ranch();

        // The state the view leaves behind: the file is stamped, the view's
        // snapshot is not.
        let stale = test_project("api");
        assert!(stale.id.is_none());
        let mut saved = stale.clone();
        config::save_project(&mut saved).unwrap();
        let id = saved.id.clone().unwrap();

        let mut app = App::new();
        app.view = AppView::Issues { project: stale };

        apply_issues_project_update(&mut app, saved);

        match &app.view {
            AppView::Issues { project } => {
                assert_eq!(project.id.as_ref(), Some(&id), "the snapshot must carry the saved id");
            }
            other => panic!("expected to stay on Issues, got {other:?}"),
        }

        app.go_back();
        match &app.view {
            AppView::Project { project } => {
                assert_eq!(
                    project.id.as_ref(),
                    Some(&id),
                    "go_back must not carry a project whose id was dropped"
                );
            }
            other => panic!("expected the project view, got {other:?}"),
        }
    }

    /// The project view's own create form, which writes a ranch hand file.
    #[test]
    fn creating_a_ranchhand_that_already_exists_is_refused_from_the_project_view() {
        let _ranch = crate::testing::temp_ranch();

        let mut project = test_project("api");
        config::save_project(&mut project).unwrap();

        let mut existing = RanchHand {
            name: "cluster".into(),
            project: "api".into(),
            rh_type: "kubernetes".into(),
            config: serde_yaml::Value::Null,
            sync_settings: RanchHandSyncSettings { auto_sync: false, interval_minutes: None },
            herd: "infra".into(),
            resource_mappings: vec![],
            last_sync: None,
            id: None,
            created_at: None,
            updated_at: None,
        };
        config::save_ranchhand(&mut existing).unwrap();
        let id = existing.id.clone().unwrap();

        let mut app = App::new();
        app.view = AppView::Project { project: project.clone() };
        apply_project_action(
            &mut app,
            project.clone(),
            ProjectAction::CreateRanchHand {
                name: "cluster".into(),
                rh_type: "terraform".into(),
                herd: "elsewhere".into(),
            },
        );

        assert!(app.error.is_some(), "the collision must be reported");
        let reloaded = config::load_ranchhands().into_iter().find(|r| r.name == "cluster").unwrap();
        assert_eq!(reloaded.id.as_ref(), Some(&id), "ranchhand uuid must not be re-minted");
        assert_eq!(reloaded.herd, "infra", "ranchhand must not be rewritten");
    }

    #[test]
    fn the_grids_bottom_bar_names_the_source_filter_and_the_help_key() {
        let _ranch = crate::testing::temp_ranch();
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
