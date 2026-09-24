use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

const BRAND_COLOR: Color = Color::Rgb(212, 160, 32);

/// A [`RowStyle::Heading`] label.
///
/// The same red the session grid uses for its header note
/// (`session_grid::STALE_NOTE`) rather than the darker `BARN_RED` it borders
/// cells with: that one was picked to sit *behind* text and is barely legible
/// as text itself. Red at all because the only headings this list has are barn
/// names, and red already means "another machine" everywhere else in the TUI.
const HEADING_COLOR: Color = Color::Rgb(190, 90, 80);

/// A [`RowStyle::Muted`] label. The same grey `meta` is drawn in — the row is
/// there, it is just not here yet.
const MUTED_COLOR: Color = Color::DarkGray;

#[derive(Debug, Clone, Default)]
pub struct ListItem {
    pub id: String,
    pub label: String,
    pub status: Option<ItemStatus>,
    pub meta: Option<String>,
    pub actions: Vec<RowAction>,
    /// How the row is drawn, and whether the cursor may land on it at all.
    ///
    /// Defaults to [`RowStyle::Normal`], which is what every row in every view
    /// was before the dashboard's sessions panel grew barn headings — so a
    /// `..Default::default()` in any existing construction keeps exactly
    /// today's behaviour.
    pub style: RowStyle,
}

/// The three kinds of row a list can hold.
///
/// One field rather than a `selectable` flag *and* a `dim` flag, because the
/// three are mutually exclusive: a heading is never merely dimmed, and a dimmed
/// row is always still selectable. Two bools would spell a fourth state
/// (`selectable: false, dim: true`) that means nothing and that every reader
/// would then have to rule out.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum RowStyle {
    /// An ordinary row. The only kind that existed before barn headings.
    #[default]
    Normal,
    /// Selectable, drawn muted. The sessions panel uses it for a remote session
    /// that is *not* open on this machine — the row is real and actionable, it
    /// just is not here yet.
    Muted,
    /// A heading for the rows beneath it. Not selectable: there is nothing to
    /// act on, and a cursor parked on one is a cursor that `Enter` cannot
    /// answer.
    Heading,
}

impl ListItem {
    /// May the cursor land on this row?
    pub fn selectable(&self) -> bool {
        self.style != RowStyle::Heading
    }
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

    // --- selection over a list that holds headings --------------------------
    //
    // The `item_count` forms above cannot answer these: a count says how many
    // rows there are, never which of them the cursor may rest on. A list with
    // no headings answers every one of these identically to its counterpart,
    // so a view only needs these if it actually builds headings.

    /// Down one row, stepping over any heading in the way.
    ///
    /// Stays put when there is no selectable row below — which is not the same
    /// as clamping to `len() - 1` the way [`Self::select_next`] does, because
    /// the last row of the sessions panel can be a heading for a barn that has
    /// no sessions.
    pub fn select_next_selectable(&mut self, items: &[ListItem]) {
        match items
            .iter()
            .enumerate()
            .skip(self.selected.saturating_add(1))
            .find(|(_, item)| item.selectable())
        {
            Some((i, _)) => self.selected = i,
            None => self.settle_on_selectable(items),
        }
    }

    /// Up one row, stepping over any heading in the way.
    pub fn select_prev_selectable(&mut self, items: &[ListItem]) {
        match items[..self.selected.min(items.len())]
            .iter()
            .enumerate()
            .rfind(|(_, item)| item.selectable())
        {
            Some((i, _)) => self.selected = i,
            None => self.settle_on_selectable(items),
        }
    }

    /// The first row the cursor may rest on, which is not always row 0: a
    /// ranch with no local sessions opens the panel on a barn heading.
    pub fn select_first_selectable(&mut self, items: &[ListItem]) {
        if let Some(i) = items.iter().position(ListItem::selectable) {
            self.selected = i;
        }
    }

    /// The last row the cursor may rest on.
    pub fn select_last_selectable(&mut self, items: &[ListItem]) {
        if let Some(i) = items.iter().rposition(ListItem::selectable) {
            self.selected = i;
        }
    }

    /// Move the cursor off a heading it is parked on.
    ///
    /// The list is rebuilt from a live stream every frame, so a row can become
    /// a heading — or vanish — under a cursor that never moved. Forwards first,
    /// because the rows a heading introduces sit *below* it: settling backwards
    /// would jump the user to the previous barn.
    ///
    /// A list with nothing selectable in it leaves the cursor exactly where it
    /// is. There is no honest answer, and inventing one would put the cursor on
    /// a heading by a different route.
    pub fn settle_on_selectable(&mut self, items: &[ListItem]) {
        if items.get(self.selected).is_some_and(ListItem::selectable) {
            return;
        }
        if let Some((i, _)) = items
            .iter()
            .enumerate()
            .skip(self.selected)
            .find(|(_, item)| item.selectable())
        {
            self.selected = i;
            return;
        }
        if let Some((i, _)) = items[..self.selected.min(items.len())]
            .iter()
            .enumerate()
            .rfind(|(_, item)| item.selectable())
        {
            self.selected = i;
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

    // Before anything is measured or drawn. The list is rebuilt from scratch on
    // every frame — the sessions panel's from a live stream — so a row the
    // cursor was resting on can have become a heading since the last keypress,
    // and a selection arrow on a heading offers an `Enter` that answers
    // nothing. A list with no headings in it is untouched by this.
    state.settle_on_selectable(items);

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
        // `selectable()` and not just the index: `settle_on_selectable` has no
        // answer for a list that is nothing but headings, so the cursor can
        // still be sitting on one here.
        let is_selected = actual_idx == state.selected && focused && item.selectable();

        let mut spans: Vec<Span> = Vec::new();

        // Selection indicator
        if is_selected {
            spans.push(Span::styled("› ", Style::default().fg(BRAND_COLOR)));
        } else {
            spans.push(Span::raw("  "));
        }

        // Label. Selection wins over the row's own style — the cursor has to be
        // findable — and below it the three kinds are told apart by colour.
        let label_style = match item.style {
            _ if is_selected => Style::default().fg(BRAND_COLOR).add_modifier(Modifier::BOLD),
            RowStyle::Heading => Style::default().fg(HEADING_COLOR).add_modifier(Modifier::BOLD),
            RowStyle::Muted => Style::default().fg(MUTED_COLOR),
            RowStyle::Normal => Style::default(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn row(label: &str, style: RowStyle) -> ListItem {
        ListItem { id: label.into(), label: label.into(), style, ..Default::default() }
    }

    /// Every existing construction site says nothing about `style`, so the
    /// default has to be the row every view has always built.
    #[test]
    fn a_row_that_says_nothing_about_its_style_is_an_ordinary_selectable_row() {
        let item = ListItem::default();
        assert_eq!(item.style, RowStyle::Normal);
        assert!(item.selectable());
    }

    #[test]
    fn a_heading_is_the_only_kind_of_row_the_cursor_may_not_rest_on() {
        assert!(row("a", RowStyle::Normal).selectable());
        assert!(row("a", RowStyle::Muted).selectable(), "muted is dimmed, not disabled");
        assert!(!row("guided", RowStyle::Heading).selectable());
    }

    /// The whole reason headings need a flag: a cursor that can stop on one is
    /// a cursor `Enter` has no answer for.
    #[test]
    fn moving_down_steps_over_a_heading_rather_than_landing_on_it() {
        // [0] local   [1] HEADING   [2] remote   [3] remote
        let items = [
            row("local", RowStyle::Normal),
            row("guided", RowStyle::Heading),
            row("A", RowStyle::Muted),
            row("B", RowStyle::Muted),
        ];
        let mut state = ListState::new();

        state.select_next_selectable(&items);
        assert_eq!(state.selected, 2, "the cursor stopped on the barn heading");

        state.select_next_selectable(&items);
        assert_eq!(state.selected, 3);
    }

    #[test]
    fn moving_up_steps_over_a_heading_rather_than_landing_on_it() {
        let items = [
            row("local", RowStyle::Normal),
            row("guided", RowStyle::Heading),
            row("A", RowStyle::Muted),
        ];
        let mut state = ListState::new();
        state.selected = 2;

        state.select_prev_selectable(&items);
        assert_eq!(state.selected, 0, "the cursor stopped on the barn heading");
    }

    /// Consecutive headings happen the moment two barns are tunneled and one of
    /// them has no sessions at all.
    #[test]
    fn a_run_of_headings_is_stepped_over_in_one_move() {
        let items = [
            row("A", RowStyle::Muted),
            row("guided", RowStyle::Heading),
            row("smash-mac", RowStyle::Heading),
            row("B", RowStyle::Muted),
        ];
        let mut state = ListState::new();

        state.select_next_selectable(&items);
        assert_eq!(state.selected, 3);

        state.select_prev_selectable(&items);
        assert_eq!(state.selected, 0);
    }

    /// At the end of the list there is nowhere to go. The cursor must stay put
    /// rather than run onto the trailing heading.
    #[test]
    fn the_cursor_stays_where_it_is_when_there_is_no_further_selectable_row() {
        let items = [row("A", RowStyle::Normal), row("guided", RowStyle::Heading)];
        let mut state = ListState::new();

        state.select_next_selectable(&items);
        assert_eq!(state.selected, 0);
    }

    /// `g` and `G`. A ranch with no local sessions opens with a heading at row
    /// 0, and a barn with no sessions puts one at the end.
    #[test]
    fn first_and_last_are_the_first_and_last_selectable_rows() {
        let items = [
            row("guided", RowStyle::Heading),
            row("A", RowStyle::Muted),
            row("B", RowStyle::Muted),
            row("smash-mac", RowStyle::Heading),
        ];
        let mut state = ListState::new();

        state.select_last_selectable(&items);
        assert_eq!(state.selected, 2);

        state.select_first_selectable(&items);
        assert_eq!(state.selected, 1);
    }

    /// The list is rebuilt from a live stream every frame, so a row can turn
    /// into a heading under a cursor that never moved.
    #[test]
    fn a_cursor_left_on_a_heading_by_a_changing_list_is_moved_off_it() {
        let items = [
            row("A", RowStyle::Normal),
            row("guided", RowStyle::Heading),
            row("B", RowStyle::Muted),
        ];
        let mut state = ListState::new();
        state.selected = 1;

        state.settle_on_selectable(&items);
        assert_ne!(state.selected, 1, "the cursor was left on a heading");
        assert!(items[state.selected].selectable());
    }

    /// Nothing to settle onto. Anything but staying put would be inventing a
    /// selection out of an empty list.
    #[test]
    fn a_list_of_nothing_but_headings_leaves_the_cursor_alone() {
        let items = [row("guided", RowStyle::Heading)];
        let mut state = ListState::new();

        state.settle_on_selectable(&items);
        state.select_next_selectable(&items);
        state.select_first_selectable(&items);
        assert_eq!(state.selected, 0, "no panic, no invented selection");
    }

    fn draw(items: &[ListItem], state: &mut ListState, focused: bool) -> ratatui::buffer::Buffer {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(40, items.len() as u16))
                .expect("test terminal");
        terminal
            .draw(|f| {
                let area = f.area();
                render_list(f, area, items, state, focused, None);
            })
            .expect("draw");
        terminal.backend().buffer().clone()
    }

    fn fg_at(buf: &ratatui::buffer::Buffer, x: u16, y: u16) -> Color {
        buf.cell((x, y)).expect("a cell").fg
    }

    fn text_at(buf: &ratatui::buffer::Buffer, y: u16, width: u16) -> String {
        (0..width).map(|x| buf.cell((x, y)).expect("a cell").symbol()).collect()
    }

    /// The three kinds have to be told apart on screen or the flag is
    /// bookkeeping only. Labels start at column 2 — column 0-1 is the cursor
    /// gutter.
    #[test]
    fn the_three_kinds_of_row_are_drawn_differently() {
        let items = [
            row("open", RowStyle::Normal),
            row("guided", RowStyle::Heading),
            row("closed", RowStyle::Muted),
        ];
        let mut state = ListState::new();

        let buf = draw(&items, &mut state, false);

        assert_eq!(fg_at(&buf, 2, 0), Color::Reset, "an ordinary row is left alone");
        assert_eq!(fg_at(&buf, 2, 1), HEADING_COLOR, "a heading is not marked out");
        assert_eq!(fg_at(&buf, 2, 2), MUTED_COLOR, "a muted row is not greyed");
    }

    /// Defence in depth. Navigation already steps over headings, but the list
    /// is rebuilt every frame from a live stream and the cursor can be left on
    /// one between a rebuild and the next keypress. Drawing the selection
    /// arrow there would offer the user an `Enter` that answers nothing.
    #[test]
    fn the_selection_arrow_is_never_drawn_on_a_heading() {
        let items = [row("guided", RowStyle::Heading), row("A", RowStyle::Muted)];
        let mut state = ListState::new();
        state.selected = 0;

        let buf = draw(&items, &mut state, true);

        assert!(!text_at(&buf, 0, 40).starts_with('\u{203a}'), "the heading got the cursor");
        assert!(text_at(&buf, 1, 40).starts_with('\u{203a}'), "and nothing else got it either");
        assert_eq!(state.selected, 1, "render left the cursor parked on a heading");
    }
}
