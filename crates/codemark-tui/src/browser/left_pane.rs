use crate::browser::{FocusArea, LeftPaneSize, Panel3Tab, SearchBar, SectionConfig, TabbedPanel};
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
    /// First tabbed panel (Repos/Accounts)
    pub panel1: TabbedPanel,
    /// Second tabbed panel (Tags/Branches)
    pub panel2: TabbedPanel,
    /// Third tabbed panel (Tours/Collections/Bookmarks)
    pub panel3: TabbedPanel,
    /// Section height configurations
    pub panel1_config: SectionConfig,
    pub panel2_config: SectionConfig,
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
            panel1: TabbedPanel::new_repos_accounts(db, registry),
            panel2: TabbedPanel::new_tags_branches(db, Panel3Tab::Bookmarks),
            panel3: TabbedPanel::new_tours_collections_bookmarks(db),
            panel1_config: SectionConfig::new(7, 9),
            panel2_config: SectionConfig::new(7, 9),
            resize_mode: LeftPaneSize::Regular,
            last_resizable_focus: FocusArea::Panel3,
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
                    FocusArea::Panel1 => {
                        // Render search and panel1 only
                        let chunks = Layout::default()
                            .direction(Direction::Vertical)
                            .constraints([Constraint::Length(3), Constraint::Min(0)])
                            .split(area);
                        self.search.render(chunks[0], buf);
                        self.panel1.render(chunks[1], buf);
                    }
                    FocusArea::Panel2 => {
                        // Render search and panel2 only
                        let chunks = Layout::default()
                            .direction(Direction::Vertical)
                            .constraints([Constraint::Length(3), Constraint::Min(0)])
                            .split(area);
                        self.search.render(chunks[0], buf);
                        self.panel2.render(chunks[1], buf);
                    }
                    FocusArea::Panel3 => {
                        // Render search and panel3 only
                        let chunks = Layout::default()
                            .direction(Direction::Vertical)
                            .constraints([Constraint::Length(3), Constraint::Min(0)])
                            .split(area);
                        self.search.render(chunks[0], buf);
                        self.panel3.render(chunks[1], buf);
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
        let p1_height =
            if self.panel1.focused { self.panel1_config.max } else { self.panel1_config.min };

        let p2_height =
            if self.panel2.focused { self.panel2_config.max } else { self.panel2_config.min };

        // Split vertically: search (3 rows), panel1, panel2, panel3 (takes the rest)
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),         // Search bar
                Constraint::Length(p1_height), // Panel 1 (Repos/Accounts)
                Constraint::Length(p2_height), // Panel 2 (Tags/Branches)
                Constraint::Min(0),            // Panel 3 (Tours/Collections/Bookmarks)
            ])
            .split(area);

        // Render search
        self.search.render(chunks[0], buf);

        // Render panel 1
        self.panel1.render(chunks[1], buf);

        // Render panel 2
        self.panel2.render(chunks[2], buf);

        // Render panel 3
        self.panel3.render(chunks[3], buf);
    }

    /// Handle an event.
    pub fn handle_event(&mut self, event: &Event) -> bool {
        if matches!(event, Event::Mouse(_)) {
            // For mouse events, check ALL panels to allow scrolling any hovered pane.
            // Use bitwise OR to avoid short-circuiting.
            let h1 = self.search.handle_event(event);
            let h2 = self.panel1.handle_event(event);
            let h3 = self.panel2.handle_event(event);
            let h4 = self.panel3.handle_event(event);
            h1 || h2 || h3 || h4
        } else {
            // Keyboard events can short-circuit for efficiency.
            self.search.handle_event(event)
                || self.panel1.handle_event(event)
                || self.panel2.handle_event(event)
                || self.panel3.handle_event(event)
        }
    }
}
