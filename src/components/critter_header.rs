use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::types::Critter;

// Dark sea green color to match Node version
const CRITTER_COLOR: Color = Color::Rgb(143, 188, 143);

// Rabbit ASCII art template - @ symbols are pattern spots
const RABBIT_TEMPLATE: &[&str] = &[
    r#"                              __"#,
    r#"                     /\    .-" /"#,
    r#"                    /  ; .'  .' "#,
    r#"                   :   :/  .'   "#,
    r#"                    \  ;-.'     "#,
    r#"       .--""""--..__/     `.    "#,
    r#"     .'@@@@@@@@@@@.'     o  \   "#,
    r#"    /@@@@@@@@@@@@@@@@        ;  "#,
    r#"   :@@@@@@@@@@@@@@@@\       :  "#,
    r#" .-;@@@@@@@@-.@@@@@@@`.__.--'  "#,
    r#":  ;@@@@@@@@@@\@@@@@,   ;       "#,
    r#"'._:@@@@@@@@@@@;@@@:   (        "#,
    r#"    \/  .__    ;    \   `-.     "#,
    r#"     ;     "-,/_..--"`-..__)    "#,
    r#"     '""--._:                  "#,
];

// Pattern characters - space and block characters
const PATTERN_CHARS: &[char] = &[' ', ' ', '\u{2591}', '\u{2591}', '\u{2592}', '\u{2593}', '\u{2588}'];

/// Generate multiple hash values from a string for better distribution
fn multi_hash(s: &str) -> [u32; 3] {
    // First hash - djb2
    let mut hash1: u32 = 5381;
    for b in s.bytes() {
        hash1 = ((hash1 << 5).wrapping_add(hash1)) ^ (b as u32);
    }

    // Second hash - sdbm
    let mut hash2: u32 = 0;
    for b in s.bytes() {
        hash2 = (b as u32)
            .wrapping_add(hash2 << 6)
            .wrapping_add(hash2 << 16)
            .wrapping_sub(hash2);
    }

    // Third hash - fnv-1a inspired
    let mut hash3: u32 = 2166136261;
    for b in s.bytes() {
        hash3 ^= b as u32;
        hash3 = hash3.wrapping_mul(16777619);
    }

    [hash1, hash2, hash3]
}

/// Generate pattern variation for the rabbit based on critter/barn data
fn generate_rabbit_art(critter: &Critter, barn_name: &str) -> Vec<String> {
    let seed = format!("{}-{}-{}", barn_name, critter.name, critter.service);
    let hashes = multi_hash(&seed);

    let mut char_index: usize = 0;
    RABBIT_TEMPLATE
        .iter()
        .enumerate()
        .map(|(line_index, line)| {
            let mut result = String::new();
            for ch in line.chars() {
                if ch == '@' {
                    let h1 = hashes[0];
                    let h2 = hashes[1];
                    let h3 = hashes[2];

                    let mix = (h1 >> (char_index % 17))
                        ^ (h2 >> ((char_index + line_index) % 13))
                        ^ (h3 >> ((char_index.wrapping_mul(7) + line_index.wrapping_mul(3)) % 19));

                    let char_choice = (mix as usize) % PATTERN_CHARS.len();
                    result.push(PATTERN_CHARS[char_choice]);
                    char_index += 1;
                } else {
                    result.push(ch);
                }
            }
            result
        })
        .collect()
}

pub fn render_critter_header(
    frame: &mut Frame,
    area: Rect,
    critter: &Critter,
    barn_name: &str,
) {
    let rabbit_art = generate_rabbit_art(critter, barn_name);
    let mut lines: Vec<Line> = Vec::new();

    // Render the rabbit art in dark sea green
    for art_line in &rabbit_art {
        lines.push(Line::from(Span::styled(
            art_line.clone(),
            Style::default().fg(CRITTER_COLOR).add_modifier(Modifier::BOLD),
        )));
    }

    lines.push(Line::from(""));

    lines.push(Line::from(vec![
        Span::styled(" Name:    ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            critter.name.clone(),
            Style::default().fg(CRITTER_COLOR).add_modifier(Modifier::BOLD),
        ),
    ]));

    lines.push(Line::from(vec![
        Span::styled(" Barn:    ", Style::default().fg(Color::DarkGray)),
        Span::styled(barn_name.to_string(), Style::default().fg(Color::White)),
    ]));

    lines.push(Line::from(vec![
        Span::styled(" Service: ", Style::default().fg(Color::DarkGray)),
        Span::styled(critter.service.clone(), Style::default().fg(Color::White)),
    ]));

    if let Some(ref endpoint) = critter.endpoint {
        lines.push(Line::from(vec![
            Span::styled(" Endpoint:", Style::default().fg(Color::DarkGray)),
            Span::styled(format!(" {}", endpoint), Style::default().fg(Color::White)),
        ]));
    }

    let total = lines.len().min(area.height as usize);
    let visible: Vec<Line> = lines.into_iter().take(total).collect();
    let paragraph = Paragraph::new(visible);
    frame.render_widget(paragraph, area);
}
