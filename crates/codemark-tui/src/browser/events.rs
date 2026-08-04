//! Event handling for the Browser layout.
//!
//! This module splits the monolithic `handle_event` into smaller, focused methods
//! organized by event type: tick, app-level, mouse, key, and delegation.

use std::sync::Arc;

use crate::component::{Component, HealthStatus, PanelItem};
use crate::event::Event;
use codemark_core::engine::resolution::LiveUIStatus;
use codemark_core::parser::languages::Language;
use codemark_core::query::classifier::get_node_icon;
use codemark_core::query::summarizer;

use super::{
    BrowserLayout, ConfirmDialog, ContentTab, ContextTab, DetailsPaneSize, DialogAction, FocusArea,
    HealNotification, RightPaneFocus, RightPaneSize, TabContent, shorten_path,
};

impl BrowserLayout {
    /// Handle an event, dispatching to the appropriate sub-handler.
    pub fn handle_event(&mut self, event: &Event) -> bool {
        // A modal dialog captures all key/mouse input while visible. Tick and
        // app-level events still fall through so background tasks keep updating.
        if self.has_active_dialog() {
            match event {
                Event::Key(key) => return self.handle_dialog_key(key),
                Event::Mouse(_) => return true,
                _ => {}
            }
        }

        if matches!(event, Event::Tick) {
            return self.handle_tick_event();
        }

        if let Some(handled) = self.handle_app_event(event) {
            return handled;
        }

        if let Event::Mouse(_) = event {
            self.handle_mouse_focus(event);
        }

        if let Event::Key(key) = event
            && let Some(handled) = self.handle_key_event(key)
        {
            return handled;
        }

        if self.handle_delegation(event) {
            return true;
        }

        // Enter on a collection/tour overview enters the bookmarks flow (or pulls
        // a remote tour), mirroring Enter on the Content list. It runs only after
        // delegation so a focused overview link still opens on Enter; otherwise
        // the overview markdown panel swallows the key and Enter does nothing.
        if let Event::Key(key) = event
            && key.code == ratatui::crossterm::event::KeyCode::Enter
            && self.focus == FocusArea::Main
            && self.right_pane.overview_active
            && self.enter_active_overview()
        {
            return true;
        }

        if let Event::Key(key) = event
            && key.code == ratatui::crossterm::event::KeyCode::Esc
            && self.focus == FocusArea::Main
        {
            self.set_focus(FocusArea::ContentPanel);
            return true;
        }

        false
    }

    /// Enter the collection/tour currently shown as a right-pane overview,
    /// switching from the overview to the per-step bookmarks flow. For a remote
    /// (not-yet-pulled) tour overview, this pulls the tour instead. Returns
    /// whether anything was activated.
    ///
    /// Mirrors Enter on the Collections / Tours content list (see
    /// [`activate_content_selection`](Self::activate_content_selection)) so the
    /// overview preview is actionable once the right pane is focused.
    fn enter_active_overview(&mut self) -> bool {
        if let Some(tour_name) = self.right_pane.active_tour_name.clone() {
            tracing::debug!(
                target: "codemark::ui",
                %tour_name,
                "entering collection overview -> bookmarks flow"
            );
            self.right_pane.load_tour_live(&self.db, &tour_name, &mut self.session_cache);
            // Only the first step was resolved live; resolve the rest off the UI
            // thread so entering the collection stays snappy.
            self.spawn_collection_live_resolve();
            self.right_pane.focus_steps();
            return true;
        }
        if let Some(remote_id) = self.right_pane.active_remote_tour_id.clone() {
            tracing::debug!(
                target: "codemark::ui",
                %remote_id,
                "entering remote tour overview -> pull"
            );
            self.start_pull_tour(remote_id);
            return true;
        }
        false
    }

    // ── Tick events ──────────────────────────────────────────────────────

    /// Advance spinner animations, fire debounced previews, and process deferred clears.
    fn handle_tick_event(&mut self) -> bool {
        // The tick counter must always advance: it drives both the list spinners
        // and the debounce clock for background previews.
        self.tick_count = self.tick_count.wrapping_add(1);

        let mut dirty = false;

        // Fire a debounced bookmark preview once the selection has settled.
        if self.maybe_spawn_pending_preview() {
            dirty = true;
        }

        // Re-render a collection/tour step preview once pager movement settles.
        if self.maybe_update_pending_step_preview() {
            dirty = true;
        }

        // Reveal the loading indicator only if a resolve outlives the grace
        // period — fast/cached resolves apply their result first, so the previous
        // preview stays on screen and nothing flashes.
        if let Some((label, since)) = &self.inflight_preview
            && !self.right_pane.is_loading()
            && self.tick_count.wrapping_sub(*since) >= super::PREVIEW_LOADING_GRACE_TICKS
        {
            let label = label.clone();
            self.right_pane.begin_loading(label);
            dirty = true;
        }

        // Animate the right-pane "Loading preview…" spinner while resolving.
        if self.right_pane.is_loading() {
            self.right_pane.advance_loading_spinner();
            dirty = true;
        }

        // Animate the search bar spinner while a search is in progress.
        if self.left_pane.search.is_loading() {
            self.left_pane.search.advance_spinner();
            dirty = true;
        }

        if self.spinning_items.is_empty() {
            return dirty;
        }

        // Check if a deferred clear is due
        if let Some(clear_at) = self.spinner_clear_at
            && self.tick_count >= clear_at
        {
            self.finish_clear_spinners();
            return true;
        }

        const SPINNER_FRAMES: &[&str] = &[
            "\u{28cb}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}", "\u{2826}",
            "\u{2827}", "\u{2807}", "\u{280f}",
        ];
        let frame = SPINNER_FRAMES[self.tick_count % SPINNER_FRAMES.len()];
        for item in &self.spinning_items {
            if let Some(panel) = self.left_pane.content_panel.get_list_panel_mut(item.tab_index) {
                panel.update_item_spinner(&item.user_data_key, Some(frame));
            }
        }
        true
    }

    /// If a debounced preview request is due, spawn the background resolve.
    /// Returns true if a task was spawned (so the frame is marked dirty).
    fn maybe_spawn_pending_preview(&mut self) -> bool {
        let due = matches!(&self.pending_preview, Some((_, _, due)) if self.tick_count >= *due);
        if !due {
            return false;
        }
        if let Some((bookmark_id, label, _)) = self.pending_preview.take() {
            self.spawn_preview_task(bookmark_id);
            // Track when the resolve started so the loading indicator can be
            // deferred past the grace period.
            self.inflight_preview = Some((label, self.tick_count));
            return true;
        }
        false
    }

    /// Render (or defer) the step preview after a pager move.
    ///
    /// The pager dots already moved instantly inside `right_pane.handle_event`;
    /// only the lag-prone re-render (file read + syntax highlight + markdown) is
    /// handled here. A discrete press — one that follows a quiet gap — renders
    /// immediately so a single step forward stays snappy. During a held key the
    /// repeats land every tick, so the synchronous render is deferred to
    /// [`maybe_update_pending_step_preview`] and runs once movement settles,
    /// rather than blocking the UI on every intermediate step.
    fn debounce_step_preview(&mut self) {
        if !self.right_pane.needs_preview_update {
            return;
        }
        self.right_pane.needs_preview_update = false;

        let settled = self
            .last_step_move_tick
            .is_none_or(|t| self.tick_count.wrapping_sub(t) >= super::STEP_MOVE_SETTLE_TICKS);
        self.last_step_move_tick = Some(self.tick_count);

        if settled {
            // Discrete press: render now for immediate feedback.
            self.step_preview_dirty = false;
            self.render_step_preview();
        } else {
            // Mid-hold: defer until the repeats stop.
            self.step_preview_dirty = true;
        }
    }

    /// Render the current step: update the code pane inline (cheap — file read +
    /// cached highlight) and kick off the expensive Info/Details/Comments
    /// markdown render on a background task. Splitting the work this way is what
    /// keeps paging snappy: the pane the user watches updates instantly while the
    /// git-projection + handlebars markdown never blocks the event loop.
    fn render_step_preview(&mut self) {
        self.right_pane.update_step_code();
        self.spawn_step_markdown_task();
    }

    /// Spawn a background task that renders the current step's Info/Details/
    /// Comments markdown and posts it back as [`Event::StepPreviewReady`].
    ///
    /// The render is pure computation — it needs only the repo path (for git
    /// ancestry projection), the step's bookmark + resolutions, HEAD, and the
    /// cached template strings — so no database handle crosses the thread
    /// boundary. Mirrors [`spawn_preview_task`](Self::spawn_preview_task) for the
    /// single-bookmark preview.
    fn spawn_step_markdown_task(&mut self) {
        let Some((bm, resolutions)) = self.right_pane.current_step_render_input() else {
            return;
        };
        // Take a fresh render id so a result from an earlier task (same bookmark,
        // older state) is superseded and dropped on arrival.
        let request_id = self.right_pane.next_step_preview_request();
        let repo_path = self.db.path().parent().unwrap_or_else(|| self.db.path()).to_path_buf();
        let head = self.right_pane.head_commit().map(|s| s.to_string());
        let (show_tpl, details_tpl, comments_tpl) = self.right_pane.markdown_templates();
        let event_handler = self.event_handler.clone();
        let bookmark_id = bm.id.clone();

        tokio::task::spawn_blocking(move || {
            let head_ref = head.as_deref();
            let render = |tpl: &str| {
                super::right_pane::render_bookmark_markdown_at(
                    &repo_path,
                    &bm,
                    &resolutions,
                    tpl,
                    head_ref,
                )
            };
            let markdown = super::StepPreviewMarkdown {
                info_markdown: render(&show_tpl),
                details_markdown: render(&details_tpl),
                comments_markdown: render(&comments_tpl),
            };
            let _ = event_handler.send(Event::StepPreviewReady {
                request_id,
                bookmark_id,
                markdown: Box::new(markdown),
            });
        });
    }

    /// Render a deferred step preview once pager movement has settled (a full
    /// quiet window since the last move). Returns true if a redraw is needed.
    /// `update_preview` reads the current pager position, so it renders the
    /// step the user landed on.
    fn maybe_update_pending_step_preview(&mut self) -> bool {
        if !self.step_preview_dirty {
            return false;
        }
        let still_moving = self
            .last_step_move_tick
            .is_some_and(|t| self.tick_count.wrapping_sub(t) < super::STEP_MOVE_SETTLE_TICKS);
        if still_moving {
            return false;
        }
        self.step_preview_dirty = false;
        self.render_step_preview();
        true
    }

    // ── App-level events (search results, heal complete, etc.) ───────────

    /// Handle application-specific events (search results, errors, sync, remote tours).
    ///
    /// Returns `Some(true)` if the event was handled, `None` if not matched.
    fn handle_app_event(&mut self, event: &Event) -> Option<bool> {
        match event {
            Event::SearchResults { request_id, bookmarks } => {
                // Apply the latest user search, or any in-flight reconcile re-run
                // (which may not be the newest request when two panels reconcile
                // at once).
                if *request_id == self.active_search_request
                    || self.is_reconcile_request(*request_id)
                {
                    self.apply_search_results(*request_id, bookmarks);
                }
                Some(true)
            }
            Event::CollectionSearchResults { request_id, collections } => {
                if *request_id == self.active_search_request
                    || self.is_reconcile_request(*request_id)
                {
                    self.apply_collection_search_results(*request_id, collections);
                }
                Some(true)
            }
            Event::SearchError { request_id, msg } => {
                if let Some(idx) = self.take_reconcile_target(*request_id) {
                    // A background semantic reconcile failed. Refresh the panel from
                    // current DB records so deleted/renamed rows still reconcile, but
                    // the semantic membership can't be recomputed, so surface the
                    // error rather than presenting the id-only refresh as a fresh
                    // reconcile — the shown results may no longer match the query.
                    if self.refresh_search_panel_by_id(idx) {
                        self.update_content_live_preview();
                    }
                    self.left_pane.search.set_error(format!("Search refresh failed: {msg}"));
                } else if *request_id == self.active_search_request {
                    self.left_pane.search.set_error(msg.clone());
                }
                Some(true)
            }
            Event::FocusGained => {
                // When the terminal regains focus, the user may have logged in or out
                // via the CLI. Reload the registry and update Tours tab visibility.
                if let Ok(registry) = codemark_core::storage::registry::open_registry() {
                    self.registry = registry;
                }
                // The user may also have installed or removed a WASM grammar via
                // `codemark languages add`/`remove` while unfocused. Re-scan the
                // grammar cache so live resolution picks up new languages instead
                // of retaining the startup snapshot until the process restarts.
                codemark_core::parser::registry::LanguageRegistry::refresh();
                // The parse caches key their `Parser`/`Profile` by language name,
                // so after a refresh an *updated* grammar would still be served
                // from the old cached parser. Drop both the synchronous session
                // cache and the background preview cache so the next resolution
                // rebuilds against the freshly discovered grammar.
                self.session_cache.clear();
                self.preview_cache.lock().unwrap_or_else(|e| e.into_inner()).clear();
                // Advance the health epoch so any live-health task spawned under
                // the previous grammar is discarded on arrival rather than
                // overwriting the panels we're about to rebuild.
                self.health_generation = self.health_generation.wrapping_add(1);
                tracing::debug!(
                    target: "codemark::ui",
                    health_generation = self.health_generation,
                    "advanced health epoch on FocusGained grammar refresh"
                );
                // Same hazard for the bookmark preview: a preview task spawned
                // before the refresh would resolve with the old grammar. Bumping
                // the request id drops any in-flight/queued preview result.
                self.cancel_inflight_preview();
                self.update_tours_tab_visibility();

                // Leaving the terminal is the only way the git HEAD moves while a
                // collection stays open (an external branch switch). Refresh the
                // cached HEAD now — before the panel rebuild below re-resolves the
                // open collection's steps — so their health projects against the
                // current checkout. Paging itself no longer refreshes HEAD (that
                // subprocess was the source of the navigation lag), so this is
                // where a branch change gets reflected in the pager dots.
                self.right_pane.refresh_head_commit(&self.db);

                // A plain `refresh_all_panels` rebuilds the Bookmarks/Collections
                // lists from the full DB set, which would drop any active search
                // narrowing (the query text lingers in the search bar but the list
                // reverts to unfiltered). Preserve the search-active panel instead,
                // so the filtered results stay put with no full-list flicker.
                self.refresh_all_panels_preserving_search();
                // Preserving keeps the rows captured before we lost focus, which
                // may reference bookmarks/collections the CLI deleted while
                // unfocused. Prune those stale rows in place (across all
                // search-active panels, not just the visible tab) so selecting
                // one can't open a missing item; the narrowed list stays put.
                self.reconcile_preserved_search_rows();
                Some(true)
            }
            Event::HealComplete(msg, success) => {
                self.pending_notification =
                    Some(HealNotification { message: msg.clone(), success: *success });
                self.schedule_clear_spinners();
                // A heal rewrites bookmark resolutions/health in the database
                // without emitting a per-bookmark live-health batch, so refresh
                // any open collection's pager dots from the updated records. This
                // runs regardless of `success`: a partial heal reports failure yet
                // still mutated the bookmarks that did resolve. It no-ops when no
                // collection is open.
                self.right_pane.refresh_step_health(&self.db);
                self.right_pane.refresh_overview_health(&self.db);
                Some(true)
            }
            Event::SyncComplete(msg, success) => {
                self.pending_notification =
                    Some(HealNotification { message: msg.clone(), success: *success });
                self.schedule_clear_spinners();
                // A sync round-trip exercises the server credentials, so the
                // login state (and thus Tours-tab visibility) may have changed
                // since this is the only runtime path that talks to the server
                // without otherwise refreshing the panels.
                self.update_tours_tab_visibility();
                // A pull can update bookmark records for an open collection (and a
                // partial pull reports failure yet still writes some), so refresh
                // its pager dots unconditionally; no-ops when no collection is open.
                self.right_pane.refresh_step_health(&self.db);
                self.right_pane.refresh_overview_health(&self.db);
                Some(true)
            }
            Event::TourPullFinished(tour_id) => {
                // The pull task settled; release the in-flight guard so a
                // follow-up activation of this tour is accepted again. Keyed by
                // tour id, so an overlapping pull of a different tour keeps its
                // own guard until its own task finishes.
                self.pulling_tour_ids.remove(tour_id);
                Some(true)
            }
            Event::RemoteToursLoaded(tours, scope) => {
                if *scope != self.pending_remote_repos {
                    return Some(true);
                }
                self.cached_remote_tours = tours.clone();
                self.rebuild_tours_panel();
                // The list now includes remote tours that weren't available when
                // the tab was first shown (the fetch is async), so the initially
                // selected item never got a preview. Render it now; otherwise it
                // stays blank until the user moves the selection.
                self.update_content_live_preview();
                Some(true)
            }
            Event::RemoteToursFetchError(msg) => {
                self.pending_notification = Some(HealNotification {
                    message: format!("Failed to fetch tours: {}", msg),
                    success: false,
                });
                Some(true)
            }
            Event::LiveHealthBatch { generation, batch } => {
                // Drop results from a superseded epoch: a grammar refresh on
                // FocusGained bumps `health_generation`, so a batch computed with
                // the old grammar must not overwrite the freshly-refreshed panels.
                if *generation != self.health_generation {
                    return Some(false);
                }
                for (bookmark_id, status) in batch {
                    let health = HealthStatus::from(*status);
                    if let Some(panel) = self.left_pane.content_panel.get_list_panel_mut(0) {
                        panel.update_item_health(bookmark_id, health);
                    }
                    // Mirror the live status into the open step so the preview
                    // border label and pager dots stay in sync with the list dots.
                    for step in &mut self.right_pane.steps_data {
                        if step.bookmark.id == *bookmark_id {
                            step.health = health;
                        }
                    }
                }
                Some(true)
            }
            Event::CollectionHealthBatch { generation, batch } => {
                if *generation != self.health_generation {
                    return Some(false);
                }
                for (collection_id, status) in batch {
                    // `None` = the collection has no live bookmarks (empty or
                    // all-archived): cache Unknown so a previously-computed
                    // Exact/Drifted/Broken can't linger and override persisted
                    // health across rebuilds.
                    let health = match status {
                        Some(s) => HealthStatus::from(*s),
                        None => HealthStatus::Unknown,
                    };
                    // Cache so subsequent panel rebuilds render the live status
                    // instead of flashing back to the persisted snapshot.
                    self.collection_live_health.insert(collection_id.clone(), health);
                    // A collection appears in both the Collections and Tours tabs,
                    // so update both dots (no-op where the panel/item is absent).
                    for idx in [ContentTab::Collections.index(), ContentTab::Tours.index()] {
                        if let Some(panel) = self.left_pane.content_panel.get_list_panel_mut(idx) {
                            panel.update_item_health(collection_id, health);
                        }
                    }
                    // Keep the open overview's border label in sync. An empty
                    // collection has no health signal, so it shows no label.
                    if self.right_pane.overview_active
                        && self.right_pane.active_collection_id.as_deref() == Some(collection_id)
                    {
                        self.right_pane.overview_health = status.map(HealthStatus::from);
                    }
                }
                Some(true)
            }
            Event::PreviewReady { request_id, payload } => {
                // Drop stale results: the selection moved on since this resolve
                // was spawned, so a newer request supersedes it.
                if *request_id == self.active_preview_request {
                    self.inflight_preview = None;
                    self.right_pane.apply_preview((**payload).clone());
                }
                Some(true)
            }
            Event::PreviewFailed { request_id, bookmark_id, unresolved } => {
                if *request_id == self.active_preview_request {
                    self.inflight_preview = None;
                    // Live resolution failed; fall back to the persisted snapshot
                    // synchronously (no tree-sitter parse, so it's cheap). The
                    // health distinguishes query-not-resolving (Unmatched) from
                    // operational failures (Error).
                    let health =
                        if *unresolved { HealthStatus::Broken } else { HealthStatus::Unknown };
                    self.right_pane.load_bookmark(&self.db, bookmark_id, health);
                    self.right_pane.finish_loading();
                }
                Some(true)
            }
            Event::StepPreviewReady { request_id, bookmark_id, markdown } => {
                // Apply only if this is still the newest render for the step on
                // screen. The request id rejects an older render of the *same*
                // bookmark that a synchronous re-render (e.g. a `FocusGained`
                // reload) or a newer navigation has since superseded; the
                // bookmark-id check drops a result for a step already paged past.
                if *request_id == self.right_pane.step_preview_request()
                    && self.right_pane.current_step_bookmark_id() == Some(bookmark_id.as_str())
                {
                    self.right_pane.apply_step_markdown((**markdown).clone());
                }
                Some(true)
            }
            Event::CollectionStepsResolved { generation, updates } => {
                // Drop a batch for a collection the user has since navigated away
                // from (or reloaded): only the current generation's steps are on
                // screen.
                if *generation == self.right_pane.collection_generation() {
                    self.right_pane.apply_step_live_updates(updates.clone());
                }
                Some(true)
            }
            _ => None,
        }
    }

    /// Build the bookmark-search list rows for a set of bookmarks, formatted to
    /// match the normal bookmark list (symbol summary as emphasis, shortened
    /// path, node icon). Health starts as `Unknown`; callers resolve it via a
    /// background live-health task. Shared by the search-apply and refocus
    /// reconcile paths.
    pub(super) fn build_bookmark_search_items(
        bookmarks: &[codemark_core::engine::bookmark::Bookmark],
    ) -> Vec<PanelItem> {
        bookmarks
            .iter()
            .map(|bm| {
                let summary_info = bm
                    .language
                    .parse::<Language>()
                    .ok()
                    .and_then(|lang| summarizer::summarize_query(&bm.query, Some(lang)).ok());

                // Show only the identifier (not the node type) so search results
                // match the formatting of the normal bookmark list.
                let summary = summary_info
                    .as_ref()
                    .and_then(|s| s.identifier.clone())
                    .unwrap_or_else(|| bm.query.clone());

                let icon = summary_info.as_ref().map(|s| get_node_icon(&s.label)).unwrap_or("");

                let short_path = shorten_path(&bm.file_path, 25);

                let mut item = PanelItem::new(short_path)
                    .metadata(bm.created_by.clone().unwrap_or_default())
                    .health(HealthStatus::Unknown)
                    .icon(icon)
                    .created_at(bm.created_at.clone())
                    .user_data(bm.id.clone());

                // Render the symbol summary as bold emphasis, matching the
                // non-search bookmark list so styling is consistent across both.
                if !summary.is_empty() {
                    item = item.emphasis(summary);
                }

                item
            })
            .collect()
    }

    /// Populate the bookmarks panel from search results.
    ///
    /// Sets health to `Unknown` initially for instant rendering, then spawns
    /// a background task to resolve all bookmarks and send `LiveHealthBatch`
    /// events that progressively update the dots.
    fn apply_search_results(
        &mut self,
        request_id: u64,
        bookmarks: &[codemark_core::engine::bookmark::Bookmark],
    ) {
        // The search finished, so stop the loading spinner.
        self.left_pane.search.set_loading(false);

        let items = Self::build_bookmark_search_items(bookmarks);

        // A reconcile re-run refreshes an already-visible narrowed list in place,
        // keeping the user's focus and selection instead of jumping onto the
        // results list and switching tabs like a fresh search does.
        if let Some(idx) = self.take_reconcile_target(request_id) {
            self.apply_reconciled_search_items(idx, items);
            self.update_content_live_preview();
            self.spawn_live_health_task(bookmarks.to_vec());
            return;
        }

        if let Some(TabContent::List(p)) = self.left_pane.content_panel.panels.get_mut(0) {
            p.set_search_items(items);
            p.set_selected(0);
            self.left_pane.content_panel.tabs.set_selected(0);
        }

        // Move focus off the search bar onto the results list and refresh the
        // preview so it reflects the newly selected result instead of lingering
        // on whatever was previewed before the search ran.
        self.set_focus(FocusArea::ContentPanel);
        self.update_content_live_preview();

        // Spawn background task to resolve health for all bookmarks
        self.spawn_live_health_task(bookmarks.to_vec());
    }

    /// Build the collection-search list rows (name, branch, step count, health)
    /// consistent with the normal collection list. Shared by the search-apply
    /// and refocus reconcile paths.
    pub(super) fn build_collection_search_items(
        collections: &[(codemark_core::engine::bookmark::Collection, usize)],
        live: &std::collections::HashMap<String, HealthStatus>,
    ) -> Vec<PanelItem> {
        collections
            .iter()
            .map(|(c, count)| {
                let health = super::collection_health_status(c.health, live.get(&c.id).copied());
                let branch = c.created_branch.clone().unwrap_or_else(|| "main".to_string());

                PanelItem::new(&c.name)
                    .secondary_text(&branch)
                    .metadata(format!("{count} steps"))
                    .health(health)
                    .published(c.published_at.is_some())
                    .created_at(c.created_at.clone())
                    .user_data(c.id.clone())
            })
            .collect()
    }

    /// Populate the collections panel from collection search results.
    ///
    /// Mirrors [`Self::apply_search_results`], but targets the Collections tab
    /// and renders collection rows (name, branch, step count, health) consistent
    /// with the normal collection list.
    fn apply_collection_search_results(
        &mut self,
        request_id: u64,
        collections: &[(codemark_core::engine::bookmark::Collection, usize)],
    ) {
        // The search finished, so stop the loading spinner.
        self.left_pane.search.set_loading(false);

        let items = Self::build_collection_search_items(collections, &self.collection_live_health);

        // A reconcile re-run refreshes the Collections panel in place, keeping
        // focus and selection. It applies even when Collections isn't the active
        // tab (the user may have switched away before the terminal refocused),
        // so it must run before the tab-active guard below.
        if let Some(idx) = self.take_reconcile_target(request_id) {
            self.apply_reconciled_search_items(idx, items);
            self.update_content_live_preview();
            return;
        }

        // A semantic collection search runs async; by the time it returns the
        // user may have switched away from the Collections tab. Applying now
        // would yank them back, so discard results unless Collections is still
        // the active Content tab. (request_id only guards superseded searches.)
        if ContentTab::from_index(self.left_pane.content_panel.tabs.selected_index())
            != Some(ContentTab::Collections)
        {
            return;
        }

        let idx = ContentTab::Collections.index();
        if let Some(TabContent::List(p)) = self.left_pane.content_panel.panels.get_mut(idx) {
            p.set_search_items(items);
            p.set_selected(0);
            self.left_pane.content_panel.tabs.set_selected(idx);
        }

        // Move focus off the search bar onto the results list, mirroring the
        // bookmark search path, so arrow keys navigate the collections directly.
        self.set_focus(FocusArea::ContentPanel);
        // Refresh the right-pane overview for the newly selected collection.
        self.update_content_live_preview();
    }

    /// Spawn a background task that resolves all bookmarks and sends
    /// `LiveHealthBatch` events to update the list health dots progressively.
    pub(super) fn spawn_live_health_task(
        &self,
        bookmarks: Vec<codemark_core::engine::bookmark::Bookmark>,
    ) {
        if bookmarks.is_empty() {
            return;
        }

        let db_path = self.db.path().to_path_buf();
        let event_handler = self.event_handler.clone();
        // Stamp the current health epoch; results are discarded on apply if the
        // epoch has since advanced (e.g. a grammar refresh on FocusGained).
        let generation = self.health_generation;

        tokio::task::spawn_blocking(move || {
            let handle = tokio::runtime::Handle::current();
            let provider = codemark_core::vfs::LocalFileProvider;

            // Group bookmarks by (language, file_path) for parse-tree reuse
            use codemark_core::parser::languages::{Language as CL, ParseCache};
            use std::collections::HashMap;

            let mut caches: HashMap<CL, ParseCache> = HashMap::new();
            let mut batch: Vec<(String, LiveUIStatus)> = Vec::new();

            // Open a fresh DB connection for the path (only for db_path)
            let db_path_ref = &db_path;

            for bm in &bookmarks {
                let status =
                    (|| -> std::result::Result<LiveUIStatus, codemark_core::error::Error> {
                        use std::str::FromStr;
                        let language = CL::from_str(&bm.language)?;
                        let cache = match caches.entry(language.clone()) {
                            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
                            std::collections::hash_map::Entry::Vacant(e) => {
                                let pc = ParseCache::new(language.clone()).map_err(|err| {
                                    codemark_core::error::Error::TreeSitter(format!(
                                        "failed to create ParseCache for {}: {}",
                                        bm.language, err
                                    ))
                                })?;
                                e.insert(pc)
                            }
                        };

                        let result = handle.block_on(
                            codemark_core::engine::resolution::resolve_transient(
                                bm,
                                cache,
                                language,
                                db_path_ref,
                                &provider,
                            ),
                        )?;
                        Ok(result.live_status())
                    })();

                let live_status = status.unwrap_or(LiveUIStatus::Broken);
                batch.push((bm.id.clone(), live_status));

                // Send in batches of 10 for progressive rendering
                if batch.len() >= 10 {
                    let _ = event_handler.send(Event::LiveHealthBatch {
                        generation,
                        batch: std::mem::take(&mut batch),
                    });
                }
            }

            // Send remaining items
            if !batch.is_empty() {
                let _ = event_handler.send(Event::LiveHealthBatch { generation, batch });
            }
        });
    }

    /// Spawn a background task that live-resolves every collection's bookmarks
    /// and sends [`Event::CollectionHealthBatch`] events carrying each
    /// collection's *worst* live status. This makes collection dots and the
    /// overview label reflect the current on-disk state (the live-preview model)
    /// rather than the persisted snapshot, which goes stale the moment a
    /// bookmark's code drifts.
    ///
    /// A collection is only as healthy as its worst bookmark, so the worst status
    /// across the collection's (non-archived) bookmarks wins. Mirrors
    /// [`spawn_live_health_task`](Self::spawn_live_health_task): reopens the DB by
    /// path, reuses parse caches across collections, and posts in small batches.
    pub(super) fn spawn_collection_health_task(&self) {
        use codemark_core::storage::db::Database;

        // Probe on the UI thread so we skip the `spawn_blocking` (which needs a
        // Tokio runtime) entirely when there are no collections — mirroring
        // [`spawn_live_health_task`](Self::spawn_live_health_task)'s empty-set
        // guard, which keeps the unit tests (no runtime) from panicking in
        // `BrowserLayout::new`.
        let collections = match self.db.list_collections() {
            Ok(c) if !c.is_empty() => c,
            _ => return,
        };

        let db_path = self.db.path().to_path_buf();
        let event_handler = self.event_handler.clone();
        let generation = self.health_generation;

        tokio::task::spawn_blocking(move || {
            let handle = tokio::runtime::Handle::current();
            let provider = codemark_core::vfs::LocalFileProvider;

            let Ok(db) = Database::open(&db_path) else { return };

            use codemark_core::parser::languages::{Language as CL, ParseCache};
            use std::collections::HashMap;
            let mut caches: HashMap<CL, ParseCache> = HashMap::new();
            let mut batch: Vec<(String, Option<LiveUIStatus>)> = Vec::new();
            let db_path_ref = &db_path;

            for (c, _count) in &collections {
                // `list_bookmarks_in_collection` already excludes archived
                // bookmarks (it routes through `list_bookmarks`, which drops
                // `r.health = 'archived'` by default), matching the persisted
                // aggregation rule.
                let Ok(bookmarks) = db.list_bookmarks_in_collection(&c.id) else {
                    continue;
                };

                let mut worst: Option<LiveUIStatus> = None;
                for bm in &bookmarks {
                    let status =
                        (|| -> std::result::Result<LiveUIStatus, codemark_core::error::Error> {
                            use std::str::FromStr;
                            let language = CL::from_str(&bm.language)?;
                            let cache = match caches.entry(language.clone()) {
                                std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
                                std::collections::hash_map::Entry::Vacant(e) => {
                                    let pc = ParseCache::new(language.clone()).map_err(|err| {
                                        codemark_core::error::Error::TreeSitter(format!(
                                            "failed to create ParseCache for {}: {}",
                                            bm.language, err
                                        ))
                                    })?;
                                    e.insert(pc)
                                }
                            };
                            let result = handle.block_on(
                                codemark_core::engine::resolution::resolve_transient(
                                    bm,
                                    cache,
                                    language,
                                    db_path_ref,
                                    &provider,
                                ),
                            )?;
                            Ok(result.live_status())
                        })()
                        .unwrap_or(LiveUIStatus::Broken);

                    worst = Some(match worst {
                        None => status,
                        Some(cur) => cur.worst(status),
                    });
                }

                // Emit an entry for *every* collection, including empty ones
                // (worst = None): the handler clears the cache for those so a
                // previously cached status doesn't linger after the collection's
                // bookmarks are removed or archived.
                batch.push((c.id.clone(), worst));
                if batch.len() >= 5 {
                    let _ = event_handler.send(Event::CollectionHealthBatch {
                        generation,
                        batch: std::mem::take(&mut batch),
                    });
                }
            }

            if !batch.is_empty() {
                let _ = event_handler.send(Event::CollectionHealthBatch { generation, batch });
            }
        });
    }

    /// Spawn a background task that live-resolves the currently open collection's
    /// steps and streams the results back as [`Event::CollectionStepsResolved`].
    ///
    /// Entering a collection renders every step immediately from cheap persisted
    /// data (see [`RightPane::load_tour_live`]); only the current step is resolved
    /// live on the UI thread. This task performs the expensive per-step work —
    /// tree-sitter resolution + git health projection — off-thread and posts
    /// batches that patch each step's location + health by index, mirroring
    /// [`spawn_live_health_task`](Self::spawn_live_health_task). The captured
    /// `generation` lets the UI drop the batch if the user has since opened a
    /// different collection.
    pub(super) fn spawn_collection_live_resolve(&self) {
        use codemark_core::storage::db::Database;

        let steps = self.right_pane.steps_resolve_input();
        if steps.is_empty() {
            return;
        }
        let generation = self.right_pane.collection_generation();
        let db_path = self.db.path().to_path_buf();
        let event_handler = self.event_handler.clone();

        tokio::task::spawn_blocking(move || {
            let Ok(db) = Database::open(&db_path) else {
                return;
            };

            use codemark_core::parser::languages::{Language as CL, ParseCache};
            use std::collections::HashMap;

            let mut caches: HashMap<CL, ParseCache> = HashMap::new();
            let mut batch: Vec<super::StepLiveUpdate> = Vec::new();

            for (index, bm) in &steps {
                // This runs on a spawn_blocking thread, not a runtime worker.
                let update =
                    match super::RightPane::resolve_bookmark_live(bm, &db, &mut caches, false) {
                        Ok((file_path, start_line, end_line, _source, live_status)) => {
                            super::StepLiveUpdate {
                                index: *index,
                                file_path,
                                line_number: start_line,
                                line_end: Some(end_line),
                                resolved: true,
                                health: crate::component::HealthStatus::from(live_status),
                            }
                        }
                        Err(e) => {
                            // Live resolution failed; fall back to the persisted
                            // location (the same one the cheap build used).
                            let (file_path, line_number, line_end, _res) =
                                super::RightPane::persisted_step_location(&db, bm);
                            let health = if matches!(e, codemark_core::error::Error::Resolution(_))
                            {
                                crate::component::HealthStatus::Broken
                            } else {
                                crate::component::HealthStatus::Unknown
                            };
                            super::StepLiveUpdate {
                                index: *index,
                                file_path,
                                line_number,
                                line_end,
                                resolved: false,
                                health,
                            }
                        }
                    };
                batch.push(update);

                // Post in small batches so the pager dots + current step correct
                // progressively rather than all at once at the end.
                if batch.len() >= 5 {
                    let _ = event_handler.send(Event::CollectionStepsResolved {
                        generation,
                        updates: std::mem::take(&mut batch),
                    });
                }
            }

            if !batch.is_empty() {
                let _ = event_handler
                    .send(Event::CollectionStepsResolved { generation, updates: batch });
            }
        });
    }

    /// Spawn a background task that resolves a single bookmark preview off the UI
    /// thread (live resolution + file read + markdown render) and sends the
    /// result back as [`Event::PreviewReady`] / [`Event::PreviewFailed`].
    ///
    /// `request_id` is captured from `active_preview_request` so the UI can drop
    /// the result if the selection has since moved on. Mirrors
    /// [`spawn_live_health_task`](Self::spawn_live_health_task) in reopening the DB
    /// by path (since `Database` isn't `Send`), but shares a long-lived
    /// `preview_cache` across tasks so repeat visits to the same file reuse the
    /// parse tree. Debounce serializes these tasks, so the lock is effectively
    /// uncontended.
    fn spawn_preview_task(&self, bookmark_id: String) {
        use codemark_core::storage::db::Database;

        let request_id = self.active_preview_request;
        let db_path = self.db.path().to_path_buf();
        let head = self.right_pane.head_commit().map(|s| s.to_string());
        let event_handler = self.event_handler.clone();
        let preview_cache = Arc::clone(&self.preview_cache);

        tokio::task::spawn_blocking(move || {
            let Ok(db) = Database::open(&db_path) else {
                let _ = event_handler.send(Event::PreviewFailed {
                    request_id,
                    bookmark_id,
                    unresolved: false,
                });
                return;
            };

            let show_template =
                codemark_core::templates::load_template(codemark_core::templates::SHOW_TEMPLATE);
            let details_template =
                codemark_core::templates::load_template(codemark_core::templates::DETAILS_TEMPLATE);
            let comments_template = codemark_core::templates::load_template(
                codemark_core::templates::COMMENTS_TEMPLATE,
            );

            // Recover from a poisoned lock: a panicked prior task can't corrupt a
            // parse cache in a way that matters (entries are independent), so reuse it.
            let mut cache = preview_cache.lock().unwrap_or_else(|e| e.into_inner());

            // `cache` is a MutexGuard; `&mut cache` derefs to `&mut HashMap`.
            match super::RightPane::build_bookmark_preview(
                &db,
                &bookmark_id,
                &mut cache,
                super::right_pane::PreviewTemplates {
                    show: &show_template,
                    details: &details_template,
                    comments: &comments_template,
                },
                head.as_deref(),
                // This closure runs on a spawn_blocking thread, not a runtime worker.
                false,
            ) {
                Ok(Some(payload)) => {
                    let _ = event_handler.send(Event::PreviewReady { request_id, payload });
                }
                Ok(None) => {
                    let _ = event_handler.send(Event::PreviewFailed {
                        request_id,
                        bookmark_id,
                        unresolved: false,
                    });
                }
                Err(e) => {
                    let unresolved = matches!(e, codemark_core::error::Error::Resolution(_));
                    let _ = event_handler.send(Event::PreviewFailed {
                        request_id,
                        bookmark_id,
                        unresolved,
                    });
                }
            }
        });
    }

    /// Update focus based on mouse click position.
    fn handle_mouse_focus(&mut self, event: &Event) {
        if let Event::Mouse(mouse) = event
            && let ratatui::crossterm::event::MouseEventKind::Down(
                ratatui::crossterm::event::MouseButton::Left,
            ) = mouse.kind
        {
            let col = mouse.column;
            let row = mouse.row;

            let search_area = self.left_pane.search.last_area();
            if col >= search_area.x
                && col < search_area.x + search_area.width
                && row >= search_area.y
                && row < search_area.y + search_area.height
            {
                self.set_focus(FocusArea::Search);
            } else {
                let p1_area = self.left_pane.context_panel.last_area();
                if col >= p1_area.x
                    && col < p1_area.x + p1_area.width
                    && row >= p1_area.y
                    && row < p1_area.y + p1_area.height
                {
                    self.set_focus(FocusArea::ContextPanel);
                } else {
                    let p2_area = self.left_pane.filters_panel.last_area();
                    if col >= p2_area.x
                        && col < p2_area.x + p2_area.width
                        && row >= p2_area.y
                        && row < p2_area.y + p2_area.height
                    {
                        self.set_focus(FocusArea::FiltersPanel);
                    } else {
                        let p3_area = self.left_pane.content_panel.last_area();
                        if col >= p3_area.x
                            && col < p3_area.x + p3_area.width
                            && row >= p3_area.y
                            && row < p3_area.y + p3_area.height
                        {
                            self.set_focus(FocusArea::ContentPanel);
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
    }

    // ── Key events ───────────────────────────────────────────────────────

    /// Handle keyboard events (focus cycling, shortcuts, actions).
    ///
    /// Returns `Some(true/false)` if handled, `None` to fall through to delegation.
    fn handle_key_event(&mut self, key: &ratatui::crossterm::event::KeyEvent) -> Option<bool> {
        match key.code {
            ratatui::crossterm::event::KeyCode::Enter => {
                return self.handle_enter_key();
            }
            ratatui::crossterm::event::KeyCode::Char(' ') => {
                return self.handle_space_key();
            }
            ratatui::crossterm::event::KeyCode::Tab => {
                if !self.should_handle_keybindings() {
                    return Some(false);
                }
                self.next_focus();
                return Some(true);
            }
            ratatui::crossterm::event::KeyCode::BackTab => {
                if !self.should_handle_keybindings() {
                    return Some(false);
                }
                self.previous_focus();
                return Some(true);
            }
            ratatui::crossterm::event::KeyCode::Esc => {
                if self.focus == FocusArea::Search {
                    self.left_pane.search.clear();
                    // Invalidate any in-flight search so it doesn't overwrite the refreshed panels
                    self.active_search_request = self.active_search_request.wrapping_add(1);
                    self.refresh_all_panels();
                    self.set_focus(FocusArea::ContentPanel);
                    return Some(true);
                }
            }
            ratatui::crossterm::event::KeyCode::Char(c @ '1'..='5')
                if self.should_handle_keybindings() && self.focus != FocusArea::Search =>
            {
                return Some(self.handle_number_key(c));
            }
            // `s` jumps focus to the search bar from any other pane. The guard
            // keeps it from firing when search is already focused, so the same
            // key types "s" into the query there.
            ratatui::crossterm::event::KeyCode::Char('s')
                if self.should_handle_keybindings() && self.focus != FocusArea::Search =>
            {
                self.focus = FocusArea::Search;
                self.update_focus_state();
                return Some(true);
            }
            // `S` cycles the sort order of the focused Content list. Only the
            // Bookmarks and Collections tabs support sorting; on any other tab
            // (or pane) it falls through so the key isn't silently swallowed.
            ratatui::crossterm::event::KeyCode::Char('S')
                if self.should_handle_keybindings() && self.focus == FocusArea::ContentPanel =>
            {
                if self.cycle_content_sort() {
                    return Some(true);
                }
            }
            ratatui::crossterm::event::KeyCode::Char('o') => {
                return self.handle_o_key(key);
            }
            ratatui::crossterm::event::KeyCode::Char('d')
                if self.should_handle_keybindings() && self.focus == FocusArea::ContentPanel =>
            {
                return self.handle_delete_key();
            }
            ratatui::crossterm::event::KeyCode::Char('P')
                if self.should_handle_keybindings()
                    && self.focus == FocusArea::ContentPanel
                    // Pushing needs sync credentials; the binding is hidden when
                    // logged out, so treat the key as a no-op to match.
                    && self.logged_in =>
            {
                if let Some(ContentTab::Collections) =
                    ContentTab::from_index(self.left_pane.content_panel.tabs.selected_index())
                {
                    self.start_push_collection();
                    return Some(true);
                }
            }
            ratatui::crossterm::event::KeyCode::Char('p')
                if self.should_handle_keybindings() && self.focus == FocusArea::ContentPanel =>
            {
                return self.handle_pull_key();
            }
            ratatui::crossterm::event::KeyCode::Char('H')
                if self.should_handle_keybindings()
                    && (self.focus == FocusArea::ContentPanel || self.focus == FocusArea::Main) =>
            {
                self.start_heal_selection();
                return Some(true);
            }
            ratatui::crossterm::event::KeyCode::Char('J')
                if self.should_handle_keybindings() && self.focus != FocusArea::Search =>
            {
                // Scroll the visible preview regardless of which pane has focus;
                // capital J/K are reserved for this so they never collide with
                // the lowercase j/k list navigation.
                return Some(self.right_pane.scroll_main_content(super::PREVIEW_SCROLL_LINES));
            }
            ratatui::crossterm::event::KeyCode::Char('K')
                if self.should_handle_keybindings() && self.focus != FocusArea::Search =>
            {
                return Some(self.right_pane.scroll_main_content(-super::PREVIEW_SCROLL_LINES));
            }
            ratatui::crossterm::event::KeyCode::Char('+')
            | ratatui::crossterm::event::KeyCode::Char('=')
                if self.should_handle_keybindings() && self.focus.is_resizable() =>
            {
                return Some(self.handle_resize(true));
            }
            ratatui::crossterm::event::KeyCode::Char('_')
            | ratatui::crossterm::event::KeyCode::Char('-')
                if self.should_handle_keybindings() && self.focus.is_resizable() =>
            {
                return Some(self.handle_resize(false));
            }
            _ => {}
        }
        None
    }

    /// Handle Enter key — activate/open the selected item in the focused panel.
    fn handle_enter_key(&mut self) -> Option<bool> {
        if self.focus == FocusArea::Search {
            // Show the loading spinner while the search runs (semantic search in
            // particular is slow). It is cleared when results or an error arrive.
            if !self.left_pane.search.query().is_empty() {
                self.left_pane.search.set_loading(true);
            }
            self.execute_search();
            return Some(true);
        }
        if self.focus == FocusArea::ContextPanel {
            return Some(self.activate_context_selection(true));
        }
        if self.focus == FocusArea::FiltersPanel
            && let Some(panel) = self.left_pane.filters_panel.active_panel_mut()
        {
            panel.activate_selected();
            self.update_tours_collections();
            return Some(true);
        }
        if self.focus == FocusArea::ContentPanel {
            return self.activate_content_selection();
        }
        None
    }

    /// Handle Space key — activate/toggle without moving focus away.
    fn handle_space_key(&mut self) -> Option<bool> {
        if self.focus == FocusArea::ContextPanel {
            return Some(self.activate_context_selection(false));
        }
        if self.focus == FocusArea::FiltersPanel
            && let Some(panel) = self.left_pane.filters_panel.active_panel_mut()
        {
            panel.activate_selected();
            self.update_tours_collections();
            return Some(true);
        }
        if self.focus == FocusArea::ContentPanel {
            return self.activate_content_selection();
        }
        None
    }

    /// Activate the currently selected item in Context panel (Repos, Owners, or Auth).
    ///
    /// When `move_focus` is true (Enter), a successful repo switch moves focus to Content panel.
    /// When false (Space), focus stays on Context panel.
    fn activate_context_selection(&mut self, move_focus: bool) -> bool {
        let active_tab = ContextTab::from_index(self.left_pane.context_panel.tabs.selected_index());
        match active_tab {
            Some(ContextTab::Owners) => {
                // Owners tab: toggle owner selection and filter repos
                if let Some(panel) = self.left_pane.context_panel.active_panel_mut() {
                    panel.activate_selected();
                }
                self.update_repos_by_owner();
                return true;
            }
            Some(ContextTab::Auth) => {
                // Auth tab is read-only; nothing to activate.
                return false;
            }
            _ => {}
        }
        if let Some(panel) = self.left_pane.context_panel.active_panel_mut()
            && let Some(selected) = panel.selected()
            && let Some(root) = selected.user_data.as_ref()
        {
            // Repos tab: switch database
            let root = root.clone();
            panel.activate_selected();
            if move_focus {
                if self.switch_database(&root).is_ok() {
                    self.set_focus(FocusArea::ContentPanel);
                }
            } else {
                let _ = self.switch_database(&root);
            }
            return true;
        }
        false
    }

    /// Activate the currently selected item in Content panel (Bookmarks, Tours, Collections).
    fn activate_content_selection(&mut self) -> Option<bool> {
        match ContentTab::from_index(self.left_pane.content_panel.tabs.selected_index()) {
            Some(ContentTab::Tours) => {
                if let Some(panel) = self.left_pane.content_panel.active_panel_mut()
                    && let Some(selected) = panel.selected()
                {
                    if let Some(ref ud) = selected.user_data
                        && let Some(remote_id) = ud.strip_prefix("remote:")
                    {
                        let remote_id = remote_id.to_string();
                        self.start_pull_tour(remote_id);
                        return Some(true);
                    }
                    let tour_name = selected.text().to_string();
                    panel.activate_selected();
                    self.right_pane.load_tour_live(&self.db, &tour_name, &mut self.session_cache);
                    // Only the first step was resolved live; resolve the rest off
                    // the UI thread so entering the tour stays snappy.
                    self.spawn_collection_live_resolve();
                    self.set_focus(FocusArea::Main);
                    return Some(true);
                }
            }
            Some(ContentTab::Collections) => {
                if let Some(panel) = self.left_pane.content_panel.active_panel_mut()
                    && let Some(selected) = panel.selected()
                {
                    let tour_name = selected.text().to_string();
                    panel.activate_selected();
                    self.right_pane.load_tour_live(&self.db, &tour_name, &mut self.session_cache);
                    // Only the first step was resolved live; resolve the rest off
                    // the UI thread so entering the collection stays snappy.
                    self.spawn_collection_live_resolve();
                    self.set_focus(FocusArea::Main);
                    return Some(true);
                }
            }
            Some(ContentTab::Bookmarks) => {
                if let Some(panel) = self.left_pane.content_panel.active_panel_mut()
                    && let Some(selected) = panel.selected()
                    && let Some(id) = selected.user_data.clone()
                {
                    panel.activate_selected();
                    self.right_pane.load_bookmark_live(&self.db, &id, &mut self.session_cache);
                    self.set_focus(FocusArea::Main);
                    return Some(true);
                }
            }
            None => {}
        }
        None
    }

    /// Cycle the sort order of the active Content list (Bookmarks or Collections).
    ///
    /// Returns `false` on tabs that don't support sorting (e.g. Tours) so the
    /// caller can let the key fall through. On success the right-pane preview is
    /// refreshed since the selection may now point at a different item.
    fn cycle_content_sort(&mut self) -> bool {
        let tab = ContentTab::from_index(self.left_pane.content_panel.tabs.selected_index());
        if !matches!(tab, Some(ContentTab::Bookmarks) | Some(ContentTab::Collections)) {
            return false;
        }
        let Some(panel) = self.left_pane.content_panel.active_panel_mut() else {
            return false;
        };
        let method = panel.cycle_sort();
        tracing::debug!(target: "codemark::ui", ?tab, ?method, "content sort cycled");
        // The selection kept its item but its row (and neighbors) moved; refresh
        // the preview so it matches whatever is now selected. Cancel any pending/
        // in-flight resolve first so a stale async result can't land on top of the
        // synchronous render below.
        self.cancel_inflight_preview();
        self.update_content_live_preview();
        true
    }

    /// Handle number keys 1-6 for direct focus switching.
    fn handle_number_key(&mut self, c: char) -> bool {
        // Numbers jump to the visible panes 1..=5, matching the `[N]` badge on
        // each pane's border. The search bar is excluded — it has its own `s`
        // shortcut — so the numbering starts at Context panel.
        match c {
            '1' => {
                self.focus = FocusArea::ContextPanel;
                self.update_focus_state();
            }
            '2' => {
                self.focus = FocusArea::FiltersPanel;
                self.update_focus_state();
            }
            '3' => {
                self.focus = FocusArea::ContentPanel;
                self.update_focus_state();
            }
            '4' => {
                self.focus = FocusArea::Main;
                self.right_pane.focus_steps();
                self.update_focus_state();
            }
            '5' => {
                self.focus = FocusArea::Main;
                self.right_pane.focus_details();
                self.update_focus_state();
            }
            _ => return false,
        }
        true
    }

    /// Handle 'o' key — open in editor (plain) or copy ID/markdown (Ctrl+O).
    fn handle_o_key(&mut self, key: &ratatui::crossterm::event::KeyEvent) -> Option<bool> {
        let ctrl = key.modifiers.contains(ratatui::crossterm::event::KeyModifiers::CONTROL);

        if !ctrl {
            if self.should_handle_keybindings() && self.focus != FocusArea::Search {
                self.open_in_editor();
                return Some(true);
            }
            return None;
        }

        // Ctrl+O: copy ID or markdown depending on focus
        if !self.should_handle_keybindings() {
            return None;
        }

        if self.focus == FocusArea::ContentPanel {
            match ContentTab::from_index(self.left_pane.content_panel.tabs.selected_index()) {
                Some(ContentTab::Bookmarks) | Some(ContentTab::Collections) => {
                    if let Some(panel) = self.left_pane.content_panel.active_panel_mut()
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
            return Some(true);
        }

        if self.focus == FocusArea::Main {
            if let Some(markdown) = self.right_pane.active_markdown_content().map(|m| m.to_owned())
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
            return Some(true);
        }

        None
    }

    /// Handle 'd' key — confirm deletion of the selected collection or bookmark.
    ///
    /// This opens a modal confirmation dialog; the actual deletion happens only
    /// once the user confirms (see [`BrowserLayout::handle_dialog_key`]).
    fn handle_delete_key(&mut self) -> Option<bool> {
        match ContentTab::from_index(self.left_pane.content_panel.tabs.selected_index()) {
            Some(ContentTab::Collections) => {
                if let Some(panel) = self.left_pane.content_panel.active_panel_mut()
                    && let Some(selected) = panel.selected()
                {
                    let collection_name = selected.text().to_string();
                    if let Ok(Some(collection)) = self.db.get_collection_by_name(&collection_name) {
                        // Deleting a collection also deletes its bookmarks — they're
                        // almost always created alongside the collection. Bookmarks
                        // shared with another collection are preserved, so only count
                        // the ones that will actually be removed.
                        //
                        // Surface a count failure rather than defaulting to 0, which
                        // would open a destructive confirmation that understates what's
                        // about to be deleted.
                        let bm_count =
                            match self.db.count_exclusive_collection_bookmarks(&collection.id) {
                                Ok(count) => count,
                                Err(err) => {
                                    tracing::warn!(
                                        target: "codemark::ui",
                                        error = %err,
                                        "failed to count collection bookmarks before delete"
                                    );
                                    self.pending_notification = Some(HealNotification {
                                        message: format!("Couldn't count bookmarks: {err}"),
                                        success: false,
                                    });
                                    return Some(true);
                                }
                            };
                        let bookmarks_line = match bm_count {
                            0 => String::new(),
                            1 => "\n1 bookmark will be deleted.".to_string(),
                            n => format!("\n{n} bookmarks will be deleted."),
                        };
                        // Delete by the stable id; names are not uniquely constrained.
                        self.request_confirmation(ConfirmDialog::new(
                            "Delete Collection",
                            format!("Delete collection \"{collection_name}\"?{bookmarks_line}"),
                            DialogAction::DeleteCollection(collection.id),
                        ));
                        return Some(true);
                    }
                }
            }
            Some(ContentTab::Bookmarks) => {
                if let Some(panel) = self.left_pane.content_panel.active_panel_mut()
                    && let Some(selected) = panel.selected()
                    && let Some(id) = selected.user_data.clone()
                {
                    let label = selected.text().to_string();
                    self.request_confirmation(ConfirmDialog::new(
                        "Delete Bookmark",
                        format!("Delete bookmark \"{}\"?", label),
                        DialogAction::DeleteBookmark(id),
                    ));
                    return Some(true);
                }
            }
            _ => {}
        }
        None
    }

    /// Handle 'p' key — pull a remote tour or refresh the remote listing.
    fn handle_pull_key(&mut self) -> Option<bool> {
        if let Some(ContentTab::Tours) =
            ContentTab::from_index(self.left_pane.content_panel.tabs.selected_index())
        {
            let should_pull = self
                .left_pane
                .content_panel
                .active_panel_mut()
                .and_then(|panel| panel.selected())
                .and_then(|selected| {
                    selected
                        .user_data
                        .as_ref()
                        .and_then(|ud| ud.strip_prefix("remote:").map(|id| id.to_string()))
                });

            if let Some(remote_id) = should_pull {
                self.start_pull_tour(remote_id);
            } else {
                self.fetch_remote_tours();
            }
            return Some(true);
        }
        None
    }

    /// Handle +/= or -/_ keys for resizing panes.
    fn handle_resize(&mut self, grow: bool) -> bool {
        if self.focus == FocusArea::Main {
            if self.right_pane.focused == RightPaneFocus::Details {
                // Cycle details pane size (Regular → Half → Full)
                let old_size = self.details_pane_size;
                self.details_pane_size = if grow {
                    self.details_pane_size.increase()
                } else {
                    self.details_pane_size.decrease()
                };
                // Reset steps fullscreen to avoid conflicting states
                self.right_pane_size = RightPaneSize::Regular;
                tracing::debug!(
                    target: "codemark::ui",
                    ?old_size,
                    new_size = ?self.details_pane_size,
                    grow,
                    "details pane resized"
                );
            } else {
                // Cycle the preview pane layout
                // (Regular → Full → PreviewOnly → Regular).
                let next = if grow {
                    self.right_pane_size.increase()
                } else {
                    self.right_pane_size.decrease()
                };
                self.right_pane_size = next;
                // Reset details fullscreen to avoid conflicting states
                self.details_pane_size = DetailsPaneSize::Regular;
                if next.is_fullscreen() {
                    self.right_pane.focus_steps();
                }
                tracing::debug!(
                    target: "codemark::ui",
                    ?next,
                    "preview pane layout cycled"
                );
            }
        } else if grow {
            self.left_pane_size = self.left_pane_size.increase();
            self.left_pane.set_resize_mode(self.left_pane_size);
            self.left_pane.set_focused_area(self.focus);
        } else {
            self.left_pane_size = self.left_pane_size.decrease();
            self.left_pane.set_resize_mode(self.left_pane_size);
            self.left_pane.set_focused_area(self.focus);
        }
        true
    }

    // ── Delegation to child components ───────────────────────────────────

    /// Delegate events to the appropriate child pane/component.
    fn handle_delegation(&mut self, event: &Event) -> bool {
        match event {
            Event::Mouse(_) => {
                let old_tab = self.left_pane.content_panel.tabs.selected_index();

                let left_handled = self.left_pane.handle_event(event);
                let right_handled = self.right_pane.handle_event(event);
                self.debounce_step_preview();
                let handled = left_handled || right_handled;

                let new_tab = self.left_pane.content_panel.tabs.selected_index();
                if new_tab != old_tab {
                    self.refresh_tags();
                    self.sync_steps_tab_label();
                    if ContentTab::from_index(new_tab) == Some(ContentTab::Tours) {
                        self.fetch_remote_tours();
                    }
                    // Refresh the preview for the newly active tab so it doesn't
                    // linger on content from the previous tab.
                    self.update_content_live_preview();
                }

                // A click on the sort glyph reordered the list; refresh the
                // preview so it tracks the (re-positioned) selection, matching
                // the `S` shortcut path. Cancel any pending/in-flight resolve
                // first so a stale async result can't overwrite the synchronous
                // render.
                if self.left_pane.content_panel.take_sort_changed() {
                    self.cancel_inflight_preview();
                    self.update_content_live_preview();
                }

                if self.focus == FocusArea::ContentPanel
                    && let Some(id) = self.left_pane.content_panel.take_selection_change()
                    && let Some(tab) = ContentTab::from_index(new_tab)
                {
                    self.on_content_selection_changed(tab, &id);
                }
                handled
            }
            Event::Key(_key) => {
                if !self.should_handle_keybindings() {
                    return false;
                }

                let old_tab = self.left_pane.content_panel.tabs.selected_index();
                let handled = match self.focus {
                    FocusArea::Search => self.left_pane.search.handle_event(event),
                    FocusArea::ContextPanel => self.left_pane.context_panel.handle_event(event),
                    FocusArea::FiltersPanel => self.left_pane.filters_panel.handle_event(event),
                    FocusArea::ContentPanel => {
                        let handled = self.left_pane.content_panel.handle_event(event);
                        if let Some(id) = self.left_pane.content_panel.take_selection_change()
                            && let Some(tab) = ContentTab::from_index(
                                self.left_pane.content_panel.tabs.selected_index(),
                            )
                        {
                            self.on_content_selection_changed(tab, &id);
                        }
                        handled
                    }
                    FocusArea::Main => {
                        let handled = self.right_pane.handle_event(event);
                        self.debounce_step_preview();
                        handled
                    }
                    // Unreachable: should_handle_keybindings() returns false in
                    // filter mode, so the early return on line 668 fires first.
                    FocusArea::Filter => unreachable!(),
                };

                let new_tab = self.left_pane.content_panel.tabs.selected_index();
                if new_tab != old_tab {
                    self.refresh_tags();
                    self.sync_steps_tab_label();
                    if ContentTab::from_index(new_tab) == Some(ContentTab::Tours) {
                        self.fetch_remote_tours();
                    }
                    // Refresh the preview for the newly active tab so it doesn't
                    // linger on content from the previous tab.
                    self.update_content_live_preview();
                }

                handled
            }
            _ => false,
        }
    }
}
