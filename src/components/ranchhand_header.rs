use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::types::RanchHand;

// Peru/tan color to match Node version
const COWBOY_COLOR: Color = Color::Rgb(205, 133, 63);

const COWBOY_ART: &[&str] = &[
    r"       ,_.,",
    r"    __/ `_(__",
    r"   '-..,__..-`",
    r"     @ *Y*|",
    r"     |  - |   ",
    r"  ___'_..'.._",
    r"  /   \_\'/_| \",
];

pub fn render_ranchhand_header(frame: &mut Frame, area: Rect, ranchhand: &RanchHand) {
    let mut lines: Vec<Line> = Vec::new();

    for art_line in COWBOY_ART {
        lines.push(Line::from(Span::styled(
            art_line.to_string(),
            Style::default().fg(COWBOY_COLOR),
        )));
    }

    lines.push(Line::from(""));

    lines.push(Line::from(vec![
        Span::styled(" Name:    ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            ranchhand.name.clone(),
            Style::default().fg(COWBOY_COLOR).add_modifier(Modifier::BOLD),
        ),
    ]));

    lines.push(Line::from(vec![
        Span::styled(" Type:    ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            ranchhand.rh_type.clone(),
            Style::default().fg(Color::White),
        ),
    ]));

    lines.push(Line::from(vec![
        Span::styled(" Project: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            ranchhand.project.clone(),
            Style::default().fg(Color::White),
        ),
    ]));

    lines.push(Line::from(vec![
        Span::styled(" Herd:    ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            ranchhand.herd.clone(),
            Style::default().fg(Color::White),
        ),
    ]));

    if let Some(ref last_sync) = ranchhand.last_sync {
        lines.push(Line::from(vec![
            Span::styled(" Synced:  ", Style::default().fg(Color::DarkGray)),
            Span::styled(last_sync.clone(), Style::default().fg(Color::White)),
        ]));
    }

    let total = lines.len().min(area.height as usize);
    let visible: Vec<Line> = lines.into_iter().take(total).collect();
    let paragraph = Paragraph::new(visible);
    frame.render_widget(paragraph, area);
}
