use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph};

const BRAND_COLOR: Color = Color::Rgb(212, 160, 32);

#[derive(Debug, Clone)]
pub struct ConfirmDialog {
    pub title: String,
    pub message: String,
    pub confirm_label: String,
    pub cancel_label: String,
    pub on_confirm: ConfirmAction,
}

#[derive(Debug, Clone)]
pub enum ConfirmAction {
    DeleteProject(String),
    DeleteBarn(String),
    DeleteWorm(String),
}

impl ConfirmDialog {
    pub fn delete_project(name: &str) -> Self {
        Self {
            title: "Delete Project".to_string(),
            message: format!("Delete project \"{}\"?\nThis cannot be undone.", name),
            confirm_label: "y - delete".to_string(),
            cancel_label: "n - cancel".to_string(),
            on_confirm: ConfirmAction::DeleteProject(name.to_string()),
        }
    }

    pub fn delete_barn(name: &str) -> Self {
        Self {
            title: "Delete Barn".to_string(),
            message: format!("Delete barn \"{}\"?\nThis cannot be undone.", name),
            confirm_label: "y - delete".to_string(),
            cancel_label: "n - cancel".to_string(),
            on_confirm: ConfirmAction::DeleteBarn(name.to_string()),
        }
    }

    pub fn delete_worm(name: &str) -> Self {
        Self {
            title: "Delete Worm".to_string(),
            message: format!("Delete worm \"{}\"?\nThis cannot be undone.", name),
            confirm_label: "y - delete".to_string(),
            cancel_label: "n - cancel".to_string(),
            on_confirm: ConfirmAction::DeleteWorm(name.to_string()),
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let width = 40u16.min(area.width.saturating_sub(4));
        let height = 8u16.min(area.height.saturating_sub(4));
        let x = area.x + (area.width.saturating_sub(width)) / 2;
        let y = area.y + (area.height.saturating_sub(height)) / 2;

        let dialog_area = Rect::new(x, y, width, height);

        frame.render_widget(Clear, dialog_area);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Double)
            .border_style(Style::default().fg(Color::Red))
            .title(Span::styled(
                format!(" {} ", self.title),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ))
            .padding(Padding::new(2, 2, 1, 0));

        let inner = block.inner(dialog_area);
        frame.render_widget(block, dialog_area);

        let lines = vec![
            Line::from(""),
            Line::from(Span::styled(&self.message, Style::default().fg(Color::White))),
            Line::from(""),
            Line::from(vec![
                Span::styled(&self.confirm_label, Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                Span::raw("   "),
                Span::styled(&self.cancel_label, Style::default().fg(BRAND_COLOR)),
            ]),
        ];

        let text = Paragraph::new(lines).alignment(Alignment::Center);
        frame.render_widget(text, inner);
    }
}
