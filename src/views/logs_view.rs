use crossterm::event::KeyCode;
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::components::header;
use crate::config;
use crate::ssh;
use crate::types::*;

pub struct LogsView {
    lines: Vec<String>,
    scroll_offset: usize,
    error: Option<String>,
}

impl LogsView {
    pub fn new(_project: &Project, livestock: &Livestock, source_barn: Option<&Barn>) -> Self {
        let mut view = Self {
            lines: vec![],
            scroll_offset: 0,
            error: None,
        };
        view.load_logs(livestock, source_barn);
        view
    }

    fn load_logs(&mut self, livestock: &Livestock, source_barn: Option<&Barn>) {
        let log_path = match &livestock.log_path {
            Some(p) => p.clone(),
            None => {
                self.error = Some("No log path configured".to_string());
                return;
            }
        };

        // Resolve relative paths against livestock path
        let full_path = if log_path.starts_with('/') {
            log_path.clone()
        } else {
            format!("{}/{}", livestock.path, log_path)
        };

        // If remote barn, use SSH to read logs.
        //
        // `barn_is_this_machine`, not `is_local_barn`. The question is whether
        // the file is on some other machine, and after
        // `migrate::adopt_this_machine` the barn a local livestock is reached
        // through carries *this* machine's real name. Asking only about the
        // synthetic `local` sent the read over ssh to a record that
        // deliberately has no host, because you do not ssh to yourself, and the
        // pane showed "SSH error: ... has no host configured" for a file it
        // could have opened directly.
        if let Some(barn) = source_barn {
            if !config::barn_is_this_machine(barn) {
                self.load_remote_logs(barn, &full_path);
                return;
            }
        }

        // Local log reading
        let expanded = expand_path(&full_path);
        let path = std::path::Path::new(&expanded);

        // If it's a directory or ends with /, use find to locate log files
        let content = if expanded.ends_with('/') || path.is_dir() {
            let cmd = format!(
                "find {} -name '*.log' -type f 2>/dev/null | xargs tail -n 200 2>/dev/null",
                expanded
            );
            match std::process::Command::new("sh").args(["-c", &cmd]).output() {
                Ok(output) => String::from_utf8_lossy(&output.stdout).to_string(),
                Err(e) => {
                    self.error = Some(format!("Could not read logs: {}", e));
                    return;
                }
            }
        } else {
            match std::fs::read_to_string(&expanded) {
                Ok(c) => c,
                Err(e) => {
                    self.error = Some(format!("Could not read log: {}", e));
                    return;
                }
            }
        };

        self.lines = content.lines().map(|l| l.to_string()).collect();
        // Take last 200 lines
        if self.lines.len() > 200 {
            self.lines = self.lines.split_off(self.lines.len() - 200);
        }
        let visible = 20usize;
        self.scroll_offset = self.lines.len().saturating_sub(visible);
    }

    fn load_remote_logs(&mut self, barn: &Barn, log_path: &str) {
        // Use find for directory paths, tail for file paths
        let remote_cmd = if log_path.ends_with('/') {
            format!(
                "find {} -name '*.log' -type f 2>/dev/null | xargs tail -n 200 2>/dev/null",
                log_path
            )
        } else {
            format!("tail -n 200 {}", log_path)
        };

        // BatchMode: this runs on the TUI thread, so it must fail rather than
        // block on a password prompt no one can see.
        // allow_failure: `tail` on a log file that has not been created yet
        // exits non-zero. The local branch reads stdout and ignores the status,
        // so this keeps both showing an empty pane rather than an error.
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
        project: &Project,
        livestock: &Livestock,
    ) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // header
                Constraint::Min(1),    // content
                Constraint::Length(1), // scroll indicator
            ])
            .split(area);

        header::render_simple_header(
            frame,
            chunks[0],
            &project.name,
            Some(&format!("Logs: {}", livestock.name)),
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

fn expand_path(path: &str) -> String {
    if path.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(&path[2..]).to_string_lossy().to_string();
        }
    }
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This machine's own barn record, as the logs view receives it after
    /// `migrate::adopt_this_machine`.
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

    fn project() -> Project {
        Project {
            name: "acme".into(),
            path: "/tmp/acme".into(),
            summary: None,
            color: None,
            gradient_spread: None,
            gradient_inverted: None,
            livestock: vec![],
            herds: vec![],
            wiki: vec![],
            issue_provider: None,
            wiki_provider: None,
            id: None,
            created_at: None,
            updated_at: None,
        }
    }

    fn livestock_logging_to(log_path: &std::path::Path) -> Livestock {
        Livestock {
            name: "api".into(),
            path: "/tmp/acme/api".into(),
            barn: None,
            repo: None,
            branch: None,
            log_path: Some(log_path.to_string_lossy().to_string()),
            env_path: None,
            source: None,
            k8s_metadata: None,
            trails: vec![],
        }
    }

    /// THE BUG. A livestock reached through this machine's own barn has its log
    /// file right here. After adoption that barn carries this machine's real
    /// name, which `is_local_barn` calls `false`, so the view went out over ssh
    /// — to a record that deliberately has no host, because you do not ssh to
    /// yourself. The pane showed "SSH error: barn 'imac' has no host configured"
    /// for a file it could have opened.
    #[test]
    fn logs_on_the_adopted_self_barn_are_read_here_instead_of_ssh_to_itself() {
        let ranch = crate::testing::temp_ranch();
        let imac = adopted_self_barn("imac");
        // Both halves of the control. The first says this barn *is* the ssh
        // branch under the old rule; the second says that branch has nothing to
        // dial, so it cannot accidentally succeed.
        assert!(!config::is_local_barn(&imac), "control: not the synthetic placeholder");
        assert!(crate::ssh::dial_host(&imac).is_none(), "control: nothing to dial");

        let log = ranch.dir.path().join("api.log");
        std::fs::write(&log, "yeehaw-log-line\n").unwrap();

        let view = LogsView::new(&project(), &livestock_logging_to(&log), Some(&imac));

        assert!(view.error.is_none(), "reading a local file failed: {:?}", view.error);
        assert_eq!(view.lines, vec!["yeehaw-log-line".to_string()]);
    }

    /// The synthetic row is still this machine on a ranch nobody has adopted.
    #[test]
    fn logs_on_the_synthetic_local_barn_are_still_read_here() {
        let ranch = crate::testing::temp_ranch();

        let log = ranch.dir.path().join("api.log");
        std::fs::write(&log, "yeehaw-log-line\n").unwrap();

        let view = LogsView::new(
            &project(),
            &livestock_logging_to(&log),
            Some(&config::local_barn()),
        );

        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(view.lines, vec!["yeehaw-log-line".to_string()]);
    }

    /// And a livestock on another machine still reads its logs over ssh, or the
    /// pane would quietly show this machine's file in place of the barn's.
    ///
    /// The remote barn has nothing to dial, so "took the ssh branch" is
    /// observable as an SSH error without a network round trip — and the log
    /// file does exist here, so a wrong branch would have produced content.
    #[test]
    fn logs_on_another_machine_still_go_over_ssh() {
        let ranch = crate::testing::temp_ranch();
        let _imac = adopted_self_barn("imac");

        let log = ranch.dir.path().join("api.log");
        std::fs::write(&log, "yeehaw-log-line\n").unwrap();

        let pi = Barn { name: "pi".into(), ..Default::default() };
        let view = LogsView::new(&project(), &livestock_logging_to(&log), Some(&pi));

        assert!(
            view.error.as_deref().is_some_and(|e| e.starts_with("SSH error")),
            "another machine's logs were read from this one: {:?} {:?}",
            view.error,
            view.lines
        );
        assert!(view.lines.is_empty());
    }
}
