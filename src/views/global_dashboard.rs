use std::collections::{HashMap, HashSet};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::app::DashboardAction;
use crate::components::header;
use crate::components::list::{self, ListItem, ListState, ItemStatus, RowAction, RowStyle};
use crate::components::panel::Panel;
use crate::components::path_input::{self, PathInputState, PathInputAction};
use crate::components::text_input::TextInput;
use crate::config;
use crate::remote_grid::RemoteFrame;
use crate::signals;
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
        remote: &HashMap<String, RemoteFrame>,
        stale: &HashSet<&str>,
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

        // The sessions list, built from the same inputs `render` builds it from
        // so the key drawn on a row and the key that resolves to it cannot
        // disagree. Below the panel-cycling keys because it reads the ranch for
        // each claude window's signal, and `Tab` has no use for any of it.
        let rows = build_session_rows(windows, remote, stale);
        let items = row_items(&rows);

        // The session keys, from any panel: `1`-`9` for this machine's windows,
        // `A`-`Z` (less the four already bound) for a barn's. Two namespaces,
        // one lookup, and it is the row itself that says which key it answers
        // to — so a barn waking up cannot renumber a local window, and a key
        // that matches nothing falls through to the focused panel untouched.
        if let KeyCode::Char(c) = key {
            if let Some(action) = rows.iter().find(|r| r.key == Some(c)).and_then(SessionRow::action)
            {
                return action;
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
                    // The third verb, next to shell and connect. `c` opens a
                    // barn's whole TUI; this asks only for its sessions on the
                    // grid, which is a different question with a different
                    // answer for most barns.
                    KeyCode::Char('t') => return DashboardAction::ToggleTunnel(self.barns_state.selected),
                    KeyCode::Char('d') => return DashboardAction::RequestDeleteBarn(self.barns_state.selected),
                    _ => {}
                }
            }
            FocusedPanel::Sessions => {
                // The `_selectable` forms throughout: this list holds barn
                // headings, and a cursor that can stop on one is a cursor
                // `Enter` has no answer for.
                match key {
                    KeyCode::Char('j') | KeyCode::Down => self.sessions_state.select_next_selectable(&items),
                    KeyCode::Char('k') | KeyCode::Up => self.sessions_state.select_prev_selectable(&items),
                    KeyCode::Char('g') => self.sessions_state.select_first_selectable(&items),
                    KeyCode::Char('G') => self.sessions_state.select_last_selectable(&items),
                    KeyCode::Enter => {
                        // The list is rebuilt from a live stream, so the row
                        // under the cursor may have become a heading since the
                        // last keypress.
                        self.sessions_state.settle_on_selectable(&items);
                        if let Some(action) =
                            rows.get(self.sessions_state.selected).and_then(SessionRow::action)
                        {
                            return action;
                        }
                    }
                    KeyCode::Char('d') => {
                        self.sessions_state.settle_on_selectable(&items);
                        if let Some(index) =
                            rows.get(self.sessions_state.selected).and_then(|r| r.viewer)
                        {
                            return DashboardAction::CloseBarnWindow(index);
                        }
                    }
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
        remote: &HashMap<String, RemoteFrame>,
        stale: &HashSet<&str>,
    ) {

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
            // The two namespaces, named where the list is. A capital letter is
            // not a key anyone would try unprompted.
            hints: Some("[1-9] here  [A-Z] barn  [d] close"),
        };
        let sessions_inner = sessions_panel.render(frame, right_panels[0]);
        let session_items = row_items(&build_session_rows(windows, remote, stale));
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
            ..Default::default()
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
        // Additive and `Some(true)`-only for the same reason `synced` is: a barn
        // switched off is not a barn never asked about. It sits before
        // `connected` because it is a standing preference of this machine, where
        // `connected` is whether a session exists this second — and since
        // `tunneled` is now what decides whether a barn's sessions reach the
        // grid, a user who cannot see it cannot tell an empty grid from a barn
        // they never switched on.
        if b.tunneled == Some(true) {
            parts.push("tunneled".to_string());
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
            ..Default::default()
        }
    }).collect()
}

// ===========================================================================
// The sessions list
// ===========================================================================
//
// One list covering the whole ranch: this machine's windows first, then every
// tunneled barn's, under a heading of its own. The two halves are numbered in
// **separate namespaces** on purpose — digits here, capitals there — because a
// barn waking up or dying must never renumber the `[3]` the user is reaching
// for. That is also why the local rows are always first and the barns follow in
// name order: the same discipline `session_grid::cells` keeps, for the same
// reason.

/// The keys the remote rows are handed, in assignment order.
///
/// `G` (select-last, in every panel of every list view), `N` (new item, and
/// "no" in the confirm dialog), `Q` (quit) and `Y` ("yes") are already bound
/// where this list is on screen, so they are skipped rather than shadowed —
/// a row that stole `Q` would be a row that stopped the user quitting.
///
/// Twenty-two rows is not a limit worth engineering around: a ranch that
/// tunnels more remote sessions than that has a grid for it. Rows past the end
/// still list and still answer `Enter`; they simply have no key of their own.
pub(crate) const REMOTE_KEYS: [char; 22] = [
    'A', 'B', 'C', 'D', 'E', 'F', 'H', 'I', 'J', 'K', 'L', 'M', 'O', 'P', 'R', 'S', 'T', 'U', 'V',
    'W', 'X', 'Z',
];

/// The most local rows that get a digit. Ten digits, and `0` is the dashboard's
/// own window.
const LOCAL_KEYS: usize = 9;

/// What a session row opens.
#[derive(Debug, Clone, PartialEq)]
enum SessionTarget {
    /// A window of this machine's yeehaw session, by **tmux window index**.
    ///
    /// The index, never a position in the list: a `barn-view` window is drawn
    /// under its barn rather than here, so the list and `tmux list-windows` no
    /// longer agree on positions and a position would land on a neighbour.
    Local(u32),
    /// A window on a barn. Opening it is a local viewer window onto that barn's
    /// session — [`crate::tmux::open_barn_window`], the same path a number key
    /// on the grid takes.
    Remote { barn: String, window_index: u32 },
}

/// One row of the sessions panel: what it draws, what key opens it, and what
/// that key opens.
///
/// The three travel together rather than as parallel lists because the panel is
/// rebuilt from a live stream on every frame, and a key drawn on one row that
/// resolves against another is the one failure mode this design cannot have.
struct SessionRow {
    item: ListItem,
    /// The key that opens this row. `None` for a heading, and for rows past the
    /// end of a namespace.
    key: Option<char>,
    /// `None` for a heading — the one row `Enter` has no answer for, which is
    /// why headings are also [`RowStyle::Heading`] and unselectable.
    target: Option<SessionTarget>,
    /// The **local** tmux window index of the viewer this row has open, if it
    /// has one. `None` for every other row, and that `None` is what `d` refuses
    /// on.
    viewer: Option<u32>,
}

impl SessionRow {
    fn action(&self) -> Option<DashboardAction> {
        match self.target.clone()? {
            SessionTarget::Local(window_index) => Some(DashboardAction::SelectWindow(window_index)),
            SessionTarget::Remote { barn, window_index } => {
                Some(DashboardAction::OpenBarnWindow { barn, window_index })
            }
        }
    }
}

fn row_items(rows: &[SessionRow]) -> Vec<ListItem> {
    rows.iter().map(|r| r.item.clone()).collect()
}

/// The whole ranch's sessions, in display order.
///
/// `remote` is keyed by barn name and holds each barn's **last** frame, which
/// `remote_grid::RemoteStreams` keeps past a failure on purpose — so a barn
/// whose stream died keeps its rows, marked stale, instead of having them
/// vanish and renumber every row after them. `stale` names those barns.
///
/// Driven off the frames rather than off `Barn.tunneled` because the two agree
/// by construction: `reconcile_at` drops the frame of any barn that is switched
/// off, so a frame exists only for a barn the user asked for.
fn build_session_rows(
    windows: &[TmuxWindow],
    remote: &HashMap<String, RemoteFrame>,
    stale: &HashSet<&str>,
) -> Vec<SessionRow> {
    // Name order, and sorted before anything is built. `remote` is a `HashMap`,
    // so its iteration order is arbitrary from one frame to the next — this is
    // not a tidy-up, it is the whole reason a letter stays on the row it was
    // drawn on.
    let mut names: Vec<&str> = remote.keys().map(String::as_str).collect();
    names.sort_unstable();

    // Settled before the local half is built, because the local half's one
    // question is whether a `barn-view` window already has a row under a barn.
    let listed_remotely: HashSet<(&str, u32)> = remote
        .iter()
        .flat_map(|(barn, frame)| {
            frame
                .windows
                .iter()
                .filter(|w| w.index > 0)
                .map(move |w| (barn.as_str(), w.index))
        })
        .collect();

    let mut rows: Vec<SessionRow> = Vec::new();

    // This machine's windows, first and unchanged. First is not a cosmetic
    // choice: it is what keeps a barn waking up or dying from moving `[3]`.
    for w in windows.iter().filter(|w| w.index > 0) {
        if folded_into_a_barn(w, &listed_remotely) {
            continue;
        }
        let position = rows.len();
        rows.push(SessionRow {
            item: local_item(position, w),
            key: local_key(position),
            target: Some(SessionTarget::Local(w.index)),
            // `is_barn_view`, and nothing weaker. A local row is the user's own
            // work — a claude session with something unsaved in it, a shell, an
            // ssh — and `d` is `RequestDeleteProject` one panel to the left. The
            // only local row it may touch is one tmux itself says is a view:
            // a viewer whose remote row is gone (the session closed, or the barn
            // stopped streaming) is listed here rather than under a heading, and
            // is otherwise the one viewer with no way to be closed at all.
            viewer: tmux::is_barn_view(w).then_some(w.index),
        });
    }

    // Then each barn, under a heading of its own.
    let mut letters = REMOTE_KEYS.iter().copied();
    for name in names {
        let Some(frame) = remote.get(name) else { continue };
        let is_stale = stale.contains(name);
        let mut barn_windows: Vec<&TmuxWindow> =
            frame.windows.iter().filter(|w| w.index > 0).collect();
        barn_windows.sort_by_key(|w| w.index);

        rows.push(heading_row(name, barn_windows.len(), is_stale));

        for w in barn_windows {
            // "Open here" is asked of the *whole* local window list, including
            // the viewers that were just folded out of it above — folding one
            // away is what proves it is open, not a reason to stop counting it.
            //
            // The window's local **index** is kept, not just the yes/no: it is
            // what `d` closes, and it is the one number that cannot be derived
            // from this row, which is labelled with a remote index belonging to
            // another machine.
            let viewer = windows
                .iter()
                .find(|local| tmux::views_remote_window(local, name, w.index))
                .map(|local| local.index);
            let open_here = viewer.is_some();
            let key = letters.next();
            rows.push(SessionRow {
                item: remote_item(key, w, frame, open_here, is_stale),
                key,
                target: Some(SessionTarget::Remote {
                    barn: name.to_string(),
                    window_index: w.index,
                }),
                viewer,
            });
        }
    }

    rows
}

/// Is this local window already listed under a barn, as a view of one of its
/// sessions?
///
/// Per remote *window*, never per barn. A viewer whose remote session has since
/// closed — or one onto a barn that is not streaming at all — has no row under
/// a heading to be folded into, and dropping it would be the only sign of that
/// session anywhere disappearing.
fn folded_into_a_barn(w: &TmuxWindow, listed_remotely: &HashSet<(&str, u32)>) -> bool {
    tmux::is_barn_view(w)
        && w.remote_window.is_some_and(|index| listed_remotely.contains(&(w.barn.as_str(), index)))
}

/// The digit a local row answers to, if it is within the first nine.
fn local_key(position: usize) -> Option<char> {
    if position >= LOCAL_KEYS {
        return None;
    }
    char::from_digit(position as u32 + 1, 10)
}

/// What a row whose barn has stopped answering says.
///
/// The rows themselves stay: `remote_grid::RemoteStreams` keeps a barn's last
/// frame past a failure for exactly this, because dropping the rows would
/// renumber every row after them.
const STALE: &str = "stale";

/// The heading a barn's sessions sit under.
fn heading_row(barn: &str, sessions: usize, is_stale: bool) -> SessionRow {
    let mut parts = vec![match sessions {
        0 => "no sessions".to_string(),
        1 => "1 session".to_string(),
        n => format!("{n} sessions"),
    }];
    if is_stale {
        parts.push(STALE.to_string());
    }
    SessionRow {
        item: ListItem {
            id: format!("barn:{barn}"),
            label: barn.to_string(),
            meta: Some(parts.join(" · ")),
            style: RowStyle::Heading,
            ..Default::default()
        },
        key: None,
        target: None,
        viewer: None,
    }
}

/// A window on a barn.
///
/// Indented through the label rather than through a second field: the row's
/// place in the list is already carried by the heading above it, and a width
/// the list component had to know about would have to be undone by every other
/// view.
fn remote_item(
    key: Option<char>,
    w: &TmuxWindow,
    frame: &RemoteFrame,
    open_here: bool,
    is_stale: bool,
) -> ListItem {
    // The same four columns whether or not there is a key, so a row past the
    // end of the alphabet still lines up under the ones above it.
    let chip = match key {
        Some(c) => format!("[{c}] "),
        None => "    ".to_string(),
    };

    let mut parts: Vec<String> = Vec::new();
    if !w.window_type.is_empty() {
        parts.push(w.window_type.clone());
    }
    if let Some(word) = remote_status_word(frame, w) {
        parts.push(word.to_string());
    }
    if is_stale {
        parts.push(STALE.to_string());
    }

    ListItem {
        id: format!("{}:{}", frame.barn, w.index),
        label: format!("  {chip}{}", w.name),
        status: Some(if w.active { ItemStatus::Active } else { ItemStatus::Inactive }),
        meta: (!parts.is_empty()).then(|| parts.join(" · ")),
        // The third state, and the only one a glance has to catch: greyed is a
        // session that is running on a barn and is *not* on this machine.
        style: if open_here { RowStyle::Normal } else { RowStyle::Muted },
        ..Default::default()
    }
}

/// What a remote session is doing, from the barn's own signal file.
///
/// `frame.fresh_signal`, never `tmux::get_window_status`: that one reads *this*
/// machine's `session-signals` directory by pane id, and pane ids collide
/// across hosts — `%1` exists everywhere — so it would answer a remote row with
/// a local session's status. `fresh_signal` also measures age against the
/// barn's clock, so a barn running a few minutes behind is not reported as
/// having nothing to say.
fn remote_status_word(frame: &RemoteFrame, w: &TmuxWindow) -> Option<&'static str> {
    Some(match frame.fresh_signal(&w.pane_id)?.status {
        signals::SessionStatus::Working => "working",
        signals::SessionStatus::Waiting => "waiting for input",
        signals::SessionStatus::Idle => "idle",
        signals::SessionStatus::Error => "error",
    })
}

/// A window of this machine's session. Unchanged from before the merge, down to
/// the `[n]` that keeps counting past the last key.
fn local_item(position: usize, w: &TmuxWindow) -> ListItem {
    // Type comes from the @yeehaw_type tmux option, not the window name.
    // The old name-sniffing decoder mislabelled anything it did not expect.
    let status_info = tmux::get_window_status(w);
    let meta_text = if !w.window_type.is_empty() {
        format!("{} · {}", w.window_type, status_info.text)
    } else {
        status_info.text
    };
    ListItem {
        id: w.index.to_string(),
        label: format!("[{}] {}", position + 1, w.name),
        status: Some(if w.active { ItemStatus::Active } else { ItemStatus::Inactive }),
        meta: Some(meta_text),
        ..Default::default()
    }
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
            ..Default::default()
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

    // --- the remote half of the sessions panel ------------------------------

    /// Nothing tunneled, which is what every test written before the merge
    /// assumes and what most ranches look like most of the time.
    fn no_frames() -> HashMap<String, RemoteFrame> {
        HashMap::new()
    }

    fn no_stale() -> HashSet<&'static str> {
        HashSet::new()
    }

    fn stale_set<'a>(names: &[&'a str]) -> HashSet<&'a str> {
        names.iter().copied().collect()
    }

    /// A window as `list-windows` reports it, local or remote.
    fn win(index: u32, name: &str, ty: &str) -> TmuxWindow {
        TmuxWindow {
            index,
            name: name.into(),
            pane_id: format!("%{index}"),
            window_type: ty.into(),
            barn: "local".into(),
            ..Default::default()
        }
    }

    /// A local window that is a view onto `barn`'s window `remote`.
    fn viewer(index: u32, barn: &str, remote: u32) -> TmuxWindow {
        TmuxWindow {
            index,
            name: format!("{barn}-{remote}"),
            pane_id: format!("%{index}"),
            window_type: tmux::VIEWER_WINDOW_TYPE.into(),
            barn: barn.into(),
            remote_window: Some(remote),
            ..Default::default()
        }
    }

    /// One barn's last frame. `barn_now` is far enough ahead of the signal
    /// clock that nothing is judged fresh unless a test says so.
    fn frame(barn: &str, windows: Vec<TmuxWindow>) -> RemoteFrame {
        RemoteFrame {
            barn: barn.into(),
            windows,
            captures: HashMap::new(),
            signals: HashMap::new(),
            barn_now: 1_000,
        }
    }

    fn frames(fs: Vec<RemoteFrame>) -> HashMap<String, RemoteFrame> {
        fs.into_iter().map(|f| (f.barn.clone(), f)).collect()
    }

    /// Just the labels, which is where the numbering and the indentation live.
    fn labels(rows: &[SessionRow]) -> Vec<String> {
        rows.iter().map(|r| r.item.label.clone()).collect()
    }

    fn keys(rows: &[SessionRow]) -> Vec<Option<char>> {
        rows.iter().map(|r| r.key).collect()
    }

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

    // === the tunnel toggle ==================================================
    //
    // Streaming a barn's sessions into the grid used to require having run
    // `connect` on it, which conflates "I opened that machine's whole TUI" with
    // "I want to see its sessions". `t` is the second question, asked on its own.

    /// `t` is the barns panel's third verb, next to `s` (shell) and `c`
    /// (connect). Without a key there is no way to want a barn's sessions
    /// without opening its whole TUI.
    #[test]
    fn t_on_the_barns_panel_asks_to_toggle_the_selected_barns_tunnel() {
        let _ranch = crate::testing::temp_ranch();
        let mut dash = GlobalDashboard::new();
        let barns = [barn("guided"), barn("smash-mac")];

        // Tab cycles Projects -> Sessions -> Barns.
        dash.handle_input(KeyCode::Tab, &[], &barns, &[], &[], &no_frames(), &no_stale());
        dash.handle_input(KeyCode::Tab, &[], &barns, &[], &[], &no_frames(), &no_stale());
        dash.handle_input(KeyCode::Char('j'), &[], &barns, &[], &[], &no_frames(), &no_stale());

        let action = dash.handle_input(KeyCode::Char('t'), &[], &barns, &[], &[], &no_frames(), &no_stale());

        assert!(
            matches!(action, DashboardAction::ToggleTunnel(1)),
            "`t` must toggle the *selected* barn"
        );
    }

    /// `t` anywhere else must not reach the barns panel, the same discipline
    /// every other panel-scoped verb keeps.
    #[test]
    fn t_outside_the_barns_panel_is_not_a_tunnel_toggle() {
        let _ranch = crate::testing::temp_ranch();
        let mut dash = GlobalDashboard::new();
        let barns = [barn("guided")];

        // Focus starts on Projects.
        let action = dash.handle_input(KeyCode::Char('t'), &[], &barns, &[], &[], &no_frames(), &no_stale());

        assert!(
            !matches!(action, DashboardAction::ToggleTunnel(_)),
            "the projects panel toggled a barn's tunnel"
        );
    }

    /// The toggle is invisible unless the row says so — and `tunneled` is now
    /// what decides whether a barn's sessions reach the grid, so a user who
    /// cannot see the state cannot tell an empty grid from a barn they never
    /// switched on.
    #[test]
    fn a_tunneled_barn_says_so_on_its_row() {
        let _ranch = crate::testing::temp_ranch();
        let mut wanted = barn("guided");
        wanted.tunneled = Some(true);
        let mut declined = barn("smash-mac");
        declined.tunneled = Some(false);

        let items = build_barn_items(&[wanted, declined, barn("never-asked")], &HashSet::new());

        assert!(items[0].meta.as_deref().unwrap().contains("tunneled"), "{:?}", items[0].meta);
        assert!(
            !items[1].meta.as_deref().unwrap().contains("tunneled"),
            "`Some(false)` is a barn this machine has switched off: {:?}",
            items[1].meta
        );
        assert!(
            !items[2].meta.as_deref().unwrap().contains("tunneled"),
            "and absent is not `false` dressed up as an answer: {:?}",
            items[2].meta
        );
    }

    /// The marker is additive like `ranch house` and `synced`, and it must not
    /// displace them or the connection.
    #[test]
    fn the_tunnel_marker_composes_with_the_others() {
        let _ranch = crate::testing::temp_ranch();
        let mut house = barn("camerons-imac");
        house.is_ranch_house = Some(true);
        house.synced = Some(true);
        house.tunneled = Some(true);
        let set = connected(&[tmux::barn_session_name("camerons-imac")]);

        let items = build_barn_items(&[house], &set);

        let meta = items[0].meta.clone().unwrap();
        for expected in ["forge@172.233.141.59", "ranch house", "synced", "tunneled", "connected"] {
            assert!(meta.contains(expected), "{:?} missing from {:?}", expected, meta);
        }
        assert_eq!(
            items[0].status,
            Some(ItemStatus::Active),
            "the dot is still the connection and nothing else"
        );
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
        dash.handle_input(KeyCode::Tab, &[], &barns, &[], &[], &no_frames(), &no_stale());
        assert_eq!(dash.focused_barn_index(), None);
        dash.handle_input(KeyCode::Tab, &[], &barns, &[], &[], &no_frames(), &no_stale());
        assert_eq!(dash.focused_barn_index(), Some(0));

        // It tracks the selection, not just the panel.
        dash.handle_input(KeyCode::Char('j'), &[], &barns, &[], &[], &no_frames(), &no_stale());
        assert_eq!(dash.focused_barn_index(), Some(1));

        // Worms is next: focus leaves the panel and so does the index.
        dash.handle_input(KeyCode::Tab, &[], &barns, &[], &[], &no_frames(), &no_stale());
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

        dash.handle_input(KeyCode::Tab, &[], &barns, &[], &[], &no_frames(), &no_stale());
        dash.handle_input(KeyCode::Tab, &[], &barns, &[], &[], &no_frames(), &no_stale());
        assert_eq!(dash.focused_barn_index(), Some(0));
        assert!(!dash.is_input_mode());

        dash.handle_input(KeyCode::Char('n'), &[], &barns, &[], &[], &no_frames(), &no_stale());
        assert!(dash.is_input_mode());
    }

    // === the whole ranch in one sessions panel ==============================
    //
    // The panel listed this machine's windows and nothing else, while the grid
    // — a keypress away — had every tunneled barn's. The two answers to "what
    // is running" disagreed, and the dashboard's was the one that was wrong.
    //
    // No fixture below carries a host or an address. `ssh::dial_host` falls
    // back to `addresses`, and this file's tests run beside a live ranch.

    /// The control. Nothing in this section can reach a real machine even if a
    /// branch went wrong and tried.
    #[test]
    fn the_session_fixtures_have_nothing_to_dial() {
        let b = Barn { name: "guided".into(), ..Default::default() };
        assert!(b.addresses.is_empty());
        assert_eq!(ssh::dial_host(&b), None, "a fixture that could open a real ssh");
    }

    /// The half that already worked. A barn arriving must not touch it.
    #[test]
    fn local_windows_keep_their_numbers_when_a_barn_arrives() {
        // `get_window_status` reads the ranch for a claude window's signal.
        // Harness only — no assertion below depends on what is in it.
        let _ranch = crate::testing::temp_ranch();
        let local = [win(1, "api-claude", "claude"), win(2, "web-shell", "shell")];

        let alone = build_session_rows(&local, &no_frames(), &no_stale());
        assert_eq!(labels(&alone), ["[1] api-claude", "[2] web-shell"]);
        assert_eq!(keys(&alone), [Some('1'), Some('2')]);

        let with_barn = build_session_rows(
            &local,
            &frames(vec![frame("guided", vec![win(3, "deploy", "claude")])]),
            &no_stale(),
        );

        assert_eq!(
            labels(&with_barn)[..2],
            ["[1] api-claude", "[2] web-shell"],
            "a barn waking up renumbered the local windows"
        );
        assert_eq!(keys(&with_barn)[..2], [Some('1'), Some('2')]);
    }

    /// Window 0 is the dashboard itself and has never been a session row.
    #[test]
    fn the_dashboards_own_window_is_not_a_session() {
        // `get_window_status` reads the ranch for a claude window's signal.
        // Harness only — no assertion below depends on what is in it.
        let _ranch = crate::testing::temp_ranch();
        let rows = build_session_rows(&[win(0, "yeehaw", ""), win(1, "api", "claude")], &no_frames(), &no_stale());

        assert_eq!(labels(&rows), ["[1] api"]);
    }

    #[test]
    fn a_tunneled_barns_windows_sit_under_a_heading_carrying_its_name() {
        // `get_window_status` reads the ranch for a claude window's signal.
        // Harness only — no assertion below depends on what is in it.
        let _ranch = crate::testing::temp_ranch();
        let rows = build_session_rows(
            &[win(1, "local-claude", "claude")],
            &frames(vec![frame("guided", vec![win(2, "api", "claude"), win(5, "web", "shell")])]),
            &no_stale(),
        );

        assert_eq!(rows.len(), 4, "{:?}", labels(&rows));
        assert_eq!(rows[1].item.label, "guided", "the heading must carry the barn name");
        assert_eq!(rows[1].item.style, RowStyle::Heading);
        assert!(rows[1].target.is_none(), "a heading opens nothing");
        assert!(
            rows[2].item.label.starts_with("  ") && rows[3].item.label.starts_with("  "),
            "a barn's windows must be indented under it: {:?}",
            labels(&rows)
        );
    }

    /// Local first, then barns in name order. The same discipline the grid's
    /// cells keep, and for the same reason: a `HashMap` iterates in whatever
    /// order it likes, and rows that reshuffle are keys that reshuffle.
    #[test]
    fn barns_are_listed_in_name_order_however_the_frames_arrived() {
        // `get_window_status` reads the ranch for a claude window's signal.
        // Harness only — no assertion below depends on what is in it.
        let _ranch = crate::testing::temp_ranch();
        let one = build_session_rows(
            &[],
            &frames(vec![
                frame("zeta", vec![win(1, "z", "claude")]),
                frame("alpha", vec![win(1, "a", "claude")]),
            ]),
            &no_stale(),
        );
        let other = build_session_rows(
            &[],
            &frames(vec![
                frame("alpha", vec![win(1, "a", "claude")]),
                frame("zeta", vec![win(1, "z", "claude")]),
            ]),
            &no_stale(),
        );

        assert_eq!(labels(&one), labels(&other));
        assert_eq!(one[0].item.label, "alpha");
        assert_eq!(one[2].item.label, "zeta");
    }

    #[test]
    fn a_barns_windows_are_listed_in_window_order() {
        // `get_window_status` reads the ranch for a claude window's signal.
        // Harness only — no assertion below depends on what is in it.
        let _ranch = crate::testing::temp_ranch();
        let rows = build_session_rows(
            &[],
            &frames(vec![frame(
                "guided",
                vec![win(9, "last", "claude"), win(2, "first", "claude")],
            )]),
            &no_stale(),
        );

        assert_eq!(labels(&rows)[1..], ["  [A] first", "  [B] last"]);
    }

    // === the two key namespaces =============================================

    #[test]
    fn remote_rows_are_keyed_by_capitals_and_never_by_a_key_the_tui_already_owns() {
        // `get_window_status` reads the ranch for a claude window's signal.
        // Harness only — no assertion below depends on what is in it.
        let _ranch = crate::testing::temp_ranch();
        // Enough windows to run past where G, N, Q and Y would have fallen.
        let windows: Vec<TmuxWindow> =
            (1..=26).map(|i| win(i, &format!("w{i}"), "claude")).collect();
        let rows =
            build_session_rows(&[], &frames(vec![frame("guided", windows)]), &no_stale());

        let assigned: Vec<char> = rows.iter().filter_map(|r| r.key).collect();

        for bound in ['G', 'N', 'Q', 'Y'] {
            assert!(
                !assigned.contains(&bound),
                "{bound} is already bound on the dashboard: {assigned:?}"
            );
        }
        assert_eq!(assigned, REMOTE_KEYS.to_vec());
        assert!(assigned.iter().all(|c| c.is_ascii_uppercase()));
    }

    /// Digits are this machine's, capitals are the barns'. Neither namespace
    /// may spill into the other, or a barn coming or going moves the key the
    /// user's fingers already know.
    #[test]
    fn the_two_namespaces_never_overlap() {
        // `get_window_status` reads the ranch for a claude window's signal.
        // Harness only — no assertion below depends on what is in it.
        let _ranch = crate::testing::temp_ranch();
        let rows = build_session_rows(
            &[win(1, "local", "claude")],
            &frames(vec![frame("guided", vec![win(4, "api", "claude")])]),
            &no_stale(),
        );

        assert_eq!(rows[0].key, Some('1'));
        assert_eq!(rows[1].key, None, "a heading has no key");
        assert_eq!(rows[2].key, Some('A'));
    }

    /// Past the end of the alphabet a row still lists and still answers
    /// `Enter`; it simply has no key. Dropping it would renumber everything
    /// after it, which is the one thing this list may not do.
    #[test]
    fn a_row_past_the_last_letter_still_lists_and_still_opens() {
        // `get_window_status` reads the ranch for a claude window's signal.
        // Harness only — no assertion below depends on what is in it.
        let _ranch = crate::testing::temp_ranch();
        let windows: Vec<TmuxWindow> =
            (1..=24).map(|i| win(i, &format!("w{i}"), "claude")).collect();
        let rows =
            build_session_rows(&[], &frames(vec![frame("guided", windows)]), &no_stale());

        let last = rows.last().expect("rows");
        assert_eq!(last.key, None);
        assert!(last.item.selectable());
        assert!(
            matches!(last.target, Some(SessionTarget::Remote { window_index: 24, .. })),
            "{:?}",
            last.target
        );
    }

    /// Ten or more local windows have always numbered past `[9]` with no key.
    #[test]
    fn local_rows_past_the_ninth_keep_their_number_and_lose_only_the_key() {
        // `get_window_status` reads the ranch for a claude window's signal.
        // Harness only — no assertion below depends on what is in it.
        let _ranch = crate::testing::temp_ranch();
        let windows: Vec<TmuxWindow> =
            (1..=11).map(|i| win(i, &format!("w{i}"), "claude")).collect();
        let rows = build_session_rows(&windows, &no_frames(), &no_stale());

        assert_eq!(rows[8].key, Some('9'));
        assert_eq!(rows[9].key, None);
        assert_eq!(rows[9].item.label, "[10] w10");
    }

    // === the three states of a remote row ===================================

    #[test]
    fn a_remote_session_not_open_on_this_machine_is_greyed() {
        // `get_window_status` reads the ranch for a claude window's signal.
        // Harness only — no assertion below depends on what is in it.
        let _ranch = crate::testing::temp_ranch();
        let rows = build_session_rows(
            &[],
            &frames(vec![frame("guided", vec![win(4, "api", "claude")])]),
            &no_stale(),
        );

        assert_eq!(rows[1].item.style, RowStyle::Muted);
    }

    #[test]
    fn a_remote_session_already_open_here_is_drawn_as_an_ordinary_row() {
        // `get_window_status` reads the ranch for a claude window's signal.
        // Harness only — no assertion below depends on what is in it.
        let _ranch = crate::testing::temp_ranch();
        let rows = build_session_rows(
            &[viewer(3, "guided", 4)],
            &frames(vec![frame("guided", vec![win(4, "api", "claude")])]),
            &no_stale(),
        );

        let remote: Vec<&SessionRow> = rows.iter().filter(|r| r.key == Some('A')).collect();
        assert_eq!(remote.len(), 1);
        assert_eq!(remote[0].item.style, RowStyle::Normal, "an open session still reads as closed");
    }

    /// One viewer must not light up the whole barn — that is the entire reason
    /// the viewer window carries the remote window index and not just the barn.
    #[test]
    fn opening_one_of_a_barns_sessions_does_not_mark_the_rest_open() {
        // `get_window_status` reads the ranch for a claude window's signal.
        // Harness only — no assertion below depends on what is in it.
        let _ranch = crate::testing::temp_ranch();
        let rows = build_session_rows(
            &[viewer(3, "guided", 4)],
            &frames(vec![frame("guided", vec![win(4, "api", "claude"), win(7, "web", "shell")])]),
            &no_stale(),
        );

        // The viewer itself is folded under the barn, so the barn's heading is
        // row 0 and its two sessions follow it.
        assert_eq!(labels(&rows), ["guided", "  [A] api", "  [B] web"]);
        assert_eq!(rows[1].item.style, RowStyle::Normal);
        assert_eq!(rows[2].item.style, RowStyle::Muted, "a neighbour read as open");
    }

    /// A barn whose stream died keeps its rows. Dropping them would renumber
    /// every row after them — which is exactly what `remote_grid` keeps the
    /// last frame to avoid.
    #[test]
    fn a_dead_barns_rows_stay_where_they_were_and_say_stale() {
        // `get_window_status` reads the ranch for a claude window's signal.
        // Harness only — no assertion below depends on what is in it.
        let _ranch = crate::testing::temp_ranch();
        let remote = frames(vec![
            frame("guided", vec![win(4, "api", "claude")]),
            frame("smash-mac", vec![win(2, "web", "shell")]),
        ]);

        let live = build_session_rows(&[], &remote, &no_stale());
        let dead = build_session_rows(&[], &remote, &stale_set(&["guided"]));

        assert_eq!(labels(&live), labels(&dead), "a dead barn renumbered the ranch");
        assert_eq!(keys(&live), keys(&dead));
        assert!(
            dead[1].item.meta.as_deref().unwrap_or_default().contains("stale"),
            "a row nobody can reach has to say so: {:?}",
            dead[1].item.meta
        );
        assert!(
            !dead[3].item.meta.as_deref().unwrap_or_default().contains("stale"),
            "and only the dead barn's: {:?}",
            dead[3].item.meta
        );
    }

    #[test]
    fn a_dead_barns_heading_says_so_too() {
        // `get_window_status` reads the ranch for a claude window's signal.
        // Harness only — no assertion below depends on what is in it.
        let _ranch = crate::testing::temp_ranch();
        let rows = build_session_rows(
            &[],
            &frames(vec![frame("guided", vec![win(4, "api", "claude")])]),
            &stale_set(&["guided"]),
        );

        assert!(
            rows[0].item.meta.as_deref().unwrap_or_default().contains("stale"),
            "{:?}",
            rows[0].item.meta
        );
    }

    // === the double count ===================================================
    //
    // A `barn-view` window is two things at once: a window of this machine's
    // session, and a view of one of a barn's. Listed as both it is the same
    // work twice, under two different keys.

    #[test]
    fn a_window_viewing_a_barn_is_listed_under_that_barn_and_not_as_local_work() {
        // `get_window_status` reads the ranch for a claude window's signal.
        // Harness only — no assertion below depends on what is in it.
        let _ranch = crate::testing::temp_ranch();
        let rows = build_session_rows(
            &[win(1, "api-claude", "claude"), viewer(2, "guided", 4)],
            &frames(vec![frame("guided", vec![win(4, "api", "claude")])]),
            &no_stale(),
        );

        assert_eq!(
            labels(&rows),
            ["[1] api-claude", "guided", "  [A] api"],
            "the viewer was counted twice"
        );
    }

    /// The de-duplication is per remote *window*, not per barn: a viewer whose
    /// remote session has since closed has no row under the barn to be folded
    /// into, and must not simply disappear.
    #[test]
    fn a_viewer_with_no_row_under_its_barn_is_still_listed_locally() {
        // `get_window_status` reads the ranch for a claude window's signal.
        // Harness only — no assertion below depends on what is in it.
        let _ranch = crate::testing::temp_ranch();
        let rows = build_session_rows(
            &[viewer(2, "guided", 99)],
            &frames(vec![frame("guided", vec![win(4, "api", "claude")])]),
            &no_stale(),
        );

        assert_eq!(
            labels(&rows),
            ["[1] guided-99", "guided", "  [A] api"],
            "a viewer onto a session the barn no longer has vanished"
        );
    }

    /// Same, for a barn that is not tunneled at all: nothing lists its windows,
    /// so the viewer is the only sign of that session anywhere.
    #[test]
    fn a_viewer_onto_a_barn_that_is_not_streaming_is_still_listed_locally() {
        // `get_window_status` reads the ranch for a claude window's signal.
        // Harness only — no assertion below depends on what is in it.
        let _ranch = crate::testing::temp_ranch();
        let rows = build_session_rows(&[viewer(2, "guided", 4)], &no_frames(), &no_stale());

        assert_eq!(labels(&rows), ["[1] guided-4"]);
    }

    // === what the keys do ===================================================

    fn dash_on(windows: &[TmuxWindow], remote: &HashMap<String, RemoteFrame>) -> GlobalDashboard {
        let _ = (windows, remote);
        GlobalDashboard::new()
    }

    #[test]
    fn a_capital_letter_opens_that_barns_window() {
        let _ranch = crate::testing::temp_ranch();
        let windows = [win(1, "local", "claude")];
        let remote = frames(vec![frame("guided", vec![win(4, "api", "claude")])]);
        let mut dash = dash_on(&windows, &remote);

        let action =
            dash.handle_input(KeyCode::Char('A'), &[], &[], &[], &windows, &remote, &no_stale());

        assert!(
            matches!(
                &action,
                DashboardAction::OpenBarnWindow { barn, window_index: 4 } if barn == "guided"
            ),
            "`A` did not open guided:4"
        );
    }

    /// Letters work from wherever focus is, exactly as the digits always have.
    #[test]
    fn a_capital_letter_works_from_any_panel() {
        let _ranch = crate::testing::temp_ranch();
        let remote = frames(vec![frame("guided", vec![win(4, "api", "claude")])]);
        let mut dash = GlobalDashboard::new();

        // Projects -> Sessions -> Barns.
        dash.handle_input(KeyCode::Tab, &[], &[], &[], &[], &remote, &no_stale());
        dash.handle_input(KeyCode::Tab, &[], &[], &[], &[], &remote, &no_stale());

        let action =
            dash.handle_input(KeyCode::Char('A'), &[], &[], &[], &[], &remote, &no_stale());

        assert!(matches!(action, DashboardAction::OpenBarnWindow { .. }), "focus swallowed the key");
    }

    // === closing a viewer ===================================================
    //
    // `d`, and only ever on a row the panel can prove is a `barn-view` window.
    // The blast radius is the whole point of these tests: the same key deletes a
    // project, a barn and a worm in the panels either side of this one, and a
    // `d` that reached an ordinary local row would kill a live claude session
    // and whatever was unsaved in it.

    /// Move focus to the sessions panel and put the cursor on its first
    /// selectable row.
    fn focus_sessions(
        dash: &mut GlobalDashboard,
        windows: &[TmuxWindow],
        remote: &HashMap<String, RemoteFrame>,
    ) {
        dash.handle_input(KeyCode::Tab, &[], &[], &[], windows, remote, &no_stale());
        dash.handle_input(KeyCode::Char('g'), &[], &[], &[], windows, remote, &no_stale());
    }

    fn press(
        dash: &mut GlobalDashboard,
        c: char,
        windows: &[TmuxWindow],
        remote: &HashMap<String, RemoteFrame>,
    ) -> DashboardAction {
        dash.handle_input(KeyCode::Char(c), &[], &[], &[], windows, remote, &no_stale())
    }

    /// A barn's row with a viewer open closes **that local window**, by its local
    /// index — never the remote index the row is labelled with, which belongs to
    /// a window on another machine.
    #[test]
    fn d_on_a_barn_row_with_a_viewer_open_closes_the_local_window() {
        let _ranch = crate::testing::temp_ranch();
        let windows = [win(1, "work", "claude"), viewer(5, "guided", 4)];
        let remote = frames(vec![frame("guided", vec![win(4, "api", "claude")])]);
        let mut dash = GlobalDashboard::new();

        focus_sessions(&mut dash, &windows, &remote);
        // Row 0 is the local claude window, row 1 the heading `j` steps over.
        press(&mut dash, 'j', &windows, &remote);

        let action = press(&mut dash, 'd', &windows, &remote);

        assert!(
            matches!(action, DashboardAction::CloseBarnWindow(5)),
            "`d` on an open viewer did not close local window 5: {:?}",
            labels(&build_session_rows(&windows, &remote, &no_stale()))
        );
    }

    /// The row a viewer is folded *out* of the local list into is the row that
    /// closes it, and the fold is what proves the viewer is open — so this is the
    /// same row `Enter` would open, answering the opposite question.
    #[test]
    fn d_and_enter_on_the_same_barn_row_are_the_two_halves_of_one_key_pair() {
        let _ranch = crate::testing::temp_ranch();
        let windows = [viewer(5, "guided", 4)];
        let remote = frames(vec![frame("guided", vec![win(4, "api", "claude")])]);
        let mut dash = GlobalDashboard::new();

        focus_sessions(&mut dash, &windows, &remote);

        let opened = dash.handle_input(KeyCode::Enter, &[], &[], &[], &windows, &remote, &no_stale());
        assert!(
            matches!(&opened, DashboardAction::OpenBarnWindow { barn, window_index: 4 } if barn == "guided"),
            "control: the cursor is not on guided:4"
        );
        assert!(matches!(
            press(&mut dash, 'd', &windows, &remote),
            DashboardAction::CloseBarnWindow(5)
        ));
    }

    /// **The one that matters.** A local row is a claude session, a shell or an
    /// ssh window — the user's actual work, with whatever is unsaved in it. `d`
    /// refuses, silently: there is nothing to close and nothing to warn about.
    #[test]
    fn d_on_a_local_working_session_does_nothing_at_all() {
        let _ranch = crate::testing::temp_ranch();
        let windows =
            [win(1, "work", "claude"), win(2, "shell", "shell"), win(3, "barn-guided", "ssh")];
        let remote = no_frames();
        let mut dash = GlobalDashboard::new();

        focus_sessions(&mut dash, &windows, &remote);

        for row in 0..windows.len() {
            let action = press(&mut dash, 'd', &windows, &remote);
            assert!(
                matches!(action, DashboardAction::None),
                "`d` reached local row {row} ({}) — that is a live session",
                windows[row].name,
            );
            press(&mut dash, 'j', &windows, &remote);
        }
    }

    /// A barn's session with no viewer open has no local window to close, and
    /// the remote index on the row is a window on another machine. Refused, and
    /// refused silently — the row is still a perfectly good `Enter`.
    #[test]
    fn d_on_a_barn_row_with_nothing_open_here_does_nothing() {
        let _ranch = crate::testing::temp_ranch();
        let windows: [TmuxWindow; 0] = [];
        let remote = frames(vec![frame("guided", vec![win(4, "api", "claude")])]);
        let mut dash = GlobalDashboard::new();

        focus_sessions(&mut dash, &windows, &remote);

        let action = press(&mut dash, 'd', &windows, &remote);

        assert!(matches!(action, DashboardAction::None), "`d` closed a window it does not have");
    }

    /// A viewer whose remote session has closed — or whose barn stopped
    /// streaming — has no row under a heading to be folded into, so it is listed
    /// as a local row. It is still only a view, and it is the one viewer that
    /// could otherwise never be closed at all.
    #[test]
    fn d_closes_a_viewer_that_lost_the_barn_row_it_was_folded_under() {
        let _ranch = crate::testing::temp_ranch();
        let windows = [viewer(5, "guided", 4)];
        let remote = no_frames();
        let mut dash = GlobalDashboard::new();

        focus_sessions(&mut dash, &windows, &remote);
        assert_eq!(
            labels(&build_session_rows(&windows, &remote, &no_stale())),
            ["[1] guided-4"],
            "control: the orphaned viewer is a local row"
        );

        let action = press(&mut dash, 'd', &windows, &remote);

        assert!(
            matches!(action, DashboardAction::CloseBarnWindow(5)),
            "an orphaned viewer has no way to be closed"
        );
    }

    /// `G` is select-last in every panel of this view. A barn row may not take
    /// it away.
    #[test]
    fn the_bound_capitals_still_do_what_they_always_did() {
        let _ranch = crate::testing::temp_ranch();
        let windows: Vec<TmuxWindow> =
            (1..=26).map(|i| win(i, &format!("w{i}"), "claude")).collect();
        let remote = frames(vec![frame("guided", windows)]);
        let mut dash = GlobalDashboard::new();
        let barns = [barn("guided"), barn("smash-mac")];

        // Projects -> Sessions -> Barns, where `G` is select-last.
        dash.handle_input(KeyCode::Tab, &[], &barns, &[], &[], &remote, &no_stale());
        dash.handle_input(KeyCode::Tab, &[], &barns, &[], &[], &remote, &no_stale());
        assert_eq!(dash.focused_barn_index(), Some(0), "control: focus is on the barns");

        let action =
            dash.handle_input(KeyCode::Char('G'), &[], &barns, &[], &[], &remote, &no_stale());

        assert!(matches!(action, DashboardAction::None), "`G` opened a barn window");
        assert_eq!(dash.focused_barn_index(), Some(1), "`G` stopped meaning select-last");
    }

    #[test]
    fn a_digit_still_switches_to_a_local_window_by_its_tmux_index() {
        let _ranch = crate::testing::temp_ranch();
        // tmux indexes are not positions: window 1 was closed.
        let windows = [win(2, "api", "claude"), win(7, "web", "shell")];
        let mut dash = GlobalDashboard::new();

        let action =
            dash.handle_input(KeyCode::Char('2'), &[], &[], &[], &windows, &no_frames(), &no_stale());

        assert!(matches!(action, DashboardAction::SelectWindow(7)), "{:?}", labels(&build_session_rows(&windows, &no_frames(), &no_stale())));
    }

    /// A digit past the end of the local list is not a remote row's key.
    #[test]
    fn a_digit_past_the_local_windows_reaches_nothing() {
        let _ranch = crate::testing::temp_ranch();
        let remote = frames(vec![frame("guided", vec![win(4, "api", "claude")])]);
        let mut dash = GlobalDashboard::new();

        let action =
            dash.handle_input(KeyCode::Char('3'), &[], &[], &[], &[], &remote, &no_stale());

        assert!(matches!(action, DashboardAction::None), "a digit reached a barn's row");
    }

    // === moving around the merged list ======================================

    #[test]
    fn the_cursor_steps_over_the_barn_headings() {
        let _ranch = crate::testing::temp_ranch();
        let windows = [win(1, "local", "claude")];
        let remote = frames(vec![frame("guided", vec![win(4, "api", "claude")])]);
        let mut dash = GlobalDashboard::new();

        // Projects -> Sessions.
        dash.handle_input(KeyCode::Tab, &[], &[], &[], &windows, &remote, &no_stale());
        dash.handle_input(KeyCode::Char('j'), &[], &[], &[], &windows, &remote, &no_stale());

        let action =
            dash.handle_input(KeyCode::Enter, &[], &[], &[], &windows, &remote, &no_stale());

        assert!(
            matches!(&action, DashboardAction::OpenBarnWindow { barn, window_index: 4 } if barn == "guided"),
            "one `j` off a single local row must land on the barn's session, not its heading"
        );
    }

    #[test]
    fn enter_on_a_local_row_still_switches_to_it() {
        let _ranch = crate::testing::temp_ranch();
        let windows = [win(6, "api", "claude")];
        let remote = frames(vec![frame("guided", vec![win(4, "web", "claude")])]);
        let mut dash = GlobalDashboard::new();

        dash.handle_input(KeyCode::Tab, &[], &[], &[], &windows, &remote, &no_stale());
        let action =
            dash.handle_input(KeyCode::Enter, &[], &[], &[], &windows, &remote, &no_stale());

        assert!(matches!(action, DashboardAction::SelectWindow(6)));
    }

    /// With no local sessions the list opens on a heading, so `Enter` has to
    /// find its way off one before it answers at all.
    #[test]
    fn enter_with_the_cursor_never_moved_off_an_opening_heading_still_opens_a_session() {
        let _ranch = crate::testing::temp_ranch();
        let remote = frames(vec![frame("guided", vec![win(4, "api", "claude")])]);
        let mut dash = GlobalDashboard::new();

        dash.handle_input(KeyCode::Tab, &[], &[], &[], &[], &remote, &no_stale());
        let action = dash.handle_input(KeyCode::Enter, &[], &[], &[], &[], &remote, &no_stale());

        assert!(
            matches!(&action, DashboardAction::OpenBarnWindow { barn, .. } if barn == "guided"),
            "the cursor was parked on a heading with nothing to open"
        );
    }

    // === the panel on screen ================================================

    fn screen(dash: &mut GlobalDashboard, windows: &[TmuxWindow], remote: &HashMap<String, RemoteFrame>) -> String {
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(150, 44))
            .expect("test terminal");
        terminal
            .draw(|f| {
                let area = f.area();
                dash.render(f, area, &[], &[], &[], windows, &HashSet::new(), remote, &no_stale());
            })
            .expect("draw");
        let buf = terminal.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf.cell((x, y)).expect("a cell").symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The builder can be right and the panel still draw the old list. This is
    /// the one assertion that the two are wired together.
    #[test]
    fn the_panel_draws_the_barns_sessions_and_not_only_this_machines() {
        let _ranch = crate::testing::temp_ranch();
        let mut dash = GlobalDashboard::new();
        let windows = [win(1, "local-window", "shell")];
        let remote = frames(vec![frame("guided", vec![win(4, "remote-window", "claude")])]);

        let text = screen(&mut dash, &windows, &remote);

        assert!(text.contains("[1] local-window"), "the local half went missing:\n{text}");
        assert!(text.contains("guided"), "no heading for the tunneled barn:\n{text}");
        assert!(text.contains("[A] remote-window"), "the barn's session is not on screen:\n{text}");
    }
}
