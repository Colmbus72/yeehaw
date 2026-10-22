use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::app::ProjectAction;
use crate::components::header;
use crate::components::list::{self, FilterKey, ListFilter, ListItem, ListState, ItemStatus, RowAction, RowStyle};
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
    /// `/` filter over the focused panel.
    filter: ListFilter,
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
            filter: ListFilter::new(),
        }
    }

    /// A form is open, or a `/` filter is being typed into — either way every
    /// key is text, and the app's own bindings must stand aside.
    pub fn is_input_mode(&self) -> bool {
        self.input_mode != InputMode::Normal || self.filter.is_typing()
    }

    /// A filter is applied, so `Esc` belongs to it rather than to "go back".
    pub fn has_filter(&self) -> bool {
        self.filter.is_active()
    }

    /// The filter, if `panel` is the one it applies to.
    fn filter_for(&self, panel: FocusedPanel) -> ListFilter {
        if self.focused_panel == panel { self.filter.clone() } else { ListFilter::new() }
    }

    fn state_for(&mut self, panel: FocusedPanel) -> &mut ListState {
        match panel {
            FocusedPanel::Livestock => &mut self.livestock_state,
            FocusedPanel::Sessions => &mut self.sessions_state,
            FocusedPanel::Herds => &mut self.herds_state,
            FocusedPanel::RanchHands => &mut self.ranchhands_state,
        }
    }

    /// The livestock rows as drawn — local first, then a divider, then remote,
    /// then filtered — with the index into `project.livestock` each one stands
    /// for. `None` for the divider.
    fn livestock_rows(&self, project: &Project) -> (Vec<ListItem>, Vec<Option<usize>>) {
        let (items, data) = build_livestock_rows(&project.livestock);
        let (items, map) = list::filter_items(items, &self.filter_for(FocusedPanel::Livestock));
        (items, map.into_iter().map(|i| data[i]).collect())
    }

    fn session_rows(&self, project: &Project, windows: &[TmuxWindow]) -> (Vec<ListItem>, Vec<u32>) {
        let session_windows = project_session_windows(project, windows);
        let items = build_session_items(&session_windows);
        let (items, map) = list::filter_items(items, &self.filter_for(FocusedPanel::Sessions));
        (items, map.into_iter().map(|i| session_windows[i].index).collect())
    }

    fn herd_rows(&self, project: &Project) -> (Vec<ListItem>, Vec<usize>) {
        list::filter_items(build_herd_items(&project.herds), &self.filter_for(FocusedPanel::Herds))
    }

    fn ranchhand_rows(&self, ranchhands: &[RanchHand]) -> (Vec<ListItem>, Vec<usize>) {
        list::filter_items(build_ranchhand_items(ranchhands), &self.filter_for(FocusedPanel::RanchHands))
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
        windows: &[TmuxWindow],
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

        // The `/` filter sees every key first: while it is being typed into,
        // `j`, `c`, a digit — all of it is text.
        match self.filter.handle_key(key) {
            FilterKey::Consumed { changed } => {
                if changed {
                    let panel = self.focused_panel;
                    self.state_for(panel).select_first();
                }
                return ProjectAction::None;
            }
            FilterKey::PassThrough => {}
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

        // While typing a filter, only the keys it passed through (↑/↓/Enter)
        // reach the panel; page-level letters and session digits are text.
        let typing = self.filter.is_typing();

        if !typing {
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

            // Session keys, from any panel — the `[n]` drawn on the sessions
            // panel's rows, counted over the unfiltered list so a filter never
            // renumbers them.
            if let KeyCode::Char(c) = key {
                if let Some(n) = c.to_digit(10).filter(|n| (1..=9).contains(n)) {
                    if let Some(w) = project_session_windows(project, windows).get(n as usize - 1) {
                        return ProjectAction::SelectWindow(w.index);
                    }
                }
            }
        }

        match self.focused_panel {
            FocusedPanel::Livestock => {
                let (items, data) = self.livestock_rows(project);
                let selected_ls = |state: &ListState| data.get(state.selected).copied().flatten();
                match key {
                    KeyCode::Down => self.livestock_state.select_next_selectable(&items),
                    KeyCode::Up => self.livestock_state.select_prev_selectable(&items),
                    KeyCode::Enter => {
                        self.livestock_state.settle_on_selectable(&items);
                        if let Some(idx) = selected_ls(&self.livestock_state) {
                            return ProjectAction::SelectLivestock(idx);
                        }
                    }
                    _ if typing => {}
                    KeyCode::Char('j') => self.livestock_state.select_next_selectable(&items),
                    KeyCode::Char('k') => self.livestock_state.select_prev_selectable(&items),
                    KeyCode::Char('g') => self.livestock_state.select_first_selectable(&items),
                    KeyCode::Char('G') => self.livestock_state.select_last_selectable(&items),
                    // Claude starts in a directory on this machine, so it is
                    // only offered for livestock that lives here.
                    KeyCode::Char('c') => {
                        if let Some(idx) = selected_ls(&self.livestock_state) {
                            if project.livestock.get(idx).is_some_and(config::livestock_is_on_this_machine) {
                                return ProjectAction::NewClaude(idx);
                            }
                        }
                    }
                    KeyCode::Char('s') => {
                        if let Some(idx) = selected_ls(&self.livestock_state) {
                            return ProjectAction::OpenShell(idx);
                        }
                    }
                    KeyCode::Char('n') => {
                        self.input_mode = InputMode::NewLivestockName;
                        self.text_input = TextInput::new("");
                        return ProjectAction::None;
                    }
                    _ => {}
                }
            }
            FocusedPanel::Sessions => {
                let (items, map) = self.session_rows(project, windows);
                let count = items.len();
                match key {
                    KeyCode::Down => self.sessions_state.select_next(count),
                    KeyCode::Up => self.sessions_state.select_prev(),
                    KeyCode::Enter => {
                        if let Some(&idx) = map.get(self.sessions_state.selected) {
                            return ProjectAction::SelectWindow(idx);
                        }
                    }
                    _ if typing => {}
                    KeyCode::Char('j') => self.sessions_state.select_next(count),
                    KeyCode::Char('k') => self.sessions_state.select_prev(),
                    KeyCode::Char('g') => self.sessions_state.select_first(),
                    KeyCode::Char('G') => self.sessions_state.select_last(count),
                    _ => {}
                }
            }
            FocusedPanel::Herds => {
                let (items, map) = self.herd_rows(project);
                let count = items.len();
                match key {
                    KeyCode::Down => self.herds_state.select_next(count),
                    KeyCode::Up => self.herds_state.select_prev(),
                    KeyCode::Enter => {
                        if let Some(&idx) = map.get(self.herds_state.selected) {
                            return ProjectAction::SelectHerd(idx);
                        }
                    }
                    _ if typing => {}
                    KeyCode::Char('j') => self.herds_state.select_next(count),
                    KeyCode::Char('k') => self.herds_state.select_prev(),
                    KeyCode::Char('g') => self.herds_state.select_first(),
                    KeyCode::Char('G') => self.herds_state.select_last(count),
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
                let (items, map) = self.ranchhand_rows(&ranchhands);
                let count = items.len();
                match key {
                    KeyCode::Down => self.ranchhands_state.select_next(count),
                    KeyCode::Up => self.ranchhands_state.select_prev(),
                    KeyCode::Enter => {
                        if let Some(rh) = map.get(self.ranchhands_state.selected).and_then(|&i| ranchhands.get(i)) {
                            return ProjectAction::SelectRanchHand(rh.name.clone());
                        }
                    }
                    _ if typing => {}
                    KeyCode::Char('j') => self.ranchhands_state.select_next(count),
                    KeyCode::Char('k') => self.ranchhands_state.select_prev(),
                    KeyCode::Char('g') => self.ranchhands_state.select_first(),
                    KeyCode::Char('G') => self.ranchhands_state.select_last(count),
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
                // Never the literal `local`: it is machine-relative with a
                // different spelling than `None`, so it looks like a real pin
                // while meaning "whichever machine reads this". Before adoption
                // that stores nothing; after, it stores this machine's real
                // name, which is what makes the record portable at all.
                self.new_ls_barn = config::stored_barn_name(&value).unwrap_or_default();
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

    /// `_barns`: the livestock list used to consult it to tell the synthetic
    /// local barn from a real one. `config::barn_label` answers that from the
    /// livestock alone now. Kept in the signature the way `handle_input` keeps
    /// its own, so every view is still called the same way.
    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        project: &Project,
        _barns: &[Barn],
        windows: &[TmuxWindow],
    ) {
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
            hints: Some("[n] new  [/] filter  [e] edit project"),
        };
        let livestock_inner = livestock_panel.render(frame, left_panels[0]);
        let (livestock_items, _) = self.livestock_rows(project);
        list::render_filtered_list(
            frame, livestock_inner, &livestock_items,
            &mut self.livestock_state,
            self.focused_panel == FocusedPanel::Livestock,
            &self.filter,
        );

        // Herds panel
        let herds_panel = Panel {
            title: "Herds",
            focused: self.focused_panel == FocusedPanel::Herds,
            hints: Some("[n] new  [/] filter"),
        };
        let herds_inner = herds_panel.render(frame, left_panels[1]);
        let (herd_items, _) = self.herd_rows(project);
        list::render_filtered_list(
            frame, herds_inner, &herd_items,
            &mut self.herds_state,
            self.focused_panel == FocusedPanel::Herds,
            &self.filter,
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
            hints: Some("[1-9] open  [/] filter"),
        };
        let sessions_inner = sessions_panel.render(frame, right_panels[0]);
        let (session_items, _) = self.session_rows(project, windows);
        list::render_filtered_list(
            frame, sessions_inner, &session_items,
            &mut self.sessions_state,
            self.focused_panel == FocusedPanel::Sessions,
            &self.filter,
        );

        // RanchHands panel
        let ranchhands = config::load_ranchhands_for_project(&project.name);
        let rh_panel = Panel {
            title: "Ranch Hands",
            focused: self.focused_panel == FocusedPanel::RanchHands,
            hints: Some("[n] new  [/] filter"),
        };
        let rh_inner = rh_panel.render(frame, right_panels[1]);
        let (rh_items, _) = self.ranchhand_rows(&ranchhands);
        list::render_filtered_list(
            frame, rh_inner, &rh_items,
            &mut self.ranchhands_state,
            self.focused_panel == FocusedPanel::RanchHands,
            &self.filter,
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

/// The windows the sessions panel lists, in the order its `[n]` keys count.
///
/// One function for the panel and the keys that open its rows, so the number
/// drawn and the window it opens cannot disagree.
pub(crate) fn project_session_windows<'a>(project: &Project, windows: &'a [TmuxWindow]) -> Vec<&'a TmuxWindow> {
    windows.iter()
        .filter(|w| w.index > 0 && w.name.contains(&project.name))
        .collect()
}

fn build_session_items(session_windows: &[&TmuxWindow]) -> Vec<ListItem> {
    session_windows.iter().enumerate().map(|(i, w)| {
        let status_info = tmux::get_window_status(w);
        ListItem {
            id: w.index.to_string(),
            label: format!("[{}] {}", i + 1, w.name),
            status: Some(if w.active { ItemStatus::Active } else { ItemStatus::Inactive }),
            meta: Some(status_info.text),
            actions: vec![],
            ..Default::default()
        }
    }).collect()
}

fn build_ranchhand_items(ranchhands: &[RanchHand]) -> Vec<ListItem> {
    ranchhands.iter().map(|rh| {
        let meta = format!("{} • {}", rh.rh_type, if rh.herd.is_empty() { "(no herd)" } else { &rh.herd });
        ListItem {
            id: rh.name.clone(),
            label: rh.name.clone(),
            status: Some(if rh.last_sync.is_some() { ItemStatus::Active } else { ItemStatus::Inactive }),
            meta: Some(meta),
            actions: vec![],
            ..Default::default()
        }
    }).collect()
}

/// The livestock panel's rows: this machine's livestock first, then — under a
/// divider — everything on other barns, each half in its stored order. The
/// second list is the index into `livestock` each row stands for; `None` for
/// the divider, which is a [`RowStyle::Heading`] so the cursor steps over it.
fn build_livestock_rows(livestock: &[Livestock]) -> (Vec<ListItem>, Vec<Option<usize>>) {
    let (local, remote): (Vec<usize>, Vec<usize>) =
        (0..livestock.len()).partition(|&i| config::livestock_is_on_this_machine(&livestock[i]));

    let mut items = Vec::with_capacity(livestock.len() + 1);
    let mut data = Vec::with_capacity(livestock.len() + 1);
    for &i in &local {
        items.push(livestock_item(&livestock[i]));
        data.push(Some(i));
    }
    if !local.is_empty() && !remote.is_empty() {
        items.push(ListItem {
            id: "divider:remote".to_string(),
            label: "── remote ──".to_string(),
            style: RowStyle::Heading,
            ..Default::default()
        });
        data.push(None);
    }
    for &i in &remote {
        items.push(livestock_item(&livestock[i]));
        data.push(Some(i));
    }
    (items, data)
}

fn livestock_item(ls: &Livestock) -> ListItem {
    // `local` covers every spelling of "on this machine", adoption's real
    // barn name included. The column says here-or-elsewhere; it is not the
    // place the user learns their own machine's name.
    let barn_info = config::barn_label(ls).to_string();

    ListItem {
        id: ls.name.clone(),
        label: ls.name.clone(),
        status: Some(ItemStatus::Active),
        meta: Some(barn_info),
        actions: if config::livestock_is_on_this_machine(ls) {
            vec![
                RowAction { key: "c".to_string(), label: "claude".to_string() },
                RowAction { key: "s".to_string(), label: "shell".to_string() },
            ]
        } else {
            vec![RowAction { key: "s".to_string(), label: "shell".to_string() }]
        },
        ..Default::default()
    }
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
            ..Default::default()
        }
    }).collect()
}


#[cfg(test)]
mod tests {
    use super::*;

    fn bare_project() -> Project {
        Project {
            name: "api".into(),
            path: "/tmp/api".into(),
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

    /// Runs the new-livestock wizard from the barn step to the end and returns
    /// the barn it asks the app to store.
    fn barn_picked(typed: &str) -> Option<String> {
        let project = bare_project();
        let mut view = ProjectContextView::new();
        view.new_ls_name = "web".into();
        view.new_ls_path = "/tmp/web".into();

        view.input_mode = InputMode::NewLivestockBarn;
        view.text_input = TextInput::new(typed);
        view.handle_submit(&project);

        // Repo, then branch: the branch step is the one that emits the action.
        view.text_input = TextInput::new("");
        view.handle_submit(&project);
        view.text_input = TextInput::new("main");
        match view.handle_submit(&project) {
            ProjectAction::CreateLivestock(_, _, barn, _, _) => barn,
            _ => panic!("the branch step must emit CreateLivestock"),
        }
    }

    /// The literal `local` must never reach disk. It is machine-relative with a
    /// different spelling than `None`, so it looks like a real pin while
    /// meaning "whichever machine reads this" — two livestock in the real ranch
    /// carry it. Before adoption the picker stores `None`; after, it stores the
    /// machine's real name, which is the entire point of adoption.
    #[test]
    fn the_barn_picker_never_stores_the_literal_local() {
        let _ranch = crate::testing::temp_ranch();

        assert_eq!(barn_picked("local"), None);
        assert_eq!(barn_picked(""), None);
        assert_eq!(barn_picked("  Local "), None, "case is not a new barn");
        assert_eq!(barn_picked("pi"), Some("pi".to_string()));

        crate::migrate::adopt_this_machine("imac").unwrap();

        assert_eq!(barn_picked("local"), Some("imac".to_string()));
        assert_eq!(barn_picked(""), Some("imac".to_string()));
        assert_eq!(barn_picked("pi"), Some("pi".to_string()));
    }

    fn livestock_on(name: &str, barn: Option<&str>) -> Livestock {
        Livestock {
            name: name.into(),
            path: format!("/srv/{}", name),
            barn: barn.map(String::from),
            repo: None,
            branch: None,
            log_path: None,
            env_path: None,
            source: None,
            k8s_metadata: None,
            trails: vec![],
        }
    }

    /// Claude starts *here*, in a directory on this machine. For livestock on
    /// another barn that directory is someone else's, so neither the row nor
    /// the key offers it — `s` (shell) is how you reach remote code.
    #[test]
    fn claude_is_offered_only_for_livestock_on_this_machine() {
        let _ranch = crate::testing::temp_ranch();
        crate::migrate::adopt_this_machine("imac").unwrap();
        let mut project = bare_project();
        project.livestock = vec![
            livestock_on("here", Some("imac")),
            livestock_on("there", Some("pi")),
        ];

        // [0] here   [1] ── remote ──   [2] there
        let (items, _) = build_livestock_rows(&project.livestock);
        let keys = |i: usize| items[i].actions.iter().map(|a| a.key.clone()).collect::<Vec<_>>();
        assert_eq!(keys(0), vec!["c", "s"]);
        assert_eq!(keys(2), vec!["s"], "a remote livestock must not offer claude");

        let mut view = ProjectContextView::new();
        view.livestock_state.selected = 0;
        assert!(matches!(view.handle_input(KeyCode::Char('c'), &project, &[], &[]), ProjectAction::NewClaude(0)));
        view.livestock_state.selected = 2;
        assert!(matches!(view.handle_input(KeyCode::Char('c'), &project, &[], &[]), ProjectAction::None));
    }

    /// The livestock column says here-or-elsewhere. Adoption renames this
    /// machine's livestock without moving it, so the column must not start
    /// reading the machine's own hostname where it read `local` yesterday.
    #[test]
    fn the_livestock_column_reads_local_on_both_sides_of_adoption() {
        let _ranch = crate::testing::temp_ranch();

        let mut here = Livestock {
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
        let mut away = here.clone();
        away.name = "worker".into();
        away.barn = Some("pi".into());

        let metas = |ls: &[Livestock]| -> Vec<String> {
            build_livestock_rows(ls).0
                .into_iter()
                .filter(|i| i.selectable())
                .map(|i| i.meta.unwrap_or_default())
                .collect()
        };

        assert_eq!(metas(&[here.clone(), away.clone()]), ["local", "pi"]);

        crate::migrate::adopt_this_machine("imac").unwrap();
        here.barn = Some("imac".into());

        assert_eq!(metas(&[here, away]), ["local", "pi"]);
    }

    fn win(index: u32, name: &str) -> TmuxWindow {
        TmuxWindow { index, name: name.into(), ..Default::default() }
    }

    /// Local livestock sit above remote, with a divider the cursor steps over,
    /// and every row still opens the livestock it names.
    #[test]
    fn local_livestock_come_first_and_rows_resolve_to_the_right_livestock() {
        let _ranch = crate::testing::temp_ranch();
        crate::migrate::adopt_this_machine("imac").unwrap();
        let mut project = bare_project();
        project.livestock = vec![
            livestock_on("far", Some("pi")),
            livestock_on("near", Some("imac")),
        ];

        let (items, data) = build_livestock_rows(&project.livestock);
        let labels: Vec<_> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, ["near", "── remote ──", "far"]);
        assert_eq!(data, [Some(1), None, Some(0)]);

        let mut view = ProjectContextView::new();
        assert!(matches!(view.handle_input(KeyCode::Enter, &project, &[], &[]), ProjectAction::SelectLivestock(1)));
        view.handle_input(KeyCode::Char('j'), &project, &[], &[]);
        assert_eq!(view.livestock_state.selected, 2, "the cursor stopped on the divider");
        assert!(matches!(view.handle_input(KeyCode::Enter, &project, &[], &[]), ProjectAction::SelectLivestock(0)));
    }

    /// The `[n]` drawn beside a session opens it, from any panel.
    #[test]
    fn a_session_digit_switches_to_that_window() {
        let project = bare_project();
        let windows = [win(0, "yeehaw"), win(3, "api-claude"), win(5, "other"), win(7, "api-shell")];
        let mut view = ProjectContextView::new();
        assert!(matches!(view.handle_input(KeyCode::Char('1'), &project, &[], &windows), ProjectAction::SelectWindow(3)));
        assert!(matches!(view.handle_input(KeyCode::Char('2'), &project, &[], &windows), ProjectAction::SelectWindow(7)));
        assert!(matches!(view.handle_input(KeyCode::Char('3'), &project, &[], &windows), ProjectAction::None));
    }

    /// `/` narrows the focused list; Enter opens the filtered row; Esc
    /// brings every row back.
    #[test]
    fn the_filter_narrows_the_list_and_enter_opens_what_is_shown() {
        let _ranch = crate::testing::temp_ranch();
        let mut project = bare_project();
        project.livestock = vec![livestock_on("web", None), livestock_on("worker", None), livestock_on("api", None)];
        let mut view = ProjectContextView::new();

        for k in [KeyCode::Char('/'), KeyCode::Char('a'), KeyCode::Char('p')] {
            assert!(matches!(view.handle_input(k, &project, &[], &[]), ProjectAction::None));
        }
        assert!(view.is_input_mode(), "typed keys must not reach the app's bindings");
        let (items, _) = view.livestock_rows(&project);
        assert_eq!(items.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(), ["api"]);

        assert!(matches!(view.handle_input(KeyCode::Enter, &project, &[], &[]), ProjectAction::SelectLivestock(2)));
        assert!(!view.is_input_mode() && view.has_filter(), "Enter keeps the filter");

        view.handle_input(KeyCode::Esc, &project, &[], &[]);
        assert!(!view.has_filter());
        assert_eq!(view.livestock_rows(&project).0.len(), 3);
    }

    /// Tabbing away drops the filter.
    #[test]
    fn tabbing_out_clears_the_filter() {
        let project = bare_project();
        let mut view = ProjectContextView::new();
        view.handle_input(KeyCode::Char('/'), &project, &[], &[]);
        view.handle_input(KeyCode::Char('x'), &project, &[], &[]);
        view.handle_input(KeyCode::Tab, &project, &[], &[]);
        assert!(!view.has_filter());
        assert_eq!(view.focused_panel, FocusedPanel::Herds);
    }
}
