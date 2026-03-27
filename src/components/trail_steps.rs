use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::trails::provider::StepStatus;

pub struct StepItem {
    pub name: String,
    pub status: StepStatus,
}

pub struct StepListState {
    pub selected: usize,
}

impl StepListState {
    pub fn new() -> Self {
        Self { selected: 0 }
    }

    pub fn select_next(&mut self, count: usize) {
        if count > 0 {
            self.selected = (self.selected + 1).min(count - 1);
        }
    }

    pub fn select_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }
}

/// Render the step status indicator character.
/// `tick` is a frame counter used for the pulse animation.
fn status_indicator(status: &StepStatus, tick: u64) -> (char, Color) {
    match status {
        StepStatus::Pending => ('\u{2591}', Color::DarkGray),     // light shade
        StepStatus::Running => {
            // Pulse between light shade and medium shade (~500ms at ~10fps)
            if (tick / 5) % 2 == 0 {
                ('\u{2591}', Color::Yellow) // light shade
            } else {
                ('\u{2592}', Color::Yellow) // medium shade
            }
        }
        StepStatus::Success => ('\u{2713}', Color::Green),        // checkmark
        StepStatus::Failed { .. } => ('\u{2717}', Color::Red),    // ballot x
        StepStatus::Skipped => ('\u{2591}', Color::DarkGray),     // same as pending
    }
}

/// Render the step list into the given area.
pub fn render_step_list(
    frame: &mut Frame,
    area: Rect,
    steps: &[StepItem],
    state: &StepListState,
    focused: bool,
    tick: u64,
) {
    let mut lines: Vec<Line> = Vec::new();

    for (i, step) in steps.iter().enumerate() {
        let is_selected = i == state.selected;
        let (indicator, indicator_color) = status_indicator(&step.status, tick);

        let selector = if is_selected && focused { "\u{203A} " } else { "  " };
        let selector_color = if focused { Color::White } else { Color::DarkGray };

        let name_color = match step.status {
            StepStatus::Success => Color::Green,
            StepStatus::Failed { .. } => Color::Red,
            StepStatus::Running => Color::Yellow,
            _ => Color::DarkGray,
        };

        let name_style = if is_selected && focused {
            Style::default().fg(name_color).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(name_color)
        };

        lines.push(Line::from(vec![
            Span::styled(selector, Style::default().fg(selector_color)),
            Span::styled(
                format!("{} ", indicator),
                Style::default().fg(indicator_color),
            ),
            Span::styled(step.name.clone(), name_style),
        ]));
    }

    // Scroll to keep the selected step visible
    let visible_height = area.height as usize;
    let scroll_offset = if state.selected >= visible_height {
        state.selected - visible_height + 1
    } else {
        0
    };

    let visible_lines: Vec<Line> = lines
        .into_iter()
        .skip(scroll_offset)
        .take(visible_height)
        .collect();

    let paragraph = Paragraph::new(visible_lines);
    frame.render_widget(paragraph, area);
}
