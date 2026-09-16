use std::collections::HashSet;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::app::DashboardAction;
use crate::components::header;
use crate::components::list::{self, ListItem, ListState, ItemStatus, RowAction};
use crate::components::panel::Panel;
use crate::components::path_input::{self, PathInputState, PathInputAction};
use crate::components::text_input::TextInput;
use crate::config;
use crate::ssh;
use crate::tmux::{self, TmuxWindow};
use crate::types::*;

const BRAND_COLOR: Color = Color::Rgb(212, 160, 32);

#[derive(Debug, Clone, Copy, PartialEq)]
enum FocusedPanel {
    Projects,
    Barns,
    Sessions,
    Worms,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum InputMode {
    Normal,
    NewProjectName,
    NewProjectPath,
    NewBarnName,
    NewBarnHost,
    NewBarnUser,
    NewBarnPort,
    NewBarnKey,
    NewWormName,
    NewWormCommand,
    NewWormSchedule,
}

pub struct GlobalDashboard {
    focused_panel: FocusedPanel,
    projects_state: ListState,
    barns_state: ListState,
    sessions_state: ListState,
    worms_state: ListState,

    // Input mode
    input_mode: InputMode,
    text_input: TextInput,
    path_input: PathInputState,

    // New project form
    new_project_name: String,

    // New barn form
    new_barn_name: String,
    new_barn_host: String,
    new_barn_user: String,
    new_barn_port: String,

    // New worm form
    new_worm_name: String,
    new_worm_command: String,
}

impl GlobalDashboard {
    pub fn new() -> Self {
        Self {
            focused_panel: FocusedPanel::Projects,
            projects_state: ListState::new(),
            barns_state: ListState::new(),
            sessions_state: ListState::new(),
            worms_state: ListState::new(),
            input_mode: InputMode::Normal,
            text_input: TextInput::new(""),
            path_input: PathInputState::new(""),
            new_project_name: String::new(),
            new_barn_name: String::new(),
            new_barn_host: String::new(),
            new_barn_user: String::new(),
            new_barn_port: String::new(),
            new_worm_name: String::new(),
            new_worm_command: String::new(),
        }
    }

    pub fn is_input_mode(&self) -> bool {
        self.input_mode != InputMode::Normal
    }

    /// Index of the selected barn, but only when the barns panel has focus.
    ///
    /// `handle_input` takes a bare `KeyCode`, so modifier chords such as `C-d`
    /// have to be handled in `app.rs` where the full `KeyEvent` survives. This
    /// exposes the one thing that handler needs — deliberately narrower than
    /// making `focused_panel`/`barns_state` public or reworking the
    /// `handle_input` signature across every view.
    pub fn focused_barn_index(&self) -> Option<usize> {
        match self.focused_panel {
            FocusedPanel::Barns => Some(self.barns_state.selected),
            _ => None,
        }
    }

    fn reset_forms(&mut self) {
        self.new_project_name.clear();
        self.new_barn_name.clear();
        self.new_barn_host.clear();
        self.new_barn_user.clear();
        self.new_barn_port.clear();
        self.new_worm_name.clear();
        self.new_worm_command.clear();
        self.input_mode = InputMode::Normal;
    }

    pub fn handle_input(
        &mut self,
        key: KeyCode,
        projects: &[Project],
        barns: &[Barn],
        worms: &[Worm],
        windows: &[TmuxWindow],
    ) -> DashboardAction {
        // Input mode handling
        if self.input_mode != InputMode::Normal {
            // Escape cancels any input
            if key == KeyCode::Esc {
                self.reset_forms();
                return DashboardAction::None;
            }

            // Use path input for path fields, text input otherwise
            let is_path_field = matches!(self.input_mode, InputMode::NewProjectPath | InputMode::NewBarnKey);
            if is_path_field {
                let key_event = KeyEvent::new(key, KeyModifiers::empty());
                match path_input::handle_key(&mut self.path_input, key_event) {
                    PathInputAction::Submit(expanded) => {
                        // Copy expanded value to text_input for handle_submit
                        self.text_input.value = expanded;
                        return self.handle_submit();
                    }
                    PathInputAction::Cancel => {
                        self.reset_forms();
                        return DashboardAction::None;
                    }
                    PathInputAction::None => {}
                }
            } else {
                let submitted = self.text_input.handle_input(key);
                if submitted {
                    return self.handle_submit();
                }
            }
            return DashboardAction::None;
        }

        // Normal mode
        let session_windows: Vec<_> = windows.iter().filter(|w| w.index > 0).collect();

        // Tab to cycle panels (right then down: Projects → Sessions → Barns → Worms)
        if key == KeyCode::Tab {
            self.focused_panel = match self.focused_panel {
                FocusedPanel::Projects => FocusedPanel::Sessions,
                FocusedPanel::Sessions => FocusedPanel::Barns,
                FocusedPanel::Barns => FocusedPanel::Worms,
                FocusedPanel::Worms => FocusedPanel::Projects,
            };
            return DashboardAction::None;
        }

        if key == KeyCode::BackTab {
            self.focused_panel = match self.focused_panel {
                FocusedPanel::Projects => FocusedPanel::Worms,
                FocusedPanel::Sessions => FocusedPanel::Projects,
                FocusedPanel::Barns => FocusedPanel::Sessions,
                FocusedPanel::Worms => FocusedPanel::Barns,
            };
            return DashboardAction::None;
        }

        // 'n' key to create new item in focused panel
        if key == KeyCode::Char('n') {
            match self.focused_panel {
                FocusedPanel::Projects => {
                    self.input_mode = InputMode::NewProjectName;
                    self.text_input = TextInput::new("");
                    return DashboardAction::None;
                }
                FocusedPanel::Barns => {
                    self.input_mode = InputMode::NewBarnName;
                    self.text_input = TextInput::new("");
                    return DashboardAction::None;
                }
                FocusedPanel::Worms => {
                    self.input_mode = InputMode::NewWormName;
                    self.text_input = TextInput::new("");
                    return DashboardAction::None;
                }
                _ => {}
            }
        }

        // Number keys for quick session switching (1-9)
        if let KeyCode::Char(c) = key {
            if let Some(num) = c.to_digit(10) {
                if num >= 1 && num <= 9 {
                    let idx = (num - 1) as usize;
                    if idx < session_windows.len() {
                        return DashboardAction::SelectWindow(idx);
                    }
                }
            }
        }

        // Panel-specific input
        match self.focused_panel {
            FocusedPanel::Projects => {
                match key {
                    KeyCode::Char('j') | KeyCode::Down => self.projects_state.select_next(projects.len()),
                    KeyCode::Char('k') | KeyCode::Up => self.projects_state.select_prev(),
                    KeyCode::Char('g') => self.projects_state.select_first(),
                    KeyCode::Char('G') => self.projects_state.select_last(projects.len()),
                    KeyCode::Enter => return DashboardAction::SelectProject(self.projects_state.selected),
                    KeyCode::Char('c') => return DashboardAction::NewClaude(self.projects_state.selected),
                    KeyCode::Char('d') => return DashboardAction::RequestDeleteProject(self.projects_state.selected),
                    _ => {}
                }
            }
            FocusedPanel::Barns => {
                match key {
                    KeyCode::Char('j') | KeyCode::Down => self.barns_state.select_next(barns.len()),
                    KeyCode::Char('k') | KeyCode::Up => self.barns_state.select_prev(),
                    KeyCode::Char('g') => self.barns_state.select_first(),
                    KeyCode::Char('G') => self.barns_state.select_last(barns.len()),
                    KeyCode::Enter => return DashboardAction::SelectBarn(self.barns_state.selected),
                    KeyCode::Char('s') => return DashboardAction::SshToBarn(self.barns_state.selected),
                    KeyCode::Char('c') => return DashboardAction::ConnectBarn(self.barns_state.selected),
                    KeyCode::Char('d') => return DashboardAction::RequestDeleteBarn(self.barns_state.selected),
                    _ => {}
                }
            }
            FocusedPanel::Sessions => {
                match key {
                    KeyCode::Char('j') | KeyCode::Down => self.sessions_state.select_next(session_windows.len()),
                    KeyCode::Char('k') | KeyCode::Up => self.sessions_state.select_prev(),
                    KeyCode::Char('g') => self.sessions_state.select_first(),
                    KeyCode::Char('G') => self.sessions_state.select_last(session_windows.len()),
                    KeyCode::Enter => return DashboardAction::SelectWindow(self.sessions_state.selected),
                    _ => {}
                }
            }
            FocusedPanel::Worms => {
                match key {
                    KeyCode::Char('j') | KeyCode::Down => self.worms_state.select_next(worms.len()),
                    KeyCode::Char('k') | KeyCode::Up => self.worms_state.select_prev(),
                    KeyCode::Char('g') => self.worms_state.select_first(),
                    KeyCode::Char('G') => self.worms_state.select_last(worms.len()),
                    KeyCode::Enter => return DashboardAction::SelectWorm(self.worms_state.selected),
                    KeyCode::Char('d') => return DashboardAction::RequestDeleteWorm(self.worms_state.selected),
                    _ => {}
                }
            }
        }

        DashboardAction::None
    }

    fn handle_submit(&mut self) -> DashboardAction {
        let value = self.text_input.value.trim().to_string();

        match self.input_mode {
            InputMode::NewProjectName => {
                if !value.is_empty() {
                    self.new_project_name = value;
                    self.input_mode = InputMode::NewProjectPath;
                    self.path_input = PathInputState::new("~/");
                }
            }
            InputMode::NewProjectPath => {
                if !value.is_empty() {
                    let name = self.new_project_name.clone();
                    let path = value;
                    self.reset_forms();
                    return DashboardAction::CreateProject(name, path);
                }
            }
            InputMode::NewBarnName => {
                if !value.is_empty() {
                    self.new_barn_name = value;
                    self.input_mode = InputMode::NewBarnHost;
                    self.text_input = TextInput::new("");
                }
            }
            InputMode::NewBarnHost => {
                if !value.is_empty() {
                    self.new_barn_host = value;
                    self.input_mode = InputMode::NewBarnUser;
                    self.text_input = TextInput::new("root");
                }
            }
            InputMode::NewBarnUser => {
                if !value.is_empty() {
                    self.new_barn_user = value;
                    self.input_mode = InputMode::NewBarnPort;
                    self.text_input = TextInput::new("22");
                }
            }
            InputMode::NewBarnPort => {
                self.new_barn_port = if value.is_empty() { "22".to_string() } else { value };
                self.input_mode = InputMode::NewBarnKey;
                self.path_input = PathInputState::new("~/.ssh/id_rsa");
            }
            InputMode::NewBarnKey => {
                let name = self.new_barn_name.clone();
                let host = self.new_barn_host.clone();
                let user = self.new_barn_user.clone();
                let port = self.new_barn_port.parse::<u16>().unwrap_or(22);
                let key = if value.is_empty() { None } else { Some(value) };
                self.reset_forms();
                return DashboardAction::CreateBarn(name, host, user, port, key);
            }
            InputMode::NewWormName => {
                if !value.is_empty() {
                    self.new_worm_name = value;
                    self.input_mode = InputMode::NewWormCommand;
                    self.text_input = TextInput::new("");
                }
            }
            InputMode::NewWormCommand => {
                if !value.is_empty() {
                    self.new_worm_command = value;
                    self.input_mode = InputMode::NewWormSchedule;
                    self.text_input = TextInput::new("0 * * * *");
                }
            }
            InputMode::NewWormSchedule => {
                let name = self.new_worm_name.clone();
                let command = self.new_worm_command.clone();
                let schedule = if value.is_empty() { "0 * * * *".to_string() } else { value };
                self.reset_forms();
                return DashboardAction::CreateWorm(name, command, schedule);
            }
            InputMode::Normal => {}
        }

        DashboardAction::None
    }

    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        projects: &[Project],
        barns: &[Barn],
        worms: &[Worm],
        windows: &[TmuxWindow],
        connected_barns: &HashSet<String>,
    ) {
        let session_windows: Vec<_> = windows.iter().filter(|w| w.index > 0).collect();

        // Layout: Header (figlet art) + Content
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(9), // figlet header + tumbleweed + version
                Constraint::Min(1),    // content
            ])
            .split(area);

        header::render_header(frame, chunks[0], &header::HeaderProps {
            text: "YEEHAW",
            subtitle: None,
            summary: None,
            color: None,
            gradient_spread: None,
            gradient_inverted: false,
            version_info: None,
        });

        // If in input mode, render the form overlay
        if self.input_mode != InputMode::Normal {
            self.render_input_form(frame, chunks[1]);
            return;
        }

        // Content: 2 columns
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(40),
                Constraint::Percentage(60),
            ])
            .margin(1)
            .split(chunks[1]);

        // Left column: Projects + Barns
        let left_panels = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(55),
                Constraint::Percentage(45),
            ])
            .split(columns[0]);

        // Right column: Sessions + Worms
        let right_panels = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(60),
                Constraint::Percentage(40),
            ])
            .split(columns[1]);

        // Projects panel
        let projects_panel = Panel {
            title: "Projects",
            focused: self.focused_panel == FocusedPanel::Projects,
            hints: Some("[n] new  [d] delete"),
        };
        let projects_inner = projects_panel.render(frame, left_panels[0]);
        let project_items = build_project_items(projects, windows);
        list::render_list(
            frame, projects_inner, &project_items,
            &mut self.projects_state,
            self.focused_panel == FocusedPanel::Projects,
            None,
        );

        // Barns panel
        let barns_panel = Panel {
            title: "Barns",
            focused: self.focused_panel == FocusedPanel::Barns,
            hints: Some("[n] new  [d] delete"),
        };
        let barns_inner = barns_panel.render(frame, left_panels[1]);
        let barn_items = build_barn_items(barns, connected_barns);
        list::render_list(
            frame, barns_inner, &barn_items,
            &mut self.barns_state,
            self.focused_panel == FocusedPanel::Barns,
            None,
        );

        // Sessions panel
        let sessions_panel = Panel {
            title: "Sessions",
            focused: self.focused_panel == FocusedPanel::Sessions,
            hints: None,
        };
        let sessions_inner = sessions_panel.render(frame, right_panels[0]);
        let session_items = build_session_items(&session_windows);
        list::render_list(
            frame, sessions_inner, &session_items,
            &mut self.sessions_state,
            self.focused_panel == FocusedPanel::Sessions,
            None,
        );

        // Worms panel
        let worms_panel = Panel {
            title: "Worms",
            focused: self.focused_panel == FocusedPanel::Worms,
            hints: Some("[n] new  [d] delete"),
        };
        let worms_inner = worms_panel.render(frame, right_panels[1]);
        let worm_items = build_worm_items(worms);
        list::render_list(
            frame, worms_inner, &worm_items,
            &mut self.worms_state,
            self.focused_panel == FocusedPanel::Worms,
            None,
        );
    }

    fn render_input_form(&self, frame: &mut Frame, area: Rect) {
        let (title, label, step_info) = match self.input_mode {
            InputMode::NewProjectName => ("New Project", "Name:", "Step 1/2"),
            InputMode::NewProjectPath => ("New Project", "Path:", "Step 2/2"),
            InputMode::NewBarnName => ("New Barn", "Name:", "Step 1/5"),
            InputMode::NewBarnHost => ("New Barn", "Host:", "Step 2/5"),
            InputMode::NewBarnUser => ("New Barn", "User:", "Step 3/5"),
            InputMode::NewBarnPort => ("New Barn", "Port:", "Step 4/5"),
            InputMode::NewBarnKey => ("New Barn", "Identity file:", "Step 5/5"),
            InputMode::NewWormName => ("New Worm", "Name:", "Step 1/3"),
            InputMode::NewWormCommand => ("New Worm", "Command:", "Step 2/3"),
            InputMode::NewWormSchedule => ("New Worm", "Schedule (cron):", "Step 3/3"),
            InputMode::Normal => return,
        };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2), // title
                Constraint::Length(2), // completed fields
                Constraint::Length(1), // current label
                Constraint::Length(1), // input
                Constraint::Length(2), // hints
                Constraint::Min(1),    // spacer
            ])
            .margin(2)
            .split(area);

        // Title
        let title_text = Paragraph::new(format!("  {} ({})", title, step_info))
            .style(Style::default().fg(BRAND_COLOR).add_modifier(Modifier::BOLD));
        frame.render_widget(title_text, chunks[0]);

        // Show completed fields
        let mut completed_lines: Vec<Line> = Vec::new();
        match self.input_mode {
            InputMode::NewProjectPath => {
                completed_lines.push(Line::from(vec![
                    Span::styled("  Name: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(&self.new_project_name),
                ]));
            }
            InputMode::NewBarnHost => {
                completed_lines.push(Line::from(vec![
                    Span::styled("  Name: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(&self.new_barn_name),
                ]));
            }
            InputMode::NewBarnUser => {
                completed_lines.push(Line::from(vec![
                    Span::styled("  Name: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(&self.new_barn_name),
                    Span::styled("  Host: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(&self.new_barn_host),
                ]));
            }
            InputMode::NewBarnPort => {
                completed_lines.push(Line::from(vec![
                    Span::styled("  Name: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(&self.new_barn_name),
                    Span::styled("  Host: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(&self.new_barn_host),
                    Span::styled("  User: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(&self.new_barn_user),
                ]));
            }
            InputMode::NewBarnKey => {
                completed_lines.push(Line::from(vec![
                    Span::styled("  Name: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(&self.new_barn_name),
                    Span::styled("  Host: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(format!("{}@{}:{}", self.new_barn_user, self.new_barn_host, self.new_barn_port)),
                ]));
            }
            InputMode::NewWormCommand => {
                completed_lines.push(Line::from(vec![
                    Span::styled("  Name: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(&self.new_worm_name),
                ]));
            }
            InputMode::NewWormSchedule => {
                completed_lines.push(Line::from(vec![
                    Span::styled("  Name: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(&self.new_worm_name),
                    Span::styled("  Cmd: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(&self.new_worm_command),
                ]));
            }
            _ => {}
        }
        if !completed_lines.is_empty() {
            let completed = Paragraph::new(completed_lines);
            frame.render_widget(completed, chunks[1]);
        }

        // Current field label
        let label_text = Paragraph::new(format!("  {}", label))
            .style(Style::default().fg(Color::White));
        frame.render_widget(label_text, chunks[2]);

        // Input field — use path input for path fields, text input otherwise
        let is_path_field = matches!(self.input_mode, InputMode::NewProjectPath | InputMode::NewBarnKey);
        let input_area = Rect {
            x: chunks[3].x + 4,
            y: chunks[3].y,
            width: chunks[3].width.saturating_sub(4),
            height: if is_path_field { chunks[3].height.max(1) + chunks[4].height + chunks[5].height } else { 1 },
        };
        if is_path_field {
            path_input::render(frame, input_area, &self.path_input);
        } else {
            self.text_input.render(frame, input_area);
        }

        // Hints
        let hint_text = if is_path_field {
            "  Tab: complete  Enter: next field  Esc: cancel"
        } else {
            "  Enter: next field  Esc: cancel"
        };
        let hints = Paragraph::new(hint_text)
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(hints, chunks[4]);
    }
}

fn build_project_items(projects: &[Project], windows: &[TmuxWindow]) -> Vec<ListItem> {
    projects.iter().map(|p| {
        let session_count = windows.iter()
            .filter(|w| w.index > 0 && w.name.starts_with(&p.name))
            .count();
        let meta = if session_count > 0 {
            Some(format!("{} session{}", session_count, if session_count > 1 { "s" } else { "" }))
        } else {
            None
        };
        ListItem {
            id: p.name.clone(),
            label: p.name.clone(),
            status: Some(if session_count > 0 { ItemStatus::Active } else { ItemStatus::Inactive }),
            meta,
            actions: vec![RowAction { key: "c".to_string(), label: "claude".to_string() }],
        }
    }).collect()
}

/// `connected` holds bare tmux session names, so membership is tested with
/// [`tmux::barn_session_name`]. `barn_session_target` is for `-t` arguments only
/// — its `=` prefix is not part of any name tmux reports, so looking one up here
/// would never match and the indicator would never appear.
///
/// No tmux call happens in here: the set is refreshed on the app's idle tick.
fn build_barn_items(barns: &[Barn], connected: &HashSet<String>) -> Vec<ListItem> {
    // Once for the panel, not once per row. `this_barn_name` is a single small
    // read by design, but this runs on every redraw and a ranch has as many rows
    // as it has machines.
    let this_machine = config::this_barn_name();

    barns.iter().map(|b| {
        let is_connected = connected.contains(&tmux::barn_session_name(&b.name));
        // Two different questions, and both answer "this machine".
        // `is_local_barn` knows only the synthetic `local` row; after adoption
        // there is *also* a real record — a name, a uuid, a place in the ranch —
        // and without the second half that row reads as a remote barn with
        // nothing to dial. The label still distinguishes them: only the
        // synthetic one is called `local`.
        let is_this_machine =
            config::is_local_barn(b) || this_machine.as_deref() == Some(b.name.as_str());

        // Built as parts and joined rather than nested formats: there are four
        // things a row can now say and every combination occurs — the Ranch
        // House is exactly the barn most likely to be all four at once.
        let mut parts: Vec<String> = Vec::new();

        // Identity first: what this row *is*, or where it will be reached.
        if is_this_machine {
            parts.push("this machine".to_string());
        } else if let Some((user, host)) = b.user.as_deref().zip(ssh::dial_host(b)) {
            // `dial_host`, not `b.host`: a barn record that arrived from the
            // machine it describes carries no host — that machine had nothing to
            // dial itself with — only the addresses it advertised, and those are
            // what ssh will actually use. Reading `b.host` here would leave the
            // row blank for precisely the barns the ranch just learned about.
            parts.push(format!("{}@{}", user, host));
        }

        // Then role, then ranch membership, then live state — least volatile to
        // most, which leaves `connected` last where it has always been.
        //
        // Both new markers are *additive*: a row says "ranch house" or "synced"
        // when it is one and says nothing extra when it is not. That is what
        // lets them ride in `meta` at all — the synthetic `local` row still
        // reads exactly "this machine", and no other view's `ListItem` changes.
        if b.is_ranch_house == Some(true) {
            parts.push("ranch house".to_string());
        }
        // `Some(true)` only. `Some(false)` is a barn this machine has decided not
        // to sync, `None` is one nothing has enrolled — different states, neither
        // of them "in the ranch", and `types.rs` pins that absent is not `false`
        // dressed up as an answer.
        if b.synced == Some(true) {
            parts.push("synced".to_string());
        }
        if is_connected {
            parts.push("connected".to_string());
        }

        ListItem {
            id: b.name.clone(),
            label: if config::is_local_barn(b) { "local".to_string() } else { b.name.clone() },
            // The green dot is the connection: it used to be Active for every
            // barn, which said nothing. Every other panel here already reads its
            // dot as live state (a project with sessions, an enabled worm).
            // `ItemStatus` has three variants and connectedness already spends
            // them, so the two ranch indicators had to go somewhere else
            // regardless of which carrier was cheapest.
            status: Some(if is_connected { ItemStatus::Active } else { ItemStatus::Inactive }),
            meta: (!parts.is_empty()).then(|| parts.join(" · ")),
            actions: vec![RowAction { key: "s".to_string(), label: "shell".to_string() }],
        }
    }).collect()
}

fn build_session_items(session_windows: &[&TmuxWindow]) -> Vec<ListItem> {
    session_windows.iter().enumerate().map(|(i, w)| {
        // Type comes from the @yeehaw_type tmux option, not the window name.
        // The old name-sniffing decoder mislabelled anything it did not expect.
        let label = w.name.clone();
        let status_info = tmux::get_window_status(w);
        let meta_text = if !w.window_type.is_empty() {
            format!("{} · {}", w.window_type, status_info.text)
        } else {
            status_info.text
        };
        ListItem {
            id: w.index.to_string(),
            label: format!("[{}] {}", i + 1, label),
            status: Some(if w.active { ItemStatus::Active } else { ItemStatus::Inactive }),
            meta: Some(meta_text),
            actions: vec![],
        }
    }).collect()
}

fn build_worm_items(worms: &[Worm]) -> Vec<ListItem> {
    worms.iter().map(|w| {
        let runs = config::load_worm_runs(&w.name);
        let last_run_meta = runs.first().map(|r| {
            let ago = format_run_age(&r.started_at);
            let icon = match r.exit_code {
                Some(0) => "✓",
                Some(_) => "✗",
                None => "○",
            };
            format!(" {} {}", icon, ago)
        }).unwrap_or_default();

        ListItem {
            id: w.name.clone(),
            label: w.name.clone(),
            status: Some(if w.enabled { ItemStatus::Active } else { ItemStatus::Inactive }),
            meta: Some(format!("{}{}", w.schedule, last_run_meta)),
            actions: vec![],
        }
    }).collect()
}

fn format_run_age(iso_timestamp: &str) -> String {
    if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(iso_timestamp) {
        let diff = chrono::Utc::now().signed_duration_since(ts);
        let seconds = diff.num_seconds();
        if seconds < 60 { return "now".to_string(); }
        let minutes = seconds / 60;
        if minutes < 60 { return format!("{}m ago", minutes); }
        let hours = minutes / 60;
        if hours < 24 { return format!("{}h ago", hours); }
        let days = hours / 24;
        return format!("{}d ago", days);
    }
    "unknown".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

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
    /// actually prints, filtered by `connected_barn_sessions`.
    fn connected(session_names: &[String]) -> HashSet<String> {
        tmux::connected_barn_sessions(session_names)
    }

    #[test]
    fn a_connected_barn_gets_the_green_dot_and_the_word_connected() {
        // `build_barn_items` asks `config::this_barn_name()` which row is this
        // machine, so it reads the config file. Harness only — no assertion below
        // depends on what is in it.
        let _ranch = crate::testing::temp_ranch();
        let barns = [barn("guided")];
        let set = connected(&[tmux::barn_session_name("guided")]);

        let items = build_barn_items(&barns, &set);

        assert_eq!(items[0].status, Some(ItemStatus::Active));
        assert!(items[0].meta.as_deref().unwrap().contains("connected"));
    }

    #[test]
    fn a_barn_with_no_session_is_not_marked_connected() {
        // `build_barn_items` asks `config::this_barn_name()` which row is this
        // machine, so it reads the config file. Harness only — no assertion below
        // depends on what is in it.
        let _ranch = crate::testing::temp_ranch();
        let barns = [barn("guided")];

        let items = build_barn_items(&barns, &HashSet::new());

        assert_eq!(items[0].status, Some(ItemStatus::Inactive));
        assert!(!items[0].meta.as_deref().unwrap().contains("connected"));
    }

    /// The lookup must use `barn_session_name`, not `barn_session_target`. The
    /// `=` prefix is for `-t` arguments; no name tmux reports carries it, so a
    /// target-keyed lookup would silently never match.
    #[test]
    fn the_indicator_matches_names_as_tmux_reports_them_not_targets() {
        // `build_barn_items` asks `config::this_barn_name()` which row is this
        // machine, so it reads the config file. Harness only — no assertion below
        // depends on what is in it.
        let _ranch = crate::testing::temp_ranch();
        let barns = [barn("guided")];
        // Exactly what `tmux list-sessions -F '#{session_name}'` prints, plus
        // unrelated sessions that must not confuse the filter.
        let sessions = [
            "yeehaw".to_string(),
            tmux::barn_session_name("guided"),
            "scratch".to_string(),
        ];

        let items = build_barn_items(&barns, &connected(&sessions));

        assert_eq!(items[0].status, Some(ItemStatus::Active));
        assert!(!tmux::barn_session_target("guided").is_empty());
        assert!(!connected(&sessions).contains(&tmux::barn_session_target("guided")));
    }

    /// `guided` must not light up because `guided-2` is connected — the two are
    /// different hosts, and the hash suffix is what keeps their sessions apart.
    #[test]
    fn a_barn_is_not_marked_connected_by_another_barns_session() {
        // `build_barn_items` asks `config::this_barn_name()` which row is this
        // machine, so it reads the config file. Harness only — no assertion below
        // depends on what is in it.
        let _ranch = crate::testing::temp_ranch();
        let barns = [barn("guided"), barn("guided-2")];
        let set = connected(&[tmux::barn_session_name("guided-2")]);

        let items = build_barn_items(&barns, &set);

        assert_eq!(items[0].status, Some(ItemStatus::Inactive));
        assert_eq!(items[1].status, Some(ItemStatus::Active));
    }

    /// The local barn is never connectable, so it never carries the indicator
    /// even though its row still shows "this machine".
    #[test]
    fn the_local_barn_keeps_its_meta_and_never_shows_connected() {
        let _ranch = crate::testing::temp_ranch();
        let barns = [config::local_barn()];

        let items = build_barn_items(&barns, &HashSet::new());

        assert_eq!(items[0].label, "local");
        assert_eq!(items[0].meta.as_deref(), Some("this machine"));
        assert_eq!(items[0].status, Some(ItemStatus::Inactive));
    }

    // === ranch membership on the barn rows ==================================
    //
    // The user joined a ranch and there was no sign of it anywhere in the TUI.
    // Both indicators ride in `meta` — see `build_barn_items` for why that
    // rather than a new `ListItem` field — and both are *additive*: a row says
    // "ranch house" or "synced" when it is one, and says nothing extra when it
    // is not. That is what keeps the local barn's row exactly "this machine"
    // and leaves every other view's `ListItem` construction untouched.

    /// There is exactly one Ranch House per ranch and it is the machine that
    /// arbitrates every merge. Nothing in the TUI said which one it was.
    #[test]
    fn the_ranch_house_is_marked_on_its_row() {
        let _ranch = crate::testing::temp_ranch();
        let mut house = barn("camerons-imac");
        house.is_ranch_house = Some(true);

        let items = build_barn_items(&[house, barn("guided")], &HashSet::new());

        assert!(
            items[0].meta.as_deref().unwrap().contains("ranch house"),
            "the house must be identifiable: {:?}",
            items[0].meta
        );
        assert!(
            !items[1].meta.as_deref().unwrap().contains("ranch house"),
            "and only the house: {:?}",
            items[1].meta
        );
    }

    /// `synced` is what says this machine actually exchanges entities with a
    /// barn. A ranch the user just joined looked identical to one they had not.
    #[test]
    fn a_barn_in_the_ranch_is_distinguished_from_one_that_is_not() {
        let _ranch = crate::testing::temp_ranch();
        let mut enrolled = barn("camerons-imac");
        enrolled.synced = Some(true);
        let mut declined = barn("guided");
        declined.synced = Some(false);

        let items = build_barn_items(&[enrolled, declined, barn("never-asked")], &HashSet::new());

        assert!(items[0].meta.as_deref().unwrap().contains("synced"), "{:?}", items[0].meta);
        assert!(
            !items[1].meta.as_deref().unwrap().contains("synced"),
            "`Some(false)` is a barn this machine does not sync: {:?}",
            items[1].meta
        );
        assert!(
            !items[2].meta.as_deref().unwrap().contains("synced"),
            "and absent is not `false` dressed up as an answer: {:?}",
            items[2].meta
        );
    }

    /// The indicators compose with each other and with the connection dot,
    /// because the Ranch House is exactly the barn most likely to be all three
    /// at once.
    #[test]
    fn the_markers_compose_without_displacing_the_connection() {
        let _ranch = crate::testing::temp_ranch();
        let mut house = barn("camerons-imac");
        house.is_ranch_house = Some(true);
        house.synced = Some(true);
        let set = connected(&[tmux::barn_session_name("camerons-imac")]);

        let items = build_barn_items(&[house], &set);

        let meta = items[0].meta.clone().unwrap();
        for expected in ["forge@172.233.141.59", "ranch house", "synced", "connected"] {
            assert!(meta.contains(expected), "{:?} missing from {:?}", expected, meta);
        }
        assert_eq!(
            items[0].status,
            Some(ItemStatus::Active),
            "the dot is still the connection and nothing else"
        );
    }

    /// After adoption this machine has a *real* barn record — a name, a uuid, a
    /// place in the ranch — and the synthetic `local` row sits above it. Both
    /// are this machine, and `is_local_barn` only ever knew the synthetic one, so
    /// the real row used to read as a remote barn with nothing to dial.
    #[test]
    fn the_adopted_self_barn_reads_as_this_machine_and_keeps_its_own_name() {
        let _ranch = crate::testing::temp_ranch();
        let mut cfg = config::load_config();
        cfg.this_barn = Some("smashed-air".into());
        config::save_config(&cfg).unwrap();

        let mut self_barn = Barn {
            name: "smashed-air".into(),
            host: None,
            user: Some("cam".into()),
            synced: Some(true),
            ..Default::default()
        };
        self_barn.addresses = vec!["smashed-air.local".into()];

        let items =
            build_barn_items(&[config::local_barn(), self_barn, barn("guided")], &HashSet::new());

        assert_eq!(items[1].label, "smashed-air", "its own name, not the `local` alias");
        let meta = items[1].meta.clone().unwrap();
        assert!(
            meta.starts_with("this machine"),
            "the user is sitting at it — `cam@smashed-air.local` is telling them their own \
             address where it used to say `local`: {:?}",
            meta
        );
        assert!(meta.contains("synced"), "and it is in the ranch: {:?}", meta);
        assert_eq!(items[0].meta.as_deref(), Some("this machine"), "the alias row is unchanged");
    }

    /// A barn that arrived from the machine it describes carries no `host` — it
    /// had nothing to dial itself with — only the addresses it advertised. The
    /// row has to show where it will actually be reached, or the fix to the ssh
    /// path is invisible in the one place the user looks.
    #[test]
    fn a_barn_that_advertises_an_address_shows_where_it_will_be_dialled() {
        let _ranch = crate::testing::temp_ranch();
        let mut imac = Barn {
            name: "camerons-imac".into(),
            host: None,
            user: Some("cam".into()),
            ..Default::default()
        };
        imac.addresses = vec!["camerons-imac.local".into()];

        let items = build_barn_items(&[imac], &HashSet::new());

        assert_eq!(items[0].meta.as_deref(), Some("cam@camerons-imac.local"));
    }

    /// `C-d` is dispatched from `app.rs`, which can only see the barns panel
    /// through this accessor. Every other panel must report nothing, or `C-d`
    /// anywhere on the dashboard would tear down whichever barn row happened to
    /// be selected underneath.
    #[test]
    fn the_focused_barn_index_is_none_unless_the_barns_panel_has_focus() {
        let mut dash = GlobalDashboard::new();
        let barns = [barn("guided"), barn("guided-2")];

        // Focus starts on Projects.
        assert_eq!(dash.focused_barn_index(), None);

        // Tab cycles Projects -> Sessions -> Barns.
        dash.handle_input(KeyCode::Tab, &[], &barns, &[], &[]);
        assert_eq!(dash.focused_barn_index(), None);
        dash.handle_input(KeyCode::Tab, &[], &barns, &[], &[]);
        assert_eq!(dash.focused_barn_index(), Some(0));

        // It tracks the selection, not just the panel.
        dash.handle_input(KeyCode::Char('j'), &[], &barns, &[], &[]);
        assert_eq!(dash.focused_barn_index(), Some(1));

        // Worms is next: focus leaves the panel and so does the index.
        dash.handle_input(KeyCode::Tab, &[], &barns, &[], &[]);
        assert_eq!(dash.focused_barn_index(), None);
    }

    /// The `C-d` guard in `app.rs` is `!in_input_mode`, which for this view is
    /// `is_input_mode()`. Opening the new-barn wizard from the barns panel must
    /// set it — otherwise `C-d` typed into the name field would disconnect the
    /// barn still selected behind the form.
    #[test]
    fn the_new_barn_wizard_puts_the_dashboard_in_input_mode() {
        let mut dash = GlobalDashboard::new();
        let barns = [barn("guided")];

        dash.handle_input(KeyCode::Tab, &[], &barns, &[], &[]);
        dash.handle_input(KeyCode::Tab, &[], &barns, &[], &[]);
        assert_eq!(dash.focused_barn_index(), Some(0));
        assert!(!dash.is_input_mode());

        dash.handle_input(KeyCode::Char('n'), &[], &barns, &[], &[]);
        assert!(dash.is_input_mode());
    }
}
