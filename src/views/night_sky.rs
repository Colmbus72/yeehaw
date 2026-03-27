use crossterm::event::KeyCode;
use ratatui::prelude::*;
use ratatui::widgets::{Paragraph, canvas::{Canvas, Points}};

struct Star {
    x: f64,
    y: f64,
    brightness: u8,
    winking: bool,
    phase: f64, // unique phase offset for each star's pulse
}

pub struct NightSkyView {
    stars: Vec<Star>,
    frame_count: u32,
    paused: bool,
    constellation_text: String,
}

impl NightSkyView {
    pub fn new(label: Option<&str>) -> Self {
        let text = label.unwrap_or("YEEHAW").to_string();
        let mut stars = Vec::new();

        // Generate random-ish stars using a simple LCG
        let mut seed: u64 = 42;
        for _ in 0..60 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let x = ((seed >> 16) % 200) as f64 - 100.0;
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let y = ((seed >> 16) % 100) as f64 - 50.0;
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let brightness = ((seed >> 16) % 200 + 55) as u8;
            let winking = (seed >> 8) % 5 == 0;
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let phase = ((seed >> 16) % 628) as f64 / 100.0; // 0..2π
            stars.push(Star { x, y, brightness, winking, phase });
        }

        Self {
            stars,
            frame_count: 0,
            paused: false,
            constellation_text: text,
        }
    }

    pub fn handle_input(&mut self, key: KeyCode) -> bool {
        match key {
            KeyCode::Esc => return true,
            KeyCode::Char('r') => {
                // Randomize
                let mut seed: u64 = self.frame_count as u64 * 7919;
                for star in &mut self.stars {
                    seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                    star.x = ((seed >> 16) % 200) as f64 - 100.0;
                    seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                    star.y = ((seed >> 16) % 100) as f64 - 50.0;
                    seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                    star.phase = ((seed >> 16) % 628) as f64 / 100.0;
                }
            }
            KeyCode::Char(' ') => {
                self.paused = !self.paused;
            }
            _ => {}
        }
        false
    }

    pub fn tick(&mut self) {
        if !self.paused {
            self.frame_count += 1;
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(1),    // sky
                Constraint::Length(3), // ground
            ])
            .split(area);

        let sky_area = chunks[0];
        let ground_area = chunks[1];

        // Sky with stars using Canvas
        let frame_count = self.frame_count;
        let stars = &self.stars;
        let constellation_text = &self.constellation_text;

        let canvas = Canvas::default()
            .x_bounds([-100.0, 100.0])
            .y_bounds([-50.0, 50.0])
            .paint(move |ctx| {
                // Draw stars with smooth sine-wave pulsing
                let time = frame_count as f64 * 0.05;
                for star in stars {
                    // All stars get a gentle pulse; winking stars get a deeper modulation
                    let pulse = if star.winking {
                        // Deeper, slower wink: 40% modulation depth
                        ((time * 0.8 + star.phase).sin() * 0.4 + 0.6) as f64
                    } else {
                        // Gentle shimmer: 15% modulation depth
                        ((time * 0.3 + star.phase).sin() * 0.15 + 0.85) as f64
                    };
                    let b = (star.brightness as f64 * pulse).clamp(20.0, 255.0) as u8;
                    ctx.draw(&Points {
                        coords: &[(star.x, star.y)],
                        color: Color::Rgb(b, b, b),
                    });
                }

                // Draw constellation text as dots
                let chars: Vec<char> = constellation_text.chars().collect();
                let total_width = chars.len() as f64 * 8.0;
                let start_x = -total_width / 2.0;

                for (ci, _ch) in chars.iter().enumerate() {
                    let cx = start_x + ci as f64 * 8.0 + 4.0;
                    // Simple dot pattern for each character position
                    let pulse = ((frame_count as f64 * 0.1).sin() * 0.3 + 0.7) as f64;
                    let g = (160.0 * pulse) as u8;
                    let r = (212.0 * pulse) as u8;
                    let b_val = (32.0 * pulse) as u8;

                    ctx.draw(&Points {
                        coords: &[(cx, 20.0)],
                        color: Color::Rgb(r, g, b_val),
                    });
                    ctx.draw(&Points {
                        coords: &[(cx - 2.0, 18.0), (cx + 2.0, 18.0)],
                        color: Color::Rgb(r, g, b_val),
                    });
                    ctx.draw(&Points {
                        coords: &[(cx, 16.0)],
                        color: Color::Rgb(r, g, b_val),
                    });
                }
            });

        frame.render_widget(canvas, sky_area);

        // Ground
        let ground_width = ground_area.width as usize;
        let ground_chars = "~-_.~-_.-~_.~-~_.-";
        let mut ground_line = String::new();
        for i in 0..ground_width {
            let idx = (i + self.frame_count as usize / 4) % ground_chars.len();
            ground_line.push(ground_chars.as_bytes()[idx] as char);
        }

        let ground_lines = vec![
            Line::from(Span::styled(&ground_line, Style::default().fg(Color::Rgb(60, 90, 40)))),
            Line::from(""),
            Line::from(vec![
                Span::styled("  [Esc] exit  ", Style::default().fg(Color::DarkGray)),
                Span::styled("[r] randomize  ", Style::default().fg(Color::DarkGray)),
                Span::styled("[Space] pause", Style::default().fg(Color::DarkGray)),
            ]),
        ];
        let ground_text = Paragraph::new(ground_lines);
        frame.render_widget(ground_text, ground_area);
    }
}
