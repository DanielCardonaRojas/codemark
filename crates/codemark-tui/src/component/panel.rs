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

/// Shorten a file path to fit within `max_width` display columns, prioritizing
/// the last path components.
///
/// If the path exceeds `max_width`, it is truncated to show the trailing
/// components with a `../` prefix. For example, `/very/long/path/to/file.rs`
/// with `max_width=18` becomes `../path/to/file.rs`.
fn shorten_path(path: &str, max_width: usize) -> String {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

    if path.width() <= max_width {
        return path.to_string();
    }

    // Try to find a good breaking point by working backwards from the end
    let components: Vec<&str> = path.split('/').collect();
    let mut result = String::new();
    let prefix_overhead = 3; // "../" prefix that will be added if we truncate

    // Start from the last component and work backwards. All widths are measured
    // in display columns so multi-byte paths aren't truncated prematurely.
    for component in components.iter().rev() {
        let component_width = component.width();
        let separator_width = if result.is_empty() { 0 } else { 1 }; // '/' separator

        // Use max_width - prefix_overhead unconditionally to avoid edge cases
        // where we build a string that then exceeds max_width after prepending "../"
        let budget = max_width.saturating_sub(prefix_overhead);
        if result.width() + component_width + separator_width > budget {
            // Stop here and add "../" prefix if we have any components
            if !result.is_empty() {
                result = format!("../{}", result);
            }
            break;
        }

        // Add this component
        if result.is_empty() {
            result = component.to_string();
        } else {
            result = format!("{}/{}", component, result);
        }
    }

    // Fallback: if we couldn't build anything meaningful, keep the last
    // `max_width` display columns with a leading ellipsis, walking back over
    // char boundaries so we never split a multi-byte character.
    if result.is_empty() || result.width() > max_width {
        if path.width() > max_width {
            let keep = max_width.saturating_sub(3); // room for the "..." prefix
            let mut width = 0;
            let mut start = path.len();
            for (idx, ch) in path.char_indices().rev() {
                let w = ch.width().unwrap_or(0);
                if width + w > keep {
                    break;
                }
                width += w;
                start = idx;
            }
            format!("...{}", &path[start..])
        } else {
            path.to_string()
        }
    } else {
        result
    }
}

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

impl From<codemark_core::engine::resolution::LiveUIStatus> for HealthStatus {
    fn from(status: codemark_core::engine::resolution::LiveUIStatus) -> Self {
        use codemark_core::engine::resolution::LiveUIStatus;
        match status {
            LiveUIStatus::Healthy => HealthStatus::Healthy,
            LiveUIStatus::Drifted => HealthStatus::Drifted,
            LiveUIStatus::Broken => HealthStatus::Broken,
        }
    }
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
            HealthStatus::Healthy => crate::theme::palette().success,
            HealthStatus::UnanchoredHealthy => crate::theme::palette().warning,
            HealthStatus::Drifted => crate::theme::palette().warning,
            HealthStatus::UnanchoredDrifting => Color::Rgb(255, 165, 0), // Orange
            HealthStatus::Broken | HealthStatus::BrokenUnanchored => crate::theme::palette().error,
            HealthStatus::Verified => crate::theme::palette().success,
            HealthStatus::Outdated => crate::theme::palette().warning,
            HealthStatus::Unknown => crate::theme::palette().dim,
            HealthStatus::Future => crate::theme::palette().info,
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
    /// Optional emphasized text rendered bold (shown after primary text)
    emphasis_text: Option<String>,
    /// Optional secondary text (shown in a different color)
    secondary_text: Option<String>,
    /// Optional metadata associated with this item
    metadata: Option<String>,
    /// Optional icon (NERD font symbol)
    icon: Option<String>,
    /// Render the icon in the normal foreground color instead of the accent color.
    plain_icon: bool,
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
    /// Optional spinner text shown at the very end of the item
    spinner_text: Option<String>,
    /// When true, the primary `text` is treated as a file path that is
    /// compressed at render time to fit the available width of the pane.
    compress_path: bool,
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
            emphasis_text: None,
            secondary_text: None,
            metadata: None,
            icon: None,
            plain_icon: false,
            health: Some(HealthStatus::Unknown),
            text_color: None,
            checkmark: false,
            sync_direction: None,
            active: false,
            published: false,
            spinner_text: None,
            compress_path: false,
            user_data: None,
        }
    }

    /// Set the icon.
    pub fn icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Render the icon in the normal foreground color instead of the accent color.
    pub fn plain_icon(mut self) -> Self {
        self.plain_icon = true;
        self
    }

    /// Set the emphasized text (rendered bold, after primary text).
    pub fn emphasis(mut self, text: impl Into<String>) -> Self {
        self.emphasis_text = Some(text.into());
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

    /// Whether this item is currently active (selected/activated).
    pub fn is_active(&self) -> bool {
        self.active
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

    /// Mark the primary text as a file path that should be compressed at render
    /// time to fit the available width of the pane. The full, untruncated path
    /// is kept in `text` so the displayed length can grow or shrink as the pane
    /// is resized (e.g. when cycling through left-pane layouts).
    pub fn compressible_path(mut self) -> Self {
        self.compress_path = true;
        self
    }

    /// Total display width consumed by the spans rendered *after* the primary
    /// text. Used to compute how much horizontal space a compressible path may
    /// occupy. Each optional segment is preceded by a single space separator,
    /// mirroring the layout in [`Self::to_line`].
    fn trailing_width(&self) -> usize {
        use unicode_width::UnicodeWidthStr;

        let mut width = 0;

        if let Some(emphasis) = &self.emphasis_text {
            width += 1 + emphasis.width();
        }
        if let Some(secondary) = &self.secondary_text {
            width += 1 + secondary.width();
        }
        if let Some(metadata) = &self.metadata {
            width += 1 + metadata.width();
        }
        if matches!(self.sync_direction, Some(SyncDirection::Push) | Some(SyncDirection::Pull)) {
            width += 2; // separator + arrow
        }
        if self.checkmark || self.active {
            width += 2; // separator + check
        }
        if self.published {
            width += 2; // separator + cloud icon
        }
        if let Some(spinner) = &self.spinner_text {
            width += 1 + spinner.width();
        }

        width
    }

    /// Render this item as a Line, compressing a path-style primary text to fit
    /// `available_width` (the inner content width of the pane) when applicable.
    fn to_line(&self, selected: bool, focused: bool, available_width: u16) -> Line<'_> {
        use unicode_width::UnicodeWidthStr;

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
            let icon_style = if self.plain_icon {
                Style::default()
            } else {
                Style::default().fg(crate::theme::palette().accent)
            };
            spans.push(Span::styled(icon, icon_style));
            spans.push(Span::raw(" "));
        }

        // Add primary text
        let primary_style = if let Some(color) = self.text_color {
            Style::default().fg(color)
        } else {
            Style::default()
        };
        if self.compress_path {
            // Reserve the width consumed by the leading spans (padding, health,
            // icon) and the trailing spans (emphasis, secondary, metadata, …),
            // then give the path whatever width remains in the pane.
            let leading: usize = spans.iter().map(|s| s.content.width()).sum();
            let budget = (available_width as usize)
                .saturating_sub(leading)
                .saturating_sub(self.trailing_width());
            let short = shorten_path(&self.text, budget);
            spans.push(Span::styled(short, primary_style));
        } else {
            spans.push(Span::styled(&self.text, primary_style));
        }

        // Trailing spans begin here. Any span added below must be mirrored in
        // `trailing_width()` so a compressible path reserves the right amount of
        // space; `test_compress_path_fits_with_all_trailing_segments` guards
        // against drift between the two.
        //
        // Add emphasized text if present (bold)
        if let Some(emphasis) = &self.emphasis_text {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(emphasis, Style::default().add_modifier(Modifier::BOLD)));
        }

        // Add secondary text if present
        if let Some(secondary) = &self.secondary_text {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(secondary, Style::default().fg(crate::theme::palette().dim)));
        }

        // Add metadata if present
        if let Some(metadata) = &self.metadata {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(metadata, Style::default().fg(crate::theme::palette().accent)));
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
                        SyncDirection::Push => ("↑", crate::theme::palette().accent),
                        SyncDirection::Pull => ("↓", crate::theme::palette().warning),
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
                Style::default().fg(crate::theme::palette().success).add_modifier(Modifier::BOLD),
            ));
        }

        // Add cloud upload icon if published to server (pushed)
        if self.published {
            spans.push(Span::raw(" "));
            spans.push(Span::styled("☁", Style::default().fg(crate::theme::palette().accent)));
        }

        // Add spinner at the very end
        if let Some(spinner) = &self.spinner_text {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(spinner, Style::default().fg(crate::theme::palette().warning)));
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
            focus_style: Style::default().fg(crate::theme::palette().accent),
            normal_style: Style::default().fg(crate::theme::palette().dim),
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

    /// Whether a non-empty filter query is currently applied to this panel.
    pub fn is_filtered(&self) -> bool {
        !self.filter_query.is_empty()
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
                .filter(|item| {
                    item.text.to_lowercase().contains(&query)
                        || item
                            .emphasis_text
                            .as_ref()
                            .is_some_and(|e| e.to_lowercase().contains(&query))
                })
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

    /// Count the currently active items in the entire list (regardless of filtering).
    /// Avoids the allocation `active_items()` performs when only the count is needed.
    pub fn active_item_count(&self) -> usize {
        self.all_items.iter().filter(|i| i.active).count()
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
                    .filter(|item| {
                        item.text.to_lowercase().contains(&query)
                            || item
                                .emphasis_text
                                .as_ref()
                                .is_some_and(|e| e.to_lowercase().contains(&query))
                    })
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

    /// Update the spinner text of an item identified by its user_data value.
    /// The spinner is rendered at the very end of the item line.
    pub fn update_item_spinner(&mut self, user_data: &str, text: Option<&str>) {
        let mut changed = false;
        for item in &mut self.all_items {
            if item.user_data.as_deref() == Some(user_data) {
                item.spinner_text = text.map(|t| t.to_string());
                changed = true;
                break;
            }
        }
        if changed {
            for item in &mut self.items {
                if item.user_data.as_deref() == Some(user_data) {
                    item.spinner_text = text.map(|t| t.to_string());
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
                item.health = Some(health);
                changed = true;
                break;
            }
        }
        if changed {
            for item in &mut self.items {
                if item.user_data.as_deref() == Some(user_data) {
                    item.health = Some(health);
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
                    let bg_color = Color::Rgb(62, 68, 81); // Light gray-blue highlight (One Dark)
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
    fn test_shorten_path_short_path() {
        let path = "src/main.rs";
        assert_eq!(shorten_path(path, 25), "src/main.rs");
    }

    #[test]
    fn test_shorten_path_exact_length() {
        let path = "src/main.rs"; // 11 characters
        assert_eq!(shorten_path(path, 11), "src/main.rs");
    }

    #[test]
    fn test_shorten_path_long_path() {
        let path = "very/long/path/to/some/deeply/nested/file.rs";
        let result = shorten_path(path, 25);
        assert!(result.len() <= 25);
        assert!(result.ends_with("file.rs"));
    }

    #[test]
    fn test_shorten_path_absolute_path() {
        let path = "/Users/danielcardona/development/codemark/src/browser/tabbed_panel.rs";
        let result = shorten_path(path, 25);
        assert!(result.len() <= 25);
        assert!(result.contains("tabbed_panel.rs") || result.contains("..."));
    }

    #[test]
    fn test_shorten_path_very_long_filename() {
        let path = "src/very_long_filename_that_exceeds_limit.rs";
        let result = shorten_path(path, 25);
        assert!(result.len() <= 25);
    }

    #[test]
    fn test_compressible_path_grows_with_width() {
        // A compressible path item shows more of the path as the pane widens.
        let path = "crates/codemark-tui/src/component/panel.rs";
        let item = PanelItem::new(path).no_health().compressible_path();

        let narrow = item.to_line(false, false, 20).to_string();
        let wide = item.to_line(false, false, 80).to_string();

        // Narrow rendering is compressed; wide rendering shows the full path.
        assert!(narrow.trim().len() < path.len());
        assert!(wide.contains(path));
        assert!(wide.trim_end().len() >= narrow.trim_end().len());
    }

    #[test]
    fn test_shorten_path_multibyte_uses_display_width() {
        // A path that fits in display columns but exceeds byte length must not
        // be truncated (each CJK char is 2 columns but 3 bytes).
        let path = "文档/main.rs"; // 12 display columns, 14 bytes
        assert_eq!(shorten_path(path, 12), path);
    }

    #[test]
    fn test_compress_path_fits_with_all_trailing_segments() {
        // A fully-populated compressible item must never render wider than the
        // available width. If a trailing span is added to `to_line` without a
        // matching entry in `trailing_width`, the path budget is over-estimated
        // and this assertion fails — catching drift between the two sites.
        let mut item = PanelItem::new("crates/codemark-tui/src/component/panel.rs")
            .compressible_path()
            .icon("")
            .emphasis("identifier")
            .secondary_text("secondary")
            .metadata("alice")
            .health(HealthStatus::Healthy)
            .sync_direction(Some(SyncDirection::Push))
            .checkmark(true)
            .published(true);
        item.spinner_text = Some("⠋".to_string());

        // Widths comfortably larger than the fixed trailing content, so the path
        // (not the trailing segments) is the limiting factor.
        for width in [50u16, 65, 80] {
            let line = item.to_line(false, false, width);
            assert!(
                line.width() <= width as usize,
                "rendered width {} exceeded available {}",
                line.width(),
                width
            );
        }
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
