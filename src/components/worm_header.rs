use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::types::Worm;

const BRAND_COLOR: Color = Color::Rgb(212, 160, 32);

// Body characters - lighter blocks for a slim worm (matches Node version)
const BODY_CHARS: &[char] = &['\u{2593}', '\u{2592}', '\u{2591}', '\u{2592}']; // ▓ ▒ ░ ▒

// Worm color palette - purples (matches Node version)
const WORM_COLORS: &[Color] = &[
    Color::Rgb(155, 89, 182),  // #9b59b6
    Color::Rgb(142, 68, 173),  // #8e44ad
    Color::Rgb(165, 105, 189), // #a569bd
    Color::Rgb(125, 60, 152),  // #7d3c98
    Color::Rgb(108, 52, 131),  // #6c3483
    Color::Rgb(187, 143, 206), // #bb8fce
    Color::Rgb(210, 180, 222), // #d2b4de
];

/// DJB2 hash for seeding the PRNG.
fn hash_string(s: &str) -> u32 {
    let mut hash: u32 = 5381;
    for b in s.bytes() {
        hash = ((hash << 5).wrapping_add(hash)) ^ (b as u32);
    }
    hash
}

/// Mulberry32 PRNG: returns a float 0..1 and advances the state.
struct SeededRng {
    state: u32,
}

impl SeededRng {
    fn new(seed: u32) -> Self {
        Self { state: seed }
    }

    fn next_f64(&mut self) -> f64 {
        self.state = self.state.wrapping_add(0x6D2B79F5);
        let mut t = self.state;
        t = (t ^ (t >> 15)).wrapping_mul(1 | t);
        t = (t.wrapping_add((t ^ (t >> 7)).wrapping_mul(61 | t))) ^ t;
        ((t ^ (t >> 14)) as f64) / 4294967296.0
    }
}

/// Generate a smooth horizontal worm path across a grid (matches Node version)
fn generate_worm_art(worm: &Worm) -> Vec<String> {
    let width: usize = 30;
    let height: usize = 7;
    let mut grid = vec![vec![' '; width]; height];

    let seed_str = format!("{}{}{}", worm.name, worm.command, worm.schedule);
    let seed = hash_string(&seed_str);
    let mut rng = SeededRng::new(seed);

    // Start from the left side, middle-ish height
    let mut y = (height / 2) as i32 + (rng.next_f64() * 3.0) as i32 - 1;
    y = y.clamp(1, (height as i32) - 2);

    // Walk across the grid left to right
    let mut path: Vec<(usize, usize)> = Vec::new();

    for x in 0..width {
        path.push((x, y as usize));

        // Decide direction: bias toward staying level with gentle undulation
        let r = rng.next_f64();
        if r < 0.25 && y > 0 {
            y -= 1;
        } else if r > 0.75 && y < (height as i32) - 1 {
            y += 1;
        }
    }

    // Draw the single-line worm body
    for i in 0..path.len() {
        let (x, py) = path[i];
        let body_idx = (rng.next_f64() * BODY_CHARS.len() as f64) as usize;
        let body_char = BODY_CHARS[body_idx.min(BODY_CHARS.len() - 1)];
        grid[py][x] = body_char;

        // Connect vertical transitions diagonally
        if i > 0 {
            let (_, prev_y) = path[i - 1];
            if prev_y != py {
                let mid_y = prev_y.min(py);
                if grid[mid_y][x] == ' ' {
                    grid[mid_y][x] = '\u{2591}'; // ░
                }
            }
        }
    }

    // Taper the tail (left end) - use lighter blocks
    if path.len() > 2 {
        let (x0, y0) = path[0];
        grid[y0][x0] = '\u{2591}'; // ░
        let (x1, y1) = path[1];
        grid[y1][x1] = '\u{2591}'; // ░
    }

    // Taper the head (right end) - use lighter blocks
    if path.len() > 2 {
        let (xn, yn) = path[path.len() - 1];
        grid[yn][xn] = '\u{2591}'; // ░
        let (xn1, yn1) = path[path.len() - 2];
        grid[yn1][xn1] = '\u{2591}'; // ░
    }

    grid.iter().map(|row| row.iter().collect()).collect()
}

/// Pick a worm color based on its name (matches Node version palette)
fn get_worm_color(worm: &Worm) -> Color {
    let hash = hash_string(&worm.name) as usize;
    WORM_COLORS[hash % WORM_COLORS.len()]
}

pub fn render_worm_header(frame: &mut Frame, area: Rect, worm: &Worm) {
    let worm_art = generate_worm_art(worm);
    let worm_color = get_worm_color(worm);

    let mut lines: Vec<Line> = Vec::new();

    // Render the worm art
    for row in &worm_art {
        let spans: Vec<Span> = row
            .chars()
            .map(|ch| {
                if ch == ' ' {
                    Span::styled(" ", Style::default())
                } else {
                    Span::styled(ch.to_string(), Style::default().fg(worm_color))
                }
            })
            .collect();
        lines.push(Line::from(spans));
    }

    lines.push(Line::from(""));

    // Metadata
    lines.push(Line::from(vec![
        Span::styled(" Name:     ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            worm.name.clone(),
            Style::default().fg(BRAND_COLOR).add_modifier(Modifier::BOLD),
        ),
    ]));

    lines.push(Line::from(vec![
        Span::styled(" Schedule: ", Style::default().fg(Color::DarkGray)),
        Span::styled(worm.schedule.clone(), Style::default().fg(Color::White)),
    ]));

    lines.push(Line::from(vec![
        Span::styled(" Type:     ", Style::default().fg(Color::DarkGray)),
        Span::styled(worm.worm_type.clone(), Style::default().fg(Color::White)),
    ]));

    let status_color = if worm.enabled { Color::Green } else { Color::Red };
    let status_text = if worm.enabled { "enabled" } else { "disabled" };
    lines.push(Line::from(vec![
        Span::styled(" Status:   ", Style::default().fg(Color::DarkGray)),
        Span::styled(status_text, Style::default().fg(status_color)),
    ]));

    lines.push(Line::from(vec![
        Span::styled(" Command:  ", Style::default().fg(Color::DarkGray)),
        Span::styled(worm.command.clone(), Style::default().fg(Color::White)),
    ]));

    let total = lines.len().min(area.height as usize);
    let visible: Vec<Line> = lines.into_iter().take(total).collect();
    let paragraph = Paragraph::new(visible);
    frame.render_widget(paragraph, area);
}
