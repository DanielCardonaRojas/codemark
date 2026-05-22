use crate::browser::{Panel3Tab, Tab, TabContent, TabSelection, tabs::BORDER_EXTENSION};
use crate::component::{CodePreview, HealthStatus, MarkdownPanel, Panel, PanelItem};
use crate::event::Event;
use codemark_core::engine::bookmark::{BookmarkFilter, BookmarkHealth};
use codemark_core::parser::languages::Language;
use codemark_core::query::classifier::get_node_icon;
use codemark_core::query::summarizer;
use codemark_core::storage::db::Database;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::Widget,
};

/// A tabbed panel component with multiple content panels.
pub struct TabbedPanel {
    /// Tab selection
    pub tabs: TabSelection,
    /// Content panels for each tab
    pub panels: Vec<TabContent>,
    /// Currently focused
    pub focused: bool,
    /// Last rendered area
    pub last_area: std::cell::Cell<Rect>,
    /// Pending selection change (bookmark ID) to be retrieved after event handling
    pub pending_selection_change: std::cell::Cell<Option<String>>,
}

/// Shorten a file path to fit within a maximum width, prioritizing the last path components.
///
/// If the path exceeds `max_width`, it will be truncated to show the last components
/// with a "../" prefix. For example, "/very/long/path/to/file.rs" with max_width=25
/// might become "../path/to/file.rs".
fn shorten_path(path: &str, max_width: usize) -> String {
    if path.len() <= max_width {
        return path.to_string();
    }

    // Try to find a good breaking point by working backwards from the end
    let components: Vec<&str> = path.split('/').collect();
    let mut result = String::new();
    let mut total_len = 0;

    // Start from the last component and work backwards
    for (_i, component) in components.iter().enumerate().rev() {
        let component_len = component.len();
        let separator_len = if result.is_empty() { 0 } else { 1 }; // '/' separator

        // Check if adding this component would exceed the limit
        if total_len + component_len + separator_len > max_width {
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
        total_len = result.len();
    }

    // If we still couldn't fit anything useful, use the filename only
    if result.is_empty()
        && let Some(filename) = components.last()
    {
        if filename.len() <= max_width {
            return filename.to_string();
        } else {
            // Truncate the filename
            return format!("...{}", &filename[filename.len().saturating_sub(max_width - 3)..]);
        }
    }

    result
}

impl TabbedPanel {
    /// Get the step preview for modification.
    pub fn get_step_preview_mut(&mut self) -> Option<&mut CodePreview> {
        match self.panels.get_mut(0) {
            Some(TabContent::Preview(p)) => Some(p),
            _ => None,
        }
    }

    /// Get the query preview for modification.
    pub fn get_query_preview_mut(&mut self) -> Option<&mut CodePreview> {
        match self.panels.get_mut(2) {
            Some(TabContent::Preview(p)) => Some(p),
            _ => None,
        }
    }

    /// Get the currently active markdown panel for modification.
    pub fn get_markdown_mut(&mut self) -> Option<&mut MarkdownPanel> {
        for panel in &mut self.panels {
            if let TabContent::Markdown(p) = panel {
                return Some(p);
            }
        }
        None
    }

    /// Get the currently active panel (immutable).
    pub fn active_panel(&self) -> Option<&Panel> {
        let active_index = self.tabs.selected_index();
        match self.panels.get(active_index) {
            Some(TabContent::List(p)) => Some(p),
            _ => None,
        }
    }

    /// Get the currently active panel for modification.
    pub fn active_panel_mut(&mut self) -> Option<&mut Panel> {
        let active_index = self.tabs.selected_index();
        match self.panels.get_mut(active_index) {
            Some(TabContent::List(p)) => Some(p),
            _ => None,
        }
    }

    /// Get a specific list panel by index for modification.
    pub fn get_list_panel_mut(&mut self, index: usize) -> Option<&mut Panel> {
        match self.panels.get_mut(index) {
            Some(TabContent::List(p)) => Some(p),
            _ => None,
        }
    }

    /// Build the list of repository items.
    pub fn build_repo_items(db: &Database, registry: &rusqlite::Connection) -> Vec<PanelItem> {
        use codemark_core::storage::registry;
        if let Ok(repos) = registry::list_repos(registry) {
            let active_root = db
                .path()
                .parent() // .codemark/
                .and_then(|p| p.parent()) // repo_root/
                .map(|p| p.to_path_buf())
                .unwrap_or_default();

            repos
                .into_iter()
                .map(|repo| {
                    let is_active = repo.repo_root == active_root;
                    PanelItem::new(repo.repo_name)
                        .secondary_text(repo.repo_owner)
                        .user_data(repo.repo_root.to_string_lossy().to_string())
                        .active(is_active)
                        .no_health()
                })
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Build tags and branches items.
    pub fn build_tags_branches_items(
        db: &Database,
        active_tab: Panel3Tab,
    ) -> (Vec<PanelItem>, Vec<PanelItem>) {
        let tags_result = match active_tab {
            Panel3Tab::Bookmarks => db.list_bookmark_tags(),
            Panel3Tab::Collections | Panel3Tab::Tours => db.list_collection_tags(),
        };

        let tags = match tags_result {
            Ok(tags) if !tags.is_empty() => tags
                .into_iter()
                .map(|tag| {
                    PanelItem::new(format!("#{tag}")).user_data(tag).no_health().color(Color::Cyan)
                })
                .collect(),
            Ok(_) => vec![PanelItem::new("No tags found").no_health().color(Color::DarkGray)],
            Err(e) => vec![PanelItem::new(format!("Error: {e}")).no_health().color(Color::Red)],
        };

        let branches = match db.list_all_branches() {
            Ok(branches) if !branches.is_empty() => branches
                .into_iter()
                .map(|branch| PanelItem::new(branch).health(HealthStatus::Branch))
                .collect(),
            Ok(_) => vec![PanelItem::new("No branches found").no_health().color(Color::DarkGray)],
            Err(e) => vec![PanelItem::new(format!("Error: {e}")).no_health().color(Color::Red)],
        };

        (tags, branches)
    }

    /// Build tours, collections, and bookmarks items.
    pub fn build_panel3_items(db: &Database) -> (Vec<PanelItem>, Vec<PanelItem>, Vec<PanelItem>) {
        let mut collections_items = Vec::new();
        let mut tours_items = Vec::new();

        if let Ok(collections) = db.list_collections() {
            for (c, count) in collections {
                let health = match c.health {
                    Some(h) => match h {
                        codemark_core::engine::bookmark::CollectionHealth::Active => {
                            HealthStatus::Healthy
                        }
                        codemark_core::engine::bookmark::CollectionHealth::Drifted => {
                            HealthStatus::Warning
                        }
                        codemark_core::engine::bookmark::CollectionHealth::Stale => {
                            HealthStatus::Error
                        }
                    },
                    None => HealthStatus::Unknown,
                };

                let is_published = c.published_at.is_some();
                let item = PanelItem::new(c.name)
                    .secondary_text(c.created_branch.unwrap_or_else(|| "main".to_string()))
                    .metadata(format!("{count} steps"))
                    .health(health)
                    .published(is_published)
                    .user_data(c.id);

                collections_items.push(item.clone());
                if is_published {
                    tours_items.push(item);
                }
            }
        }

        let bookmarks = match db.list_bookmarks(&BookmarkFilter::default()) {
            Ok(bookmarks) => bookmarks
                .into_iter()
                .map(|bm| {
                    // Get the best resolution for preview to determine health status
                    let health = db
                        .get_preview_resolution(&bm.id)
                        .ok()
                        .flatten()
                        .map(|res| match res.health {
                            BookmarkHealth::Active => HealthStatus::Healthy,
                            BookmarkHealth::Drifted => HealthStatus::Warning,
                            BookmarkHealth::Stale | BookmarkHealth::Archived => HealthStatus::Error,
                        })
                        .unwrap_or(HealthStatus::Unknown);

                    // Try to get a summary from the query for better display
                    let summary_info = bm
                        .language
                        .parse::<Language>()
                        .ok()
                        .and_then(|lang| summarizer::summarize_query(&bm.query, Some(lang)).ok());

                    let summary = summary_info
                        .as_ref()
                        .and_then(|s| s.format())
                        .unwrap_or_else(|| bm.query.clone());

                    let icon = summary_info
                        .as_ref()
                        .map(|s| get_node_icon(&s.label))
                        .unwrap_or("󰈙");

                    // Shrink the file path to prioritize last path components
                    let short_path = shorten_path(&bm.file_path, 25);

                    // Format: short_file_path query_summary
                    let display_text = format!("{} {}", short_path, summary);

                    PanelItem::new(display_text)
                        .metadata(bm.created_by.unwrap_or_default())
                        .health(health)
                        .icon(icon)
                        .user_data(bm.id)
                })
                .collect(),
            Err(_) => Vec::new(),
        };

        (tours_items, collections_items, bookmarks)
    }

    /// Create panel 1 with Repos/Accounts tabs.
    pub fn new_repos_accounts(db: &Database, registry: &rusqlite::Connection) -> Self {
        let repos_items = TabbedPanel::build_repo_items(db, registry);
        let mut repos_panel = Panel::new("").bordered(false);
        repos_panel = repos_panel.items(repos_items);

        let accounts = Panel::new("")
            .items(vec![
                PanelItem::new("GitHub").no_health(),
                PanelItem::new("GitLab").no_health(),
                PanelItem::new("Bitbucket").no_health(),
            ])
            .bordered(false);

        let tabs = TabSelection::new(vec![Tab::new("Repos"), Tab::new("Accounts")]);

        Self {
            tabs,
            panels: vec![TabContent::List(repos_panel), TabContent::List(accounts)],
            focused: false,
            last_area: std::cell::Cell::new(Rect::default()),
            pending_selection_change: std::cell::Cell::new(None),
        }
    }

    /// Create panel 2 with Tags/Branches tabs.
    pub fn new_tags_branches(db: &Database, active_tab: Panel3Tab) -> Self {
        let (tags_items, branches_items) = TabbedPanel::build_tags_branches_items(db, active_tab);
        let tags_panel = Panel::new("").bordered(false).multi_select(true).items(tags_items);
        let branches_panel =
            Panel::new("").bordered(false).multi_select(true).items(branches_items);

        let tabs = TabSelection::new(vec![Tab::new("Tags"), Tab::new("Branches")]);

        Self {
            tabs,
            panels: vec![TabContent::List(tags_panel), TabContent::List(branches_panel)],
            focused: false,
            last_area: std::cell::Cell::new(Rect::default()),
            pending_selection_change: std::cell::Cell::new(None),
        }
    }

    /// Create panel 3 with Bookmarks/Collections/Tours tabs.
    pub fn new_tours_collections_bookmarks(db: &Database) -> Self {
        let (tours_items, collections_items, bookmarks_items) = TabbedPanel::build_panel3_items(db);
        let tours_panel = Panel::new("").bordered(false).items(tours_items);
        let collections_panel = Panel::new("").bordered(false).items(collections_items);
        let bookmarks_panel = Panel::new("").bordered(false).items(bookmarks_items);

        let tabs = TabSelection::new(vec![
            Tab::new("Bookmarks"),
            Tab::new("Collections"),
            Tab::new("Tours"),
        ]);

        Self {
            tabs,
            panels: vec![
                TabContent::List(bookmarks_panel),
                TabContent::List(collections_panel),
                TabContent::List(tours_panel),
            ],
            focused: false,
            last_area: std::cell::Cell::new(Rect::default()),
            pending_selection_change: std::cell::Cell::new(None),
        }
    }

    /// Create steps/info/query tabs for right pane.
    pub fn new_steps_info(_db: &Database) -> Self {
        use crate::component::CodePreview;
        let fixture_path = "tests/fixtures/rust/api_client.rs";
        let code = std::fs::read_to_string(fixture_path).unwrap_or_else(|_| {
            "Error: Could not load fixture tests/fixtures/rust/api_client.rs".to_string()
        });

        let mut preview = CodePreview::new(code, "rs");
        preview.jump_to_line(49); // Jump to line 50 (0-indexed)

        let info = MarkdownPanel::new();

        let query_preview = CodePreview::new("(node) @cap", "scm");

        let tabs = TabSelection::new(vec![Tab::new("Steps"), Tab::new("Info"), Tab::new("Query")]);

        Self {
            tabs,
            panels: vec![
                TabContent::Preview(preview),
                TabContent::Markdown(info),
                TabContent::Preview(query_preview),
            ],
            focused: false,
            last_area: std::cell::Cell::new(Rect::default()),
            pending_selection_change: std::cell::Cell::new(None),
        }
    }

    /// Get the last rendered area.
    pub fn last_area(&self) -> Rect {
        self.last_area.get()
    }

    /// Render the tabbed panel.
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        self.last_area.set(area);
        let tab_titles = self.tabs.render_as_titles(self.focused);

        // Render outer border for the entire panel area with inline tabs
        let border_style = if self.focused {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let block = ratatui::widgets::Block::bordered()
            .border_type(ratatui::widgets::BorderType::Rounded)
            .border_style(border_style)
            .title(tab_titles)
            .title_style(border_style)
            .title_alignment(ratatui::layout::Alignment::Left);

        let inner = block.inner(area);
        block.render(area, buf);

        // Extend the top border line after the ╭ character
        for i in 1..=BORDER_EXTENSION {
            let x = area.left() + i;
            let y = area.top();
            if x < area.right()
                && let Some(cell) = buf.cell_mut((x, y))
            {
                cell.set_char('─');
                cell.set_style(border_style);
            }
        }

        // Render active panel content (full inner area, no separate tab row)
        let active_index = self.tabs.selected_index();
        if let Some(panel) = self.panels.get(active_index) {
            panel.render(inner, buf);
        }

        // Render selection indicator on bottom border
        if let Some(TabContent::List(panel)) = self.panels.get(active_index)
            && !panel.is_empty()
        {
            let current = panel.selected_index().map_or(0, |i| i + 1);
            let total = panel.len();
            let indicator = format!(" {current} of {total} ");
            let indicator_width = indicator.len() as u16;

            // Position on bottom border, aligned right (shift left by 1 to avoid corner)
            let x = area.right().saturating_sub(indicator_width + 1);
            let y = area.bottom() - 1;

            if x > area.left() {
                let indicator_style = if self.focused {
                    Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::DarkGray)
                };

                // Render the indicator by modifying the border cells
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

    /// Handle an event.
    /// Returns true if event was handled.
    pub fn handle_event(&mut self, event: &Event) -> bool {
        // Check for tab switching with [ and ]
        let mut tab_changed = false;
        if let Event::Key(key) = event {
            match key.code {
                ratatui::crossterm::event::KeyCode::Char(']') => {
                    self.tabs.next();
                    tab_changed = true;
                }
                ratatui::crossterm::event::KeyCode::Char('[') => {
                    self.tabs.previous();
                    tab_changed = true;
                }
                _ => {}
            }
        }

        // Forward to active panel
        let active_index = self.tabs.selected_index();
        let handled = if !tab_changed {
            if let Some(panel) = self.panels.get_mut(active_index) {
                panel.handle_event(event)
            } else {
                false
            }
        } else {
            true
        };

        // Check if this is the bookmarks panel (index 0) and selection changed (or tab switched to it)
        if active_index == 0
            && let Some(panel) = self.panels.get_mut(0)
            && let Some(id) = panel.take_selection_change()
        {
            self.pending_selection_change.set(Some(id));
        }

        handled
    }

    /// Take the pending selection change (bookmark ID) if any.
    pub fn take_selection_change(&self) -> Option<String> {
        self.pending_selection_change.take()
    }

    /// Set focus state.
    pub fn set_focus(&mut self, focused: bool) {
        self.focused = focused;
        for panel in &mut self.panels {
            panel.set_focus(focused);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // Result should be shorter than 25 chars and end with the filename
        assert!(result.len() <= 26); // Allow for "../" prefix
        assert!(result.ends_with("file.rs"));
    }

    #[test]
    fn test_shorten_path_absolute_path() {
        let path = "/Users/danielcardona/development/codemark/src/browser/tabbed_panel.rs";
        let result = shorten_path(path, 25);
        // Result should prioritize the last components
        assert!(result.len() <= 26);
        assert!(result.contains("tabbed_panel.rs") || result.contains("..."));
    }

    #[test]
    fn test_shorten_path_very_long_filename() {
        let path = "src/very_long_filename_that_exceeds_limit.rs";
        let result = shorten_path(path, 25);
        // Should truncate the filename if needed
        assert!(result.len() <= 25);
    }
}
