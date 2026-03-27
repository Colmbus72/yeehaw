use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

const BRAND_COLOR: Color = Color::Rgb(212, 160, 32);

#[derive(Debug, Clone)]
pub struct ListItem {
    pub id: String,
    pub label: String,
    pub status: Option<ItemStatus>,
    pub meta: Option<String>,
    pub actions: Vec<RowAction>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ItemStatus {
    Active,
    Inactive,
    Error,
}

#[derive(Debug, Clone)]
pub struct RowAction {
    pub key: String,
    pub label: String,
}

pub struct ListState {
    pub selected: usize,
    pub scroll_offset: usize,
}

impl ListState {
    pub fn new() -> Self {
        Self {
            selected: 0,
            scroll_offset: 0,
        }
    }

    pub fn select_next(&mut self, item_count: usize) {
        if item_count == 0 { return; }
        self.selected = (self.selected + 1).min(item_count - 1);
    }

    pub fn select_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn select_first(&mut self) {
        self.selected = 0;
    }

    pub fn select_last(&mut self, item_count: usize) {
        if item_count > 0 {
            self.selected = item_count - 1;
        }
    }

    /// Ensure the selected item is visible within the viewport
    fn ensure_visible(&mut self, max_visible: usize) {
        if self.selected >= self.scroll_offset + max_visible {
            self.scroll_offset = self.selected - max_visible + 1;
        }
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        }
    }
}

pub fn render_list(
    frame: &mut Frame,
    area: Rect,
    items: &[ListItem],
    state: &mut ListState,
    focused: bool,
    max_visible: Option<usize>,
) {
    if items.is_empty() {
        let text = Paragraph::new("No items")
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(text, area);
        return;
    }

    let max_vis = max_visible.unwrap_or(area.height as usize);
    let effective_max = max_vis.min(area.height as usize);
    state.ensure_visible(effective_max);

    let visible_items: Vec<_> = items
        .iter()
        .skip(state.scroll_offset)
        .take(effective_max)
        .collect();

    let can_scroll_up = state.scroll_offset > 0;
    let can_scroll_down = state.scroll_offset + effective_max < items.len();

    let mut y = area.y;

    // Scroll up indicator
    if can_scroll_up {
        let indicator = Paragraph::new(format!("▲ {} more", state.scroll_offset))
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center);
        if y < area.y + area.height {
            frame.render_widget(indicator, Rect { x: area.x, y, width: area.width, height: 1 });
            y += 1;
        }
    }

    for (vis_idx, item) in visible_items.iter().enumerate() {
        if y >= area.y + area.height {
            break;
        }

        let actual_idx = state.scroll_offset + vis_idx;
        let is_selected = actual_idx == state.selected && focused;

        let mut spans: Vec<Span> = Vec::new();

        // Selection indicator
        if is_selected {
            spans.push(Span::styled("› ", Style::default().fg(BRAND_COLOR)));
        } else {
            spans.push(Span::raw("  "));
        }

        // Label
        let label_style = if is_selected {
            Style::default().fg(BRAND_COLOR).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        spans.push(Span::styled(&item.label, label_style));

        // Status dot
        if let Some(ref status) = item.status {
            let color = match status {
                ItemStatus::Active => Color::Green,
                ItemStatus::Inactive => Color::DarkGray,
                ItemStatus::Error => Color::Red,
            };
            spans.push(Span::styled(" ●", Style::default().fg(color)));
        }

        // Meta
        if let Some(ref meta) = item.meta {
            spans.push(Span::styled(format!(" {}", meta), Style::default().fg(Color::DarkGray)));
        }

        // Actions (only on selected item)
        if is_selected && !item.actions.is_empty() {
            let actions_str: Vec<String> = item.actions.iter()
                .map(|a| format!("[{}] {}", a.key, a.label))
                .collect();
            spans.push(Span::styled(
                format!("  {}", actions_str.join("  ")),
                Style::default().fg(BRAND_COLOR),
            ));
        }

        let line = Paragraph::new(Line::from(spans));
        frame.render_widget(line, Rect { x: area.x, y, width: area.width, height: 1 });
        y += 1;
    }

    // Scroll down indicator
    if can_scroll_down && y < area.y + area.height {
        let remaining = items.len() - state.scroll_offset - effective_max;
        let indicator = Paragraph::new(format!("▼ {} more", remaining))
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center);
        frame.render_widget(indicator, Rect { x: area.x, y, width: area.width, height: 1 });
    }
}
