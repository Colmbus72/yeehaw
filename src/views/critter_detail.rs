use crossterm::event::KeyCode;
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::components::critter_header;
use crate::components::header;
use crate::components::text_input::TextInput;
use crate::config;
use crate::types::*;

const BRAND_COLOR: Color = Color::Rgb(212, 160, 32);

pub enum CritterAction {
    None,
    OpenLogs,
    UpdateCritter(Critter),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum EditMode {
    Normal,
    EditName,
    EditService,
    EditConfigPath,
    EditLogPath,
    EditJournald,
}

pub struct CritterDetailView {
    edit_mode: EditMode,
    text_input: TextInput,
    edit_name: String,
    edit_service: String,
    edit_config_path: String,
    edit_log_path: String,
    edit_use_journald: bool,
}

impl CritterDetailView {
    pub fn new() -> Self {
        Self {
            edit_mode: EditMode::Normal,
            text_input: TextInput::new(""),
            edit_name: String::new(),
            edit_service: String::new(),
            edit_config_path: String::new(),
            edit_log_path: String::new(),
            edit_use_journald: true,
        }
    }

    pub fn is_editing(&self) -> bool {
        self.edit_mode != EditMode::Normal
    }

    fn start_edit(&mut self, critter: &Critter) {
        self.edit_name = critter.name.clone();
        self.edit_service = critter.service.clone();
        self.edit_config_path = critter.config_path.clone().unwrap_or_default();
        self.edit_log_path = critter.log_path.clone().unwrap_or_default();
        self.edit_use_journald = critter.use_journald.unwrap_or(true);
        self.edit_mode = EditMode::EditName;
        self.text_input = TextInput::new(&self.edit_name);
    }

    fn cancel_edit(&mut self) {
        self.edit_mode = EditMode::Normal;
    }

    fn advance_field(&mut self) -> Option<()> {
        let value = self.text_input.value.trim().to_string();

        match self.edit_mode {
            EditMode::EditName => {
                if !value.is_empty() { self.edit_name = value; }
                self.edit_mode = EditMode::EditService;
                self.text_input = TextInput::new(&self.edit_service);
            }
            EditMode::EditService => {
                if !value.is_empty() { self.edit_service = value; }
                self.edit_mode = EditMode::EditConfigPath;
                self.text_input = TextInput::new(&self.edit_config_path);
            }
            EditMode::EditConfigPath => {
                self.edit_config_path = value;
                self.edit_mode = EditMode::EditLogPath;
                self.text_input = TextInput::new(&self.edit_log_path);
            }
            EditMode::EditLogPath => {
                self.edit_log_path = value;
                self.edit_mode = EditMode::EditJournald;
                // No text input needed for toggle
            }
            EditMode::EditJournald => {
                return None; // Signal save
            }
            EditMode::Normal => {}
        }
        Some(())
    }

    fn build_updated(&self, original: &Critter) -> Critter {
        Critter {
            name: self.edit_name.clone(),
            service: self.edit_service.clone(),
            service_path: original.service_path.clone(),
            config_path: if self.edit_config_path.is_empty() { None } else { Some(self.edit_config_path.clone()) },
            log_path: if self.edit_log_path.is_empty() { None } else { Some(self.edit_log_path.clone()) },
            use_journald: Some(self.edit_use_journald),
            source: original.source.clone(),
            endpoint: original.endpoint.clone(),
            port: original.port,
            k8s_metadata: original.k8s_metadata.clone(),
            tf_metadata: original.tf_metadata.clone(),
        }
    }

    pub fn handle_input(&mut self, key: KeyCode, _barn: &Barn, critter: &Critter) -> CritterAction {
        if self.edit_mode != EditMode::Normal {
            if key == KeyCode::Esc {
                self.cancel_edit();
                return CritterAction::None;
            }

            // Special handling for journald toggle
            if self.edit_mode == EditMode::EditJournald {
                match key {
                    KeyCode::Char(' ') => {
                        self.edit_use_journald = !self.edit_use_journald;
                        return CritterAction::None;
                    }
                    KeyCode::Enter => {
                        let updated = self.build_updated(critter);
                        self.edit_mode = EditMode::Normal;
                        return CritterAction::UpdateCritter(updated);
                    }
                    _ => return CritterAction::None,
                }
            }

            let submitted = self.text_input.handle_input(key);
            if submitted {
                if self.advance_field().is_none() {
                    let updated = self.build_updated(critter);
                    self.edit_mode = EditMode::Normal;
                    return CritterAction::UpdateCritter(updated);
                }
            }
            return CritterAction::None;
        }

        match key {
            KeyCode::Char('e') => {
                self.start_edit(critter);
                CritterAction::None
            }
            KeyCode::Char('l') => CritterAction::OpenLogs,
            _ => CritterAction::None,
        }
    }

    pub fn render(
        &self,
        frame: &mut Frame,
        area: Rect,
        barn: &Barn,
        critter: &Critter,
    ) {
        if self.edit_mode != EditMode::Normal {
            self.render_edit_form(frame, area, barn, critter);
            return;
        }

        let barn_name = if config::is_local_barn(barn) { "local" } else { barn.host.as_deref().unwrap_or("unknown") };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(22), // ASCII header (15 lines rabbit + blank + 4 metadata + margin)
                Constraint::Min(1),     // content area
                Constraint::Length(1),  // hints
            ])
            .split(area);

        critter_header::render_critter_header(frame, chunks[0], critter, barn_name);

        // Detail info below the header
        let mut detail_lines: Vec<Line> = Vec::new();
        if let Some(ref config_path) = critter.config_path {
            detail_lines.push(Line::from(vec![
                Span::styled(" Config:  ", Style::default().fg(Color::DarkGray)),
                Span::styled(config_path.clone(), Style::default().fg(Color::White)),
            ]));
        }
        if let Some(ref log_path) = critter.log_path {
            detail_lines.push(Line::from(vec![
                Span::styled(" Logs:    ", Style::default().fg(Color::DarkGray)),
                Span::styled(log_path.clone(), Style::default().fg(Color::White)),
            ]));
        }
        let journald = critter.use_journald.unwrap_or(true);
        detail_lines.push(Line::from(vec![
            Span::styled(" Journald:", Style::default().fg(Color::DarkGray)),
            Span::styled(
                if journald { " Yes" } else { " No" },
                Style::default().fg(if journald { Color::Green } else { Color::DarkGray }),
            ),
        ]));
        if let Some(ref port) = critter.port {
            detail_lines.push(Line::from(vec![
                Span::styled(" Port:    ", Style::default().fg(Color::DarkGray)),
                Span::styled(port.to_string(), Style::default().fg(Color::White)),
            ]));
        }
        if !detail_lines.is_empty() {
            frame.render_widget(Paragraph::new(detail_lines), chunks[1]);
        }

        // Hints
        let hints = Paragraph::new("  [l] view logs  [e] edit  [Esc] back")
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(hints, chunks[2]);
    }

    fn render_edit_form(
        &self,
        frame: &mut Frame,
        area: Rect,
        barn: &Barn,
        critter: &Critter,
    ) {
        let (label, step) = match self.edit_mode {
            EditMode::EditName => ("Name:", "1/5"),
            EditMode::EditService => ("Service:", "2/5"),
            EditMode::EditConfigPath => ("Config path (optional):", "3/5"),
            EditMode::EditLogPath => ("Log path (optional):", "4/5"),
            EditMode::EditJournald => ("Use journald:", "5/5"),
            EditMode::Normal => return,
        };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // header
                Constraint::Length(2), // title
                Constraint::Length(3), // completed fields
                Constraint::Length(1), // label
                Constraint::Length(1), // input / toggle
                Constraint::Length(2), // hints
                Constraint::Min(1),
            ])
            .split(area);

        let subtitle = if config::is_local_barn(barn) { "local" } else { barn.host.as_deref().unwrap_or("unknown") };
        header::render_simple_header(
            frame,
            chunks[0],
            &format!("Critter: {}", critter.name),
            Some(subtitle),
        );

        let title = Paragraph::new(format!("  Edit Critter (Step {})", step))
            .style(Style::default().fg(BRAND_COLOR).add_modifier(Modifier::BOLD));
        frame.render_widget(title, chunks[1]);

        // Show completed fields
        let mut completed: Vec<Line> = Vec::new();
        let fields: Vec<(&str, &str, EditMode)> = vec![
            ("Name", &self.edit_name, EditMode::EditName),
            ("Service", &self.edit_service, EditMode::EditService),
            ("Config", &self.edit_config_path, EditMode::EditConfigPath),
            ("Log path", &self.edit_log_path, EditMode::EditLogPath),
        ];
        for (fname, fval, fmode) in &fields {
            if *fmode as u8 >= self.edit_mode as u8 {
                break;
            }
            completed.push(Line::from(vec![
                Span::styled(format!("  {}: ", fname), Style::default().fg(Color::DarkGray)),
                Span::raw(if fval.is_empty() { "—" } else { fval }),
            ]));
        }
        if !completed.is_empty() {
            frame.render_widget(Paragraph::new(completed), chunks[2]);
        }

        let label_text = Paragraph::new(format!("  {}", label))
            .style(Style::default().fg(Color::White));
        frame.render_widget(label_text, chunks[3]);

        if self.edit_mode == EditMode::EditJournald {
            // Toggle display
            let toggle_text = if self.edit_use_journald { "Yes" } else { "No" };
            let toggle = Paragraph::new(format!("    [{}]", toggle_text))
                .style(Style::default().fg(BRAND_COLOR).add_modifier(Modifier::BOLD));
            frame.render_widget(toggle, chunks[4]);

            let hints = Paragraph::new("  Space: toggle  Enter: save  Esc: cancel")
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(hints, chunks[5]);
        } else {
            let input_area = Rect {
                x: chunks[4].x + 4,
                y: chunks[4].y,
                width: chunks[4].width.saturating_sub(4),
                height: 1,
            };
            self.text_input.render(frame, input_area);

            let hints = Paragraph::new("  Enter: next field  Esc: cancel")
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(hints, chunks[5]);
        }
    }
}
