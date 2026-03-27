use crossterm::event::KeyCode;
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::app::BarnAction;
use crate::components::barn_header;
use crate::components::header;
use crate::components::list::{self, ListItem, ListState, ItemStatus};
use crate::components::panel::Panel;
use crate::components::text_input::TextInput;
use crate::config;
use crate::types::*;

const BRAND_COLOR: Color = Color::Rgb(212, 160, 32);

#[derive(Debug, Clone, Copy, PartialEq)]
enum FocusedPanel {
    Livestock,
    Critters,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum EditMode {
    Normal,
    EditHost,
    EditUser,
    EditPort,
    EditIdentityFile,
    // Critter creation
    NewCritterName,
    NewCritterService,
}

pub struct BarnContextView {
    focused_panel: FocusedPanel,
    livestock_state: ListState,
    critters_state: ListState,
    // Edit state
    edit_mode: EditMode,
    text_input: TextInput,
    edit_host: String,
    edit_user: String,
    edit_port: String,
    edit_identity_file: String,
    // New critter form
    new_critter_name: String,
}

impl BarnContextView {
    pub fn new() -> Self {
        Self {
            focused_panel: FocusedPanel::Livestock,
            livestock_state: ListState::new(),
            critters_state: ListState::new(),
            edit_mode: EditMode::Normal,
            text_input: TextInput::new(""),
            edit_host: String::new(),
            edit_user: String::new(),
            edit_port: String::new(),
            edit_identity_file: String::new(),
            new_critter_name: String::new(),
        }
    }

    pub fn is_editing(&self) -> bool {
        self.edit_mode != EditMode::Normal
    }

    fn start_edit(&mut self, barn: &Barn) {
        self.edit_host = barn.host.clone().unwrap_or_default();
        self.edit_user = barn.user.clone().unwrap_or_default();
        self.edit_port = barn.port.map(|p| p.to_string()).unwrap_or_else(|| "22".to_string());
        self.edit_identity_file = barn.identity_file.clone().unwrap_or_default();
        self.edit_mode = EditMode::EditHost;
        self.text_input = TextInput::new(&self.edit_host);
    }

    fn cancel_edit(&mut self) {
        self.edit_mode = EditMode::Normal;
        self.new_critter_name.clear();
    }

    fn advance_field(&mut self) -> Option<()> {
        let value = self.text_input.value.trim().to_string();

        match self.edit_mode {
            EditMode::EditHost => {
                if !value.is_empty() { self.edit_host = value; }
                self.edit_mode = EditMode::EditUser;
                self.text_input = TextInput::new(&self.edit_user);
            }
            EditMode::EditUser => {
                if !value.is_empty() { self.edit_user = value; }
                self.edit_mode = EditMode::EditPort;
                self.text_input = TextInput::new(&self.edit_port);
            }
            EditMode::EditPort => {
                if !value.is_empty() { self.edit_port = value; }
                self.edit_mode = EditMode::EditIdentityFile;
                self.text_input = TextInput::new(&self.edit_identity_file);
            }
            EditMode::EditIdentityFile => {
                self.edit_identity_file = value;
                return None; // Signal save
            }
            EditMode::Normal | EditMode::NewCritterName | EditMode::NewCritterService => {}
        }
        Some(())
    }

    fn build_updated(&self, original: &Barn) -> Barn {
        let port = self.edit_port.parse::<u16>().unwrap_or(22);
        Barn {
            name: original.name.clone(),
            host: if self.edit_host.is_empty() { None } else { Some(self.edit_host.clone()) },
            user: if self.edit_user.is_empty() { None } else { Some(self.edit_user.clone()) },
            port: Some(port),
            identity_file: if self.edit_identity_file.is_empty() { None } else { Some(self.edit_identity_file.clone()) },
            critters: original.critters.clone(),
            source: original.source.clone(),
            connection_type: original.connection_type.clone(),
            connection_config: original.connection_config.clone(),
            connectable: original.connectable,
        }
    }

    pub fn handle_input(&mut self, key: KeyCode, barn: &Barn, livestock_count: usize) -> BarnAction {
        // Handle edit mode input
        if self.edit_mode != EditMode::Normal {
            if key == KeyCode::Esc {
                self.cancel_edit();
                return BarnAction::None;
            }

            // Handle critter creation form
            if matches!(self.edit_mode, EditMode::NewCritterName | EditMode::NewCritterService) {
                let submitted = self.text_input.handle_input(key);
                if submitted {
                    let value = self.text_input.value.trim().to_string();
                    match self.edit_mode {
                        EditMode::NewCritterName => {
                            if !value.is_empty() {
                                self.new_critter_name = value;
                                self.edit_mode = EditMode::NewCritterService;
                                self.text_input = TextInput::new("");
                            }
                        }
                        EditMode::NewCritterService => {
                            if !value.is_empty() {
                                let name = self.new_critter_name.clone();
                                let service = value;
                                self.cancel_edit();
                                return BarnAction::CreateCritter(name, service);
                            }
                        }
                        _ => {}
                    }
                }
                return BarnAction::None;
            }

            let submitted = self.text_input.handle_input(key);
            if submitted {
                if self.advance_field().is_none() {
                    let updated = self.build_updated(barn);
                    self.edit_mode = EditMode::Normal;
                    return BarnAction::UpdateBarn(updated);
                }
            }
            return BarnAction::None;
        }

        // Normal mode
        // Tab to cycle
        if key == KeyCode::Tab {
            self.focused_panel = match self.focused_panel {
                FocusedPanel::Livestock => FocusedPanel::Critters,
                FocusedPanel::Critters => FocusedPanel::Livestock,
            };
            return BarnAction::None;
        }

        // Page-level: 's' to SSH into barn
        if key == KeyCode::Char('s') {
            return BarnAction::SshToBarn;
        }

        // Page-level: 'e' to edit barn (only for remote barns)
        if key == KeyCode::Char('e') {
            if !config::is_local_barn(barn) {
                self.start_edit(barn);
            }
            return BarnAction::None;
        }

        match self.focused_panel {
            FocusedPanel::Livestock => {
                match key {
                    KeyCode::Char('j') | KeyCode::Down => self.livestock_state.select_next(livestock_count),
                    KeyCode::Char('k') | KeyCode::Up => self.livestock_state.select_prev(),
                    KeyCode::Enter => return BarnAction::SelectLivestock(self.livestock_state.selected),
                    _ => {}
                }
            }
            FocusedPanel::Critters => {
                let count = barn.critters.len();
                match key {
                    KeyCode::Char('j') | KeyCode::Down => self.critters_state.select_next(count),
                    KeyCode::Char('k') | KeyCode::Up => self.critters_state.select_prev(),
                    KeyCode::Enter => return BarnAction::SelectCritter(self.critters_state.selected),
                    KeyCode::Char('n') => {
                        self.edit_mode = EditMode::NewCritterName;
                        self.text_input = TextInput::new("");
                        return BarnAction::None;
                    }
                    _ => {}
                }
            }
        }

        BarnAction::None
    }

    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        barn: &Barn,
        livestock: &[(Project, Livestock)],
    ) {
        if self.edit_mode != EditMode::Normal {
            if matches!(self.edit_mode, EditMode::NewCritterName | EditMode::NewCritterService) {
                self.render_critter_form(frame, area, barn);
            } else {
                self.render_edit_form(frame, area, barn);
            }
            return;
        }

        // Layout: Header (ASCII art + metadata) + Content
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(12), // ASCII header with metadata
                Constraint::Min(1),     // content
            ])
            .split(area);

        barn_header::render_barn_header(frame, chunks[0], barn);

        // Content: 2 panels
        let panels = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(50),
                Constraint::Percentage(50),
            ])
            .margin(1)
            .split(chunks[1]);

        // Livestock on this barn
        let livestock_panel = Panel {
            title: "Livestock",
            focused: self.focused_panel == FocusedPanel::Livestock,
            hints: Some("[n] add"),
        };
        let livestock_inner = livestock_panel.render(frame, panels[0]);
        let livestock_items: Vec<ListItem> = livestock.iter().map(|(p, ls)| {
            ListItem {
                id: format!("{}:{}", p.name, ls.name),
                label: format!("{} / {}", p.name, ls.name),
                status: Some(ItemStatus::Active),
                meta: Some(ls.path.clone()),
                actions: vec![],
            }
        }).collect();
        list::render_list(
            frame, livestock_inner, &livestock_items,
            &mut self.livestock_state,
            self.focused_panel == FocusedPanel::Livestock,
            None,
        );

        // Critters on this barn
        let critters_panel = Panel {
            title: "Critters",
            focused: self.focused_panel == FocusedPanel::Critters,
            hints: Some("[n] add  [d] remove"),
        };
        let critters_inner = critters_panel.render(frame, panels[1]);
        let critter_items: Vec<ListItem> = barn.critters.iter().map(|c| {
            ListItem {
                id: c.name.clone(),
                label: c.name.clone(),
                status: Some(ItemStatus::Active),
                meta: Some(c.service.clone()),
                actions: vec![],
            }
        }).collect();
        list::render_list(
            frame, critters_inner, &critter_items,
            &mut self.critters_state,
            self.focused_panel == FocusedPanel::Critters,
            None,
        );
    }

    fn render_edit_form(
        &self,
        frame: &mut Frame,
        area: Rect,
        barn: &Barn,
    ) {
        let (label, step) = match self.edit_mode {
            EditMode::EditHost => ("Host:", "1/4"),
            EditMode::EditUser => ("User:", "2/4"),
            EditMode::EditPort => ("Port:", "3/4"),
            EditMode::EditIdentityFile => ("Identity file (SSH key path):", "4/4"),
            EditMode::Normal | EditMode::NewCritterName | EditMode::NewCritterService => return,
        };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // header
                Constraint::Length(2), // title
                Constraint::Length(4), // completed fields
                Constraint::Length(1), // label
                Constraint::Length(1), // input
                Constraint::Length(2), // hints
                Constraint::Min(1),
            ])
            .split(area);

        let subtitle = barn.host.as_deref().unwrap_or("unknown");
        header::render_simple_header(
            frame,
            chunks[0],
            &format!("Barn: {}", barn.name),
            Some(subtitle),
        );

        let title = Paragraph::new(format!("  Edit Barn (Step {})", step))
            .style(Style::default().fg(BRAND_COLOR).add_modifier(Modifier::BOLD));
        frame.render_widget(title, chunks[1]);

        // Show completed fields
        let mut completed: Vec<Line> = Vec::new();
        let fields: Vec<(&str, &str, EditMode)> = vec![
            ("Host", &self.edit_host, EditMode::EditHost),
            ("User", &self.edit_user, EditMode::EditUser),
            ("Port", &self.edit_port, EditMode::EditPort),
            ("Key", &self.edit_identity_file, EditMode::EditIdentityFile),
        ];
        for (fname, fval, fmode) in &fields {
            if *fmode as u8 >= self.edit_mode as u8 {
                break;
            }
            completed.push(Line::from(vec![
                Span::styled(format!("  {}: ", fname), Style::default().fg(Color::DarkGray)),
                Span::raw(if fval.is_empty() { "\u{2014}" } else { fval }),
            ]));
        }
        if !completed.is_empty() {
            frame.render_widget(Paragraph::new(completed), chunks[2]);
        }

        let label_text = Paragraph::new(format!("  {}", label))
            .style(Style::default().fg(Color::White));
        frame.render_widget(label_text, chunks[3]);

        let input_area = Rect {
            x: chunks[4].x + 4,
            y: chunks[4].y,
            width: chunks[4].width.saturating_sub(4),
            height: 1,
        };
        self.text_input.render(frame, input_area);

        let hint_text = if self.edit_mode == EditMode::EditIdentityFile {
            "  Enter: save  Esc: cancel"
        } else {
            "  Enter: next field  Esc: cancel"
        };
        let hints = Paragraph::new(hint_text)
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(hints, chunks[5]);
    }

    fn render_critter_form(
        &self,
        frame: &mut Frame,
        area: Rect,
        barn: &Barn,
    ) {
        let (label, step) = match self.edit_mode {
            EditMode::NewCritterName => ("Name:", "1/2"),
            EditMode::NewCritterService => ("Service (systemd unit):", "2/2"),
            _ => return,
        };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // header
                Constraint::Length(2), // title
                Constraint::Length(2), // completed fields
                Constraint::Length(1), // label
                Constraint::Length(1), // input
                Constraint::Length(2), // hints
                Constraint::Min(1),
            ])
            .split(area);

        let subtitle = if config::is_local_barn(barn) { "local" } else { barn.host.as_deref().unwrap_or("unknown") };
        header::render_simple_header(
            frame,
            chunks[0],
            &format!("Barn: {}", barn.name),
            Some(subtitle),
        );

        let title = Paragraph::new(format!("  New Critter (Step {})", step))
            .style(Style::default().fg(BRAND_COLOR).add_modifier(Modifier::BOLD));
        frame.render_widget(title, chunks[1]);

        // Show completed fields
        if self.edit_mode == EditMode::NewCritterService {
            let completed = Paragraph::new(Line::from(vec![
                Span::styled("  Name: ", Style::default().fg(Color::DarkGray)),
                Span::raw(&self.new_critter_name),
            ]));
            frame.render_widget(completed, chunks[2]);
        }

        let label_text = Paragraph::new(format!("  {}", label))
            .style(Style::default().fg(Color::White));
        frame.render_widget(label_text, chunks[3]);

        let input_area = Rect {
            x: chunks[4].x + 4,
            y: chunks[4].y,
            width: chunks[4].width.saturating_sub(4),
            height: 1,
        };
        self.text_input.render(frame, input_area);

        let hint_text = if self.edit_mode == EditMode::NewCritterService {
            "  Enter: create  Esc: cancel"
        } else {
            "  Enter: next field  Esc: cancel"
        };
        let hints = Paragraph::new(hint_text)
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(hints, chunks[5]);
    }
}
