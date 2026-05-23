use crate::browser::{Panel3Tab, SearchBar, SectionConfig, TabbedPanel};
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
}

impl LeftPane {
    /// Create a new left pane.
    pub fn new(db: &Database, registry: &rusqlite::Connection) -> Self {
        Self {
            search: SearchBar::new(),
            panel1: TabbedPanel::new_repos_accounts(db, registry),
            panel2: TabbedPanel::new_tags_branches(db, Panel3Tab::Bookmarks),
            panel3: TabbedPanel::new_tours_collections_bookmarks(db),
            panel1_config: SectionConfig::new(4, 6),
            panel2_config: SectionConfig::new(4, 8),
        }
    }

    /// Render the left pane.
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
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
