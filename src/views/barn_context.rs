use crossterm::event::KeyCode;
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::app::BarnAction;
use crate::components::barn_header;
use crate::components::header;
use crate::components::list::{self, FilterKey, ListFilter, ListItem, ListState, ItemStatus};
use crate::components::panel::Panel;
use crate::components::text_input::TextInput;
use crate::config;
use crate::types::*;

const BRAND_COLOR: Color = Color::Rgb(212, 160, 32);

/// What the wizard headers put under "Barn: <name>": where this barn is
/// reached.
///
/// `ssh::dial_host`, not `barn.host`, so a blank host reads as "unknown"
/// exactly when ssh would refuse it.
fn header_subtitle(barn: &Barn) -> &str {
    crate::ssh::dial_host(barn).unwrap_or("unknown")
}

/// The subtitle any barn-bearing header shows: the word `local` for the
/// synthetic placeholder, otherwise where the barn is actually reached.
///
/// Shared with the critter detail page and the critter log pane
/// (`views::critter_detail`, `views::critter_logs`), which ask the same
/// question about the same barn and had drifted into three answers — two of
/// them wrong.
///
/// `is_local_barn`, not `barn_is_this_machine`: this asks whether the row is
/// the synthetic placeholder, which has no host, no uuid and no file, so there
/// is nothing to show but the word.
pub(crate) fn barn_subtitle(barn: &Barn) -> &str {
    if config::is_local_barn(barn) { "local" } else { header_subtitle(barn) }
}

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
    /// `/` filter over the focused panel.
    filter: ListFilter,
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
            filter: ListFilter::new(),
        }
    }

    pub fn is_editing(&self) -> bool {
        self.edit_mode != EditMode::Normal
    }

    /// A `/` filter is being typed into: every key is text.
    pub fn is_filter_typing(&self) -> bool {
        self.filter.is_typing()
    }

    /// A filter is applied, so `Esc` belongs to it rather than to "go back".
    pub fn has_filter(&self) -> bool {
        self.filter.is_active()
    }

    fn filter_for(&self, panel: FocusedPanel) -> ListFilter {
        if self.focused_panel == panel { self.filter.clone() } else { ListFilter::new() }
    }

    fn livestock_rows(&self, livestock: &[(Project, Livestock)]) -> (Vec<ListItem>, Vec<usize>) {
        let items = livestock.iter().map(|(p, ls)| ListItem {
            id: format!("{}:{}", p.name, ls.name),
            label: format!("{} / {}", p.name, ls.name),
            status: Some(ItemStatus::Active),
            meta: Some(ls.path.clone()),
            actions: vec![],
            ..Default::default()
        }).collect();
        list::filter_items(items, &self.filter_for(FocusedPanel::Livestock))
    }

    fn critter_rows(&self, barn: &Barn) -> (Vec<ListItem>, Vec<usize>) {
        let items = barn.critters.iter().map(|c| ListItem {
            id: c.name.clone(),
            label: c.name.clone(),
            status: Some(ItemStatus::Active),
            meta: Some(c.service.clone()),
            actions: vec![],
            ..Default::default()
        }).collect();
        list::filter_items(items, &self.filter_for(FocusedPanel::Critters))
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
            // Carried, and deliberately NOT `..Default::default()`. This is an
            // edit of an existing barn: a default here is not a default, it is a
            // deletion. Dropping the identity would make the save look like a
            // delete plus a create to anything that compares on the uuid.
            id: original.id.clone(),
            created_at: original.created_at.clone(),
            updated_at: original.updated_at.clone(),
        }
    }

    pub fn handle_input(&mut self, key: KeyCode, barn: &Barn, livestock: &[(Project, Livestock)]) -> BarnAction {
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
        // The `/` filter sees every key first.
        match self.filter.handle_key(key) {
            FilterKey::Consumed { changed } => {
                if changed {
                    match self.focused_panel {
                        FocusedPanel::Livestock => self.livestock_state.select_first(),
                        FocusedPanel::Critters => self.critters_state.select_first(),
                    }
                }
                return BarnAction::None;
            }
            FilterKey::PassThrough => {}
        }
        // Only ↑/↓/Enter get past a filter being typed into.
        let typing = self.filter.is_typing();

        // Tab to cycle
        if key == KeyCode::Tab {
            self.focused_panel = match self.focused_panel {
                FocusedPanel::Livestock => FocusedPanel::Critters,
                FocusedPanel::Critters => FocusedPanel::Livestock,
            };
            return BarnAction::None;
        }

        if !typing {
            match key {
                // Page-level: 's' to SSH into barn
                KeyCode::Char('s') => return BarnAction::SshToBarn,
                // Page-level: 'c' to connect to the barn's remote yeehaw
                KeyCode::Char('c') => return BarnAction::ConnectBarn,
                // Page-level: 'e' to edit barn (only for remote barns)
                KeyCode::Char('e') => {
                    if !config::is_local_barn(barn) {
                        self.start_edit(barn);
                    }
                    return BarnAction::None;
                }
                _ => {}
            }
        }

        match self.focused_panel {
            FocusedPanel::Livestock => {
                let map = self.livestock_rows(livestock).1;
                let count = map.len();
                match key {
                    KeyCode::Down => self.livestock_state.select_next(count),
                    KeyCode::Up => self.livestock_state.select_prev(),
                    KeyCode::Enter => {
                        if let Some(&i) = map.get(self.livestock_state.selected) {
                            return BarnAction::SelectLivestock(i);
                        }
                    }
                    _ if typing => {}
                    KeyCode::Char('j') => self.livestock_state.select_next(count),
                    KeyCode::Char('k') => self.livestock_state.select_prev(),
                    KeyCode::Char('g') => self.livestock_state.select_first(),
                    KeyCode::Char('G') => self.livestock_state.select_last(count),
                    _ => {}
                }
            }
            FocusedPanel::Critters => {
                let map = self.critter_rows(barn).1;
                let count = map.len();
                match key {
                    KeyCode::Down => self.critters_state.select_next(count),
                    KeyCode::Up => self.critters_state.select_prev(),
                    KeyCode::Enter => {
                        if let Some(&i) = map.get(self.critters_state.selected) {
                            return BarnAction::SelectCritter(i);
                        }
                    }
                    _ if typing => {}
                    KeyCode::Char('j') => self.critters_state.select_next(count),
                    KeyCode::Char('k') => self.critters_state.select_prev(),
                    KeyCode::Char('g') => self.critters_state.select_first(),
                    KeyCode::Char('G') => self.critters_state.select_last(count),
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
            hints: Some("[n] add  [/] filter"),
        };
        let livestock_inner = livestock_panel.render(frame, panels[0]);
        let (livestock_items, _) = self.livestock_rows(livestock);
        list::render_filtered_list(
            frame, livestock_inner, &livestock_items,
            &mut self.livestock_state,
            self.focused_panel == FocusedPanel::Livestock,
            &self.filter,
        );

        // Critters on this barn
        let critters_panel = Panel {
            title: "Critters",
            focused: self.focused_panel == FocusedPanel::Critters,
            hints: Some("[n] add  [d] remove  [/] filter"),
        };
        let critters_inner = critters_panel.render(frame, panels[1]);
        let (critter_items, _) = self.critter_rows(barn);
        list::render_filtered_list(
            frame, critters_inner, &critter_items,
            &mut self.critters_state,
            self.focused_panel == FocusedPanel::Critters,
            &self.filter,
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

        let subtitle = header_subtitle(barn);
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

        let subtitle = barn_subtitle(barn);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Editing a barn's host must not drop what the form does not edit.
    ///
    /// `build_updated` rebuilds the whole struct from a literal, so the only
    /// way an unedited field survives is by being carried explicitly.
    #[test]
    fn editing_a_host_preserves_the_barns_other_fields() {
        let original = Barn {
            name: "pi".into(),
            host: Some("10.0.0.2".into()),
            source: Some("ranchhand:cluster".into()),
            connectable: Some(true),
            id: Some("3f0c1e8a-5f2b-4a55-9a3d-6d1c2b7e4f01".into()),
            created_at: Some("2026-08-01T10:00:00+00:00".into()),
            ..Default::default()
        };

        let mut view = BarnContextView::new();
        view.start_edit(&original);
        view.edit_host = "10.0.0.9".into();
        let updated = view.build_updated(&original);

        assert_eq!(updated.host.as_deref(), Some("10.0.0.9"), "the edit must land");
        assert_eq!(updated.source, original.source);
        assert_eq!(updated.connectable, original.connectable);
        assert_eq!(updated.id, original.id, "a host edit must keep the identity");
        assert_eq!(updated.created_at, original.created_at);
    }

    /// `host` still wins where there is one — an explicitly configured address
    /// is the one the user typed.
    #[test]
    fn a_configured_host_is_still_what_the_header_shows() {
        let pi = Barn {
            name: "pi".into(),
            host: Some("10.0.0.2".into()),
            ..Default::default()
        };
        assert_eq!(header_subtitle(&pi), "10.0.0.2");

        let nothing = Barn { name: "ghost".into(), ..Default::default() };
        assert_eq!(header_subtitle(&nothing), "unknown", "and nowhere to dial still says so");
    }

    /// What every barn-bearing header shows, and the reason it is one function:
    /// the barn detail page, the critter detail page and the critter log pane
    /// all answer "where is this barn" and must not answer it three ways.
    ///
    /// `is_local_barn` is the right question *here* — it asks whether the row is
    /// the synthetic placeholder, which has no host, no uuid and no file, so
    /// there is nothing to show but the word.
    #[test]
    fn the_synthetic_placeholder_is_the_only_barn_that_reads_local() {
        let _ranch = crate::testing::temp_ranch();
        assert_eq!(barn_subtitle(&config::local_barn()), "local");

        crate::migrate::adopt_this_machine("camerons-imac").unwrap();
        let imac = config::load_barns()
            .into_iter()
            .find(|b| b.name == "camerons-imac")
            .expect("adoption persists the self-barn");
        assert!(!config::is_local_barn(&imac), "control: not the synthetic placeholder");
        assert_ne!(barn_subtitle(&imac), "local", "only the placeholder reads 'local'");
    }

    /// A configured host still wins, and a barn with nowhere to dial still says
    /// so rather than inventing an address.
    #[test]
    fn a_barn_header_shows_the_address_it_will_be_dialled_at() {
        let _ranch = crate::testing::temp_ranch();
        let pi = Barn { name: "pi".into(), host: Some("10.0.0.2".into()), ..Default::default() };
        assert_eq!(barn_subtitle(&pi), "10.0.0.2");

        let ghost = Barn { name: "ghost".into(), ..Default::default() };
        assert_eq!(barn_subtitle(&ghost), "unknown");
    }
}
