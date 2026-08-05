use crate::browser::{
    ContentTab, ContextTab, FiltersTab, Tab, TabContent, TabSelection, tabs::BORDER_EXTENSION,
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
    /// Set when a click on the top-border sort glyph cycled the active list's
    /// sort during the last `handle_event`. The owning layout drains this to
    /// refresh the preview, mirroring the `S` keyboard shortcut.
    pub sort_changed: std::cell::Cell<bool>,
    /// Number-key shortcut that jumps focus to this pane, rendered as a `[N]`
    /// badge on the top border. `None` hides the badge (e.g. in tests).
    pub pane_number: Option<u8>,
    /// Optional health label drawn as knockout text on the top border, just left
    /// of the sort-icon/pane-number badge. Set by the owning pane before render
    /// so the preview's health is spelled out alongside the colored dots. `None`
    /// hides the label (all panels except the preview/steps panel).
    pub health_label: std::cell::Cell<Option<HealthStatus>>,
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
/// Compute the [`HealthStatus`] for a bookmark, using live projection against
/// the current HEAD when a resolution is available and falling back to the
/// bookmark's persisted health otherwise.
///
/// This is the single source of truth for the health dot shown across the TUI
/// (bookmarks panel, collection pager), so those indicators stay consistent.
/// Map a persisted [`BookmarkHealth`] to a UI [`HealthStatus`] without any git
/// projection or database read. Cheap enough to call per step during a
/// synchronous list/collection build; live projection (which touches git) is
/// layered on top asynchronously via [`bookmark_health`].
pub fn persisted_bookmark_health(health: BookmarkHealth) -> HealthStatus {
    match health {
        BookmarkHealth::Active => HealthStatus::Healthy,
        BookmarkHealth::Drifted => HealthStatus::Drifted,
        BookmarkHealth::Stale | BookmarkHealth::Archived => HealthStatus::Broken,
    }
}

pub fn bookmark_health(
    bookmark: &Bookmark,
    db: &Database,
    current_head: Option<&str>,
) -> HealthStatus {
    let Some(ref resolution_id) = bookmark.current_resolution_id else {
        return persisted_bookmark_health(bookmark.health);
    };

    match db.get_resolution(resolution_id) {
        Ok(Some(resolution)) => {
            match projection::project_resolution_status(
                &resolution,
                bookmark,
                current_head,
                db.path(),
            ) {
                Ok(ui_status) => HealthStatus::from(ui_status),
                Err(_) => persisted_bookmark_health(resolution.health),
            }
        }
        _ => persisted_bookmark_health(bookmark.health),
    }
}

pub fn bookmark_to_panel_item(
    bookmark: &Bookmark,
    db: &Database,
    use_full_summary: bool,
    current_head: Option<&str>,
) -> PanelItem {
    let health = bookmark_health(bookmark, db, current_head);

    // Try to get a summary from the query for better display
    let summary_info = bookmark
        .language
        .parse::<Language>()
        .ok()
        .and_then(|lang| summarizer::summarize_query(&bookmark.query, Some(lang)).ok());

    let summary = if use_full_summary {
        summary_info.as_ref().and_then(|s| s.format()).unwrap_or_else(|| bookmark.query.clone())
    } else {
        summary_info
            .as_ref()
            .and_then(|s| s.identifier.clone())
            .unwrap_or_else(|| bookmark.query.clone())
    };

    let icon = summary_info.as_ref().map(|s| get_node_icon(&s.label)).unwrap_or("");

    // Keep the full path; it is compressed to the available pane width at render
    // time so it grows/shrinks as the left-pane layout is cycled.
    let mut item = PanelItem::new(&bookmark.file_path)
        .compressible_path()
        .metadata(bookmark.created_by.clone().unwrap_or_default())
        .health(health)
        .icon(icon)
        .created_at(bookmark.created_at.clone())
        .user_data(bookmark.id.clone());

    if !summary.is_empty() {
        item = item.emphasis(summary);
    }

    item
}

/// Create a `PanelItem` from a bookmark, seeding its health dot from the cached
/// *live* status when one has already been resolved (else `Unknown`).
///
/// Used when building panels whose health dots are updated asynchronously via
/// `LiveHealthBatch` events. Seeding from the cache means a rebuild (e.g. a
/// tag/branch filter) shows the resolved dot immediately instead of flashing
/// grey, while still avoiding the per-bookmark DB + git ancestry queries that
/// `bookmark_to_panel_item` performs.
fn bookmark_to_panel_item_cached(
    bookmark: &Bookmark,
    live: &std::collections::HashMap<String, HealthStatus>,
) -> PanelItem {
    let summary_info = bookmark
        .language
        .parse::<Language>()
        .ok()
        .and_then(|lang| summarizer::summarize_query(&bookmark.query, Some(lang)).ok());

    let summary = summary_info
        .as_ref()
        .and_then(|s| s.identifier.clone())
        .unwrap_or_else(|| bookmark.query.clone());

    let icon = summary_info.as_ref().map(|s| get_node_icon(&s.label)).unwrap_or("");

    let health = live.get(&bookmark.id).copied().unwrap_or(HealthStatus::Unknown);
    let mut item = PanelItem::new(&bookmark.file_path)
        .compressible_path()
        .metadata(bookmark.created_by.clone().unwrap_or_default())
        .health(health)
        .icon(icon)
        .created_at(bookmark.created_at.clone())
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

    /// Re-apply the current default syntax theme to every code preview held by
    /// this panel, re-highlighting their cached content.
    ///
    /// Previews capture their theme at construction, so a runtime theme change
    /// (via the Settings overlay) must push the new theme into existing
    /// previews — otherwise the chrome re-themes while the code stays stale.
    pub fn reapply_preview_theme(&mut self) {
        let theme = crate::component::code_preview::current_default_theme();
        for panel in &mut self.panels {
            if let TabContent::Preview(preview) = panel {
                preview.set_theme(theme.clone());
            }
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

    /// The active tab's list sort method, if that tab is a list with sorting
    /// enabled. Drives the sort glyph on the top border.
    pub fn active_sort_method(&self) -> Option<crate::component::SortMethod> {
        self.active_panel().and_then(|p| p.sort_method())
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
        active_tab: ContentTab,
    ) -> (Vec<PanelItem>, Vec<PanelItem>) {
        let tags_result = match active_tab {
            ContentTab::Bookmarks => db.list_bookmark_tags(),
            ContentTab::Collections | ContentTab::Tours => db.list_collection_tags(),
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
    pub fn build_content_items(
        db: &Database,
        live: &std::collections::HashMap<String, HealthStatus>,
        bookmark_live: &std::collections::HashMap<String, HealthStatus>,
    ) -> (Vec<PanelItem>, Vec<PanelItem>, Vec<PanelItem>) {
        let mut collections_items = Vec::new();
        let mut tours_items = Vec::new();

        if let Ok(collections) = db.list_collections() {
            for (c, count) in collections {
                let health = super::collection_health_status(c.health, live.get(&c.id).copied());

                let is_published = c.published_at.is_some();
                let branch = c.created_branch.clone().unwrap_or_else(|| "main".to_string());
                let name = c.name.clone();

                // Collections item - shows publish icon
                let collection_item = PanelItem::new(&name)
                    .secondary_text(&branch)
                    .metadata(format!("{count} steps"))
                    .health(health)
                    .published(is_published) // Only show icon on Collections tab
                    .created_at(c.created_at.clone())
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
            Ok(bookmarks) => bookmarks
                .iter()
                .map(|bm| bookmark_to_panel_item_cached(bm, bookmark_live))
                .collect(),
            Err(_) => Vec::new(),
        };

        (tours_items, collections_items, bookmarks)
    }

    /// Create panel 1 with Repos/Owners/Auth tabs.
    pub fn new_repos_accounts(db: &Database, registry: &rusqlite::Connection) -> Self {
        let repos_items = TabbedPanel::build_repo_items(db, registry);
        let mut repos_panel = Panel::new("").bordered(false);
        repos_panel = repos_panel.items(repos_items);

        // Move the cursor to the active repo (the one with the checkmark) so the
        // highlighted line matches it on startup instead of defaulting to the
        // first item. `set_selected` defers the scroll-into-view until the first
        // render (the panel has no area yet), keeping the active repo visible
        // even when the list is long enough to overflow the viewport.
        if let Some(active_idx) = repos_panel.all_items().iter().position(|item| item.is_active()) {
            tracing::debug!(target: "codemark::ui", active_idx, "selecting active repo on startup");
            repos_panel.set_selected(active_idx);
        }

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
            sort_changed: std::cell::Cell::new(false),
            pane_number: Some(1),
            health_label: std::cell::Cell::new(None),
        }
    }

    /// Create panel 2 with Tags/Branches tabs.
    pub fn new_tags_branches(db: &Database, active_tab: ContentTab) -> Self {
        let (tags_items, branches_items) = TabbedPanel::build_tags_branches_items(db, active_tab);
        let tags_panel = Panel::new("").bordered(false).multi_select(true).items(tags_items);
        let branches_panel =
            Panel::new("").bordered(false).multi_select(true).items(branches_items);

        let mut tabs = TabSelection::new(vec![
            Tab::new(FiltersTab::Tags.label()),
            Tab::new(FiltersTab::Branches.label()),
        ]);
        tabs.set_visible_count(Self::filters_visible_tab_count(active_tab));

        Self {
            tabs,
            panels: vec![TabContent::List(tags_panel), TabContent::List(branches_panel)],
            focused: false,
            last_area: std::cell::Cell::new(Rect::default()),
            pending_selection_change: std::cell::Cell::new(None),
            tab_changed: std::cell::Cell::new(false),
            sort_changed: std::cell::Cell::new(false),
            pane_number: Some(2),
            health_label: std::cell::Cell::new(None),
        }
    }

    /// Number of Filters panel tabs to show for the given active Content panel tab.
    ///
    /// Only Collections and Tours have an associated branch (by design), so the
    /// trailing Branches tab is hidden — leaving just Tags — whenever Bookmarks
    /// are active in Content panel.
    fn filters_visible_tab_count(active_tab: ContentTab) -> usize {
        match active_tab {
            ContentTab::Bookmarks => FiltersTab::Branches.index(),
            ContentTab::Collections | ContentTab::Tours => FiltersTab::all().len(),
        }
    }

    /// Show or hide Filters panel's trailing Branches tab based on the active Content panel
    /// tab. Bookmarks have no branch, so the Branches tab is hidden when they
    /// are active. See [`Self::filters_visible_tab_count`].
    pub fn sync_branches_tab_visibility(&mut self, active_tab: ContentTab) {
        self.tabs.set_visible_count(Self::filters_visible_tab_count(active_tab));
    }

    /// Create panel 3 with Bookmarks/Collections/Tours tabs.
    pub fn new_tours_collections_bookmarks(db: &Database) -> Self {
        let (tours_items, collections_items, bookmarks_items) =
            TabbedPanel::build_content_items(
                db,
                &std::collections::HashMap::new(),
                &std::collections::HashMap::new(),
            );
        // Bookmarks and Collections expose the `S` sort cycle, so they start with
        // an explicit order (most recent first). Tours keep insertion order.
        let tours_panel = Panel::new("").bordered(false).items(tours_items);
        let collections_panel = Panel::new("")
            .bordered(false)
            .sort(crate::component::SortMethod::DateNewest)
            .items(collections_items);
        let bookmarks_panel = Panel::new("")
            .bordered(false)
            .sort(crate::component::SortMethod::DateNewest)
            .items(bookmarks_items);

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
            sort_changed: std::cell::Cell::new(false),
            pane_number: Some(3),
            health_label: std::cell::Cell::new(None),
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
            sort_changed: std::cell::Cell::new(false),
            pane_number: Some(4),
            health_label: std::cell::Cell::new(None),
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
            sort_changed: std::cell::Cell::new(false),
            pane_number: Some(5),
            health_label: std::cell::Cell::new(None),
        }
    }

    /// Get the last rendered area.
    pub fn last_area(&self) -> Rect {
        self.last_area.get()
    }

    /// Clear the cached render area so this panel is excluded from all mouse
    /// hit-testing (tab switching and nested forwarding) until it is rendered
    /// again. See the empty-area guard in [`Self::handle_event`].
    ///
    /// The left pane only draws the focused panel when expanded (Half/Full),
    /// leaving the others with a stale `last_area` from an earlier layout. Mouse
    /// routing still consults every panel, so without this a click on the
    /// expanded panel can land on a hidden panel's old tab row or nested list.
    /// The layout invalidates the non-rendered panels each frame to keep
    /// hit-testing in sync with what is actually on screen.
    pub fn invalidate_area(&self) {
        self.last_area.set(Rect::default());
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
        let mut tab_titles = self.tabs.render_as_titles_with_counts_and_filters(&counts, &filtered);

        // The active tab's sort glyph (if that tab's list has sorting enabled),
        // drawn just left of the pane-number badge.
        let sort_icon = self.active_sort_method().map(crate::browser::tabs::sort_method_icon);

        // When a pane-number badge is drawn on the right of the top border,
        // cap the (left-aligned) title width so a long tab row can't run into
        // the badge — otherwise it visually attaches to the last tab label and
        // corrupts the shortcut cue. Reserve the badge's columns (plus the sort
        // glyph, when shown) and the left corner. See `render_pane_number_badge`.
        if let Some(number) = self.pane_number {
            let health = self.health_label.get();
            let reserved = crate::browser::tabs::top_border_reserved_width_full(
                number,
                sort_icon.is_some(),
                health,
            );
            // `- 1` for the left corner the title sits after.
            let max_title_width = (area.width as usize).saturating_sub(reserved as usize + 1);
            tab_titles = crate::browser::tabs::truncate_line_to_width(tab_titles, max_title_width);
        }

        // Render outer border for the entire panel area with inline tabs
        let border_color =
            if self.focused { crate::theme::palette().accent } else { crate::theme::palette().dim };
        let border_style = Style::default().fg(border_color);

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

        // Draw the pane-number badge (e.g. `[3]`) on the top-right border as a
        // visual cue for the number-key shortcut that jumps focus here, plus the
        // active tab's sort glyph just to its left.
        if let Some(number) = self.pane_number {
            crate::browser::tabs::render_pane_number_badge(area, buf, number, border_style);
            if let Some(icon) = sort_icon {
                crate::browser::tabs::render_sort_icon(area, buf, number, icon, border_style);
            }
        }

        // Draw the health label (knockout text) just left of the sort-icon/badge.
        if let (Some(number), Some(health)) = (self.pane_number, self.health_label.get()) {
            crate::browser::tabs::render_health_label(
                area,
                buf,
                health,
                crate::theme::palette().accent,
                number,
                sort_icon.is_some(),
            );
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
        // A panel that wasn't drawn this frame has an invalidated (empty) area,
        // so it is off-screen. Ignore mouse events entirely rather than routing
        // them by position: neither this panel's tab row nor its nested panel
        // owns live coordinates, and the caller (`LeftPane::handle_event`)
        // dispatches every mouse event to every panel. Without this guard a
        // hidden panel would still forward clicks/scrolls to its active nested
        // panel against stale cached coordinates, stealing input from the
        // visible, expanded panel.
        if matches!(event, Event::Mouse(_)) && self.last_area.get().is_empty() {
            return false;
        }

        // Check for tab switching with [ and ] or mouse click
        let old_index = self.tabs.selected_index();
        let mut tab_changed = false;
        // Set when a click landed on the top-border sort glyph; suppresses tab
        // hit-testing and body forwarding for that same click.
        let mut sort_cycled = false;
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
                        // A click on the top-border sort glyph cycles the active
                        // list's order, mirroring the `S` shortcut. Only fires
                        // when the active tab exposes sorting (the glyph is drawn).
                        if let Some(number) = self.pane_number
                            && self.active_sort_method().is_some()
                            && let Some((sx, ex)) =
                                crate::browser::tabs::sort_icon_hit_range(area, number)
                            && mouse.column >= sx
                            && mouse.column < ex
                            && let Some(panel) = self.active_panel_mut()
                        {
                            panel.cycle_sort();
                            self.sort_changed.set(true);
                            sort_cycled = true;
                        }
                        // Calculate x position relative to the panel's left edge
                        let relative_x = mouse.column - area.left();
                        // The tab click ranges are recorded before the title is
                        // truncated to make room for the pane-number badge, so
                        // ignore clicks past that same reserved boundary — they
                        // land on hidden title cells or the badge, not a tab.
                        let in_visible_title_region = match self.pane_number {
                            Some(number) => {
                                let reserved = crate::browser::tabs::top_border_reserved_width_full(
                                    number,
                                    self.active_sort_method().is_some(),
                                    self.health_label.get(),
                                );
                                relative_x <= area.width.saturating_sub(reserved + 1)
                            }
                            None => true,
                        };
                        if !sort_cycled
                            && in_visible_title_region
                            && self.tabs.handle_click(relative_x, mouse.row)
                        {
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

        // Forward to active panel. A tab switch or a sort-glyph click already
        // consumed the event, so don't also forward it to the nested panel.
        let handled = if tab_changed || sort_cycled {
            true
        } else if let Some(panel) = self.panels.get_mut(active_index) {
            panel.handle_event(event)
        } else {
            false
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

    /// Take (and reset) the flag indicating a top-border sort-glyph click cycled
    /// the active list's order during the last `handle_event`. The layout drains
    /// this to refresh the preview, mirroring the `S` keyboard shortcut.
    pub fn take_sort_changed(&self) -> bool {
        self.sort_changed.replace(false)
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
    use ratatui::crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };

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
            sort_changed: std::cell::Cell::new(false),
            pane_number: None,
            health_label: std::cell::Cell::new(None),
        }
    }

    /// A pane with a badge and a sortable active list, for exercising the
    /// top-border sort-glyph click.
    fn sortable_panel() -> TabbedPanel {
        let p1 = Panel::new("").sort(crate::component::SortMethod::DateNewest).items(vec![
            PanelItem::new("a").created_at("2024-01-01T00:00:00Z"),
            PanelItem::new("b").created_at("2026-01-01T00:00:00Z"),
        ]);
        let p2 = Panel::new("").items(vec![PanelItem::new("x")]);
        TabbedPanel {
            tabs: TabSelection::new(vec![Tab::new("A"), Tab::new("B")]),
            panels: vec![TabContent::List(p1), TabContent::List(p2)],
            focused: false,
            last_area: std::cell::Cell::new(Rect::default()),
            pending_selection_change: std::cell::Cell::new(None),
            tab_changed: std::cell::Cell::new(false),
            sort_changed: std::cell::Cell::new(false),
            pane_number: Some(3),
            health_label: std::cell::Cell::new(None),
        }
    }

    fn left_click(col: u16, row: u16) -> Event {
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        })
    }

    #[test]
    fn test_sort_glyph_click_cycles_sort_without_switching_tab() {
        let mut panel = sortable_panel();
        let area = Rect::new(0, 0, 40, 6);
        let mut buf = Buffer::empty(area);
        panel.render(area, &mut buf);

        assert_eq!(panel.active_sort_method(), Some(crate::component::SortMethod::DateNewest));

        // The glyph slot sits just left of the `[3]` badge: badge left edge is at
        // right - badge_width - 2 = 40 - 3 - 2 = 35, so the glyph is at col 33.
        let (sx, ex) = crate::browser::tabs::sort_icon_hit_range(area, 3).unwrap();
        assert!(sx <= 33 && 33 < ex);

        assert!(panel.handle_event(&left_click(33, 0)));
        // Cycled to the next method, stayed on tab 0, and raised the drain flag.
        assert_eq!(panel.active_sort_method(), Some(crate::component::SortMethod::DateOldest));
        assert_eq!(panel.tabs.selected_index(), 0);
        assert!(panel.take_sort_changed());
        assert!(!panel.take_sort_changed());
    }

    #[test]
    fn test_top_border_click_outside_glyph_does_not_cycle_sort() {
        let mut panel = sortable_panel();
        let area = Rect::new(0, 0, 40, 6);
        let mut buf = Buffer::empty(area);
        panel.render(area, &mut buf);

        // A click on the tab-title region (the second tab, "B", at col 5)
        // switches tabs rather than touching the sort order.
        panel.handle_event(&left_click(5, 0));
        assert_eq!(panel.tabs.selected_index(), 1);
        assert!(!panel.take_sort_changed());
        assert!(panel.take_tab_changed());
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

    /// Mouse input is honored only while the panel's cached render area is live.
    /// After `invalidate_area` (which the left pane calls each frame for panels
    /// it doesn't draw when expanded), both a tab-row click *and* a body click
    /// forwarded to the nested list must be ignored — otherwise a click in an
    /// expanded pane lands on a hidden panel's stale tab row or list row.
    #[test]
    fn test_invalidated_area_ignores_mouse_events() {
        let mut panel = two_tab_panel();
        // The nested list only acts on a Down while focused (see `Panel::handle_event`).
        panel.set_focus(true);
        let area = Rect::new(0, 0, 20, 6);
        let mut buf = Buffer::empty(area);

        // First render records the tab click boundaries on the top border.
        panel.render(area, &mut buf);
        assert_eq!(panel.tabs.selected_index(), 0);

        // Column landing on the second tab's title. With BORDER_EXTENSION (2),
        // tab "A" occupies cols 2..4 (" A " minus its leading offset), a
        // separator sits at 4, and tab "B" starts at col 5.
        let tab_b_col = 5u16;
        let click = |col: u16, row: u16| {
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: col,
                row,
                modifiers: KeyModifiers::NONE,
            })
        };

        // A click on the second tab switches to it, and a click on a body row
        // selects that nested-list item — proving the coordinates are live.
        assert!(panel.handle_event(&click(tab_b_col, area.top())));
        assert_eq!(panel.tabs.selected_index(), 1);
        panel.tabs.set_selected(0);
        assert!(panel.handle_event(&click(area.left() + 1, area.top() + 2)));
        let selected_after_body_click = panel.active_panel().unwrap().selected_index();

        // Invalidate the cached area as the left pane does for a panel it stops
        // rendering. The identical clicks must now be no-ops: the panel is
        // off-screen, so neither its tab row nor its nested list owns a hit-test
        // region.
        panel.invalidate_area();
        assert!(!panel.handle_event(&click(tab_b_col, area.top())));
        assert_eq!(panel.tabs.selected_index(), 0);
        assert!(!panel.handle_event(&click(area.left() + 1, area.top() + 3)));
        assert_eq!(panel.active_panel().unwrap().selected_index(), selected_after_body_click);
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
