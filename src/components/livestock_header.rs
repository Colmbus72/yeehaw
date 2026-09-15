use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::types::Livestock;

const BRAND_COLOR: Color = Color::Rgb(212, 160, 32);

// Base cow ASCII art with @ symbols as pattern spots
// The @ symbols will be replaced with pattern characters for variation
const COW_TEMPLATE: &[&str] = &[
    r"                                 /;    ;\",
    r"                             __  \\____//",
    r"                            /{_\_/   `'\____",
    r"                            \___   (o)  (o  }",
    r"       _______________________/          :--'",
    r#"   ,-,'`@@@@@@@@@@@@@@@@@@@@@@  \_    `__\"#,
    r"  ;:(  @@@@@@@@@@@@@@@@@@@@@@@@   \___(o'o)",
    r"  :: ) @@@@@@@@@@@@@@@@@@@@@@@,'@@(  `===='",
    r"  :: \ @@@@@@: @@@@@@@) @@ (  '@@@'",
    r"  ;; /\ @@@  /`,  @@@@@\   :@@@@@)",
    r"  ::/  )    {_----------:  :~`,~~;",
    r" ;;'`; :   )            :  / `; ;",
    r"`'`' / :  :             :  :  : :",
    r"    )_ \_;             :_ ;  \_\",
    r"    :__\  \             \  \  :  \",
    r"        `^'              `^'  `-^-'",
];

// Pattern characters - space and block characters only
// Space creates gaps in the spots, blocks create varying density
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

/// Generate pattern variation for the cow based on livestock/project data
fn generate_cow_art(livestock: &Livestock, project_name: &str) -> Vec<String> {
    let branch = livestock.branch.as_deref().unwrap_or("default");
    let seed = format!("{}-{}-{}", branch, livestock.name, project_name);
    let hashes = multi_hash(&seed);

    let mut char_index: usize = 0;
    COW_TEMPLATE
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

pub fn render_livestock_header(
    frame: &mut Frame,
    area: Rect,
    livestock: &Livestock,
    project_name: &str,
) {
    let cow_art = generate_cow_art(livestock, project_name);
    let mut lines: Vec<Line> = Vec::new();

    // Render the cow art — all in brand color
    for art_line in &cow_art {
        lines.push(Line::from(Span::styled(
            art_line.clone(),
            Style::default().fg(BRAND_COLOR).add_modifier(Modifier::BOLD),
        )));
    }

    // Blank separator
    lines.push(Line::from(""));

    // Metadata lines
    lines.push(Line::from(vec![
        Span::styled(
            " Name:    ",
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            livestock.name.clone(),
            Style::default().fg(BRAND_COLOR).add_modifier(Modifier::BOLD),
        ),
    ]));

    lines.push(Line::from(vec![
        Span::styled(
            " Project: ",
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(project_name.to_string(), Style::default().fg(Color::White)),
    ]));

    lines.push(Line::from(vec![
        Span::styled(
            " Path:    ",
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            livestock.path.clone(),
            Style::default().fg(Color::White),
        ),
    ]));

    // Only livestock that lives somewhere else gets a `Barn:` row. A livestock
    // on this machine never had one, and adoption renaming it from `None` to
    // the machine's own name is not a fact worth a line of the header.
    if let Some(barn) = crate::config::resolve_livestock_barn(livestock) {
        lines.push(Line::from(vec![
            Span::styled(
                " Barn:    ",
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(barn.to_string(), Style::default().fg(Color::White)),
        ]));
    }

    if let Some(ref repo) = livestock.repo {
        lines.push(Line::from(vec![
            Span::styled(
                " Repo:    ",
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(repo.clone(), Style::default().fg(Color::White)),
        ]));
    }

    let total = lines.len().min(area.height as usize);
    let visible: Vec<Line> = lines.into_iter().take(total).collect();

    let paragraph = Paragraph::new(visible);
    frame.render_widget(paragraph, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn livestock_on(barn: Option<&str>) -> Livestock {
        Livestock {
            name: "web".into(),
            path: "/tmp/web".into(),
            barn: barn.map(str::to_string),
            repo: None,
            branch: None,
            log_path: None,
            env_path: None,
            source: None,
            k8s_metadata: None,
            trails: vec![],
        }
    }

    /// Draws the header and reads the screen back as one string.
    fn screen(livestock: &Livestock) -> String {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 40)).expect("terminal");
        terminal
            .draw(|f| render_livestock_header(f, f.area(), livestock, "api"))
            .expect("draw");
        let buf = terminal.backend().buffer().clone();
        (0..40)
            .map(|y| {
                (0..60)
                    .map(|x| {
                        buf.cell((x, y))
                            .map(|c| c.symbol().to_string())
                            .unwrap_or_default()
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The header names a barn only for livestock that lives somewhere else.
    /// A livestock here never had the row — `barn: None` skipped it — and
    /// adoption renaming it to this machine's own name must not conjure one.
    #[test]
    fn the_header_names_a_barn_only_for_livestock_that_is_elsewhere() {
        let _ranch = crate::testing::temp_ranch();

        assert!(!screen(&livestock_on(None)).contains("Barn:"));
        assert!(!screen(&livestock_on(Some("local"))).contains("Barn:"));
        assert!(screen(&livestock_on(Some("pi"))).contains("Barn:"));

        crate::migrate::adopt_this_machine("imac").unwrap();

        assert!(
            !screen(&livestock_on(Some("imac"))).contains("Barn:"),
            "adoption renamed this livestock; it did not move it"
        );
        assert!(!screen(&livestock_on(None)).contains("Barn:"));
        assert!(screen(&livestock_on(Some("pi"))).contains("Barn:"));
    }
}
