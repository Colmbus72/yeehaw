use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph};

const BRAND_COLOR: Color = Color::Rgb(212, 160, 32);

/// Columns the key column is right-aligned in, before its 2 spaces either side.
const KEY_COL: usize = 12;

/// Border on both sides plus [`Padding::new`]'s 2 columns on each — the columns
/// an overlay spends before a single character of content.
const CHROME_W: usize = 2 + 4;
/// The same vertically: one border row each side, one padding row each side.
const CHROME_H: usize = 2 + 2;

/// Narrowest the box is allowed to get. Nothing needs it — the widest row of
/// the widest scope is 45 columns — but a scope with two short bindings in a box
/// hugging them looks like a rendering accident rather than a help panel.
const MIN_W: u16 = 50;

const FOOTER: &str = "Press ? or Esc to close";

struct HotkeyGroup {
    title: &'static str,
    keys: Vec<(&'static str, &'static str)>,
}

/// Columns the widest row of these groups needs.
///
/// Derived rather than declared, and that is the point. This overlay has been
/// clipped silently twice: once vertically, when a row was added past the fixed
/// 22 usable lines and `q`/`Q`/`?` simply stopped being drawn, and once
/// horizontally, where `Open Claude / connect to barn` had been rendering as
/// `...connect to bar` for as long as the string has existed — one column over a
/// hardcoded 50. A `Paragraph` inside a `Block` reports neither. Sizing the box
/// from the text means the next long description grows the box instead.
fn content_width(groups: &[HotkeyGroup]) -> usize {
    let mut w = FOOTER.chars().count();
    for group in groups {
        w = w.max(group.title.chars().count());
        for (key, desc) in &group.keys {
            w = w.max(2 + key.chars().count().max(KEY_COL) + 2 + desc.chars().count());
        }
    }
    w
}

pub fn render_help_overlay(frame: &mut Frame, area: Rect, scope: &str) {
    // Build the content first: both dimensions of the box come from it.
    let groups = get_hotkey_groups(scope);

    let mut lines: Vec<Line> = Vec::new();

    for group in &groups {
        // Section header
        lines.push(Line::from(Span::styled(
            group.title,
            Style::default().fg(BRAND_COLOR).add_modifier(Modifier::BOLD),
        )));

        for (key, desc) in &group.keys {
            let padded_key = format!("  {:>width$}  ", key, width = KEY_COL);
            lines.push(Line::from(vec![
                Span::styled(padded_key, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
                Span::styled(*desc, Style::default().fg(Color::DarkGray)),
            ]));
        }

        lines.push(Line::from("")); // spacing
    }

    // Footer
    lines.push(Line::from(Span::styled(
        FOOTER,
        Style::default().fg(Color::DarkGray),
    )));

    // Centered overlay, sized to what it is about to draw. The `min` against the
    // area is the only clipping left, and it takes a terminal too small to hold
    // the list at all.
    let overlay_width = ((content_width(&groups) + CHROME_W) as u16)
        .max(MIN_W)
        .min(area.width.saturating_sub(4));
    let overlay_height = ((lines.len() + CHROME_H) as u16).min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(overlay_width)) / 2;
    let y = area.y + (area.height.saturating_sub(overlay_height)) / 2;

    let overlay_area = Rect::new(x, y, overlay_width, overlay_height);

    // Clear the area behind the overlay
    frame.render_widget(Clear, overlay_area);

    // Draw bordered box
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Double)
        .border_style(Style::default().fg(BRAND_COLOR))
        .title(Span::styled(" Keyboard Shortcuts ", Style::default().fg(BRAND_COLOR).add_modifier(Modifier::BOLD)))
        .padding(Padding::new(2, 2, 1, 1));

    let inner = block.inner(overlay_area);
    frame.render_widget(block, overlay_area);

    let text = Paragraph::new(lines);
    frame.render_widget(text, inner);
}

fn get_hotkey_groups(scope: &str) -> Vec<HotkeyGroup> {
    let mut groups = Vec::new();

    // Navigation (always present for non-global)
    //
    // The grid is excluded because it is not a list: `j`/`k`/`Tab` reach only
    // the filter panel, and `g`/`G` reach nothing at all. It brings its own
    // sections below.
    if scope != "global" && scope != "sessiongrid" {
        groups.push(HotkeyGroup {
            title: "Navigation",
            keys: vec![
                ("j / ↓", "Move down"),
                ("k / ↑", "Move up"),
                ("g", "Go to top"),
                ("G", "Go to bottom"),
                ("Tab", "Switch panel"),
                ("Enter", "Select item"),
                ("Esc", "Go back"),
            ],
        });
    }

    match scope {
        "global" => {
            groups.push(HotkeyGroup {
                title: "Navigation",
                keys: vec![
                    ("j / ↓", "Move down"),
                    ("k / ↑", "Move up"),
                    ("Tab", "Switch panel"),
                    ("Enter", "Select item"),
                    ("1-9", "Switch to session"),
                ],
            });
            groups.push(HotkeyGroup {
                title: "Actions",
                keys: vec![
                    ("c", "Open Claude / connect to barn"),
                    ("Ctrl+D", "Disconnect from barn"),
                    ("s", "SSH to barn / open shell"),
                    ("n", "Create new item"),
                    ("d", "Delete item"),
                    ("v", "Live session grid"),
                ],
            });
            groups.push(HotkeyGroup {
                title: "System",
                keys: vec![
                    ("Ctrl+R", "Restart Yeehaw"),
                    ("q", "Detach session"),
                    ("Q", "Quit & kill session"),
                    ("?", "Toggle help"),
                ],
            });
        }
        "project" => {
            groups.push(HotkeyGroup {
                title: "Actions",
                keys: vec![
                    ("c", "Open Claude for livestock"),
                    ("s", "Open shell for livestock"),
                    ("w", "Open wiki"),
                    ("i", "Open issues"),
                    ("e", "Edit project"),
                    ("v", "Live session grid"),
                ],
            });
        }
        "barn" => {
            groups.push(HotkeyGroup {
                title: "Actions",
                keys: vec![
                    ("c", "Connect to barn"),
                    ("Ctrl+D", "Disconnect from barn"),
                    ("s", "SSH to barn"),
                    ("e", "Edit barn"),
                    ("v", "Live session grid"),
                ],
            });
        }
        "worm" => {
            groups.push(HotkeyGroup {
                title: "Actions",
                keys: vec![
                    ("t", "Toggle worm enabled/disabled"),
                    ("r", "Run worm now"),
                    ("e", "Edit worm"),
                    ("d", "Delete worm"),
                ],
            });
        }
        // The live session grid. Every key here is answered by
        // `session_grid::handle_input`, `handle_filter_input`, or the global
        // pre-view handler in `app::run` — nothing aspirational.
        //
        // `l` is why this section exists: it has been bound since the sources
        // filter shipped and was named in no overlay, no header and no bottom
        // bar, which makes it a feature only its author knows about.
        "sessiongrid" => {
            groups.push(HotkeyGroup {
                title: "Session Grid",
                keys: vec![
                    ("1-9", "Jump to that session"),
                    ("c", "Claude sessions only"),
                    ("l", "Local sessions only"),
                    ("f", "Open the filter"),
                    ("v / Esc", "Back"),
                ],
            });
            groups.push(HotkeyGroup {
                title: "Filter",
                keys: vec![
                    ("j / k", "Move"),
                    ("Space", "Toggle row"),
                    ("Esc / f", "Close filter"),
                ],
            });
            groups.push(HotkeyGroup {
                title: "System",
                keys: vec![
                    ("Ctrl+R", "Restart Yeehaw"),
                    ("?", "Toggle help"),
                ],
            });
        }
        "livestock" => {
            groups.push(HotkeyGroup {
                title: "Actions",
                keys: vec![
                    ("c", "Open Claude"),
                    ("s", "Open shell"),
                    ("l", "View logs"),
                    ("e", "Edit livestock"),
                ],
            });
        }
        _ => {
            // Generic scope
            groups.push(HotkeyGroup {
                title: "Actions",
                keys: vec![
                    ("Esc", "Go back"),
                ],
            });
        }
    }

    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every scope string `app::help_scope` can hand this overlay.
    ///
    /// Mirrored by hand rather than derived, because the mapping lives in a
    /// match on `AppView` that this module cannot see. `app::tests::
    /// every_view_asks_for_a_scope_this_overlay_knows` is the other half:
    /// it walks the views and checks each one lands on a name in here.
    const SCOPES: &[&str] = &[
        "global",
        "project",
        "barn",
        "worm",
        "livestock",
        "vault",
        "sessiongrid",
        "general",
    ];

    /// Draw the overlay over a `w`x`h` terminal and read the screen back, one
    /// string per row.
    ///
    /// A real `TestBackend` and not a line count, because the failure this
    /// guards is silent: a `Paragraph` inside a `Block` that runs out of room
    /// simply stops drawing. The rows still look right in the source and the
    /// binding is just gone from the screen.
    fn screen(scope: &str, w: u16, h: u16) -> Vec<String> {
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h))
            .expect("test terminal");
        terminal
            .draw(|f| {
                let area = f.area();
                render_help_overlay(f, area, scope);
            })
            .expect("draw");
        let buf = terminal.backend().buffer().clone();
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| {
                        buf.cell((x, y))
                            .map(|c| c.symbol().to_string())
                            .unwrap_or_default()
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn the_help_overlay_shows_every_binding_without_clipping() {
        // Key and description are asserted as one string, exactly as the
        // renderer lays them out. Searching for them separately would pass on a
        // row that drew the key and clipped the description off the right edge —
        // and would also match the letter `c` in any word on the screen.
        for scope in SCOPES {
            let rows = screen(scope, 120, 40);
            for group in get_hotkey_groups(scope) {
                assert!(
                    rows.iter().any(|r| r.contains(group.title)),
                    "scope {scope:?} lost the {:?} heading off the overlay:\n{}",
                    group.title,
                    rows.join("\n")
                );
                for (key, desc) in &group.keys {
                    let needle = format!("{:>12}  {}", key, desc);
                    assert!(
                        rows.iter().any(|r| r.contains(&needle)),
                        "scope {scope:?} clipped {key:?} / {desc:?}:\n{}",
                        rows.join("\n")
                    );
                }
            }
            assert!(
                rows.iter().any(|r| r.contains("Press ? or Esc to close")),
                "scope {scope:?} clipped the last row of the overlay:\n{}",
                rows.join("\n")
            );
        }
    }

    #[test]
    fn the_session_grid_help_lists_every_key_the_grid_answers() {
        // Enumerated from `session_grid::handle_input` and
        // `handle_filter_input`. `l` is the one this task exists for — it has
        // been bound since the sources filter shipped and named nowhere at all —
        // but a section that lists only the new key is no more discoverable than
        // no section, so the whole set is asserted.
        let rows = screen("sessiongrid", 120, 40);
        let text = rows.join("\n");
        for key in ["1-9", "c", "l", "f", "Space", "?"] {
            let needle = format!("{:>12}  ", key);
            assert!(
                rows.iter().any(|r| r.contains(&needle)),
                "the grid's help never mentions {key:?}:\n{text}"
            );
        }
        assert!(
            text.contains("Local sessions only"),
            "`l` is listed without saying what it does:\n{text}"
        );
    }
}
