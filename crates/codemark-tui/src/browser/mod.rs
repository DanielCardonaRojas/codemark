//! Browser layout for the Codemark TUI.
//!
//! This module provides the main browser layout with a left sidebar
//! containing search, repos, and tours, and a right main content area.

mod bindings;
mod data;
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
    ExternalCommand, FocusArea, HealNotification, HealTarget, LeftPaneSize, RightPaneSize,
    SectionConfig, StepData, TabContent, escape_markdown,
};

use crate::component::{Component, HealthStatus, PanelItem};
use crate::event::Event;
use codemark_core::config::Config;
use codemark_core::embeddings::config::EmbeddingModel;
use codemark_core::engine::bookmark::{Bookmark, BookmarkFilter};
use codemark_core::parser::languages::Language;
use codemark_core::query::classifier::get_node_icon;
use codemark_core::query::summarizer;
use codemark_core::storage::{SemanticRepo, db::Database};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
};

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
        };
        layout.update_focus_state();
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
            self.right_pane.load_bookmark(&self.db, id);
        }
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
                    match handle.block_on(semantic_repo.search(db.conn(), &query, 20)) {
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
            self.refresh_all_panels();
        }
        Ok(())
    }

    /// Start healing the currently selected bookmark(s) based on focus.
    ///
    /// This spawns an async background task to perform the heal operation.
    /// When complete, a HealComplete event will be sent with the result.
    pub fn start_heal_selection(&mut self) {
        let db_path = self.db.path().to_path_buf();
        let event_handler = self.event_handler.clone();

        // Determine the heal target based on current focus
        let target = match self.focus {
            FocusArea::Panel3 => {
                if let Some(panel) = self.left_pane.panel3.active_panel() {
                    match tabs::Panel3Tab::from_index(self.left_pane.panel3.tabs.selected_index()) {
                        Some(tabs::Panel3Tab::Bookmarks) => {
                            // Heal selected bookmark
                            panel
                                .selected()
                                .and_then(|s| s.user_data.clone())
                                .map(HealTarget::Bookmark)
                        }
                        Some(tabs::Panel3Tab::Collections) | Some(tabs::Panel3Tab::Tours) => {
                            // Heal all bookmarks in collection/tour
                            panel.selected().and_then(|s| {
                                if let Some(id) = &s.user_data {
                                    Some(HealTarget::Collection(id.clone()))
                                } else {
                                    // Fallback to name lookup if user_data is missing
                                    let name = s.text().to_string();
                                    self.db
                                        .get_collection_by_name(&name)
                                        .ok()
                                        .flatten()
                                        .map(|c| HealTarget::Collection(c.id))
                                }
                            })
                        }
                        None => None,
                    }
                } else {
                    None
                }
            }
            FocusArea::Main => {
                // Heal the currently displayed bookmark in preview
                self.right_pane
                    .steps_data
                    .get(self.right_pane.pager_current)
                    .map(|step| HealTarget::Bookmark(step.bookmark.id.clone()))
            }
            _ => None,
        };

        let Some(target) = target else {
            // No valid target - show error
            let _ = event_handler
                .send(Event::HealComplete("Nothing selected to heal".to_string(), false));
            return;
        };

        // Spawn a background task to perform the heal.
        // Note: We use spawn_blocking because Database is not Send/Sync (rusqlite limitation).
        // This runs the blocking database operations on a dedicated thread pool.
        tokio::task::spawn_blocking(move || {
            let handle = tokio::runtime::Handle::current();
            handle.block_on(data::perform_heal(db_path, target, event_handler))
        });
    }

    /// Refresh all panels from the current active database.
    pub fn refresh_all_panels(&mut self) {
        // 1. Update Panel 1 Repos (in-place to preserve selection)
        let repo_items = TabbedPanel::build_repo_items(&self.db, &self.registry);
        if let Some(p) = self.left_pane.panel1.get_list_panel_mut(0) {
            p.set_items(repo_items);
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

        // 4. Update Step previews (Right Pane)
        let current_step = self.right_pane.pager_current;
        if let Some(tour_name) = self.right_pane.active_tour_name.clone() {
            self.right_pane.load_tour(&self.db, &tour_name);
            // Restore step if possible
            if current_step < self.right_pane.pager_total {
                self.right_pane.pager_current = current_step;
                self.right_pane.update_preview(&self.db);
            }
        } else if let Some(bm_id) = self.right_pane.active_bookmark_id.clone() {
            self.right_pane.load_bookmark(&self.db, &bm_id);
        } else if let Ok(collections) = self.db.list_collections() {
            // Default to first tour only if nothing was active
            if let Some((first_tour, _)) = collections.first() {
                let name = first_tour.name.clone();
                self.right_pane.load_tour(&self.db, &name);
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

        Line::from(spans)
    }

    /// Open the currently selected bookmark or tour step in the editor.
    pub fn open_in_editor(&mut self) {
        use codemark_core::config::Config;

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

        let command_template = if let Some(cmd) =
            config.open.get_command_for_extension(extension).or(config.open.default.as_ref())
        {
            cmd.clone()
        } else {
            let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vim".to_string());
            format!("{} {{FILE}}", editor)
        };

        // Substitute placeholders
        // Note: StepData already has resolved absolute path and line number
        let line_start = step.line_number + 1;
        let substituted = command_template
            .replace("{FILE}", &step.file_path)
            .replace("{LINE_START}", &line_start.to_string())
            .replace("{LINE_END}", &line_start.to_string())
            .replace("{ID}", &step.bookmark.id);

        if let Some(tokens) = shlex::split(&substituted)
            && !tokens.is_empty()
        {
            let program = tokens[0].clone();
            let args = tokens[1..].to_vec();
            let program_name =
                std::path::Path::new(&program).file_name().and_then(|n| n.to_str()).unwrap_or("");
            let should_wait = config.open.should_wait_for_editor(program_name);

            self.pending_command = Some(ExternalCommand { program, args, should_wait });
        }
    }

    /// Open a bookmark directly in the editor (used for Panel3 bookmarks).
    fn open_bookmark_in_editor(&mut self, bookmark: Bookmark) {
        use codemark_core::config::Config;

        let Some(codemark_dir) = self.db.path().parent() else {
            return;
        };
        let config = Config::load_layered(codemark_dir);

        // Resolve the file path and line number from the bookmark
        let (file_path, line_number) =
            if let Ok(Some(resolution)) = self.db.get_resolution(&bookmark.id) {
                (
                    resolution.file_path.clone().unwrap_or_else(|| bookmark.file_path.clone()),
                    resolution
                        .line_range
                        .and_then(|r| {
                            // Parse "(start,end)" format
                            let parts: Vec<&str> = r.split(',').collect();
                            if parts.len() == 2 {
                                parts[0].trim().trim_start_matches('(').parse::<usize>().ok()
                            } else {
                                None
                            }
                        })
                        .unwrap_or(0),
                )
            } else {
                // Fallback to bookmark file path
                (bookmark.file_path.clone(), 0)
            };

        let extension =
            std::path::Path::new(&file_path).extension().and_then(|e| e.to_str()).unwrap_or("");

        let command_template = if let Some(cmd) =
            config.open.get_command_for_extension(extension).or(config.open.default.as_ref())
        {
            cmd.clone()
        } else {
            let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vim".to_string());
            format!("{} {{FILE}}", editor)
        };

        // Substitute placeholders
        let line_start = line_number + 1;
        let substituted = command_template
            .replace("{FILE}", &file_path)
            .replace("{LINE_START}", &line_start.to_string())
            .replace("{LINE_END}", &line_start.to_string())
            .replace("{ID}", &bookmark.id);

        if let Some(tokens) = shlex::split(&substituted)
            && !tokens.is_empty()
        {
            let program = tokens[0].clone();
            let args = tokens[1..].to_vec();
            let program_name =
                std::path::Path::new(&program).file_name().and_then(|n| n.to_str()).unwrap_or("");
            let should_wait = config.open.should_wait_for_editor(program_name);

            self.pending_command = Some(ExternalCommand { program, args, should_wait });
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
                    let item = PanelItem::new(c.name)
                        .secondary_text(c.created_branch.unwrap_or_else(|| "main".to_string()))
                        .metadata(format!("{count} steps"))
                        .health(health)
                        .published(is_published);

                    collection_items.push(item.clone());
                    if is_published {
                        tour_items.push(item);
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
            let filtered_items: Vec<PanelItem> = bookmarks
                .iter()
                .filter(|bm| {
                    let branch_match = active_branches.is_empty(); // Bookmarks don't have direct branch column in this version
                    let tag_match =
                        active_tags.is_empty() || bm.tags.iter().any(|t| active_tags.contains(t));
                    branch_match && tag_match
                })
                .map(|bm| bookmark_to_panel_item(bm, &self.db, false))
                .collect();

            if let Some(TabContent::List(p)) = self.left_pane.panel3.panels.get_mut(0) {
                p.set_items(filtered_items);
            }
        }
    }

    /// Set the focus area.
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
        let (left_constraints, render_mode) = if self.right_pane_size.is_fullscreen() {
            // Right pane takes full width
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

        match render_mode {
            RenderMode::Both => {
                self.left_pane.render(chunks[0], buf);
                self.right_pane.render(chunks[1], buf, false);
            }
            RenderMode::LeftOnly => {
                self.left_pane.render(chunks[0], buf);
            }
            RenderMode::RightOnly => {
                self.right_pane.render(chunks[0], buf, true);
            }
        }
    }

    /// Handle an event.
    pub fn handle_event(&mut self, event: &Event) -> bool {
        // Handle search results and errors
        match event {
            Event::SearchResults(bookmarks) => {
                let items: Vec<PanelItem> = bookmarks
                    .iter()
                    .map(|bm| {
                        // Project UI status for the bookmark
                        let bm = codemark_core::engine::projection::project_ui_status_for_bookmark(
                            bm.clone(),
                            &self.db,
                            None,
                        )
                        .unwrap_or(bm.clone());

                        let health = match bm.ui_status.as_deref() {
                            Some("healthy") => HealthStatus::Healthy,
                            Some("unanchored_healthy") => HealthStatus::UnanchoredHealthy,
                            Some("drifted") => HealthStatus::Drifted,
                            Some("unanchored_drifting") => HealthStatus::UnanchoredDrifting,
                            Some("broken") => HealthStatus::Broken,
                            Some("broken_unanchored") => HealthStatus::BrokenUnanchored,
                            Some("verified") => HealthStatus::Verified,
                            Some("outdated") => HealthStatus::Outdated,
                            Some("future") => HealthStatus::Future,
                            _ => HealthStatus::Unknown,
                        };

                        // Try to get a summary from the query for better display
                        let summary_info = bm.language.parse::<Language>().ok().and_then(|lang| {
                            summarizer::summarize_query(&bm.query, Some(lang)).ok()
                        });

                        let summary = summary_info
                            .as_ref()
                            .and_then(|s| s.format())
                            .unwrap_or_else(|| bm.query.clone());

                        let icon =
                            summary_info.as_ref().map(|s| get_node_icon(&s.label)).unwrap_or("");

                        // Shrink the file path to prioritize last path components
                        let short_path = shorten_path(&bm.file_path, 25);

                        // Format: short_file_path summary (or query if summarization failed)
                        let display_text = if summary.is_empty() {
                            short_path
                        } else {
                            format!("{} {}", short_path, summary)
                        };

                        PanelItem::new(display_text)
                            .metadata(bm.created_by.clone().unwrap_or_default())
                            .health(health)
                            .icon(icon)
                            .user_data(bm.id.clone())
                    })
                    .collect();

                if let Some(TabContent::List(p)) = self.left_pane.panel3.panels.get_mut(0) {
                    p.set_items(items);
                    // Select the first bookmark result
                    p.set_selected(0);
                    // Ensure the Bookmarks tab is selected
                    self.left_pane.panel3.tabs.set_selected(0);
                }
                return true;
            }
            Event::SearchError(msg) => {
                // Store the error message in the search bar for display
                self.left_pane.search.set_error(msg.clone());
                return true;
            }
            Event::HealComplete(msg, success) => {
                // Store the heal result as a notification
                self.pending_notification =
                    Some(HealNotification { message: msg.clone(), success: *success });
                // Refresh panels to show updated health status
                self.refresh_all_panels();
                return true;
            }
            _ => {}
        }

        // 1. Handle mouse clicks for focus switching
        if let Event::Mouse(mouse) = event
            && let ratatui::crossterm::event::MouseEventKind::Down(
                ratatui::crossterm::event::MouseButton::Left,
            ) = mouse.kind
        {
            let col = mouse.column;
            let row = mouse.row;

            // Check each section for focus switching
            let search_area = self.left_pane.search.last_area();
            if col >= search_area.x
                && col < search_area.x + search_area.width
                && row >= search_area.y
                && row < search_area.y + search_area.height
            {
                self.set_focus(FocusArea::Search);
            } else {
                let p1_area = self.left_pane.panel1.last_area();
                if col >= p1_area.x
                    && col < p1_area.x + p1_area.width
                    && row >= p1_area.y
                    && row < p1_area.y + p1_area.height
                {
                    self.set_focus(FocusArea::Panel1);
                } else {
                    let p2_area = self.left_pane.panel2.last_area();
                    if col >= p2_area.x
                        && col < p2_area.x + p2_area.width
                        && row >= p2_area.y
                        && row < p2_area.y + p2_area.height
                    {
                        self.set_focus(FocusArea::Panel2);
                    } else {
                        let p3_area = self.left_pane.panel3.last_area();
                        if col >= p3_area.x
                            && col < p3_area.x + p3_area.width
                            && row >= p3_area.y
                            && row < p3_area.y + p3_area.height
                        {
                            self.set_focus(FocusArea::Panel3);
                        } else {
                            let right_area = self.right_pane.last_area();
                            if col >= right_area.x
                                && col < right_area.x + right_area.width
                                && row >= right_area.y
                                && row < right_area.y + right_area.height
                            {
                                self.set_focus(FocusArea::Main);
                            }
                        }
                    }
                }
            }
        }

        // 2. Handle focus cycling and number shortcuts (Keys only)
        if let Event::Key(key) = event {
            match key.code {
                ratatui::crossterm::event::KeyCode::Enter => {
                    if self.focus == FocusArea::Search {
                        self.execute_search();
                        return true;
                    }
                    if self.focus == FocusArea::Panel1
                        && let Some(panel) = self.left_pane.panel1.active_panel_mut()
                        && let Some(selected) = panel.selected()
                        && let Some(root) = selected.user_data.as_ref()
                    {
                        let root = root.clone();
                        panel.activate_selected();
                        // Only shift focus to bookmarks if database switch succeeds
                        if self.switch_database(&root).is_ok() {
                            self.set_focus(FocusArea::Panel3);
                        }
                        return true;
                    }
                    if self.focus == FocusArea::Panel2
                        && let Some(panel) = self.left_pane.panel2.active_panel_mut()
                    {
                        panel.activate_selected();
                        self.update_tours_collections();
                        return true;
                    }
                    if self.focus == FocusArea::Panel3 {
                        match Panel3Tab::from_index(self.left_pane.panel3.tabs.selected_index()) {
                            Some(Panel3Tab::Tours) | Some(Panel3Tab::Collections) => {
                                // Tours or Collections
                                if let Some(panel) = self.left_pane.panel3.active_panel_mut()
                                    && let Some(selected) = panel.selected()
                                {
                                    let tour_name = selected.text().to_string();
                                    panel.activate_selected(); // Mark as active in current panel
                                    self.right_pane.load_tour(&self.db, &tour_name);

                                    self.set_focus(FocusArea::Main);
                                    return true;
                                }
                            }
                            Some(Panel3Tab::Bookmarks) => {
                                // Bookmarks
                                if let Some(panel) = self.left_pane.panel3.active_panel_mut()
                                    && let Some(selected) = panel.selected()
                                    && let Some(id) = selected.user_data.clone()
                                {
                                    panel.activate_selected();
                                    self.right_pane.load_bookmark(&self.db, &id);
                                    self.set_focus(FocusArea::Main);
                                    return true;
                                }
                            }
                            None => {}
                        }
                    }
                }
                ratatui::crossterm::event::KeyCode::Char(' ') => {
                    if self.focus == FocusArea::Panel1
                        && let Some(panel) = self.left_pane.panel1.active_panel_mut()
                        && let Some(selected) = panel.selected()
                        && let Some(root) = selected.user_data.as_ref()
                    {
                        let root = root.clone();
                        panel.activate_selected();
                        let _ = self.switch_database(&root);
                        return true;
                    }
                    if self.focus == FocusArea::Panel2
                        && let Some(panel) = self.left_pane.panel2.active_panel_mut()
                    {
                        panel.activate_selected();
                        self.update_tours_collections();
                        return true;
                    }
                    if self.focus == FocusArea::Panel3 {
                        match Panel3Tab::from_index(self.left_pane.panel3.tabs.selected_index()) {
                            Some(Panel3Tab::Tours) | Some(Panel3Tab::Collections) => {
                                // Tours or Collections
                                if let Some(panel) = self.left_pane.panel3.active_panel_mut()
                                    && let Some(selected) = panel.selected()
                                {
                                    let tour_name = selected.text().to_string();
                                    panel.activate_selected(); // Mark as active in current panel
                                    self.right_pane.load_tour(&self.db, &tour_name);

                                    self.set_focus(FocusArea::Main);
                                    return true;
                                }
                            }
                            Some(Panel3Tab::Bookmarks) => {
                                // Bookmarks
                                if let Some(panel) = self.left_pane.panel3.active_panel_mut()
                                    && let Some(selected) = panel.selected()
                                    && let Some(id) = selected.user_data.clone()
                                {
                                    panel.activate_selected();
                                    self.right_pane.load_bookmark(&self.db, &id);
                                    self.set_focus(FocusArea::Main);
                                    return true;
                                }
                            }
                            None => {}
                        }
                    }
                }
                ratatui::crossterm::event::KeyCode::Tab => {
                    if !self.should_handle_keybindings() {
                        return false;
                    }
                    self.next_focus();
                    return true;
                }
                ratatui::crossterm::event::KeyCode::BackTab => {
                    if !self.should_handle_keybindings() {
                        return false;
                    }
                    self.previous_focus();
                    return true;
                }
                ratatui::crossterm::event::KeyCode::Esc => {
                    if self.focus == FocusArea::Search {
                        // Clear search results and restore original lists
                        self.left_pane.search.clear();
                        self.refresh_all_panels();
                        self.set_focus(FocusArea::Panel3);
                        return true;
                    }
                    if self.focus == FocusArea::Main {
                        self.set_focus(FocusArea::Panel3);
                        return true;
                    }
                }
                // Number keys for direct section access (disabled when in filter mode or Search focused)
                ratatui::crossterm::event::KeyCode::Char('1')
                    if self.should_handle_keybindings() && self.focus != FocusArea::Search =>
                {
                    self.focus = FocusArea::Search;
                    self.update_focus_state();
                    return true;
                }
                ratatui::crossterm::event::KeyCode::Char('2')
                    if self.should_handle_keybindings() && self.focus != FocusArea::Search =>
                {
                    self.focus = FocusArea::Panel1;
                    self.update_focus_state();
                    return true;
                }
                ratatui::crossterm::event::KeyCode::Char('3')
                    if self.should_handle_keybindings() && self.focus != FocusArea::Search =>
                {
                    self.focus = FocusArea::Panel2;
                    self.update_focus_state();
                    return true;
                }
                ratatui::crossterm::event::KeyCode::Char('4')
                    if self.should_handle_keybindings() && self.focus != FocusArea::Search =>
                {
                    self.focus = FocusArea::Panel3;
                    self.update_focus_state();
                    return true;
                }
                ratatui::crossterm::event::KeyCode::Char('5')
                    if self.should_handle_keybindings() && self.focus != FocusArea::Search =>
                {
                    self.focus = FocusArea::Main;
                    self.right_pane.focus_steps();
                    self.update_focus_state();
                    return true;
                }
                ratatui::crossterm::event::KeyCode::Char('6')
                    if self.should_handle_keybindings() && self.focus != FocusArea::Search =>
                {
                    self.focus = FocusArea::Main;
                    self.right_pane.focus_details();
                    self.update_focus_state();
                    return true;
                }
                ratatui::crossterm::event::KeyCode::Char('o')
                    if self.should_handle_keybindings()
                        && self.focus != FocusArea::Search
                        && !key
                            .modifiers
                            .contains(ratatui::crossterm::event::KeyModifiers::CONTROL) =>
                {
                    self.open_in_editor();
                    return true;
                }
                // Copy ID keybinding (Ctrl+O) - only available in Bookmarks or Collections tabs
                ratatui::crossterm::event::KeyCode::Char('o')
                    if self.should_handle_keybindings()
                        && self.focus == FocusArea::Panel3
                        && key
                            .modifiers
                            .contains(ratatui::crossterm::event::KeyModifiers::CONTROL) =>
                {
                    match Panel3Tab::from_index(self.left_pane.panel3.tabs.selected_index()) {
                        Some(Panel3Tab::Bookmarks) | Some(Panel3Tab::Collections) => {
                            if let Some(panel) = self.left_pane.panel3.active_panel_mut()
                                && let Some(selected) = panel.selected()
                                && let Some(id) = selected.user_data.clone()
                            {
                                match self.copy_to_clipboard(&id) {
                                    Ok(()) => {
                                        self.pending_notification = Some(HealNotification {
                                            message: format!("Copied ID: {}", id),
                                            success: true,
                                        });
                                    }
                                    Err(e) => {
                                        self.pending_notification = Some(HealNotification {
                                            message: format!("Failed to copy: {}", e),
                                            success: false,
                                        });
                                    }
                                }
                            } else {
                                self.pending_notification = Some(HealNotification {
                                    message: "No item selected".to_string(),
                                    success: false,
                                });
                            }
                        }
                        _ => {}
                    }
                    return true;
                }
                // Copy markdown keybinding (Ctrl+O) - available when focus is on Main area
                ratatui::crossterm::event::KeyCode::Char('o')
                    if self.should_handle_keybindings()
                        && self.focus == FocusArea::Main
                        && key
                            .modifiers
                            .contains(ratatui::crossterm::event::KeyModifiers::CONTROL) =>
                {
                    if let Some(markdown) =
                        self.right_pane.active_markdown_content().map(|m| m.to_owned())
                    {
                        match self.copy_to_clipboard(&markdown) {
                            Ok(()) => {
                                self.pending_notification = Some(HealNotification {
                                    message: "Copied markdown content".to_string(),
                                    success: true,
                                });
                            }
                            Err(e) => {
                                self.pending_notification = Some(HealNotification {
                                    message: format!("Failed to copy: {}", e),
                                    success: false,
                                });
                            }
                        }
                    } else {
                        self.pending_notification = Some(HealNotification {
                            message: "No markdown content to copy".to_string(),
                            success: false,
                        });
                    }
                    return true;
                }
                // Delete collection or bookmark based on active tab
                ratatui::crossterm::event::KeyCode::Char('d')
                    if self.should_handle_keybindings() && self.focus == FocusArea::Panel3 =>
                {
                    match Panel3Tab::from_index(self.left_pane.panel3.tabs.selected_index()) {
                        Some(Panel3Tab::Collections) => {
                            if let Some(panel) = self.left_pane.panel3.active_panel_mut()
                                && let Some(selected) = panel.selected()
                            {
                                let collection_name = selected.text().to_string();
                                if let Ok(Some(_collection)) =
                                    self.db.get_collection_by_name(&collection_name)
                                {
                                    let _ = self.db.delete_collection(&collection_name);
                                    self.refresh_all_panels();
                                    return true;
                                }
                            }
                        }
                        Some(Panel3Tab::Bookmarks) => {
                            if let Some(panel) = self.left_pane.panel3.active_panel_mut()
                                && let Some(selected) = panel.selected()
                                && let Some(id) = selected.user_data.clone()
                            {
                                let _ = self.db.delete_bookmark(&id);
                                self.refresh_all_panels();
                                return true;
                            }
                        }
                        _ => {}
                    }
                }
                // Heal keybinding - only available in Bookmarks, Collections, or Preview
                ratatui::crossterm::event::KeyCode::Char('H')
                    if self.should_handle_keybindings()
                        && (self.focus == FocusArea::Panel3 || self.focus == FocusArea::Main) =>
                {
                    self.start_heal_selection();
                    return true;
                }
                // Increase pane size with + (only for resizable panels)
                ratatui::crossterm::event::KeyCode::Char('+')
                | ratatui::crossterm::event::KeyCode::Char('=')
                    if self.should_handle_keybindings() && self.focus.is_resizable() =>
                {
                    if self.focus == FocusArea::Main {
                        let next = self.right_pane_size.toggle();
                        self.right_pane_size = next;
                        if next.is_fullscreen() {
                            self.right_pane.focus_steps();
                        }
                    } else {
                        self.left_pane_size = self.left_pane_size.increase();
                        self.left_pane.set_resize_mode(self.left_pane_size);
                        self.left_pane.set_focused_area(self.focus);
                    }
                    return true;
                }
                // Decrease pane size with _ (only for resizable panels)
                ratatui::crossterm::event::KeyCode::Char('_')
                | ratatui::crossterm::event::KeyCode::Char('-')
                    if self.should_handle_keybindings() && self.focus.is_resizable() =>
                {
                    if self.focus == FocusArea::Main {
                        let next = self.right_pane_size.toggle();
                        self.right_pane_size = next;
                        if next.is_fullscreen() {
                            self.right_pane.focus_steps();
                        }
                    } else {
                        self.left_pane_size = self.left_pane_size.decrease();
                        self.left_pane.set_resize_mode(self.left_pane_size);
                        self.left_pane.set_focused_area(self.focus);
                    }
                    return true;
                }
                _ => {}
            }
        }

        // 3. Delegate to panes
        match event {
            Event::Mouse(_) => {
                let old_tab = self.left_pane.panel3.tabs.selected_index();

                // Always delegate mouse events to both panes to allow hovering/scrolling
                // regardless of focus.
                let left_handled = self.left_pane.handle_event(event);
                let right_handled = self.right_pane.handle_event(event);
                let handled = left_handled || right_handled;

                // Refresh tags if Panel 3 tab changed via mouse
                if self.left_pane.panel3.tabs.selected_index() != old_tab {
                    self.refresh_tags();
                }

                // Check for bookmark selection changes for live preview after mouse events
                // Only if focus is actually on Panel3
                if self.focus == FocusArea::Panel3
                    && let Some(id) = self.left_pane.panel3.take_selection_change()
                {
                    self.right_pane.load_bookmark(&self.db, &id);
                }
                handled
            }
            Event::Key(_key) => {
                // Don't delegate key events to panels when in filter mode
                // They should only be handled by the state handler for the filter buffer
                if !self.should_handle_keybindings() {
                    return false;
                }

                let old_tab = self.left_pane.panel3.tabs.selected_index();
                let handled = match self.focus {
                    FocusArea::Search => self.left_pane.search.handle_event(event),
                    FocusArea::Panel1 => self.left_pane.panel1.handle_event(event),
                    FocusArea::Panel2 => self.left_pane.panel2.handle_event(event),
                    FocusArea::Panel3 => {
                        let handled = self.left_pane.panel3.handle_event(event);
                        // Check for bookmark selection changes for live preview
                        if let Some(id) = self.left_pane.panel3.take_selection_change() {
                            self.right_pane.load_bookmark(&self.db, &id);
                        }
                        handled
                    }
                    FocusArea::Main => self.right_pane.handle_event(event),
                    FocusArea::Filter => false,
                };

                // Refresh tags if Panel 3 tab changed via keyboard (e.g., [ or ])
                if self.left_pane.panel3.tabs.selected_index() != old_tab {
                    self.refresh_tags();
                }

                handled
            }
            _ => false,
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
