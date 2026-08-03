use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph};

const BRAND_COLOR: Color = Color::Rgb(212, 160, 32);

#[derive(Debug, Clone)]
pub struct ConfirmDialog {
    pub title: String,
    pub message: String,
    pub confirm_label: String,
    pub cancel_label: String,
    pub on_confirm: ConfirmAction,
}

#[derive(Debug, Clone)]
pub enum ConfirmAction {
    DeleteProject(String),
    DeleteBarn(String),
    DeleteWorm(String),
    /// Quit the ranch, closing every open barn connection on the way out.
    /// Carries no names: the sessions to close are re-read from tmux when the
    /// action runs, so a barn connected between the prompt and the `y` is still
    /// closed rather than orphaned.
    QuitClosingBarnSessions,
}

/// Barn names past this many are summarised, so a ranch with thirty connections
/// cannot grow the dialog past the terminal.
const MAX_LISTED_BARNS: usize = 6;

/// Columns a listed barn name gets before it is ellipsised.
const MAX_NAME_COLUMNS: usize = 30;

impl ConfirmDialog {
    pub fn delete_project(name: &str) -> Self {
        Self {
            title: "Delete Project".to_string(),
            message: format!("Delete project \"{}\"?\nThis cannot be undone.", name),
            confirm_label: "y - delete".to_string(),
            cancel_label: "n - cancel".to_string(),
            on_confirm: ConfirmAction::DeleteProject(name.to_string()),
        }
    }

    pub fn delete_barn(name: &str) -> Self {
        Self {
            title: "Delete Barn".to_string(),
            message: format!("Delete barn \"{}\"?\nThis cannot be undone.", name),
            confirm_label: "y - delete".to_string(),
            cancel_label: "n - cancel".to_string(),
            on_confirm: ConfirmAction::DeleteBarn(name.to_string()),
        }
    }

    pub fn delete_worm(name: &str) -> Self {
        Self {
            title: "Delete Worm".to_string(),
            message: format!("Delete worm \"{}\"?\nThis cannot be undone.", name),
            confirm_label: "y - delete".to_string(),
            cancel_label: "n - cancel".to_string(),
            on_confirm: ConfirmAction::DeleteWorm(name.to_string()),
        }
    }

    /// Quitting while barn connections are open. `barn_names` are barn names as
    /// the ranch knows them, never tmux session names — the session name is a
    /// lossy slug plus a hash, so it is meaningless to read and impossible to
    /// invert back into the name on the dashboard.
    ///
    /// `total` is how many sessions will actually close, which can exceed the
    /// names: a barn deleted from the ranch while connected leaves a session
    /// with nothing left to call it. The count stays honest either way.
    pub fn quit_with_barn_sessions(barn_names: &[String], total: usize) -> Self {
        Self {
            title: "Quit Yeehaw".to_string(),
            message: quit_message(barn_names, total),
            confirm_label: "y - quit".to_string(),
            cancel_label: "n - cancel".to_string(),
            on_confirm: ConfirmAction::QuitClosingBarnSessions,
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        // Sized to the message rather than fixed at 40x8: the quit dialog lists
        // one line per open barn, and a fixed box would clip the names it exists
        // to show. Chrome is 6 rows (2 border, 1 top pad, 1 blank, 1 blank, 1
        // label row) and 6 columns (2 border, 2+2 pad).
        let message_lines: Vec<&str> = self.message.lines().collect();
        let labels_width =
            self.confirm_label.chars().count() + 3 + self.cancel_label.chars().count();
        let content = message_lines
            .iter()
            .map(|l| l.chars().count())
            .max()
            .unwrap_or(0)
            .max(labels_width) as u16;

        let width = (content + 6).max(40).min(area.width.saturating_sub(4));
        let height = (message_lines.len() as u16 + 6)
            .max(8)
            .min(area.height.saturating_sub(4));
        let x = area.x + (area.width.saturating_sub(width)) / 2;
        let y = area.y + (area.height.saturating_sub(height)) / 2;

        let dialog_area = Rect::new(x, y, width, height);

        frame.render_widget(Clear, dialog_area);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Double)
            .border_style(Style::default().fg(Color::Red))
            .title(Span::styled(
                format!(" {} ", self.title),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ))
            .padding(Padding::new(2, 2, 1, 0));

        let inner = block.inner(dialog_area);
        frame.render_widget(block, dialog_area);

        // One Line per '\n'. Rendering the message as a single Span put the
        // newline in a cell of its own, so the second half of every message ran
        // off the right edge instead of onto the next row.
        let mut lines = vec![Line::from("")];
        lines.extend(
            message_lines
                .iter()
                .map(|l| Line::from(Span::styled(*l, Style::default().fg(Color::White)))),
        );
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(&self.confirm_label, Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
            Span::raw("   "),
            Span::styled(&self.cancel_label, Style::default().fg(BRAND_COLOR)),
        ]));

        let text = Paragraph::new(lines).alignment(Alignment::Center);
        frame.render_widget(text, inner);
    }
}

/// Body of the quit dialog: what is about to close, by barn name, and what is
/// not. The distinction is the whole point of the prompt — `Q` closes local
/// windows onto remote ranches, it does not stop anything running on them.
///
/// `total` counts the sessions; `barn_names` are the ones with a name to print.
/// The remainder — a list too long for the box, or a session whose barn is gone
/// from the ranch — is summarised, so the count at the top always matches what
/// closing actually does.
fn quit_message(barn_names: &[String], total: usize) -> String {
    let total = total.max(barn_names.len());
    let mut out = if total == 1 {
        "Close 1 barn connection and quit?\n".to_string()
    } else {
        format!("Close {} barn connections and quit?\n", total)
    };

    let listed = barn_names.len().min(MAX_LISTED_BARNS);
    for name in barn_names.iter().take(listed) {
        out.push_str("\n  ");
        out.push_str(&truncate(name, MAX_NAME_COLUMNS));
    }
    let rest = total - listed;
    if rest > 0 {
        out.push_str(&format!("\n  and {} more", rest));
    }

    out.push_str("\n\nThe remote ranches keep running —\nonly these local windows close.");
    out
}

/// Barn names are free-form, and `render` truncates rather than wraps, so a long
/// one is cut here where the ellipsis is visible instead of at the border where
/// the text just stops.
fn truncate(s: &str, columns: usize) -> String {
    if s.chars().count() <= columns {
        return s.to_string();
    }
    s.chars().take(columns.saturating_sub(1)).collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn quit_dialog_names_every_open_barn() {
        let msg = quit_message(&names(&["guided", "camera pi", "BIG UPS"]), 3);
        for n in ["guided", "camera pi", "BIG UPS"] {
            assert!(msg.contains(n), "{n} missing from:\n{msg}");
        }
        assert!(msg.contains('3'), "count missing from:\n{msg}");
    }

    #[test]
    fn quit_dialog_lists_barn_names_not_session_names() {
        // A session name is a slug plus a hash — unreadable, and not what the
        // dashboard shows. Nothing tmux-shaped belongs in this message.
        let msg = quit_message(&names(&["guided"]), 1);
        assert!(!msg.contains("yh-barn"), "session name leaked into:\n{msg}");
    }

    #[test]
    fn quit_dialog_says_the_remote_ranches_survive() {
        let msg = quit_message(&names(&["guided"]), 1);
        assert!(msg.to_lowercase().contains("keep running"), "{msg}");
        assert!(msg.to_lowercase().contains("local"), "{msg}");
    }

    #[test]
    fn quit_dialog_counts_one_connection_in_the_singular() {
        let msg = quit_message(&names(&["guided"]), 1);
        assert!(msg.contains("Close 1 barn connection and quit?"), "{msg}");
    }

    #[test]
    fn quit_dialog_summarises_a_long_list_instead_of_growing_off_screen() {
        let all = names(&["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"]);
        let msg = quit_message(&all, all.len());

        assert!(msg.contains("and 4 more"), "{msg}");
        assert!(msg.contains("Close 10 barn connections and quit?"), "{msg}");
        // Chrome is 6 rows, so the box has to stay well under a short terminal.
        assert!(msg.lines().count() + 6 <= 20, "dialog is {} rows", msg.lines().count() + 6);
    }

    #[test]
    fn quit_dialog_ellipsises_a_barn_name_too_long_for_the_box() {
        let long = "a".repeat(80);
        let msg = quit_message(&[long.clone()], 1);

        assert!(!msg.contains(&long), "full name would run past the border");
        assert!(msg.contains('…'), "{msg}");
        assert!(msg.contains(&"a".repeat(20)), "not enough of the name survived");
    }

    #[test]
    fn quit_dialog_counts_sessions_it_has_no_name_for() {
        // A barn deleted while connected leaves a session with no name to print.
        // Undercounting here would promise to close one thing and close two.
        let msg = quit_message(&names(&["guided"]), 3);

        assert!(msg.contains("Close 3 barn connections and quit?"), "{msg}");
        assert!(msg.contains("guided"), "{msg}");
        assert!(msg.contains("and 2 more"), "{msg}");
    }

    #[test]
    fn confirming_the_quit_dialog_runs_the_quit_action() {
        let dialog = ConfirmDialog::quit_with_barn_sessions(&names(&["guided"]), 1);
        assert!(matches!(dialog.on_confirm, ConfirmAction::QuitClosingBarnSessions));
    }

    #[test]
    fn no_dialog_message_line_can_outgrow_a_small_terminal() {
        // render() widens the box to the longest line but clamps at the terminal,
        // and a Paragraph without wrap truncates rather than reflows. Keeping
        // every line inside 40 columns keeps the box at 46 — comfortable even on
        // an 80x24 terminal — for any input.
        let dialogs = [
            ConfirmDialog::delete_project("proj"),
            ConfirmDialog::delete_barn("barn"),
            ConfirmDialog::delete_worm("worm"),
            ConfirmDialog::quit_with_barn_sessions(&names(&["guided", "camera pi"]), 2),
            ConfirmDialog::quit_with_barn_sessions(&vec!["x".repeat(120); 40], 40),
        ];
        for d in dialogs {
            for line in d.message.lines() {
                assert!(line.chars().count() <= 40, "too wide for the dialog: {line}");
            }
        }
    }
}
