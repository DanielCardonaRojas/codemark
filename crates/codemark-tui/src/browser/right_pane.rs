use crate::browser::{SectionConfig, StepData, TabbedPanel};
use crate::component::{Component, MarkdownPanel};
use crate::event::Event;
use codemark_core::engine::bookmark::{Bookmark, Resolution};
use codemark_core::storage::db::Database;
use codemark_core::templates::{self, load_template};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style, Stylize},
    widgets::{Block, BorderType, Widget},
};

/// Tab index for the Info tab in the steps panel.
/// The steps panel has tabs in order: Steps (0), Info (1), Query (2).
const INFO_TAB_INDEX: usize = 1;

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
            cached_show_template,
            cached_details_template,
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

    /// Update the code preview based on current step.
    pub fn update_preview(&mut self) {
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
                preview.jump_to_range(step.line_number, step.line_end);
            }

            // Update Info tab with markdown
            let markdown = self.generate_markdown(
                &step.bookmark,
                step.resolution.as_ref(),
                templates::SHOW_TEMPLATE,
            );
            if let Some(md_panel) = self.steps.get_markdown_mut() {
                md_panel.set_markdown(markdown);
            }

            // Update Query tab
            if let Some(query_preview) = self.steps.get_query_preview_mut() {
                query_preview.set_code(step.bookmark.query.clone());
                query_preview.set_extension("scm".to_string());
            }

            // Update Details panel
            let details_markdown = self.generate_markdown(
                &step.bookmark,
                step.resolution.as_ref(),
                templates::DETAILS_TEMPLATE,
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
                }];
                self.pager_total = 1;
                self.pager_current = 0;
                self.active_bookmark_id = Some(bookmark_id.to_string());
                self.active_tour_name = None;
                self.update_preview();
            }
        } else {
            // Bookmark not found - clear stale preview state
            self.clear_preview_state();
        }
    }

    /// Clear the preview state (used when a bookmark cannot be loaded).
    pub fn clear_preview_state(&mut self) {
        self.steps_data.clear();
        self.pager_total = 0;
        self.pager_current = 0;
        self.active_bookmark_id = None;
        self.active_tour_name = None;
        self.update_preview();
    }

    /// Load a tour and its steps from the database.
    pub fn load_tour(&mut self, db: &Database, tour_name: &str) {
        let Some(collection) = db.get_collection_by_name(tour_name).ok().flatten() else {
            // Collection not found - clear stale preview state
            self.clear_preview_state();
            return;
        };

        let Ok(bookmarks) = db.list_bookmarks_in_collection(&collection.id) else {
            // Failed to load bookmarks - clear stale preview state
            self.clear_preview_state();
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
                    });
                }
            }

            if !new_steps.is_empty() {
                self.steps_data = new_steps;
                self.pager_total = self.steps_data.len();
                self.pager_current = 0;
                self.active_tour_name = Some(tour_name.to_string());
                self.active_bookmark_id = None;
                self.update_preview();
            } else {
                // Clear the right-pane state when no steps are available
                self.clear_preview_state();
            }
        }
    }

    /// Generate markdown for a bookmark using a specific template.
    pub fn generate_markdown(
        &self,
        bm: &Bookmark,
        res: Option<&Resolution>,
        template: &str,
    ) -> String {
        // Use the shared template from codemark_core with cached template content
        let resolutions = if let Some(r) = res { vec![r.clone()] } else { vec![] };

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
        let context = templates::BookmarkTemplateContext::from_bookmark(bm, &resolutions);
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
                        self.update_preview();
                    }
                    return true;
                }
                ratatui::crossterm::event::KeyCode::Right
                | ratatui::crossterm::event::KeyCode::Char('l') => {
                    if self.pager_current + 1 < self.pager_total {
                        self.pager_current += 1;
                        self.update_preview();
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
