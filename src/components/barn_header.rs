use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::types::Barn;

const BRAND_COLOR: Color = Color::Rgb(212, 160, 32);

const BARN_ART: &[&str] = &[
    r"       _.-^-._    .--.",
    r"    .-'   _   '-. |__|",
    r"   /     |_|     \|  |",
    r"  /               \  |",
    r" /|     _____     |\ |",
    r"  |    |==|==|    |  |",
    r"  |    |--|--|    |  |",
    r"  |    |==|==|    |  |",
];

/// Grey gradient from light to dark for coloring the barn art.
const GREY_GRADIENT: &[Color] = &[
    Color::Rgb(160, 160, 160),
    Color::Rgb(130, 130, 130),
    Color::Rgb(100, 100, 100),
    Color::Rgb(80, 80, 80),
    Color::Rgb(60, 60, 60),
];

pub fn render_barn_header(frame: &mut Frame, area: Rect, barn: &Barn) {
    let mut lines: Vec<Line> = Vec::new();

    for (row_idx, art_line) in BARN_ART.iter().enumerate() {
        let color = GREY_GRADIENT[row_idx % GREY_GRADIENT.len()];
        lines.push(Line::from(Span::styled(
            art_line.to_string(),
            Style::default().fg(color),
        )));
    }

    lines.push(Line::from(""));

    lines.push(Line::from(vec![
        Span::styled(" Name: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            barn.name.clone(),
            Style::default().fg(BRAND_COLOR).add_modifier(Modifier::BOLD),
        ),
    ]));

    if let Some(ref host) = barn.host {
        let user_prefix = barn.user.as_deref().map_or(String::new(), |u| format!("{}@", u));
        let port_suffix = barn.port.map_or(String::new(), |p| format!(":{}", p));
        lines.push(Line::from(vec![
            Span::styled(" Host: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{}{}{}", user_prefix, host, port_suffix),
                Style::default().fg(Color::White),
            ),
        ]));
    }

    let critter_count = barn.critters.len();
    lines.push(Line::from(vec![
        Span::styled(" Critters: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            critter_count.to_string(),
            Style::default().fg(Color::White),
        ),
    ]));

    let total = lines.len().min(area.height as usize);
    let visible: Vec<Line> = lines.into_iter().take(total).collect();
    let paragraph = Paragraph::new(visible);
    frame.render_widget(paragraph, area);
}
