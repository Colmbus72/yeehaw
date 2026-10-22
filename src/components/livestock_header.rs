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

/// Rows the cow takes.
pub const COW_HEIGHT: u16 = COW_TEMPLATE.len() as u16;

/// Columns the cow takes.
fn cow_width() -> u16 {
    COW_TEMPLATE.iter().map(|l| l.chars().count()).max().unwrap_or(0) as u16
}

/// Room the details need beside the cow before it is worth putting them there.
const DETAILS_MIN_WIDTH: u16 = 30;

/// Below this many rows for the whole view, the cow is dropped and only the
/// details are drawn: the panels underneath are what the page is for.
const MIN_VIEW_HEIGHT_FOR_COW: u16 = 30;

/// How many rows the header wants, given the whole area the view has.
///
/// Wide enough: the cow, with the details beside it — one cow's height and not
/// a row more. Too narrow for that: the cow over the details. Too short for a
/// cow at all: the details alone.
pub fn preferred_height(view: Rect, livestock: &Livestock) -> u16 {
    let details = detail_lines(livestock, "").len() as u16;
    if view.height < MIN_VIEW_HEIGHT_FOR_COW {
        details
    } else if view.width >= cow_width() + 2 + DETAILS_MIN_WIDTH {
        COW_HEIGHT
    } else {
        COW_HEIGHT + 1 + details
    }
}

fn detail_lines(livestock: &Livestock, project_name: &str) -> Vec<Line<'static>> {
    let label = |s: &'static str| Span::styled(s, Style::default().fg(Color::DarkGray));
    let value = |s: String| Span::styled(s, Style::default().fg(Color::White));

    let mut lines = vec![
        Line::from(vec![
            label(" Name:    "),
            Span::styled(livestock.name.clone(), Style::default().fg(BRAND_COLOR).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![label(" Project: "), value(project_name.to_string())]),
        Line::from(vec![label(" Path:    "), value(livestock.path.clone())]),
    ];

    // Only livestock that lives somewhere else gets a `Barn:` row. A livestock
    // on this machine never had one, and adoption renaming it from `None` to
    // the machine's own name is not a fact worth a line of the header.
    if let Some(barn) = crate::config::resolve_livestock_barn(livestock) {
        lines.push(Line::from(vec![label(" Barn:    "), value(barn.to_string())]));
    }
    if let Some(ref repo) = livestock.repo {
        lines.push(Line::from(vec![label(" Repo:    "), value(repo.clone())]));
    }
    lines
}

pub fn render_livestock_header(
    frame: &mut Frame,
    area: Rect,
    livestock: &Livestock,
    project_name: &str,
) {
    let details = detail_lines(livestock, project_name);

    // Not enough rows for the cow: details only.
    if area.height < COW_HEIGHT {
        frame.render_widget(Paragraph::new(details), area);
        return;
    }

    let cow: Vec<Line> = generate_cow_art(livestock, project_name)
        .into_iter()
        .map(|l| Line::from(Span::styled(l, Style::default().fg(BRAND_COLOR).add_modifier(Modifier::BOLD))))
        .collect();

    let cow_w = cow_width();
    if area.width >= cow_w + 2 + DETAILS_MIN_WIDTH {
        // Side by side: the details sit level with the cow's head and body.
        let cow_area = Rect { width: cow_w, height: COW_HEIGHT, ..area };
        frame.render_widget(Paragraph::new(cow), cow_area);

        let top = (COW_HEIGHT.saturating_sub(details.len() as u16)) / 2;
        let details_area = Rect {
            x: area.x + cow_w + 2,
            y: area.y + top,
            width: area.width - cow_w - 2,
            height: area.height.saturating_sub(top),
        };
        frame.render_widget(Paragraph::new(details), details_area);
        return;
    }

    // Narrow: the cow over the details.
    let mut lines = cow;
    lines.push(Line::from(""));
    lines.extend(details);
    frame.render_widget(Paragraph::new(lines), area);
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

    /// On a wide screen the details sit beside the cow, so the header is one
    /// cow tall and every detail line is on screen — the feet included.
    #[test]
    fn a_wide_header_puts_the_details_beside_the_cow() {
        let _ranch = crate::testing::temp_ranch();
        let ls = livestock_on(None);
        let view = Rect::new(0, 0, 120, 40);
        let h = preferred_height(view, &ls);
        assert_eq!(h, COW_HEIGHT);

        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, h)).expect("terminal");
        terminal.draw(|f| render_livestock_header(f, f.area(), &ls, "api")).expect("draw");
        let buf = terminal.backend().buffer().clone();
        let text: String = (0..h)
            .map(|y| (0..120).map(|x| buf.cell((x, y)).unwrap().symbol().to_string()).collect::<String>() + "\n")
            .collect();
        assert!(text.contains("Path:"), "{text}");
        assert!(text.contains("`^'"), "the cow's feet were cut off:\n{text}");
    }

    /// A short terminal drops the cow rather than the panels below it.
    #[test]
    fn a_short_view_gets_the_details_only() {
        let _ranch = crate::testing::temp_ranch();
        let ls = livestock_on(None);
        assert!(preferred_height(Rect::new(0, 0, 120, 24), &ls) < COW_HEIGHT);
    }
}
