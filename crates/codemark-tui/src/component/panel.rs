//! Scrollable panel component for displaying lists of items.
//!
//! Panels are the primary container component for displaying scrollable
//! content with headers and borders.

use ratatui::{
    buffer::Buffer,
    layout::{Margin, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{
        Block, BorderType, List, ListItem, ListState, Scrollbar, ScrollbarOrientation,
        ScrollbarState, StatefulWidget,
    },
};

use super::{Component, SizeConstraints};
use crate::event::Event;

use std::cell::{Cell, RefCell};

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
}

/// Health status indicator for an item based on the projected UI status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    /// 🟢 Healthy
    Healthy,
    /// 🟡 Unanchored (Healthy)
    UnanchoredHealthy,
    /// 🟡 Drifted
    Drifted,
    /// 🟠 Unanchored (Drifting)
    UnanchoredDrifting,
    /// 🔴 Broken
    Broken,
    /// 🔴 Broken (Unanchored)
    BrokenUnanchored,
    /// ⚪ Verified (Historical)
    Verified,
    /// ⚪ Outdated (Historical)
    Outdated,
    /// 🔵 Future
    Future,
    /// Unknown/Gray - gray
    Unknown,
}

impl From<codemark_core::engine::projection::UIStatus> for HealthStatus {
    fn from(status: codemark_core::engine::projection::UIStatus) -> Self {
        use codemark_core::engine::projection::UIStatus;
        match status {
            UIStatus::Healthy => HealthStatus::Healthy,
            UIStatus::UnanchoredHealthy => HealthStatus::UnanchoredHealthy,
            UIStatus::Drifted => HealthStatus::Drifted,
            UIStatus::UnanchoredDrifting => HealthStatus::UnanchoredDrifting,
            UIStatus::Broken => HealthStatus::Broken,
            UIStatus::BrokenUnanchored => HealthStatus::BrokenUnanchored,
            UIStatus::Verified => HealthStatus::Verified,
            UIStatus::Outdated => HealthStatus::Outdated,
            UIStatus::Future => HealthStatus::Future,
        }
    }
}

impl HealthStatus {
    /// Get the color for this health status.
    fn color(&self) -> Color {
        match self {
            HealthStatus::Healthy => Color::Green,
            HealthStatus::UnanchoredHealthy => Color::Yellow,
            HealthStatus::Drifted => Color::Yellow,
            HealthStatus::UnanchoredDrifting => Color::Rgb(255, 165, 0), // Orange
            HealthStatus::Broken | HealthStatus::BrokenUnanchored => Color::Red,
            HealthStatus::Verified => Color::Green,
            HealthStatus::Outdated => Color::Yellow,
            HealthStatus::Unknown => Color::DarkGray,
            HealthStatus::Future => Color::Blue,
        }
    }

    /// Get the symbol for this health status.
    fn symbol(&self) -> &'static str {
        match self {
            HealthStatus::Verified | HealthStatus::Outdated => "○", // Unfilled circle for historical statuses
            _ => "●", // Filled dot for all other statuses
        }
    }
}

/// An item in a panel.
#[derive(Debug, Clone)]
pub struct PanelItem {
    /// The primary text to display
    text: String,
    /// Optional secondary text (shown in a different color)
    secondary_text: Option<String>,
    /// Optional metadata associated with this item
    metadata: Option<String>,
    /// Optional icon (NERD font symbol)
    icon: Option<String>,
    /// Health status indicator
    health: Option<HealthStatus>,
    /// Primary text color
    text_color: Option<Color>,
    /// Whether the item has a trailing checkmark (e.g., published tour)
    checkmark: bool,
    /// Sync direction indicator for tours (push/pull)
    sync_direction: Option<SyncDirection>,
    /// Whether this item is currently active (e.g., active workspace)
    active: bool,
    /// Whether the item is published to a server (shows cloud upload icon)
    published: bool,
    /// Optional hidden user data (e.g., database ID)
    pub user_data: Option<String>,
}

/// Sync direction indicator for tours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncDirection {
    /// Tour can be pushed up (local changes need to be published)
    Push,
    /// Tour can be pulled down (remote updates available)
    Pull,
    /// Tour is in sync
    Synced,
}

impl PanelItem {
    /// Create a new panel item.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            secondary_text: None,
            metadata: None,
            icon: None,
            health: Some(HealthStatus::Unknown),
            text_color: None,
            checkmark: false,
            sync_direction: None,
            active: false,
            published: false,
            user_data: None,
        }
    }

    /// Set the icon.
    pub fn icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Set hidden user data.
    pub fn user_data(mut self, data: impl Into<String>) -> Self {
        self.user_data = Some(data.into());
        self
    }

    /// Set whether this item is active.
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// Set whether the item is published.
    pub fn published(mut self, published: bool) -> Self {
        self.published = published;
        self
    }

    /// Get the item text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Get the secondary text value.
    pub fn get_secondary_text(&self) -> Option<&str> {
        self.secondary_text.as_deref()
    }

    /// Set the secondary text.
    pub fn secondary_text(mut self, text: impl Into<String>) -> Self {
        self.secondary_text = Some(text.into());
        self
    }

    /// Update the secondary text in place.
    pub fn set_secondary_text(&mut self, text: impl Into<String>) {
        self.secondary_text = Some(text.into());
    }

    /// Set the metadata.
    pub fn metadata(mut self, metadata: impl Into<String>) -> Self {
        self.metadata = Some(metadata.into());
        self
    }

    /// Set the health status.
    pub fn health(mut self, health: HealthStatus) -> Self {
        self.health = Some(health);
        self
    }

    /// Set the health status to None (hide the indicator).
    pub fn no_health(mut self) -> Self {
        self.health = None;
        self
    }

    /// Set the primary text color.
    pub fn color(mut self, color: Color) -> Self {
        self.text_color = Some(color);
        self
    }

    /// Set whether to show a trailing checkmark (e.g., for published tours).
    pub fn checkmark(mut self, checkmark: bool) -> Self {
        self.checkmark = checkmark;
        self
    }

    /// Set the sync direction indicator for tours.
    pub fn sync_direction(mut self, direction: Option<SyncDirection>) -> Self {
        self.sync_direction = direction;
        self
    }

    /// Render this item as a Line.
    fn to_line(&self, selected: bool, focused: bool) -> Line<'_> {
        let mut spans = Vec::new();

        // Add padding prefix for alignment
        spans.push(Span::raw("  "));

        // Add health status indicator if present
        if let Some(health) = self.health {
            spans.push(Span::styled(health.symbol(), Style::default().fg(health.color())));
            spans.push(Span::raw(" "));
        }

        // Add icon if present
        if let Some(icon) = &self.icon
            && !icon.is_empty()
        {
            spans.push(Span::styled(icon, Style::default().fg(Color::Cyan)));
            spans.push(Span::raw(" "));
        }

        // Add primary text
        let primary_style = if let Some(color) = self.text_color {
            Style::default().fg(color)
        } else {
            Style::default()
        };
        spans.push(Span::styled(&self.text, primary_style));

        // Add secondary text if present
        if let Some(secondary) = &self.secondary_text {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(secondary, Style::default().fg(Color::DarkGray)));
        }

        // Add metadata if present
        if let Some(metadata) = &self.metadata {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(metadata, Style::default().fg(Color::Cyan)));
        }

        // Add sync direction arrow for tours (omit if synced)
        if let Some(direction) = &self.sync_direction {
            match direction {
                SyncDirection::Synced => {
                    // Don't show anything when synced
                }
                _ => {
                    spans.push(Span::raw(" "));
                    let (arrow, color) = match direction {
                        SyncDirection::Push => ("↑", Color::Cyan),
                        SyncDirection::Pull => ("↓", Color::Yellow),
                        SyncDirection::Synced => unreachable!(),
                    };
                    spans.push(Span::styled(arrow, Style::default().fg(color)));
                }
            }
        }
        // Add trailing checkmark if enabled or active
        if self.checkmark || self.active {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                "✓",
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            ));
        }

        // Add cloud upload icon if published to server (pushed)
        if self.published {
            spans.push(Span::raw(" "));
            spans.push(Span::styled("☁", Style::default().fg(Color::Cyan)));
        }

        let mut line = Line::from(spans);
        if selected && focused {
            line = line.bold();
        }
        line
    }
}

impl Panel {
    /// Create a new panel with the given title.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            items: Vec::new(),
            all_items: Vec::new(),
            filter_query: String::new(),
            list_state: RefCell::new(ListState::default()),
            bordered: true,
            focused: false,
            focus_style: Style::default().fg(Color::Green),
            normal_style: Style::default().fg(Color::DarkGray),
            show_scrollbar: true,
            multi_select: false,
            border_type: BorderType::Rounded,
            last_area: Cell::new(Rect::default()),
            last_selected_index: Cell::new(None),
        }
    }

    /// Set whether the panel supports multi-selection.
    pub fn multi_select(mut self, multi_select: bool) -> Self {
        self.multi_select = multi_select;
        self
    }

    /// Get the last rendered area.
    pub fn last_area(&self) -> Rect {
        self.last_area.get()
    }

    /// Add an item to the panel.
    pub fn add_item(mut self, item: PanelItem) -> Self {
        self.all_items.push(item);
        self.apply_filter();
        if self.selected_index().is_none() && !self.items.is_empty() {
            self.set_selected(0);
        }
        self
    }

    /// Add multiple items to the panel.
    pub fn items(mut self, items: impl IntoIterator<Item = PanelItem>) -> Self {
        self.all_items.extend(items);
        self.apply_filter();
        if self.selected_index().is_none() && !self.items.is_empty() {
            self.set_selected(0);
        }
        self
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
            self.items = self
                .all_items
                .iter()
                .filter(|item| item.text.to_lowercase().contains(&query))
                .cloned()
                .collect();
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
            .filter(|i| i.active)
            .map(|i| i.user_data.clone().unwrap_or_else(|| i.text.clone()))
            .collect()
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
            let key_text = item.text.clone();

            // Helper to match items by stable identifier (user_data) or fallback to text
            let matches = |i: &PanelItem| -> bool {
                if let Some(ref key) = key_user_data {
                    i.user_data.as_ref() == Some(key)
                } else {
                    i.text == key_text
                }
            };

            if self.multi_select {
                // Toggle in all_items
                if let Some(item) = self.all_items.iter_mut().find(|i| matches(i)) {
                    item.active = !item.active;
                }
            } else {
                // Determine if the target item is already active
                let was_active =
                    self.all_items.iter().find(|i| matches(i)).map(|i| i.active).unwrap_or(false);

                // Deactivate all
                for item in &mut self.all_items {
                    item.active = false;
                }

                // If it wasn't active before, activate it now.
                // If it was active, it remains inactive (toggled off).
                if !was_active && let Some(item) = self.all_items.iter_mut().find(|i| matches(i)) {
                    item.active = true;
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
                    .filter(|item| item.text.to_lowercase().contains(&query))
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
                item.active = true;
            }
        }
        self.apply_filter();
    }

    /// Clear all items from the panel.
    pub fn clear(&mut self) {
        self.items.clear();
        self.all_items.clear();
        self.list_state.borrow_mut().select(None);
        self.filter_query.clear();
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

    /// Update the items in the panel, preserving selection if possible.
    pub fn set_items(&mut self, items: Vec<PanelItem>) {
        let selected_text = self
            .list_state
            .borrow()
            .selected()
            .and_then(|idx| self.items.get(idx).map(|i| i.text.clone()));
        self.all_items = items;
        self.apply_filter();

        let mut state = self.list_state.borrow_mut();
        if !self.items.is_empty() {
            // Try to restore the selection
            let new_sel = if let Some(text) = selected_text {
                self.items.iter().position(|item| item.text == text).or(Some(0))
            } else {
                Some(0)
            };
            state.select(new_sel);
        } else {
            state.select(None);
        }
    }
}

impl Component for Panel {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.last_area.set(area);
        // Calculate inner area (excluding borders)
        let inner = if self.bordered { area.inner(Margin::new(1, 1)) } else { area };

        let height = inner.height as usize;

        // Build all items (Ratatui List handles scrolling internally via ListState)
        let list_items: Vec<ListItem> = self
            .items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let is_selected = self.list_state.borrow().selected() == Some(i);
                let line = item.to_line(is_selected, self.focused);
                let mut list_item = ListItem::new(line);

                if is_selected {
                    let bg_color = if self.focused {
                        Color::Rgb(62, 68, 81) // Light gray-blue highlight for focused (One Dark)
                    } else {
                        Color::Rgb(40, 44, 52) // Darker gray-blue for unfocused
                    };
                    list_item = list_item.style(Style::default().bg(bg_color));
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

            // Note: We don't set viewport_content_length because Ratatui's scrollbar has a bug
            // where it calculates thumb size incorrectly. When viewport_content_length is 0,
            // the scrollbar uses its track height, which happens to be correct for our use case
            // since the list and scrollbar have the same height.
            // See: https://github.com/ratatui/ratatui/issues/966
            let mut scrollbar_state = ScrollbarState::new(self.items.len()).position(state.offset());

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

impl Panel {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_panel_item_creation() {
        let item = PanelItem::new("test").secondary_text("secondary").metadata("meta");
        assert_eq!(item.text, "test");
        assert_eq!(item.secondary_text, Some("secondary".to_string()));
        assert_eq!(item.metadata, Some("meta".to_string()));
    }

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
    fn test_ui_status_to_health_status_exhaustive() {
        use codemark_core::engine::projection::UIStatus;

        assert_eq!(HealthStatus::from(UIStatus::Healthy), HealthStatus::Healthy);
        assert_eq!(
            HealthStatus::from(UIStatus::UnanchoredHealthy),
            HealthStatus::UnanchoredHealthy
        );
        assert_eq!(HealthStatus::from(UIStatus::Drifted), HealthStatus::Drifted);
        assert_eq!(
            HealthStatus::from(UIStatus::UnanchoredDrifting),
            HealthStatus::UnanchoredDrifting
        );
        assert_eq!(HealthStatus::from(UIStatus::Broken), HealthStatus::Broken);
        assert_eq!(HealthStatus::from(UIStatus::BrokenUnanchored), HealthStatus::BrokenUnanchored);
        assert_eq!(HealthStatus::from(UIStatus::Verified), HealthStatus::Verified);
        assert_eq!(HealthStatus::from(UIStatus::Outdated), HealthStatus::Outdated);
        assert_eq!(HealthStatus::from(UIStatus::Future), HealthStatus::Future);
    }

    #[test]
    fn test_health_status_colors() {
        use ratatui::style::Color;

        assert_eq!(HealthStatus::Healthy.color(), Color::Green);
        assert_eq!(HealthStatus::UnanchoredHealthy.color(), Color::Yellow);
        assert_eq!(HealthStatus::Drifted.color(), Color::Yellow);
        assert_eq!(HealthStatus::UnanchoredDrifting.color(), Color::Rgb(255, 165, 0));
        assert_eq!(HealthStatus::Broken.color(), Color::Red);
        assert_eq!(HealthStatus::BrokenUnanchored.color(), Color::Red);
        assert_eq!(HealthStatus::Verified.color(), Color::Green);
        assert_eq!(HealthStatus::Outdated.color(), Color::Yellow);
        assert_eq!(HealthStatus::Unknown.color(), Color::DarkGray);
        assert_eq!(HealthStatus::Future.color(), Color::Blue);
    }

    #[test]
    fn test_health_status_symbols() {
        // Verified and Outdated statuses use an unfilled circle (historical)
        assert_eq!(HealthStatus::Verified.symbol(), "○");
        assert_eq!(HealthStatus::Outdated.symbol(), "○");
        // All other statuses use a filled dot
        assert_eq!(HealthStatus::Healthy.symbol(), "●");
        assert_eq!(HealthStatus::UnanchoredHealthy.symbol(), "●");
        assert_eq!(HealthStatus::Drifted.symbol(), "●");
        assert_eq!(HealthStatus::Broken.symbol(), "●");
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
}
