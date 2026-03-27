use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph};

const BRAND_COLOR: Color = Color::Rgb(212, 160, 32);

struct HotkeyGroup {
    title: &'static str,
    keys: Vec<(&'static str, &'static str)>,
}

pub fn render_help_overlay(frame: &mut Frame, area: Rect, scope: &str) {
    // Calculate centered overlay dimensions
    let overlay_width = 50u16.min(area.width.saturating_sub(4));
    let overlay_height = 22u16.min(area.height.saturating_sub(4));
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

    // Build hotkey groups based on scope
    let groups = get_hotkey_groups(scope);

    let mut lines: Vec<Line> = Vec::new();

    for group in &groups {
        // Section header
        lines.push(Line::from(Span::styled(
            group.title,
            Style::default().fg(BRAND_COLOR).add_modifier(Modifier::BOLD),
        )));

        for (key, desc) in &group.keys {
            let padded_key = format!("  {:>12}  ", key);
            lines.push(Line::from(vec![
                Span::styled(padded_key, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
                Span::styled(*desc, Style::default().fg(Color::DarkGray)),
            ]));
        }

        lines.push(Line::from("")); // spacing
    }

    // Footer
    lines.push(Line::from(Span::styled(
        "Press ? or Esc to close",
        Style::default().fg(Color::DarkGray),
    )));

    let text = Paragraph::new(lines);
    frame.render_widget(text, inner);
}

fn get_hotkey_groups(scope: &str) -> Vec<HotkeyGroup> {
    let mut groups = Vec::new();

    // Navigation (always present for non-global)
    if scope != "global" {
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
                    ("c", "Open Claude"),
                    ("s", "SSH to barn / open shell"),
                    ("n", "Create new item"),
                    ("d", "Delete item"),
                    ("v", "Night sky visualizer"),
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
                    ("v", "Night sky visualizer"),
                ],
            });
        }
        "barn" => {
            groups.push(HotkeyGroup {
                title: "Actions",
                keys: vec![
                    ("s", "SSH to barn"),
                    ("e", "Edit barn"),
                    ("v", "Night sky visualizer"),
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
