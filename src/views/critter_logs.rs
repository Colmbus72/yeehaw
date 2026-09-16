use std::process::Command;

use crossterm::event::KeyCode;
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::components::header;
use crate::config;
use crate::ssh;
use crate::types::*;
use crate::views::barn_context::barn_subtitle;

pub struct CritterLogsView {
    lines: Vec<String>,
    scroll_offset: usize,
    error: Option<String>,
}

impl CritterLogsView {
    pub fn new(barn: &Barn, critter: &Critter) -> Self {
        let mut view = Self {
            lines: vec![],
            scroll_offset: 0,
            error: None,
        };
        view.load_logs(barn, critter);
        view
    }

    /// `barn_is_this_machine`, not `is_local_barn`. The question is whether the
    /// critter's logs live on some other machine, and after
    /// `migrate::adopt_this_machine` the barn for the machine this pane is
    /// *running on* carries that machine's real name. Asking only about the
    /// synthetic `local` sent the read over ssh to a record that deliberately
    /// has no host, because you do not ssh to yourself, and every critter on
    /// this machine showed "SSH error: ... has no host configured" instead of
    /// its logs.
    fn load_logs(&mut self, barn: &Barn, critter: &Critter) {
        if config::barn_is_this_machine(barn) {
            self.load_local_logs(critter);
        } else {
            self.load_remote_logs(barn, critter);
        }
    }

    fn load_local_logs(&mut self, critter: &Critter) {
        let use_journald = critter.use_journald.unwrap_or(true);

        if use_journald {
            // Read from journald
            let result = Command::new("journalctl")
                .args(["-u", &critter.service, "-n", "200", "--no-pager"])
                .output();

            match result {
                Ok(output) => {
                    let content = String::from_utf8_lossy(&output.stdout);
                    self.lines = content.lines().map(|l| l.to_string()).collect();
                    let visible = 20usize;
                    self.scroll_offset = self.lines.len().saturating_sub(visible);
                }
                Err(e) => {
                    self.error = Some(format!("Failed to read journald: {}", e));
                }
            }
        } else if let Some(ref log_path) = critter.log_path {
            match std::fs::read_to_string(log_path) {
                Ok(content) => {
                    self.lines = content.lines().map(|l| l.to_string()).collect();
                    if self.lines.len() > 200 {
                        self.lines = self.lines.split_off(self.lines.len() - 200);
                    }
                    let visible = 20usize;
                    self.scroll_offset = self.lines.len().saturating_sub(visible);
                }
                Err(e) => {
                    self.error = Some(format!("Could not read log: {}", e));
                }
            }
        } else {
            self.error = Some("No log source configured".to_string());
        }
    }

    fn load_remote_logs(&mut self, barn: &Barn, critter: &Critter) {
        let use_journald = critter.use_journald.unwrap_or(true);

        let remote_cmd = if use_journald {
            format!("journalctl -u {} -n 200 --no-pager", critter.service)
        } else if let Some(ref log_path) = critter.log_path {
            format!("tail -n 200 {}", log_path)
        } else {
            self.error = Some("No log source configured".to_string());
            return;
        };

        // BatchMode: this runs on the TUI thread, so it must fail rather than
        // block on a password prompt no one can see.
        // allow_failure: `journalctl` for a unit with no entries and `tail` on a
        // not-yet-created log both exit non-zero. The local branch ignores the
        // exit status, so this keeps both rendering an empty pane.
        match ssh::run(barn, &remote_cmd, ssh::Opts { batch: true, allow_failure: true, ..Default::default() }) {
            Ok(content) => {
                self.lines = content.lines().map(|l| l.to_string()).collect();
                let visible = 20usize;
                self.scroll_offset = self.lines.len().saturating_sub(visible);
            }
            Err(e) => {
                self.error = Some(format!("SSH error: {}", e));
            }
        }
    }

    pub fn handle_input(&mut self, key: KeyCode) -> bool {
        let visible = 20usize;
        let max_offset = self.lines.len().saturating_sub(visible);

        match key {
            KeyCode::Char('j') | KeyCode::Down => {
                self.scroll_offset = (self.scroll_offset + 1).min(max_offset);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.scroll_offset = self.scroll_offset.saturating_sub(1);
            }
            KeyCode::Char('g') => {
                self.scroll_offset = 0;
            }
            KeyCode::Char('G') => {
                self.scroll_offset = max_offset;
            }
            KeyCode::PageDown => {
                self.scroll_offset = (self.scroll_offset + visible).min(max_offset);
            }
            KeyCode::PageUp => {
                self.scroll_offset = self.scroll_offset.saturating_sub(visible);
            }
            KeyCode::Esc => return true,
            _ => {}
        }
        false
    }

    pub fn render(
        &self,
        frame: &mut Frame,
        area: Rect,
        barn: &Barn,
        critter: &Critter,
    ) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // header
                Constraint::Min(1),    // content
                Constraint::Length(1), // indicator
            ])
            .split(area);

        let subtitle = barn_subtitle(barn);
        header::render_simple_header(
            frame,
            chunks[0],
            &format!("Critter Logs: {}", critter.name),
            Some(subtitle),
        );

        let content_area = chunks[1];
        let visible_height = content_area.height as usize;

        if let Some(ref error) = self.error {
            let err = Paragraph::new(format!("  {}", error)).style(Style::default().fg(Color::Red));
            frame.render_widget(err, content_area);
        } else if self.lines.is_empty() {
            let empty = Paragraph::new("  No log content").style(Style::default().fg(Color::DarkGray));
            frame.render_widget(empty, content_area);
        } else {
            let visible_lines: Vec<Line> = self.lines
                .iter()
                .skip(self.scroll_offset)
                .take(visible_height)
                .map(|l| Line::from(format!("  {}", l)))
                .collect();
            let text = Paragraph::new(visible_lines);
            frame.render_widget(text, content_area);
        }

        // Scroll indicator
        if !self.lines.is_empty() {
            let end = (self.scroll_offset + visible_height).min(self.lines.len());
            let indicator = format!(
                "  [{}-{}/{}]  j/k scroll  g/G top/bottom  r refresh",
                self.scroll_offset + 1,
                end,
                self.lines.len()
            );
            let ind_text = Paragraph::new(indicator).style(Style::default().fg(Color::DarkGray));
            frame.render_widget(ind_text, chunks[2]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This machine's own barn record, as the critter log pane receives it
    /// after `migrate::adopt_this_machine`.
    ///
    /// `addresses` is emptied on purpose, and that is what makes these tests
    /// safe to run against a live ranch. The list holds what a *peer* dials to
    /// reach this machine — `ssh::dial_host` falls back to it — so a test that
    /// took the ssh branch by mistake would open a real connection to the
    /// developer's own machine and write its host key into `~/.ssh/known_hosts`.
    /// Emptied, the ssh branch fails before spawning anything, which is what
    /// lets an assertion about *which branch ran* mean something.
    fn adopted_self_barn(name: &str) -> Barn {
        crate::migrate::adopt_this_machine(name).unwrap();
        let mut barn = config::load_barns()
            .into_iter()
            .find(|b| b.name == name)
            .expect("adoption mints this machine's record");
        barn.addresses.clear();
        barn
    }

    /// A critter that logs to a file rather than journald — the only shape this
    /// suite can assert on, since the machines it runs on have no journald and
    /// the local branch would fail on `journalctl` for reasons unrelated to the
    /// branch it took.
    fn critter_logging_to(log_path: &std::path::Path) -> Critter {
        Critter {
            name: "mysql".into(),
            service: "mysql.service".into(),
            service_path: None,
            config_path: None,
            log_path: Some(log_path.to_string_lossy().to_string()),
            use_journald: Some(false),
            source: None,
            endpoint: None,
            port: None,
            k8s_metadata: None,
            tf_metadata: None,
        }
    }

    /// THE BUG. A critter on this machine's own barn has its log file right
    /// here. After adoption that barn carries this machine's real name, which
    /// `is_local_barn` calls `false`, so the pane went out over ssh — to a
    /// record that deliberately has no host, because you do not ssh to yourself
    /// — and showed "SSH error: barn 'imac' has no host configured" for a file
    /// it could have opened.
    #[test]
    fn critter_logs_on_the_adopted_self_barn_are_read_here_instead_of_ssh_to_itself() {
        let ranch = crate::testing::temp_ranch();
        let imac = adopted_self_barn("imac");
        // Both halves of the control. The first says this barn *is* the ssh
        // branch under the old rule; the second says that branch has nothing to
        // dial, so it cannot accidentally succeed.
        assert!(!config::is_local_barn(&imac), "control: not the synthetic placeholder");
        assert!(ssh::dial_host(&imac).is_none(), "control: nothing to dial");

        let log = ranch.dir.path().join("mysql.log");
        std::fs::write(&log, "yeehaw-critter-line\n").unwrap();

        let view = CritterLogsView::new(&imac, &critter_logging_to(&log));

        assert!(view.error.is_none(), "reading a local file failed: {:?}", view.error);
        assert_eq!(view.lines, vec!["yeehaw-critter-line".to_string()]);
    }

    /// The synthetic row is still this machine on a ranch nobody has adopted.
    #[test]
    fn critter_logs_on_the_synthetic_local_barn_are_still_read_here() {
        let ranch = crate::testing::temp_ranch();

        let log = ranch.dir.path().join("mysql.log");
        std::fs::write(&log, "yeehaw-critter-line\n").unwrap();

        let view = CritterLogsView::new(&config::local_barn(), &critter_logging_to(&log));

        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(view.lines, vec!["yeehaw-critter-line".to_string()]);
    }

    /// And a critter on another machine still reads its logs over ssh, or the
    /// pane would quietly show this machine's file in place of the barn's.
    ///
    /// The remote barn has nothing to dial, so "took the ssh branch" is
    /// observable as an SSH error without a network round trip — and the log
    /// file does exist here, so a wrong branch would have produced content.
    #[test]
    fn critter_logs_on_another_machine_still_go_over_ssh() {
        let ranch = crate::testing::temp_ranch();
        let _imac = adopted_self_barn("imac");

        let log = ranch.dir.path().join("mysql.log");
        std::fs::write(&log, "yeehaw-critter-line\n").unwrap();

        let pi = Barn { name: "pi".into(), ..Default::default() };
        let view = CritterLogsView::new(&pi, &critter_logging_to(&log));

        assert!(
            view.error.as_deref().is_some_and(|e| e.starts_with("SSH error")),
            "another machine's critter logs were read from this one: {:?} {:?}",
            view.error,
            view.lines
        );
        assert!(view.lines.is_empty());
    }
}
