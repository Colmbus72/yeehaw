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
        let barn_items = build_barn_items(barns);
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

fn build_barn_items(barns: &[Barn]) -> Vec<ListItem> {
    barns.iter().map(|b| {
        let meta = if config::is_local_barn(b) {
            Some("this machine".to_string())
        } else {
            b.user.as_ref().zip(b.host.as_ref())
                .map(|(u, h)| format!("{}@{}", u, h))
        };
        ListItem {
            id: b.name.clone(),
            label: if config::is_local_barn(b) { "local".to_string() } else { b.name.clone() },
            status: Some(ItemStatus::Active),
            meta,
            actions: vec![RowAction { key: "s".to_string(), label: "shell".to_string() }],
        }
    }).collect()
}

fn build_session_items(session_windows: &[&TmuxWindow]) -> Vec<ListItem> {
    session_windows.iter().enumerate().map(|(i, w)| {
        let (label, type_hint) = format_session_label(&w.name);
        let status_info = tmux::get_window_status(w);
        let meta_text = if !type_hint.is_empty() {
            format!("{} · {}", type_hint, status_info.text)
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

fn format_session_label(name: &str) -> (String, String) {
    if let Some(rest) = name.strip_prefix("remote:") {
        return (rest.to_string(), "remote".to_string());
    }
    if let Some(rest) = name.strip_prefix("worm:") {
        return (rest.to_string(), "worm".to_string());
    }
    if let Some(rest) = name.strip_prefix("slack:") {
        return (rest.to_string(), "slack".to_string());
    }
    if let Some(rest) = name.strip_prefix("barn-") {
        return (rest.to_string(), "barn".to_string());
    }
    if let Some(rest) = name.strip_suffix("-claude") {
        return (rest.to_string(), "claude".to_string());
    }
    if let Some(pos) = name.rfind('-') {
        let project = &name[..pos];
        let livestock = &name[pos+1..];
        return (format!("{} · {}", project, livestock), "shell".to_string());
    }
    (name.to_string(), String::new())
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
