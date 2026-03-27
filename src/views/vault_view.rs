use std::time::Instant;

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph};

use crate::components::list::{self, ListItem, ListState};
use crate::components::panel::Panel;
use crate::components::text_input::TextInput;
use crate::types::VaultEntry;
use crate::vault::generator;

const BRAND_COLOR: Color = Color::Rgb(212, 160, 32);

// ============================================================================
// Enums
// ============================================================================

#[derive(Debug, Clone, PartialEq)]
pub enum VaultMode {
    Creating,
    CreatingConfirm,
    Locked,
    Unlocked,
    Adding,
    Editing(usize),
}

pub enum VaultAction {
    None,
    Unlock(String),
    CreateVault(String),
    InjectPassword(String),
    CopyPassword(String),
    SaveEntry {
        name: String,
        username: Option<String>,
        password: String,
        notes: Option<String>,
        edit_index: Option<usize>,
    },
    DeleteEntry(usize),
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum FormField {
    Name,
    Username,
    Password,
    Notes,
}

impl FormField {
    fn next(self) -> Self {
        match self {
            FormField::Name => FormField::Username,
            FormField::Username => FormField::Password,
            FormField::Password => FormField::Notes,
            FormField::Notes => FormField::Name,
        }
    }

    fn prev(self) -> Self {
        match self {
            FormField::Name => FormField::Notes,
            FormField::Username => FormField::Name,
            FormField::Password => FormField::Username,
            FormField::Notes => FormField::Password,
        }
    }
}

// ============================================================================
// VaultView
// ============================================================================

pub struct VaultView {
    pub mode: VaultMode,
    pub entries: Vec<VaultEntry>,
    pub list_state: ListState,
    pub master_input: TextInput,
    pub confirm_input: TextInput,
    pub master_password: Option<String>,
    pub error: Option<String>,
    pub idle_timer: Instant,
    pub revealed: Option<usize>,
    pub search_query: String,
    pub searching: bool,
    pub confirm_delete: Option<usize>,

    // Form fields for add/edit
    form_field: FormField,
    form_name: TextInput,
    form_username: TextInput,
    form_password: TextInput,
    form_notes: TextInput,

    // Create flow: store first password entry during confirm
    first_password: Option<String>,
}

impl VaultView {
    pub fn new() -> Self {
        Self {
            mode: VaultMode::Locked,
            entries: Vec::new(),
            list_state: ListState::new(),
            master_input: TextInput::new(""),
            confirm_input: TextInput::new(""),
            master_password: None,
            error: None,
            idle_timer: Instant::now(),
            revealed: None,
            search_query: String::new(),
            searching: false,
            confirm_delete: None,
            form_field: FormField::Name,
            form_name: TextInput::new(""),
            form_username: TextInput::new(""),
            form_password: TextInput::new(""),
            form_notes: TextInput::new(""),
            first_password: None,
        }
    }

    // ========================================================================
    // Mode transitions
    // ========================================================================

    pub fn enter_creating(&mut self) {
        self.mode = VaultMode::Creating;
        self.master_input = TextInput::new("");
        self.confirm_input = TextInput::new("");
        self.first_password = None;
        self.error = None;
    }

    pub fn enter_locked(&mut self) {
        self.mode = VaultMode::Locked;
        self.entries.clear();
        self.master_password = None;
        self.revealed = None;
        self.master_input = TextInput::new("");
        self.error = None;
    }

    pub fn enter_unlocked(&mut self, entries: Vec<VaultEntry>) {
        self.set_entries(entries);
        self.mode = VaultMode::Unlocked;
        self.revealed = None;
        self.searching = false;
        self.search_query.clear();
        self.error = None;
    }

    pub fn enter_adding(&mut self) {
        self.mode = VaultMode::Adding;
        self.form_field = FormField::Name;
        self.form_name = TextInput::new("");
        self.form_username = TextInput::new("");
        self.form_password = TextInput::new("");
        self.form_notes = TextInput::new("");
        self.error = None;
    }

    pub fn enter_editing(&mut self, idx: usize) {
        if idx < self.entries.len() {
            let entry = &self.entries[idx];
            self.form_name = TextInput::new(&entry.name);
            self.form_username = TextInput::new(entry.username.as_deref().unwrap_or(""));
            self.form_password = TextInput::new(&entry.password);
            self.form_notes = TextInput::new(entry.notes.as_deref().unwrap_or(""));
            self.form_field = FormField::Name;
            self.mode = VaultMode::Editing(idx);
            self.error = None;
        }
    }

    // ========================================================================
    // Helpers
    // ========================================================================

    pub fn reset_idle(&mut self) {
        self.idle_timer = Instant::now();
    }

    pub fn is_idle_expired(&self) -> bool {
        self.idle_timer.elapsed().as_secs() >= 120
    }

    pub fn set_entries(&mut self, entries: Vec<VaultEntry>) {
        self.entries = entries;
        self.list_state = ListState::new();
    }

    fn active_input(&mut self) -> &mut TextInput {
        match self.form_field {
            FormField::Name => &mut self.form_name,
            FormField::Username => &mut self.form_username,
            FormField::Password => &mut self.form_password,
            FormField::Notes => &mut self.form_notes,
        }
    }

    fn filtered_entries(&self) -> Vec<(usize, &VaultEntry)> {
        if self.search_query.is_empty() {
            self.entries.iter().enumerate().collect()
        } else {
            let query = self.search_query.to_lowercase();
            self.entries
                .iter()
                .enumerate()
                .filter(|(_, e)| {
                    e.name.to_lowercase().contains(&query)
                        || e.username
                            .as_deref()
                            .map(|u| u.to_lowercase().contains(&query))
                            .unwrap_or(false)
                        || e.notes.as_deref().unwrap_or("").to_lowercase().contains(&query)
                })
                .collect()
        }
    }

    // ========================================================================
    // Input handling
    // ========================================================================

    pub fn handle_input(&mut self, key: KeyCode, modifiers: KeyModifiers) -> VaultAction {
        self.reset_idle();
        self.error = None;

        match self.mode.clone() {
            VaultMode::Creating => self.handle_creating(key),
            VaultMode::CreatingConfirm => self.handle_creating_confirm(key),
            VaultMode::Locked => self.handle_locked(key),
            VaultMode::Unlocked => self.handle_unlocked(key, modifiers),
            VaultMode::Adding => self.handle_form(key, modifiers, None),
            VaultMode::Editing(idx) => self.handle_form(key, modifiers, Some(idx)),
        }
    }

    fn handle_creating(&mut self, key: KeyCode) -> VaultAction {
        match key {
            KeyCode::Esc => VaultAction::Close,
            KeyCode::Enter => {
                let pw = self.master_input.value.clone();
                if pw.is_empty() {
                    self.error = Some("Password cannot be empty".to_string());
                    return VaultAction::None;
                }
                self.first_password = Some(pw);
                self.mode = VaultMode::CreatingConfirm;
                self.confirm_input = TextInput::new("");
                VaultAction::None
            }
            _ => {
                self.master_input.handle_input(key);
                VaultAction::None
            }
        }
    }

    fn handle_creating_confirm(&mut self, key: KeyCode) -> VaultAction {
        match key {
            KeyCode::Esc => {
                self.mode = VaultMode::Creating;
                self.master_input = TextInput::new("");
                self.first_password = None;
                VaultAction::None
            }
            KeyCode::Enter => {
                let confirm = self.confirm_input.value.clone();
                if let Some(ref first) = self.first_password {
                    if &confirm == first {
                        VaultAction::CreateVault(confirm)
                    } else {
                        self.error = Some("Passwords do not match".to_string());
                        self.confirm_input = TextInput::new("");
                        VaultAction::None
                    }
                } else {
                    self.error = Some("No password set".to_string());
                    VaultAction::None
                }
            }
            _ => {
                self.confirm_input.handle_input(key);
                VaultAction::None
            }
        }
    }

    fn handle_locked(&mut self, key: KeyCode) -> VaultAction {
        match key {
            KeyCode::Esc => VaultAction::Close,
            KeyCode::Enter => {
                let pw = self.master_input.value.clone();
                if pw.is_empty() {
                    self.error = Some("Password cannot be empty".to_string());
                    return VaultAction::None;
                }
                VaultAction::Unlock(pw)
            }
            _ => {
                self.master_input.handle_input(key);
                VaultAction::None
            }
        }
    }

    fn handle_unlocked(&mut self, key: KeyCode, _modifiers: KeyModifiers) -> VaultAction {
        if self.searching {
            return self.handle_search(key);
        }

        let filtered = self.filtered_entries();
        let filtered_len = filtered.len();

        // Map current selected index to real entry index
        let real_idx = if !filtered.is_empty() && self.list_state.selected < filtered.len() {
            Some(filtered[self.list_state.selected].0)
        } else {
            None
        };

        // Reset delete confirmation on any key other than 'd'
        if key != KeyCode::Char('d') {
            self.confirm_delete = None;
        }

        match key {
            KeyCode::Char('j') | KeyCode::Down => {
                self.list_state.select_next(filtered_len);
                self.revealed = None;
                VaultAction::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.list_state.select_prev();
                self.revealed = None;
                VaultAction::None
            }
            KeyCode::Char('g') => {
                self.list_state.select_first();
                self.revealed = None;
                VaultAction::None
            }
            KeyCode::Char('G') => {
                self.list_state.select_last(filtered_len);
                self.revealed = None;
                VaultAction::None
            }
            KeyCode::Char('n') => {
                self.enter_adding();
                VaultAction::None
            }
            KeyCode::Char('e') => {
                if let Some(idx) = real_idx {
                    self.enter_editing(idx);
                }
                VaultAction::None
            }
            KeyCode::Char('d') => {
                if let Some(idx) = real_idx {
                    if self.confirm_delete == Some(idx) {
                        self.confirm_delete = None;
                        VaultAction::DeleteEntry(idx)
                    } else {
                        self.confirm_delete = Some(idx);
                        self.error = Some("Press d again to confirm delete".to_string());
                        VaultAction::None
                    }
                } else {
                    VaultAction::None
                }
            }
            KeyCode::Char('i') => {
                if let Some(idx) = real_idx {
                    VaultAction::InjectPassword(self.entries[idx].password.clone())
                } else {
                    VaultAction::None
                }
            }
            KeyCode::Char('c') => {
                if let Some(idx) = real_idx {
                    VaultAction::CopyPassword(self.entries[idx].password.clone())
                } else {
                    VaultAction::None
                }
            }
            KeyCode::Enter => {
                if let Some(idx) = real_idx {
                    if self.revealed == Some(idx) {
                        self.revealed = None;
                    } else {
                        self.revealed = Some(idx);
                    }
                }
                VaultAction::None
            }
            KeyCode::Char('/') => {
                self.searching = true;
                self.search_query.clear();
                VaultAction::None
            }
            KeyCode::Esc => VaultAction::Close,
            _ => VaultAction::None,
        }
    }

    fn handle_search(&mut self, key: KeyCode) -> VaultAction {
        match key {
            KeyCode::Esc => {
                self.searching = false;
                self.search_query.clear();
                self.list_state.select_first();
            }
            KeyCode::Enter => {
                self.searching = false;
            }
            KeyCode::Backspace => {
                self.search_query.pop();
                self.list_state.select_first();
            }
            KeyCode::Char(c) => {
                self.search_query.push(c);
                self.list_state.select_first();
            }
            _ => {}
        }
        VaultAction::None
    }

    fn handle_form(
        &mut self,
        key: KeyCode,
        modifiers: KeyModifiers,
        edit_index: Option<usize>,
    ) -> VaultAction {
        // Ctrl+G generates password when on Password field
        if modifiers.contains(KeyModifiers::CONTROL) && key == KeyCode::Char('g') {
            if self.form_field == FormField::Password {
                let pw = generator::generate_password(20);
                self.form_password = TextInput::new(&pw);
            }
            return VaultAction::None;
        }

        match key {
            KeyCode::Esc => {
                self.mode = VaultMode::Unlocked;
                VaultAction::None
            }
            KeyCode::Tab => {
                self.form_field = self.form_field.next();
                VaultAction::None
            }
            KeyCode::BackTab => {
                self.form_field = self.form_field.prev();
                VaultAction::None
            }
            KeyCode::Enter => {
                let name = self.form_name.value.trim().to_string();
                let password = self.form_password.value.trim().to_string();

                if name.is_empty() {
                    self.error = Some("Name is required".to_string());
                    return VaultAction::None;
                }
                if password.is_empty() {
                    self.error = Some("Password is required".to_string());
                    return VaultAction::None;
                }

                let username = {
                    let u = self.form_username.value.trim().to_string();
                    if u.is_empty() { None } else { Some(u) }
                };
                let notes = {
                    let n = self.form_notes.value.trim().to_string();
                    if n.is_empty() { None } else { Some(n) }
                };

                VaultAction::SaveEntry {
                    name,
                    username,
                    password,
                    notes,
                    edit_index,
                }
            }
            _ => {
                self.active_input().handle_input(key);
                VaultAction::None
            }
        }
    }

    // ========================================================================
    // Rendering
    // ========================================================================

    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        // Clear the area
        frame.render_widget(Clear, area);

        // Outer bordered box
        let outer_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(BRAND_COLOR))
            .title(Span::styled(
                " Vault ",
                Style::default().fg(BRAND_COLOR).add_modifier(Modifier::BOLD),
            ))
            .padding(Padding::new(2, 2, 1, 1));
        let inner = outer_block.inner(area);
        frame.render_widget(outer_block, area);

        match &self.mode {
            VaultMode::Creating | VaultMode::CreatingConfirm => {
                self.render_create(frame, inner);
            }
            VaultMode::Locked => {
                self.render_locked(frame, inner);
            }
            VaultMode::Unlocked => {
                self.render_unlocked(frame, inner);
            }
            VaultMode::Adding | VaultMode::Editing(_) => {
                self.render_form(frame, inner);
            }
        }
    }

    fn render_locked(&self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2), // title
                Constraint::Length(1), // label
                Constraint::Length(1), // input
                Constraint::Length(1), // spacer
                Constraint::Length(1), // error
                Constraint::Min(0),   // filler
            ])
            .split(area);

        let title = Paragraph::new("Enter master password")
            .style(Style::default().fg(BRAND_COLOR).add_modifier(Modifier::BOLD));
        frame.render_widget(title, chunks[0]);

        let label = Paragraph::new("Password:")
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(label, chunks[1]);

        self.render_masked_input(frame, chunks[2], &self.master_input);

        if let Some(ref err) = self.error {
            let err_text = Paragraph::new(err.as_str())
                .style(Style::default().fg(Color::Red));
            frame.render_widget(err_text, chunks[4]);
        }
    }

    fn render_create(&self, frame: &mut Frame, area: Rect) {
        let is_confirm = self.mode == VaultMode::CreatingConfirm;

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2), // title
                Constraint::Length(1), // label
                Constraint::Length(1), // input
                Constraint::Length(1), // spacer
                Constraint::Length(1), // error
                Constraint::Min(0),   // filler
            ])
            .split(area);

        let title_text = if is_confirm {
            "Confirm master password"
        } else {
            "Create new vault - enter master password"
        };

        let title = Paragraph::new(title_text)
            .style(Style::default().fg(BRAND_COLOR).add_modifier(Modifier::BOLD));
        frame.render_widget(title, chunks[0]);

        let label_text = if is_confirm { "Confirm:" } else { "Password:" };
        let label = Paragraph::new(label_text)
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(label, chunks[1]);

        let input = if is_confirm {
            &self.confirm_input
        } else {
            &self.master_input
        };
        self.render_masked_input(frame, chunks[2], input);

        if let Some(ref err) = self.error {
            let err_text = Paragraph::new(err.as_str())
                .style(Style::default().fg(Color::Red));
            frame.render_widget(err_text, chunks[4]);
        }
    }

    fn render_unlocked(&mut self, frame: &mut Frame, area: Rect) {
        let filtered = self.filtered_entries();

        // Layout: optional search bar + list panel
        let has_search = self.searching || !self.search_query.is_empty();
        let constraints = if has_search {
            vec![Constraint::Length(1), Constraint::Min(1)]
        } else {
            vec![Constraint::Min(1)]
        };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(area);

        let (list_area, search_offset) = if has_search {
            // Render search bar
            let search_display = format!("/{}", self.search_query);
            let search_style = if self.searching {
                Style::default().fg(BRAND_COLOR)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let search_bar = Paragraph::new(search_display).style(search_style);
            frame.render_widget(search_bar, chunks[0]);
            (chunks[1], 1)
        } else {
            (chunks[0], 0)
        };

        let _ = search_offset; // used only for layout logic above

        let hints = "[n]ew [e]dit [d]el [i]nject [c]opy [Enter]reveal [/]search";
        let panel = Panel {
            title: &format!(" Entries ({}) ", filtered.len()),
            focused: true,
            hints: Some(hints),
        };
        let list_inner = panel.render(frame, list_area);

        // Build list items from filtered entries
        let items: Vec<ListItem> = filtered
            .iter()
            .map(|(real_idx, entry)| {
                let meta = {
                    let user_part = entry
                        .username
                        .as_deref()
                        .unwrap_or("-");
                    let pw_part = if self.revealed == Some(*real_idx) {
                        entry.password.clone()
                    } else {
                        "\u{2022}".repeat(entry.password.len().min(12))
                    };
                    format!("{}  {}", user_part, pw_part)
                };

                ListItem {
                    id: entry.id.clone(),
                    label: entry.name.clone(),
                    status: None,
                    meta: Some(meta),
                    actions: vec![],
                }
            })
            .collect();

        list::render_list(
            frame,
            list_inner,
            &items,
            &mut self.list_state,
            true,
            None,
        );
    }

    fn render_form(&self, frame: &mut Frame, area: Rect) {
        let is_edit = matches!(self.mode, VaultMode::Editing(_));
        let title_text = if is_edit { "Edit Entry" } else { "New Entry" };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2), // title
                Constraint::Length(1), // name label
                Constraint::Length(1), // name input
                Constraint::Length(1), // spacer
                Constraint::Length(1), // username label
                Constraint::Length(1), // username input
                Constraint::Length(1), // spacer
                Constraint::Length(1), // password label
                Constraint::Length(1), // password input
                Constraint::Length(1), // spacer
                Constraint::Length(1), // notes label
                Constraint::Length(1), // notes input
                Constraint::Length(1), // spacer
                Constraint::Length(1), // error
                Constraint::Length(1), // hints
                Constraint::Min(0),   // filler
            ])
            .split(area);

        let title = Paragraph::new(title_text)
            .style(Style::default().fg(BRAND_COLOR).add_modifier(Modifier::BOLD));
        frame.render_widget(title, chunks[0]);

        // Name field
        self.render_field_label(frame, chunks[1], "Name:", self.form_field == FormField::Name);
        self.render_field_input(frame, chunks[2], &self.form_name, self.form_field == FormField::Name, false);

        // Username field
        self.render_field_label(frame, chunks[4], "Username:", self.form_field == FormField::Username);
        self.render_field_input(frame, chunks[5], &self.form_username, self.form_field == FormField::Username, false);

        // Password field
        let pw_label = if self.form_field == FormField::Password {
            "Password: (Ctrl+G to generate)"
        } else {
            "Password:"
        };
        self.render_field_label(frame, chunks[7], pw_label, self.form_field == FormField::Password);
        self.render_field_input(frame, chunks[8], &self.form_password, self.form_field == FormField::Password, true);

        // Notes field
        self.render_field_label(frame, chunks[10], "Notes:", self.form_field == FormField::Notes);
        self.render_field_input(frame, chunks[11], &self.form_notes, self.form_field == FormField::Notes, false);

        // Error
        if let Some(ref err) = self.error {
            let err_text = Paragraph::new(err.as_str())
                .style(Style::default().fg(Color::Red));
            frame.render_widget(err_text, chunks[13]);
        }

        // Hints
        let hints = Paragraph::new("[Tab] next field  [Enter] save  [Esc] cancel")
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(hints, chunks[14]);
    }

    fn render_field_label(&self, frame: &mut Frame, area: Rect, text: &str, active: bool) {
        let style = if active {
            Style::default().fg(BRAND_COLOR).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let label = Paragraph::new(text).style(style);
        frame.render_widget(label, area);
    }

    fn render_field_input(
        &self,
        frame: &mut Frame,
        area: Rect,
        input: &TextInput,
        active: bool,
        is_password: bool,
    ) {
        if active {
            // Show input with cursor (password field shows cleartext when focused for editing)
            if is_password {
                // When focused, show actual password with cursor for editing
                input.render(frame, area);
            } else {
                input.render(frame, area);
            }
        } else {
            // Inactive: show value as plain text (masked for password)
            let display = if is_password {
                if input.value.is_empty() {
                    String::new()
                } else {
                    "\u{2022}".repeat(input.value.len().min(20))
                }
            } else {
                input.value.clone()
            };
            let style = Style::default().fg(Color::White);
            let text = Paragraph::new(display).style(style);
            frame.render_widget(text, area);
        }
    }

    fn render_masked_input(&self, frame: &mut Frame, area: Rect, input: &TextInput) {
        if input.value.is_empty() {
            let cursor = Span::styled("_", Style::default().fg(BRAND_COLOR));
            let text = Paragraph::new(Line::from(cursor));
            frame.render_widget(text, area);
        } else {
            // Each bullet is 3 bytes in UTF-8, so we compute via char offsets
            let bullet_chars: Vec<char> = "\u{2022}".repeat(input.value.len()).chars().collect();
            let cursor_pos = input.cursor.min(bullet_chars.len());

            let before_str: String = bullet_chars[..cursor_pos].iter().collect();
            let cursor_char = if cursor_pos < bullet_chars.len() {
                bullet_chars[cursor_pos].to_string()
            } else {
                " ".to_string()
            };
            let after_str: String = if cursor_pos < bullet_chars.len() {
                bullet_chars[cursor_pos + 1..].iter().collect()
            } else {
                String::new()
            };

            let line = Line::from(vec![
                Span::raw(before_str),
                Span::styled(
                    cursor_char,
                    Style::default().bg(BRAND_COLOR).fg(Color::Black),
                ),
                Span::raw(after_str),
            ]);
            let text = Paragraph::new(line);
            frame.render_widget(text, area);
        }
    }
}
