use crate::browser::{ContentTab, FocusArea, LeftPaneSize, SearchBar, SectionConfig, TabbedPanel};
use crate::component::Component;
use crate::event::Event;
use codemark_core::storage::db::Database;
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
};

/// Left sidebar containing search and three tabbed panels.
pub struct LeftPane {
    /// Search bar component
    pub search: SearchBar,
    /// Context panel (Repos/Owners/Auth)
    pub context_panel: TabbedPanel,
    /// Filters panel (Tags/Branches)
    pub filters_panel: TabbedPanel,
    /// Content panel (Bookmarks/Collections/Tours)
    pub content_panel: TabbedPanel,
    /// Section height configurations
    pub context_config: SectionConfig,
    pub filters_config: SectionConfig,
    /// Current resize mode for the left pane
    resize_mode: LeftPaneSize,
    /// The last resizable panel that was focused (for rendering in Half/Full mode)
    last_resizable_focus: FocusArea,
}

impl LeftPane {
    /// Create a new left pane.
    pub fn new(db: &Database, registry: &rusqlite::Connection) -> Self {
        Self {
            search: SearchBar::new(),
            context_panel: TabbedPanel::new_repos_accounts(db, registry),
            filters_panel: TabbedPanel::new_tags_branches(db, ContentTab::Bookmarks),
            content_panel: TabbedPanel::new_tours_collections_bookmarks(db),
            context_config: SectionConfig::new(7, 9),
            filters_config: SectionConfig::new(7, 9),
            resize_mode: LeftPaneSize::Regular,
            last_resizable_focus: FocusArea::ContentPanel,
        }
    }

    /// Set the resize mode for the left pane.
    pub fn set_resize_mode(&mut self, mode: LeftPaneSize) {
        self.resize_mode = mode;
    }

    /// Get the current resize mode.
    pub fn resize_mode(&self) -> LeftPaneSize {
        self.resize_mode
    }

    /// Set the focused area.
    pub fn set_focused_area(&mut self, focus: FocusArea) {
        // Only update the last resizable focus if this is a resizable panel
        if focus.is_resizable() {
            self.last_resizable_focus = focus;
        }
    }

    /// Render the left pane.
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        // In Half or Full mode, only render the focused panel and search bar
        // The focused panel takes all available height
        match self.resize_mode {
            LeftPaneSize::Half | LeftPaneSize::Full => {
                match self.last_resizable_focus {
                    FocusArea::ContextPanel => {
                        // Repos/Owners/Auth are filtered, not searched, so the search
                        // bar is hidden and the panel takes the full height.
                        self.context_panel.render(area, buf);
                    }
                    FocusArea::FiltersPanel => {
                        // Tags/Branches are filtered, not searched, so the search bar
                        // is hidden and the panel takes the full height.
                        self.filters_panel.render(area, buf);
                    }
                    FocusArea::ContentPanel => {
                        // Render search and content_panel only
                        let chunks = Layout::default()
                            .direction(Direction::Vertical)
                            .constraints([Constraint::Length(3), Constraint::Min(0)])
                            .split(area);
                        self.search.render(chunks[0], buf);
                        self.content_panel.render(chunks[1], buf);
                    }
                    _ => {
                        // Fallback: render everything normally
                        self.render_all(area, buf);
                    }
                }
            }
            LeftPaneSize::Regular => {
                // Render all panels normally
                self.render_all(area, buf);
            }
        }
    }

    /// Render all panels in the left pane (regular mode).
    fn render_all(&self, area: Rect, buf: &mut Buffer) {
        // Calculate heights based on focus
        let p1_height = if self.context_panel.focused {
            self.context_config.max
        } else {
            self.context_config.min
        };

        let p2_height = if self.filters_panel.focused {
            self.filters_config.max
        } else {
            self.filters_config.min
        };

        // Split vertically: search (3 rows), context_panel, filters_panel, content_panel (takes the rest)
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),         // Search bar
                Constraint::Length(p1_height), // Context panel (Repos/Accounts)
                Constraint::Length(p2_height), // Filters panel (Tags/Branches)
                Constraint::Min(0),            // Content panel (Tours/Collections/Bookmarks)
            ])
            .split(area);

        // Render search
        self.search.render(chunks[0], buf);

        // Render the context panel
        self.context_panel.render(chunks[1], buf);

        // Render the filters panel
        self.filters_panel.render(chunks[2], buf);

        // Render the content panel
        self.content_panel.render(chunks[3], buf);
    }

    /// Handle an event.
    pub fn handle_event(&mut self, event: &Event) -> bool {
        if matches!(event, Event::Mouse(_)) {
            // For mouse events, check ALL panels to allow scrolling any hovered pane.
            // Use bitwise OR to avoid short-circuiting.
            let h1 = self.search.handle_event(event);
            let h2 = self.context_panel.handle_event(event);
            let h3 = self.filters_panel.handle_event(event);
            let h4 = self.content_panel.handle_event(event);
            h1 || h2 || h3 || h4
        } else {
            // Keyboard events can short-circuit for efficiency.
            self.search.handle_event(event)
                || self.context_panel.handle_event(event)
                || self.filters_panel.handle_event(event)
                || self.content_panel.handle_event(event)
        }
    }
}
