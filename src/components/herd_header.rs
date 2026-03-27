use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::types::Herd;

const BRAND_COLOR: Color = Color::Rgb(212, 160, 32);

const FENCE_ART: &[&str] = &[
    r"   _             _              _",
    r"__| |___________| |____________| |________",
    r"__| |___________| |____________| |________",
    r"  | |           | |            | |",
    r"  | |           | |            | |",
    r"__| |___________| |____________| |________",
    r"__| |___________| |____________| |________",
    r"  | |           | |            | |",
];

pub fn render_herd_header(frame: &mut Frame, area: Rect, herd: &Herd, project_name: &str) {
    let mut lines: Vec<Line> = Vec::new();

    for art_line in FENCE_ART {
        lines.push(Line::from(Span::styled(
            art_line.to_string(),
            Style::default().fg(Color::Rgb(139, 115, 85)), // earthy brown
        )));
    }

    lines.push(Line::from(""));

    lines.push(Line::from(vec![
        Span::styled(" Name:      ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            herd.name.clone(),
            Style::default().fg(BRAND_COLOR).add_modifier(Modifier::BOLD),
        ),
    ]));

    lines.push(Line::from(vec![
        Span::styled(" Project:   ", Style::default().fg(Color::DarkGray)),
        Span::styled(project_name.to_string(), Style::default().fg(Color::White)),
    ]));

    lines.push(Line::from(vec![
        Span::styled(" Livestock: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            herd.livestock.len().to_string(),
            Style::default().fg(Color::White),
        ),
    ]));

    lines.push(Line::from(vec![
        Span::styled(" Critters:  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            herd.critters.len().to_string(),
            Style::default().fg(Color::White),
        ),
    ]));

    let total = lines.len().min(area.height as usize);
    let visible: Vec<Line> = lines.into_iter().take(total).collect();
    let paragraph = Paragraph::new(visible);
    frame.render_widget(paragraph, area);
}
