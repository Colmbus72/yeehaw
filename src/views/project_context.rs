use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::app::ProjectAction;
use crate::components::header;
use crate::components::list::{self, ListItem, ListState, ItemStatus, RowAction};
use crate::components::panel::Panel;
use crate::components::path_input::{self, PathInputState, PathInputAction};
use crate::components::text_input::TextInput;
use crate::config;
use crate::git;
use crate::tmux::{self, TmuxWindow};
use crate::types::*;

const BRAND_COLOR: Color = Color::Rgb(212, 160, 32);

#[derive(Debug, Clone, Copy, PartialEq)]
enum FocusedPanel {
    Livestock,
    Sessions,
    Herds,
    RanchHands,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum InputMode {
    Normal,
    // Livestock creation
    NewLivestockName,
    NewLivestockPath,
    NewLivestockBarn,
    NewLivestockRepo,
    NewLivestockBranch,
    // Project editing
    EditName,
    EditPath,
    EditSummary,
    EditColor,
    EditIssueProvider,
    EditWikiProvider,
    // Herd creation
    NewHerdName,
    // RanchHand creation
    NewRhName,
    NewRhType,
    NewRhHerd,
}

pub struct ProjectContextView {
    focused_panel: FocusedPanel,
    livestock_state: ListState,
    sessions_state: ListState,
    herds_state: ListState,
    // Input mode
    input_mode: InputMode,
    text_input: TextInput,
    path_input: PathInputState,
    new_ls_name: String,
    new_ls_path: String,
    new_ls_barn: String,
    new_ls_repo: String,
    // Edit project state
    edit_name: String,
    edit_path: String,
    edit_summary: String,
    edit_color: String,
    edit_issue_provider: usize, // 0=github, 1=linear, 2=none
    edit_wiki_provider: usize,  // 0=local, 1=linear
    // RanchHand creation
    new_rh_name: String,
    new_rh_type: usize,  // 0=terraform, 1=kubernetes
    new_rh_herd: String,
    // RanchHands panel
    ranchhands_state: ListState,
}

impl ProjectContextView {
    pub fn new() -> Self {
        Self {
            focused_panel: FocusedPanel::Livestock,
            livestock_state: ListState::new(),
            sessions_state: ListState::new(),
            herds_state: ListState::new(),
            input_mode: InputMode::Normal,
            text_input: TextInput::new(""),
            path_input: PathInputState::new(""),
            new_ls_name: String::new(),
            new_ls_path: String::new(),
            new_ls_barn: String::new(),
            new_ls_repo: String::new(),
            edit_name: String::new(),
            edit_path: String::new(),
            edit_summary: String::new(),
            edit_color: String::new(),
            edit_issue_provider: 0,
            edit_wiki_provider: 0,
            new_rh_name: String::new(),
            new_rh_type: 0,
            new_rh_herd: String::new(),
            ranchhands_state: ListState::new(),
        }
    }

    pub fn is_input_mode(&self) -> bool {
        self.input_mode != InputMode::Normal
    }

    fn reset_forms(&mut self) {
        self.new_ls_name.clear();
        self.new_ls_path.clear();
        self.new_ls_barn.clear();
        self.new_ls_repo.clear();
        self.new_rh_name.clear();
        self.new_rh_type = 0;
        self.new_rh_herd.clear();
        self.input_mode = InputMode::Normal;
    }

    fn start_edit(&mut self, project: &Project) {
        self.edit_name = project.name.clone();
        self.edit_path = project.path.clone();
        self.edit_summary = project.summary.clone().unwrap_or_default();
        self.edit_color = project.color.clone().unwrap_or_default();
        self.edit_issue_provider = match &project.issue_provider {
            Some(IssueProviderConfig::GitHub) => 0,
            Some(IssueProviderConfig::Linear { .. }) => 1,
            Some(IssueProviderConfig::None) => 2,
            None => 0,
        };
        self.edit_wiki_provider = match &project.wiki_provider {
            Some(WikiProviderConfig::Linear { .. }) => 1,
            _ => 0,
        };
        self.input_mode = InputMode::EditName;
        self.text_input = TextInput::new(&self.edit_name);
    }

    fn build_updated_project(&self, project: &Project) -> Project {
        let issue_provider = match self.edit_issue_provider {
            0 => Some(IssueProviderConfig::GitHub),
            1 => {
                // Preserve existing team_id/team_name if already Linear
                let (tid, tn) = match &project.issue_provider {
                    Some(IssueProviderConfig::Linear { team_id, team_name }) => {
                        (team_id.clone(), team_name.clone())
                    }
                    _ => (None, None),
                };
                Some(IssueProviderConfig::Linear { team_id: tid, team_name: tn })
            }
            _ => Some(IssueProviderConfig::None),
        };

        let wiki_provider = match self.edit_wiki_provider {
            1 => {
                let (tid, tn) = match &project.wiki_provider {
                    Some(WikiProviderConfig::Linear { team_id, team_name }) => {
                        (team_id.clone(), team_name.clone())
                    }
                    _ => (None, None),
                };
                Some(WikiProviderConfig::Linear { team_id: tid, team_name: tn })
            }
            _ => Some(WikiProviderConfig::Local),
        };

        Project {
            name: self.edit_name.clone(),
            path: self.edit_path.clone(),
            summary: if self.edit_summary.is_empty() { None } else { Some(self.edit_summary.clone()) },
            color: if self.edit_color.is_empty() { None } else { Some(self.edit_color.clone()) },
            issue_provider,
            wiki_provider,
            ..project.clone()
        }
    }

    pub fn handle_input(
        &mut self,
        key: KeyCode,
        project: &Project,
        _barns: &[Barn],
    ) -> ProjectAction {
        // Input mode handling
        if self.input_mode != InputMode::Normal {
            if key == KeyCode::Esc {
                self.reset_forms();
                return ProjectAction::None;
            }

            // Selection-based modes
            match self.input_mode {
                InputMode::EditIssueProvider => {
                    return self.handle_issue_provider_input(key, project);
                }
                InputMode::EditWikiProvider => {
                    return self.handle_wiki_provider_input(key, project);
                }
                InputMode::NewRhType => {
                    return self.handle_rh_type_input(key);
                }
                _ => {}
            }

            // Use path input for path fields, text input otherwise
            let is_path_field = matches!(self.input_mode, InputMode::NewLivestockPath | InputMode::EditPath);
            if is_path_field {
                let key_event = KeyEvent::new(key, KeyModifiers::empty());
                match path_input::handle_key(&mut self.path_input, key_event) {
                    PathInputAction::Submit(expanded) => {
                        self.text_input.value = expanded;
                        return self.handle_submit(project);
                    }
                    PathInputAction::Cancel => {
                        self.reset_forms();
                        return ProjectAction::None;
                    }
                    PathInputAction::None => {}
                }
            } else {
                let submitted = self.text_input.handle_input(key);
                if submitted {
                    return self.handle_submit(project);
                }
            }
            return ProjectAction::None;
        }

        // Tab to cycle panels (left→right, then top→bottom)
        if key == KeyCode::Tab {
            self.focused_panel = match self.focused_panel {
                FocusedPanel::Livestock => FocusedPanel::Herds,
                FocusedPanel::Herds => FocusedPanel::Sessions,
                FocusedPanel::Sessions => FocusedPanel::RanchHands,
                FocusedPanel::RanchHands => FocusedPanel::Livestock,
            };
            return ProjectAction::None;
        }

        // Page-level keys
        match key {
            KeyCode::Char('w') => return ProjectAction::OpenWiki,
            KeyCode::Char('i') => return ProjectAction::OpenIssues,
            KeyCode::Char('e') => {
                self.start_edit(project);
                return ProjectAction::None;
            }
            _ => {}
        }

        match self.focused_panel {
            FocusedPanel::Livestock => {
                let count = project.livestock.len();
                match key {
                    KeyCode::Char('j') | KeyCode::Down => self.livestock_state.select_next(count),
                    KeyCode::Char('k') | KeyCode::Up => self.livestock_state.select_prev(),
                    KeyCode::Char('g') => self.livestock_state.select_first(),
                    KeyCode::Char('G') => self.livestock_state.select_last(count),
                    KeyCode::Enter => return ProjectAction::SelectLivestock(self.livestock_state.selected),
                    KeyCode::Char('c') => return ProjectAction::NewClaude(self.livestock_state.selected),
                    KeyCode::Char('s') => return ProjectAction::OpenShell(self.livestock_state.selected),
                    KeyCode::Char('n') => {
                        self.input_mode = InputMode::NewLivestockName;
                        self.text_input = TextInput::new("");
                        return ProjectAction::None;
                    }
                    _ => {}
                }
            }
            FocusedPanel::Sessions => {
                // Sessions are read-only list with number switching handled at app level
            }
            FocusedPanel::Herds => {
                let count = project.herds.len();
                match key {
                    KeyCode::Char('j') | KeyCode::Down => self.herds_state.select_next(count),
                    KeyCode::Char('k') | KeyCode::Up => self.herds_state.select_prev(),
                    KeyCode::Enter => return ProjectAction::SelectHerd(self.herds_state.selected),
                    KeyCode::Char('n') => {
                        self.input_mode = InputMode::NewHerdName;
                        self.text_input = TextInput::new("");
                        return ProjectAction::None;
                    }
                    _ => {}
                }
            }
            FocusedPanel::RanchHands => {
                let ranchhands = config::load_ranchhands_for_project(&project.name);
                let count = ranchhands.len();
                match key {
                    KeyCode::Char('j') | KeyCode::Down => self.ranchhands_state.select_next(count),
                    KeyCode::Char('k') | KeyCode::Up => self.ranchhands_state.select_prev(),
                    KeyCode::Enter => {
                        if let Some(rh) = ranchhands.get(self.ranchhands_state.selected) {
                            return ProjectAction::SelectRanchHand(rh.name.clone());
                        }
                    }
                    KeyCode::Char('n') => {
                        self.input_mode = InputMode::NewRhName;
                        self.text_input = TextInput::new("");
                        return ProjectAction::None;
                    }
                    _ => {}
                }
            }
        }

        ProjectAction::None
    }

    fn handle_issue_provider_input(&mut self, key: KeyCode, _project: &Project) -> ProjectAction {
        match key {
            KeyCode::Char('j') | KeyCode::Down => {
                if self.edit_issue_provider < 2 {
                    self.edit_issue_provider += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.edit_issue_provider = self.edit_issue_provider.saturating_sub(1);
            }
            KeyCode::Enter => {
                self.input_mode = InputMode::EditWikiProvider;
            }
            _ => {}
        }
        ProjectAction::None
    }

    fn handle_wiki_provider_input(&mut self, key: KeyCode, project: &Project) -> ProjectAction {
        match key {
            KeyCode::Char('j') | KeyCode::Down => {
                if self.edit_wiki_provider < 1 {
                    self.edit_wiki_provider += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.edit_wiki_provider = self.edit_wiki_provider.saturating_sub(1);
            }
            KeyCode::Enter => {
                // Save and finish edit
                let updated = self.build_updated_project(project);
                self.reset_forms();
                return ProjectAction::UpdateProject(updated);
            }
            _ => {}
        }
        ProjectAction::None
    }

    fn handle_rh_type_input(&mut self, key: KeyCode) -> ProjectAction {
        match key {
            KeyCode::Char('j') | KeyCode::Down => {
                if self.new_rh_type < 1 { self.new_rh_type += 1; }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.new_rh_type = self.new_rh_type.saturating_sub(1);
            }
            KeyCode::Enter => {
                self.input_mode = InputMode::NewRhHerd;
                self.text_input = TextInput::new("");
            }
            _ => {}
        }
        ProjectAction::None
    }

    fn handle_submit(&mut self, _project: &Project) -> ProjectAction {
        let value = self.text_input.value.trim().to_string();

        match self.input_mode {
            // Livestock creation
            InputMode::NewLivestockName => {
                if !value.is_empty() {
                    self.new_ls_name = value;
                    self.input_mode = InputMode::NewLivestockPath;
                    self.path_input = PathInputState::new("~/");
                }
            }
            InputMode::NewLivestockPath => {
                if !value.is_empty() {
                    self.new_ls_path = value.clone();
                    // Auto-detect git info from the path
                    let expanded = if value.starts_with("~/") {
                        dirs::home_dir()
                            .map(|h| h.join(&value[2..]).to_string_lossy().to_string())
                            .unwrap_or(value)
                    } else {
                        value
                    };
                    let git_info = git::detect_git_info(&expanded);
                    if git_info.is_git_repo {
                        self.new_ls_repo = git_info.remote_url.unwrap_or_default();
                        // Will be used as default when we reach branch step
                    }
                    self.input_mode = InputMode::NewLivestockBarn;
                    self.text_input = TextInput::new("local");
                }
            }
            InputMode::NewLivestockBarn => {
                self.new_ls_barn = if value.is_empty() || value == "local" { String::new() } else { value };
                self.input_mode = InputMode::NewLivestockRepo;
                // Pre-fill with git-detected repo URL
                self.text_input = TextInput::new(&self.new_ls_repo);
            }
            InputMode::NewLivestockRepo => {
                self.new_ls_repo = value;
                self.input_mode = InputMode::NewLivestockBranch;
                // Pre-fill with git-detected branch, fallback to "main"
                let expanded = if self.new_ls_path.starts_with("~/") {
                    dirs::home_dir()
                        .map(|h| h.join(&self.new_ls_path[2..]).to_string_lossy().to_string())
                        .unwrap_or_else(|| self.new_ls_path.clone())
                } else {
                    self.new_ls_path.clone()
                };
                let git_info = git::detect_git_info(&expanded);
                let default_branch = git_info.branch.unwrap_or_else(|| "main".to_string());
                self.text_input = TextInput::new(&default_branch);
            }
            InputMode::NewLivestockBranch => {
                let name = self.new_ls_name.clone();
                let path = self.new_ls_path.clone();
                let barn = if self.new_ls_barn.is_empty() { None } else { Some(self.new_ls_barn.clone()) };
                let repo = if self.new_ls_repo.is_empty() { None } else { Some(self.new_ls_repo.clone()) };
                let branch = if value.is_empty() { None } else { Some(value) };
                self.reset_forms();
                return ProjectAction::CreateLivestock(name, path, barn, repo, branch);
            }
            // Project editing
            InputMode::EditName => {
                if !value.is_empty() {
                    self.edit_name = value;
                    self.input_mode = InputMode::EditPath;
                    self.path_input = PathInputState::new(&self.edit_path);
                }
            }
            InputMode::EditPath => {
                if !value.is_empty() {
                    self.edit_path = value;
                    self.input_mode = InputMode::EditSummary;
                    self.text_input = TextInput::new(&self.edit_summary);
                }
            }
            InputMode::EditSummary => {
                self.edit_summary = value;
                self.input_mode = InputMode::EditColor;
                self.text_input = TextInput::new(&self.edit_color);
            }
            InputMode::EditColor => {
                self.edit_color = value;
                self.input_mode = InputMode::EditIssueProvider;
            }
            // RanchHand creation
            InputMode::NewRhName => {
                if !value.is_empty() {
                    self.new_rh_name = value;
                    self.input_mode = InputMode::NewRhType;
                }
            }
            InputMode::NewRhHerd => {
                self.new_rh_herd = value;
                let name = self.new_rh_name.clone();
                let rh_type = if self.new_rh_type == 0 { "terraform" } else { "kubernetes" }.to_string();
                let herd = self.new_rh_herd.clone();
                self.reset_forms();
                return ProjectAction::CreateRanchHand { name, rh_type, herd };
            }
            InputMode::NewHerdName => {
                if !value.is_empty() {
                    self.reset_forms();
                    return ProjectAction::CreateHerd(value);
                }
            }
            InputMode::Normal | InputMode::EditIssueProvider | InputMode::EditWikiProvider | InputMode::NewRhType => {}
        }

        ProjectAction::None
    }

    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        project: &Project,
        barns: &[Barn],
        windows: &[TmuxWindow],
    ) {
        let session_windows: Vec<_> = windows.iter()
            .filter(|w| w.index > 0 && w.name.contains(&project.name))
            .collect();

        // Layout: Header (figlet) + Content
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(9), // figlet header + summary
                Constraint::Min(1),    // content
            ])
            .split(area);

        // Use live preview color when editing color field
        let preview_color = if matches!(self.input_mode, InputMode::EditColor) {
            let current_val = self.text_input.value.trim().to_string();
            if current_val.is_empty() { None } else { Some(current_val) }
        } else if matches!(self.input_mode, InputMode::EditIssueProvider | InputMode::EditWikiProvider) {
            // Show the already-entered color during later edit steps
            if self.edit_color.is_empty() { None } else { Some(self.edit_color.clone()) }
        } else {
            None
        };
        let display_color = preview_color.as_deref().or(project.color.as_deref());

        header::render_header(frame, chunks[0], &header::HeaderProps {
            text: &project.name,
            subtitle: None,
            summary: project.summary.as_deref(),
            color: display_color,
            gradient_spread: project.gradient_spread.map(|s| s as u8),
            gradient_inverted: project.gradient_inverted.unwrap_or(false),
            version_info: None,
        });

        // If in input mode, render form
        if self.input_mode != InputMode::Normal {
            self.render_input_form(frame, chunks[1], project);
            return;
        }

        // Content: 2 columns
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(50),
                Constraint::Percentage(50),
            ])
            .margin(1)
            .split(chunks[1]);

        // Left: Livestock + Herds
        let left_panels = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(55),
                Constraint::Percentage(45),
            ])
            .split(columns[0]);

        let livestock_panel = Panel {
            title: "Livestock",
            focused: self.focused_panel == FocusedPanel::Livestock,
            hints: Some("[n] new  [c] claude  [s] shell  [e] edit project"),
        };
        let livestock_inner = livestock_panel.render(frame, left_panels[0]);
        let livestock_items = build_livestock_items(&project.livestock, barns);
        list::render_list(
            frame, livestock_inner, &livestock_items,
            &mut self.livestock_state,
            self.focused_panel == FocusedPanel::Livestock,
            None,
        );

        // Herds panel
        let herds_panel = Panel {
            title: "Herds",
            focused: self.focused_panel == FocusedPanel::Herds,
            hints: Some("[n] new"),
        };
        let herds_inner = herds_panel.render(frame, left_panels[1]);
        let herd_items = build_herd_items(&project.herds);
        list::render_list(
            frame, herds_inner, &herd_items,
            &mut self.herds_state,
            self.focused_panel == FocusedPanel::Herds,
            None,
        );

        // Right: Sessions + RanchHands
        let right_panels = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(50),
                Constraint::Percentage(50),
            ])
            .split(columns[1]);

        // Sessions panel
        let sessions_panel = Panel {
            title: "Sessions",
            focused: self.focused_panel == FocusedPanel::Sessions,
            hints: None,
        };
        let sessions_inner = sessions_panel.render(frame, right_panels[0]);
        let session_items: Vec<ListItem> = session_windows.iter().enumerate().map(|(i, w)| {
            let status_info = tmux::get_window_status(w);
            ListItem {
                id: w.index.to_string(),
                label: format!("[{}] {}", i + 1, w.name),
                status: Some(if w.active { ItemStatus::Active } else { ItemStatus::Inactive }),
                meta: Some(status_info.text),
                actions: vec![],
            }
        }).collect();
        list::render_list(
            frame, sessions_inner, &session_items,
            &mut self.sessions_state,
            self.focused_panel == FocusedPanel::Sessions,
            None,
        );

        // RanchHands panel
        let ranchhands = config::load_ranchhands_for_project(&project.name);
        let rh_panel = Panel {
            title: "Ranch Hands",
            focused: self.focused_panel == FocusedPanel::RanchHands,
            hints: Some("[n] new"),
        };
        let rh_inner = rh_panel.render(frame, right_panels[1]);
        let rh_items: Vec<ListItem> = ranchhands.iter().map(|rh| {
            let meta = format!("{} • {}", rh.rh_type, if rh.herd.is_empty() { "(no herd)" } else { &rh.herd });
            ListItem {
                id: rh.name.clone(),
                label: rh.name.clone(),
                status: Some(if rh.last_sync.is_some() { ItemStatus::Active } else { ItemStatus::Inactive }),
                meta: Some(meta),
                actions: vec![],
            }
        }).collect();
        list::render_list(
            frame, rh_inner, &rh_items,
            &mut self.ranchhands_state,
            self.focused_panel == FocusedPanel::RanchHands,
            None,
        );
    }

    fn render_input_form(&self, frame: &mut Frame, area: Rect, _project: &Project) {
        match self.input_mode {
            InputMode::NewRhType => {
                self.render_provider_select(frame, area, "Ranch Hand Type", &[
                    ("Terraform", "Sync infrastructure from Terraform state"),
                    ("Kubernetes", "Sync resources from K8s cluster"),
                ], self.new_rh_type, "Step 2/3");
                return;
            }
            InputMode::EditIssueProvider => {
                self.render_provider_select(frame, area, "Issue Tracking", &[
                    ("GitHub Issues", "Use GitHub CLI (gh) for issues"),
                    ("Linear", "Use Linear for issue tracking"),
                    ("None", "Disable issue tracking"),
                ], self.edit_issue_provider, "Step 5/6");
                return;
            }
            InputMode::EditWikiProvider => {
                self.render_provider_select(frame, area, "Wiki Provider", &[
                    ("Local", "Store wiki sections in project config"),
                    ("Linear Projects", "Fetch wiki from Linear Projects (read-only)"),
                ], self.edit_wiki_provider, "Step 6/6");
                return;
            }
            _ => {}
        }

        let (title, label, step_info) = match self.input_mode {
            InputMode::NewLivestockName => ("New Livestock", "Name:", "Step 1/5"),
            InputMode::NewLivestockPath => ("New Livestock", "Path:", "Step 2/5"),
            InputMode::NewLivestockBarn => ("New Livestock", "Barn (local):", "Step 3/5"),
            InputMode::NewLivestockRepo => ("New Livestock", "Repo (optional):", "Step 4/5"),
            InputMode::NewLivestockBranch => ("New Livestock", "Branch:", "Step 5/5"),
            InputMode::EditName => ("Edit Project", "Name:", "Step 1/6"),
            InputMode::EditPath => ("Edit Project", "Path:", "Step 2/6"),
            InputMode::EditSummary => ("Edit Project", "Summary:", "Step 3/6"),
            InputMode::EditColor => ("Edit Project", "Color (hex):", "Step 4/6"),
            InputMode::NewHerdName => ("New Herd", "Name:", "Step 1/1"),
            InputMode::NewRhName => ("New Ranch Hand", "Name:", "Step 1/3"),
            InputMode::NewRhHerd => ("New Ranch Hand", "Herd name:", "Step 3/3"),
            InputMode::Normal | InputMode::EditIssueProvider | InputMode::EditWikiProvider | InputMode::NewRhType => return,
        };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2), // title
                Constraint::Length(5), // completed fields
                Constraint::Length(1), // current label
                Constraint::Length(1), // input
                Constraint::Length(2), // hints
                Constraint::Min(1),
            ])
            .margin(2)
            .split(area);

        let title_text = Paragraph::new(format!("  {} ({})", title, step_info))
            .style(Style::default().fg(BRAND_COLOR).add_modifier(Modifier::BOLD));
        frame.render_widget(title_text, chunks[0]);

        // Completed fields
        let mut completed_lines: Vec<Line> = Vec::new();

        match self.input_mode {
            // Livestock creation completed fields
            m if matches!(m, InputMode::NewLivestockPath | InputMode::NewLivestockBarn
                | InputMode::NewLivestockRepo | InputMode::NewLivestockBranch) => {
                if m as u8 > InputMode::NewLivestockName as u8 {
                    completed_lines.push(Line::from(vec![
                        Span::styled("  Name: ", Style::default().fg(Color::DarkGray)),
                        Span::raw(&self.new_ls_name),
                    ]));
                }
                if m as u8 > InputMode::NewLivestockPath as u8 {
                    completed_lines.push(Line::from(vec![
                        Span::styled("  Path: ", Style::default().fg(Color::DarkGray)),
                        Span::raw(&self.new_ls_path),
                    ]));
                }
                if m as u8 > InputMode::NewLivestockBarn as u8 {
                    completed_lines.push(Line::from(vec![
                        Span::styled("  Barn: ", Style::default().fg(Color::DarkGray)),
                        Span::raw(if self.new_ls_barn.is_empty() { "local" } else { &self.new_ls_barn }),
                    ]));
                }
                if m as u8 > InputMode::NewLivestockRepo as u8 {
                    completed_lines.push(Line::from(vec![
                        Span::styled("  Repo: ", Style::default().fg(Color::DarkGray)),
                        Span::raw(if self.new_ls_repo.is_empty() { "—" } else { &self.new_ls_repo }),
                    ]));
                }
            }
            // Project edit completed fields
            m if matches!(m, InputMode::EditPath | InputMode::EditSummary | InputMode::EditColor) => {
                if m as u8 > InputMode::EditName as u8 {
                    completed_lines.push(Line::from(vec![
                        Span::styled("  Name:    ", Style::default().fg(Color::DarkGray)),
                        Span::raw(&self.edit_name),
                    ]));
                }
                if m as u8 > InputMode::EditPath as u8 {
                    completed_lines.push(Line::from(vec![
                        Span::styled("  Path:    ", Style::default().fg(Color::DarkGray)),
                        Span::raw(&self.edit_path),
                    ]));
                }
                if m as u8 > InputMode::EditSummary as u8 {
                    completed_lines.push(Line::from(vec![
                        Span::styled("  Summary: ", Style::default().fg(Color::DarkGray)),
                        Span::raw(if self.edit_summary.is_empty() { "—" } else { &self.edit_summary }),
                    ]));
                }
            }
            _ => {}
        }

        if !completed_lines.is_empty() {
            frame.render_widget(Paragraph::new(completed_lines), chunks[1]);
        }

        let label_text = Paragraph::new(format!("  {}", label))
            .style(Style::default().fg(Color::White));
        frame.render_widget(label_text, chunks[2]);

        // Use path input for path fields, text input otherwise
        let is_path_field = matches!(self.input_mode, InputMode::NewLivestockPath | InputMode::EditPath);
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

        let hint_text = if is_path_field {
            "  Tab: complete  Enter: next field  Esc: cancel"
        } else {
            "  Enter: next field  Esc: cancel"
        };
        let hints = Paragraph::new(hint_text)
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(hints, chunks[4]);
    }

    fn render_provider_select(
        &self,
        frame: &mut Frame,
        area: Rect,
        title: &str,
        options: &[(&str, &str)],
        selected: usize,
        step_info: &str,
    ) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2), // title
                Constraint::Length(5), // completed fields
                Constraint::Length(1), // label
                Constraint::Min(1),   // options
                Constraint::Length(2), // hints
            ])
            .margin(2)
            .split(area);

        let title_text = Paragraph::new(format!("  Edit Project ({})", step_info))
            .style(Style::default().fg(BRAND_COLOR).add_modifier(Modifier::BOLD));
        frame.render_widget(title_text, chunks[0]);

        // Show completed text fields
        let completed = vec![
            Line::from(vec![
                Span::styled("  Name:    ", Style::default().fg(Color::DarkGray)),
                Span::raw(&self.edit_name),
            ]),
            Line::from(vec![
                Span::styled("  Path:    ", Style::default().fg(Color::DarkGray)),
                Span::raw(&self.edit_path),
            ]),
            Line::from(vec![
                Span::styled("  Summary: ", Style::default().fg(Color::DarkGray)),
                Span::raw(if self.edit_summary.is_empty() { "—" } else { &self.edit_summary }),
            ]),
            Line::from(vec![
                Span::styled("  Color:   ", Style::default().fg(Color::DarkGray)),
                Span::raw(if self.edit_color.is_empty() { "—" } else { &self.edit_color }),
            ]),
        ];
        frame.render_widget(Paragraph::new(completed), chunks[1]);

        let label = Paragraph::new(format!("  {}:", title))
            .style(Style::default().fg(Color::White));
        frame.render_widget(label, chunks[2]);

        let mut lines: Vec<Line> = Vec::new();
        for (i, (name, desc)) in options.iter().enumerate() {
            let prefix = if i == selected { "  ▸ " } else { "    " };
            let style = if i == selected {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            lines.push(Line::from(vec![
                Span::styled(prefix, style),
                Span::styled(*name, style.add_modifier(if i == selected { Modifier::BOLD } else { Modifier::empty() })),
                Span::styled(format!("  {}", desc), Style::default().fg(Color::DarkGray)),
            ]));
        }
        frame.render_widget(Paragraph::new(lines), chunks[3]);

        let hints = Paragraph::new("  j/k: select  Enter: confirm  Esc: cancel")
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(hints, chunks[4]);
    }
}

fn build_livestock_items(livestock: &[Livestock], barns: &[Barn]) -> Vec<ListItem> {
    livestock.iter().map(|ls| {
        let barn_info = ls.barn.as_ref().map(|bn| {
            let is_local = bn == "local" || barns.iter().any(|b| b.name == *bn && config::is_local_barn(b));
            if is_local { "local".to_string() } else { bn.clone() }
        }).unwrap_or_else(|| "local".to_string());

        ListItem {
            id: ls.name.clone(),
            label: ls.name.clone(),
            status: Some(ItemStatus::Active),
            meta: Some(barn_info),
            actions: vec![
                RowAction { key: "c".to_string(), label: "claude".to_string() },
                RowAction { key: "s".to_string(), label: "shell".to_string() },
            ],
        }
    }).collect()
}

fn build_herd_items(herds: &[Herd]) -> Vec<ListItem> {
    herds.iter().map(|h| {
        let meta = format!("{} livestock, {} critters", h.livestock.len(), h.critters.len());
        ListItem {
            id: h.name.clone(),
            label: h.name.clone(),
            status: Some(ItemStatus::Active),
            meta: Some(meta),
            actions: vec![],
        }
    }).collect()
}
