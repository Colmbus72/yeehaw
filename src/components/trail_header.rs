use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::trails::Trail;
use crate::types::Livestock;

const BRAND_COLOR: Color = Color::Rgb(212, 160, 32);

/// DJB2 hash for seeding the PRNG.
fn djb2_hash(s: &str) -> u32 {
    let mut hash: u32 = 5381;
    for b in s.bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(b as u32);
    }
    hash
}

/// Mulberry32 PRNG.
fn mulberry32(state: &mut u32) -> u32 {
    *state = state.wrapping_add(0x6D2B79F5);
    let mut z = *state;
    z = (z ^ (z >> 15)).wrapping_mul(z | 1);
    z ^= z.wrapping_add((z ^ (z >> 7)).wrapping_mul(z | 61));
    z ^ (z >> 14)
}

const TRAIL_CHARS: &[char] = &['.', ':', '~', '-', '='];

/// Generate a trail path art -- a winding path across the grid.
fn generate_trail_art(name: &str, width: usize, height: usize) -> Vec<Vec<char>> {
    let mut grid = vec![vec![' '; width]; height];
    let mut state = djb2_hash(name);

    let mut y = (mulberry32(&mut state) as usize % height).clamp(1, height.saturating_sub(2));

    for x in 0..width {
        let ch = TRAIL_CHARS[mulberry32(&mut state) as usize % TRAIL_CHARS.len()];
        if y < height {
            grid[y][x] = ch;
        }

        // Occasionally shift y up or down for a winding effect
        if mulberry32(&mut state) % 3 == 0 {
            let direction: i32 = if mulberry32(&mut state) % 2 == 0 { 1 } else { -1 };
            y = (y as i32 + direction).clamp(0, (height as i32) - 1) as usize;
        }
    }

    grid
}

pub fn render_trail_header(
    frame: &mut Frame,
    area: Rect,
    trail: &Trail,
    livestock: &Livestock,
) {
    let art_width = 30usize.min(area.width as usize);
    let art_height = 4usize.min(area.height.saturating_sub(5) as usize);

    let grid = generate_trail_art(&trail.name, art_width, art_height);

    let mut lines: Vec<Line> = Vec::new();

    for row in &grid {
        let spans: Vec<Span> = row
            .iter()
            .map(|&ch| {
                if ch == ' ' {
                    Span::styled(" ", Style::default())
                } else {
                    Span::styled(ch.to_string(), Style::default().fg(BRAND_COLOR))
                }
            })
            .collect();
        lines.push(Line::from(spans));
    }

    lines.push(Line::from(""));

    lines.push(Line::from(vec![
        Span::styled(" Trail:      ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            trail.name.clone(),
            Style::default().fg(BRAND_COLOR).add_modifier(Modifier::BOLD),
        ),
    ]));

    lines.push(Line::from(vec![
        Span::styled(" Livestock:  ", Style::default().fg(Color::DarkGray)),
        Span::styled(livestock.name.clone(), Style::default().fg(Color::White)),
    ]));

    let (step_count, runs_on) = trail.first_job()
        .map(|(_, job)| (job.steps.len(), job.runs_on.clone()))
        .unwrap_or((0, "native".to_string()));

    lines.push(Line::from(vec![
        Span::styled(" Steps:      ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{}", step_count),
            Style::default().fg(Color::White),
        ),
    ]));

    lines.push(Line::from(vec![
        Span::styled(" Runs on:    ", Style::default().fg(Color::DarkGray)),
        Span::styled(runs_on, Style::default().fg(Color::White)),
    ]));

    let total = lines.len().min(area.height as usize);
    let visible: Vec<Line> = lines.into_iter().take(total).collect();
    let paragraph = Paragraph::new(visible);
    frame.render_widget(paragraph, area);
}
