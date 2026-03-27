use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossterm::event::{self, Event, KeyEventKind};
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;
use ratatui::DefaultTerminal;

const BRAND_COLOR: Color = Color::Rgb(212, 160, 32);
const DARK_GOLD: Color = Color::Rgb(140, 105, 20);

// Tumbleweed ASCII art
const TUMBLEWEED: [&str; 4] = [
    " \u{2591} \u{2591}\u{2592}\u{2591} \u{2591}\u{2592}\u{2591} ",
    "\u{2591}\u{2592} \u{00B7} \u{203F} \u{00B7} \u{2592}\u{2591}",
    "\u{2592}\u{2591} \u{2592}\u{2591}\u{2592}\u{2591} \u{2591}\u{2592} ",
    " \u{2591}\u{2592}\u{2591} \u{2591}\u{2592}\u{2591} \u{2591} ",
];

// YEEHAW in large block letters
const YEEHAW_ART: [&str; 5] = [
    "\u{2588}\u{2588}    \u{2588}\u{2588} \u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588} \u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588} \u{2588}\u{2588}   \u{2588}\u{2588}  \u{2588}\u{2588}\u{2588}\u{2588}\u{2588}  \u{2588}\u{2588}     \u{2588}\u{2588}",
    " \u{2588}\u{2588}  \u{2588}\u{2588}  \u{2588}\u{2588}       \u{2588}\u{2588}       \u{2588}\u{2588}   \u{2588}\u{2588} \u{2588}\u{2588}   \u{2588}\u{2588} \u{2588}\u{2588}     \u{2588}\u{2588}",
    "  \u{2588}\u{2588}\u{2588}\u{2588}   \u{2588}\u{2588}\u{2588}\u{2588}\u{2588}    \u{2588}\u{2588}\u{2588}\u{2588}\u{2588}    \u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588} \u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588} \u{2588}\u{2588}  \u{2588}  \u{2588}\u{2588}",
    "   \u{2588}\u{2588}    \u{2588}\u{2588}       \u{2588}\u{2588}       \u{2588}\u{2588}   \u{2588}\u{2588} \u{2588}\u{2588}   \u{2588}\u{2588} \u{2588}\u{2588} \u{2588}\u{2588}\u{2588} \u{2588}\u{2588}",
    "   \u{2588}\u{2588}    \u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588} \u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588} \u{2588}\u{2588}   \u{2588}\u{2588} \u{2588}\u{2588}   \u{2588}\u{2588}  \u{2588}\u{2588}\u{2588} \u{2588}\u{2588}\u{2588} ",
];

struct CharPos {
    row: usize,
    col: usize,
    ch: char,
}

/// Simple Fisher-Yates shuffle using a time-seeded LCG
fn simple_shuffle<T>(slice: &mut [T]) {
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let mut state = seed.wrapping_add(1);
    for i in (1..slice.len()).rev() {
        // LCG: state = state * 6364136223846793005 + 1
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let j = (state >> 33) as usize % (i + 1);
        slice.swap(i, j);
    }
}

pub fn run_splash(terminal: &mut DefaultTerminal) {
    let start = Instant::now();
    let total_duration = Duration::from_millis(2200);

    // Phase 1: Build tumbleweed (0-800ms)
    // Phase 2: Wave reveal YEEHAW text (800-2200ms)
    let phase1_end = Duration::from_millis(800);

    // Collect tumbleweed char positions and shuffle
    let mut tw_chars: Vec<CharPos> = Vec::new();
    for (row, line) in TUMBLEWEED.iter().enumerate() {
        for (col, ch) in line.chars().enumerate() {
            if ch != ' ' {
                tw_chars.push(CharPos { row, col, ch });
            }
        }
    }
    simple_shuffle(&mut tw_chars);

    // Collect YEEHAW char positions for wave reveal
    let mut yh_chars: Vec<(usize, usize, char, f64)> = Vec::new();
    let wave_origin_row = 2.0_f64;
    let wave_origin_col = 6.0_f64;

    for (row, line) in YEEHAW_ART.iter().enumerate() {
        for (col, ch) in line.chars().enumerate() {
            if ch != ' ' {
                let dy = row as f64 - wave_origin_row;
                let dx = (col as f64 * 0.5) - wave_origin_col;
                let dist = (dx * dx + dy * dy).sqrt();
                yh_chars.push((row, col, ch, dist));
            }
        }
    }
    let max_dist = yh_chars.iter().map(|c| c.3).fold(0.0_f64, f64::max);

    loop {
        let elapsed = start.elapsed();
        if elapsed >= total_duration {
            break;
        }

        // Check for key press to skip
        if event::poll(Duration::from_millis(16)).unwrap_or(false) {
            if let Ok(Event::Key(key)) = event::read() {
                if key.kind == KeyEventKind::Press {
                    break;
                }
            }
        }

        let _ = terminal.draw(|frame| {
            let area = frame.area();
            let content_height = 11u16; // tumbleweed(4) + gap(2) + YEEHAW(5)
            let content_width = 52u16;

            let x = area.x + area.width.saturating_sub(content_width) / 2;
            let y = area.y + area.height.saturating_sub(content_height) / 2;

            let tw_y = y;
            let yh_y = tw_y + 6;

            // Phase 1: Tumbleweed assembly
            let tw_progress = if elapsed < phase1_end {
                elapsed.as_millis() as f64 / phase1_end.as_millis() as f64
            } else {
                1.0
            };

            let chars_to_show = (tw_progress * tw_chars.len() as f64) as usize;

            for char_pos in tw_chars.iter().take(chars_to_show) {
                let cx = x + char_pos.col as u16;
                let cy = tw_y + char_pos.row as u16;
                if cx < area.x + area.width && cy < area.y + area.height {
                    let cell_area = Rect::new(cx, cy, 1, 1);
                    let ch_str = char_pos.ch.to_string();
                    let widget = Paragraph::new(ch_str)
                        .style(Style::default().fg(Color::Rgb(184, 134, 11)));
                    frame.render_widget(widget, cell_area);
                }
            }

            // Phase 2: YEEHAW wave reveal
            if elapsed > phase1_end {
                let wave_elapsed = (elapsed - phase1_end).as_millis() as f64;
                let wave_duration = (total_duration - phase1_end).as_millis() as f64;
                let wave_progress = wave_elapsed / wave_duration;
                let wave_front = wave_progress * (max_dist + 5.0);

                for (row, col, ch, dist) in &yh_chars {
                    if *dist <= wave_front {
                        let cx = x + *col as u16;
                        let cy = yh_y + *row as u16;
                        if cx < area.x + area.width && cy < area.y + area.height {
                            let cell_area = Rect::new(cx, cy, 1, 1);
                            let row_frac = *row as f64 / 4.0;
                            let color = interpolate_color(BRAND_COLOR, DARK_GOLD, row_frac);
                            let ch_str = ch.to_string();
                            let widget = Paragraph::new(ch_str)
                                .style(Style::default().fg(color).add_modifier(Modifier::BOLD));
                            frame.render_widget(widget, cell_area);
                        }
                    }
                }
            }
        });
    }
}

fn interpolate_color(a: Color, b: Color, t: f64) -> Color {
    if let (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) = (a, b) {
        let t = t.clamp(0.0, 1.0);
        Color::Rgb(
            (r1 as f64 + (r2 as f64 - r1 as f64) * t) as u8,
            (g1 as f64 + (g2 as f64 - g1 as f64) * t) as u8,
            (b1 as f64 + (b2 as f64 - b1 as f64) * t) as u8,
        )
    } else {
        a
    }
}
