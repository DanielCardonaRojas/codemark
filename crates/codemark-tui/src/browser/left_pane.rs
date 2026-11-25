use crate::browser::{
    ContentTabOrder, FocusArea, LeftPaneSize, SearchBar, SectionConfig, TabbedPanel,
};
use crate::component::Component;
use crate::event::Event;
use codemark_core::storage::db::Database;
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
};

/// Rows taken by the search bar.
const SEARCH_ROWS: u16 = 3;

/// Rows the Content list needs before the Context/Filters panels give up
/// their regular heights and squash to title bars.
const MIN_CONTENT_ROWS: u16 = 10;

/// Rows the active section gets in the stacked (portrait) layout.
///
/// Fixed, and the same for Context, Filters and Content: when the active
/// section instead filled the lists column, each one came out a different
/// height and the column itself grew when focus reached Context or Filters, so
/// moving focus made the whole stack jump. The frame plus eight list rows.
pub const STACKED_SECTION_ROWS: u16 = 10;

/// Render `panel` expanded, or as its title bar when only one row was laid
/// out for it.
fn render_section(panel: &TabbedPanel, area: Rect, buf: &mut Buffer) {
    if area.height < 3 {
        panel.render_collapsed(area, buf);
    } else {
        panel.render(area, buf);
    }
}

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
    /// Create a new left pane with the Content panel tabs in display `order`.
    pub fn new(db: &Database, registry: &rusqlite::Connection, order: &ContentTabOrder) -> Self {
        // The Content panel opens on its first displayed tab, so the Filters
        // panel's Tags/Branches visibility is seeded from that same tab.
        let initial_tab = order.tab_at(0).unwrap_or_default();
        Self {
            search: SearchBar::new(),
            context_panel: TabbedPanel::new_repos_accounts(db, registry),
            filters_panel: TabbedPanel::new_tags_branches(db, initial_tab),
            content_panel: TabbedPanel::new_tours_collections_bookmarks(db, order),
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

    /// The panel that is shown when the left pane is expanded (Half/Full).
    /// This is the last resizable panel that held focus.
    pub fn expanded_focus(&self) -> FocusArea {
        self.last_resizable_focus
    }

    /// Invalidate every child's cached hit-test area.
    ///
    /// Each panel re-records its area when (and only when) it is rendered, so
    /// clearing them first keeps a hidden panel's stale area from capturing a
    /// click meant for the visible one. This must also be called when the left
    /// pane itself isn't rendered (e.g. the right pane is fullscreen), otherwise
    /// its children keep the areas from the previous layout.
    pub fn invalidate_areas(&self) {
        self.search.invalidate_area();
        self.context_panel.invalidate_area();
        self.filters_panel.invalidate_area();
        self.content_panel.invalidate_area();
    }

    /// Render the left pane.
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        self.render_with(area, buf, false);
    }

    /// Render the left pane, squashing the unfocused Context/Filters panels
    /// to one-line title bars when `force_squash` (the portrait layout) or
    /// when the pane is too short to give them their regular heights.
    pub fn render_with(&self, area: Rect, buf: &mut Buffer, force_squash: bool) {
        // Invalidate every child's cached hit-test area up front; each panel
        // re-records its area when (and only when) it is rendered below. When
        // expanded (Half/Full) only the focused panel is drawn, so this keeps a
        // hidden panel's stale area from capturing a click meant for the visible
        // one (e.g. a bookmark click landing on the Filters panel's old tab row).
        self.invalidate_areas();

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
                        self.render_all(area, buf, force_squash);
                    }
                }
            }
            LeftPaneSize::Regular => {
                self.render_all(area, buf, force_squash);
            }
        }
    }

    /// Render all panels in the left pane (regular mode).
    ///
    /// When `force_squash` (the portrait layout) or when the pane can't fit
    /// every section at its regular height, the pane squashes like lazygit's
    /// side panels: the active section (the last focused of Context, Filters
    /// and Content; Content while the preview has focus) fills the pane and
    /// the others collapse to one-line title bars, as does the search bar
    /// while it isn't focused.
    fn render_all(&self, area: Rect, buf: &mut Buffer, force_squash: bool) {
        let regular_height =
            SEARCH_ROWS + self.context_config.min + self.filters_config.min + MIN_CONTENT_ROWS;
        if force_squash {
            self.render_squashed(area, buf, Some(STACKED_SECTION_ROWS));
            return;
        }
        if area.height < regular_height {
            self.render_squashed(area, buf, None);
            return;
        }

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
                Constraint::Length(SEARCH_ROWS), // Search bar
                Constraint::Length(p1_height),   // Context panel (Repos/Accounts)
                Constraint::Length(p2_height),   // Filters panel (Tags/Branches)
                Constraint::Min(0),              // Content panel (Tours/Collections/Bookmarks)
            ])
            .split(area);

        self.search.render(chunks[0], buf);
        self.context_panel.render(chunks[1], buf);
        self.filters_panel.render(chunks[2], buf);
        self.content_panel.render(chunks[3], buf);
    }

    /// Height of the lists column in the stacked (portrait) layout: the search
    /// bar, the active section at [`STACKED_SECTION_ROWS`], and a title bar for
    /// each of the other two sections.
    pub fn stacked_height(&self) -> u16 {
        self.search_rows_squashed() + STACKED_SECTION_ROWS + 2
    }

    /// The search bar's height in the squashed layout: a title bar unless it
    /// has focus.
    fn search_rows_squashed(&self) -> u16 {
        if self.search.is_focused() { SEARCH_ROWS } else { 1 }
    }

    /// The squashed layout of [`Self::render_all`].
    ///
    /// `active_rows` fixes the active section's height (the portrait layout);
    /// `None` lets it fill the pane, for a landscape pane too short to fit
    /// every section, where the column is the full terminal height.
    fn render_squashed(&self, area: Rect, buf: &mut Buffer, active_rows: Option<u16>) {
        // The search bar drives the Content list, so typing a query shows it.
        let active = match self.last_resizable_focus {
            _ if self.search.is_focused() => FocusArea::ContentPanel,
            FocusArea::ContextPanel => FocusArea::ContextPanel,
            FocusArea::FiltersPanel => FocusArea::FiltersPanel,
            _ => FocusArea::ContentPanel,
        };
        let height = |section: FocusArea| match (section == active, active_rows) {
            (true, Some(rows)) => Constraint::Length(rows),
            (true, None) => Constraint::Min(0),
            (false, _) => Constraint::Length(1),
        };
        let search_rows = self.search_rows_squashed();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(search_rows),
                height(FocusArea::ContextPanel),
                height(FocusArea::FiltersPanel),
                height(FocusArea::ContentPanel),
                // With a fixed active height, absorbs any spare rows so they
                // never stretch a title bar. Otherwise the active section is
                // the one filling the pane, so this must take nothing.
                if active_rows.is_some() { Constraint::Min(0) } else { Constraint::Length(0) },
            ])
            .split(area);

        if search_rows < SEARCH_ROWS {
            self.search.render_collapsed(chunks[0], buf);
        } else {
            self.search.render(chunks[0], buf);
        }
        render_section(&self.context_panel, chunks[1], buf);
        render_section(&self.filters_panel, chunks[2], buf);
        render_section(&self.content_panel, chunks[3], buf);
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
