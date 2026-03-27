use std::process::Command;

use crossterm::event::KeyCode;
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::components::header;
use crate::config;
use crate::types::*;

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

    fn load_logs(&mut self, barn: &Barn, critter: &Critter) {
        if config::is_local_barn(barn) {
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
        let host = barn.host.as_deref().unwrap_or("?");
        let user = barn.user.as_deref().unwrap_or("root");
        let port = barn.port.unwrap_or(22);
        let use_journald = critter.use_journald.unwrap_or(true);

        let remote_cmd = if use_journald {
            format!("journalctl -u {} -n 200 --no-pager", critter.service)
        } else if let Some(ref log_path) = critter.log_path {
            format!("tail -n 200 {}", log_path)
        } else {
            self.error = Some("No log source configured".to_string());
            return;
        };

        let mut cmd = Command::new("ssh");
        cmd.arg("-o").arg("StrictHostKeyChecking=no")
            .arg("-p").arg(port.to_string());

        if let Some(ref key) = barn.identity_file {
            cmd.arg("-i").arg(key);
        }

        cmd.arg(format!("{}@{}", user, host)).arg(&remote_cmd);

        match cmd.output() {
            Ok(output) => {
                if output.status.success() {
                    let content = String::from_utf8_lossy(&output.stdout);
                    self.lines = content.lines().map(|l| l.to_string()).collect();
                    let visible = 20usize;
                    self.scroll_offset = self.lines.len().saturating_sub(visible);
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    self.error = Some(format!("SSH error: {}", stderr.trim()));
                }
            }
            Err(e) => {
                self.error = Some(format!("Failed to run SSH: {}", e));
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

        let subtitle = if config::is_local_barn(barn) {
            "local"
        } else {
            barn.host.as_deref().unwrap_or("?")
        };
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
