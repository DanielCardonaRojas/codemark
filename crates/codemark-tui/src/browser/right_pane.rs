use crate::browser::{SectionConfig, StepData, TabbedPanel};
use crate::component::{Component, MarkdownPanel};
use crate::event::Event;
use codemark_core::engine::bookmark::{Bookmark, Resolution};
use codemark_core::engine::resolution as live_resolution;
use codemark_core::parser::languages::{Language as CodemarkLanguage, ParseCache};
use codemark_core::storage::db::Database;
use codemark_core::templates::{self, load_template};
use std::collections::HashMap;
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style, Stylize},
    widgets::{Block, BorderType, Widget},
};

/// Tab index for the Info tab in the steps panel.
/// The steps panel has tabs in order: Steps (0), Info (1), Query (2).
pub const INFO_TAB_INDEX: usize = 1;

/// Focus areas within the right pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RightPaneFocus {
    Steps,
    Details,
}

pub struct RightPane {
    /// Steps tabbed panel (Steps/Info)
    pub steps: TabbedPanel,
    /// Details panel showing bookmark metadata (now template-driven)
    pub details: MarkdownPanel,
    /// Data for each step in the current tour
    pub steps_data: Vec<StepData>,
    /// Currently focused section
    pub focused: RightPaneFocus,
    /// Pager total pages
    pub pager_total: usize,
    /// Pager current page
    pub pager_current: usize,
    /// Last rendered area
    pub last_area: std::cell::Cell<Rect>,
    /// Details height configuration
    pub info_config: SectionConfig,
    /// Active tour name (if a tour is loaded)
    pub active_tour_name: Option<String>,
    /// Active bookmark ID (if a single bookmark is loaded)
    pub active_bookmark_id: Option<String>,
    /// Cached show template content to avoid repeated disk reads
    cached_show_template: String,
    /// Cached details template content to avoid repeated disk reads
    cached_details_template: String,
    /// Flag set when step navigation changes and the caller must call update_preview
    pub needs_preview_update: bool,
    /// Cached HEAD commit hash to avoid re-running git I/O on every preview navigation
    cached_head_commit: Option<String>,
}

impl RightPane {
    /// Create a new right pane.
    pub fn new(db: &Database) -> Self {
        let cached_show_template = load_template(templates::SHOW_TEMPLATE);
        let cached_details_template = load_template(templates::DETAILS_TEMPLATE);

        let mut pane = Self {
            steps: TabbedPanel::new_steps_info(db),
            details: MarkdownPanel::new(),
            steps_data: Vec::new(),
            focused: RightPaneFocus::Steps,
            pager_total: 0,
            pager_current: 0,
            last_area: std::cell::Cell::new(Rect::default()),
            info_config: SectionConfig::new(7, 13),
            active_tour_name: None,
            active_bookmark_id: None,
            needs_preview_update: false,
            cached_show_template,
            cached_details_template,
            cached_head_commit: {
                let db_dir = db.path().parent().unwrap_or_else(|| db.path());
                codemark_core::git::context::detect_context(db_dir).and_then(|ctx| ctx.head_commit)
            },
        };

        // Try to load the first tour automatically
        if let Ok(collections) = db.list_collections()
            && let Some((first_tour, _)) = collections.first()
        {
            let name = first_tour.name.clone();
            pane.load_tour(db, &name);
        }

        pane
    }

    /// Refresh the cached HEAD commit (call after switching databases or repos).
    pub fn refresh_head_commit(&mut self, db: &Database) {
        let db_dir = db.path().parent().unwrap_or_else(|| db.path());
        self.cached_head_commit =
            codemark_core::git::context::detect_context(db_dir).and_then(|ctx| ctx.head_commit);
    }

    /// Update the code preview based on current step.
    pub fn update_preview(&mut self, db: &Database) {
        if let Some(step) = self.steps_data.get(self.pager_current) {
            let code = std::fs::read_to_string(&step.file_path)
                .unwrap_or_else(|_| format!("Error: Could not load file {}", step.file_path));

            let ext = std::path::Path::new(&step.file_path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("txt");

            if let Some(preview) = self.steps.get_step_preview_mut() {
                preview.set_code(code);
                preview.set_extension(ext.to_string());

                // Use the bookmark's relative file path (or the resolution's override)
                let relative_path = step
                    .resolution
                    .as_ref()
                    .and_then(|r| r.file_path.clone())
                    .unwrap_or_else(|| step.bookmark.file_path.clone());
                tracing::debug!(target: "codemark::ui", %relative_path, "Setting preview file header");
                preview.set_file_header(Some(relative_path));

                preview.jump_to_range(step.line_number, step.line_end);
            }

            let head_ref = self.cached_head_commit.as_deref();

            // Update Info tab with markdown (Full bookmark details)
            let info_markdown = self.generate_markdown(
                db,
                &step.bookmark,
                &step.resolutions,
                templates::SHOW_TEMPLATE,
                head_ref,
            );
            if let Some(md_panel) = self.steps.get_markdown_mut() {
                md_panel.set_markdown(info_markdown);
            }

            // Update Query tab
            if let Some(query_preview) = self.steps.get_query_preview_mut() {
                query_preview.set_code(step.bookmark.query.clone());
                query_preview.set_extension("scm".to_string());
            }

            // Update bottom Details pane with markdown (Annotations/Notes only)
            let details_markdown = self.generate_markdown(
                db,
                &step.bookmark,
                &step.resolutions,
                templates::DETAILS_TEMPLATE,
                head_ref,
            );
            self.details.set_markdown(details_markdown);
        }
    }

    // @lat: [[tui-line-range-selection#Load bookmark with range]]
    /// Load a single bookmark for previewing.
    pub fn load_bookmark(&mut self, db: &Database, bookmark_id: &str) {
        if let Ok(Some(bm)) = db.get_bookmark(bookmark_id) {
            let mut line_number = 0;
            let mut line_end = None;
            let mut file_path = bm.file_path.clone();

            // Get the best resolution for preview (from nearest ancestor commit)
            let resolution = db.get_preview_resolution(&bm.id).ok().flatten();

            // Get all resolutions for showing full history
            let resolutions = db.list_resolutions(&bm.id, 100).unwrap_or_default();

            // Extract line_range and file_path from the resolution
            if let Some(ref res) = resolution {
                if let Some(fp) = res.file_path.as_ref() {
                    file_path = fp.clone();
                }
                if let Some(lr) = res.line_range.as_ref() {
                    let parts: Vec<&str> = lr.split('-').collect();
                    if let (Some(start), Some(end)) = (
                        parts.first().and_then(|s| s.parse::<usize>().ok()),
                        parts.get(1).and_then(|s| s.parse::<usize>().ok()),
                    ) {
                        line_number = start.saturating_sub(1);
                        line_end = Some(end.saturating_sub(1));
                    } else if let Some(start) = parts.first().and_then(|s| s.parse::<usize>().ok())
                    {
                        line_number = start.saturating_sub(1);
                    }
                }
            }

            if let Ok(abs_path) =
                codemark_core::git::context::resolve_bookmark_file_path(&file_path, db.path())
            {
                self.steps_data = vec![StepData {
                    file_path: abs_path.to_string_lossy().to_string(),
                    line_number,
                    line_end,
                    bookmark: bm,
                    resolution,
                    resolutions,
                }];
                self.pager_total = 1;
                self.pager_current = 0;
                self.active_bookmark_id = Some(bookmark_id.to_string());
                self.active_tour_name = None;
                self.update_preview(db);
            }
        } else {
            // Bookmark not found - clear stale preview state
            self.clear_preview_state(db);
        }
    }

    /// Load a single bookmark for previewing using live (on-the-fly) resolution.
    ///
    /// Runs `resolve_transient()` synchronously via `block_on` to get the current
    /// location of the bookmarked code directly from disk + tree-sitter, without
    /// reading persisted resolutions from the database. Falls back to the
    /// persisted path (`load_bookmark()`) on error.
    pub fn load_bookmark_live(
        &mut self,
        db: &Database,
        bookmark_id: &str,
        session_cache: &mut HashMap<CodemarkLanguage, ParseCache>,
    ) {
        let Some(bm) = db.get_bookmark(bookmark_id).ok().flatten() else {
            self.clear_preview_state(db);
            return;
        };

        // Try live resolution
        match Self::resolve_bookmark_live(&bm, db, session_cache) {
            Ok((abs_path, start_line, end_line)) => {
                // Get all resolutions for showing full history in Info tab
                let resolutions = db.list_resolutions(&bm.id, 100).unwrap_or_default();

                self.steps_data = vec![StepData {
                    file_path: abs_path,
                    line_number: start_line,
                    line_end: Some(end_line),
                    bookmark: bm,
                    resolution: None,
                    resolutions,
                }];
                self.pager_total = 1;
                self.pager_current = 0;
                self.active_bookmark_id = Some(bookmark_id.to_string());
                self.active_tour_name = None;
                self.update_preview(db);
            }
            Err(e) => {
                tracing::warn!(
                    target: "codemark::ui",
                    bookmark_id = %bookmark_id,
                    error = %e,
                    "Live resolution failed, falling back to persisted path"
                );
                self.load_bookmark(db, bookmark_id);
            }
        }
    }

    /// Load a tour using live resolution for each step.
    ///
    /// Same pattern as `load_tour()` but uses `resolve_transient()` for each
    /// bookmark in the collection to get current-disk locations.
    pub fn load_tour_live(
        &mut self,
        db: &Database,
        tour_name: &str,
        session_cache: &mut HashMap<CodemarkLanguage, ParseCache>,
    ) {
        let Some(collection) = db.get_collection_by_name(tour_name).ok().flatten() else {
            self.clear_preview_state(db);
            return;
        };

        let Ok(bookmarks) = db.list_bookmarks_in_collection(&collection.id) else {
            self.clear_preview_state(db);
            return;
        };

        let mut new_steps = Vec::new();
        for bm in bookmarks {
            let resolutions = db.list_resolutions(&bm.id, 100).unwrap_or_default();

            match Self::resolve_bookmark_live(&bm, db, session_cache) {
                Ok((abs_path, start_line, end_line)) => {
                    new_steps.push(StepData {
                        file_path: abs_path,
                        line_number: start_line,
                        line_end: Some(end_line),
                        bookmark: bm,
                        resolution: None,
                        resolutions,
                    });
                }
                Err(_) => {
                    // Fallback to persisted resolution for this step
                    let mut line_number = 0;
                    let mut line_end = None;
                    let mut file_path = bm.file_path.clone();
                    let resolution = db.get_preview_resolution(&bm.id).ok().flatten();

                    if let Some(ref res) = resolution {
                        if let Some(fp) = res.file_path.as_ref() {
                            file_path = fp.clone();
                        }
                        if let Some(lr) = res.line_range.as_ref() {
                            let parts: Vec<&str> = lr.split('-').collect();
                            if let (Some(start), Some(end)) = (
                                parts.first().and_then(|s| s.parse::<usize>().ok()),
                                parts.get(1).and_then(|s| s.parse::<usize>().ok()),
                            ) {
                                line_number = start.saturating_sub(1);
                                line_end = Some(end.saturating_sub(1));
                            } else if let Some(start) =
                                parts.first().and_then(|s| s.parse::<usize>().ok())
                            {
                                line_number = start.saturating_sub(1);
                            }
                        }
                    }

                    if let Ok(abs_path) = codemark_core::git::context::resolve_bookmark_file_path(
                        &file_path,
                        db.path(),
                    ) {
                        new_steps.push(StepData {
                            file_path: abs_path.to_string_lossy().to_string(),
                            line_number,
                            line_end,
                            bookmark: bm,
                            resolution,
                            resolutions,
                        });
                    }
                }
            }
        }

        if !new_steps.is_empty() {
            self.steps_data = new_steps;
            self.pager_total = self.steps_data.len();
            self.pager_current = 0;
            self.active_tour_name = Some(tour_name.to_string());
            self.active_bookmark_id = None;
            self.update_preview(db);
        } else {
            self.clear_preview_state(db);
        }
    }

    /// Resolve a bookmark on-the-fly using tree-sitter, returning (abs_path, start_line, end_line).
    /// All line numbers are 0-indexed (from tree-sitter Point.row).
    fn resolve_bookmark_live(
        bm: &Bookmark,
        db: &Database,
        session_cache: &mut HashMap<CodemarkLanguage, ParseCache>,
    ) -> std::result::Result<(String, usize, usize), codemark_core::error::Error> {
        use std::str::FromStr;

        let language = CodemarkLanguage::from_str(&bm.language).map_err(|e| {
            codemark_core::error::Error::Input(format!("unsupported language {}: {}", bm.language, e))
        })?;

        // Get or create a ParseCache for this language
        let cache = session_cache.entry(language).or_insert_with(|| {
            ParseCache::new(language).expect("failed to create ParseCache")
        });

        let provider = codemark_core::vfs::LocalFileProvider;
        let handle = tokio::runtime::Handle::current();

        let result = tokio::task::block_in_place(|| {
            handle.block_on(live_resolution::resolve_transient(
                bm,
                cache,
                language,
                db.path(),
                &provider,
            ))
        })?;

        // Resolve the file path to absolute
        let abs_path =
            codemark_core::git::context::resolve_bookmark_file_path(&result.file_path, db.path())?;

        Ok((
            abs_path.to_string_lossy().to_string(),
            result.start_line,
            result.end_line,
        ))
    }

    /// Clear the preview state (used when a bookmark cannot be loaded).
    pub fn clear_preview_state(&mut self, db: &Database) {
        self.steps_data.clear();
        self.pager_total = 0;
        self.pager_current = 0;
        self.active_bookmark_id = None;
        self.active_tour_name = None;
        self.update_preview(db);
    }

    /// Load a tour and its steps from the database.
    pub fn load_tour(&mut self, db: &Database, tour_name: &str) {
        let Some(collection) = db.get_collection_by_name(tour_name).ok().flatten() else {
            // Collection not found - clear stale preview state
            self.clear_preview_state(db);
            return;
        };

        let Ok(bookmarks) = db.list_bookmarks_in_collection(&collection.id) else {
            // Failed to load bookmarks - clear stale preview state
            self.clear_preview_state(db);
            return;
        };

        {
            let mut new_steps = Vec::new();
            for bm in bookmarks {
                let mut line_number = 0;
                let mut line_end = None;
                let mut file_path = bm.file_path.clone();

                // Get the best resolution for preview (from nearest ancestor commit)
                let resolution = db.get_preview_resolution(&bm.id).ok().flatten();

                // Get all resolutions for showing full history
                let resolutions = db.list_resolutions(&bm.id, 100).unwrap_or_default();

                // Extract line_range and file_path from the resolution
                if let Some(ref res) = resolution {
                    if let Some(fp) = res.file_path.as_ref() {
                        file_path = fp.clone();
                    }
                    if let Some(lr) = res.line_range.as_ref() {
                        let parts: Vec<&str> = lr.split('-').collect();
                        if let (Some(start), Some(end)) = (
                            parts.first().and_then(|s| s.parse::<usize>().ok()),
                            parts.get(1).and_then(|s| s.parse::<usize>().ok()),
                        ) {
                            line_number = start.saturating_sub(1);
                            line_end = Some(end.saturating_sub(1));
                        } else if let Some(start) =
                            parts.first().and_then(|s| s.parse::<usize>().ok())
                        {
                            line_number = start.saturating_sub(1);
                        }
                    }
                }

                // Resolve absolute path
                if let Ok(abs_path) =
                    codemark_core::git::context::resolve_bookmark_file_path(&file_path, db.path())
                {
                    new_steps.push(StepData {
                        file_path: abs_path.to_string_lossy().to_string(),
                        line_number,
                        line_end,
                        bookmark: bm,
                        resolution,
                        resolutions,
                    });
                }
            }

            if !new_steps.is_empty() {
                self.steps_data = new_steps;
                self.pager_total = self.steps_data.len();
                self.pager_current = 0;
                self.active_tour_name = Some(tour_name.to_string());
                self.active_bookmark_id = None;
                self.update_preview(db);
            } else {
                // Clear the right-pane state when no steps are available
                self.clear_preview_state(db);
            }
        }
    }

    /// Generate markdown for a bookmark using a specific template.
    pub fn generate_markdown(
        &self,
        db: &Database,
        bm: &Bookmark,
        resolutions: &[Resolution],
        template: &str,
        current_head: Option<&str>,
    ) -> String {
        // Select the appropriate cached template to avoid repeated disk reads
        let template_content = match template {
            templates::SHOW_TEMPLATE => &self.cached_show_template,
            templates::DETAILS_TEMPLATE => &self.cached_details_template,
            _ => {
                // Fallback for unknown templates (shouldn't happen in normal use)
                return format!(
                    "# Bookmark: {}\n\nError: Unknown template {}",
                    &bm.id[..8.min(bm.id.len())],
                    template
                );
            }
        };

        // Create context and render using the cached template content
        let repo_path = db.path().parent().unwrap_or_else(|| db.path());
        let context = templates::BookmarkTemplateContext::from_bookmark(
            bm,
            resolutions,
            repo_path,
            current_head,
        );
        let handlebars = templates::create_handlebars_engine();

        match handlebars.render_template(template_content, &context) {
            Ok(rendered) => rendered,
            Err(e) => {
                // Fallback to simple format if template fails
                format!(
                    "# Bookmark: {}\n\nError rendering template {}: {}",
                    &bm.id[..8.min(bm.id.len())],
                    template,
                    e
                )
            }
        }
    }

    /// Render the right pane.
    ///
    /// # Arguments
    /// * `area` - The area to render in
    /// * `buf` - The buffer to render to
    /// * `fullscreen` - If true, hide the details pane and use full area for steps
    pub fn render(&self, area: Rect, buf: &mut Buffer, fullscreen: bool) {
        self.last_area.set(area);

        if fullscreen {
            // In fullscreen mode, use the entire area for the steps panel
            self.steps.render(area, buf);
            return;
        }

        let info_height = if self.focused == RightPaneFocus::Details {
            self.info_config.max
        } else {
            self.info_config.min
        };

        // If only one step, hide the pager
        let constraints = if self.pager_total > 1 {
            vec![Constraint::Min(0), Constraint::Length(1), Constraint::Length(info_height)]
        } else {
            vec![Constraint::Min(0), Constraint::Length(0), Constraint::Length(info_height)]
        };

        // Split vertically: steps (flex), pager (1 row or 0), details (dynamic height)
        let chunks =
            Layout::default().direction(Direction::Vertical).constraints(constraints).split(area);

        // Render steps tabbed panel
        self.steps.render(chunks[0], buf);

        // Render pager if needed
        if self.pager_total > 1 {
            use crate::component::Pager;
            let pager = Pager::new(self.pager_total, self.pager_current);
            pager.render(chunks[1], buf);
        }

        // Render details with border
        let border_style = if self.focused == RightPaneFocus::Details {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .title("Details")
            .title_style(Style::default().bold())
            .border_style(border_style);

        let inner = block.inner(chunks[2]);
        block.render(chunks[2], buf);
        self.details.render(inner, buf);
    }

    /// Handle an event.
    pub fn handle_event(&mut self, event: &Event) -> bool {
        // Handle mouse clicks for internal focus switching
        if let Event::Mouse(mouse) = event
            && let ratatui::crossterm::event::MouseEventKind::Down(
                ratatui::crossterm::event::MouseButton::Left,
            ) = mouse.kind
        {
            let col = mouse.column;
            let row = mouse.row;

            let steps_area = self.steps.last_area();
            if col >= steps_area.x
                && col < steps_area.x + steps_area.width
                && row >= steps_area.y
                && row < steps_area.y + steps_area.height
            {
                self.focus_steps();
            } else {
                let info_area = self.details_area();
                if col >= info_area.x
                    && col < info_area.x + info_area.width
                    && row >= info_area.y
                    && row < info_area.y + info_area.height
                {
                    self.focus_details();
                }
            }
        }

        // Forward to components
        let handled = match event {
            Event::Mouse(_) => {
                // For mouse events, check both to allow scrolling any hovered pane
                self.steps.handle_event(event) || self.details.handle_event(event)
            }
            _ => {
                // For keyboard events, follow focus
                match self.focused {
                    RightPaneFocus::Steps => self.steps.handle_event(event),
                    RightPaneFocus::Details => self.details.handle_event(event),
                }
            }
        };

        if handled {
            return true;
        }

        // Handle navigation within right pane if not handled by components
        if let Event::Key(key) = event {
            match key.code {
                // Left/right navigation works for both Steps and Details focus
                ratatui::crossterm::event::KeyCode::Left
                | ratatui::crossterm::event::KeyCode::Char('h') => {
                    if self.pager_current > 0 {
                        self.pager_current = self.pager_current.saturating_sub(1);
                        self.needs_preview_update = true;
                    }
                    return true;
                }
                ratatui::crossterm::event::KeyCode::Right
                | ratatui::crossterm::event::KeyCode::Char('l') => {
                    if self.pager_current + 1 < self.pager_total {
                        self.pager_current += 1;
                        self.needs_preview_update = true;
                    }
                    return true;
                }
                ratatui::crossterm::event::KeyCode::Down
                    if self.focused == RightPaneFocus::Steps =>
                {
                    self.focus_details();
                    return true;
                }
                ratatui::crossterm::event::KeyCode::Up
                    if self.focused == RightPaneFocus::Details =>
                {
                    self.focus_steps();
                    return true;
                }
                _ => {}
            }
        }

        false
    }

    /// Set focus state.
    pub fn set_focus(&mut self, focused: bool) {
        match self.focused {
            RightPaneFocus::Steps => self.steps.set_focus(focused),
            RightPaneFocus::Details => self.details.set_focus(focused),
        }
    }

    /// Focus the steps section.
    pub fn focus_steps(&mut self) {
        self.focused = RightPaneFocus::Steps;
        self.steps.set_focus(true);
        self.details.set_focus(false);
    }

    /// Focus the details section.
    pub fn focus_details(&mut self) {
        self.focused = RightPaneFocus::Details;
        self.details.set_focus(true);
        self.steps.set_focus(false);
    }

    /// Get the last rendered area.
    pub fn last_area(&self) -> Rect {
        self.last_area.get()
    }

    /// Toggle internal focus between Steps and Details.
    pub fn toggle_internal_focus(&mut self) {
        match self.focused {
            RightPaneFocus::Steps => self.focus_details(),
            RightPaneFocus::Details => self.focus_steps(),
        }
    }

    fn details_area(&self) -> Rect {
        let area = self.last_area.get();
        let info_height = if self.focused == RightPaneFocus::Details {
            self.info_config.max
        } else {
            self.info_config.min
        };

        Rect {
            x: area.x,
            y: area.y + area.height.saturating_sub(info_height),
            width: area.width,
            height: info_height,
        }
    }

    /// Get the markdown content from the currently focused markdown panel.
    /// Returns the content from the Details panel when focused on Details,
    /// or from the Info tab's markdown panel when the Info tab is selected.
    /// Returns None if there's no content or the preview state is cleared.
    pub fn active_markdown_content(&self) -> Option<&str> {
        // Return None if there are no steps loaded (preview state is cleared)
        if self.steps_data.is_empty() {
            return None;
        }

        let content = match self.focused {
            RightPaneFocus::Details => Some(self.details.markdown()),
            RightPaneFocus::Steps => {
                // Only return markdown if the Info tab is selected
                if self.steps.tabs.selected_index() == INFO_TAB_INDEX {
                    self.steps.get_markdown().map(|m| m.markdown())
                } else {
                    None
                }
            }
        };

        // Filter out empty or whitespace-only content
        content.filter(|m| !m.trim().is_empty())
    }
}
