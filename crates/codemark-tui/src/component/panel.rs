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
#[derive(Debug, Clone)]
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
    /// Custom style for selected item
    selected_style: Style,
    /// Whether to show a scrollbar
    show_scrollbar: bool,
    /// Whether multiple items can be active at once
    multi_select: bool,
    /// The type of border to render
    border_type: BorderType,
    /// Last rendered area for mouse handling
    last_area: Cell<Rect>,
}

/// Health status indicator for an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    /// Healthy - green
    Healthy,
    /// Warning - yellow
    Warning,
    /// Error/Unhealthy - red
    Error,
    /// Unknown/Gray - gray
    Unknown,
}

impl HealthStatus {
    /// Get the color for this health status.
    fn color(&self) -> Color {
        match self {
            HealthStatus::Healthy => Color::Green,
            HealthStatus::Warning => Color::Yellow,
            HealthStatus::Error => Color::Red,
            HealthStatus::Unknown => Color::DarkGray,
        }
    }

    /// Get the symbol for this health status.
    fn symbol(&self) -> &'static str {
        "●"
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
            health: Some(HealthStatus::Unknown),
            text_color: None,
            checkmark: false,
            sync_direction: None,
            active: false,
        }
    }

    /// Set whether this item is active.
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// Get the item text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Set the secondary text.
    pub fn secondary_text(mut self, text: impl Into<String>) -> Self {
        self.secondary_text = Some(text.into());
        self
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
    fn to_line(&self, selected: bool, focused: bool) -> Line {
        let mut spans = Vec::new();

        // Add padding prefix for alignment
        spans.push(Span::raw("  "));

        // Add health status indicator if present
        if let Some(health) = self.health {
            spans.push(Span::styled(
                health.symbol(),
                Style::default().fg(health.color()),
            ));
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
            spans.push(Span::styled(
                secondary,
                Style::default().fg(Color::DarkGray),
            ));
        }

        // Add metadata if present
        if let Some(metadata) = &self.metadata {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                metadata,
                Style::default().fg(Color::Cyan),
            ));
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

        let line = Line::from(spans);
        if selected && focused {
            line.bold()
        } else {
            line
        }
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
            selected_style: Style::default().bg(Color::Blue).fg(Color::White),
            show_scrollbar: true,
            multi_select: false,
            border_type: BorderType::Rounded,
            last_area: Cell::new(Rect::default()),
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
        self.filter_query = query.to_string();
        self.apply_filter();
    }

    /// Apply the current filter to all_items.
    fn apply_filter(&mut self) {
        if self.filter_query.is_empty() {
            self.items = self.all_items.clone();
        } else {
            let query = self.filter_query.to_lowercase();
            self.items = self.all_items
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
        
        // Reset scroll offset on filter change by creating a new state but preserving selection
        let selected = state.selected();
        *state = ListState::default();
        state.select(selected);
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

    /// Set the selected item by index.
    pub fn set_selected(&mut self, idx: usize) {
        if idx < self.items.len() {
            self.list_state.borrow_mut().select(Some(idx));
        }
    }

    /// Select the next item.
    pub fn select_next(&mut self) {
        if self.items.is_empty() {
            return;
        }
        let mut state = self.list_state.borrow_mut();
        let next = state.selected().map_or(0, |i| i.saturating_add(1) % self.items.len());
        state.select(Some(next));
    }

    /// Select the previous item.
    pub fn select_previous(&mut self) {
        if self.items.is_empty() {
            return;
        }
        let mut state = self.list_state.borrow_mut();
        let prev = state.selected().map_or(
            self.items.len() - 1,
            |i| if i == 0 { self.items.len() - 1 } else { i - 1 },
        );
        state.select(Some(prev));
    }

    /// Set the currently selected item as active and deactivate all others (or toggle in multi-select mode).
    pub fn activate_selected(&mut self) {
        if let Some(idx) = self.selected_index() {
            if let Some(item) = self.items.get(idx) {
                let text = item.text.clone();
                
                if self.multi_select {
                    // Toggle in all_items
                    if let Some(item) = self.all_items.iter_mut().find(|i| i.text == text) {
                        item.active = !item.active;
                    }
                } else {
                    // Determine if the target item is already active
                    let was_active = self.all_items.iter()
                        .find(|i| i.text == text)
                        .map(|i| i.active)
                        .unwrap_or(false);

                    // Deactivate all
                    for item in &mut self.all_items {
                        item.active = false;
                    }

                    // If it wasn't active before, activate it now.
                    // If it was active, it remains inactive (toggled off).
                    if !was_active {
                        if let Some(item) = self.all_items.iter_mut().find(|i| i.text == text) {
                            item.active = true;
                        }
                    }
                }

                // Sync items with all_items (preserving current filter and list state)
                let query = self.filter_query.to_lowercase();
                if query.is_empty() {
                    self.items = self.all_items.clone();
                } else {
                    self.items = self.all_items
                        .iter()
                        .filter(|item| item.text.to_lowercase().contains(&query))
                        .cloned()
                        .collect();
                }
            }
        }
    }

    /// Clear all items from the panel.
    pub fn clear(&mut self) {
        self.items.clear();
        self.all_items.clear();
        self.list_state.borrow_mut().select(None);
        self.filter_query.clear();
    }

    /// Update the items in the panel, preserving selection if possible.
    pub fn set_items(&mut self, items: Vec<PanelItem>) {
        let selected_text = self.list_state.borrow().selected().and_then(|idx| self.items.get(idx).map(|i| i.text.clone()));
        self.all_items = items;
        self.apply_filter();

        let mut state = self.list_state.borrow_mut();
        if !self.items.is_empty() {
            // Try to restore the selection
            let new_sel = if let Some(text) = selected_text {
                self.items
                    .iter()
                    .position(|item| item.text == text)
                    .or(Some(0))
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
        let inner = if self.bordered {
            area.inner(Margin::new(1, 1))
        } else {
            area
        };

        let height = inner.height as usize;

        // Build all items (Ratatui List handles scrolling internally via ListState)
        let list_items: Vec<ListItem> = self.items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let is_selected = self.list_state.borrow().selected() == Some(i);
                let line = item.to_line(is_selected, self.focused);
                let mut list_item = ListItem::new(line);
                
                if is_selected {
                    let bg_color = if self.focused {
                        Color::Rgb(50, 50, 50)  // Light gray highlight for focused
                    } else {
                        Color::Rgb(35, 35, 35)  // Darker gray for unfocused
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
            .border_style(if self.focused {
                self.focus_style
            } else {
                self.normal_style
            });

        // Render the list using StatefulWidget for native scrolling
        let list = List::new(list_items);

        let mut state = self.list_state.borrow_mut();
        if self.bordered {
            StatefulWidget::render(list.block(block), area, buf, &mut *state);
        } else {
            StatefulWidget::render(list, area, buf, &mut *state);
        };

        // Render scrollbar if needed
        if self.show_scrollbar && !self.items.is_empty() && self.items.len() > height {
            let scrollbar_style = if self.focused {
                self.focus_style
            } else {
                self.normal_style
            };

            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(None)
                .thumb_symbol("┃")
                .style(scrollbar_style);

            let mut scrollbar_state = ScrollbarState::new(self.items.len())
                .position(state.offset());

            let scrollbar_area = if self.bordered {
                Rect {
                    x: area.right() - 1,
                    y: area.top() + 1,
                    width: 1,
                    height: area.height.saturating_sub(2),
                }
            } else {
                Rect {
                    x: area.right() - 1,
                    y: area.top(),
                    width: 1,
                    height: area.height,
                }
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
        if !self.focused {
            return false;
        }

        match event {
            Event::Key(key) => match key.code {
                ratatui::crossterm::event::KeyCode::Down | ratatui::crossterm::event::KeyCode::Char('j') => {
                    self.select_next();
                    true
                }
                ratatui::crossterm::event::KeyCode::Up | ratatui::crossterm::event::KeyCode::Char('k') => {
                    self.select_previous();
                    true
                }
                ratatui::crossterm::event::KeyCode::Char(' ') => {
                    self.activate_selected();
                    true
                }
                _ => false,
            },
            Event::Mouse(mouse) => {
                match mouse.kind {
                    ratatui::crossterm::event::MouseEventKind::Down(button) => {
                        if button == ratatui::crossterm::event::MouseButton::Left {
                            let area = self.last_area.get();
                            if mouse.column >= area.x
                                && mouse.column < area.x + area.width
                                && mouse.row >= area.y
                                && mouse.row < area.y + area.height
                            {
                                // Calculate inner area to get correct item index
                                let inner = if self.bordered {
                                    area.inner(Margin::new(1, 1))
                                } else {
                                    area
                                };

                                if mouse.column >= inner.x
                                    && mouse.column < inner.x + inner.width
                                    && mouse.row >= inner.y
                                    && mouse.row < inner.y + inner.height
                                {
                                    let relative_row = mouse.row.saturating_sub(inner.y) as usize;
                                    let item_idx = relative_row + self.list_state.borrow().offset();
                                    if item_idx < self.items.len() {
                                        self.list_state.borrow_mut().select(Some(item_idx));
                                        return true;
                                    }
                                }
                                return true;
                            }
                        }
                        false
                    }
                    ratatui::crossterm::event::MouseEventKind::ScrollDown => {
                        self.select_next();
                        true
                    }
                    ratatui::crossterm::event::MouseEventKind::ScrollUp => {
                        self.select_previous();
                        true
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
    fn test_panel_item_creation() {
        let item = PanelItem::new("test").secondary_text("secondary").metadata("meta");
        assert_eq!(item.text, "test");
        assert_eq!(item.secondary_text, Some("secondary".to_string()));
        assert_eq!(item.metadata, Some("meta".to_string()));
    }

    #[test]
    fn test_panel_navigation() {
        let mut panel = Panel::new("Test")
            .items(vec![
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
    fn test_panel_set_items() {
        let mut panel = Panel::new("Test")
            .items(vec![
                PanelItem::new("item1"),
                PanelItem::new("item2"),
            ]);

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
