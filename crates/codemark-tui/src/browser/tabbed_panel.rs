use crate::browser::{
    ContextTab, Panel2Tab, Panel3Tab, Tab, TabContent, TabSelection, tabs::BORDER_EXTENSION,
};
use crate::component::{CodePreview, HealthStatus, MarkdownPanel, Panel, PanelItem};
use crate::event::Event;
use codemark_core::engine::bookmark::{Bookmark, BookmarkFilter, BookmarkHealth};
use codemark_core::engine::projection;
use codemark_core::git::forge::ForgeKind;
use codemark_core::parser::languages::Language;
use codemark_core::query::classifier::get_node_icon;
use codemark_core::query::summarizer;
use codemark_core::storage::db::Database;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
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
    /// Set when the active tab changed during the last `handle_event`. Filters
    /// are pane-scoped (one query per pane, applied to the active tab), so the
    /// owning layout drains this to clear the pane's stored filter on a tab
    /// switch instead of carrying it over to the newly active tab.
    pub tab_changed: std::cell::Cell<bool>,
}

/// Convert a bookmark to a PanelItem with consistent formatting.
///
/// This function encapsulates the common logic for displaying bookmarks
/// in the TUI, ensuring consistent formatting across all contexts.
///
/// # Arguments
/// * `bookmark` - The bookmark to convert
/// * `db` - Database reference for fetching health status
/// * `use_full_summary` - If true, use the full summary format (label + identifier).
///   If false, use only the identifier for more compact display.
///
/// # Returns
/// A PanelItem with formatted display text, health status, icon, and metadata
pub fn bookmark_to_panel_item(
    bookmark: &Bookmark,
    db: &Database,
    use_full_summary: bool,
    current_head: Option<&str>,
) -> PanelItem {
    // Get the current resolution for projection
    let health = if let Some(ref resolution_id) = bookmark.current_resolution_id {
        match db.get_resolution(resolution_id) {
            Ok(Some(resolution)) => {
                match projection::project_resolution_status(
                    &resolution,
                    bookmark,
                    current_head,
                    db.path(),
                ) {
                    Ok(ui_status) => match ui_status {
                        codemark_core::engine::projection::UIStatus::Healthy => {
                            HealthStatus::Healthy
                        }
                        codemark_core::engine::projection::UIStatus::UnanchoredHealthy => {
                            HealthStatus::UnanchoredHealthy
                        }
                        codemark_core::engine::projection::UIStatus::Drifted => {
                            HealthStatus::Drifted
                        }
                        codemark_core::engine::projection::UIStatus::UnanchoredDrifting => {
                            HealthStatus::UnanchoredDrifting
                        }
                        codemark_core::engine::projection::UIStatus::Broken => HealthStatus::Broken,
                        codemark_core::engine::projection::UIStatus::BrokenUnanchored => {
                            HealthStatus::BrokenUnanchored
                        }
                        codemark_core::engine::projection::UIStatus::Verified => {
                            HealthStatus::Verified
                        }
                        codemark_core::engine::projection::UIStatus::Outdated => {
                            HealthStatus::Outdated
                        }
                        codemark_core::engine::projection::UIStatus::Future => HealthStatus::Future,
                    },
                    Err(_) => match resolution.health {
                        BookmarkHealth::Active => HealthStatus::Healthy,
                        BookmarkHealth::Drifted => HealthStatus::Drifted,
                        BookmarkHealth::Stale | BookmarkHealth::Archived => HealthStatus::Broken,
                    },
                }
            }
            _ => match bookmark.health {
                BookmarkHealth::Active => HealthStatus::Healthy,
                BookmarkHealth::Drifted => HealthStatus::Drifted,
                BookmarkHealth::Stale | BookmarkHealth::Archived => HealthStatus::Broken,
            },
        }
    } else {
        match bookmark.health {
            BookmarkHealth::Active => HealthStatus::Healthy,
            BookmarkHealth::Drifted => HealthStatus::Drifted,
            BookmarkHealth::Stale | BookmarkHealth::Archived => HealthStatus::Broken,
        }
    };

    // Try to get a summary from the query for better display
    let summary_info = bookmark
        .language
        .parse::<Language>()
        .ok()
        .and_then(|lang| summarizer::summarize_query(&bookmark.query, Some(lang)).ok());

    let summary = if use_full_summary {
        summary_info.as_ref().and_then(|s| s.format()).unwrap_or_else(|| bookmark.query.clone())
    } else {
        summary_info.as_ref().and_then(|s| s.identifier.clone()).unwrap_or_else(|| {
            if summary_info.is_some() { String::new() } else { bookmark.query.clone() }
        })
    };

    let icon = summary_info.as_ref().map(|s| get_node_icon(&s.label)).unwrap_or("");

    // Keep the full path; it is compressed to the available pane width at render
    // time so it grows/shrinks as the left-pane layout is cycled.
    let mut item = PanelItem::new(&bookmark.file_path)
        .compressible_path()
        .metadata(bookmark.created_by.clone().unwrap_or_default())
        .health(health)
        .icon(icon)
        .user_data(bookmark.id.clone());

    if !summary.is_empty() {
        item = item.emphasis(summary);
    }

    item
}

/// Create a `PanelItem` from a bookmark with `Unknown` health status.
///
/// Used when building panels that will have their health dots updated
/// asynchronously via `LiveHealthBatch` events. Avoids the per-bookmark
/// DB + git ancestry queries that `bookmark_to_panel_item` performs.
fn bookmark_to_panel_item_unknown(bookmark: &Bookmark) -> PanelItem {
    let summary_info = bookmark
        .language
        .parse::<Language>()
        .ok()
        .and_then(|lang| summarizer::summarize_query(&bookmark.query, Some(lang)).ok());

    let summary = summary_info.as_ref().and_then(|s| s.identifier.clone()).unwrap_or_else(|| {
        if summary_info.is_some() { String::new() } else { bookmark.query.clone() }
    });

    let icon = summary_info.as_ref().map(|s| get_node_icon(&s.label)).unwrap_or("");

    let mut item = PanelItem::new(&bookmark.file_path)
        .compressible_path()
        .metadata(bookmark.created_by.clone().unwrap_or_default())
        .health(HealthStatus::Unknown)
        .icon(icon)
        .user_data(bookmark.id.clone());

    if !summary.is_empty() {
        item = item.emphasis(summary);
    }

    item
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

    /// Scroll the active tab's content by `delta` lines (positive scrolls down),
    /// regardless of focus. Returns true if the viewport moved. Only the
    /// scrollable content kinds (code preview, markdown) respond; list tabs are
    /// driven by selection, not a free viewport, so they ignore this.
    pub fn scroll_active(&mut self, delta: i32) -> bool {
        match self.panels.get_mut(self.tabs.selected_index()) {
            Some(TabContent::Preview(p)) => p.scroll_by(delta),
            Some(TabContent::Markdown(p)) => p.scroll_by(delta),
            _ => false,
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

    /// Get the currently active markdown panel (immutable).
    /// Returns the markdown panel only if the active tab contains markdown content.
    pub fn get_markdown(&self) -> Option<&MarkdownPanel> {
        match self.panels.get(self.tabs.selected_index()) {
            Some(TabContent::Markdown(p)) => Some(p),
            _ => None,
        }
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
                    let forge = repo
                        .origin_url
                        .as_deref()
                        .map(ForgeKind::from_origin_url)
                        .unwrap_or(ForgeKind::Unknown);
                    PanelItem::new(repo.repo_name)
                        .icon(forge.icon())
                        .plain_icon()
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

    /// Build the list of repo owner (user/org) items from the global registry,
    /// each tagged with a forge icon inferred from its origin URL.
    pub fn build_owner_items(registry: &rusqlite::Connection) -> Vec<PanelItem> {
        use codemark_core::storage::registry;
        if let Ok(owners) = registry::list_repo_owners_with_forge(registry) {
            owners
                .into_iter()
                .map(|(owner, forge)| {
                    PanelItem::new(owner.clone())
                        .icon(forge.icon())
                        .plain_icon()
                        .user_data(owner)
                        .no_health()
                })
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Build the list of authenticated account items from the global registry.
    ///
    /// Each item shows the username with a forge icon and the Codetours server
    /// URL inline as secondary text. Every listed account is active (logout
    /// removes its row), so no extra status marker is needed. Mirrors
    /// `codemark auth list`. Read-only.
    pub fn build_auth_account_items(registry: &rusqlite::Connection) -> Vec<PanelItem> {
        use codemark_core::storage::registry;
        match registry::list_accounts(registry, None) {
            Ok(accounts) if !accounts.is_empty() => accounts
                .into_iter()
                .map(|account| {
                    let forge = ForgeKind::from_forge_str(&account.forge_kind);
                    PanelItem::new(account.username)
                        .icon(forge.icon())
                        .plain_icon()
                        .secondary_text(account.server_url)
                        .no_health()
                })
                .collect(),
            Ok(_) => vec![
                PanelItem::new(format!(
                    "No accounts — default: {}",
                    codemark_core::DEFAULT_SERVER_URL
                ))
                .no_health()
                .color(crate::theme::palette().dim),
            ],
            Err(e) => vec![
                PanelItem::new(format!("Error: {e}"))
                    .no_health()
                    .color(crate::theme::palette().error),
            ],
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
                    // `marker` is the keyword hue (base0E) — the same color the
                    // code preview uses to highlight keywords.
                    PanelItem::new(format!("#{tag}"))
                        .user_data(tag)
                        .no_health()
                        .color(crate::theme::palette().marker)
                })
                .collect(),
            Ok(_) => {
                vec![PanelItem::new("No tags found").no_health().color(crate::theme::palette().dim)]
            }
            Err(e) => vec![
                PanelItem::new(format!("Error: {e}"))
                    .no_health()
                    .color(crate::theme::palette().error),
            ],
        };

        let branches = match db.list_all_branches() {
            Ok(branches) if !branches.is_empty() => branches
                .into_iter()
                .map(|branch| PanelItem::new(branch).no_health().icon(""))
                .collect(),
            Ok(_) => vec![
                PanelItem::new("No branches found").no_health().color(crate::theme::palette().dim),
            ],
            Err(e) => vec![
                PanelItem::new(format!("Error: {e}"))
                    .no_health()
                    .color(crate::theme::palette().error),
            ],
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
                            HealthStatus::Drifted
                        }
                        codemark_core::engine::bookmark::CollectionHealth::Stale => {
                            HealthStatus::Broken
                        }
                    },
                    None => HealthStatus::Unknown,
                };

                let is_published = c.published_at.is_some();
                let branch = c.created_branch.clone().unwrap_or_else(|| "main".to_string());
                let name = c.name.clone();

                // Collections item - shows publish icon
                let collection_item = PanelItem::new(&name)
                    .secondary_text(&branch)
                    .metadata(format!("{count} steps"))
                    .health(health)
                    .published(is_published) // Only show icon on Collections tab
                    .user_data(c.id.clone());

                collections_items.push(collection_item);

                // Tours item - show if published or imported (pulled from remote)
                let is_tour = is_published || c.imported_from_url.is_some();
                if is_tour {
                    let tour_item = PanelItem::new(&name)
                        .secondary_text(&branch)
                        .metadata(format!("{count} steps"))
                        .health(health)
                        .checkmark(true)
                        .user_data(c.id);
                    tours_items.push(tour_item);
                }
            }
        }

        let bookmarks = match db.list_bookmarks(&BookmarkFilter::default()) {
            Ok(bookmarks) => bookmarks.iter().map(bookmark_to_panel_item_unknown).collect(),
            Err(_) => Vec::new(),
        };

        (tours_items, collections_items, bookmarks)
    }

    /// Create panel 1 with Repos/Owners/Auth tabs.
    pub fn new_repos_accounts(db: &Database, registry: &rusqlite::Connection) -> Self {
        let repos_items = TabbedPanel::build_repo_items(db, registry);
        let mut repos_panel = Panel::new("").bordered(false);
        repos_panel = repos_panel.items(repos_items);

        // Owners tab: repo owners (users/orgs); multi-select filters the Repos list.
        let owner_items = TabbedPanel::build_owner_items(registry);
        let owners = Panel::new("").items(owner_items).bordered(false).multi_select(true);

        // Auth tab: authenticated accounts; read-only.
        let auth_items = TabbedPanel::build_auth_account_items(registry);
        let auth = Panel::new("").items(auth_items).bordered(false);

        let tabs = TabSelection::new(vec![
            Tab::new(ContextTab::Repos.label()),
            Tab::new(ContextTab::Owners.label()),
            Tab::new(ContextTab::Auth.label()),
        ]);

        Self {
            tabs,
            panels: vec![
                TabContent::List(repos_panel),
                TabContent::List(owners),
                TabContent::List(auth),
            ],
            focused: false,
            last_area: std::cell::Cell::new(Rect::default()),
            pending_selection_change: std::cell::Cell::new(None),
            tab_changed: std::cell::Cell::new(false),
        }
    }

    /// Create panel 2 with Tags/Branches tabs.
    pub fn new_tags_branches(db: &Database, active_tab: Panel3Tab) -> Self {
        let (tags_items, branches_items) = TabbedPanel::build_tags_branches_items(db, active_tab);
        let tags_panel = Panel::new("").bordered(false).multi_select(true).items(tags_items);
        let branches_panel =
            Panel::new("").bordered(false).multi_select(true).items(branches_items);

        let mut tabs = TabSelection::new(vec![
            Tab::new(Panel2Tab::Tags.label()),
            Tab::new(Panel2Tab::Branches.label()),
        ]);
        tabs.set_visible_count(Self::panel2_visible_tab_count(active_tab));

        Self {
            tabs,
            panels: vec![TabContent::List(tags_panel), TabContent::List(branches_panel)],
            focused: false,
            last_area: std::cell::Cell::new(Rect::default()),
            pending_selection_change: std::cell::Cell::new(None),
            tab_changed: std::cell::Cell::new(false),
        }
    }

    /// Number of Panel 2 tabs to show for the given active Panel 3 tab.
    ///
    /// Only Collections and Tours have an associated branch (by design), so the
    /// trailing Branches tab is hidden — leaving just Tags — whenever Bookmarks
    /// are active in Panel 3.
    fn panel2_visible_tab_count(active_tab: Panel3Tab) -> usize {
        match active_tab {
            Panel3Tab::Bookmarks => Panel2Tab::Branches.index(),
            Panel3Tab::Collections | Panel3Tab::Tours => Panel2Tab::all().len(),
        }
    }

    /// Show or hide Panel 2's trailing Branches tab based on the active Panel 3
    /// tab. Bookmarks have no branch, so the Branches tab is hidden when they
    /// are active. See [`Self::panel2_visible_tab_count`].
    pub fn sync_branches_tab_visibility(&mut self, active_tab: Panel3Tab) {
        self.tabs.set_visible_count(Self::panel2_visible_tab_count(active_tab));
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
            tab_changed: std::cell::Cell::new(false),
        }
    }

    /// Create steps/info/query tabs for right pane.
    pub fn new_steps_info(_db: &Database) -> Self {
        use crate::component::CodePreview;

        // Start with empty previews - content will be loaded when a bookmark is selected
        let preview = CodePreview::new("", "rs");

        let info = MarkdownPanel::new();

        let query_preview = CodePreview::new("", "scm");

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
            tab_changed: std::cell::Cell::new(false),
        }
    }

    /// Create details/comments tabs for right pane bottom area.
    pub fn new_details_comments() -> Self {
        let details = MarkdownPanel::new();
        let comments = MarkdownPanel::new();

        let tabs = TabSelection::new(vec![Tab::new("Details"), Tab::new("Comments")]);

        Self {
            tabs,
            panels: vec![TabContent::Markdown(details), TabContent::Markdown(comments)],
            focused: false,
            last_area: std::cell::Cell::new(Rect::default()),
            pending_selection_change: std::cell::Cell::new(None),
            tab_changed: std::cell::Cell::new(false),
        }
    }

    /// Get the last rendered area.
    pub fn last_area(&self) -> Rect {
        self.last_area.get()
    }

    /// Render the tabbed panel.
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        self.last_area.set(area);

        // For multi-select list panels, surface the number of active (selected)
        // items as a `(n)` suffix on the tab title, e.g. "Tags (2)".
        let counts: Vec<Option<usize>> = self
            .panels
            .iter()
            .map(|panel| match panel {
                TabContent::List(p) if p.is_multi_select() => {
                    let count = p.active_item_count();
                    (count > 0).then_some(count)
                }
                _ => None,
            })
            .collect();

        // Flag each list tab whose results are currently narrowed by a filter,
        // so a filter glyph can be drawn beside its title.
        let filtered: Vec<bool> = self
            .panels
            .iter()
            .map(|panel| matches!(panel, TabContent::List(p) if p.is_filtered()))
            .collect();
        let tab_titles = self.tabs.render_as_titles_with_counts_and_filters(&counts, &filtered);

        // Render outer border for the entire panel area with inline tabs
        let border_style = if self.focused {
            Style::default().fg(crate::theme::palette().accent)
        } else {
            Style::default().fg(crate::theme::palette().dim)
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
                    Style::default().fg(crate::theme::palette().accent).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(crate::theme::palette().dim)
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
        // Check for tab switching with [ and ] or mouse click
        let old_index = self.tabs.selected_index();
        let mut tab_changed = false;
        match event {
            Event::Key(key) => match key.code {
                ratatui::crossterm::event::KeyCode::Char(']') => {
                    self.tabs.next();
                    tab_changed = true;
                }
                ratatui::crossterm::event::KeyCode::Char('[') => {
                    self.tabs.previous();
                    tab_changed = true;
                }
                _ => {}
            },
            Event::Mouse(mouse) => {
                use ratatui::crossterm::event::MouseButton;
                if matches!(
                    mouse.kind,
                    ratatui::crossterm::event::MouseEventKind::Down(MouseButton::Left)
                ) {
                    let area = self.last_area.get();
                    // Check if click is on the top border (where tabs are)
                    // mouse.row is y (vertical), mouse.column is x (horizontal)
                    if mouse.row == area.top()
                        && mouse.column >= area.left()
                        && mouse.column < area.right()
                    {
                        // Calculate x position relative to the panel's left edge
                        let relative_x = mouse.column - area.left();
                        if self.tabs.handle_click(relative_x, mouse.row) {
                            tab_changed = true;
                        }
                    }
                }
            }
            _ => {}
        }

        // A filter is pane-scoped (one query applied to the active tab), so on
        // an actual tab switch clear every tab's filter and flag the change so
        // the owning layout can drop the pane's stored filter. Without this the
        // filter would visually carry over to the newly active tab.
        let active_index = self.tabs.selected_index();
        if active_index != old_index {
            self.clear_filters();
            self.tab_changed.set(true);
        }

        // Forward to active panel
        let handled = if !tab_changed {
            if let Some(panel) = self.panels.get_mut(active_index) {
                panel.handle_event(event)
            } else {
                false
            }
        } else {
            true
        };

        // Capture selection changes for the active list panel (bookmarks,
        // collections, or tours) so the right pane can live-update its preview.
        // This also fires when switching tabs, surfacing the newly active
        // panel's current selection.
        if let Some(TabContent::List(panel)) = self.panels.get_mut(active_index)
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

    /// Take (and reset) the flag indicating the active tab changed during the
    /// last `handle_event`. Used by the layout to clear the pane's stored
    /// filter so it does not carry over to the newly active tab.
    pub fn take_tab_changed(&self) -> bool {
        self.tab_changed.replace(false)
    }

    /// Clear any active filter on every tab's list panel.
    pub fn clear_filters(&mut self) {
        for panel in &mut self.panels {
            if let TabContent::List(p) = panel {
                p.set_filter("");
            }
        }
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
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn two_tab_panel() -> TabbedPanel {
        let p1 = Panel::new("").items(vec![PanelItem::new("alpha"), PanelItem::new("beta")]);
        let p2 = Panel::new("").items(vec![PanelItem::new("gamma")]);
        TabbedPanel {
            tabs: TabSelection::new(vec![Tab::new("A"), Tab::new("B")]),
            panels: vec![TabContent::List(p1), TabContent::List(p2)],
            focused: false,
            last_area: std::cell::Cell::new(Rect::default()),
            pending_selection_change: std::cell::Cell::new(None),
            tab_changed: std::cell::Cell::new(false),
        }
    }

    #[test]
    fn test_tab_switch_clears_filter_and_flags_change() {
        let mut panel = two_tab_panel();
        panel.active_panel_mut().unwrap().set_filter("alp");
        assert!(panel.active_panel().unwrap().is_filtered());

        // Switching tabs clears every tab's filter and raises the change flag.
        let next = Event::Key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE));
        panel.handle_event(&next);

        assert_eq!(panel.tabs.selected_index(), 1);
        assert!(panel.take_tab_changed());
        // The flag is drained on read.
        assert!(!panel.take_tab_changed());
        for content in &panel.panels {
            if let TabContent::List(p) = content {
                assert!(!p.is_filtered());
            }
        }
    }

    #[test]
    fn test_non_tab_key_keeps_filter() {
        let mut panel = two_tab_panel();
        panel.active_panel_mut().unwrap().set_filter("alp");

        // A non-tab key doesn't switch tabs, so the filter and flag are untouched.
        let down = Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        panel.handle_event(&down);

        assert_eq!(panel.tabs.selected_index(), 0);
        assert!(!panel.take_tab_changed());
        assert!(panel.active_panel().unwrap().is_filtered());
    }
}
