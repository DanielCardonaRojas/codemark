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
    BrowserLayout, ConfirmDialog, ContextTab, DetailsPaneSize, DialogAction, FocusArea,
    HealNotification, Panel3Tab, RightPaneFocus, RightPaneSize, TabContent, shorten_path,
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

        self.handle_delegation(event)
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
            if let Some(panel) = self.left_pane.panel3.get_list_panel_mut(item.tab_index) {
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

    // ── App-level events (search results, heal complete, etc.) ───────────

    /// Handle application-specific events (search results, errors, sync, remote tours).
    ///
    /// Returns `Some(true)` if the event was handled, `None` if not matched.
    fn handle_app_event(&mut self, event: &Event) -> Option<bool> {
        match event {
            Event::SearchResults(bookmarks) => {
                self.apply_search_results(bookmarks);
                Some(true)
            }
            Event::SearchError(msg) => {
                self.left_pane.search.set_error(msg.clone());
                Some(true)
            }
            Event::HealComplete(msg, success) => {
                self.pending_notification =
                    Some(HealNotification { message: msg.clone(), success: *success });
                self.schedule_clear_spinners();
                Some(true)
            }
            Event::SyncComplete(msg, success) => {
                self.pending_notification =
                    Some(HealNotification { message: msg.clone(), success: *success });
                self.schedule_clear_spinners();
                Some(true)
            }
            Event::RemoteToursLoaded(tours, scope) => {
                if *scope != self.pending_remote_repos {
                    return Some(true);
                }
                self.cached_remote_tours = tours.clone();
                self.rebuild_tours_panel();
                Some(true)
            }
            Event::RemoteToursFetchError(msg) => {
                self.pending_notification = Some(HealNotification {
                    message: format!("Failed to fetch tours: {}", msg),
                    success: false,
                });
                Some(true)
            }
            Event::LiveHealthBatch(batch) => {
                if let Some(panel) = self.left_pane.panel3.get_list_panel_mut(0) {
                    for (bookmark_id, status) in batch {
                        panel.update_item_health(bookmark_id, HealthStatus::from(*status));
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
            Event::PreviewFailed { request_id, bookmark_id } => {
                if *request_id == self.active_preview_request {
                    self.inflight_preview = None;
                    // Live resolution failed; fall back to the persisted snapshot
                    // synchronously (no tree-sitter parse, so it's cheap).
                    self.right_pane.load_bookmark(&self.db, bookmark_id);
                    self.right_pane.finish_loading();
                }
                Some(true)
            }
            _ => None,
        }
    }

    /// Populate the bookmarks panel from search results.
    ///
    /// Sets health to `Unknown` initially for instant rendering, then spawns
    /// a background task to resolve all bookmarks and send `LiveHealthBatch`
    /// events that progressively update the dots.
    fn apply_search_results(&mut self, bookmarks: &[codemark_core::engine::bookmark::Bookmark]) {
        let items: Vec<PanelItem> = bookmarks
            .iter()
            .map(|bm| {
                let summary_info = bm
                    .language
                    .parse::<Language>()
                    .ok()
                    .and_then(|lang| summarizer::summarize_query(&bm.query, Some(lang)).ok());

                let summary = summary_info
                    .as_ref()
                    .and_then(|s| s.format())
                    .unwrap_or_else(|| bm.query.clone());

                let icon = summary_info.as_ref().map(|s| get_node_icon(&s.label)).unwrap_or("");

                let short_path = shorten_path(&bm.file_path, 25);

                let mut item = PanelItem::new(short_path)
                    .metadata(bm.created_by.clone().unwrap_or_default())
                    .health(HealthStatus::Unknown)
                    .icon(icon)
                    .user_data(bm.id.clone());

                // Render the symbol summary as bold emphasis, matching the
                // non-search bookmark list so styling is consistent across both.
                if !summary.is_empty() {
                    item = item.emphasis(summary);
                }

                item
            })
            .collect();

        if let Some(TabContent::List(p)) = self.left_pane.panel3.panels.get_mut(0) {
            p.set_items(items);
            p.set_selected(0);
            self.left_pane.panel3.tabs.set_selected(0);
        }

        // Spawn background task to resolve health for all bookmarks
        self.spawn_live_health_task(bookmarks.to_vec());
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
                        let cache = match caches.entry(language) {
                            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
                            std::collections::hash_map::Entry::Vacant(e) => {
                                let pc = ParseCache::new(language).map_err(|err| {
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
                    let _ = event_handler.send(Event::LiveHealthBatch(std::mem::take(&mut batch)));
                }
            }

            // Send remaining items
            if !batch.is_empty() {
                let _ = event_handler.send(Event::LiveHealthBatch(batch));
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
                let _ = event_handler.send(Event::PreviewFailed { request_id, bookmark_id });
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
                Some(payload) => {
                    let _ = event_handler.send(Event::PreviewReady { request_id, payload });
                }
                None => {
                    let _ = event_handler.send(Event::PreviewFailed { request_id, bookmark_id });
                }
            }
        });
    }

    // ── Mouse events ─────────────────────────────────────────────────────

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
                    self.refresh_all_panels();
                    self.set_focus(FocusArea::Panel3);
                    return Some(true);
                }
                if self.focus == FocusArea::Main {
                    self.set_focus(FocusArea::Panel3);
                    return Some(true);
                }
            }
            ratatui::crossterm::event::KeyCode::Char(c @ '1'..='6')
                if self.should_handle_keybindings() && self.focus != FocusArea::Search =>
            {
                return Some(self.handle_number_key(c));
            }
            ratatui::crossterm::event::KeyCode::Char('o') => {
                return self.handle_o_key(key);
            }
            ratatui::crossterm::event::KeyCode::Char('d')
                if self.should_handle_keybindings() && self.focus == FocusArea::Panel3 =>
            {
                return self.handle_delete_key();
            }
            ratatui::crossterm::event::KeyCode::Char('P')
                if self.should_handle_keybindings() && self.focus == FocusArea::Panel3 =>
            {
                if let Some(Panel3Tab::Collections) =
                    Panel3Tab::from_index(self.left_pane.panel3.tabs.selected_index())
                {
                    self.start_push_collection();
                    return Some(true);
                }
            }
            ratatui::crossterm::event::KeyCode::Char('p')
                if self.should_handle_keybindings() && self.focus == FocusArea::Panel3 =>
            {
                return self.handle_pull_key();
            }
            ratatui::crossterm::event::KeyCode::Char('H')
                if self.should_handle_keybindings()
                    && (self.focus == FocusArea::Panel3 || self.focus == FocusArea::Main) =>
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
            self.execute_search();
            return Some(true);
        }
        if self.focus == FocusArea::Panel1 {
            return Some(self.activate_panel1_selection(true));
        }
        if self.focus == FocusArea::Panel2
            && let Some(panel) = self.left_pane.panel2.active_panel_mut()
        {
            panel.activate_selected();
            self.update_tours_collections();
            return Some(true);
        }
        if self.focus == FocusArea::Panel3 {
            return self.activate_panel3_selection();
        }
        None
    }

    /// Handle Space key — activate/toggle without moving focus away.
    fn handle_space_key(&mut self) -> Option<bool> {
        if self.focus == FocusArea::Panel1 {
            return Some(self.activate_panel1_selection(false));
        }
        if self.focus == FocusArea::Panel2
            && let Some(panel) = self.left_pane.panel2.active_panel_mut()
        {
            panel.activate_selected();
            self.update_tours_collections();
            return Some(true);
        }
        if self.focus == FocusArea::Panel3 {
            return self.activate_panel3_selection();
        }
        None
    }

    /// Activate the currently selected item in Panel 1 (Repos, Owners, or Auth).
    ///
    /// When `move_focus` is true (Enter), a successful repo switch moves focus to Panel 3.
    /// When false (Space), focus stays on Panel 1.
    fn activate_panel1_selection(&mut self, move_focus: bool) -> bool {
        let active_tab = ContextTab::from_index(self.left_pane.panel1.tabs.selected_index());
        match active_tab {
            Some(ContextTab::Owners) => {
                // Owners tab: toggle owner selection and filter repos
                if let Some(panel) = self.left_pane.panel1.active_panel_mut() {
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
        if let Some(panel) = self.left_pane.panel1.active_panel_mut()
            && let Some(selected) = panel.selected()
            && let Some(root) = selected.user_data.as_ref()
        {
            // Repos tab: switch database
            let root = root.clone();
            panel.activate_selected();
            if move_focus {
                if self.switch_database(&root).is_ok() {
                    self.set_focus(FocusArea::Panel3);
                }
            } else {
                let _ = self.switch_database(&root);
            }
            return true;
        }
        false
    }

    /// Activate the currently selected item in Panel 3 (Bookmarks, Tours, Collections).
    fn activate_panel3_selection(&mut self) -> Option<bool> {
        match Panel3Tab::from_index(self.left_pane.panel3.tabs.selected_index()) {
            Some(Panel3Tab::Tours) => {
                if let Some(panel) = self.left_pane.panel3.active_panel_mut()
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
                    self.set_focus(FocusArea::Main);
                    return Some(true);
                }
            }
            Some(Panel3Tab::Collections) => {
                if let Some(panel) = self.left_pane.panel3.active_panel_mut()
                    && let Some(selected) = panel.selected()
                {
                    let tour_name = selected.text().to_string();
                    panel.activate_selected();
                    self.right_pane.load_tour_live(&self.db, &tour_name, &mut self.session_cache);
                    self.set_focus(FocusArea::Main);
                    return Some(true);
                }
            }
            Some(Panel3Tab::Bookmarks) => {
                if let Some(panel) = self.left_pane.panel3.active_panel_mut()
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

    /// Handle number keys 1-6 for direct focus switching.
    fn handle_number_key(&mut self, c: char) -> bool {
        match c {
            '1' => {
                self.focus = FocusArea::Search;
                self.update_focus_state();
            }
            '2' => {
                self.focus = FocusArea::Panel1;
                self.update_focus_state();
            }
            '3' => {
                self.focus = FocusArea::Panel2;
                self.update_focus_state();
            }
            '4' => {
                self.focus = FocusArea::Panel3;
                self.update_focus_state();
            }
            '5' => {
                self.focus = FocusArea::Main;
                self.right_pane.focus_steps();
                self.update_focus_state();
            }
            '6' => {
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

        if self.focus == FocusArea::Panel3 {
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
        match Panel3Tab::from_index(self.left_pane.panel3.tabs.selected_index()) {
            Some(Panel3Tab::Collections) => {
                if let Some(panel) = self.left_pane.panel3.active_panel_mut()
                    && let Some(selected) = panel.selected()
                {
                    let collection_name = selected.text().to_string();
                    if let Ok(Some(collection)) = self.db.get_collection_by_name(&collection_name) {
                        // Delete by the stable id; names are not uniquely constrained.
                        self.request_confirmation(ConfirmDialog::new(
                            "Delete Collection",
                            format!("Delete collection \"{}\"?", collection_name),
                            DialogAction::DeleteCollection(collection.id),
                        ));
                        return Some(true);
                    }
                }
            }
            Some(Panel3Tab::Bookmarks) => {
                if let Some(panel) = self.left_pane.panel3.active_panel_mut()
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
        if let Some(Panel3Tab::Tours) =
            Panel3Tab::from_index(self.left_pane.panel3.tabs.selected_index())
        {
            let should_pull = self
                .left_pane
                .panel3
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
                let old_tab = self.left_pane.panel3.tabs.selected_index();

                let left_handled = self.left_pane.handle_event(event);
                let right_handled = self.right_pane.handle_event(event);
                if self.right_pane.needs_preview_update {
                    self.right_pane.needs_preview_update = false;
                    self.right_pane.update_preview(&self.db);
                }
                let handled = left_handled || right_handled;

                let new_tab = self.left_pane.panel3.tabs.selected_index();
                if new_tab != old_tab {
                    self.refresh_tags();
                    self.sync_steps_tab_label();
                    if Panel3Tab::from_index(new_tab) == Some(Panel3Tab::Tours) {
                        self.fetch_remote_tours();
                    }
                    // Refresh the preview for the newly active tab so it doesn't
                    // linger on content from the previous tab.
                    self.update_panel3_live_preview();
                }

                if self.focus == FocusArea::Panel3
                    && let Some(id) = self.left_pane.panel3.take_selection_change()
                    && let Some(tab) = Panel3Tab::from_index(new_tab)
                {
                    self.on_panel3_selection_changed(tab, &id);
                }
                handled
            }
            Event::Key(_key) => {
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
                        if let Some(id) = self.left_pane.panel3.take_selection_change()
                            && let Some(tab) =
                                Panel3Tab::from_index(self.left_pane.panel3.tabs.selected_index())
                        {
                            self.on_panel3_selection_changed(tab, &id);
                        }
                        handled
                    }
                    FocusArea::Main => {
                        let handled = self.right_pane.handle_event(event);
                        if self.right_pane.needs_preview_update {
                            self.right_pane.needs_preview_update = false;
                            self.right_pane.update_preview(&self.db);
                        }
                        handled
                    }
                    // Unreachable: should_handle_keybindings() returns false in
                    // filter mode, so the early return on line 668 fires first.
                    FocusArea::Filter => unreachable!(),
                };

                let new_tab = self.left_pane.panel3.tabs.selected_index();
                if new_tab != old_tab {
                    self.refresh_tags();
                    self.sync_steps_tab_label();
                    if Panel3Tab::from_index(new_tab) == Some(Panel3Tab::Tours) {
                        self.fetch_remote_tours();
                    }
                    // Refresh the preview for the newly active tab so it doesn't
                    // linger on content from the previous tab.
                    self.update_panel3_live_preview();
                }

                handled
            }
            _ => false,
        }
    }
}
