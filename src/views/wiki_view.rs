use crossterm::event::KeyCode;
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::components::header;
use crate::components::list::{self, ListItem, ListState, ItemStatus};
use crate::components::markdown::{self, MarkdownState};
use crate::components::panel::Panel;
use crate::types::*;

#[derive(Debug, Clone, Copy, PartialEq)]
enum FocusedPanel {
    Sections,
    Content,
}

pub struct WikiView {
    focused_panel: FocusedPanel,
    sections_state: ListState,
    markdown_state: MarkdownState,
}

impl WikiView {
    pub fn new() -> Self {
        Self {
            focused_panel: FocusedPanel::Sections,
            sections_state: ListState::new(),
            markdown_state: MarkdownState::new(),
        }
    }

    pub fn handle_input(&mut self, key: KeyCode, project: &Project) -> bool {
        match key {
            KeyCode::Tab => {
                self.focused_panel = match self.focused_panel {
                    FocusedPanel::Sections => FocusedPanel::Content,
                    FocusedPanel::Content => FocusedPanel::Sections,
                };
            }
            KeyCode::Esc => return true,
            _ => {}
        }

        match self.focused_panel {
            FocusedPanel::Sections => {
                let count = project.wiki.len();
                match key {
                    KeyCode::Char('j') | KeyCode::Down => {
                        self.sections_state.select_next(count);
                        self.markdown_state = MarkdownState::new();
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        self.sections_state.select_prev();
                        self.markdown_state = MarkdownState::new();
                    }
                    KeyCode::Char('g') => {
                        self.sections_state.select_first();
                        self.markdown_state = MarkdownState::new();
                    }
                    KeyCode::Char('G') => {
                        self.sections_state.select_last(count);
                        self.markdown_state = MarkdownState::new();
                    }
                    _ => {}
                }
            }
            FocusedPanel::Content => {
                let visible_height = 20; // approximate; actual height from render
                markdown::handle_key(
                    &mut self.markdown_state,
                    crossterm::event::KeyEvent::new(key, crossterm::event::KeyModifiers::NONE),
                    visible_height,
                );
            }
        }

        false
    }

    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        project: &Project,
    ) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // header
                Constraint::Min(1),    // content
            ])
            .split(area);

        let provider_label = match &project.wiki_provider {
            Some(WikiProviderConfig::Local) => "local",
            Some(WikiProviderConfig::Linear { .. }) => "linear",
            None => "local",
        };
        header::render_simple_header(frame, chunks[0], &format!("Wiki: {}", project.name), Some(provider_label));

        // Two-panel layout: sections (30%) + content (70%)
        let panels = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(30),
                Constraint::Percentage(70),
            ])
            .margin(1)
            .split(chunks[1]);

        // Sections panel
        let sections_panel = Panel {
            title: "Sections",
            focused: self.focused_panel == FocusedPanel::Sections,
            hints: None,
        };
        let sections_inner = sections_panel.render(frame, panels[0]);

        let section_items: Vec<ListItem> = project.wiki.iter().enumerate().map(|(i, s)| {
            ListItem {
                id: i.to_string(),
                label: s.title.clone(),
                status: Some(ItemStatus::Active),
                meta: None,
                actions: vec![],
            }
        }).collect();

        list::render_list(
            frame,
            sections_inner,
            &section_items,
            &mut self.sections_state,
            self.focused_panel == FocusedPanel::Sections,
            Some(20),
        );

        // Content panel
        let content_panel = Panel {
            title: "Content",
            focused: self.focused_panel == FocusedPanel::Content,
            hints: if self.focused_panel == FocusedPanel::Content { Some("[j/k] scroll") } else { None },
        };
        let content_inner = content_panel.render(frame, panels[1]);

        if let Some(section) = project.wiki.get(self.sections_state.selected) {
            markdown::render_markdown(frame, content_inner, &section.content, &mut self.markdown_state);
        } else {
            let empty = Paragraph::new("No section selected")
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(empty, content_inner);
        }
    }
}
