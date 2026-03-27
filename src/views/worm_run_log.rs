use std::fs;

use crossterm::event::KeyCode;
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::components::header;
use crate::config;
use crate::types::*;

pub struct WormRunLogView {
    lines: Vec<String>,
    scroll_offset: usize,
    error: Option<String>,
}

impl WormRunLogView {
    pub fn new(worm: &Worm, run: &WormRun) -> Self {
        let mut view = Self {
            lines: vec![],
            scroll_offset: 0,
            error: None,
        };
        view.load_log(worm, run);
        view
    }

    fn load_log(&mut self, worm: &Worm, run: &WormRun) {
        let log_path = config::worm_runs_for(&worm.name).join(&run.log_file);
        match fs::read_to_string(&log_path) {
            Ok(content) => {
                let cleaned = content.replace('\r', "");
                self.lines = cleaned.lines().map(|l| l.to_string()).collect();
                // Scroll to bottom (most recent output)
                let visible = 20usize;
                self.scroll_offset = self.lines.len().saturating_sub(visible);
                self.error = None;
            }
            Err(e) => {
                self.error = Some(format!("Could not read log: {}", e));
                self.lines.clear();
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
            KeyCode::Esc => return true, // signal go back
            _ => {}
        }
        false
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, worm: &Worm, run: &WormRun) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // header
                Constraint::Length(1), // meta line
                Constraint::Min(1),    // log content
                Constraint::Length(1), // scroll indicator
            ])
            .split(area);

        // Header
        header::render_simple_header(frame, chunks[0], &format!("Worm: {}", worm.name), Some("Run Log"));

        // Meta line
        let exit_info = run.exit_code.map(|c| format!("exit {}", c)).unwrap_or_default();
        let status_str = run.status.as_deref().unwrap_or("unknown");
        let meta = format!(
            "  {} | {} | {} {}",
            run.started_at, run.trigger, status_str, exit_info
        );
        let meta_text = Paragraph::new(meta).style(Style::default().fg(Color::DarkGray));
        frame.render_widget(meta_text, chunks[1]);

        // Log content
        let content_area = chunks[2];
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
                "  [{}-{}/{}]  j/k scroll  g/G top/bottom  PgUp/PgDn",
                self.scroll_offset + 1,
                end,
                self.lines.len()
            );
            let ind_text = Paragraph::new(indicator).style(Style::default().fg(Color::DarkGray));
            frame.render_widget(ind_text, chunks[3]);
        }
    }
}
