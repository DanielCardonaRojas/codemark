//! Browser layout for the Codemark TUI.
//!
//! This module provides the main browser layout with a left sidebar
//! containing search, repos, and tours, and a right main content area.

mod bindings;
mod data;
mod events;
mod left_pane;
mod right_pane;
mod search;
mod tabbed_panel;
mod tabs;
mod types;

pub use left_pane::LeftPane;
pub use right_pane::{RightPane, RightPaneFocus};
pub use search::{SearchBar, SearchMode};
pub use tabbed_panel::{TabbedPanel, bookmark_to_panel_item};
pub use tabs::{Panel2Tab, Panel3Tab, Tab, TabSelection};
pub use types::{
    DetailsPaneSize, ExternalCommand, FocusArea, HealNotification, HealTarget, LeftPaneSize,
    RightPaneSize, SectionConfig, SpinningItem, StepData, TabContent, escape_markdown,
};

use crate::component::{Component, HealthStatus, PanelItem};
use crate::event::Event;
use codemark_core::config::Config;
use codemark_core::embeddings::config::EmbeddingModel;
use codemark_core::engine::bookmark::{Bookmark, BookmarkFilter};
use codemark_core::parser::languages::{Language as CodemarkLanguage, ParseCache};
use codemark_core::storage::{SemanticRepo, db::Database};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
};
use std::collections::HashMap;

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
    let prefix_overhead = 3; // "../" prefix that will be added if we truncate

    // Start from the last component and work backwards
    for (_i, component) in components.iter().enumerate().rev() {
        let component_len = component.len();
        let separator_len = if result.is_empty() { 0 } else { 1 }; // '/' separator

        // Use max_width - prefix_overhead unconditionally to avoid edge cases
        // where we build a string that then exceeds max_width after prepending "../"
        let budget = max_width.saturating_sub(prefix_overhead);
        if result.len() + component_len + separator_len > budget {
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

    // Fallback: if we couldn't build anything meaningful, just truncate with ellipsis
    // Use char_indices() for UTF-8 safety
    if result.is_empty() || result.len() > max_width {
        if path.len() > max_width {
            let take = max_width.saturating_sub(3);
            let start =
                path.char_indices().rev().nth(take.saturating_sub(1)).map(|(i, _)| i).unwrap_or(0);
            format!("...{}", &path[start..])
        } else {
            path.to_string()
        }
    } else {
        result
    }
}

/// The main browser layout.
///
/// Splits the screen vertically with a left sidebar (40%) and right main area (60%).
/// Each section has numbered tabs that can be cycled with `[` and `]`.
pub struct BrowserLayout {
    /// Database connection
    db: Database,
    /// Global repository registry
    registry: rusqlite::Connection,
    /// Left sidebar components
    left_pane: LeftPane,
    /// Right main content area
    right_pane: RightPane,
    /// Current focus area
    focus: FocusArea,
    /// Previous focus area before entering filter mode
    previous_focus: Option<FocusArea>,
    /// Pending external command to be executed
    pending_command: Option<ExternalCommand>,
    /// Pending heal notification to be displayed
    pending_notification: Option<HealNotification>,
    /// Event handler for sending custom events
    event_handler: crate::event::EventHandler,
    /// Clipboard context for copy operations (kept alive for X11 selection ownership)
    clipboard: Option<copypasta::ClipboardContext>,
    /// Current size mode for the left pane
    left_pane_size: LeftPaneSize,
    /// Current size mode for the right pane (preview)
    right_pane_size: RightPaneSize,
    /// Current size mode for the details pane
    details_pane_size: DetailsPaneSize,
    /// Whether a tour pull is in progress (for post-pull panel rebuild)
    is_pulling_tour: bool,
    /// Panel items currently showing an animated spinner
    spinning_items: Vec<SpinningItem>,
    /// Tick at which spinners should be cleared (deferred to complete at least one full cycle)
    spinner_clear_at: Option<usize>,
    /// Tick counter for spinner animation
    tick_count: usize,
    /// Last-fetched remote tours (cached to avoid re-fetching after pull)
    cached_remote_tours: Vec<codemark_core::sync::RemoteTourSummary>,
    /// Repo URL used for the in-flight remote tours fetch (guards against stale responses)
    pending_remote_repo_url: Option<String>,
    /// Session-level parse cache for live resolution, keyed by language.
    /// Each `ParseCache` is single-language (parser locked at creation).
    session_cache: HashMap<CodemarkLanguage, ParseCache>,
}

impl BrowserLayout {
    /// Create a new browser layout.
    pub fn new(db: Database, event_handler: crate::event::EventHandler) -> Self {
        use codemark_core::storage::registry;
        let registry = registry::open_registry().expect("Failed to open global registry");

        // Determine initial focus: if there are no bookmarks in the current database,
        // focus the repos pane (Panel1) so the user can select a repository.
        // Otherwise, focus the bookmarks pane (Panel3).
        let initial_focus = if db
            .list_bookmarks(&codemark_core::engine::bookmark::BookmarkFilter::default())
            .map(|b| !b.is_empty())
            .unwrap_or(false)
        {
            FocusArea::Panel3
        } else {
            FocusArea::Panel1
        };

        let mut layout = Self {
            left_pane: LeftPane::new(&db, &registry),
            right_pane: RightPane::new(&db),
            focus: initial_focus,
            previous_focus: None,
            db,
            registry,
            pending_command: None,
            pending_notification: None,
            event_handler,
            clipboard: None,
            left_pane_size: LeftPaneSize::Regular,
            right_pane_size: RightPaneSize::Regular,
            details_pane_size: DetailsPaneSize::Regular,
            is_pulling_tour: false,
            spinning_items: Vec::new(),
            spinner_clear_at: None,
            tick_count: 0,
            cached_remote_tours: Vec::new(),
            pending_remote_repo_url: None,
            session_cache: HashMap::new(),
        };
        layout.update_focus_state();
        layout.sync_steps_tab_label();

        // Spawn background live health resolution so bookmark dots update on startup
        if let Ok(all_bookmarks) =
            layout.db.list_bookmarks(&codemark_core::engine::bookmark::BookmarkFilter::default())
        {
            layout.spawn_live_health_task(all_bookmarks);
        }

        layout
    }

    /// Update live preview for bookmarks panel when on the bookmarks tab.
    fn update_bookmarks_live_preview(&mut self) {
        if self.focus != FocusArea::Panel3 {
            return;
        }

        if let Some(Panel3Tab::Bookmarks) =
            Panel3Tab::from_index(self.left_pane.panel3.tabs.selected_index())
            && let Some(TabContent::List(panel)) = self.left_pane.panel3.panels.first()
            && let Some(selected) = panel.selected()
            && let Some(ref id) = selected.user_data
        {
            self.right_pane.load_bookmark_live(&self.db, id, &mut self.session_cache);
        }
    }

    /// Sync the right pane's first tab label based on the active Panel3 tab.
    /// Shows "Content" when on Bookmarks (single items), "Steps" otherwise (collections/tours).
    fn sync_steps_tab_label(&mut self) {
        let panel3_tab = Panel3Tab::from_index(self.left_pane.panel3.tabs.selected_index());
        let label = match panel3_tab {
            Some(Panel3Tab::Bookmarks) => "Content",
            _ => "Steps",
        };
        tracing::debug!(
            target: "codemark::ui",
            ?panel3_tab,
            new_label = %label,
            "syncing right pane tab label"
        );
        self.right_pane.steps.tabs.set_tab_label(0, label);
    }

    /// Take the pending external command, if any.
    pub fn take_pending_command(&mut self) -> Option<ExternalCommand> {
        self.pending_command.take()
    }

    /// Take the pending heal notification, if any.
    pub fn take_pending_notification(&mut self) -> Option<HealNotification> {
        self.pending_notification.take()
    }

    /// Execute a search based on the current search bar query and mode.
    pub fn execute_search(&self) {
        let query = self.left_pane.search.query().to_string();
        if query.is_empty() {
            return;
        }

        let mode = self.left_pane.search.mode();
        let db_path = self.db.path().to_path_buf();
        let event_handler = self.event_handler.clone();

        match mode {
            SearchMode::Fts => {
                // FTS search can be done synchronously as it's usually fast
                let bookmarks =
                    self.db.search_bookmarks(Some(&query), None, None, None, None, None, None);
                match bookmarks {
                    Ok(bm) => {
                        let _ = event_handler.send(Event::SearchResults(bm));
                    }
                    Err(e) => {
                        let _ = event_handler.send(Event::SearchError(e.to_string()));
                    }
                }
            }
            SearchMode::Semantic => {
                // Load config to get semantic settings
                let Some(codemark_dir) = self.db.path().parent() else {
                    return;
                };
                let config = Config::load_layered(codemark_dir);

                tokio::task::spawn_blocking(move || {
                    // Open a new DB connection for the background task to avoid !Send issues
                    let Ok(db) = Database::open(&db_path) else {
                        let _ = event_handler.send(Event::SearchError(
                            "Failed to open database for search".to_string(),
                        ));
                        return;
                    };

                    let model = config
                        .semantic
                        .model
                        .as_deref()
                        .and_then(|m| m.parse::<EmbeddingModel>().ok())
                        .unwrap_or(EmbeddingModel::AllMiniLmL6V2);

                    let distance_metric = config.semantic.get_distance_metric();
                    let threshold = config.semantic.threshold;
                    let models_dir = config.semantic.get_models_dir();

                    let semantic_repo =
                        SemanticRepo::with_config(models_dir, model, distance_metric, threshold);

                    let handle = tokio::runtime::Handle::current();
                    match tokio::task::block_in_place(|| {
                        handle.block_on(semantic_repo.search(db.conn(), &query, 20))
                    }) {
                        Ok(results) => {
                            let mut bookmarks = Vec::new();
                            for result in results {
                                if let Ok(Some(bm)) = db.get_bookmark(&result.bookmark_id) {
                                    bookmarks.push(bm);
                                }
                            }
                            let _ = event_handler.send(Event::SearchResults(bookmarks));
                        }
                        Err(e) => {
                            let _ = event_handler.send(Event::SearchError(e.to_string()));
                        }
                    }
                });
            }
        }
    }

    /// Switch the active database to a specific repository root.
    pub fn switch_database(&mut self, repo_root: &str) -> codemark_core::error::Result<()> {
        let db_path = std::path::Path::new(repo_root).join(".codemark").join("codemark.db");
        if db_path.exists() {
            self.db = Database::open(&db_path)?;
            self.right_pane.refresh_head_commit(&self.db);
            self.session_cache.clear();
            self.refresh_all_panels();
        }
        Ok(())
    }

    /// Minimum number of ticks a spinner must run before clearing (one visual cycle).
    const SPINNER_MIN_TICKS: usize = 5;

    /// Add a spinner to a panel item. The spinner animates on each tick.
    fn add_spinner(&mut self, user_data_key: &str, tab_index: usize) {
        if let Some(panel) = self.left_pane.panel3.get_list_panel_mut(tab_index) {
            panel.update_item_spinner(user_data_key, Some("\u{28cb}"));
        }
        self.spinner_clear_at = None;
        self.spinning_items.push(SpinningItem {
            user_data_key: user_data_key.to_string(),
            tab_index,
            start_tick: self.tick_count,
        });
    }

    /// Schedule spinners to be cleared after completing at least one full cycle.
    /// If no spinners are active, refreshes panels immediately.
    fn schedule_clear_spinners(&mut self) {
        if self.spinning_items.is_empty() {
            // No spinners to clear — still refresh panels for the completed operation
            self.refresh_all_panels();
            if std::mem::take(&mut self.is_pulling_tour) {
                self.rebuild_tours_panel();
            }
            return;
        }
        // Find the earliest start tick among active spinners
        let earliest_start = self.spinning_items.iter().map(|s| s.start_tick).min().unwrap();
        let elapsed = self.tick_count.wrapping_sub(earliest_start);
        if elapsed >= Self::SPINNER_MIN_TICKS {
            // Already completed a full cycle — clear immediately
            self.finish_clear_spinners();
        } else {
            // Defer until the cycle completes
            self.spinner_clear_at = Some(earliest_start.wrapping_add(Self::SPINNER_MIN_TICKS));
        }
    }

    /// Actually remove all spinners and refresh panels.
    fn finish_clear_spinners(&mut self) {
        self.spinner_clear_at = None;
        for item in self.spinning_items.drain(..) {
            if let Some(panel) = self.left_pane.panel3.get_list_panel_mut(item.tab_index) {
                panel.update_item_spinner(&item.user_data_key, None);
            }
        }
        self.refresh_all_panels();
        // Rebuild the hybrid local+remote Tours view after refresh_all_panels
        // (which only shows DB-local data). This must come second so the remote
        // tours are overlaid on top of the fresh local list.
        if std::mem::take(&mut self.is_pulling_tour) {
            self.rebuild_tours_panel();
        }
    }

    /// Start healing the currently selected bookmark(s) based on focus.
    ///
    /// This spawns an async background task to perform the heal operation.
    /// When complete, a HealComplete event will be sent with the result.
    pub fn start_heal_selection(&mut self) {
        let db_path = self.db.path().to_path_buf();
        let event_handler = self.event_handler.clone();

        // Determine the heal target and the item to show a spinner on.
        // heal_item is (user_data_key, panel_tab_index) for spinner animation.
        let (target, heal_item): (Option<HealTarget>, Option<(String, usize)>) = match self.focus {
            FocusArea::Panel3 => {
                if let Some(panel) = self.left_pane.panel3.active_panel() {
                    let tab =
                        tabs::Panel3Tab::from_index(self.left_pane.panel3.tabs.selected_index());
                    match tab {
                        Some(tabs::Panel3Tab::Bookmarks) => {
                            // Heal selected bookmark
                            let selected = panel.selected();
                            let user_data = selected.and_then(|s| s.user_data.clone());
                            (
                                user_data.clone().map(HealTarget::Bookmark),
                                user_data.map(|ud| (ud, tabs::Panel3Tab::Bookmarks.index())),
                            )
                        }
                        Some(tab @ tabs::Panel3Tab::Collections)
                        | Some(tab @ tabs::Panel3Tab::Tours) => {
                            // Heal all bookmarks in collection/tour
                            let selected = panel.selected();
                            let result = selected.and_then(|s| {
                                if let Some(id) = &s.user_data {
                                    Some((
                                        HealTarget::Collection(id.clone()),
                                        (id.clone(), tab.index()),
                                    ))
                                } else {
                                    // Fallback to name lookup if user_data is missing
                                    let name = s.text().to_string();
                                    self.db.get_collection_by_name(&name).ok().flatten().map(|c| {
                                        (HealTarget::Collection(c.id.clone()), (c.id, tab.index()))
                                    })
                                }
                            });
                            match result {
                                Some((target, item)) => (Some(target), Some(item)),
                                None => (None, None),
                            }
                        }
                        None => (None, None),
                    }
                } else {
                    (None, None)
                }
            }
            FocusArea::Main => {
                // Heal the currently displayed bookmark in preview
                let result =
                    self.right_pane.steps_data.get(self.right_pane.pager_current).map(|step| {
                        let id = step.bookmark.id.clone();
                        (HealTarget::Bookmark(id.clone()), (id, tabs::Panel3Tab::Bookmarks.index()))
                    });
                match result {
                    Some((target, item)) => (Some(target), Some(item)),
                    None => (None, None),
                }
            }
            _ => (None, None),
        };

        let Some(target) = target else {
            // No valid target - show error directly (don't send an event that
            // could interfere with an in-flight heal's spinner lifecycle)
            self.pending_notification = Some(HealNotification {
                message: "Nothing selected to heal".to_string(),
                success: false,
            });
            return;
        };

        // Show spinner on the healing item
        if let Some((ref user_data_key, tab_index)) = heal_item {
            self.add_spinner(user_data_key, tab_index);
        }

        // Spawn a background task to perform the heal.
        // Note: We use spawn_blocking because Database is not Send/Sync (rusqlite limitation).
        // This runs the blocking database operations on a dedicated thread pool.
        tokio::task::spawn_blocking(move || {
            let handle = tokio::runtime::Handle::current();
            handle.block_on(data::perform_heal(db_path, target, event_handler))
        });
    }

    /// Start pushing the currently selected collection to the server.
    ///
    /// This spawns an async background task to perform the push operation.
    /// When complete, a SyncComplete event will be sent with the result.
    pub fn start_push_collection(&mut self) {
        let db_path = self.db.path().to_path_buf();
        let event_handler = self.event_handler.clone();

        // Get the selected collection
        let target = if self.focus == FocusArea::Panel3 {
            if let Some(panel) = self.left_pane.panel3.active_panel() {
                if let Some(Panel3Tab::Collections) =
                    Panel3Tab::from_index(self.left_pane.panel3.tabs.selected_index())
                {
                    panel.selected().and_then(|s| {
                        if let Some(id) = &s.user_data {
                            Some(id.clone())
                        } else {
                            // Fallback to name lookup if user_data is missing
                            let name = s.text().to_string();
                            self.db.get_collection_by_name(&name).ok().flatten().map(|c| c.id)
                        }
                    })
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        let Some(collection_id) = target else {
            let _ = event_handler
                .send(Event::SyncComplete("No collection selected".to_string(), false));
            return;
        };

        // Show a spinner on the collection item being pushed. The spinner is
        // cleared when the SyncComplete event triggers schedule_clear_spinners().
        self.add_spinner(&collection_id, Panel3Tab::Collections.index());

        // Get config for the push operation
        let codemark_dir = match self.db.path().parent() {
            Some(dir) => dir.to_path_buf(),
            None => {
                let _ = event_handler
                    .send(Event::SyncComplete("Failed to get config directory".to_string(), false));
                return;
            }
        };

        let config = Config::load_layered(&codemark_dir);
        let project_root = codemark_dir.parent().unwrap_or(&codemark_dir).to_path_buf();

        // Resolve server URL and token using the helper from core crate
        let (server_url, token) = match codemark_core::sync::resolve_server_and_token(&config) {
            Ok(result) => result,
            Err(e) => {
                let _ = event_handler
                    .send(Event::SyncComplete(format!("Failed to resolve server: {}", e), false));
                return;
            }
        };

        tokio::task::spawn_blocking(move || {
            let handle = tokio::runtime::Handle::current();

            let result = handle.block_on(async {
                let db = Database::open(&db_path)?;
                let collection = db.get_collection_by_id(&collection_id)?.ok_or_else(|| {
                    codemark_core::error::Error::Input("Collection not found".to_string())
                })?;

                let sync_opts = codemark_core::sync::SyncOptions {
                    collection_id: collection.id.clone(),
                    server_url: server_url.clone(),
                    direction: codemark_core::sync::SyncDirection::Push,
                    token,
                    visibility: Some("public".to_string()),
                    title: None,
                    description: None,
                    dry_run: false,
                    save_name: None,
                    db: Some(db),
                    project_root: Some(project_root.to_string_lossy().to_string()),
                    config: Some(config),
                };

                codemark_core::sync::sync(sync_opts).await
            });

            match result {
                Ok(()) => {
                    let _ = event_handler.send(Event::SyncComplete(
                        "Collection published successfully".to_string(),
                        true,
                    ));
                    // Trigger a refresh
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    let _ = event_handler.send(Event::Tick);
                }
                Err(e) => {
                    let _ = event_handler
                        .send(Event::SyncComplete(format!("Push failed: {}", e), false));
                }
            }
        });
    }

    /// Fetch remote tours from the server in the background.
    ///
    /// On completion, merges results into the Tours tab panel.
    pub fn fetch_remote_tours(&mut self) {
        let event_handler = self.event_handler.clone();

        // Resolve server URL and token
        let codemark_dir = match self.db.path().parent() {
            Some(dir) => dir.to_path_buf(),
            None => {
                let _ = event_handler.send(Event::RemoteToursFetchError(
                    "Failed to get config directory".to_string(),
                ));
                return;
            }
        };

        let config = Config::load_layered(&codemark_dir);
        let project_root = codemark_dir.parent().unwrap_or(&codemark_dir).to_path_buf();

        let (server_url, token) = match codemark_core::sync::resolve_server_and_token(&config) {
            Ok(result) => result,
            Err(e) => {
                let _ = event_handler
                    .send(Event::RemoteToursFetchError(format!("Failed to resolve server: {}", e)));
                return;
            }
        };

        // Get repo origin URL for filtering
        let repo_url = codemark_core::git::remote::get_origin_url(&project_root).ok();

        // Record the repo identity so we can discard stale responses if the user switches repos
        self.pending_remote_repo_url = repo_url.clone();

        // Spawn background task to fetch tours
        let request_repo_url = repo_url.clone();
        tokio::spawn(async move {
            let opts = codemark_core::sync::ListRemoteToursOptions { server_url, token, repo_url };

            match codemark_core::sync::list_remote_tours(opts).await {
                Ok(tours) => {
                    let _ = event_handler.send(Event::RemoteToursLoaded(tours, request_repo_url));
                }
                Err(e) => {
                    let _ = event_handler.send(Event::RemoteToursFetchError(e.to_string()));
                }
            }
        });
    }

    /// Start pulling a specific tour from the server by tour_id.
    pub fn start_pull_tour(&mut self, tour_id: String) {
        // Mark the item as pulling (spinner will be shown on tick)
        self.is_pulling_tour = true;
        let user_data_key = format!("remote:{}", tour_id);
        self.add_spinner(&user_data_key, tabs::Panel3Tab::Tours.index());

        let db_path = self.db.path().to_path_buf();
        let event_handler = self.event_handler.clone();

        let codemark_dir = match self.db.path().parent() {
            Some(dir) => dir.to_path_buf(),
            None => {
                let _ = event_handler
                    .send(Event::SyncComplete("Failed to get config directory".to_string(), false));
                return;
            }
        };

        let config = Config::load_layered(&codemark_dir);

        let (server_url, token) = match codemark_core::sync::resolve_server_and_token(&config) {
            Ok(result) => result,
            Err(e) => {
                let _ = event_handler
                    .send(Event::SyncComplete(format!("Failed to resolve server: {}", e), false));
                return;
            }
        };

        tokio::task::spawn_blocking(move || {
            let handle = tokio::runtime::Handle::current();

            let result = handle.block_on(async {
                let db = Database::open(&db_path)?;

                let sync_opts = codemark_core::sync::SyncOptions {
                    collection_id: tour_id,
                    server_url,
                    direction: codemark_core::sync::SyncDirection::Pull,
                    token,
                    visibility: None,
                    title: None,
                    description: None,
                    dry_run: false,
                    save_name: None,
                    db: Some(db),
                    project_root: None,
                    config: None,
                };

                codemark_core::sync::sync(sync_opts).await
            });

            match result {
                Ok(()) => {
                    let _ = event_handler
                        .send(Event::SyncComplete("Tour pulled successfully".to_string(), true));
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    let _ = event_handler.send(Event::Tick);
                }
                Err(e) => {
                    let _ = event_handler
                        .send(Event::SyncComplete(format!("Pull failed: {}", e), false));
                }
            }
        });
    }

    /// Rebuild the Tours panel (index 2) from local DB + cached remote tours.
    /// Extract the original remote tour ID from an imported_from_url.
    /// e.g. "http://127.0.0.1:8080/tours/5efea669-..." -> "5efea669-..."
    fn extract_remote_tour_id(imported_url: &str) -> Option<&str> {
        imported_url.rsplit('/').next()
    }

    fn rebuild_tours_panel(&mut self) {
        let mut local_items = Vec::new();
        // Track which remote tour IDs have been pulled locally
        let mut matched_remote_ids = std::collections::HashSet::new();
        if let Ok(collections) = self.db.list_collections() {
            for (c, count) in collections {
                let is_tour = c.published_at.is_some() || c.imported_from_url.is_some();
                if is_tour {
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
                    let branch = c.created_branch.clone().unwrap_or_else(|| "main".to_string());
                    let item = PanelItem::new(&c.name)
                        .secondary_text(&branch)
                        .metadata(format!("{count} steps"))
                        .health(health)
                        .checkmark(true)
                        .user_data(c.id.clone());
                    // Track both the local ID and the original remote ID
                    if let Some(ref url) = c.imported_from_url
                        && let Some(remote_id) = Self::extract_remote_tour_id(url)
                    {
                        matched_remote_ids.insert(remote_id.to_string());
                    }
                    matched_remote_ids.insert(c.id);
                    local_items.push(item);
                }
            }
        }

        let remote_items: Vec<PanelItem> = self
            .cached_remote_tours
            .iter()
            .filter(|t| !matched_remote_ids.contains(&t.tour_id))
            .map(|t| {
                PanelItem::new(&t.title)
                    .secondary_text(t.updated_at.chars().take(10).collect::<String>())
                    .health(HealthStatus::Unknown)
                    .user_data(format!("remote:{}", t.tour_id))
            })
            .collect();

        let mut all_items = local_items;
        all_items.extend(remote_items);

        if let Some(panel) = self.left_pane.panel3.get_list_panel_mut(2) {
            panel.set_items(all_items);
        }
    }

    /// Refresh all panels from the current active database.
    pub fn refresh_all_panels(&mut self) {
        // 1. Update Panel 1 Accounts (preserving active owner selections)
        let active_owners: Vec<String> = self
            .left_pane
            .panel1
            .panels
            .get(1)
            .and_then(|c| match c {
                TabContent::List(p) => Some(p.active_items()),
                _ => None,
            })
            .unwrap_or_default();

        let account_items = TabbedPanel::build_account_items(&self.registry);
        if let Some(p) = self.left_pane.panel1.get_list_panel_mut(1) {
            p.set_items(account_items);
            // Re-activate previously selected owners
            for owner in &active_owners {
                p.activate_by_user_data(owner);
            }
        }

        // Update Panel 1 Repos (respecting active owner filter)
        if active_owners.is_empty() {
            let repo_items = TabbedPanel::build_repo_items(&self.db, &self.registry);
            if let Some(p) = self.left_pane.panel1.get_list_panel_mut(0) {
                p.set_items(repo_items);
            }
        } else {
            self.update_repos_by_owner();
        }

        // 2. Update Tags/Branches (in-place)
        self.refresh_tags();

        // 3. Update Bookmarks/Collections/Tours (in-place)
        let (tours, collections, bookmarks) = TabbedPanel::build_panel3_items(&self.db);
        if let Some(p) = self.left_pane.panel3.get_list_panel_mut(0) {
            p.set_items(bookmarks);
        }
        if let Some(p) = self.left_pane.panel3.get_list_panel_mut(1) {
            p.set_items(collections);
        }
        if let Some(p) = self.left_pane.panel3.get_list_panel_mut(2) {
            p.set_items(tours);
        }

        // 3b. Spawn background live health resolution for all bookmarks
        if let Ok(all_bookmarks) = self.db.list_bookmarks(&BookmarkFilter::default()) {
            self.spawn_live_health_task(all_bookmarks);
        }

        // 4. Update Step previews (Right Pane) using live resolution
        let current_step = self.right_pane.pager_current;
        if let Some(tour_name) = self.right_pane.active_tour_name.clone() {
            self.right_pane.load_tour_live(&self.db, &tour_name, &mut self.session_cache);
            // Restore step if possible
            if current_step < self.right_pane.pager_total {
                self.right_pane.pager_current = current_step;
                self.right_pane.update_preview(&self.db);
            }
        } else if let Some(bm_id) = self.right_pane.active_bookmark_id.clone() {
            self.right_pane.load_bookmark_live(&self.db, &bm_id, &mut self.session_cache);
        } else if let Ok(collections) = self.db.list_collections() {
            // Default to first tour only if nothing was active
            if let Some((first_tour, _)) = collections.first() {
                let name = first_tour.name.clone();
                self.right_pane.load_tour_live(&self.db, &name, &mut self.session_cache);
            }
        } else {
            // Clear steps if nothing found
            self.right_pane.steps_data.clear();
            self.right_pane.pager_total = 0;
            self.right_pane.pager_current = 0;
        }
    }

    /// Refresh tags in Panel 2 based on the active tab in Panel 3.
    pub fn refresh_tags(&mut self) {
        let active_tab = Panel3Tab::from_index(self.left_pane.panel3.tabs.selected_index())
            .unwrap_or(Panel3Tab::Bookmarks);
        let (tags, branches) = TabbedPanel::build_tags_branches_items(&self.db, active_tab);
        if let Some(p) = self.left_pane.panel2.get_list_panel_mut(0) {
            p.set_items(tags);
        }
        if let Some(p) = self.left_pane.panel2.get_list_panel_mut(1) {
            p.set_items(branches);
        }
    }

    /// Get the current focus area.
    pub fn focus(&self) -> FocusArea {
        self.focus
    }

    /// Get current filters/metadata for the status bar.
    pub fn get_status_metadata(&self) -> Line<'_> {
        let repo_name = self
            .db
            .list_repos()
            .ok()
            .and_then(|repos| repos.first().map(|r| r.repo_name.clone()))
            .unwrap_or_else(|| {
                self.db
                    .path()
                    .parent()
                    .and_then(|p| p.parent())
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "unknown".to_string())
            });

        let active_tags = self
            .left_pane
            .panel2
            .panels
            .first()
            .and_then(|c| match c {
                TabContent::List(p) => Some(p.active_items()),
                _ => None,
            })
            .unwrap_or_default();

        let active_branches = self
            .left_pane
            .panel2
            .panels
            .get(1)
            .and_then(|c| match c {
                TabContent::List(p) => Some(p.active_items()),
                _ => None,
            })
            .unwrap_or_default();

        let mut spans = vec![
            Span::styled("Repo: ", Style::default().fg(Color::DarkGray)),
            Span::styled(repo_name, Style::default().fg(Color::Cyan)),
        ];

        if !active_branches.is_empty() {
            spans.push(Span::styled(" | ", Style::default().fg(Color::Gray)));
            spans.push(Span::styled("Branch: ", Style::default().fg(Color::DarkGray)));
            spans
                .push(Span::styled(active_branches.join(", "), Style::default().fg(Color::Yellow)));
        }

        if !active_tags.is_empty() {
            spans.push(Span::styled(" | ", Style::default().fg(Color::Gray)));
            spans.push(Span::styled(
                active_tags.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(" "),
                Style::default().fg(Color::Magenta),
            ));
        }

        spans.push(Span::styled(" | ", Style::default().fg(Color::Gray)));
        spans.push(Span::styled(
            format!("v{}", env!("CARGO_PKG_VERSION")),
            Style::default().fg(Color::White).bold(),
        ));

        Line::from(spans)
    }

    /// Open the currently selected bookmark or tour step in the editor.
    pub fn open_in_editor(&mut self) {
        // Special handling for Panel3 bookmarks: open directly without StepData
        if self.focus == FocusArea::Panel3
            && let Some(bookmark) = self
                .left_pane
                .panel3
                .active_panel_mut()
                .and_then(|panel| panel.selected())
                .and_then(|item| {
                    // Get the bookmark ID from user_data
                    let bookmark_id = item.user_data.as_ref()?;
                    // Get the bookmark (flatten Result<Option<Bookmark>>)
                    self.db.get_bookmark(bookmark_id).ok().flatten()
                })
        {
            self.open_bookmark_in_editor(bookmark);
            return;
        }

        // Default: get step from the right pane (Main)
        let Some(step) = self.right_pane.steps_data.get(self.right_pane.pager_current) else {
            return;
        };

        let Some(codemark_dir) = self.db.path().parent() else {
            return;
        };
        let config = Config::load_layered(codemark_dir);

        let extension = std::path::Path::new(&step.file_path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");

        // StepData already has resolved absolute path and line numbers (0-indexed)
        let line_start = step.line_number + 1;
        let line_end = step.line_end.map(|e| e + 1).unwrap_or(line_start);

        tracing::debug!(
            target: "codemark::shell",
            file_path = %step.file_path,
            line_start = line_start,
            line_end = line_end,
            "open_in_editor: building editor command"
        );

        if let Some(cmd) = config.open.build_editor_command(
            &step.file_path,
            extension,
            line_start,
            line_end,
            &step.bookmark.id,
        ) {
            tracing::debug!(
                target: "codemark::shell",
                program = %cmd.program,
                args = ?cmd.args,
                should_wait = cmd.should_wait,
                "open_in_editor: pending command set"
            );

            self.pending_command = Some(ExternalCommand {
                program: cmd.program,
                args: cmd.args,
                should_wait: cmd.should_wait,
            });
        }
    }

    /// Open a bookmark directly in the editor (used for Panel3 bookmarks).
    fn open_bookmark_in_editor(&mut self, bookmark: Bookmark) {
        use codemark_core::git::context::resolve_bookmark_file_path;

        let Some(codemark_dir) = self.db.path().parent() else {
            return;
        };
        let config = Config::load_layered(codemark_dir);

        // Resolve the file path and line range from the bookmark's latest resolution
        let (relative_path, line_start, line_end) = if let Some(resolution) =
            self.db.list_resolutions(&bookmark.id, 1).ok().and_then(|mut v| v.pop())
        {
            tracing::debug!(
                target: "codemark::shell",
                bookmark_id = %bookmark.id,
                resolution_id = %resolution.id,
                line_range = ?resolution.line_range,
                file_path = ?resolution.file_path,
                "open_bookmark_in_editor: found resolution"
            );
            let rel_path =
                resolution.file_path.clone().unwrap_or_else(|| bookmark.file_path.clone());
            // line_range is stored as "start-end" (1-indexed) by heal.rs
            let (start, end) = resolution
                .line_range
                .and_then(|r| {
                    let (s_str, e_str) = r.split_once('-')?;
                    let s = s_str.trim().parse::<usize>().ok()?;
                    let e = e_str.trim().parse::<usize>().ok()?;
                    Some((s, e))
                })
                .unwrap_or((1, 1));
            (rel_path, start, end)
        } else {
            // Fallback to bookmark file path (use 1-indexed line 1 as a safe default)
            (bookmark.file_path.clone(), 1, 1)
        };

        // Resolve relative path to absolute path
        let absolute_path = match resolve_bookmark_file_path(&relative_path, self.db.path()) {
            Ok(p) => p.to_string_lossy().to_string(),
            Err(_) => {
                tracing::warn!(
                    target: "codemark::shell",
                    relative_path = %relative_path,
                    "open_bookmark_in_editor: failed to resolve absolute path"
                );
                return;
            }
        };

        let extension =
            std::path::Path::new(&relative_path).extension().and_then(|e| e.to_str()).unwrap_or("");

        // line_start/line_end are already 1-indexed from the stored resolution format
        tracing::debug!(
            target: "codemark::shell",
            absolute_path = %absolute_path,
            line_start = line_start,
            line_end = line_end,
            "open_bookmark_in_editor: building editor command"
        );

        if let Some(cmd) = config.open.build_editor_command(
            &absolute_path,
            extension,
            line_start,
            line_end,
            &bookmark.id,
        ) {
            tracing::debug!(
                target: "codemark::shell",
                program = %cmd.program,
                args = ?cmd.args,
                should_wait = cmd.should_wait,
                "open_bookmark_in_editor: pending command set"
            );

            self.pending_command = Some(ExternalCommand {
                program: cmd.program,
                args: cmd.args,
                should_wait: cmd.should_wait,
            });
        }
    }

    /// Apply a filter to the currently focused panel.
    pub fn apply_filter(&mut self, query: &str) {
        let target_focus = if self.focus == FocusArea::Filter {
            match self.previous_focus {
                Some(FocusArea::Panel1) => FocusArea::Panel1,
                Some(FocusArea::Panel2) => FocusArea::Panel2,
                Some(FocusArea::Panel3) => FocusArea::Panel3,
                Some(FocusArea::Main) => FocusArea::Main,
                // Search focus filters Panel1 (consistent with main.rs filter_target logic)
                Some(FocusArea::Search) => FocusArea::Panel1,
                // Filter focus with no previous focus defaults to Panel3
                Some(FocusArea::Filter) | None => FocusArea::Panel3,
            }
        } else {
            self.focus
        };

        match target_focus {
            FocusArea::Panel1 => {
                if let Some(panel) = self.left_pane.panel1.active_panel_mut() {
                    panel.set_filter(query);
                }
            }
            FocusArea::Panel2 => {
                if let Some(panel) = self.left_pane.panel2.active_panel_mut() {
                    panel.set_filter(query);
                }
            }
            FocusArea::Panel3 => {
                if let Some(panel) = self.left_pane.panel3.active_panel_mut() {
                    panel.set_filter(query);
                }
            }
            FocusArea::Main => {
                if let Some(panel) = self.right_pane.steps.active_panel_mut() {
                    panel.set_filter(query);
                }
            }
            _ => {}
        }
    }

    /// Update the Tours/Collections/Bookmarks panel based on active filters (tags/branches).
    fn update_tours_collections(&mut self) {
        let active_tags = self
            .left_pane
            .panel2
            .panels
            .first()
            .and_then(|c| match c {
                TabContent::List(p) => Some(p.active_items()),
                _ => None,
            })
            .unwrap_or_default();

        let active_branches = self
            .left_pane
            .panel2
            .panels
            .get(1)
            .and_then(|c| match c {
                TabContent::List(p) => Some(p.active_items()),
                _ => None,
            })
            .unwrap_or_default();

        // 1. Update Tours/Collections (Panel 3, tabs 0 and 1)
        if let Ok(collections) = self.db.list_collections() {
            let mut collection_items = Vec::new();
            let mut tour_items = Vec::new();

            for (c, count) in collections {
                // Filter by branch if any are active
                let branch_match = active_branches.is_empty()
                    || c.created_branch.as_ref().is_some_and(|b| active_branches.contains(b));

                // Filter by tags if any are active
                let tag_match = active_tags.is_empty() || {
                    if let Ok(c_tags) = self.db.list_tags_for_collection(&c.id) {
                        c_tags.iter().any(|t| active_tags.contains(&t.tag))
                    } else {
                        false
                    }
                };

                if branch_match && tag_match {
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
                    let is_tour = is_published || c.imported_from_url.is_some();
                    let branch = c.created_branch.unwrap_or_else(|| "main".to_string());
                    let item = PanelItem::new(&c.name)
                        .secondary_text(&branch)
                        .metadata(format!("{count} steps"))
                        .health(health)
                        .published(is_published)
                        .user_data(c.id.clone());

                    collection_items.push(item);
                    if is_tour {
                        let tour_item = PanelItem::new(c.name)
                            .secondary_text(branch)
                            .metadata(format!("{count} steps"))
                            .health(health)
                            .checkmark(true)
                            .user_data(c.id);
                        tour_items.push(tour_item);
                    }
                }
            }

            if let Some(TabContent::List(p)) = self.left_pane.panel3.panels.get_mut(2) {
                p.set_items(tour_items);
            }
            if let Some(TabContent::List(p)) = self.left_pane.panel3.panels.get_mut(1) {
                p.set_items(collection_items);
            }
        }

        // 2. Update Bookmarks (Panel 3, tab 0)
        if let Ok(bookmarks) = self.db.list_bookmarks(&BookmarkFilter::default()) {
            let filtered_bookmarks: Vec<_> = bookmarks
                .into_iter()
                .filter(|bm| {
                    let branch_match = active_branches.is_empty();
                    let tag_match =
                        active_tags.is_empty() || bm.tags.iter().any(|t| active_tags.contains(t));
                    branch_match && tag_match
                })
                .collect();

            let filtered_items: Vec<PanelItem> = filtered_bookmarks
                .iter()
                .map(|bm| {
                    let summary_info = bm
                        .language
                        .parse::<codemark_core::parser::languages::Language>()
                        .ok()
                        .and_then(|lang| {
                            codemark_core::query::summarizer::summarize_query(&bm.query, Some(lang))
                                .ok()
                        });
                    let summary = summary_info
                        .as_ref()
                        .and_then(|s| s.identifier.clone())
                        .unwrap_or_else(|| {
                            if summary_info.is_some() { String::new() } else { bm.query.clone() }
                        });
                    let icon = summary_info
                        .as_ref()
                        .map(|s| codemark_core::query::classifier::get_node_icon(&s.label))
                        .unwrap_or("");
                    let short_path = shorten_path(&bm.file_path, 25);

                    let mut item = PanelItem::new(&short_path)
                        .metadata(bm.created_by.clone().unwrap_or_default())
                        .health(HealthStatus::Unknown)
                        .icon(icon)
                        .user_data(bm.id.clone());
                    if !summary.is_empty() {
                        item = item.emphasis(summary);
                    }
                    item
                })
                .collect();

            if let Some(TabContent::List(p)) = self.left_pane.panel3.panels.get_mut(0) {
                p.set_items(filtered_items);
            }

            // Spawn background live health resolution for filtered bookmarks
            self.spawn_live_health_task(filtered_bookmarks);
        }
    }

    /// Update the Repos panel (Panel 1, tab 0) based on active account (owner) filters.
    ///
    /// Follows the same pattern as `update_tours_collections()`:
    /// reads active owners from the Accounts panel, re-queries repos from the registry,
    /// and filters to only show repos matching the selected owners.
    fn update_repos_by_owner(&mut self) {
        let active_owners = self
            .left_pane
            .panel1
            .panels
            .get(1)
            .and_then(|c| match c {
                TabContent::List(p) => Some(p.active_items()),
                _ => None,
            })
            .unwrap_or_default();

        let repo_items = if active_owners.is_empty() {
            // No filter — show all repos
            TabbedPanel::build_repo_items(&self.db, &self.registry)
        } else {
            // Filter repos by selected owners
            TabbedPanel::build_repo_items(&self.db, &self.registry)
                .into_iter()
                .filter(|item| {
                    item.get_secondary_text()
                        .is_some_and(|owner| active_owners.iter().any(|o| o == owner))
                })
                .collect()
        };

        if let Some(p) = self.left_pane.panel1.get_list_panel_mut(0) {
            p.set_items(repo_items);
        }
    }

    /// Set the focus area.
    /// Current left pane size mode. Exposed for tests.
    #[cfg(test)]
    pub(crate) fn left_pane_size(&self) -> LeftPaneSize {
        self.left_pane_size
    }

    /// Set the left pane size mode directly. Exposed for tests.
    #[cfg(test)]
    pub(crate) fn set_left_pane_size(&mut self, size: LeftPaneSize) {
        self.left_pane_size = size;
        self.left_pane.set_resize_mode(size);
    }

    pub fn set_focus(&mut self, focus: FocusArea) {
        // If we're in filter mode, we don't allow changing focus visually
        // but we allow updating what we'll restore to when exiting filter mode.
        if self.focus == FocusArea::Filter {
            if focus != FocusArea::Filter && focus != FocusArea::Search {
                self.previous_focus = Some(focus);
            }
            return;
        }

        self.focus = focus;
        self.update_focus_state();
    }

    /// Enter filter mode, saving the current focus and clearing it.
    ///
    /// This should be called when the user presses `/` to enter filtering mode.
    /// It saves the current focus area so it can be restored later, and clears
    /// focus to prevent keybindings from interfering with typing.
    pub fn enter_filter_mode(&mut self) {
        // Only save previous focus if it's not already Filter focus
        if self.focus != FocusArea::Filter {
            self.previous_focus = Some(self.focus);
        }
        self.focus = FocusArea::Filter;
        self.update_focus_state();
    }

    /// Exit filter mode, restoring the previous focus.
    ///
    /// This should be called when the user presses ESC or Enter to exit filtering mode.
    /// It restores the focus to the area that was focused before entering filter mode.
    pub fn exit_filter_mode(&mut self) {
        let prev = self.previous_focus.take();
        match prev {
            Some(FocusArea::Filter) | None => {
                // Filter focus with no previous focus defaults to Panel3
                self.focus = FocusArea::Panel3;
            }
            Some(f) => {
                // Restore the previous focus (including Search, which is a valid focus area)
                self.focus = f;
            }
        }
        self.update_focus_state();
    }

    /// Check if keybindings should be handled.
    ///
    /// Returns false when in filter mode (previous_focus is Some), preventing
    /// keybindings from interfering with typing in the filter input.
    fn should_handle_keybindings(&self) -> bool {
        self.previous_focus.is_none()
    }

    /// Cycle to the next focusable area within the current pane.
    pub fn next_focus(&mut self) {
        match self.focus {
            FocusArea::Main => {
                self.right_pane.toggle_internal_focus();
            }
            _ => {
                self.focus = match self.focus {
                    FocusArea::Search => FocusArea::Panel1,
                    FocusArea::Panel1 => FocusArea::Panel2,
                    FocusArea::Panel2 => FocusArea::Panel3,
                    FocusArea::Panel3 => FocusArea::Search,
                    _ => FocusArea::Panel3,
                };
            }
        }
        self.update_focus_state();
    }

    /// Cycle to the previous focusable area within the current pane.
    pub fn previous_focus(&mut self) {
        match self.focus {
            FocusArea::Main => {
                self.right_pane.toggle_internal_focus();
            }
            _ => {
                self.focus = match self.focus {
                    FocusArea::Search => FocusArea::Panel3,
                    FocusArea::Panel3 => FocusArea::Panel2,
                    FocusArea::Panel2 => FocusArea::Panel1,
                    FocusArea::Panel1 => FocusArea::Search,
                    _ => FocusArea::Panel3,
                };
            }
        }
        self.update_focus_state();
    }

    /// Update focus state based on current focus area.
    fn update_focus_state(&mut self) {
        // Reset all focus
        self.left_pane.search.set_focus(false);
        self.left_pane.panel1.set_focus(false);
        self.left_pane.panel2.set_focus(false);
        self.left_pane.panel3.set_focus(false);
        self.right_pane.set_focus(false);

        // If in filter mode, don't set visual focus on any panel
        // This keeps panes visually inactive while the user is typing in the filter bar at the bottom
        if self.focus == FocusArea::Filter {
            return;
        }

        // Moving focus to the preview pane restores the left pane to its
        // default size. Otherwise an expanded left pane (Half/Full from `+`)
        // stays applied behind the preview, leaving the default vertical-split
        // layout unreachable while focus is on Main (preview cycles a separate
        // RightPaneSize state that never touches left_pane_size).
        if self.focus == FocusArea::Main {
            self.left_pane_size = LeftPaneSize::Regular;
        }

        // Sync focused area and resize mode to left pane
        self.left_pane.set_focused_area(self.focus);
        self.left_pane.set_resize_mode(self.left_pane_size);

        // Set focus on current area
        match self.focus {
            FocusArea::Search => {
                self.left_pane.search.set_focus(true);
            }
            FocusArea::Panel1 => {
                self.left_pane.panel1.set_focus(true);
            }
            FocusArea::Panel2 => {
                self.left_pane.panel2.set_focus(true);
            }
            FocusArea::Panel3 => {
                self.left_pane.panel3.set_focus(true);
                self.update_bookmarks_live_preview();
            }
            FocusArea::Main => {
                self.right_pane.set_focus(true);
            }
            FocusArea::Filter => {} // Visual focus is handled by early return above
        }
    }

    /// Copy text to the system clipboard.
    /// Keeps the clipboard context alive for the lifetime of the BrowserLayout
    /// to maintain X11 selection ownership.
    fn copy_to_clipboard(&mut self, text: &str) -> Result<(), String> {
        use copypasta::ClipboardProvider;

        // Create clipboard context lazily and keep it alive
        if self.clipboard.is_none() {
            use copypasta::ClipboardContext;
            self.clipboard = Some(
                ClipboardContext::new()
                    .map_err(|e| format!("Failed to access clipboard: {}", e))?,
            );
        }

        if let Some(ref mut ctx) = self.clipboard {
            ctx.set_contents(text.to_owned())
                .map_err(|e| format!("Failed to set clipboard contents: {}", e))?;
        }
        Ok(())
    }
}

/// Render mode for the browser layout.
enum RenderMode {
    /// Both left and right panes are visible
    Both,
    /// Only left pane is visible
    LeftOnly,
    /// Only right pane is visible (fullscreen)
    RightOnly,
}

impl BrowserLayout {
    /// Render the browser layout.
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        let (left_constraints, render_mode) =
            if self.right_pane_size.is_fullscreen() || self.details_pane_size.is_fullscreen() {
                // Right pane (or details) takes full width
                (vec![Constraint::Percentage(100)], RenderMode::RightOnly)
            } else {
                // Use left pane size to determine layout
                let left_percent = self.left_pane_size.left_width_percent();
                if let Some(right_percent) = self.left_pane_size.right_width_percent() {
                    // Both panes visible
                    (
                        vec![
                            Constraint::Percentage(left_percent),
                            Constraint::Percentage(right_percent),
                        ],
                        RenderMode::Both,
                    )
                } else {
                    // Only left pane visible (right hidden)
                    (vec![Constraint::Percentage(100)], RenderMode::LeftOnly)
                }
            };

        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(left_constraints)
            .split(area);

        let steps_fullscreen = self.right_pane_size.is_fullscreen();

        match render_mode {
            RenderMode::Both => {
                self.left_pane.render(chunks[0], buf);
                self.right_pane.render(chunks[1], buf, false, self.details_pane_size);
            }
            RenderMode::LeftOnly => {
                self.left_pane.render(chunks[0], buf);
            }
            RenderMode::RightOnly => {
                self.right_pane.render(chunks[0], buf, steps_fullscreen, self.details_pane_size);
            }
        }
    }
}

impl Component for BrowserLayout {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.render(area, buf);
    }

    fn handle_event(&mut self, event: &Event) -> bool {
        self.handle_event(event)
    }

    fn focused(&self) -> bool {
        true // Always has some focused component
    }

    fn set_focus(&mut self, _focused: bool) {
        // BrowserLayout always manages its own focus
    }

    fn size_constraints(&self) -> crate::component::SizeConstraints {
        crate::component::SizeConstraints::min(40, 20)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventHandler, EventHandlerConfig};
    use codemark_core::storage::db::Database;

    fn test_layout() -> BrowserLayout {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Database::open(&dir.path().join("test.db")).expect("open db");
        let handler = EventHandler::new(EventHandlerConfig::default()).expect("event handler");
        // Keep the tempdir alive for the duration of the test by leaking it;
        // the OS reclaims it when the test process exits.
        std::mem::forget(dir);
        BrowserLayout::new(db, handler)
    }

    #[test]
    fn focusing_preview_restores_default_left_pane_size() {
        let mut layout = test_layout();

        // Simulate the user expanding the left pane while a left panel is focused.
        layout.set_focus(FocusArea::Panel3);
        layout.set_left_pane_size(LeftPaneSize::Half);
        assert_eq!(layout.left_pane_size(), LeftPaneSize::Half);

        // Pressing Enter on a bookmark/collection moves focus to the preview pane.
        // The default vertical-split layout must be reachable again, so the left
        // pane size resets to Regular instead of staying expanded behind the preview.
        layout.set_focus(FocusArea::Main);
        assert_eq!(layout.left_pane_size(), LeftPaneSize::Regular);
    }

    #[test]
    fn left_pane_size_preserved_across_left_panel_focus() {
        let mut layout = test_layout();

        // Expanding while on Panel3, then moving between left panels must keep the
        // expanded size — only focusing the preview pane resets it.
        layout.set_focus(FocusArea::Panel3);
        layout.set_left_pane_size(LeftPaneSize::Full);
        layout.set_focus(FocusArea::Panel2);
        assert_eq!(layout.left_pane_size(), LeftPaneSize::Full);
    }
}
