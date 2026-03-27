use ratatui::prelude::*;
use ratatui::widgets::{Paragraph, Wrap};

const BRAND_COLOR: Color = Color::Rgb(212, 160, 32);

// ANSI Shadow font embedded at compile time
const ANSI_SHADOW_FONT: &str = include_str!("../../fonts/ANSI Shadow.flf");

// Tumbleweed mascot art (matches npm version)
const TUMBLEWEED: &[&str] = &[
    " ░ ░▒░ ░▒░",
    "░▒ · ‿ · ▒░",
    "▒░ ▒░▒░ ░▒",
    " ░▒░ ░▒░ ░",
];
const TUMBLEWEED_COLOR: Color = Color::Rgb(184, 134, 11);

/// Parse hex color string like "#ff6b6b" into RGB
fn hex_to_rgb(hex: &str) -> Option<(u8, u8, u8)> {
    let hex = hex.trim_start_matches('#');
    if hex.len() != 6 { return None; }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some((r, g, b))
}

/// Interpolate between two RGB colors
fn interpolate(c1: (u8, u8, u8), c2: (u8, u8, u8), factor: f64) -> Color {
    let r = (c1.0 as f64 + (c2.0 as f64 - c1.0 as f64) * factor).round() as u8;
    let g = (c1.1 as f64 + (c2.1 as f64 - c1.1 as f64) * factor).round() as u8;
    let b = (c1.2 as f64 + (c2.2 as f64 - c1.2 as f64) * factor).round() as u8;
    Color::Rgb(r, g, b)
}

/// Generate gradient colors for each line of ASCII art
fn generate_gradient(
    line_count: usize,
    base_color: &str,
    spread: u8,
    inverted: bool,
) -> Vec<Color> {
    let rgb = match hex_to_rgb(base_color) {
        Some(c) => c,
        None => return vec![BRAND_COLOR; line_count],
    };

    let spread_factor = 1.0 - (spread as f64 / 10.0) * 0.9;

    // Calculate luminance to detect dark colors
    let luminance = (0.299 * rgb.0 as f64 + 0.587 * rgb.1 as f64 + 0.114 * rgb.2 as f64) / 255.0;
    let should_lighten = luminance < 0.3;

    let (mut start, mut end) = if should_lighten {
        let lift = 1.0 + (spread as f64 / 10.0) * 2.0;
        let s = (
            (rgb.0 as f64 * lift + spread as f64 * 8.0).min(255.0) as u8,
            (rgb.1 as f64 * lift + spread as f64 * 8.0).min(255.0) as u8,
            (rgb.2 as f64 * lift + spread as f64 * 8.0).min(255.0) as u8,
        );
        (s, rgb)
    } else {
        let e = (
            (rgb.0 as f64 * spread_factor).round() as u8,
            (rgb.1 as f64 * spread_factor).round() as u8,
            (rgb.2 as f64 * spread_factor).round() as u8,
        );
        (rgb, e)
    };

    if inverted {
        std::mem::swap(&mut start, &mut end);
    }

    (0..line_count)
        .map(|i| {
            let factor = if line_count <= 1 { 0.0 } else { i as f64 / (line_count - 1) as f64 };
            interpolate(start, end, factor)
        })
        .collect()
}

/// Convert text to ANSI Shadow figlet art
fn figlet_text(text: &str) -> String {
    use figlet_rs::FIGfont;
    match FIGfont::from_content(ANSI_SHADOW_FONT) {
        Ok(font) => {
            match font.convert(text) {
                Some(rendered) => rendered.to_string(),
                None => text.to_uppercase(),
            }
        }
        Err(_) => text.to_uppercase(),
    }
}

pub struct HeaderProps<'a> {
    pub text: &'a str,
    pub subtitle: Option<&'a str>,
    pub summary: Option<&'a str>,
    pub color: Option<&'a str>,
    pub gradient_spread: Option<u8>,
    pub gradient_inverted: bool,
    pub version_info: Option<(&'a str, Option<&'a str>)>, // (current, latest)
}

pub fn render_header(frame: &mut Frame, area: Rect, props: &HeaderProps) {
    let ascii = figlet_text(&props.text.to_uppercase());
    let lines: Vec<&str> = ascii.lines().filter(|l| !l.trim().is_empty()).collect();

    let base_color = props.color.unwrap_or("#f0c040");
    let spread = props.gradient_spread.unwrap_or(5);
    let gradient = generate_gradient(lines.len(), base_color, spread, props.gradient_inverted);

    let show_tumbleweed = props.text.to_lowercase() == "yeehaw";

    // Build the lines of text, combining tumbleweed + figlet art side by side
    let mut result_lines: Vec<Line> = Vec::new();

    let tumbleweed_pad = if show_tumbleweed {
        lines.len().saturating_sub(TUMBLEWEED.len()) / 2
    } else {
        0
    };

    let tw_width = TUMBLEWEED.iter().map(|l| l.chars().count()).max().unwrap_or(0);

    for (i, figlet_line) in lines.iter().enumerate() {
        let mut spans: Vec<Span> = Vec::new();

        // Add tumbleweed column if showing
        if show_tumbleweed {
            let tw_idx = i.checked_sub(tumbleweed_pad);
            let tw_line = tw_idx.and_then(|idx| TUMBLEWEED.get(idx));
            if let Some(tw) = tw_line {
                // Pad tumbleweed line to consistent width so figlet text aligns
                let char_count = tw.chars().count();
                let padding = tw_width.saturating_sub(char_count);
                spans.push(Span::styled(
                    format!("  {}{} ", tw, " ".repeat(padding)),
                    Style::default().fg(TUMBLEWEED_COLOR).add_modifier(Modifier::BOLD),
                ));
            } else {
                // Pad with spaces to align
                spans.push(Span::raw(format!("  {:width$} ", "", width = tw_width)));
            }
        } else {
            spans.push(Span::raw("  "));
        }

        // Add figlet line with gradient color
        spans.push(Span::styled(
            figlet_line.to_string(),
            Style::default().fg(gradient[i]),
        ));

        result_lines.push(Line::from(spans));
    }

    // Add version info on the right side of the last line if applicable
    if show_tumbleweed {
        if let Some((current, latest)) = &props.version_info {
            let version_line = if let Some(lat) = latest {
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(format!("v{}", current), Style::default().fg(Color::DarkGray)),
                    Span::styled(" → ", Style::default().fg(Color::DarkGray)),
                    Span::styled(format!("v{}", lat), Style::default().fg(Color::Yellow)),
                ])
            } else {
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(format!("v{}", current), Style::default().fg(Color::DarkGray)),
                ])
            };
            result_lines.push(version_line);
        }
    }

    // Add subtitle/summary line
    if props.subtitle.is_some() || props.summary.is_some() {
        let mut sub_spans = vec![Span::raw("  ")];
        if let Some(sub) = props.subtitle {
            sub_spans.push(Span::styled(sub.to_string(), Style::default().fg(Color::DarkGray)));
        }
        if let Some(summary) = props.summary {
            if props.subtitle.is_some() {
                sub_spans.push(Span::styled(" - ", Style::default().fg(Color::DarkGray)));
            }
            sub_spans.push(Span::styled(summary.to_string(), Style::default().fg(Color::Gray)));
        }
        result_lines.push(Line::from(sub_spans));
    }

    let paragraph = Paragraph::new(result_lines).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

/// Simple header for non-figlet contexts (sub-pages that show entity-specific ASCII art)
pub fn render_simple_header(frame: &mut Frame, area: Rect, title: &str, subtitle: Option<&str>) {
    let text = if let Some(sub) = subtitle {
        Line::from(vec![
            Span::styled(
                format!(" {} ", title),
                Style::default().fg(BRAND_COLOR).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("│ {} ", sub),
                Style::default().fg(Color::DarkGray),
            ),
        ])
    } else {
        Line::from(Span::styled(
            format!(" {} ", title),
            Style::default().fg(BRAND_COLOR).add_modifier(Modifier::BOLD),
        ))
    };

    let header = Paragraph::new(text);
    frame.render_widget(header, area);
}
