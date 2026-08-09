//! Browser layout for the Codemark TUI.
//!
//! This module provides the main browser layout with a left sidebar
//! containing search, repos, and tours, and a right main content area.

mod bindings;
mod config_info;
mod data;
mod dialog;
mod events;
mod left_pane;
mod right_pane;
mod search;
mod tabbed_panel;
mod tabs;
mod types;
mod workspace;

pub use left_pane::LeftPane;
pub use right_pane::{RightPane, RightPaneFocus};
pub use search::{SearchBar, SearchMode};
pub use tabbed_panel::{TabbedPanel, bookmark_to_panel_item};
pub use tabs::{ContentTab, ContextTab, FiltersTab, Tab, TabSelection};
pub use types::{
    ConfirmDialog, DetailsPaneSize, DialogAction, DialogButton, ExternalCommand, FocusArea,
    HealNotification, HealTarget, LeftPaneSize, PreviewPayload, RightPaneSize, SectionConfig,
    SpinningItem, StepData, StepLiveUpdate, StepPreviewMarkdown, TabContent, escape_markdown,
};
pub use workspace::RepoWorkspace;

use crate::component::{Component, HealthStatus, PanelItem};
use crate::event::Event;
use codemark_core::config::Config;
#[cfg(feature = "semantic")]
use codemark_core::embeddings::config::EmbeddingModel;
use codemark_core::engine::bookmark::{Bookmark, BookmarkFilter};
use codemark_core::parser::languages::{Language as CodemarkLanguage, ParseCache};
#[cfg(feature = "semantic")]
use codemark_core::storage::SemanticRepo;
use codemark_core::storage::db::Database;
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Style, Stylize},
    text::{Line, Span},
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Number of ticks (tick rate ≈ 100ms) to wait after the last Content panel selection
/// change before spawning the background preview resolve. Coalesces fast
/// scrolling so only the item the user lands on is resolved.
const PREVIEW_DEBOUNCE_TICKS: usize = 1;

/// Ticks of no pager movement that mark a step move as "settled". A move after
/// this much quiet is a discrete press and renders immediately; while a held
/// key repeats, moves land within this window every tick, so the (synchronous)
/// step render is deferred until a quiet window follows the last move. Must be
/// ≥ 2: a hold produces a move every tick, i.e. a one-tick gap, which must not
/// count as settled.
const STEP_MOVE_SETTLE_TICKS: usize = 2;

/// Number of ticks a preview may stay in flight before the loading indicator is
/// shown. Fast/cached resolves complete (and the result event is applied) within
/// this window, so the previous preview stays put and no spinner flashes; only a
/// genuinely slow resolve reveals the indicator.
const PREVIEW_LOADING_GRACE_TICKS: usize = 2;

/// Number of lines the preview scrolls per `J`/`K` press. These keys scroll the
/// visible preview from any focus, so they step a few lines at a time for quick
/// reading without overshooting.
const PREVIEW_SCROLL_LINES: i32 = 5;

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
    /// Open database connections for the checked repos, with a focused repo that
    /// single-repo operations act on via [`BrowserLayout::db`].
    workspace: RepoWorkspace,
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
    /// Remote tour ids with a pull task currently in flight. The dedup guard for
    /// [`start_pull_tour`](Self::start_pull_tour) reads this instead of the
    /// spinner list, which the global cleanup can drain while an overlapping
    /// pull is still running. An id is released by [`Event::TourPullFinished`].
    pulling_tour_ids: std::collections::HashSet<String>,
    /// Panel items currently showing an animated spinner
    spinning_items: Vec<SpinningItem>,
    /// Tick at which spinners should be cleared (deferred to complete at least one full cycle)
    spinner_clear_at: Option<usize>,
    /// Tick counter for spinner animation
    tick_count: usize,
    /// Last-fetched remote tours (cached to avoid re-fetching after pull)
    cached_remote_tours: Vec<codemark_core::sync::RemoteTourSummary>,
    /// Repos scope (comma-joined `owner/name`) used for the in-flight remote
    /// tours fetch — guards against applying a stale response after the scope changes.
    pending_remote_repos: Option<String>,
    /// Session-level parse cache for live resolution, keyed by language.
    /// Each `ParseCache` is single-language (parser locked at creation).
    /// Used by the synchronous preview paths (init, focus-enter, tours).
    session_cache: HashMap<CodemarkLanguage, ParseCache>,
    /// Parse cache shared with the background preview tasks. Reused across
    /// selections (debounce serializes the tasks, so contention is rare), so
    /// scrolling among bookmarks in the same unchanged file is a cache hit on the
    /// hot async path — not just within a single resolve.
    preview_cache: Arc<Mutex<HashMap<CodemarkLanguage, ParseCache>>>,
    /// Monotonic counter for bookmark preview requests. Bumped on every Content panel
    /// selection change so background results can be matched/discarded.
    preview_seq: u64,
    /// The id of the preview request whose result we currently want to apply.
    /// Results carrying a different id are stale (the selection moved on) and
    /// are dropped — this is how superseded previews are "cancelled".
    active_preview_request: u64,
    /// Epoch for background live-health tasks. Bumped whenever a grammar refresh
    /// (on `FocusGained`) could change how bookmarks resolve, so a `LiveHealthBatch`
    /// computed under an old grammar is discarded instead of overwriting the
    /// freshly-refreshed panels.
    health_generation: u64,

    /// Cached worst-case *live* health per collection id, populated by the
    /// background [`spawn_collection_health_task`](Self::spawn_collection_health_task).
    /// Collection dots and the overview label read from here so they reflect the
    /// current on-disk state of each collection's bookmarks instead of the
    /// persisted snapshot, which goes stale the moment a bookmark's code drifts.
    collection_live_health: HashMap<String, crate::component::HealthStatus>,
    /// Cached *live* health per bookmark id, populated by the background
    /// [`spawn_live_health_task`](Self::spawn_live_health_task) via
    /// [`Event::LiveHealthBatch`]. Bookmark list rows read from here so a tag/branch
    /// filter (or any panel rebuild) renders the already-resolved dot instead of
    /// flashing back to `Unknown` and re-resolving every bookmark against the file.
    /// Refreshed whenever a batch is applied, so it stays current as code drifts.
    bookmark_live_health: HashMap<String, crate::component::HealthStatus>,
    /// ID of the most recent search request, used to discard stale background results.
    pub active_search_request: u64,
    /// The query and mode that produced the search results currently shown in
    /// each content panel, indexed by [`ContentTab`] (`Bookmarks`, `Collections`).
    /// Recorded when a search is dispatched so a refocus reconcile can re-run the
    /// originating search against the current DB, refreshing rows whose record
    /// changed while unfocused — not just dropping ones that were deleted. Only
    /// meaningful while the matching panel `is_search_active`.
    search_contexts: [Option<(String, SearchMode)>; 2],
    /// In-flight background *reconcile* re-runs (a refocus refreshing a semantic
    /// result set against the current DB), mapping each request id to the content
    /// panel index it targets. Their results are applied in place — keeping focus
    /// and selection — rather than jumping onto the results list like a user
    /// search. A map (not a single id) so a refocus can reconcile the Bookmarks
    /// and Collections panels concurrently without one superseding the other.
    reconcile_search_requests: std::collections::HashMap<u64, usize>,
    /// A debounced, not-yet-spawned preview request:
    /// (bookmark_id, label, repo_root, due tick). `repo_root` is the selected
    /// row's owning repo tag (None → focused repo) so the resolve reads from the
    /// right db under multi-select. Coalesces rapid scrolling into a single
    /// resolve once movement settles.
    pending_preview: Option<(String, Option<String>, Option<String>, usize)>,
    /// A spawned-but-not-yet-resolved preview: (label, spawn tick). Used to defer
    /// the loading indicator — the previous preview stays visible and the spinner
    /// only appears if the resolve outlives a short grace period, so fast/cached
    /// resolves never flash an intermediate loading state.
    inflight_preview: Option<(Option<String>, usize)>,
    /// A collection/tour step-preview render is owed but was deferred because a
    /// held key was still repeating. Re-rendering the step's code preview (file
    /// read + syntax highlight + markdown) runs synchronously on the UI thread,
    /// so during a hold it is held back and fired from the tick handler once
    /// movement settles. A single discrete press renders immediately instead and
    /// never sets this.
    step_preview_dirty: bool,
    /// Tick of the most recent pager move, used to tell a discrete press (render
    /// now) from a held-key repeat (defer). `None` until the first move.
    last_step_move_tick: Option<usize>,
    /// Active modal dialog, if any. Captures all input while displayed.
    dialog: Option<ConfirmDialog>,
    /// Cached sync-login state, refreshed by [`Self::update_tours_tab_visibility`].
    /// Read every frame by the status-bar/help binding builders to decide whether
    /// to advertise remote actions (e.g. collection Push), so it must not do disk
    /// IO — hence the cache rather than calling [`Self::is_logged_in`] per render.
    logged_in: bool,
}

/// Maximum results returned by a search-bar query (bookmarks or collections,
/// FTS or semantic). Shared so every search path caps consistently.
const SEARCH_RESULT_LIMIT: usize = 20;

/// Compose the composite cache key for a live-health map entry.
///
/// The live-health caches ([`BrowserLayout::bookmark_live_health`] /
/// [`BrowserLayout::collection_live_health`]) are keyed by `(repo_root, id)` so
/// two checked repos holding items with the *same* id (e.g. a published tour
/// imported into both) keep their own health dot instead of cross-contaminating.
/// The separator `\u{1f}` (ASCII unit separator) can appear in neither a
/// filesystem path nor a ULID, so the key is unambiguous. A missing `repo_root`
/// (in-memory/degenerate db paths) folds to the empty prefix — but populate and
/// lookup must then agree on that same `None`, which they do because both derive
/// the root from the same db path.
fn health_key(repo_root: Option<&str>, id: &str) -> String {
    format!("{}\u{1f}{}", repo_root.unwrap_or(""), id)
}

/// The repo-root key for a database, derived the same way
/// [`crate::browser::tabbed_panel::build_merged_content`] and
/// `RightPane::repo_root_of` derive it: `<root>/.codemark/codemark.db` ->
/// `<root>`. `None` for in-memory/degenerate paths (no grandparent). Kept as a
/// single derivation so the string used at *populate* time matches the one a
/// `PanelItem::repo_root()` tag carries at *lookup* time.
fn db_repo_root(db: &codemark_core::storage::db::Database) -> Option<String> {
    db.path().parent().and_then(|p| p.parent()).map(|p| p.to_string_lossy().to_string())
}

/// Resolve a collection's display health: a cached worst-case *live* status of
/// its bookmarks (`live`) wins when a background pass has computed one;
/// otherwise it falls back to the persisted snapshot. Centralizes the
/// `CollectionHealth → HealthStatus` mapping that every panel-build site shares,
/// so the fallback stays consistent.
fn collection_health_status(
    persisted: Option<codemark_core::engine::bookmark::CollectionHealth>,
    live: Option<crate::component::HealthStatus>,
) -> crate::component::HealthStatus {
    use codemark_core::engine::bookmark::CollectionHealth;
    match live {
        Some(h) => h,
        None => match persisted {
            Some(CollectionHealth::Active) => crate::component::HealthStatus::Healthy,
            Some(CollectionHealth::Drifted) => crate::component::HealthStatus::Drifted,
            Some(CollectionHealth::Stale) => crate::component::HealthStatus::Broken,
            None => crate::component::HealthStatus::Unknown,
        },
    }
}

impl BrowserLayout {
    /// Create a new browser layout.
    pub fn new(db: Database, event_handler: crate::event::EventHandler) -> Self {
        use codemark_core::storage::registry;
        let registry = registry::open_registry().expect("Failed to open global registry");

        // Determine initial focus: if there are no bookmarks in the current database,
        // focus the repos pane (ContextPanel) so the user can select a repository.
        // Otherwise, focus the bookmarks pane (ContentPanel).
        let initial_focus = if db
            .list_bookmarks(&codemark_core::engine::bookmark::BookmarkFilter::default())
            .map(|b| !b.is_empty())
            .unwrap_or(false)
        {
            FocusArea::ContentPanel
        } else {
            FocusArea::ContextPanel
        };

        let left_pane = LeftPane::new(&db, &registry);
        let right_pane = RightPane::new(&db);

        // Derive the repo root from the db path the same way `build_repo_items`
        // does: `<root>/.codemark/codemark.db`.
        // In-memory (`:memory:`) paths have no parent, so fall back to the db
        // path itself as the lookup key — `focus_db()` returns the single db
        // regardless.
        let root = db
            .path()
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| db.path().to_path_buf());
        let workspace = RepoWorkspace::from_db(root, db);

        let mut layout = Self {
            left_pane,
            right_pane,
            focus: initial_focus,
            previous_focus: None,
            workspace,
            registry,
            pending_command: None,
            pending_notification: None,
            event_handler,
            clipboard: None,
            left_pane_size: LeftPaneSize::Regular,
            right_pane_size: RightPaneSize::Regular,
            details_pane_size: DetailsPaneSize::Regular,
            is_pulling_tour: false,
            pulling_tour_ids: std::collections::HashSet::new(),
            spinning_items: Vec::new(),
            spinner_clear_at: None,
            tick_count: 0,
            cached_remote_tours: Vec::new(),
            collection_live_health: HashMap::new(),
            bookmark_live_health: HashMap::new(),
            pending_remote_repos: None,
            session_cache: HashMap::new(),
            preview_cache: Arc::new(Mutex::new(HashMap::new())),
            preview_seq: 0,
            active_preview_request: 0,
            health_generation: 0,
            active_search_request: 0,
            search_contexts: [None, None],
            reconcile_search_requests: std::collections::HashMap::new(),
            pending_preview: None,
            inflight_preview: None,
            step_preview_dirty: false,
            last_step_move_tick: None,
            dialog: None,
            // Corrected immediately by update_tours_tab_visibility below.
            logged_in: false,
        };
        layout.update_focus_state();
        layout.sync_steps_tab_label();
        layout.update_tours_tab_visibility();

        // Spawn background live health resolution so bookmark dots update on startup
        if let Ok(all_bookmarks) =
            layout.db().list_bookmarks(&codemark_core::engine::bookmark::BookmarkFilter::default())
        {
            layout.spawn_live_health_task(all_bookmarks);
            layout.spawn_collection_health_task();
        }

        layout
    }

    /// The focused repo's database. Single-repo operations read/write through
    /// this accessor so they act on the currently-focused repo.
    fn db(&self) -> &codemark_core::storage::db::Database {
        self.workspace.focus_db()
    }

    /// Load a collection overview and override its health label with the cached
    /// worst-case *live* status of the collection's bookmarks, so the preview
    /// border label reflects the current on-disk state instead of the persisted
    /// snapshot. When no live pass has run yet, the persisted value (set by
    /// [`RightPane::load_collection_overview`]) stands until the background task
    /// arrives and the handler corrects it.
    fn load_collection_overview_live(&mut self, id: &str) {
        self.right_pane.load_collection_overview(self.workspace.focus_db(), id);
        let key = health_key(db_repo_root(self.workspace.focus_db()).as_deref(), id);
        match self.collection_live_health.get(&key).copied() {
            // A real live status overrides the persisted snapshot.
            Some(h) if h != HealthStatus::Unknown => self.right_pane.overview_health = Some(h),
            // Unknown means the collection has no live bookmarks (empty or
            // all-archived): there is no health to report, so show no label
            // rather than the "Error" label Unknown would otherwise render.
            Some(_) => self.right_pane.overview_health = None,
            None => {}
        }
    }

    /// Update the right-pane live preview for the active Content panel tab + selection.
    ///
    /// Bookmarks show the bookmark's code content; Collections show a live
    /// collection overview (metadata + steps). Used when focus enters Content panel
    /// and when the active Content panel tab changes, so the preview never lingers on
    /// stale content from a different tab.
    fn update_content_live_preview(&mut self) {
        if self.focus != FocusArea::ContentPanel {
            return;
        }

        let Some(tab) = ContentTab::from_index(self.left_pane.content_panel.tabs.selected_index())
        else {
            return;
        };

        let selected_id = self
            .left_pane
            .content_panel
            .active_panel()
            .and_then(|panel| panel.selected())
            .and_then(|selected| selected.user_data.clone());

        if let Some(id) = selected_id {
            self.preview_content_item(tab, &id);
        }
    }

    /// Whether the user has usable sync credentials (a resolvable server *and* a
    /// token). Drives whether the Tours tab — which lists remote tours — is shown.
    ///
    /// Requires a token, not just a resolvable URL: a direct `default_server`
    /// URL resolves successfully with no token, so checking only `is_ok()` would
    /// keep the tab visible after `codemark auth logout` and let remote actions
    /// run without credentials (they'd just fail).
    fn is_logged_in(&self) -> bool {
        if let Some(dir) = self.db().path().parent() {
            let config = codemark_core::config::Config::load_layered(dir);
            codemark_core::sync::resolve_server_and_token(&config)
                .map(|(_, token)| token.is_some())
                .unwrap_or(false)
        } else {
            false
        }
    }

    /// Show or hide the Content panel Tours tab based on login state. The Tours tab is
    /// the third tab (index 2); hiding it leaves Bookmarks + Collections. If the
    /// Tours tab was selected when hidden, the selection clamps back to
    /// Collections, so the right-pane preview is refreshed to match.
    ///
    /// Tours are also hidden whenever more than one repo is checked: they are backed
    /// by per-repo local collections and a remote server scoped to a single repo set,
    /// and merging tours across repos is out of scope. With exactly one repo checked
    /// Tours behave exactly as before.
    pub(super) fn update_tours_tab_visibility(&mut self) {
        self.logged_in = self.is_logged_in();
        let visible_count = if self.logged_in && !self.workspace.is_multi() { 3 } else { 2 };
        let previous = self.left_pane.content_panel.tabs.selected_index();
        self.left_pane.content_panel.tabs.set_visible_count(visible_count);
        let current = self.left_pane.content_panel.tabs.selected_index();

        if current == previous {
            return;
        }

        // Selection was clamped off the now-hidden Tours tab. Drop any remote-tour
        // overview first: the Tours tab is gone, so its preview must not linger.
        // (Done unconditionally because the clamped-to tab may have no selection
        // to load, in which case the branch below wouldn't refresh the pane.)
        if self.right_pane.active_remote_tour_id.is_some() {
            self.right_pane.clear_preview_state(self.workspace.focus_db());
        }

        // Then refresh the preview for the clamped-to tab's current selection.
        let Some(tab) = ContentTab::from_index(current) else {
            return;
        };
        let selected_id = self
            .left_pane
            .content_panel
            .active_panel()
            .and_then(|panel| panel.selected())
            .and_then(|selected| selected.user_data.clone());
        if let Some(id) = selected_id {
            self.on_content_selection_changed(tab, &id);
        }
    }

    /// Route a Content panel item (by user-data id) to the appropriate live preview.
    ///
    /// Synchronous: used for focus-enter / tab-change previews where an instant,
    /// fully-rendered pane is expected (and exercised by the e2e snapshots). The
    /// hot scrolling path uses [`on_content_selection_changed`] instead.
    pub(super) fn preview_content_item(&mut self, tab: ContentTab, id: &str) {
        match tab {
            ContentTab::Bookmarks => {
                // Resolve from the selected row's owning repo (owned copy before
                // the &mut borrow of the right pane), so the preview loads from
                // the right db under multi-select.
                let repo_root = self
                    .left_pane
                    .content_panel
                    .active_panel()
                    .and_then(|panel| panel.selected())
                    .and_then(|s| s.repo_root().map(str::to_string));
                self.right_pane.load_bookmark_live(
                    self.workspace.db_for(repo_root.as_deref()),
                    id,
                    &mut self.session_cache,
                );
            }
            ContentTab::Collections => {
                self.load_collection_overview_live(id);
            }
            ContentTab::Tours => self.preview_tour_item(id),
        }
    }

    /// Render an overview for a Content panel Tours item. The Tours tab mixes local
    /// (pulled) tours, keyed by their collection ID, with remote tours keyed as
    /// `remote:<tour_id>`. Remote items render server metadata so the user can
    /// preview a tour before pulling; local items reuse the collection overview.
    fn preview_tour_item(&mut self, id: &str) {
        if let Some(tour_id) = id.strip_prefix("remote:") {
            match self.cached_remote_tours.iter().find(|t| t.tour_id == tour_id) {
                Some(tour) => self.right_pane.load_tour_overview(tour),
                // No cached summary (e.g. stale selection) — clear stale preview.
                None => self.right_pane.clear_preview_state(self.workspace.focus_db()),
            }
        } else {
            self.load_collection_overview_live(id);
        }
    }

    /// Handle a Content panel selection change from keyboard/mouse navigation.
    ///
    /// For Bookmarks this is the lag-prone path (every up/down used to block the
    /// event loop on a tree-sitter resolve + file read + markdown render), so it
    /// is now debounced and resolved on a background task — see
    /// [`request_bookmark_preview`](Self::request_bookmark_preview). Collections
    /// are cheap (no parse) and stay synchronous.
    pub(super) fn on_content_selection_changed(&mut self, tab: ContentTab, id: &str) {
        match tab {
            ContentTab::Bookmarks => self.request_bookmark_preview(id),
            ContentTab::Collections => self.load_collection_overview_live(id),
            ContentTab::Tours => self.preview_tour_item(id),
        }
    }

    /// Queue a debounced background preview for the given bookmark.
    ///
    /// Moves the selection instantly: it records a pending request but leaves the
    /// *previous* preview on screen (no immediate loading state — see
    /// [`inflight_preview`](Self::inflight_preview)). The resolve is spawned later
    /// from the tick handler once movement settles
    /// (see [`maybe_spawn_pending_preview`]), so a fast scroll through many items
    /// resolves only the final one.
    fn request_bookmark_preview(&mut self, id: &str) {
        // Re-entering the same already-loaded bookmark needs no work.
        if !self.right_pane.is_loading()
            && self.inflight_preview.is_none()
            && self.right_pane.active_bookmark_id.as_deref() == Some(id)
        {
            // Cancel any debounced request for a *different* bookmark that hasn't
            // fired yet (e.g. A→B→A within one tick): otherwise B's queued task
            // would still spawn and overwrite the already-correct A preview.
            self.pending_preview = None;
            return;
        }

        // Bump the request id so any in-flight/older result is treated as stale.
        self.preview_seq = self.preview_seq.wrapping_add(1);
        self.active_preview_request = self.preview_seq;

        // Cancel any in-flight loading state for the previous selection. This
        // prevents the grace period from expiring and showing the wrong file
        // label while the new selection is still debouncing.
        self.inflight_preview = None;
        if self.right_pane.is_loading() {
            self.right_pane.finish_loading();
        }

        // Label the (eventual) loading indicator with the selected row's path,
        // and capture its owning repo tag (both cheap, read from the
        // already-rendered list item) so the resolve targets the right db.
        let (label, repo_root) = self
            .left_pane
            .content_panel
            .active_panel()
            .and_then(|panel| panel.selected())
            .map(|selected| {
                (Some(selected.text().to_string()), selected.repo_root().map(str::to_string))
            })
            .unwrap_or((None, None));

        // Debounce: fire one tick from now; rapid moves keep resetting this.
        self.pending_preview =
            Some((id.to_string(), label, repo_root, self.tick_count + PREVIEW_DEBOUNCE_TICKS));
    }

    /// Invalidate any pending or in-flight bookmark preview so a stale async
    /// result can't clobber a subsequent *synchronous* render.
    ///
    /// [`update_content_live_preview`](Self::update_content_live_preview) loads
    /// the selected bookmark synchronously and authoritatively. If a debounced
    /// or in-flight resolve from an earlier navigation is still outstanding, its
    /// `PreviewReady` would otherwise arrive afterward and overwrite the pane
    /// with a now-stale preview. Bumping the request id makes that result get
    /// dropped on arrival (id mismatch), and clearing the debounce/in-flight
    /// bookkeeping stops a queued task from spawning. Mirrors the cancellation
    /// in [`request_bookmark_preview`](Self::request_bookmark_preview).
    pub(super) fn cancel_inflight_preview(&mut self) {
        self.preview_seq = self.preview_seq.wrapping_add(1);
        self.active_preview_request = self.preview_seq;
        self.pending_preview = None;
        self.inflight_preview = None;
        if self.right_pane.is_loading() {
            self.right_pane.finish_loading();
        }
    }

    /// Sync the right pane's first tab label based on the active Content panel tab.
    /// Shows "Content" when on Bookmarks (single items), "Steps" otherwise (collections/tours).
    fn sync_steps_tab_label(&mut self) {
        let content_tab =
            ContentTab::from_index(self.left_pane.content_panel.tabs.selected_index());
        let label = match content_tab {
            Some(ContentTab::Bookmarks) => "Content",
            _ => "Steps",
        };
        tracing::debug!(
            target: "codemark::ui",
            ?content_tab,
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
    pub fn execute_search(&mut self) {
        let query = self.left_pane.search.query().to_string();
        if query.is_empty() {
            // An empty query restores the full list — the same effect as Esc's
            // "clear search", but without moving focus, so the user can start a
            // new query immediately. Drop any in-flight search, clear the
            // recorded search contexts, and rebuild the panels from the DB.
            self.reconcile_search_requests.clear();
            self.active_search_request = self.active_search_request.wrapping_add(1);
            self.search_contexts = [None, None];
            self.refresh_all_panels();
            return;
        }

        // A user-initiated search supersedes any in-flight reconcile; drop their
        // markers so late results aren't mistaken for an in-place refresh.
        self.reconcile_search_requests.clear();
        self.active_search_request = self.active_search_request.wrapping_add(1);
        let request_id = self.active_search_request;

        let mode = self.left_pane.search.mode();

        // The active Content panel tab decides what the query searches: the
        // Collections tab searches collections (name/description/tags), every
        // other tab searches bookmarks. Record the query+mode against the target
        // panel so a later refocus reconcile can re-run this exact search.
        if let Some(ContentTab::Collections) =
            ContentTab::from_index(self.left_pane.content_panel.tabs.selected_index())
        {
            self.search_contexts[ContentTab::Collections.index()] = Some((query.clone(), mode));
            self.execute_collection_search(request_id, mode, query);
            return;
        }
        self.search_contexts[ContentTab::Bookmarks.index()] = Some((query.clone(), mode));
        self.execute_bookmark_search(request_id, mode, query);
    }

    /// The focus repo's `(display_name, root)` tag, matching what a merged row
    /// and the live-health cache use. `root` is the empty string for
    /// in-memory/degenerate db paths (mirrors [`db_repo_root`] folding `None` to
    /// the empty prefix), so a focus-tagged hit's health key agrees with the
    /// focus keying used before fan-out.
    fn focus_repo_tag(&self) -> (String, String) {
        let root = db_repo_root(self.db()).unwrap_or_default();
        let name = crate::browser::tabbed_panel::repo_display_name(std::path::Path::new(&root));
        (name, root)
    }

    /// Run an FTS bookmark search across every checked repo, tagging each hit
    /// with its owning repo. Returns the first db error encountered. In
    /// single-repo mode this is exactly one `search_bookmarks` call, identical to
    /// the pre-fan-out behavior.
    fn fts_bookmark_hits(
        workspace: &RepoWorkspace,
        query: &str,
    ) -> codemark_core::error::Result<Vec<crate::event::BookmarkHit>> {
        let mut hits = Vec::new();
        for (root, db) in workspace.dbs() {
            let bookmarks = db.search_bookmarks(Some(query), None, None, None, None, None, None)?;
            let repo_name = crate::browser::tabbed_panel::repo_display_name(root);
            let repo_root = root.to_string_lossy().into_owned();
            for bookmark in bookmarks {
                hits.push(crate::event::BookmarkHit {
                    repo_name: repo_name.clone(),
                    repo_root: repo_root.clone(),
                    bookmark,
                });
            }
        }
        Ok(hits)
    }

    /// Run an FTS collection search across every checked repo, tagging each hit
    /// with its owning repo and carrying the collection's bookmark count. Each
    /// repo's results are truncated to [`SEARCH_RESULT_LIMIT`], matching the
    /// single-repo behavior. Returns the first db error encountered.
    fn fts_collection_hits(
        workspace: &RepoWorkspace,
        query: &str,
    ) -> codemark_core::error::Result<Vec<crate::event::CollectionHit>> {
        let mut hits = Vec::new();
        for (root, db) in workspace.dbs() {
            let mut collections = db.search_collections(Some(query), None)?;
            collections.truncate(SEARCH_RESULT_LIMIT);
            let repo_name = crate::browser::tabbed_panel::repo_display_name(root);
            let repo_root = root.to_string_lossy().into_owned();
            for (collection, count) in collections {
                hits.push(crate::event::CollectionHit {
                    repo_name: repo_name.clone(),
                    repo_root: repo_root.clone(),
                    collection,
                    count,
                });
            }
        }
        Ok(hits)
    }

    /// Execute a bookmark search for `query`, emitting `SearchResults` (or
    /// `SearchError`). FTS runs synchronously (a fast `LIKE` scan); semantic
    /// search runs on a blocking task since it loads an embedding model. Split
    /// out of [`Self::execute_search`] so a refocus reconcile can re-run the
    /// bookmark search directly, independent of the selected tab.
    fn execute_bookmark_search(&mut self, request_id: u64, mode: SearchMode, query: String) {
        let event_handler = self.event_handler.clone();

        match mode {
            SearchMode::Fts => {
                // FTS search can be done synchronously as it's usually fast.
                // Fan out across every checked repo and tag each hit with its
                // owning repo so selection/preview/health resolve per repo. In
                // single-repo mode this is exactly one db, identical to before.
                match Self::fts_bookmark_hits(&self.workspace, &query) {
                    Ok(hits) => {
                        let _ = event_handler.send(Event::SearchResults { request_id, hits });
                    }
                    Err(e) => {
                        let _ = event_handler
                            .send(Event::SearchError { request_id, msg: e.to_string() });
                    }
                }
            }
            // When built without the `semantic` feature the search bar can never
            // enter Semantic mode (the toggle/Ctrl+s are gone), so this arm is
            // unreachable; keep it as a no-op that drops the captured values.
            #[cfg(not(feature = "semantic"))]
            SearchMode::Semantic => {
                let _ = (request_id, query);
            }
            #[cfg(feature = "semantic")]
            SearchMode::Semantic => {
                // Model/metric/threshold are resolved once from the FOCUS repo's
                // config and applied to every checked repo (v1 assumes a shared
                // embedding model across repos — see the design's known limits).
                let Some(codemark_dir) = self.db().path().parent() else {
                    return;
                };
                let config = Config::load_layered(codemark_dir);

                // Capture every checked repo's (name, root, db path) before the
                // spawn; Database isn't Send, so the task reopens by path.
                let repos = self.semantic_repo_targets();

                tokio::task::spawn_blocking(move || {
                    let model = config
                        .semantic
                        .model
                        .as_deref()
                        .and_then(|m| m.parse::<EmbeddingModel>().ok())
                        .unwrap_or(EmbeddingModel::AllMiniLmL6V2);
                    let distance_metric = config.semantic.get_distance_metric();
                    let threshold = config.semantic.effective_threshold();
                    let models_dir = config.semantic.get_models_dir();
                    let semantic_repo =
                        SemanticRepo::with_config(models_dir, model, distance_metric, threshold);

                    let handle = tokio::runtime::Handle::current();
                    // Embed the query ONCE (loads the model), then search each
                    // repo's db with the same vector — no per-repo model reload.
                    let embedding = match tokio::task::block_in_place(|| {
                        handle.block_on(semantic_repo.embed_query(&query))
                    }) {
                        Ok(e) => e,
                        Err(e) => {
                            let _ = event_handler
                                .send(Event::SearchError { request_id, msg: e.to_string() });
                            return;
                        }
                    };

                    // Score hits across all checked repos, resolving each to its
                    // full bookmark from its own db (open once per repo).
                    let mut scored: Vec<(f64, crate::event::BookmarkHit)> = Vec::new();
                    for (repo_name, repo_root, db_path) in &repos {
                        let Ok(db) = Database::open(db_path) else { continue };
                        // A repo with no vec index / a dimension mismatch simply
                        // contributes nothing rather than failing the whole query.
                        let Ok(results) = semantic_repo.search_prepared(
                            db.conn(),
                            &embedding,
                            SEARCH_RESULT_LIMIT,
                            threshold,
                        ) else {
                            continue;
                        };
                        for r in results {
                            if let Ok(Some(bookmark)) = db.get_bookmark(&r.id) {
                                scored.push((
                                    r.distance,
                                    crate::event::BookmarkHit {
                                        repo_name: repo_name.clone(),
                                        repo_root: repo_root.clone(),
                                        bookmark,
                                    },
                                ));
                            }
                        }
                    }
                    // Global rank by ascending distance, cap at the limit.
                    scored
                        .sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
                    scored.truncate(SEARCH_RESULT_LIMIT);
                    let hits = scored.into_iter().map(|(_, hit)| hit).collect();
                    let _ = event_handler.send(Event::SearchResults { request_id, hits });
                });
            }
        }
    }

    #[cfg(feature = "semantic")]
    /// The (repo_name, repo_root, db_path) of every checked repo, for capturing
    /// before a `spawn_blocking` semantic search (Database isn't Send, so the
    /// task reopens each db by path). In single-repo mode this is one entry.
    fn semantic_repo_targets(&self) -> Vec<(String, String, std::path::PathBuf)> {
        self.workspace
            .dbs()
            .map(|(root, db)| {
                let name = crate::browser::tabbed_panel::repo_display_name(root);
                (name, root.to_string_lossy().into_owned(), db.path().to_path_buf())
            })
            .collect()
    }

    /// Execute a search over collections (name/description/tags) for the current
    /// query, emitting `CollectionSearchResults` (or `SearchError`).
    ///
    /// FTS runs synchronously (a fast `LIKE` scan); semantic search runs on a
    /// blocking task like the bookmark path, since it loads an embedding model.
    fn execute_collection_search(&mut self, request_id: u64, mode: SearchMode, query: String) {
        let event_handler = self.event_handler.clone();
        // Log the mode, not the query text (queries may be sensitive).
        let mode_label = match mode {
            SearchMode::Fts => "fts",
            SearchMode::Semantic => "semantic",
        };
        tracing::debug!(
            target: "codemark::ui",
            request_id,
            mode = mode_label,
            "collection search dispatched"
        );

        match mode {
            // Fan out across every checked repo, tagging each hit with its
            // owning repo; single-repo mode is exactly one db as before.
            SearchMode::Fts => match Self::fts_collection_hits(&self.workspace, &query) {
                Ok(hits) => {
                    tracing::debug!(
                        target: "codemark::ui",
                        request_id,
                        mode = mode_label,
                        result_count = hits.len(),
                        "collection search completed"
                    );
                    let _ = event_handler.send(Event::CollectionSearchResults { request_id, hits });
                }
                Err(e) => {
                    tracing::debug!(
                        target: "codemark::ui",
                        request_id,
                        mode = mode_label,
                        error = %e,
                        "collection search failed"
                    );
                    let _ =
                        event_handler.send(Event::SearchError { request_id, msg: e.to_string() });
                }
            },
            // Unreachable without the `semantic` feature (see the bookmark path).
            #[cfg(not(feature = "semantic"))]
            SearchMode::Semantic => {
                let _ = (request_id, query);
            }
            #[cfg(feature = "semantic")]
            SearchMode::Semantic => {
                // Model/metric/threshold resolved once from the focus repo's
                // config, applied to every checked repo (v1 shared-model assumption).
                let Some(codemark_dir) = self.db().path().parent() else {
                    return;
                };
                let config = Config::load_layered(codemark_dir);
                let repos = self.semantic_repo_targets();

                tokio::task::spawn_blocking(move || {
                    let model = config
                        .semantic
                        .model
                        .as_deref()
                        .and_then(|m| m.parse::<EmbeddingModel>().ok())
                        .unwrap_or(EmbeddingModel::AllMiniLmL6V2);
                    let distance_metric = config.semantic.get_distance_metric();
                    let threshold = config.semantic.effective_threshold();
                    let models_dir = config.semantic.get_models_dir();
                    let semantic_repo =
                        SemanticRepo::with_config(models_dir, model, distance_metric, threshold);

                    let handle = tokio::runtime::Handle::current();
                    // Embed once, then search each repo's collection index with
                    // the same vector.
                    let embedding = match tokio::task::block_in_place(|| {
                        handle.block_on(semantic_repo.embed_query(&query))
                    }) {
                        Ok(e) => e,
                        Err(e) => {
                            let _ = event_handler
                                .send(Event::SearchError { request_id, msg: e.to_string() });
                            return;
                        }
                    };

                    let mut scored: Vec<(f64, crate::event::CollectionHit)> = Vec::new();
                    for (repo_name, repo_root, db_path) in &repos {
                        let Ok(db) = Database::open(db_path) else { continue };
                        let Ok(results) = semantic_repo.search_collections_prepared(
                            db.conn(),
                            &embedding,
                            SEARCH_RESULT_LIMIT,
                            threshold,
                        ) else {
                            continue;
                        };
                        for r in results {
                            if let Ok(Some(collection)) = db.get_collection_by_id(&r.id) {
                                let count = db
                                    .list_bookmarks_in_collection(&collection.id)
                                    .map(|b| b.len())
                                    .unwrap_or(0);
                                scored.push((
                                    r.distance,
                                    crate::event::CollectionHit {
                                        repo_name: repo_name.clone(),
                                        repo_root: repo_root.clone(),
                                        collection,
                                        count,
                                    },
                                ));
                            }
                        }
                    }
                    scored
                        .sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
                    scored.truncate(SEARCH_RESULT_LIMIT);
                    let hits: Vec<_> = scored.into_iter().map(|(_, hit)| hit).collect();
                    tracing::debug!(
                        target: "codemark::ui",
                        request_id,
                        mode = mode_label,
                        result_count = hits.len(),
                        "collection search completed"
                    );
                    let _ = event_handler.send(Event::CollectionSearchResults { request_id, hits });
                });
            }
        }
    }

    /// Reset the caches and derived state that become stale after the workspace
    /// scope (or focus) changes, then rebuild every panel.
    ///
    /// Called by the multi-select Repos toggle so it takes the same clears as
    /// any scope change: the live-health caches, the health epoch bump (so an
    /// in-flight task for a dropped repo can't repopulate a freshly-cleared
    /// cache or panel dot with a stale status), the remote-tour caches, and the
    /// head commit.
    pub(super) fn after_scope_change(&mut self) {
        self.right_pane.refresh_head_commit(self.workspace.focus_db());
        self.collection_live_health.clear();
        self.bookmark_live_health.clear();
        self.health_generation = self.health_generation.wrapping_add(1);
        self.cached_remote_tours.clear();
        self.pending_remote_repos = None;
        self.right_pane.active_remote_tour_id = None;
        self.refresh_all_panels();
    }

    /// Minimum number of ticks a spinner must run before clearing (one visual cycle).
    const SPINNER_MIN_TICKS: usize = 5;

    /// Add a spinner to a panel item. The spinner animates on each tick.
    fn add_spinner(&mut self, user_data_key: &str, tab_index: usize) {
        if let Some(panel) = self.left_pane.content_panel.get_list_panel_mut(tab_index) {
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
            if let Some(panel) = self.left_pane.content_panel.get_list_panel_mut(item.tab_index) {
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
        let event_handler = self.event_handler.clone();

        // Determine the heal target and the item to show a spinner on, plus the
        // owning repo of the selection so the heal runs against that repo's db.
        // heal_item is (user_data_key, panel_tab_index) for spinner animation.
        let (target, heal_item, selected_repo_root): (
            Option<HealTarget>,
            Option<(String, usize)>,
            Option<String>,
        ) = match self.focus {
            FocusArea::ContentPanel => {
                // Capture the selected row's owning repo up front (owned) so the
                // db-path resolution below doesn't overlap the panel borrow.
                let selected_repo_root = self
                    .left_pane
                    .content_panel
                    .active_panel()
                    .and_then(|panel| panel.selected())
                    .and_then(|s| s.repo_root().map(str::to_string));
                if let Some(panel) = self.left_pane.content_panel.active_panel() {
                    let tab = tabs::ContentTab::from_index(
                        self.left_pane.content_panel.tabs.selected_index(),
                    );
                    match tab {
                        Some(tabs::ContentTab::Bookmarks) => {
                            // Heal selected bookmark
                            let selected = panel.selected();
                            let user_data = selected.and_then(|s| s.user_data.clone());
                            (
                                user_data.clone().map(HealTarget::Bookmark),
                                user_data.map(|ud| (ud, tabs::ContentTab::Bookmarks.index())),
                                selected_repo_root,
                            )
                        }
                        Some(tab @ tabs::ContentTab::Collections)
                        | Some(tab @ tabs::ContentTab::Tours) => {
                            // Heal all bookmarks in collection/tour
                            let selected = panel.selected();
                            let result = selected.and_then(|s| {
                                if let Some(id) = &s.user_data {
                                    Some((
                                        HealTarget::Collection(id.clone()),
                                        (id.clone(), tab.index()),
                                    ))
                                } else {
                                    // Fallback to name lookup if user_data is missing.
                                    // Resolve against the selected row's repo db.
                                    let name = s.text().to_string();
                                    self.workspace
                                        .db_for(selected_repo_root.as_deref())
                                        .get_collection_by_name(&name)
                                        .ok()
                                        .flatten()
                                        .map(|c| {
                                            (
                                                HealTarget::Collection(c.id.clone()),
                                                (c.id, tab.index()),
                                            )
                                        })
                                }
                            });
                            match result {
                                Some((target, item)) => {
                                    (Some(target), Some(item), selected_repo_root)
                                }
                                None => (None, None, None),
                            }
                        }
                        None => (None, None, None),
                    }
                } else {
                    (None, None, None)
                }
            }
            FocusArea::Main => {
                // Heal the currently displayed bookmark in preview. Its owning repo
                // is the active preview's repo (tracked on the right pane).
                let repo_root = self.right_pane.active_repo_root.clone();
                let result =
                    self.right_pane.steps_data.get(self.right_pane.pager_current).map(|step| {
                        let id = step.bookmark.id.clone();
                        (
                            HealTarget::Bookmark(id.clone()),
                            (id, tabs::ContentTab::Bookmarks.index()),
                        )
                    });
                match result {
                    Some((target, item)) => (Some(target), Some(item), repo_root),
                    None => (None, None, None),
                }
            }
            _ => (None, None, None),
        };

        // Resolve the db path from the selected item's repo (None → focused repo).
        let db_path = self.workspace.db_for(selected_repo_root.as_deref()).path().to_path_buf();

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
        let db_path = self.db().path().to_path_buf();
        let event_handler = self.event_handler.clone();

        // Get the selected collection
        let target = if self.focus == FocusArea::ContentPanel {
            if let Some(panel) = self.left_pane.content_panel.active_panel() {
                if let Some(ContentTab::Collections) =
                    ContentTab::from_index(self.left_pane.content_panel.tabs.selected_index())
                {
                    panel.selected().and_then(|s| {
                        if let Some(id) = &s.user_data {
                            Some(id.clone())
                        } else {
                            // Fallback to name lookup if user_data is missing
                            let name = s.text().to_string();
                            self.db().get_collection_by_name(&name).ok().flatten().map(|c| c.id)
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
        self.add_spinner(&collection_id, ContentTab::Collections.index());

        // Get config for the push operation
        let codemark_dir = match self.db().path().parent() {
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
        let codemark_dir = match self.db().path().parent() {
            Some(dir) => dir.to_path_buf(),
            None => {
                let _ = event_handler.send(Event::RemoteToursFetchError(
                    "Failed to get config directory".to_string(),
                ));
                return;
            }
        };

        let config = Config::load_layered(&codemark_dir);

        let (server_url, token) = match codemark_core::sync::resolve_server_and_token(&config) {
            Ok(result) => result,
            Err(e) => {
                let _ = event_handler
                    .send(Event::RemoteToursFetchError(format!("Failed to resolve server: {}", e)));
                return;
            }
        };

        // Build the repo scope from the repos *selected* (active) in the Repos
        // panel (Context panel tab 0) — not every repo in the registry. Each item carries
        // the owner (secondary text) and name (primary text). `GET /tours` is an
        // authorization-scoped lookup, so we name the selected repos in one
        // `repos=a/b,c/d` request rather than one request per repo.
        let mut repos: Vec<String> = Vec::new();
        if let Some(TabContent::List(panel)) = self.left_pane.context_panel.panels.first() {
            for item in panel.all_items().iter().filter(|i| i.is_active()) {
                if let Some(owner) = item.get_secondary_text() {
                    repos.push(format!("{}/{}", owner, item.text()));
                }
            }
        }
        // Dedupe (case-insensitively, matching the server) and cap at the server's
        // per-query limit so one fetch can't exceed it.
        repos.sort_by_key(|r| r.to_lowercase());
        repos.dedup_by_key(|r| r.to_lowercase());
        const MAX_REPOS: usize = 50;
        repos.truncate(MAX_REPOS);

        if repos.is_empty() {
            // No repos to scope to — nothing to fetch (avoids a guaranteed 400).
            self.pending_remote_repos = None;
            self.cached_remote_tours.clear();
            // A remote overview can no longer be backed by the now-empty cache, so
            // clear it (mirrors after_scope_change) instead of leaving stale markdown.
            if self.right_pane.active_remote_tour_id.is_some() {
                self.right_pane.clear_preview_state(self.workspace.focus_db());
            }
            self.rebuild_tours_panel();
            return;
        }

        // Record the scope so we can discard stale responses if it changes later.
        let scope = repos.join(",");
        self.pending_remote_repos = Some(scope.clone());

        // Spawn background task to fetch tours
        tokio::spawn(async move {
            let opts = codemark_core::sync::ListRemoteToursOptions { server_url, token, repos };

            match codemark_core::sync::list_remote_tours(opts).await {
                Ok(tours) => {
                    let _ = event_handler.send(Event::RemoteToursLoaded(tours, Some(scope)));
                }
                Err(e) => {
                    let _ = event_handler.send(Event::RemoteToursFetchError(e.to_string()));
                }
            }
        });
    }

    /// Start pulling a specific tour from the server by tour_id.
    pub fn start_pull_tour(&mut self, tour_id: String) {
        let user_data_key = format!("remote:{}", tour_id);
        // This exact tour is already being pulled (e.g. a held Enter on its
        // focused overview, or a repeated `p`); ignore the repeat so we don't
        // spawn concurrent network + database sync tasks for the same tour. A
        // pull of a *different* tour is still allowed to proceed. The set (not
        // the spinner list) is the source of truth: the global spinner cleanup
        // can drain a still-running pull's spinner when an overlapping pull
        // finishes, whereas this id is released only by `TourPullFinished`.
        if self.pulling_tour_ids.contains(&tour_id) {
            return;
        }
        // Mark the item as pulling (spinner will be shown on tick)
        self.is_pulling_tour = true;
        self.add_spinner(&user_data_key, tabs::ContentTab::Tours.index());

        let db_path = self.db().path().to_path_buf();
        let event_handler = self.event_handler.clone();

        let codemark_dir = match self.db().path().parent() {
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

        // Committed to spawning: mark the tour in-flight. Released on completion
        // (success or failure) via `TourPullFinished`. The pre-spawn early
        // returns above never reach here, so they need no release.
        self.pulling_tour_ids.insert(tour_id.clone());
        let finished_tour_id = tour_id.clone();

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

            // Release the in-flight guard first, so a follow-up activation of
            // this tour is accepted the moment the pull settles regardless of
            // outcome.
            let _ = event_handler.send(Event::TourPullFinished(finished_tour_id));

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
    ///
    /// Normalizes the URL first: strips any `?query`/`#fragment` and trailing
    /// slashes before taking the last path segment, so a saved URL with those
    /// extras still matches the bare tour id used by remote summaries. Returns
    /// `None` when no non-empty segment remains.
    fn extract_remote_tour_id(imported_url: &str) -> Option<&str> {
        imported_url
            .split(['?', '#'])
            .next()
            .unwrap_or(imported_url)
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .filter(|segment| !segment.is_empty())
    }

    fn rebuild_tours_panel(&mut self) {
        // Capture the selected item's stable id so it can be re-pinned after the
        // rebuild. `set_items` only restores selection by display text, which can
        // drift to a sibling that shares a title (e.g. a remote tour with the same
        // name as a local one), so the preview would follow the wrong item.
        let prev_selected = self
            .left_pane
            .content_panel
            .get_list_panel_mut(ContentTab::Tours.index())
            .and_then(|p| p.selected().and_then(|i| i.user_data.clone()));

        let mut local_items = Vec::new();
        // Track which remote tour IDs have been pulled locally
        let mut matched_remote_ids = std::collections::HashSet::new();
        // Tours are focus-repo scoped, so key live health by the focus root.
        let focus_root = db_repo_root(self.db());
        if let Ok(collections) = self.db().list_collections() {
            for (c, count) in collections {
                let is_tour = c.published_at.is_some() || c.imported_from_url.is_some();
                if is_tour {
                    let health = collection_health_status(
                        c.health,
                        self.collection_live_health
                            .get(&health_key(focus_root.as_deref(), &c.id))
                            .copied(),
                    );
                    let branch = c.created_branch.clone().unwrap_or_else(|| "main".to_string());
                    // Show the author (if known) before the branch name.
                    let secondary = match c.created_by.as_deref() {
                        Some(author) if !author.is_empty() => format!("{author} · {branch}"),
                        _ => branch,
                    };
                    let item = PanelItem::new(&c.name)
                        .secondary_text(secondary)
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
                let date = t.updated_at.chars().take(10).collect::<String>();
                let secondary = match &t.author {
                    Some(author) if !author.is_empty() => format!("{author} · {date}"),
                    _ => date,
                };
                PanelItem::new(&t.title)
                    .secondary_text(secondary)
                    .health(HealthStatus::Unknown)
                    .user_data(format!("remote:{}", t.tour_id))
            })
            .collect();

        let mut all_items = local_items;
        all_items.extend(remote_items);

        if let Some(panel) = self.left_pane.content_panel.get_list_panel_mut(2) {
            panel.set_items(all_items);
            // Re-pin the selection by stable user_data so it survives the rebuild
            // (no-op if that item is gone, e.g. a pulled remote tour was removed).
            if let Some(ud) = prev_selected {
                panel.select_by_user_data(&ud);
            }
        }

        // The Tours tab mirrors collection rows, so a rebuild is also the right
        // moment to refresh collection live health (this also covers the refresh
        // path, which calls here, and the pull/sync/remote-load paths that don't).
        self.spawn_collection_health_task();
    }

    /// Refresh all panels from the current active database.
    ///
    /// Rebuilds the content lists from the full DB set, discarding any active
    /// search narrowing. Callers that must not drop the user's live search
    /// (e.g. a terminal refocus) should use [`Self::refresh_all_panels_preserving_search`].
    pub fn refresh_all_panels(&mut self) {
        self.refresh_all_panels_inner(false);
    }

    /// Like [`Self::refresh_all_panels`], but leaves a content panel that is
    /// currently showing search results untouched, so an active search filter
    /// survives the refresh instead of flashing back to the full list.
    pub fn refresh_all_panels_preserving_search(&mut self) {
        self.refresh_all_panels_inner(true);
    }

    /// After a search-preserving refresh, reconcile each content panel showing
    /// search results with the current DB — across *every* search-active panel,
    /// not just the selected tab, since the user may have searched one tab and
    /// switched to another before the terminal regained focus.
    ///
    /// A preserved row references a record captured before focus was lost. If
    /// the CLI changed the DB while unfocused, the originating search is re-run
    /// so renamed/edited rows pick up fresh text and rows that no longer match
    /// (or were deleted) drop out. FTS re-runs synchronously and applies in
    /// place; semantic re-runs on a background task (its membership can't be
    /// recomputed without re-embedding), keeping the preserved rows visible with
    /// a spinner until the fresh set lands. Reconciling preserves selection and
    /// leaves focus untouched, so the narrowed list stays put.
    pub fn reconcile_preserved_search_rows(&mut self) {
        let mut changed = false;
        for tab in [ContentTab::Bookmarks, ContentTab::Collections] {
            let idx = tab.index();
            let search_active = self
                .left_pane
                .content_panel
                .get_list_panel_mut(idx)
                .is_some_and(|p| p.is_search_active());
            if !search_active {
                continue;
            }

            match &self.search_contexts[idx] {
                // FTS is cheap to re-run, so refresh the whole result set.
                Some((query, SearchMode::Fts)) => {
                    let query = query.clone();
                    // Re-run the FTS fan-out across every checked repo so the
                    // reconciled set stays multi-repo and each row keeps its tag.
                    let items = match tab {
                        ContentTab::Bookmarks => Self::fts_bookmark_hits(&self.workspace, &query)
                            .map(|hits| {
                                Self::build_bookmark_search_items(&hits, &self.bookmark_live_health)
                            }),
                        ContentTab::Collections => {
                            Self::fts_collection_hits(&self.workspace, &query).map(|hits| {
                                Self::build_collection_search_items(
                                    &hits,
                                    &self.collection_live_health,
                                )
                            })
                        }
                        ContentTab::Tours => continue,
                    };
                    if let Ok(items) = items {
                        changed |= self.apply_reconciled_search_items(idx, items);
                    }
                }
                // Semantic membership can't be recomputed without re-embedding
                // the query, so re-run the search on a background task. Results
                // arrive via `SearchResults`/`CollectionSearchResults` and, thanks
                // to the reconcile marker, are applied in place (see
                // `apply_search_results`). The preserved rows stay visible with a
                // spinner until the fresh set lands, so nothing flickers.
                Some((query, SearchMode::Semantic)) => {
                    let query = query.clone();
                    self.active_search_request = self.active_search_request.wrapping_add(1);
                    let request_id = self.active_search_request;
                    self.reconcile_search_requests.insert(request_id, idx);
                    self.left_pane.search.set_loading(true);
                    match tab {
                        ContentTab::Collections => {
                            self.execute_collection_search(request_id, SearchMode::Semantic, query)
                        }
                        _ => self.execute_bookmark_search(request_id, SearchMode::Semantic, query),
                    }
                    // Applied asynchronously; don't touch `changed`/preview here.
                }
                // No recorded context (shouldn't happen while search-active):
                // fall back to dropping rows whose record no longer exists.
                None => {
                    changed |= self.refresh_search_panel_by_id(idx);
                }
            }
        }

        // Reconciling may have moved the selection off a removed row, so refresh
        // the preview to match what is now selected.
        if changed {
            self.update_content_live_preview();
        }
    }

    /// Replace a content panel's rows with reconciled search results, keeping the
    /// panel flagged as search-active and re-pinning the selection by the row's
    /// `user_data` id. Pinning by id (rather than letting `set_items` restore by
    /// display text) avoids drifting the selection onto a different row that
    /// happens to share a label. Returns whether the panel existed.
    fn apply_reconciled_search_items(&mut self, idx: usize, items: Vec<PanelItem>) -> bool {
        let Some(p) = self.left_pane.content_panel.get_list_panel_mut(idx) else {
            return false;
        };
        let selected_id = p.selected().and_then(|i| i.user_data.clone());
        p.set_search_items(items);
        if let Some(id) = selected_id {
            p.select_by_user_data(&id);
        }
        true
    }

    /// Rebuild a search-active panel's rows from their current DB records,
    /// dropping rows whose record no longer exists. Unlike a re-run this keeps
    /// the matched set (it can't recompute membership), but it refreshes text and
    /// removes deleted rows — the best available reconcile when a semantic re-run
    /// isn't run or fails. Returns whether the panel existed.
    fn refresh_search_panel_by_id(&mut self, idx: usize) -> bool {
        // Each row carries its owning repo (name + root); resolve each id against
        // that repo's db so a multi-repo result set reconciles correctly. Rows
        // without a tag (shouldn't happen for search rows) fall back to focus.
        let (focus_name, focus_root) = self.focus_repo_tag();
        let rows: Vec<(String, String, String)> = self
            .left_pane
            .content_panel
            .get_list_panel_mut(idx)
            .map(|p| {
                p.all_items()
                    .iter()
                    .filter_map(|i| {
                        i.user_data.clone().map(|id| {
                            (
                                i.repo_name()
                                    .map(str::to_string)
                                    .unwrap_or_else(|| focus_name.clone()),
                                i.repo_root()
                                    .map(str::to_string)
                                    .unwrap_or_else(|| focus_root.clone()),
                                id,
                            )
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let items = if idx == ContentTab::Collections.index() {
            let hits: Vec<crate::event::CollectionHit> = rows
                .iter()
                .filter_map(|(repo_name, repo_root, id)| {
                    let db = self.workspace.db_for(Some(repo_root));
                    let collection = db.get_collection_by_id(id).ok().flatten()?;
                    let count = db
                        .list_bookmarks_in_collection(&collection.id)
                        .map(|b| b.len())
                        .unwrap_or(0);
                    Some(crate::event::CollectionHit {
                        repo_name: repo_name.clone(),
                        repo_root: repo_root.clone(),
                        collection,
                        count,
                    })
                })
                .collect();
            Self::build_collection_search_items(&hits, &self.collection_live_health)
        } else {
            let hits: Vec<crate::event::BookmarkHit> = rows
                .iter()
                .filter_map(|(repo_name, repo_root, id)| {
                    let bookmark =
                        self.workspace.db_for(Some(repo_root)).get_bookmark(id).ok().flatten()?;
                    Some(crate::event::BookmarkHit {
                        repo_name: repo_name.clone(),
                        repo_root: repo_root.clone(),
                        bookmark,
                    })
                })
                .collect();
            Self::build_bookmark_search_items(&hits, &self.bookmark_live_health)
        };

        self.apply_reconciled_search_items(idx, items)
    }

    /// Whether `request_id` is an in-flight reconcile re-run (without consuming
    /// it). Used by the event guard to accept a reconcile result even when a
    /// newer request has advanced `active_search_request`.
    fn is_reconcile_request(&self, request_id: u64) -> bool {
        self.reconcile_search_requests.contains_key(&request_id)
    }

    /// If `request_id` belongs to an in-flight reconcile re-run, remove it and
    /// return the content panel index it targets. Lets the search-result and
    /// error handlers tell a background reconcile apart from a user search and
    /// route it to the right panel.
    fn take_reconcile_target(&mut self, request_id: u64) -> Option<usize> {
        self.reconcile_search_requests.remove(&request_id)
    }

    /// Shared body for the two refresh entry points. When `preserve_search` is
    /// set, a content panel that is displaying search results (non-empty query
    /// and `search_active`) keeps its narrowed items rather than being rebuilt
    /// from the full DB set.
    fn refresh_all_panels_inner(&mut self, preserve_search: bool) {
        // 1. Update Context panel Owners (preserving active owner selections)
        let active_owners: Vec<String> = self
            .left_pane
            .context_panel
            .panels
            .get(ContextTab::Owners.index())
            .and_then(|c| match c {
                TabContent::List(p) => Some(p.active_items()),
                _ => None,
            })
            .unwrap_or_default();

        let owner_items = TabbedPanel::build_owner_items(&self.registry);
        if let Some(p) = self.left_pane.context_panel.get_list_panel_mut(ContextTab::Owners.index())
        {
            p.set_items(owner_items);
            // Re-activate previously selected owners
            for owner in &active_owners {
                p.activate_by_user_data(owner);
            }
        }

        // Update Context panel Auth accounts (read-only)
        let auth_items = TabbedPanel::build_auth_account_items(&self.registry);
        if let Some(p) = self.left_pane.context_panel.get_list_panel_mut(ContextTab::Auth.index()) {
            p.set_items(auth_items);
        }

        // Update Context panel Repos (respecting active owner filter)
        if active_owners.is_empty() {
            let repo_items = TabbedPanel::build_repo_items(self.db(), &self.registry);
            if let Some(p) =
                self.left_pane.context_panel.get_list_panel_mut(ContextTab::Repos.index())
            {
                p.set_items(repo_items);
            }
        } else {
            self.update_repos_by_owner();
        }
        // Re-activate every checked repo (build_repo_items only marks the focus
        // repo, so a rebuild would drop the multi-select checkmarks).
        self.restore_active_repos();
        // 2. Update Tags/Branches (in-place)
        self.refresh_tags();

        // 3. Update Bookmarks/Collections/Tours (in-place). When preserving an
        // active search, a panel currently showing search results is left as-is
        // so the narrowed list doesn't flash back to the full set; stale rows are
        // then pruned in place by `reconcile_preserved_search_rows`. The Esc
        // "clear search" path uses the non-preserving refresh, so the full list
        // is rebuilt there regardless of this flag.
        // Bookmarks and Collections merge across every checked repo (with a repo
        // tag on each row, and the repo name surfaced when multiple are checked).
        let (collections, bookmarks) = crate::browser::tabbed_panel::build_merged_content(
            self.workspace.dbs(),
            &self.collection_live_health,
            &self.bookmark_live_health,
        );
        // Tours stay scoped to the focus repo (re-merged with cached remote rows
        // by `rebuild_tours_panel` below).
        let focus_root = db_repo_root(self.db());
        let (tours, _, _) = TabbedPanel::build_content_items(
            self.db(),
            &self.collection_live_health,
            &self.bookmark_live_health,
            focus_root.as_deref(),
        );
        if let Some(p) = self.left_pane.content_panel.get_list_panel_mut(0)
            && !(preserve_search && p.is_search_active())
        {
            p.set_items(bookmarks);
        }
        if let Some(p) = self.left_pane.content_panel.get_list_panel_mut(1)
            && !(preserve_search && p.is_search_active())
        {
            p.set_items(collections);
        }
        if let Some(p) = self.left_pane.content_panel.get_list_panel_mut(2) {
            p.set_items(tours);
        }

        // 3a. The Tours tab is a hybrid of local tours and cached remote tours;
        // build_content_items only knows the DB-local rows, so re-merge the cached
        // remote rows or they would vanish until the next fetch.
        self.rebuild_tours_panel();

        // 3b. Hide the Tours tab unless the user is logged in (it only holds
        // remote tours, which are meaningless without a sync server).
        self.update_tours_tab_visibility();

        // 3c. Spawn background live health resolution for all bookmarks across
        // every checked repo (each batch tagged with its repo root so the caches
        // don't collide on shared ids).
        self.spawn_live_health_all_repos();

        // 4. Update Step previews (Right Pane) using live resolution
        let current_step = self.right_pane.pager_current;
        // Restore the active preview from its own repo (tracked when it was
        // loaded); `None` falls back to the focused repo.
        let active_root = self.right_pane.active_repo_root.clone();
        if let Some(tour_name) = self.right_pane.active_tour_name.clone() {
            self.right_pane.load_tour_live(
                self.workspace.db_for(active_root.as_deref()),
                &tour_name,
                &mut self.session_cache,
            );
            // Restore step if possible
            if current_step < self.right_pane.pager_total {
                self.right_pane.pager_current = current_step;
                self.right_pane.update_preview(self.workspace.db_for(active_root.as_deref()));
            }
            // load_tour_live resolves only the first step live; resolve the rest
            // (including the restored current step) off the UI thread.
            self.spawn_collection_live_resolve();
        } else if let Some(bm_id) = self.right_pane.active_bookmark_id.clone() {
            self.right_pane.load_bookmark_live(
                self.workspace.db_for(active_root.as_deref()),
                &bm_id,
                &mut self.session_cache,
            );
        } else if let Some(remote_id) = self.right_pane.active_remote_tour_id.clone() {
            // A remote tour overview was showing. If it has since been pulled
            // (a local collection imported from this remote id now exists), the
            // Tours list shows the local row, so translate the preview to that
            // local tour instead of re-rendering stale server metadata.
            let pulled_local = self.db().list_collections().ok().and_then(|cols| {
                cols.into_iter()
                    .find(|(c, _)| {
                        c.imported_from_url
                            .as_deref()
                            .and_then(Self::extract_remote_tour_id)
                            .is_some_and(|rid| rid == remote_id)
                    })
                    .map(|(c, _)| c.id)
            });
            if let Some(local_id) = pulled_local {
                // load_collection_overview clears active_remote_tour_id.
                self.load_collection_overview_live(&local_id);
                // The remote:<id> row is gone, so move the Tours selection to the
                // pulled local collection — otherwise the list could stay on a
                // same-titled sibling while the right pane shows the pulled tour.
                if let Some(panel) =
                    self.left_pane.content_panel.get_list_panel_mut(ContentTab::Tours.index())
                {
                    panel.select_by_user_data(&local_id);
                }
            } else {
                // Otherwise re-render from the cached summary. If the summary is
                // gone (cache cleared, or a reload omitted this tour), clear the
                // preview rather than leaving stale remote markdown or rendering
                // unrelated local content under the same selection.
                match self.cached_remote_tours.iter().find(|t| t.tour_id == remote_id).cloned() {
                    Some(tour) => {
                        self.right_pane.load_tour_overview(&tour);
                        // `rebuild_tours_panel` restores the list selection by item
                        // text, which can drift from the right pane (e.g. a remote
                        // tour sharing a title with a local one, or a reorder).
                        // Re-pin the Tours selection to this remote id so the next
                        // keypress doesn't jump the preview to a different item.
                        if let Some(panel) = self
                            .left_pane
                            .content_panel
                            .get_list_panel_mut(ContentTab::Tours.index())
                        {
                            panel.select_by_user_data(&format!("remote:{remote_id}"));
                        }
                    }
                    None => self.right_pane.clear_preview_state(self.workspace.focus_db()),
                }
            }
        } else if let Some((first_tour, _)) =
            self.db().list_collections().ok().and_then(|c| c.into_iter().next())
        {
            // Default to the first tour only if nothing was active.
            self.right_pane.load_tour_live(
                self.workspace.focus_db(),
                &first_tour.name,
                &mut self.session_cache,
            );
            self.spawn_collection_live_resolve();
        } else {
            // Nothing to show — clear the *whole* preview (steps and any overview)
            // so a stale remote/collection overview doesn't linger, e.g. after
            // switching to a repo with no collections.
            self.right_pane.clear_preview_state(self.workspace.focus_db());
        }
    }

    /// Refresh tags in Filters panel based on the active tab in Content panel.
    pub fn refresh_tags(&mut self) {
        let active_tab = ContentTab::from_index(self.left_pane.content_panel.tabs.selected_index())
            .unwrap_or(ContentTab::Bookmarks);
        let (tags, branches) = TabbedPanel::build_tags_branches_items(
            self.workspace.dbs().map(|(_, db)| db),
            active_tab,
        );
        if let Some(p) = self.left_pane.filters_panel.get_list_panel_mut(0) {
            p.set_items(tags);
        }
        if let Some(p) = self.left_pane.filters_panel.get_list_panel_mut(1) {
            p.set_items(branches);
        }
        // Bookmarks have no associated branch, so hide Filters panel's Branches tab
        // when they are active; only Collections and Tours carry a branch.
        self.left_pane.filters_panel.sync_branches_tab_visibility(active_tab);
    }

    /// Get the current focus area.
    pub fn focus(&self) -> FocusArea {
        self.focus
    }

    /// Drain the tab-change flags from every tabbed panel, returning the
    /// filter-target key ("panel1"/"panel2"/"panel3"/"main") of each pane whose
    /// active tab changed during the last handled event.
    ///
    /// Filters are pane-scoped (one query per pane, applied to the active tab),
    /// so the event loop clears these panes' stored filters to keep the filter
    /// from carrying over to the newly active tab.
    pub fn take_filter_targets_to_clear(&self) -> Vec<&'static str> {
        let mut targets = Vec::new();
        if self.left_pane.context_panel.take_tab_changed() {
            targets.push("panel1");
        }
        if self.left_pane.filters_panel.take_tab_changed() {
            targets.push("panel2");
        }
        if self.left_pane.content_panel.take_tab_changed() {
            targets.push("panel3");
        }
        if self.right_pane.steps.take_tab_changed() {
            targets.push("main");
        }
        targets
    }

    /// Get current filters/metadata for the status bar.
    pub fn get_status_metadata(&self) -> Line<'_> {
        let repo_name = self
            .db()
            .list_repos()
            .ok()
            .and_then(|repos| repos.first().map(|r| r.repo_name.clone()))
            .unwrap_or_else(|| {
                self.db()
                    .path()
                    .parent()
                    .and_then(|p| p.parent())
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "unknown".to_string())
            });

        let active_branches = self
            .left_pane
            .filters_panel
            .panels
            .get(1)
            .and_then(|c| match c {
                TabContent::List(p) => Some(p.active_items()),
                _ => None,
            })
            .unwrap_or_default();

        let mut spans = vec![
            Span::styled("Repo: ", Style::default().fg(crate::theme::palette().dim)),
            Span::styled(repo_name, Style::default().fg(crate::theme::palette().accent)),
        ];

        if !active_branches.is_empty() {
            spans.push(Span::styled(" | ", Style::default().fg(crate::theme::palette().gray)));
            spans.push(Span::styled("Branch: ", Style::default().fg(crate::theme::palette().dim)));
            spans.push(Span::styled(
                active_branches.join(", "),
                Style::default().fg(crate::theme::palette().warning),
            ));
        }

        spans.push(Span::styled(" | ", Style::default().fg(crate::theme::palette().gray)));
        spans.push(Span::styled(
            format!("\u{f0031} v{}", env!("CARGO_PKG_VERSION")),
            Style::default().fg(crate::theme::palette().emphasis).bold(),
        ));

        Line::from(spans)
    }

    /// Open the currently selected bookmark or tour step in the editor.
    pub fn open_in_editor(&mut self) {
        // Special handling for ContentPanel bookmarks: open directly without StepData
        if self.focus == FocusArea::ContentPanel
            && let Some(bookmark) = self
                .left_pane
                .content_panel
                .active_panel_mut()
                .and_then(|panel| panel.selected())
                .and_then(|item| {
                    // Get the bookmark ID from user_data
                    let bookmark_id = item.user_data.as_ref()?;
                    // Get the bookmark (flatten Result<Option<Bookmark>>)
                    self.workspace.focus_db().get_bookmark(bookmark_id).ok().flatten()
                })
        {
            self.open_bookmark_in_editor(bookmark);
            return;
        }

        // Default: get step from the right pane (Main)
        let Some(step) = self.right_pane.steps_data.get(self.right_pane.pager_current) else {
            return;
        };

        let Some(codemark_dir) = self.db().path().parent() else {
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

    /// Open a bookmark directly in the editor (used for ContentPanel bookmarks).
    fn open_bookmark_in_editor(&mut self, bookmark: Bookmark) {
        use codemark_core::git::context::resolve_bookmark_file_path;

        let Some(codemark_dir) = self.db().path().parent() else {
            return;
        };
        let config = Config::load_layered(codemark_dir);

        // Resolve the file path and line range from the bookmark's latest resolution
        let (relative_path, line_start, line_end) = if let Some(resolution) =
            self.db().list_resolutions(&bookmark.id, 1).ok().and_then(|mut v| v.pop())
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
        let absolute_path = match resolve_bookmark_file_path(&relative_path, self.db().path()) {
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
                Some(FocusArea::ContextPanel) => FocusArea::ContextPanel,
                Some(FocusArea::FiltersPanel) => FocusArea::FiltersPanel,
                Some(FocusArea::ContentPanel) => FocusArea::ContentPanel,
                Some(FocusArea::Main) => FocusArea::Main,
                // Search focus filters ContextPanel (consistent with main.rs filter_target logic)
                Some(FocusArea::Search) => FocusArea::ContextPanel,
                // Filter focus with no previous focus defaults to ContentPanel
                Some(FocusArea::Filter) | None => FocusArea::ContentPanel,
            }
        } else {
            self.focus
        };

        match target_focus {
            FocusArea::ContextPanel => {
                if let Some(panel) = self.left_pane.context_panel.active_panel_mut() {
                    panel.set_filter(query);
                }
            }
            FocusArea::FiltersPanel => {
                if let Some(panel) = self.left_pane.filters_panel.active_panel_mut() {
                    panel.set_filter(query);
                }
            }
            FocusArea::ContentPanel => {
                if let Some(panel) = self.left_pane.content_panel.active_panel_mut() {
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
            .filters_panel
            .panels
            .first()
            .and_then(|c| match c {
                TabContent::List(p) => Some(p.active_items()),
                _ => None,
            })
            .unwrap_or_default();

        let active_branches = self
            .left_pane
            .filters_panel
            .panels
            .get(1)
            .and_then(|c| match c {
                TabContent::List(p) => Some(p.active_items()),
                _ => None,
            })
            .unwrap_or_default();
        let dbs: Vec<_> = self.workspace.dbs().collect();
        let multi = dbs.len() > 1;

        let mut collection_items = Vec::new();
        let mut tour_items = Vec::new();
        let mut bookmark_items = Vec::new();
        let mut all_filtered_bookmarks = Vec::new();

        for (root, db) in dbs {
            let repo_name = tabbed_panel::repo_display_name(root);
            let root_key = root.to_string_lossy().into_owned();

            // 1. Update Tours/Collections
            if let Ok(collections) = db.list_collections() {
                for (c, count) in collections {
                    // Filter by branch if any are active
                    let branch_match = active_branches.is_empty()
                        || c.created_branch.as_ref().is_some_and(|b| active_branches.contains(b));

                    // Filter by tags if any are active
                    let tag_match = active_tags.is_empty() || {
                        if let Ok(c_tags) = db.list_tags_for_collection(&c.id) {
                            c_tags.iter().any(|t| active_tags.contains(&t.tag))
                        } else {
                            false
                        }
                    };

                    if branch_match && tag_match {
                        let health = collection_health_status(
                            c.health,
                            self.collection_live_health
                                .get(&health_key(Some(&root_key), &c.id))
                                .copied(),
                        );

                        let is_published = c.published_at.is_some();
                        let is_tour = is_published || c.imported_from_url.is_some();
                        let branch = c.created_branch.unwrap_or_else(|| "main".to_string());
                        let mut item = PanelItem::new(&c.name)
                            .secondary_text(&branch)
                            .metadata(format!("{count} steps"))
                            .health(health)
                            .published(is_published)
                            .user_data(c.id.clone());
                        
                        if multi {
                            item = item.repo(&repo_name, &root_key);
                        }

                        collection_items.push(item);
                        if is_tour {
                            // Tours are strictly single-repo, no multi-tag needed
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
            }

            // 2. Update Bookmarks
            if let Ok(bookmarks) = db.list_bookmarks(&BookmarkFilter::default()) {
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
                            .unwrap_or_else(|| bm.query.clone());
                        let icon = summary_info
                            .as_ref()
                            .map(|s| codemark_core::query::classifier::get_node_icon(&s.label))
                            .unwrap_or("");
                        let short_path = shorten_path(&bm.file_path, 25);

                        // Seed the dot from the live-health cache
                        let health = self
                            .bookmark_live_health
                            .get(&health_key(Some(&root_key), &bm.id))
                            .copied()
                            .unwrap_or(HealthStatus::Unknown);
                        let mut item = PanelItem::new(&short_path)
                            .metadata(bm.created_by.clone().unwrap_or_default())
                            .health(health)
                            .icon(icon)
                            .user_data(bm.id.clone());
                        
                        if !summary.is_empty() {
                            item = item.emphasis(summary);
                        }
                        if multi {
                            item = item.repo(&repo_name, &root_key);
                        }
                        item
                    })
                    .collect();
                
                bookmark_items.extend(filtered_items);
                all_filtered_bookmarks.extend(filtered_bookmarks);
            }
        }

        if let Some(TabContent::List(p)) = self.left_pane.content_panel.panels.get_mut(2) {
            p.set_items(tour_items);
        }
        if let Some(TabContent::List(p)) = self.left_pane.content_panel.panels.get_mut(1) {
            p.set_items(collection_items);
        }
        if let Some(TabContent::List(p)) = self.left_pane.content_panel.panels.get_mut(0) {
            p.set_items(bookmark_items);
        }
        // Spawn background live health resolution for filtered bookmarks
        self.spawn_live_health_task(all_filtered_bookmarks);

        // A tag/branch toggle rebuilds the collection rows from the (cached) live
        // health; refresh the cache so newly-relevant collections are current.
        self.spawn_collection_health_task();
    }

    /// Re-activate every checked repo in the Repos panel after a rebuild.
    ///
    /// `build_repo_items` marks only the focus repo `.active(true)`, so a panel
    /// rebuild via `set_items` wipes the multi-select state of every other
    /// checked repo. This restores the checkmarks from the workspace's checked
    /// roots so every in-scope repo shows its checkmark.
    fn restore_active_repos(&mut self) {
        let checked_roots: Vec<String> =
            self.workspace.dbs().map(|(root, _)| root.to_string_lossy().into_owned()).collect();
        if let Some(p) = self.left_pane.context_panel.get_list_panel_mut(ContextTab::Repos.index())
        {
            for root in &checked_roots {
                p.activate_by_user_data(root);
            }
        }
    }

    ///
    /// Follows the same pattern as `update_tours_collections()`:
    /// reads active owners from the Owners panel, re-queries repos from the registry,
    /// and filters to only show repos matching the selected owners.
    fn update_repos_by_owner(&mut self) {
        let active_owners = self
            .left_pane
            .context_panel
            .panels
            .get(ContextTab::Owners.index())
            .and_then(|c| match c {
                TabContent::List(p) => Some(p.active_items()),
                _ => None,
            })
            .unwrap_or_default();

        let repo_items = if active_owners.is_empty() {
            // No filter — show all repos
            TabbedPanel::build_repo_items(self.db(), &self.registry)
        } else {
            // Filter repos by selected owners
            TabbedPanel::build_repo_items(self.db(), &self.registry)
                .into_iter()
                .filter(|item| {
                    item.get_secondary_text()
                        .is_some_and(|owner| active_owners.iter().any(|o| o == owner))
                })
                .collect()
        };

        if let Some(p) = self.left_pane.context_panel.get_list_panel_mut(ContextTab::Repos.index())
        {
            p.set_items(repo_items);
        }
        self.restore_active_repos();
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
                // Filter focus with no previous focus defaults to ContentPanel
                self.focus = FocusArea::ContentPanel;
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

    /// The ordered list of focusable areas in the left pane.
    ///
    /// When the left pane is expanded (Half/Full), only the panes that are
    /// actually rendered are included, so Tab can't move focus to a hidden
    /// pane. The content panel is shown alongside the search bar, while the
    /// context and filters panels are shown alone.
    fn left_pane_focus_cycle(&self) -> Vec<FocusArea> {
        // All left-pane areas, in tab order. Used in regular mode and as the
        // fallback for expanded mode, mirroring LeftPane::render()'s fallback
        // which draws every panel.
        let all = vec![
            FocusArea::Search,
            FocusArea::ContextPanel,
            FocusArea::FiltersPanel,
            FocusArea::ContentPanel,
        ];
        match self.left_pane_size {
            LeftPaneSize::Regular => all,
            LeftPaneSize::Half | LeftPaneSize::Full => {
                match self.left_pane.expanded_focus() {
                    // Content panel is rendered together with the search bar.
                    FocusArea::ContentPanel => {
                        vec![FocusArea::Search, FocusArea::ContentPanel]
                    }
                    // Context/Filters panels are rendered alone.
                    FocusArea::ContextPanel => vec![FocusArea::ContextPanel],
                    FocusArea::FiltersPanel => vec![FocusArea::FiltersPanel],
                    _ => all,
                }
            }
        }
    }

    /// Cycle to the next focusable area within the current pane.
    pub fn next_focus(&mut self) {
        match self.focus {
            FocusArea::Main => {
                self.right_pane.toggle_internal_focus();
            }
            _ => {
                let cycle = self.left_pane_focus_cycle();
                self.focus = match cycle.iter().position(|&f| f == self.focus) {
                    Some(i) => cycle[(i + 1) % cycle.len()],
                    None => cycle[0],
                };
            }
        }
        tracing::debug!(target: "codemark::ui", focus = ?self.focus, "next_focus");
        self.update_focus_state();
    }

    /// Cycle to the previous focusable area within the current pane.
    pub fn previous_focus(&mut self) {
        match self.focus {
            FocusArea::Main => {
                self.right_pane.toggle_internal_focus();
            }
            _ => {
                let cycle = self.left_pane_focus_cycle();
                self.focus = match cycle.iter().position(|&f| f == self.focus) {
                    Some(i) => cycle[(i + cycle.len() - 1) % cycle.len()],
                    None => cycle[0],
                };
            }
        }
        tracing::debug!(target: "codemark::ui", focus = ?self.focus, "previous_focus");
        self.update_focus_state();
    }

    /// Update focus state based on current focus area.
    fn update_focus_state(&mut self) {
        // Reset all focus
        self.left_pane.search.set_focus(false);
        self.left_pane.context_panel.set_focus(false);
        self.left_pane.filters_panel.set_focus(false);
        self.left_pane.content_panel.set_focus(false);
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
        } else {
            // Leaving the preview pane resets its expansion (the `+`/`-`
            // fullscreen state). Otherwise an expanded, full-width preview keeps
            // rendering RenderMode::RightOnly after focus moves to a left panel,
            // hiding the now-focused pane entirely.
            self.right_pane_size = RightPaneSize::Regular;
            self.details_pane_size = DetailsPaneSize::Regular;
        }

        // Sync focused area and resize mode to left pane
        self.left_pane.set_focused_area(self.focus);
        self.left_pane.set_resize_mode(self.left_pane_size);

        // Set focus on current area
        match self.focus {
            FocusArea::Search => {
                self.left_pane.search.set_focus(true);
            }
            FocusArea::ContextPanel => {
                self.left_pane.context_panel.set_focus(true);
            }
            FocusArea::FiltersPanel => {
                self.left_pane.filters_panel.set_focus(true);
            }
            FocusArea::ContentPanel => {
                self.left_pane.content_panel.set_focus(true);
                self.update_content_live_preview();
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

        let hide_details = self.right_pane_size.hides_details();

        match render_mode {
            RenderMode::Both => {
                self.left_pane.render(chunks[0], buf);
                self.right_pane.render(chunks[1], buf, false, self.details_pane_size);
            }
            RenderMode::LeftOnly => {
                self.left_pane.render(chunks[0], buf);
            }
            RenderMode::RightOnly => {
                self.right_pane.render(chunks[0], buf, hide_details, self.details_pane_size);
            }
        }

        // Modal dialogs draw last so they overlay every pane.
        self.render_dialog(area, buf);
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

    /// Create a temp repo directory with an initialized `<root>/.codemark/codemark.db`,
    /// mirroring the on-disk layout `RepoWorkspace` opens against (see
    /// `RepoWorkspace::db_path`). The tempdir is leaked so it outlives the returned
    /// root; the OS reclaims it at process exit.
    fn temp_repo_root() -> std::path::PathBuf {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        Database::create(&root.join(".codemark").join("codemark.db")).expect("init db");
        std::mem::forget(dir);
        root
    }

    /// A layout whose focus db is a real on-disk repo (so `RepoWorkspace::set_scope`
    /// can add sibling repos), plus the repo roots for driving scope changes.
    fn repo_layout() -> (BrowserLayout, std::path::PathBuf) {
        let root = temp_repo_root();
        let db = Database::open(&root.join(".codemark").join("codemark.db")).expect("open db");
        let handler = EventHandler::new(EventHandlerConfig::default()).expect("event handler");
        (BrowserLayout::new(db, handler), root)
    }

    #[test]
    fn extract_remote_tour_id_normalizes_url() {
        let id = "5efea669-1234";
        // Bare path segment.
        assert_eq!(
            BrowserLayout::extract_remote_tour_id(&format!("http://h:8080/tours/{id}")),
            Some(id)
        );
        // Trailing slash, query string, and fragment must not corrupt the id.
        assert_eq!(
            BrowserLayout::extract_remote_tour_id(&format!("http://h/tours/{id}/")),
            Some(id)
        );
        assert_eq!(
            BrowserLayout::extract_remote_tour_id(&format!("http://h/tours/{id}?foo=bar")),
            Some(id)
        );
        assert_eq!(
            BrowserLayout::extract_remote_tour_id(&format!("http://h/tours/{id}#frag")),
            Some(id)
        );
        // No usable segment.
        assert_eq!(BrowserLayout::extract_remote_tour_id("/"), None);
        assert_eq!(BrowserLayout::extract_remote_tour_id(""), None);
    }

    #[test]
    fn collection_health_status_prefers_live_over_persisted() {
        use codemark_core::engine::bookmark::CollectionHealth;
        // A cached live status wins over the persisted snapshot — this is the
        // fix: a collection persisted as Active shows Drifted once its bookmarks
        // are resolved live and found to have drifted.
        assert_eq!(
            collection_health_status(Some(CollectionHealth::Active), Some(HealthStatus::Drifted),),
            HealthStatus::Drifted
        );
        assert_eq!(
            collection_health_status(Some(CollectionHealth::Active), Some(HealthStatus::Broken)),
            HealthStatus::Broken
        );
        // No live value yet → fall back to the persisted mapping.
        assert_eq!(
            collection_health_status(Some(CollectionHealth::Active), None),
            HealthStatus::Healthy
        );
        assert_eq!(
            collection_health_status(Some(CollectionHealth::Stale), None),
            HealthStatus::Broken
        );
        assert_eq!(collection_health_status(None, None), HealthStatus::Unknown);
    }

    #[test]
    fn health_key_isolates_same_id_across_repos() {
        // Two checked repos hold an item with the SAME id (e.g. a published tour
        // imported into both). Keying the live-health cache by (repo_root, id)
        // must give them distinct buckets so one repo's dot can't overwrite the
        // other's.
        let mut map: HashMap<String, HealthStatus> = HashMap::new();
        let id = "01J000000000000000000SHARED";
        let root_a = "/home/u/repo-a";
        let root_b = "/home/u/repo-b";

        let key_a = health_key(Some(root_a), id);
        let key_b = health_key(Some(root_b), id);
        assert_ne!(key_a, key_b, "same id in different repos must produce different keys");

        // Apply each repo's own live status; they must not collide.
        map.insert(key_a.clone(), HealthStatus::Healthy);
        map.insert(key_b.clone(), HealthStatus::Broken);
        assert_eq!(map.get(&key_a).copied(), Some(HealthStatus::Healthy));
        assert_eq!(map.get(&key_b).copied(), Some(HealthStatus::Broken));

        // A bare-id lookup (the old, buggy behavior) misses entirely, proving the
        // buckets are namespaced by repo.
        assert_eq!(map.get(id), None);

        // The unit-separator makes the composite key unambiguous, and a missing
        // repo_root folds to the empty prefix (its own bucket).
        assert_eq!(health_key(Some(root_a), id), format!("{root_a}\u{1f}{id}"));
        assert_ne!(health_key(None, id), health_key(Some(root_a), id));
    }

    #[test]
    fn focusing_preview_restores_default_left_pane_size() {
        let mut layout = test_layout();

        // Simulate the user expanding the left pane while a left panel is focused.
        layout.set_focus(FocusArea::ContentPanel);
        layout.set_left_pane_size(LeftPaneSize::Half);
        assert_eq!(layout.left_pane_size(), LeftPaneSize::Half);

        // Pressing Enter on a bookmark/collection moves focus to the preview pane.
        // The default vertical-split layout must be reachable again, so the left
        // pane size resets to Regular instead of staying expanded behind the preview.
        layout.set_focus(FocusArea::Main);
        assert_eq!(layout.left_pane_size(), LeftPaneSize::Regular);
    }

    #[test]
    fn leaving_preview_resets_fullscreen_expansion() {
        let mut layout = test_layout();

        // Enter the preview pane and expand it to full width via `+`.
        layout.set_focus(FocusArea::Main);
        layout.right_pane_size = RightPaneSize::Full;
        layout.details_pane_size = DetailsPaneSize::Full;

        // Pressing Esc moves focus back to the bookmarks panel. The fullscreen
        // expansion must reset so the now-focused panel is actually visible
        // instead of staying hidden behind a full-width preview.
        layout.set_focus(FocusArea::ContentPanel);
        assert_eq!(layout.right_pane_size, RightPaneSize::Regular);
        assert_eq!(layout.details_pane_size, DetailsPaneSize::Regular);
    }

    #[test]
    fn left_pane_size_preserved_across_left_panel_focus() {
        let mut layout = test_layout();

        // Expanding while on ContentPanel, then moving between left panels must keep the
        // expanded size — only focusing the preview pane resets it.
        layout.set_focus(FocusArea::ContentPanel);
        layout.set_left_pane_size(LeftPaneSize::Full);
        layout.set_focus(FocusArea::FiltersPanel);
        assert_eq!(layout.left_pane_size(), LeftPaneSize::Full);
    }

    /// Number of currently-selectable Content tabs, probed through the public tab
    /// API: a trailing (hidden) tab can never be selected, so `set_selected(idx)`
    /// followed by reading `selected_index()` reveals whether `idx` is visible.
    fn tours_tab_visible(layout: &mut BrowserLayout) -> bool {
        let tabs = &mut layout.left_pane.content_panel.tabs;
        let restore = tabs.selected_index();
        tabs.set_selected(ContentTab::Tours.index());
        let visible = tabs.selected_index() == ContentTab::Tours.index();
        tabs.set_selected(restore);
        visible
    }

    #[test]
    fn tours_tab_single_repo_follows_login_state_parity() {
        // Parity: with exactly one repo checked, Tours visibility is governed
        // solely by login state (as before this change) — the new multi-repo gate
        // must not touch the single-repo path. We assert the visibility tracks
        // `logged_in` rather than hard-coding a token-dependent expectation, so the
        // test is stable regardless of the ambient sync config.
        let (mut layout, _root) = repo_layout();
        assert!(!layout.workspace.is_multi());
        layout.update_tours_tab_visibility();
        assert_eq!(
            tours_tab_visible(&mut layout),
            layout.logged_in,
            "single-repo Tours visibility must equal login state",
        );
    }

    #[test]
    fn tours_tab_hidden_in_multi_repo_even_if_selected() {
        let (mut layout, root_a) = repo_layout();
        let root_b = temp_repo_root();

        // Pretend Tours is the selected tab (as it would be for a logged-in user
        // browsing tours) so we exercise the graceful-fallback path when it hides.
        {
            let tabs = &mut layout.left_pane.content_panel.tabs;
            tabs.set_visible_count(3);
            tabs.set_selected(ContentTab::Tours.index());
            assert_eq!(tabs.selected_index(), ContentTab::Tours.index());
        }

        // Check a second repo: the workspace is now multi.
        layout.workspace.set_scope(&[root_a, root_b]).expect("set scope");
        assert!(layout.workspace.is_multi());

        layout.update_tours_tab_visibility();

        // Tours is hidden regardless of login state, and the selection has fallen
        // back off the now-hidden tab (reusing `set_visible_count`'s clamp).
        assert!(!tours_tab_visible(&mut layout), "multi-repo hides Tours");
        assert!(
            layout.left_pane.content_panel.tabs.selected_index() < ContentTab::Tours.index(),
            "selection fell back off the hidden Tours tab",
        );
    }

    // ---------------------------------------------------------------------
    // End-to-end multi-repo browse / select / delete (Task 6.2)
    //
    // These live in-crate (not in `tests/browser_e2e.rs`) on purpose: the
    // external e2e crate only sees the public API, and scoping *two* repos plus
    // reading a row's repo tag / Tours visibility / the owning db has no public
    // surface (`workspace`/`left_pane` are private). Rather than add permanent
    // public accessors purely for a test, the test runs in-crate where it can
    // drive real key events, render into a `TestBackend`, and assert on the
    // *real* merged panel items, the workspace scope, and each repo's db — exercising
    // the actual multi-repo path (`set_scope` -> `after_scope_change` ->
    // `refresh_all_panels`) with no new production surface. The seed/scope helpers
    // mirror what the multi-select Repos panel does at runtime.
    // ---------------------------------------------------------------------

    use codemark_core::engine::bookmark::{Bookmark, BookmarkHealth, Collection, Visibility};

    /// A minimal valid bookmark for seeding (mirrors the e2e harness's builder).
    fn e2e_bookmark(id: &str, query: &str, file_path: &str) -> Bookmark {
        Bookmark {
            id: id.to_string(),
            query: query.to_string(),
            language: "rust".to_string(),
            file_path: file_path.to_string(),
            content_hash: None,
            commit_hash: None,
            health: BookmarkHealth::Active,
            resolution_method: None,
            last_resolved_at: None,
            stale_since: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            created_by: None,
            current_resolution_id: None,
            repo_id: None,
            tags: Vec::new(),
            annotations: Vec::new(),
            comments: Vec::new(),
        }
    }

    /// A minimal private collection for seeding.
    fn e2e_collection(id: &str, name: &str) -> Collection {
        Collection {
            id: id.to_string(),
            name: name.to_string(),
            description: None,
            visibility: Visibility::Private,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            created_by: None,
            created_branch: None,
            published_at: None,
            published_commit_sha: None,
            repo_url: None,
            repo_id: None,
            status: None,
            health: None,
            health_computed_at: None,
            updated_at: None,
            imported_from_url: None,
        }
    }

    /// Build a temp repo on disk with an initialized db, seed one bookmark (+ a
    /// collection containing it), and write the bookmark's source file so live
    /// resolution succeeds and the preview renders real code. Returns the repo
    /// root (its tempdir is leaked so it outlives the test).
    fn seeded_repo(
        bookmark_id: &str,
        query: &str,
        rel_path: &str,
        source: &str,
        collection_id: &str,
        collection_name: &str,
    ) -> std::path::PathBuf {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        let db = Database::create(&root.join(".codemark").join("codemark.db")).expect("init db");
        db.insert_bookmark(&e2e_bookmark(bookmark_id, query, rel_path)).expect("seed bookmark");
        db.insert_collection(&e2e_collection(collection_id, collection_name))
            .expect("seed collection");
        db.add_to_collection(collection_id, &[bookmark_id.to_string()])
            .expect("populate collection");
        // Seed the real source so live resolution succeeds (preview shows code,
        // not a machine-specific "could not load file" error).
        let full = root.join(rel_path);
        std::fs::create_dir_all(full.parent().unwrap()).expect("create source dir");
        std::fs::write(full, source).expect("write source file");
        std::mem::forget(dir);
        root
    }

    /// Render the layout into a fixed-size `TestBackend` and return the screen as
    /// a newline-joined string, matching the external e2e harness's helper.
    fn render_layout(layout: &BrowserLayout, width: u16, height: u16) -> String {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal.draw(|f| Component::render(layout, f.area(), f.buffer_mut())).expect("draw");
        let buffer = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..height {
            for x in 0..width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn multi_repo_browse_select_and_delete_end_to_end() {
        use crate::event::{Event, EventHandler, EventHandlerConfig};
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        // Two repos on disk, each with a distinct bookmark + collection.
        let root_a = seeded_repo(
            "bm-alpha",
            "fn alpha",
            "src/a.rs",
            "fn alpha() -> u8 { 1 }\n",
            "col-a",
            "AlphaTour",
        );
        let root_b = seeded_repo(
            "bm-beta",
            "fn beta",
            "src/b.rs",
            "fn beta() -> u8 { 2 }\n",
            "col-b",
            "BetaTour",
        );

        // Build the layout seeded on repo A via its normal constructor, then check
        // repo B too — the exact `set_scope` + `after_scope_change` the multi-select
        // Repos toggle drives at runtime (see `activate_context_selection`).
        let db_a = Database::open(&root_a.join(".codemark").join("codemark.db")).expect("open A");
        let handler = EventHandler::new(EventHandlerConfig::default()).expect("event handler");
        let mut layout = BrowserLayout::new(db_a, handler);
        layout.workspace.set_scope(&[root_a.clone(), root_b.clone()]).expect("scope both repos");
        layout.after_scope_change();

        // -- Assertion 2: multi-repo mode is active. --
        assert!(layout.workspace.is_multi(), "both repos checked => is_multi");

        // -- Assertion 1: the merged Bookmarks panel holds BOTH repos' bookmarks, --
        // -- each tagged with its owning repo (real panel items, not just pixels). --
        let bookmarks: Vec<(String, Option<String>, Option<String>)> = layout
            .left_pane
            .content_panel
            .get_list_panel_mut(ContentTab::Bookmarks.index())
            .expect("bookmarks panel")
            .all_items()
            .iter()
            .map(|i| {
                (
                    i.text().to_string(),
                    i.repo_name().map(str::to_string),
                    i.repo_root().map(str::to_string),
                )
            })
            .collect();
        let a_row = bookmarks
            .iter()
            .find(|(_, _, root)| root.as_deref() == Some(&*root_a.to_string_lossy()))
            .expect("repo A's bookmark is in the merged list");
        let b_row = bookmarks
            .iter()
            .find(|(_, _, root)| root.as_deref() == Some(&*root_b.to_string_lossy()))
            .expect("repo B's bookmark is in the merged list");
        // Each row is tagged with its repo's display name (dir file_name).
        assert_eq!(a_row.1.as_deref(), root_a.file_name().and_then(|n| n.to_str()));
        assert_eq!(b_row.1.as_deref(), root_b.file_name().and_then(|n| n.to_str()));

        // Both repo names surface in the rendered list too (multi-repo replaces the
        // author metadata with the bare repo name on each row).
        let name_a = root_a.file_name().unwrap().to_string_lossy().to_string();
        let name_b = root_b.file_name().unwrap().to_string_lossy().to_string();
        let screen = render_layout(&layout, 120, 32);
        assert!(
            screen.contains(&name_a) && screen.contains(&name_b),
            "both repo names should be visible in the merged list; got:\n{screen}"
        );

        // -- Assertion 3: selecting repo B's bookmark resolves against repo B's db. --
        // Pin the selection to B's row by its stable id and enter the Content panel,
        // then drive the synchronous preview the real focus-enter path uses.
        layout.set_focus(FocusArea::ContentPanel);
        assert!(
            layout
                .left_pane
                .content_panel
                .get_list_panel_mut(ContentTab::Bookmarks.index())
                .expect("bookmarks panel")
                .select_by_user_data("bm-beta"),
            "repo B's bookmark row must be selectable by id"
        );
        layout.preview_content_item(ContentTab::Bookmarks, "bm-beta");
        let preview = render_layout(&layout, 120, 32);
        assert!(
            preview.contains("src/b.rs") && preview.contains("fn beta"),
            "selecting repo B's bookmark should preview B's file/code; got:\n{preview}"
        );

        // -- Assertion 4: the Tours tab is hidden in multi-repo mode. --
        // (Independent of login state — see `update_tours_tab_visibility`.)
        assert!(!tours_tab_visible(&mut layout), "multi-repo hides the Tours tab");

        // -- Assertion 5: delete targets the OWNING repo (B), leaving A untouched. --
        // Drive the real key flow: 'd' opens the confirm dialog, → selects Confirm,
        // Enter performs the delete against the selected row's db (repo B).
        assert_eq!(
            layout.handle_event(&Event::Key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE))),
            true,
            "'d' on a bookmark opens the delete-confirm dialog"
        );
        // The dialog captures input; move focus onto Confirm, then press Enter.
        layout.handle_event(&Event::Key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)));
        layout.handle_event(&Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
        assert!(!layout.has_active_dialog(), "dialog closes after confirming the delete");

        // Repo B lost its bookmark; repo A's is untouched — proving the delete was
        // routed to the selected row's owning db, not the focus repo.
        let db_b = Database::open(&root_b.join(".codemark").join("codemark.db")).expect("reopen B");
        let db_a2 =
            Database::open(&root_a.join(".codemark").join("codemark.db")).expect("reopen A");
        assert!(
            db_b.get_bookmark("bm-beta").expect("query B").is_none(),
            "repo B's bookmark should be deleted"
        );
        assert!(
            db_a2.get_bookmark("bm-alpha").expect("query A").is_some(),
            "repo A's bookmark must be untouched by a delete targeting repo B"
        );
    }
    struct CleanupGuard {
        registry: rusqlite::Connection,
        roots: Vec<String>,
    }
    impl Drop for CleanupGuard {
        fn drop(&mut self) {
            for root in &self.roots {
                let _ = self.registry.execute(
                    "DELETE FROM known_repos WHERE repo_root = ?1",
                    rusqlite::params![root],
                );
            }
        }
    }

    #[test]
    fn checked_repos_keep_checkmark_after_panel_refresh() {
        // Regression: build_repo_items only marks the focus repo `.active(true)`,
        // so a panel rebuild via set_items wiped every other checked repo's
        // checkmark. restore_active_repos re-activates all checked repos after
        // every rebuild so the multi-select state survives refresh.
        let root_a = temp_repo_root();
        let root_b = temp_repo_root();

        // Register both repos in the layout's registry so they appear in the
        // Repos panel (build_repo_items reads from the registry).
        let db = Database::open(&root_a.join(".codemark").join("codemark.db")).expect("open A");
        let handler = EventHandler::new(EventHandlerConfig::default()).expect("handler");
        let mut layout = BrowserLayout::new(db, handler);

        // Guard ensures cleanup runs even if assertions below panic.
        let _guard = CleanupGuard {
            registry: codemark_core::storage::registry::open_registry().expect("open registry"),
            roots: vec![
                root_a.to_string_lossy().into_owned(),
                root_b.to_string_lossy().into_owned(),
            ],
        };

        for root in [&root_a, &root_b] {
            codemark_core::storage::registry::upsert_repo(
                &layout.registry,
                &codemark_core::storage::registry::RepoUpsert {
                    id: &format!("test-cm-{}", root.file_name().unwrap().to_string_lossy()),
                    repo_owner: "owner",
                    repo_name: root.file_name().unwrap().to_str().unwrap(),
                    origin_url: None,
                    repo_root: &root.to_string_lossy(),
                    db_owner_email: "test@example.com",
                    db_owner_name: None,
                    server_url: None,
                    default_username: None,
                },
            )
            .expect("register repo");
        }

        // Check both repos — the exact set_scope + after_scope_change path the
        // multi-select Repos toggle drives at runtime.
        layout.workspace.set_scope(&[root_a.clone(), root_b.clone()]).expect("scope both");
        layout.after_scope_change();

        // Both test repos must show as active (checked) in the Repos panel.
        // (The global registry may hold many repos from the dev machine —
        // filter to just the two we registered by their root path.)
        let root_a_str = root_a.to_string_lossy().to_string();
        let root_b_str = root_b.to_string_lossy().to_string();
        let panel = layout
            .left_pane
            .context_panel
            .get_list_panel_mut(ContextTab::Repos.index())
            .expect("repos panel");
        let active_a = panel
            .all_items()
            .iter()
            .find(|i| i.user_data.as_deref() == Some(&root_a_str))
            .expect("repo A row exists")
            .is_active();
        let active_b = panel
            .all_items()
            .iter()
            .find(|i| i.user_data.as_deref() == Some(&root_b_str))
            .expect("repo B row exists")
            .is_active();
        assert!(active_a, "repo A (focus) must be checked after refresh");
        assert!(active_b, "repo B must be checked after refresh — this is the regression");

        // Uncheck repo B — only repo A should be active after refresh.
        layout.workspace.set_scope(&[root_a.clone()]).expect("scope A only");
        layout.after_scope_change();

        let active_b = layout
            .left_pane
            .context_panel
            .get_list_panel_mut(ContextTab::Repos.index())
            .expect("repos panel")
            .all_items()
            .iter()
            .find(|i| i.user_data.as_deref() == Some(&root_b.to_string_lossy()))
            .expect("repo B row exists")
            .is_active();

        assert!(!active_b, "unchecking repo B must clear its checkmark after refresh");
    }
}
