//! Scrollable panel component for displaying lists of items.
//!
//! Panels are the primary container component for displaying scrollable
//! content with headers and borders.

use ratatui::{
    buffer::Buffer,
    layout::{Margin, Rect},
    style::{Modifier, Style},
    widgets::{
        Block, BorderType, List, ListItem, ListState, Scrollbar, ScrollbarOrientation,
        ScrollbarState, StatefulWidget,
    },
};

use super::{Component, SizeConstraints};
use crate::event::Event;
use codemark_core::sort::sort_by;
// Re-exported from `crate::component` (see `component/mod.rs`); the ordering
// logic itself lives in `codemark_core::sort` so the CLI can share it.
use codemark_core::sort::SortMethod;

use std::cell::{Cell, RefCell};

mod health;
mod item;
mod path;

pub use health::HealthStatus;
pub use item::{PanelItem, SyncDirection};

/// A scrollable panel component.
///
/// Panels display a list of items with a title and optional border.
/// They support scrolling, selection, and custom styling.
pub struct Panel {
    /// The title displayed at the top of the panel
    title: String,
    /// The items to display in the panel (possibly filtered)
    items: Vec<PanelItem>,
    /// All items in the panel before filtering
    all_items: Vec<PanelItem>,
    /// Current filter query
    filter_query: String,
    /// Whether this panel is displaying externally-supplied search results
    /// (e.g. FTS/semantic search), which counts as a filtered view even though
    /// no local `filter_query` is set. Reset by [`Self::set_items`].
    search_active: bool,
    /// The state of the list (handles selection and scrolling)
    list_state: RefCell<ListState>,
    /// Whether the panel has a border
    bordered: bool,
    /// Whether the panel is focused
    focused: bool,
    /// Custom style for the panel border when focused
    focus_style: Style,
    /// Custom style for the panel border when not focused
    normal_style: Style,
    /// Whether to show a scrollbar
    show_scrollbar: bool,
    /// Whether multiple items can be active at once
    multi_select: bool,
    /// The type of border to render
    border_type: BorderType,
    /// Last rendered area for mouse handling
    last_area: Cell<Rect>,
    /// Track the last selected index to detect changes
    last_selected_index: Cell<Option<usize>>,
    /// Set when a scroll-into-view was requested but the panel's area (and thus
    /// height) was not yet known — e.g. a selection applied at construction
    /// time, before the first render. On the next render the *current* selection
    /// is revealed; the flag deliberately stores no index so that item rebuilds
    /// (`set_items`, `apply_filter`, `clear`) which move or clamp the selection
    /// can't leave a stale target pointing past the new list.
    pending_scroll: Cell<bool>,
    /// Ordering applied to `all_items`. `None` leaves items in insertion order
    /// (the default for panels that don't expose sorting); `Some` re-orders on
    /// every rebuild so the chosen order survives refreshes.
    sort: Option<SortMethod>,
}

impl Panel {
    /// Create a new panel with the given title.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            items: Vec::new(),
            all_items: Vec::new(),
            filter_query: String::new(),
            search_active: false,
            list_state: RefCell::new(ListState::default()),
            bordered: true,
            focused: false,
            focus_style: Style::default().fg(crate::theme::palette().accent),
            normal_style: Style::default().fg(crate::theme::palette().dim),
            show_scrollbar: true,
            multi_select: false,
            border_type: BorderType::Rounded,
            last_area: Cell::new(Rect::default()),
            last_selected_index: Cell::new(None),
            pending_scroll: Cell::new(false),
            sort: None,
        }
    }

    /// Enable sorting on this panel with the given initial [`SortMethod`]. The
    /// order is re-applied whenever items are (re)built, so it persists across
    /// panel refreshes. Panels created without this stay in insertion order.
    pub fn sort(mut self, sort: SortMethod) -> Self {
        self.sort = Some(sort);
        self
    }

    /// The panel's current sort method, if sorting is enabled.
    pub fn sort_method(&self) -> Option<SortMethod> {
        self.sort
    }

    /// Advance to the next [`SortMethod`] in the cycle and re-order the list,
    /// keeping the current selection on the same item. If sorting was not yet
    /// enabled the cycle starts at [`SortMethod::AlphabeticalAsc`]. Returns the
    /// newly active method.
    pub fn cycle_sort(&mut self) -> SortMethod {
        let next = self.sort.map_or(SortMethod::AlphabeticalAsc, SortMethod::next);
        self.set_sort(next);
        next
    }

    /// Set the sort method, re-order `all_items`, and re-apply the filter while
    /// preserving the current selection by identity (user data, falling back to
    /// text).
    pub fn set_sort(&mut self, sort: SortMethod) {
        // Remember what's selected so the cursor follows the item across the
        // reorder rather than sticking to a now-different row index.
        let selected_key = self.selected().map(|i| (i.user_data.clone(), i.text().to_string()));

        self.sort = Some(sort);
        self.apply_sort();
        self.apply_filter();

        if let Some((user_data, text)) = selected_key {
            let idx = self.items.iter().position(|i| match &user_data {
                Some(ud) => i.user_data.as_ref() == Some(ud),
                None => i.text() == text,
            });
            if let Some(idx) = idx {
                self.list_state.borrow_mut().select(Some(idx));
                self.scroll_to_view(idx);
            }
        }
    }

    /// Re-order `all_items` in place according to the active sort method,
    /// delegating to the shared [`codemark_core::sort`] logic. A no-op when
    /// sorting is disabled.
    fn apply_sort(&mut self) {
        if let Some(sort) = self.sort {
            sort_by(&mut self.all_items, sort);
        }
    }

    /// Set whether the panel supports multi-selection.
    pub fn multi_select(mut self, multi_select: bool) -> Self {
        self.multi_select = multi_select;
        self
    }

    /// Whether the panel supports multi-selection.
    pub fn is_multi_select(&self) -> bool {
        self.multi_select
    }

    /// Get the last rendered area.
    pub fn last_area(&self) -> Rect {
        self.last_area.get()
    }

    /// Add an item to the panel.
    pub fn add_item(mut self, item: PanelItem) -> Self {
        self.all_items.push(item);
        self.apply_sort();
        self.apply_filter();
        if self.selected_index().is_none() && !self.items.is_empty() {
            self.set_selected(0);
        }
        self
    }

    /// Add multiple items to the panel.
    pub fn items(mut self, items: impl IntoIterator<Item = PanelItem>) -> Self {
        self.all_items.extend(items);
        self.apply_sort();
        self.apply_filter();
        if self.selected_index().is_none() && !self.items.is_empty() {
            self.set_selected(0);
        }
        self
    }

    /// Whether this panel's list is currently narrowed — either by a non-empty
    /// local filter query or by externally-supplied search results.
    pub fn is_filtered(&self) -> bool {
        !self.filter_query.is_empty() || self.search_active
    }

    /// Mark (or unmark) this panel as displaying externally-supplied search
    /// results, so its tab shows a filter glyph even with no local filter
    /// query. Reset by [`Self::set_items`].
    pub fn set_search_active(&mut self, active: bool) {
        self.search_active = active;
    }

    /// Whether this panel is currently displaying externally-supplied search
    /// results (as opposed to a local filter query). See [`Self::set_search_active`].
    pub fn is_search_active(&self) -> bool {
        self.search_active
    }

    /// Set the filter query and apply it.
    pub fn set_filter(&mut self, query: &str) {
        if self.filter_query != query {
            self.filter_query = query.to_string();
            self.apply_filter();
        }
    }

    /// Apply the current filter to all_items.
    fn apply_filter(&mut self) {
        let old_items_len = self.items.len();
        if self.filter_query.is_empty() {
            self.items = self.all_items.clone();
        } else {
            let query = self.filter_query.to_lowercase();
            self.items =
                self.all_items.iter().filter(|item| item.matches_query(&query)).cloned().collect();
        }

        let mut state = self.list_state.borrow_mut();
        // Adjust selection if it's now out of bounds
        if let Some(sel) = state.selected() {
            if sel >= self.items.len() {
                state.select(if self.items.is_empty() { None } else { Some(0) });
            }
        } else if !self.items.is_empty() {
            state.select(Some(0));
        }

        // Only reset scroll offset if the number of items changed,
        // which usually means the filter results are different.
        if self.items.len() != old_items_len {
            let selected = state.selected();
            *state = ListState::default();
            state.select(selected);
        }
    }

    /// Set whether the panel has a border.
    pub fn bordered(mut self, bordered: bool) -> Self {
        self.bordered = bordered;
        self
    }

    /// Set the focus style.
    pub fn focus_style(mut self, style: Style) -> Self {
        self.focus_style = style;
        self
    }

    /// Set the normal (unfocused) style.
    pub fn normal_style(mut self, style: Style) -> Self {
        self.normal_style = style;
        self
    }

    /// Set whether to show the scrollbar.
    pub fn show_scrollbar(mut self, show: bool) -> Self {
        self.show_scrollbar = show;
        self
    }

    /// Set the border type.
    pub fn border_type(mut self, border_type: BorderType) -> Self {
        self.border_type = border_type;
        self
    }

    /// Get all items (unfiltered) in the panel.
    pub fn all_items(&self) -> &[PanelItem] {
        &self.all_items
    }

    /// Get the number of items in the panel.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Check if the panel is empty.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Get the currently selected item.
    pub fn selected(&self) -> Option<&PanelItem> {
        self.list_state.borrow().selected().and_then(|idx| self.items.get(idx))
    }

    /// Get the index of the currently selected item.
    pub fn selected_index(&self) -> Option<usize> {
        self.list_state.borrow().selected()
    }

    /// Scroll to ensure the given index is visible.
    fn scroll_to_view(&self, idx: usize) {
        let area = self.last_area.get();
        let height = area.height.saturating_sub(if self.bordered { 2 } else { 0 }) as usize;
        if height == 0 {
            // The area is not known yet (e.g. selection set before the first
            // render). Flag a deferred reveal; render re-derives the current
            // selection then, so the index used here can't go stale.
            tracing::debug!(
                target: "codemark::ui",
                idx,
                "panel scroll deferred until first render (area unknown)"
            );
            self.pending_scroll.set(true);
            return;
        }

        let mut state = self.list_state.borrow_mut();
        let offset = state.offset();

        if idx >= offset + height {
            *state.offset_mut() = idx.saturating_sub(height) + 1;
        } else if idx < offset {
            *state.offset_mut() = idx;
        }
    }

    /// Set the selected item by index.
    pub fn set_selected(&mut self, idx: usize) {
        if idx < self.items.len() {
            self.list_state.borrow_mut().select(Some(idx));
            self.scroll_to_view(idx);
        }
    }

    /// Get all currently active items in the entire list (regardless of filtering).
    /// Returns user_data if available, otherwise fallback to text.
    pub fn active_items(&self) -> Vec<String> {
        self.all_items
            .iter()
            .filter(|i| i.is_active())
            .map(|i| i.user_data.clone().unwrap_or_else(|| i.text().to_string()))
            .collect()
    }

    /// Count the currently active items in the entire list (regardless of filtering).
    /// Avoids the allocation `active_items()` performs when only the count is needed.
    pub fn active_item_count(&self) -> usize {
        self.all_items.iter().filter(|i| i.is_active()).count()
    }

    /// Select the next item.
    /// Returns true if selection changed.
    pub fn select_next(&mut self) -> bool {
        if self.items.is_empty() {
            return false;
        }
        let next = {
            let state = self.list_state.borrow();
            state.selected().map_or(0, |i| i.saturating_add(1) % self.items.len())
        };
        let old_index = self.list_state.borrow().selected();
        self.list_state.borrow_mut().select(Some(next));
        self.scroll_to_view(next);
        old_index != Some(next)
    }

    /// Select the previous item.
    /// Returns true if selection changed.
    pub fn select_previous(&mut self) -> bool {
        if self.items.is_empty() {
            return false;
        }
        let prev = {
            let state = self.list_state.borrow();
            state
                .selected()
                .map_or(self.items.len() - 1, |i| if i == 0 { self.items.len() - 1 } else { i - 1 })
        };
        let old_index = self.list_state.borrow().selected();
        self.list_state.borrow_mut().select(Some(prev));
        self.scroll_to_view(prev);
        old_index != Some(prev)
    }

    /// Set the currently selected item as active and deactivate all others (or toggle in multi-select mode).
    pub fn activate_selected(&mut self) {
        if let Some(idx) = self.selected_index()
            && let Some(item) = self.items.get(idx)
        {
            let key_user_data = item.user_data.clone();
            let key_text = item.text().to_string();

            // Helper to match items by stable identifier (user_data) or fallback to text
            let matches = |i: &PanelItem| -> bool {
                if let Some(ref key) = key_user_data {
                    i.user_data.as_ref() == Some(key)
                } else {
                    i.text() == key_text
                }
            };

            if self.multi_select {
                // Toggle in all_items
                if let Some(item) = self.all_items.iter_mut().find(|i| matches(i)) {
                    item.set_active(!item.is_active());
                }
            } else {
                // Determine if the target item is already active
                let was_active = self
                    .all_items
                    .iter()
                    .find(|i| matches(i))
                    .map(|i| i.is_active())
                    .unwrap_or(false);

                // Deactivate all
                for item in &mut self.all_items {
                    item.set_active(false);
                }

                // If it wasn't active before, activate it now.
                // If it was active, it remains inactive (toggled off).
                if !was_active && let Some(item) = self.all_items.iter_mut().find(|i| matches(i)) {
                    item.set_active(true);
                }
            }

            // Sync items with all_items (preserving current filter and list state)
            let query = self.filter_query.to_lowercase();
            if query.is_empty() {
                self.items = self.all_items.clone();
            } else {
                self.items = self
                    .all_items
                    .iter()
                    .filter(|item| item.matches_query(&query))
                    .cloned()
                    .collect();
            }
        }
    }

    /// Activate an item by its `user_data` value without changing cursor selection.
    ///
    /// Used to restore active state after rebuilding items with `set_items`.
    /// Follows the same pattern as `activate_selected`: modifies `all_items`
    /// then re-derives `items` via `apply_filter`.
    pub fn activate_by_user_data(&mut self, data: &str) {
        for item in &mut self.all_items {
            if item.user_data.as_deref() == Some(data) {
                item.set_active(true);
            }
        }
        self.apply_filter();
    }

    /// Move the cursor selection to the (currently visible) item whose
    /// `user_data` matches `data`. Unlike [`set_items`], which restores selection
    /// by text, this targets a stable identifier — used to keep a list selection
    /// aligned with externally-restored state after a rebuild. Returns true if a
    /// matching item was found and selected.
    pub fn select_by_user_data(&mut self, data: &str) -> bool {
        if let Some(idx) = self.items.iter().position(|i| i.user_data.as_deref() == Some(data)) {
            self.list_state.borrow_mut().select(Some(idx));
            self.scroll_to_view(idx);
            true
        } else {
            false
        }
    }

    /// Clear all items from the panel.
    pub fn clear(&mut self) {
        self.items.clear();
        self.all_items.clear();
        self.list_state.borrow_mut().select(None);
        self.filter_query.clear();
        self.search_active = false;
    }

    /// Update the secondary text of an item identified by its user_data value.
    /// Re-applies the filter so the change is visible immediately.
    pub fn update_item_secondary_text(&mut self, user_data: &str, text: &str) {
        let mut changed = false;
        for item in &mut self.all_items {
            if item.user_data.as_deref() == Some(user_data) {
                item.set_secondary_text(text);
                changed = true;
                break;
            }
        }
        if changed {
            // Also update the filtered items list directly to avoid resetting selection
            for item in &mut self.items {
                if item.user_data.as_deref() == Some(user_data) {
                    item.set_secondary_text(text);
                    break;
                }
            }
        }
    }

    /// Update the spinner text of an item identified by its user_data value.
    /// The spinner is rendered at the very end of the item line.
    pub fn update_item_spinner(&mut self, user_data: &str, text: Option<&str>) {
        let mut changed = false;
        for item in &mut self.all_items {
            if item.user_data.as_deref() == Some(user_data) {
                item.set_spinner_text(text.map(|t| t.to_string()));
                changed = true;
                break;
            }
        }
        if changed {
            for item in &mut self.items {
                if item.user_data.as_deref() == Some(user_data) {
                    item.set_spinner_text(text.map(|t| t.to_string()));
                    break;
                }
            }
        }
    }

    /// Update the health status of an item identified by its user_data value.
    /// Same pattern as `update_item_spinner()`: mutates both `all_items` and `items`.
    pub fn update_item_health(&mut self, user_data: &str, health: HealthStatus) {
        let mut changed = false;
        for item in &mut self.all_items {
            if item.user_data.as_deref() == Some(user_data) {
                item.set_health(Some(health));
                changed = true;
                break;
            }
        }
        if changed {
            for item in &mut self.items {
                if item.user_data.as_deref() == Some(user_data) {
                    item.set_health(Some(health));
                    break;
                }
            }
        }
    }

    /// Update the items in the panel, preserving selection if possible.
    ///
    /// Re-applies the panel's sort method, so this is for browse/refresh
    /// rebuilds where the configured order should win. For pre-ranked search
    /// results use [`set_search_items`](Self::set_search_items), which skips
    /// the sort.
    pub fn set_items(&mut self, items: Vec<PanelItem>) {
        let selected_text = self.capture_selected_text();
        self.all_items = items;
        // Rebuilding the list drops any previous search-results marking.
        self.search_active = false;
        self.apply_sort();
        self.rebuild_view(selected_text);
    }

    /// Replace the panel's items with externally-supplied, pre-ranked search
    /// results (FTS or semantic) and mark the panel as showing search results.
    ///
    /// Unlike [`set_items`](Self::set_items) this does **not** re-apply the
    /// panel's sort method: search results carry their own ranking (relevance
    /// order for semantic search), which a date/name sort would destroy.
    /// Selection is preserved by text like `set_items`.
    pub fn set_search_items(&mut self, items: Vec<PanelItem>) {
        let selected_text = self.capture_selected_text();
        self.all_items = items;
        self.search_active = true;
        self.rebuild_view(selected_text);
    }

    /// Capture the text of the currently selected (filtered) item, if any.
    fn capture_selected_text(&self) -> Option<String> {
        self.list_state
            .borrow()
            .selected()
            .and_then(|idx| self.items.get(idx).map(|i| i.text().to_string()))
    }

    /// Re-apply the filter and restore the selection by text, falling back to
    /// the first item. Shared tail of [`set_items`] and [`set_search_items`].
    fn rebuild_view(&mut self, selected_text: Option<String>) {
        self.apply_filter();

        let mut state = self.list_state.borrow_mut();
        if !self.items.is_empty() {
            // Try to restore the selection
            let new_sel = if let Some(text) = selected_text {
                self.items.iter().position(|item| item.text() == text).or(Some(0))
            } else {
                Some(0)
            };
            state.select(new_sel);
        } else {
            state.select(None);
        }
    }

    /// Check if selection changed (for live preview).
    /// Returns the selected item's user data if changed, None otherwise.
    pub fn take_selection_change(&mut self) -> Option<String> {
        let current = self.list_state.borrow().selected();
        let last = self.last_selected_index.get();

        if current != last {
            self.last_selected_index.set(current);
            current.and_then(|idx| self.items.get(idx)?.user_data.clone())
        } else {
            None
        }
    }
}

impl Component for Panel {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.last_area.set(area);

        // Apply any selection scroll that was deferred because the area was
        // unknown when the selection was set (e.g. selecting the active repo at
        // construction). Re-derive the *current* selection now that the height
        // is known — reading it here (rather than a saved index) keeps it valid
        // even if the item list was rebuilt in the meantime.
        if self.pending_scroll.take() {
            // Read (and drop the borrow) before `scroll_to_view` takes it mutably.
            let selected = self.list_state.borrow().selected();
            if let Some(idx) = selected {
                tracing::debug!(target: "codemark::ui", idx, "applying deferred panel scroll on first render");
                self.scroll_to_view(idx);
            }
        }

        // Calculate inner area (excluding borders)
        let inner = if self.bordered { area.inner(Margin::new(1, 1)) } else { area };

        let height = inner.height as usize;

        // Width available to each item's text. Leave one column for the
        // scrollbar/selection indicator so compressed paths don't touch the edge.
        let content_width = inner.width.saturating_sub(1);

        // Build all items (Ratatui List handles scrolling internally via ListState)
        let list_items: Vec<ListItem> = self
            .items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let is_selected = self.list_state.borrow().selected() == Some(i);
                let line = item.to_line(is_selected, self.focused, content_width);
                let mut list_item = ListItem::new(line);

                if is_selected {
                    // Keep the same highlight background whether or not the pane is
                    // focused — the previous dimmed/blackish color looked poor.
                    list_item = list_item.style(Style::default().bg(crate::theme::SELECTION_BG));
                }
                list_item
            })
            .collect();

        // Create the block with border
        let block = Block::bordered()
            .border_type(self.border_type)
            .title(self.title.as_str())
            .title_style(if self.focused {
                self.focus_style.add_modifier(Modifier::BOLD)
            } else {
                self.normal_style
            })
            .border_style(if self.focused { self.focus_style } else { self.normal_style });

        // Render the list using StatefulWidget for native scrolling.
        // We temporarily hide the selection from ListState during render so that
        // the List widget doesn't force the selected item into view, which
        // would block our manual scroll offset. We already manually highlighted
        // the selected item in list_items above.
        let list = List::new(list_items);

        let mut state = self.list_state.borrow_mut();
        let actual_selection = state.selected();
        let actual_offset = state.offset();
        state.select(None);
        *state.offset_mut() = actual_offset; // Restore offset after select(None) clears it

        if self.bordered {
            StatefulWidget::render(list.block(block), area, buf, &mut *state);
        } else {
            StatefulWidget::render(list, area, buf, &mut *state);
        };

        // Restore the selection and offset
        state.select(actual_selection);
        *state.offset_mut() = actual_offset;

        // Render scrollbar if needed
        if self.show_scrollbar && !self.items.is_empty() && self.items.len() > height {
            let scrollbar_style = if self.focused { self.focus_style } else { self.normal_style };

            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(None)
                .thumb_symbol("┃")
                .style(scrollbar_style);

            // Ratatui 0.29's thumb math sizes the thumb as
            // `viewport / (content_length - 1 + viewport)` and caps the thumb position at
            // `content_length - 1`. Passing the raw item count there makes the thumb too
            // short and unable to reach the bottom (a gap of one thumb-height is left).
            // Instead, model `content_length` as the number of scroll positions
            // (`items.len() - height + 1`) and leave `viewport_content_length` at its
            // default so Ratatui uses the track height (which equals `height` here) as the
            // viewport. This yields a thumb of size `height^2 / items.len()` (the correct
            // visible/total ratio) and a max position of `items.len() - height`, so the
            // thumb reaches the bottom on the last page.
            let scroll_positions = self.items.len() - height + 1;
            let mut scrollbar_state =
                ScrollbarState::new(scroll_positions).position(state.offset());

            let scrollbar_area = if self.bordered {
                Rect {
                    x: area.right() - 1,
                    y: area.top() + 1,
                    width: 1,
                    height: area.height.saturating_sub(2),
                }
            } else {
                Rect { x: area.right() - 1, y: area.top(), width: 1, height: area.height }
            };

            scrollbar.render(scrollbar_area, buf, &mut scrollbar_state);
        }

        // Render selection indicator on bottom border (e.g., "3 of 15")
        if self.bordered && !self.items.is_empty() {
            let current = state.selected().map_or(0, |i| i + 1);
            let total = self.items.len();
            let indicator = format!(" {current} of {total} ");

            let indicator_width = indicator.len() as u16;

            // Position on bottom border, aligned right (shift left by 1 to avoid corner)
            let x = area.right().saturating_sub(indicator_width + 1);
            let y = area.bottom() - 1;

            if x > area.left() {
                let indicator_style = if self.focused {
                    self.focus_style.add_modifier(Modifier::BOLD)
                } else {
                    self.normal_style
                };

                // Text characters
                for (i, c) in indicator.chars().enumerate() {
                    let cx = x + i as u16;
                    if let Some(cell) = buf.cell_mut((cx, y)) {
                        cell.set_char(c);
                        cell.set_style(indicator_style);
                    }
                }
            }
        }
    }

    fn handle_event(&mut self, event: &Event) -> bool {
        match event {
            Event::Key(key) => {
                if !self.focused {
                    return false;
                }
                match key.code {
                    ratatui::crossterm::event::KeyCode::Down
                    | ratatui::crossterm::event::KeyCode::Char('j') => {
                        self.select_next();
                        true
                    }
                    ratatui::crossterm::event::KeyCode::Up
                    | ratatui::crossterm::event::KeyCode::Char('k') => {
                        self.select_previous();
                        true
                    }
                    ratatui::crossterm::event::KeyCode::Char('J') => {
                        let mut state = self.list_state.borrow_mut();
                        let area = self.last_area.get();
                        let height =
                            area.height.saturating_sub(if self.bordered { 2 } else { 0 }) as usize;
                        if self.items.len() > height {
                            let offset = state.offset();
                            let new_offset =
                                (offset + 5).min(self.items.len().saturating_sub(height));
                            if new_offset != offset {
                                *state.offset_mut() = new_offset;
                                return true;
                            }
                        }
                        false
                    }
                    ratatui::crossterm::event::KeyCode::Char('K') => {
                        let mut state = self.list_state.borrow_mut();
                        let offset = state.offset();
                        let new_offset = offset.saturating_sub(5);
                        if new_offset != offset {
                            *state.offset_mut() = new_offset;
                            return true;
                        }
                        false
                    }
                    ratatui::crossterm::event::KeyCode::Char(' ') => {
                        self.activate_selected();
                        true
                    }
                    _ => false,
                }
            }
            Event::Mouse(mouse) => {
                let area = self.last_area.get();
                let is_hovered =
                    area.contains(ratatui::layout::Position::from((mouse.column, mouse.row)));

                match mouse.kind {
                    ratatui::crossterm::event::MouseEventKind::Down(button) => {
                        if !self.focused {
                            return false;
                        }

                        if button == ratatui::crossterm::event::MouseButton::Left && is_hovered {
                            // Calculate inner area to get correct item index
                            let inner =
                                if self.bordered { area.inner(Margin::new(1, 1)) } else { area };

                            if inner.contains(ratatui::layout::Position::from((
                                mouse.column,
                                mouse.row,
                            ))) {
                                let relative_row = mouse.row.saturating_sub(inner.y) as usize;
                                let item_idx = relative_row + self.list_state.borrow().offset();
                                if item_idx < self.items.len() {
                                    self.list_state.borrow_mut().select(Some(item_idx));
                                    return true;
                                }
                            }
                            return true;
                        }
                        false
                    }
                    ratatui::crossterm::event::MouseEventKind::ScrollDown if is_hovered => {
                        let mut state = self.list_state.borrow_mut();
                        let height =
                            area.height.saturating_sub(if self.bordered { 2 } else { 0 }) as usize;
                        if self.items.len() > height {
                            let offset = state.offset();
                            let new_offset =
                                (offset + 1).min(self.items.len().saturating_sub(height));
                            if new_offset != offset {
                                *state.offset_mut() = new_offset;
                                return true;
                            }
                        }
                        false
                    }
                    ratatui::crossterm::event::MouseEventKind::ScrollUp if is_hovered => {
                        let mut state = self.list_state.borrow_mut();
                        let offset = state.offset();
                        let new_offset = offset.saturating_sub(1);
                        if new_offset != offset {
                            *state.offset_mut() = new_offset;
                            return true;
                        }
                        false
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }

    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focus(&mut self, focused: bool) {
        self.focused = focused;
    }

    fn size_constraints(&self) -> SizeConstraints {
        SizeConstraints::min(10, 5)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_panel_navigation() {
        let mut panel = Panel::new("Test").items(vec![
            PanelItem::new("item1"),
            PanelItem::new("item2"),
            PanelItem::new("item3"),
        ]);

        assert_eq!(panel.selected_index(), Some(0));

        panel.select_next();
        assert_eq!(panel.selected_index(), Some(1));

        panel.select_next();
        assert_eq!(panel.selected_index(), Some(2));

        panel.select_next(); // Should wrap to 0
        assert_eq!(panel.selected_index(), Some(0));

        panel.select_previous(); // Should wrap to 2
        assert_eq!(panel.selected_index(), Some(2));
    }

    #[test]
    fn test_panel_empty() {
        let panel = Panel::new("Test");
        assert!(panel.is_empty());
        assert_eq!(panel.len(), 0);
    }

    #[test]
    fn test_set_selected_before_render_scrolls_into_view_on_first_render() {
        // A selection set while the panel has no area (e.g. at construction)
        // must scroll the selected item into view once the first render supplies
        // real dimensions — mirroring selecting the active repo on startup.
        let items: Vec<PanelItem> = (0..30).map(|i| PanelItem::new(format!("item{i}"))).collect();
        let panel = Panel::new("").bordered(false).items(items);

        // Select an item far down the list before any render has happened.
        let mut panel = panel;
        panel.set_selected(25);
        assert_eq!(panel.selected_index(), Some(25));
        // No area yet, so the offset can't move and the scroll is deferred.
        assert_eq!(panel.list_state.borrow().offset(), 0);

        // First render with a short viewport (10 rows) reveals the selection.
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        panel.render(area, &mut buf);

        let offset = panel.list_state.borrow().offset();
        assert!(offset > 0, "expected the deferred scroll to move the viewport");
        // The selected index must fall within the visible window [offset, offset+height).
        assert!(offset <= 25 && 25 < offset + 10, "selection {} not visible (offset {offset})", 25);
    }

    #[test]
    fn test_deferred_scroll_survives_item_rebuild_before_first_render() {
        // Regression: a deferred scroll must reveal whatever is selected at
        // render time, not a numeric index captured earlier. If the list is
        // rebuilt (via `set_items`) after the selection but before the first
        // render, the render must still bring the *current* selection into view
        // rather than scrolling to a now-stale index.
        let items: Vec<PanelItem> = (0..30).map(|i| PanelItem::new(format!("item{i}"))).collect();
        let mut panel = Panel::new("").bordered(false).items(items);

        // Select far down the list before any render (defers the scroll).
        panel.set_selected(25);

        // Rebuild with a shorter list before rendering. Selection falls back to
        // 0 because "item25" is gone; the old deferred index (25) is now stale.
        panel.set_items((0..5).map(|i| PanelItem::new(format!("new{i}"))).collect());
        assert_eq!(panel.selected_index(), Some(0));

        // First render must not scroll to the stale index 25 (which would push
        // the viewport past the 5-item list); the offset stays at 0.
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        panel.render(area, &mut buf);
        assert_eq!(panel.list_state.borrow().offset(), 0);
    }

    #[test]
    fn test_panel_set_items() {
        let mut panel =
            Panel::new("Test").items(vec![PanelItem::new("item1"), PanelItem::new("item2")]);

        panel.set_selected(1);

        panel.set_items(vec![
            PanelItem::new("new1"),
            PanelItem::new("item2"), // Same text, should preserve selection
            PanelItem::new("new3"),
        ]);

        assert_eq!(panel.len(), 3);
        // Should preserve selection based on text match
        assert_eq!(panel.selected_index(), Some(1));
    }

    #[test]
    fn multi_select_toggle_tracks_active_items_and_reveals_uncheck_last() {
        // The multi-select Repos panel drives the query scope: `activate_selected`
        // toggles the selected item, and `active_items()` reports the checked set
        // by `user_data`. This is exactly the API `activate_context_selection`
        // relies on, including its uncheck-last guard, which triggers when
        // toggling off the sole checked item leaves `active_items()` empty.
        let mut panel = Panel::new("").multi_select(true).items(vec![
            PanelItem::new("repo-a").user_data("/repos/a").active(true),
            PanelItem::new("repo-b").user_data("/repos/b"),
        ]);

        // Startup: exactly one repo checked (mirrors the focus repo).
        assert_eq!(panel.active_items(), vec!["/repos/a".to_string()]);

        // Toggle the second repo on → both checked (scope fans out).
        panel.set_selected(1);
        panel.activate_selected();
        let mut checked = panel.active_items();
        checked.sort();
        assert_eq!(checked, vec!["/repos/a".to_string(), "/repos/b".to_string()]);

        // Toggle the second repo back off → single repo again.
        panel.activate_selected();
        assert_eq!(panel.active_items(), vec!["/repos/a".to_string()]);

        // Uncheck-last: toggling off the sole remaining checked repo leaves the
        // panel with zero checkmarks — the condition the guard detects. Re-toggling
        // (the guard's revert) restores the single checkmark.
        panel.set_selected(0);
        panel.activate_selected();
        assert!(panel.active_items().is_empty(), "expected zero checkmarks after uncheck-last");
        panel.activate_selected();
        assert_eq!(panel.active_items(), vec!["/repos/a".to_string()]);
    }

    #[test]
    fn test_alphabetical_sort_orders_by_display_name() {
        // Alphabetical sort keys off the emphasis text (a bookmark's symbol)
        // when present, otherwise the primary text.
        let panel = Panel::new("").sort(SortMethod::AlphabeticalAsc).items(vec![
            PanelItem::new("charlie"),
            PanelItem::new("alpha"),
            PanelItem::new("bravo"),
        ]);
        let order: Vec<&str> = panel.all_items().iter().map(|i| i.text()).collect();
        assert_eq!(order, vec!["alpha", "bravo", "charlie"]);

        let mut panel = panel;
        panel.set_sort(SortMethod::AlphabeticalDesc);
        let order: Vec<&str> = panel.all_items().iter().map(|i| i.text()).collect();
        assert_eq!(order, vec!["charlie", "bravo", "alpha"]);
    }

    #[test]
    fn test_date_sort_orders_by_created_at_with_missing_last() {
        let panel = Panel::new("").sort(SortMethod::DateNewest).items(vec![
            PanelItem::new("old").created_at("2024-01-01T00:00:00Z"),
            PanelItem::new("new").created_at("2026-01-01T00:00:00Z"),
            PanelItem::new("mid").created_at("2025-01-01T00:00:00Z"),
            PanelItem::new("undated"),
        ]);
        // Newest first; the undated item sinks to the end.
        let order: Vec<&str> = panel.all_items().iter().map(|i| i.text()).collect();
        assert_eq!(order, vec!["new", "mid", "old", "undated"]);

        let mut panel = panel;
        panel.set_sort(SortMethod::DateOldest);
        let order: Vec<&str> = panel.all_items().iter().map(|i| i.text()).collect();
        // Oldest first; the undated item still sinks to the end.
        assert_eq!(order, vec!["old", "mid", "new", "undated"]);
    }

    #[test]
    fn test_cycle_sort_keeps_selection_on_same_item() {
        let mut panel = Panel::new("").sort(SortMethod::DateNewest).items(vec![
            PanelItem::new("a").created_at("2024-01-01T00:00:00Z").user_data("id-a"),
            PanelItem::new("b").created_at("2026-01-01T00:00:00Z").user_data("id-b"),
            PanelItem::new("c").created_at("2025-01-01T00:00:00Z").user_data("id-c"),
        ]);
        // Select "c" (currently in the middle under DateNewest: b, c, a).
        panel.set_selected(1);
        assert_eq!(panel.selected().and_then(|i| i.user_data.clone()), Some("id-c".to_string()));

        // Cycling the order must keep the cursor on "c", not the row index.
        panel.cycle_sort(); // DateNewest -> DateOldest: a, c, b
        assert_eq!(panel.selected().and_then(|i| i.user_data.clone()), Some("id-c".to_string()));
    }

    #[test]
    fn test_sort_persists_across_set_items() {
        let mut panel = Panel::new("")
            .sort(SortMethod::AlphabeticalAsc)
            .items(vec![PanelItem::new("b"), PanelItem::new("a")]);
        assert_eq!(panel.all_items().iter().map(|i| i.text()).collect::<Vec<_>>(), vec!["a", "b"]);

        // A rebuild (e.g. panel refresh) re-applies the active sort.
        panel.set_items(vec![PanelItem::new("z"), PanelItem::new("m"), PanelItem::new("a")]);
        assert_eq!(
            panel.all_items().iter().map(|i| i.text()).collect::<Vec<_>>(),
            vec!["a", "m", "z"]
        );
    }

    #[test]
    fn test_sort_is_preserved_through_filtering_both_orders() {
        let make = || {
            Panel::new("").sort(SortMethod::AlphabeticalAsc).items(vec![
                PanelItem::new("banana"),
                PanelItem::new("apricot"),
                PanelItem::new("avocado"),
                PanelItem::new("cherry"),
            ])
        };

        // Sort-then-filter: the visible list is the sorted order, narrowed to
        // matches (substring "a" hits every item but "cherry"), still ascending.
        let mut panel = make();
        panel.set_filter("a");
        let visible: Vec<&str> = panel.items.iter().map(|i| i.text()).collect();
        assert_eq!(visible, vec!["apricot", "avocado", "banana"]);

        // Filter-first, then cycle sort: re-sorting re-applies the active filter,
        // so the narrowed list flips to descending order.
        let mut panel = make();
        panel.set_filter("a"); // apricot, avocado, banana
        panel.set_sort(SortMethod::AlphabeticalDesc);
        assert!(panel.is_filtered());
        let visible: Vec<&str> = panel.items.iter().map(|i| i.text()).collect();
        assert_eq!(visible, vec!["banana", "avocado", "apricot"]);

        // Clearing the filter reveals the full list, still in the active
        // (descending) sort order.
        panel.set_filter("");
        let visible: Vec<&str> = panel.items.iter().map(|i| i.text()).collect();
        assert_eq!(visible, vec!["cherry", "banana", "avocado", "apricot"]);
    }

    #[test]
    fn test_unsorted_panel_keeps_insertion_order() {
        // Panels without an explicit sort (the default) are untouched.
        let panel = Panel::new("").items(vec![
            PanelItem::new("charlie"),
            PanelItem::new("alpha"),
            PanelItem::new("bravo"),
        ]);
        assert_eq!(
            panel.all_items().iter().map(|i| i.text()).collect::<Vec<_>>(),
            vec!["charlie", "alpha", "bravo"]
        );
        assert_eq!(panel.sort_method(), None);
    }

    #[test]
    fn test_search_active_counts_as_filtered_and_resets_on_rebuild() {
        let mut panel = Panel::new("Bookmarks");
        assert!(!panel.is_filtered());

        // Applying search results: marked as filtered without a local query.
        panel.set_search_items(vec![PanelItem::new("result1")]);
        assert!(panel.is_filtered());

        // A subsequent rebuild (e.g. clearing the search) drops the marking.
        panel.set_items(vec![PanelItem::new("all1"), PanelItem::new("all2")]);
        assert!(!panel.is_filtered());
    }

    #[test]
    fn test_search_items_preserve_caller_order_over_panel_sort() {
        // Regression: a search-result list is pre-ranked by the search engine
        // (e.g. semantic distance). The panel must NOT re-sort it by its
        // configured SortMethod — otherwise the relevance order is destroyed
        // and every query yields the same date-sorted list.
        let mut panel = Panel::new("").sort(SortMethod::DateNewest);

        // Hand in results out of date order on purpose.
        panel.set_search_items(vec![
            PanelItem::new("old").created_at("2024-01-01T00:00:00Z"),
            PanelItem::new("new").created_at("2026-01-01T00:00:00Z"),
            PanelItem::new("mid").created_at("2025-01-01T00:00:00Z"),
        ]);

        // Insertion (relevance) order is preserved, not re-sorted by date.
        let order: Vec<&str> = panel.all_items().iter().map(|i| i.text()).collect();
        assert_eq!(order, vec!["old", "new", "mid"]);
        assert!(panel.is_search_active());

        // A normal rebuild re-applies the sort, so date order returns — the
        // bypass only applies while showing search results.
        panel.set_items(vec![
            PanelItem::new("old").created_at("2024-01-01T00:00:00Z"),
            PanelItem::new("new").created_at("2026-01-01T00:00:00Z"),
            PanelItem::new("mid").created_at("2025-01-01T00:00:00Z"),
        ]);
        let order: Vec<&str> = panel.all_items().iter().map(|i| i.text()).collect();
        assert_eq!(order, vec!["new", "mid", "old"]);
    }
}
