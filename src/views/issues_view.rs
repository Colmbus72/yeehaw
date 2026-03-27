use crossterm::event::KeyCode;
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::components::header;
use crate::components::list::{self, ListItem, ListState, ItemStatus};
use crate::components::panel::Panel;
use crate::issues::auth;
use crate::issues::types::*;
use crate::issues::github;
use crate::issues::linear;
use crate::types::*;

#[derive(Debug, Clone, Copy, PartialEq)]
enum FocusedPanel {
    List,
    Details,
}

#[derive(Debug, Clone, PartialEq)]
enum ViewState {
    Loading,
    Ready,
    Error(String),
    NotAuthenticated,
    LinearAuthInput,
    SelectTeam,
    Disabled,
    FilterMode,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum FilterField {
    Assignee,
    Cycle,
    Status,
    Sort,
}

pub enum IssuesAction {
    None,
    Back,
    OpenClaude(String), // context string for claude
}

pub struct IssuesView {
    focused_panel: FocusedPanel,
    list_state: ListState,
    detail_scroll: usize,
    view_state: ViewState,
    issues: Vec<Issue>,
    // Linear state
    teams: Vec<LinearTeam>,
    team_select_index: usize,
    cycles: Vec<LinearCycle>,
    assignees: Vec<LinearAssignee>,
    active_cycle_id: Option<String>,
    filter: LinearIssueFilter,
    // Filter UI state
    filter_field: FilterField,
    filter_field_index: [usize; 4], // index within each filter field
    // Auth input
    auth_input: String,
    auth_error: Option<String>,
}

impl IssuesView {
    pub fn new() -> Self {
        Self {
            focused_panel: FocusedPanel::List,
            list_state: ListState::new(),
            detail_scroll: 0,
            view_state: ViewState::Loading,
            issues: Vec::new(),
            teams: Vec::new(),
            team_select_index: 0,
            cycles: Vec::new(),
            assignees: Vec::new(),
            active_cycle_id: None,
            filter: LinearIssueFilter::default(),
            filter_field: FilterField::Assignee,
            filter_field_index: [0; 4],
            auth_input: String::new(),
            auth_error: None,
        }
    }

    /// Called when navigating to this view
    pub fn enter(&mut self, project: &Project) {
        self.issues.clear();
        self.list_state = ListState::new();
        self.detail_scroll = 0;

        match &project.issue_provider {
            None | Some(IssueProviderConfig::None) => {
                self.view_state = ViewState::Disabled;
            }
            Some(IssueProviderConfig::GitHub) => {
                if auth::is_gh_authenticated() {
                    self.view_state = ViewState::Loading;
                    self.load_github_issues(project);
                } else {
                    self.view_state = ViewState::NotAuthenticated;
                }
            }
            Some(IssueProviderConfig::Linear { team_id, .. }) => {
                if !auth::is_linear_authenticated() {
                    self.view_state = ViewState::NotAuthenticated;
                    self.auth_input.clear();
                    self.auth_error = None;
                    return;
                }
                if team_id.is_none() {
                    self.view_state = ViewState::Loading;
                    self.load_linear_teams();
                } else {
                    self.view_state = ViewState::Loading;
                    self.load_linear_issues(team_id.as_ref().unwrap());
                }
            }
        }
    }

    fn load_github_issues(&mut self, project: &Project) {
        match github::fetch_github_issues(&project.livestock, IssueState::Open) {
            Ok(issues) => {
                self.issues = issues;
                self.view_state = ViewState::Ready;
            }
            Err(e) => {
                self.view_state = ViewState::Error(e);
            }
        }
    }

    fn load_linear_teams(&mut self) {
        match linear::fetch_teams() {
            Ok(teams) => {
                self.teams = teams;
                self.team_select_index = 0;
                self.view_state = ViewState::SelectTeam;
            }
            Err(e) => {
                self.view_state = ViewState::Error(e);
            }
        }
    }

    fn load_linear_issues(&mut self, team_id: &str) {
        // Fetch cycles to get active cycle
        if self.cycles.is_empty() {
            if let Ok((cycles, active_id)) = linear::fetch_cycles(team_id) {
                self.active_cycle_id = active_id.clone();
                self.cycles = cycles;
                // Default filter: active cycle
                if self.filter.cycle_id.is_none() {
                    self.filter.cycle_id = active_id;
                }
            }
        }
        // Fetch assignees for filter
        if self.assignees.is_empty() {
            if let Ok(assignees) = linear::fetch_assignees(team_id) {
                self.assignees = assignees;
            }
        }

        match linear::fetch_issues(team_id, IssueState::Open, &self.filter, 50) {
            Ok(issues) => {
                self.issues = issues;
                self.view_state = ViewState::Ready;
            }
            Err(e) => {
                self.view_state = ViewState::Error(e);
            }
        }
    }

    pub fn handle_input(&mut self, key: KeyCode, project: &Project) -> IssuesAction {
        match &self.view_state {
            ViewState::LinearAuthInput => return self.handle_auth_input(key),
            ViewState::SelectTeam => return self.handle_team_select(key, project),
            ViewState::FilterMode => return self.handle_filter_input(key, project),
            ViewState::NotAuthenticated => {
                match key {
                    KeyCode::Esc => return IssuesAction::Back,
                    KeyCode::Enter => {
                        // Start auth flow
                        match &project.issue_provider {
                            Some(IssueProviderConfig::Linear { .. }) => {
                                self.view_state = ViewState::LinearAuthInput;
                                self.auth_input.clear();
                                self.auth_error = None;
                            }
                            _ => {} // GitHub: user must run `gh auth login` externally
                        }
                    }
                    _ => {}
                }
                return IssuesAction::None;
            }
            ViewState::Loading | ViewState::Error(_) | ViewState::Disabled => {
                if key == KeyCode::Esc {
                    return IssuesAction::Back;
                }
                if key == KeyCode::Char('r') && self.view_state != ViewState::Loading {
                    self.enter(project);
                }
                return IssuesAction::None;
            }
            ViewState::Ready => {}
        }

        // Ready state input handling
        match key {
            KeyCode::Esc => return IssuesAction::Back,
            KeyCode::Tab => {
                self.focused_panel = match self.focused_panel {
                    FocusedPanel::List => FocusedPanel::Details,
                    FocusedPanel::Details => FocusedPanel::List,
                };
            }
            KeyCode::Char('r') => {
                self.view_state = ViewState::Loading;
                self.enter(project);
            }
            KeyCode::Char('f') => {
                if matches!(&project.issue_provider, Some(IssueProviderConfig::Linear { .. })) {
                    self.filter_field = FilterField::Assignee;
                    self.view_state = ViewState::FilterMode;
                }
            }
            KeyCode::Char('o') => {
                // Open in browser
                if let Some(issue) = self.issues.get(self.list_state.selected) {
                    if !issue.url.is_empty() {
                        let _ = std::process::Command::new("open")
                            .arg(&issue.url)
                            .spawn();
                    }
                }
            }
            KeyCode::Char('c') => {
                // Open in Claude with context
                if let Some(issue) = self.issues.get(self.list_state.selected) {
                    let context = build_issue_context(project, issue);
                    return IssuesAction::OpenClaude(context);
                }
            }
            _ => {}
        }

        match self.focused_panel {
            FocusedPanel::List => match key {
                KeyCode::Char('j') | KeyCode::Down => {
                    self.list_state.select_next(self.issues.len());
                    self.detail_scroll = 0;
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.list_state.select_prev();
                    self.detail_scroll = 0;
                }
                KeyCode::Char('g') => {
                    self.list_state.select_first();
                    self.detail_scroll = 0;
                }
                KeyCode::Char('G') => {
                    self.list_state.select_last(self.issues.len());
                    self.detail_scroll = 0;
                }
                _ => {}
            },
            FocusedPanel::Details => match key {
                KeyCode::Char('j') | KeyCode::Down => self.detail_scroll += 1,
                KeyCode::Char('k') | KeyCode::Up => {
                    self.detail_scroll = self.detail_scroll.saturating_sub(1);
                }
                KeyCode::PageDown => self.detail_scroll += 20,
                KeyCode::PageUp => {
                    self.detail_scroll = self.detail_scroll.saturating_sub(20);
                }
                _ => {}
            },
        }

        IssuesAction::None
    }

    fn handle_auth_input(&mut self, key: KeyCode) -> IssuesAction {
        match key {
            KeyCode::Esc => {
                self.view_state = ViewState::NotAuthenticated;
            }
            KeyCode::Enter => {
                let key_str = self.auth_input.trim().to_string();
                if key_str.is_empty() {
                    self.auth_error = Some("API key cannot be empty".into());
                    return IssuesAction::None;
                }
                // Validate
                if auth::validate_linear_api_key(&key_str) {
                    auth::set_linear_token(&key_str);
                    self.auth_error = None;
                    self.view_state = ViewState::Loading;
                    self.load_linear_teams();
                } else {
                    self.auth_error = Some("Invalid API key".into());
                }
            }
            KeyCode::Char(c) => {
                self.auth_input.push(c);
            }
            KeyCode::Backspace => {
                self.auth_input.pop();
            }
            _ => {}
        }
        IssuesAction::None
    }

    fn handle_team_select(&mut self, key: KeyCode, project: &Project) -> IssuesAction {
        match key {
            KeyCode::Esc => return IssuesAction::Back,
            KeyCode::Char('j') | KeyCode::Down => {
                if self.team_select_index < self.teams.len().saturating_sub(1) {
                    self.team_select_index += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.team_select_index = self.team_select_index.saturating_sub(1);
            }
            KeyCode::Enter => {
                if let Some(team) = self.teams.get(self.team_select_index) {
                    let team_id = team.id.clone();
                    let team_name = team.name.clone();
                    // Save to project config
                    let mut proj = project.clone();
                    proj.issue_provider = Some(IssueProviderConfig::Linear {
                        team_id: Some(team_id.clone()),
                        team_name: Some(team_name),
                    });
                    let _ = crate::config::save_project(&proj);
                    // Load issues
                    self.view_state = ViewState::Loading;
                    self.load_linear_issues(&team_id);
                }
            }
            _ => {}
        }
        IssuesAction::None
    }

    fn handle_filter_input(&mut self, key: KeyCode, project: &Project) -> IssuesAction {
        match key {
            KeyCode::Esc => {
                self.view_state = ViewState::Ready;
            }
            KeyCode::Tab => {
                self.filter_field = match self.filter_field {
                    FilterField::Assignee => FilterField::Cycle,
                    FilterField::Cycle => FilterField::Status,
                    FilterField::Status => FilterField::Sort,
                    FilterField::Sort => FilterField::Assignee,
                };
            }
            KeyCode::BackTab => {
                self.filter_field = match self.filter_field {
                    FilterField::Assignee => FilterField::Sort,
                    FilterField::Cycle => FilterField::Assignee,
                    FilterField::Status => FilterField::Cycle,
                    FilterField::Sort => FilterField::Status,
                };
            }
            KeyCode::Char('j') | KeyCode::Down => {
                let idx = self.filter_field as usize;
                let max = self.filter_option_count();
                if self.filter_field_index[idx] < max.saturating_sub(1) {
                    self.filter_field_index[idx] += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                let idx = self.filter_field as usize;
                self.filter_field_index[idx] = self.filter_field_index[idx].saturating_sub(1);
            }
            KeyCode::Enter => {
                self.apply_filter_selection();
                self.view_state = ViewState::Loading;
                if let Some(IssueProviderConfig::Linear { team_id: Some(ref tid), .. }) = project.issue_provider {
                    let tid = tid.clone();
                    self.load_linear_issues(&tid);
                }
            }
            _ => {}
        }
        IssuesAction::None
    }

    fn filter_option_count(&self) -> usize {
        match self.filter_field {
            FilterField::Assignee => 3 + self.assignees.len(), // Any, Me, Unassigned, + each member
            FilterField::Cycle => 2 + self.cycles.len(),       // Any, Active, + each cycle
            FilterField::Status => 3,                          // Open, Closed, All
            FilterField::Sort => 3,                            // Priority, Updated, Created
        }
    }

    fn apply_filter_selection(&mut self) {
        let idx = self.filter_field_index;

        // Assignee
        match idx[0] {
            0 => { self.filter.assignee_is_me = false; self.filter.assignee_id = None; }
            1 => { self.filter.assignee_is_me = true; self.filter.assignee_id = None; }
            2 => { self.filter.assignee_is_me = false; self.filter.assignee_id = Some(None); }
            i => {
                if let Some(a) = self.assignees.get(i - 3) {
                    self.filter.assignee_is_me = false;
                    self.filter.assignee_id = Some(Some(a.id.clone()));
                }
            }
        }

        // Cycle
        match idx[1] {
            0 => { self.filter.cycle_id = None; }
            1 => { self.filter.cycle_id = self.active_cycle_id.clone(); }
            i => {
                if let Some(c) = self.cycles.get(i - 2) {
                    self.filter.cycle_id = Some(c.id.clone());
                }
            }
        }

        // Status
        self.filter.state_types = match idx[2] {
            0 => Some(vec!["backlog".into(), "unstarted".into(), "started".into()]),
            1 => Some(vec!["completed".into(), "canceled".into()]),
            2 => None, // All
            _ => None,
        };

        // Sort
        self.filter.sort_by = match idx[3] {
            0 => SortBy::Priority,
            1 => SortBy::UpdatedAt,
            _ => SortBy::CreatedAt,
        };
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, project: &Project) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // header
                Constraint::Min(1),    // content
            ])
            .split(area);

        let provider_label = match &project.issue_provider {
            Some(IssueProviderConfig::GitHub) => "github",
            Some(IssueProviderConfig::Linear { .. }) => "linear",
            _ => "none",
        };
        header::render_simple_header(
            frame,
            chunks[0],
            &format!("Issues: {}", project.name),
            Some(provider_label),
        );

        match &self.view_state {
            ViewState::Loading => {
                let text = Paragraph::new("Loading issues...")
                    .alignment(Alignment::Center)
                    .style(Style::default().fg(Color::Yellow));
                frame.render_widget(text, chunks[1]);
            }
            ViewState::Disabled => {
                let text = Paragraph::new(
                    "Issue tracking is not configured for this project.\n\nSet issueProvider in the project config to 'github' or 'linear'.",
                )
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::DarkGray));
                frame.render_widget(text, chunks[1]);
            }
            ViewState::NotAuthenticated => {
                self.render_auth_prompt(frame, chunks[1], project);
            }
            ViewState::LinearAuthInput => {
                self.render_auth_input(frame, chunks[1]);
            }
            ViewState::SelectTeam => {
                self.render_team_select(frame, chunks[1]);
            }
            ViewState::Error(msg) => {
                let text = Paragraph::new(format!("Error: {}\n\nPress [r] to retry", msg))
                    .alignment(Alignment::Center)
                    .style(Style::default().fg(Color::Red));
                frame.render_widget(text, chunks[1]);
            }
            ViewState::FilterMode => {
                self.render_filter(frame, chunks[1]);
            }
            ViewState::Ready => {
                self.render_issues(frame, chunks[1], project);
            }
        }
    }

    fn render_auth_prompt(&self, frame: &mut Frame, area: Rect, project: &Project) {
        let msg = match &project.issue_provider {
            Some(IssueProviderConfig::GitHub) => {
                "GitHub CLI not authenticated.\n\nRun `gh auth login` in your terminal, then press [r] to retry."
            }
            Some(IssueProviderConfig::Linear { .. }) => {
                "Linear not authenticated.\n\nPress [Enter] to enter your Personal API Key.\nCreate one at: https://linear.app/settings/api"
            }
            _ => "Not configured",
        };
        let text = Paragraph::new(msg)
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::Yellow));
        frame.render_widget(text, area);
    }

    fn render_auth_input(&self, frame: &mut Frame, area: Rect) {
        let inner = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(1),
                Constraint::Length(5),
                Constraint::Min(1),
            ])
            .split(area);

        let block_area = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(1),
                Constraint::Length(60),
                Constraint::Min(1),
            ])
            .split(inner[1]);

        let mut lines = vec![
            Line::from("Enter Linear Personal API Key:").style(Style::default().fg(Color::Yellow)),
            Line::from(""),
            Line::from(format!("  > {}_", "*".repeat(self.auth_input.len()))),
        ];

        if let Some(ref err) = self.auth_error {
            lines.push(Line::from(""));
            lines.push(Line::from(err.as_str()).style(Style::default().fg(Color::Red)));
        }

        let text = Paragraph::new(lines);
        frame.render_widget(text, block_area[1]);
    }

    fn render_team_select(&self, frame: &mut Frame, area: Rect) {
        let inner = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Min(1),
            ])
            .margin(2)
            .split(area);

        let title = Paragraph::new("Select a Linear team:")
            .style(Style::default().fg(Color::Yellow));
        frame.render_widget(title, inner[0]);

        let items: Vec<Line> = self.teams.iter().enumerate().map(|(i, t)| {
            let prefix = if i == self.team_select_index { "▸ " } else { "  " };
            let style = if i == self.team_select_index {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default()
            };
            Line::from(format!("{}[{}] {}", prefix, t.key, t.name)).style(style)
        }).collect();

        let list = Paragraph::new(items);
        frame.render_widget(list, inner[1]);
    }

    fn render_filter(&self, frame: &mut Frame, area: Rect) {
        let inner = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .margin(1)
            .split(area);

        let title = Paragraph::new("Filter Issues  [Tab] switch field  [j/k] select  [Enter] apply  [Esc] cancel")
            .style(Style::default().fg(Color::Yellow));
        frame.render_widget(title, inner[0]);

        // Four columns for filter fields
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(25),
                Constraint::Percentage(25),
                Constraint::Percentage(25),
                Constraint::Percentage(25),
            ])
            .split(inner[1]);

        self.render_filter_column(frame, cols[0], "Assignee", FilterField::Assignee, &{
            let mut opts = vec!["Any".into(), "Assigned to me".into(), "Unassigned".into()];
            for a in &self.assignees {
                opts.push(a.display_name.clone());
            }
            opts
        });

        self.render_filter_column(frame, cols[1], "Cycle", FilterField::Cycle, &{
            let mut opts = vec!["Any".into(), "Active cycle".into()];
            for c in &self.cycles {
                opts.push(c.name.clone());
            }
            opts
        });

        self.render_filter_column(frame, cols[2], "Status", FilterField::Status, &[
            "Open".into(),
            "Closed".into(),
            "All".into(),
        ]);

        self.render_filter_column(frame, cols[3], "Sort", FilterField::Sort, &[
            "Priority".into(),
            "Updated".into(),
            "Created".into(),
        ]);
    }

    fn render_filter_column(
        &self,
        frame: &mut Frame,
        area: Rect,
        title: &str,
        field: FilterField,
        options: &[String],
    ) {
        let is_active = self.filter_field == field;
        let field_idx = field as usize;
        let selected = self.filter_field_index[field_idx];

        let mut lines = vec![
            Line::from(title).style(Style::default().fg(if is_active {
                Color::Cyan
            } else {
                Color::DarkGray
            }).add_modifier(if is_active { Modifier::BOLD } else { Modifier::empty() })),
            Line::from(""),
        ];

        for (i, opt) in options.iter().enumerate() {
            let prefix = if i == selected { "▸ " } else { "  " };
            let style = if i == selected && is_active {
                Style::default().fg(Color::Cyan)
            } else if i == selected {
                Style::default().fg(Color::White)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            lines.push(Line::from(format!("{}{}", prefix, opt)).style(style));
        }

        let text = Paragraph::new(lines);
        frame.render_widget(text, area);
    }

    fn render_issues(&mut self, frame: &mut Frame, area: Rect, project: &Project) {
        let is_linear = matches!(&project.issue_provider, Some(IssueProviderConfig::Linear { .. }));

        // Two-panel layout: list (45%) + details (55%)
        let panels = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(45),
                Constraint::Percentage(55),
            ])
            .split(area);

        // List panel
        let filter_hint = if is_linear { " [f] filter" } else { "" };
        let list_hints = format!("[o] open [c] claude [r] refresh{}", filter_hint);
        let list_panel = Panel {
            title: &format!("Issues ({})", self.issues.len()),
            focused: self.focused_panel == FocusedPanel::List,
            hints: Some(&list_hints),
        };
        let list_inner = list_panel.render(frame, panels[0]);

        let items: Vec<ListItem> = self.issues.iter().map(|issue| {
            let (label, meta) = if is_linear {
                let state_char = issue.state_type
                    .map(|st| st.status_char())
                    .unwrap_or(' ');
                let pri = issue.priority_char();
                let initials = issue.assignee_initials();
                let pts = issue.estimate.map(|e| format!(" {}p", e)).unwrap_or_default();

                let label = format!(
                    "{}{} {} {}",
                    state_char, pri, issue.identifier, issue.title,
                );
                let meta = format!("{}{}", initials, pts);
                (label, Some(meta))
            } else {
                let repo_label = match &issue.source {
                    IssueSource::GitHub { livestock_name, .. } => {
                        format!("[{}] ", livestock_name)
                    }
                    _ => String::new(),
                };
                let label = format!("{}{} {}", repo_label, issue.identifier, issue.title);
                (label, None)
            };

            let status = if issue.is_open {
                Some(ItemStatus::Active)
            } else {
                Some(ItemStatus::Inactive)
            };

            ListItem {
                id: issue.id.clone(),
                label,
                status,
                meta,
                actions: vec![],
            }
        }).collect();

        list::render_list(
            frame,
            list_inner,
            &items,
            &mut self.list_state,
            self.focused_panel == FocusedPanel::List,
            Some(30),
        );

        // Details panel
        let detail_panel = Panel {
            title: "Details",
            focused: self.focused_panel == FocusedPanel::Details,
            hints: if self.focused_panel == FocusedPanel::Details {
                Some("[j/k] scroll")
            } else {
                None
            },
        };
        let detail_inner = detail_panel.render(frame, panels[1]);

        if let Some(issue) = self.issues.get(self.list_state.selected) {
            let mut lines: Vec<Line> = Vec::new();

            // Title
            lines.push(Line::from(vec![
                Span::styled(&issue.identifier, Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                Span::raw(" "),
                Span::styled(&issue.title, Style::default().add_modifier(Modifier::BOLD)),
            ]));
            lines.push(Line::from(""));

            // Metadata
            lines.push(Line::from(vec![
                Span::styled("State:    ", Style::default().fg(Color::DarkGray)),
                Span::styled(&issue.state, Style::default().fg(if issue.is_open { Color::Green } else { Color::Red })),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Author:   ", Style::default().fg(Color::DarkGray)),
                Span::raw(&issue.author),
            ]));

            if let Some(ref a) = issue.assignee {
                lines.push(Line::from(vec![
                    Span::styled("Assignee: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(&a.display_name),
                ]));
            }

            if let Some(pri) = issue.priority {
                let pri_label = match pri {
                    1 => "Urgent",
                    2 => "High",
                    3 => "Medium",
                    4 => "Low",
                    _ => "None",
                };
                lines.push(Line::from(vec![
                    Span::styled("Priority: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(pri_label),
                ]));
            }

            if let Some(ref c) = issue.cycle {
                lines.push(Line::from(vec![
                    Span::styled("Cycle:    ", Style::default().fg(Color::DarkGray)),
                    Span::raw(&c.name),
                ]));
            }

            if let Some(est) = issue.estimate {
                lines.push(Line::from(vec![
                    Span::styled("Points:   ", Style::default().fg(Color::DarkGray)),
                    Span::raw(format!("{}", est)),
                ]));
            }

            if !issue.labels.is_empty() {
                lines.push(Line::from(vec![
                    Span::styled("Labels:   ", Style::default().fg(Color::DarkGray)),
                    Span::raw(issue.labels.join(", ")),
                ]));
            }

            if !issue.url.is_empty() {
                lines.push(Line::from(vec![
                    Span::styled("URL:      ", Style::default().fg(Color::DarkGray)),
                    Span::styled(&issue.url, Style::default().fg(Color::Blue)),
                ]));
            }

            // Body
            if !issue.body.is_empty() {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled("─── Description ───", Style::default().fg(Color::DarkGray))));
                lines.push(Line::from(""));
                for line in issue.body.lines() {
                    lines.push(Line::from(line));
                }
            }

            // Comments
            if !issue.comments.is_empty() {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    format!("─── Comments ({}) ───", issue.comments.len()),
                    Style::default().fg(Color::DarkGray),
                )));

                for comment in &issue.comments {
                    lines.push(Line::from(""));
                    lines.push(Line::from(vec![
                        Span::styled(&comment.author, Style::default().fg(Color::Cyan)),
                        Span::styled(
                            format!("  {}", &comment.created_at.get(..10).unwrap_or(&comment.created_at)),
                            Style::default().fg(Color::DarkGray),
                        ),
                    ]));
                    for line in comment.body.lines() {
                        lines.push(Line::from(format!("  {}", line)));
                    }
                }
            }

            // Scrolling
            let total = lines.len();
            let visible_height = detail_inner.height as usize;
            let max_scroll = total.saturating_sub(visible_height);
            if self.detail_scroll > max_scroll {
                self.detail_scroll = max_scroll;
            }

            let visible_lines: Vec<Line> = lines
                .into_iter()
                .skip(self.detail_scroll)
                .take(visible_height)
                .collect();

            let content = Paragraph::new(visible_lines).wrap(ratatui::widgets::Wrap { trim: false });
            frame.render_widget(content, detail_inner);
        } else {
            let empty = Paragraph::new("No issue selected")
                .style(Style::default().fg(Color::DarkGray))
                .alignment(Alignment::Center);
            frame.render_widget(empty, detail_inner);
        }

        // Filter status line (Linear only)
        if is_linear && !self.issues.is_empty() {
            // Render inline with the issue count - already handled by panel title
        }
    }
}

fn build_issue_context(project: &Project, issue: &Issue) -> String {
    let mut ctx = crate::context::build_project_context(project);
    ctx.push_str(&format!("\n\nIssue: {} {}", issue.identifier, issue.title));
    ctx.push_str(&format!("\nState: {}", issue.state));
    ctx.push_str(&format!("\nAuthor: {}", issue.author));

    if let Some(ref a) = issue.assignee {
        ctx.push_str(&format!("\nAssignee: {}", a.display_name));
    }
    if let Some(pri) = issue.priority {
        let label = match pri { 1 => "Urgent", 2 => "High", 3 => "Medium", 4 => "Low", _ => "None" };
        ctx.push_str(&format!("\nPriority: {}", label));
    }
    if !issue.url.is_empty() {
        ctx.push_str(&format!("\nURL: {}", issue.url));
    }
    if !issue.labels.is_empty() {
        ctx.push_str(&format!("\nLabels: {}", issue.labels.join(", ")));
    }
    if !issue.body.is_empty() {
        ctx.push_str(&format!("\n\nDescription:\n{}", issue.body));
    }
    if !issue.comments.is_empty() {
        ctx.push_str("\n\nComments:");
        for c in &issue.comments {
            ctx.push_str(&format!("\n\n--- {} ({}) ---\n{}", c.author, c.created_at, c.body));
        }
    }
    ctx
}
