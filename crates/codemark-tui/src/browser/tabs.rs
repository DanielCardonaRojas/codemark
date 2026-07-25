//! Tab selection component for switching between views.

use codemark_core::sort::SortMethod;
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Widget, Wrap},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Nerd Font glyph shown for a [`SortMethod`] beside the pane-number badge.
///
/// This is the TUI's presentation of the (presentation-agnostic) core sort
/// method: alphabetical methods use the sort-alpha glyphs, date methods the
/// sort-numeric glyphs, so the two axes read differently.
pub fn sort_method_icon(method: SortMethod) -> &'static str {
    match method {
        SortMethod::AlphabeticalAsc => "\u{f15d}",  // nf-fa-sort_alpha_asc
        SortMethod::AlphabeticalDesc => "\u{f15e}", // nf-fa-sort_alpha_desc
        SortMethod::DateNewest => "\u{f163}",       // nf-fa-sort_numeric_desc
        SortMethod::DateOldest => "\u{f162}",       // nf-fa-sort_numeric_asc
    }
}

/// Number of characters to extend the top border line for tab titles.
/// This must be kept in sync with the rendering in `TabbedPanel`.
pub const BORDER_EXTENSION: u16 = 2;

/// Nerd Font glyph (nf-fa-filter) shown beside a tab title when that tab's
/// list has an active filter narrowing its results.
pub const FILTER_ICON: &str = "\u{f0b0}";

/// Number of terminal columns reserved for [`FILTER_ICON`]. The glyph lives in
/// Unicode's Private Use Area, where `unicode_width` reports a width of 1, but
/// many Nerd Font terminals render it two columns wide. Reserving a fixed
/// 2-column slot (the glyph padded with a trailing space) makes the drawn width
/// match the recorded tab boundaries on both 1- and 2-column terminals, so tab
/// click detection stays aligned.
pub const FILTER_ICON_WIDTH: u16 = 2;

/// Columns the `[N]` pane-number badge reserves on the right of the top border,
/// measured from (and excluding) the left corner: a one-column separator from the
/// title, the badge itself, a one-column gap, and the right corner. A
/// left-aligned title may therefore occupy at most `area.width - 1 - reserved`
/// columns without colliding with the badge.
pub fn pane_number_badge_reserved_width(number: u8) -> u16 {
    let badge_width = format!("[{number}]").width() as u16;
    // separator(1) + badge + gap(1) + corner(1)
    badge_width + 3
}

/// Columns reserved on the top border for a sort-order glyph drawn just left of
/// the `[N]` badge: a blank on each side of the glyph so it doesn't touch the
/// border line or the badge. The trailing blank also absorbs the extra column
/// when a Nerd Font glyph renders two columns wide, keeping the drawn width
/// aligned (mirroring [`FILTER_ICON_WIDTH`]).
pub const SORT_ICON_WIDTH: u16 = 3;

/// Total columns reserved on the right of the top border for the pane-number
/// badge plus (optionally) a sort glyph to its left. Callers cap a left-aligned
/// title to `area.width - 1 - reserved` so it never runs into either.
pub fn top_border_reserved_width(number: u8, has_sort_icon: bool) -> u16 {
    pane_number_badge_reserved_width(number) + if has_sort_icon { SORT_ICON_WIDTH } else { 0 }
}

/// The absolute column range `[start, end)` of the sort-glyph slot on the top
/// border (the [`SORT_ICON_WIDTH`] cells left of the `[N]` badge, padding
/// included), or `None` when there isn't room for it. Mirrors the placement in
/// [`render_sort_icon`] so click hit-testing stays aligned with what's drawn.
pub fn sort_icon_hit_range(area: Rect, number: u8) -> Option<(u16, u16)> {
    let badge_width = format!("[{number}]").width() as u16;
    let badge_x = area.right().saturating_sub(badge_width + 2);
    if badge_x <= area.left() + SORT_ICON_WIDTH {
        return None;
    }
    Some((badge_x - SORT_ICON_WIDTH, badge_x))
}

/// Draw a sort-order glyph on the top border just left of the `[N]` badge,
/// flanked by a blank on each side so it doesn't butt against the border line or
/// the badge.
///
/// The badge's left edge is at `area.right() - badge_width - 2`; going left from
/// there the cells are: blank (right pad), glyph, blank (left pad) — the
/// [`SORT_ICON_WIDTH`] slot. `number` must match the badge drawn by
/// [`render_pane_number_badge`] so the two line up.
pub fn render_sort_icon(area: Rect, buf: &mut Buffer, number: u8, icon: &str, style: Style) {
    let badge_width = format!("[{number}]").width() as u16;
    // Left edge of the badge, mirroring `render_pane_number_badge`.
    let badge_x = area.right().saturating_sub(badge_width + 2);
    // Need room for the full slot (left pad + glyph + right pad) after the corner.
    if badge_x <= area.left() + SORT_ICON_WIDTH {
        return;
    }
    let y = area.top();
    // right pad (between glyph and badge), glyph, left pad (between glyph and border)
    if let Some(cell) = buf.cell_mut((badge_x - 1, y)) {
        cell.set_char(' ');
        cell.set_style(style);
    }
    if let Some(cell) = buf.cell_mut((badge_x - 2, y)) {
        cell.set_symbol(icon);
        cell.set_style(style);
    }
    if let Some(cell) = buf.cell_mut((badge_x - 3, y)) {
        cell.set_char(' ');
        cell.set_style(style);
    }
}

/// Draw a pane-number badge (e.g. `[3]`) on the top border of `area`, aligned to
/// the right edge just inside the rounded corner.
///
/// The number mirrors the keybinding that jumps focus to this pane (`1`–`5`), so
/// the badge doubles as a visual cue for that shortcut. Mirrors the placement of
/// the bottom-border "n of m" indicator, but on the top border.
///
/// Callers that draw a left-aligned title on the same border must reserve
/// [`pane_number_badge_reserved_width`] columns on the right so the title can't
/// run into the badge.
pub fn render_pane_number_badge(area: Rect, buf: &mut Buffer, number: u8, style: Style) {
    let badge = format!("[{number}]");
    let badge_width = badge.width() as u16;

    // Need room for the badge, a one-column border gap, and the corner glyph;
    // skip on tiny areas.
    if area.width <= badge_width + 2 {
        return;
    }

    // Right-aligned, leaving one border character between the badge and the
    // corner (╮) so the number isn't crammed against it.
    let x = area.right().saturating_sub(badge_width + 2);
    let y = area.top();
    for (i, c) in badge.chars().enumerate() {
        if let Some(cell) = buf.cell_mut((x + i as u16, y)) {
            cell.set_char(c);
            cell.set_style(style);
        }
    }
}

/// Truncate a (left-aligned) title `Line` to at most `max_width` display columns,
/// preserving each span's style. Trailing spans that don't fit are dropped, and a
/// span straddling the boundary is cut on a character boundary.
///
/// Used to keep a tab-title row from extending into the pane-number badge drawn
/// on the right of the same border.
pub fn truncate_line_to_width(line: Line<'static>, max_width: usize) -> Line<'static> {
    if line.width() <= max_width {
        return line;
    }

    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    for span in line.spans {
        if used >= max_width {
            break;
        }
        let mut text = String::new();
        for ch in span.content.chars() {
            let w = ch.width().unwrap_or(0);
            if used + w > max_width {
                break;
            }
            text.push(ch);
            used += w;
        }
        if !text.is_empty() {
            spans.push(Span::styled(text, span.style));
        }
    }

    Line::from(spans).alignment(Alignment::Left)
}

/// Content panel tabs (Bookmarks/Collections/Tours).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentTab {
    Bookmarks = 0,
    Collections = 1,
    Tours = 2,
}

impl ContentTab {
    /// Get the index of this tab.
    pub fn index(self) -> usize {
        self as usize
    }

    /// Get all tabs in order.
    pub fn all() -> &'static [ContentTab] {
        &[ContentTab::Bookmarks, ContentTab::Collections, ContentTab::Tours]
    }

    /// Get the tab label.
    pub fn label(self) -> &'static str {
        match self {
            ContentTab::Bookmarks => "Bookmarks",
            ContentTab::Collections => "Collections",
            ContentTab::Tours => "Tours",
        }
    }

    /// Try to convert an index to a ContentTab.
    pub fn from_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(ContentTab::Bookmarks),
            1 => Some(ContentTab::Collections),
            2 => Some(ContentTab::Tours),
            _ => None,
        }
    }
}

/// Tabs for the context panel (Context panel): Repos/Owners/Auth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextTab {
    /// Known repositories; selecting one switches the active database.
    Repos = 0,
    /// Repo owners (users/orgs); multi-select filters the Repos list.
    Owners = 1,
    /// Authenticated accounts (server + username), read-only.
    Auth = 2,
}

impl ContextTab {
    /// Get the index of this tab.
    pub fn index(self) -> usize {
        self as usize
    }

    /// Get all tabs in order.
    pub fn all() -> &'static [ContextTab] {
        &[ContextTab::Repos, ContextTab::Owners, ContextTab::Auth]
    }

    /// Get the tab label.
    pub fn label(self) -> &'static str {
        match self {
            ContextTab::Repos => "Repos",
            ContextTab::Owners => "Owners",
            ContextTab::Auth => "Auth",
        }
    }

    /// Try to convert an index to a ContextTab.
    pub fn from_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(ContextTab::Repos),
            1 => Some(ContextTab::Owners),
            2 => Some(ContextTab::Auth),
            _ => None,
        }
    }
}

/// Filters panel tabs (Tags/Branches).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FiltersTab {
    Tags = 0,
    Branches = 1,
}

impl FiltersTab {
    /// Get the index of this tab.
    pub fn index(self) -> usize {
        self as usize
    }

    /// Get all tabs in order.
    pub fn all() -> &'static [FiltersTab] {
        &[FiltersTab::Tags, FiltersTab::Branches]
    }

    /// Get the tab label.
    pub fn label(self) -> &'static str {
        match self {
            FiltersTab::Tags => "Tags",
            FiltersTab::Branches => "Branches",
        }
    }

    /// Try to convert an index to a FiltersTab.
    pub fn from_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(FiltersTab::Tags),
            1 => Some(FiltersTab::Branches),
            _ => None,
        }
    }
}

/// A single tab.
#[derive(Debug, Clone)]
pub struct Tab {
    /// The tab label
    label: String,
}

impl Tab {
    /// Create a new tab.
    pub fn new(label: impl Into<String>) -> Self {
        Self { label: label.into() }
    }

    /// Get the tab label.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Set the tab label.
    pub fn set_label(&mut self, label: impl Into<String>) {
        self.label = label.into();
    }

    /// Render this tab as a Line.
    fn render(&self, selected: bool, _focused: bool) -> Line<'_> {
        let base_style = if selected {
            Style::default()
                .fg(crate::theme::palette().accent)
                .add_modifier(ratatui::style::Modifier::BOLD)
        } else {
            Style::default().fg(crate::theme::palette().dim)
        };

        let spans = vec![
            Span::styled(" ", base_style),
            Span::styled(&self.label, base_style),
            Span::styled(" ", base_style),
        ];

        Line::from(spans)
    }
}

/// Tab selection component.
#[derive(Debug, Clone)]
pub struct TabSelection {
    /// Available tabs
    tabs: Vec<Tab>,
    /// Currently selected tab index
    selected: usize,
    /// Number of leading tabs that are currently visible/selectable. `None`
    /// means every tab is visible. Used to hide trailing tabs (e.g. the
    /// Branches tab in Filters panel when Bookmarks — which have no branch — are the
    /// active Content panel tab). Hidden tabs are neither rendered nor selectable.
    visible_count: Option<usize>,
    /// Whether the tabs are focused
    focused: bool,
    /// Last rendered tab positions: (x_start, x_end) for each tab
    last_tab_positions: std::cell::RefCell<Vec<(u16, u16)>>,
}

impl TabSelection {
    /// Create a new tab selection.
    pub fn new(tabs: Vec<Tab>) -> Self {
        Self {
            tabs,
            selected: 0,
            visible_count: None,
            focused: false,
            last_tab_positions: std::cell::RefCell::new(Vec::new()),
        }
    }

    /// Number of currently visible (selectable) leading tabs.
    fn visible_len(&self) -> usize {
        self.visible_count.map_or(self.tabs.len(), |n| n.min(self.tabs.len()))
    }

    /// Limit selection and rendering to the first `count` tabs, hiding any
    /// trailing tabs. If the current selection falls outside the visible
    /// range it is clamped back into range.
    pub fn set_visible_count(&mut self, count: usize) {
        self.visible_count = Some(count);
        let visible = self.visible_len();
        if visible > 0 && self.selected >= visible {
            self.selected = visible - 1;
        }
    }

    /// Get the selected tab index.
    pub fn selected_index(&self) -> usize {
        self.selected
    }

    /// Set the selected tab index. Indices outside the visible range are
    /// ignored, so a hidden trailing tab can never become selected.
    pub fn set_selected(&mut self, index: usize) {
        if index < self.visible_len() {
            self.selected = index;
        }
    }

    /// Select the next tab, wrapping within the visible range.
    pub fn next(&mut self) {
        let visible = self.visible_len();
        if visible > 0 {
            self.selected = (self.selected + 1) % visible;
        }
    }

    /// Select the previous tab, wrapping within the visible range.
    pub fn previous(&mut self) {
        let visible = self.visible_len();
        if visible > 0 {
            self.selected = if self.selected == 0 { visible - 1 } else { self.selected - 1 };
        }
    }

    /// Get the number of tabs.
    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    /// Check if there are no tabs.
    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    /// Set the label of a tab by index.
    pub fn set_tab_label(&mut self, index: usize, label: impl Into<String>) {
        if let Some(tab) = self.tabs.get_mut(index) {
            tab.set_label(label);
        }
    }

    /// Render the tabs.
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        let mut lines = Vec::new();

        let visible = self.visible_len();
        for (i, tab) in self.tabs.iter().take(visible).enumerate() {
            let selected = i == self.selected;
            lines.push(tab.render(selected, self.focused));
        }

        // Join tabs with spacing
        let mut spans = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw(" "));
            }
            spans.extend(line.spans.clone());
        }

        let paragraph = ratatui::widgets::Paragraph::new(Line::from(spans))
            .wrap(Wrap { trim: false })
            .alignment(Alignment::Left);

        paragraph.render(area, buf);
    }

    /// Render tabs as a Title for use with Block borders (inline with border).
    pub fn render_as_titles(&self, _focused: bool) -> Line<'_> {
        self.render_as_titles_with_counts(&[])
    }

    /// Render tabs as a Title, appending a `(n)` count to a tab's label when the
    /// corresponding entry in `counts` is `Some(n)`. A tab's label is shown
    /// unchanged when `counts` has no entry for it (or it is `None`).
    ///
    /// Tab styling depends only on which tab is selected (`self.selected`), not on
    /// panel focus, so no `focused` argument is taken.
    pub fn render_as_titles_with_counts(&self, counts: &[Option<usize>]) -> Line<'static> {
        self.render_as_titles_with_counts_and_filters(counts, &[])
    }

    /// Like [`Self::render_as_titles_with_counts`], but also appends a filter
    /// glyph ([`FILTER_ICON`]) to the right of a tab's title when the matching
    /// entry in `filtered` is `true`, indicating that tab's list is currently
    /// showing filtered results.
    pub fn render_as_titles_with_counts_and_filters(
        &self,
        counts: &[Option<usize>],
        filtered: &[bool],
    ) -> Line<'static> {
        let mut spans = vec![Span::raw(" ".repeat(BORDER_EXTENSION as usize))]; // Offset for border extension
        let mut positions = Vec::new();
        let mut current_x = BORDER_EXTENSION;

        let visible = self.visible_len();
        for (i, tab) in self.tabs.iter().take(visible).enumerate() {
            let selected = i == self.selected;
            let style = if selected {
                Style::default()
                    .fg(crate::theme::palette().accent)
                    .add_modifier(ratatui::style::Modifier::BOLD)
            } else {
                Style::default().fg(crate::theme::palette().dim)
            };

            if i > 0 {
                spans.push(Span::styled(" ", Style::default()));
                current_x += 1;
            }

            let label = match counts.get(i).copied().flatten() {
                Some(count) => format!("{} ({count})", tab.label()),
                None => tab.label().to_string(),
            };

            let x_start = current_x;
            current_x += label.width() as u16;
            spans.push(Span::styled(label, style));

            // A filter glyph sits to the right of the title when this tab's
            // list has an active filter: a leading space separates it from the
            // title, then the glyph occupies a fixed `FILTER_ICON_WIDTH` slot.
            // The glyph is padded with a trailing space so it spans exactly that
            // slot whether the terminal draws it one or two columns wide, which
            // keeps the recorded click boundaries (`current_x`) aligned with the
            // rendered text.
            if filtered.get(i).copied().unwrap_or(false) {
                spans.push(Span::styled(" ", style));
                current_x += 1;
                spans.push(Span::styled(format!("{FILTER_ICON} "), style));
                current_x += FILTER_ICON_WIDTH;
            }

            spans.push(Span::styled(" ", style));
            current_x += 1;

            positions.push((x_start, current_x));
        }

        // Store positions for click detection
        *self.last_tab_positions.borrow_mut() = positions;

        Line::from(spans).alignment(Alignment::Left)
    }

    /// Set focus state.
    pub fn set_focus(&mut self, focused: bool) {
        self.focused = focused;
    }

    /// Handle a mouse click at the given x position (relative to the panel's left edge).
    /// Returns true if a tab was clicked.
    pub fn handle_click(&mut self, x: u16, _y: u16) -> bool {
        let positions = self.last_tab_positions.borrow();
        for (i, (x_start, x_end)) in positions.iter().enumerate() {
            if x >= *x_start && x < *x_end {
                self.selected = i;
                return true;
            }
        }
        false
    }
}

impl Default for TabSelection {
    fn default() -> Self {
        Self::new(vec![Tab::new("Tab 1"), Tab::new("Tab 2")])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_each_sort_method_has_a_distinct_icon() {
        let icons: Vec<&str> = SortMethod::all().iter().copied().map(sort_method_icon).collect();
        for (i, a) in icons.iter().enumerate() {
            for b in &icons[i + 1..] {
                assert_ne!(a, b, "sort icons must be unique");
            }
        }
    }

    #[test]
    fn test_truncate_line_to_width_caps_and_preserves_shorter() {
        let line =
            Line::from(vec![Span::raw("Bookmarks"), Span::raw(" "), Span::raw("Collections")]);
        assert_eq!(line.width(), 21);

        // A width >= the line's width returns it unchanged.
        assert_eq!(truncate_line_to_width(line.clone(), 21).width(), 21);
        assert_eq!(truncate_line_to_width(line.clone(), 50).width(), 21);

        // A smaller width truncates on a char boundary, spanning span edges.
        let cut = truncate_line_to_width(line, 12);
        assert_eq!(cut.width(), 12);
        assert_eq!(cut.to_string(), "Bookmarks Co");
    }

    #[test]
    fn test_pane_number_badge_reserves_room_for_title() {
        // A single-digit badge renders as "[N]" (3 cols) and reserves a
        // separator + gap + corner around it (3 + 3 = 6 cols on the right).
        assert_eq!(pane_number_badge_reserved_width(3), 6);

        // In a 40-wide pane the title (which starts after the left corner) is
        // capped so a border cell always separates it from the badge.
        let area_width = 40usize;
        let max_title = area_width - pane_number_badge_reserved_width(3) as usize - 1;
        assert_eq!(max_title, 33);
    }

    #[test]
    fn test_context_tab_index_roundtrip() {
        for (i, tab) in ContextTab::all().iter().enumerate() {
            assert_eq!(tab.index(), i);
            assert_eq!(ContextTab::from_index(i), Some(*tab));
        }
        assert_eq!(ContextTab::from_index(3), None);
        assert_eq!(ContextTab::Repos.label(), "Repos");
        assert_eq!(ContextTab::Owners.label(), "Owners");
        assert_eq!(ContextTab::Auth.label(), "Auth");
    }

    #[test]
    fn test_tab_creation() {
        let tab = Tab::new("Test");
        assert_eq!(tab.label(), "Test");
    }

    #[test]
    fn test_tab_selection() {
        let mut tabs = TabSelection::new(vec![Tab::new("Local"), Tab::new("Remote")]);

        assert_eq!(tabs.selected_index(), 0);

        tabs.next();
        assert_eq!(tabs.selected_index(), 1);

        tabs.next();
        assert_eq!(tabs.selected_index(), 0); // Wrapped

        tabs.previous();
        assert_eq!(tabs.selected_index(), 1); // Wrapped
    }

    #[test]
    fn test_tab_selection_empty() {
        let tabs = TabSelection::new(vec![]);
        assert!(tabs.is_empty());
        assert_eq!(tabs.len(), 0);
    }

    #[test]
    fn test_tab_click_handling() {
        let mut tabs =
            TabSelection::new(vec![Tab::new("Tab1"), Tab::new("Tab2"), Tab::new("Tab3")]);

        // First render to establish positions
        tabs.render_as_titles(false);

        // Click positions based on BORDER_EXTENSION (2) + space + tab name + space
        // Tab1: starts at 2, ends at 2+1+4+1 = 8 (2 spaces + "Tab1" + 2 spaces, but no separator before first)
        // Actually: BORDER_EXTENSION(2) + " Tab1 " = 2 + 1 + 4 + 1 = 8
        // Tab2: starts at 8 + 1 (separator), ends at 8+1+1+4+1 = 15
        // Tab3: starts at 15 + 1 (separator), ends at 15+1+1+4+1 = 22

        // Click on first tab (position 3 is within " Tab1 " which spans 2-7)
        assert!(tabs.handle_click(3, 0));
        assert_eq!(tabs.selected_index(), 0);

        // Click on second tab (position 9 is within separator + " Tab2 " which spans 8-14)
        assert!(tabs.handle_click(9, 0));
        assert_eq!(tabs.selected_index(), 1);

        // Click on third tab
        assert!(tabs.handle_click(16, 0));
        assert_eq!(tabs.selected_index(), 2);

        // Click outside tabs
        assert!(!tabs.handle_click(100, 0));
        assert_eq!(tabs.selected_index(), 2); // Should remain unchanged
    }

    #[test]
    fn test_render_titles_with_counts_appends_suffix() {
        let tabs = TabSelection::new(vec![Tab::new("Tags"), Tab::new("Branches")]);

        // Two selected tags, no selected branches -> "Tags (2)" / "Branches".
        tabs.render_as_titles_with_counts(&[Some(2), None]);
        let positions = tabs.last_tab_positions.borrow();
        // "Tags (2)" is 8 cols wide (+1 trailing space), after the BORDER_EXTENSION offset.
        assert_eq!(positions[0], (BORDER_EXTENSION, BORDER_EXTENSION + 8 + 1));
        // Separator (1) before the next tab; "Branches" is 8 cols (+1 trailing space).
        let branches_start = BORDER_EXTENSION + 8 + 1 + 1;
        assert_eq!(positions[1], (branches_start, branches_start + 8 + 1));
    }

    #[test]
    fn test_render_titles_with_filter_icon_widens_tab() {
        let tabs = TabSelection::new(vec![Tab::new("Tags"), Tab::new("Branches")]);

        // Only the first tab is filtered: its slot widens by a leading space
        // plus the fixed-width glyph slot, shifting the second tab right by the
        // same amount.
        tabs.render_as_titles_with_counts_and_filters(&[None, None], &[true, false]);
        let positions = tabs.last_tab_positions.borrow();
        // "Tags" (4) + leading space (1) + glyph slot (FILTER_ICON_WIDTH) +
        // trailing space (1), after the offset.
        let filtered_end = BORDER_EXTENSION + 4 + 1 + FILTER_ICON_WIDTH + 1;
        assert_eq!(positions[0], (BORDER_EXTENSION, filtered_end));
        // Separator (1) before "Branches" (8) + trailing space (1); no icon.
        let branches_start = filtered_end + 1;
        assert_eq!(positions[1], (branches_start, branches_start + 8 + 1));
    }

    #[test]
    fn test_visible_count_hides_trailing_tabs() {
        let mut tabs = TabSelection::new(vec![Tab::new("Tags"), Tab::new("Branches")]);

        // Hiding the trailing tab leaves navigation pinned to the first tab.
        tabs.set_visible_count(1);
        tabs.next();
        assert_eq!(tabs.selected_index(), 0);
        tabs.previous();
        assert_eq!(tabs.selected_index(), 0);

        // Only the visible tab is rendered, so only it records a click position.
        tabs.render_as_titles_with_counts(&[None, None]);
        assert_eq!(tabs.last_tab_positions.borrow().len(), 1);

        // Restoring full visibility re-enables the trailing tab.
        tabs.set_visible_count(2);
        tabs.next();
        assert_eq!(tabs.selected_index(), 1);
    }

    #[test]
    fn test_set_visible_count_clamps_selection() {
        let mut tabs = TabSelection::new(vec![Tab::new("Tags"), Tab::new("Branches")]);
        tabs.set_selected(1);
        assert_eq!(tabs.selected_index(), 1);

        // Hiding the selected trailing tab clamps the selection back into range.
        tabs.set_visible_count(1);
        assert_eq!(tabs.selected_index(), 0);

        // While hidden, the trailing tab can't be re-selected programmatically.
        tabs.set_selected(1);
        assert_eq!(tabs.selected_index(), 0);
    }

    #[test]
    fn test_set_tab_label() {
        let mut tabs = TabSelection::new(vec![Tab::new("Initial"), Tab::new("Tab2")]);

        tabs.set_tab_label(0, "Updated");
        assert_eq!(tabs.tabs[0].label(), "Updated");
        assert_eq!(tabs.tabs[1].label(), "Tab2");

        // Out of bounds should be safe (no panic)
        tabs.set_tab_label(10, "Invalid");
    }
}
