use crossterm::event::KeyCode;
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::components::header;
use crate::config;
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

        // If remote barn, use SSH to read logs
        if let Some(barn) = source_barn {
            if !config::is_local_barn(barn) {
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
        let host = barn.host.as_deref().unwrap_or("?");
        let user = barn.user.as_deref().unwrap_or("root");
        let port = barn.port.unwrap_or(22);

        // Use find for directory paths, tail for file paths
        let remote_cmd = if log_path.ends_with('/') {
            format!(
                "find {} -name '*.log' -type f 2>/dev/null | xargs tail -n 200 2>/dev/null",
                log_path
            )
        } else {
            format!("tail -n 200 {}", log_path)
        };

        let mut cmd = std::process::Command::new("ssh");
        cmd.arg("-o").arg("StrictHostKeyChecking=no")
            .arg("-o").arg("ConnectTimeout=10")
            .arg("-p").arg(port.to_string());

        if let Some(ref key) = barn.identity_file {
            cmd.arg("-i").arg(key);
        }

        cmd.arg(format!("{}@{}", user, host))
            .arg(&remote_cmd);

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
