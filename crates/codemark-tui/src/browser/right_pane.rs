use crate::browser::{SectionConfig, StepData, TabbedPanel, escape_markdown};
use crate::component::Component;
use crate::event::Event;
use codemark_core::engine::bookmark::{Bookmark, Resolution};
use codemark_core::storage::db::Database;
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Paragraph, Widget, Wrap},
};

/// Focus areas within the right pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RightPaneFocus {
    Steps,
    Details,
}

pub struct RightPane {
    /// Steps tabbed panel (Steps/Info)
    pub steps: TabbedPanel,
    /// Details panel showing bookmark metadata
    pub details: DetailsPanel,
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
}

pub struct DetailsPanel {
    /// Bookmark ID (short)
    pub id: String,
    /// Author
    pub author: String,
    /// Health status
    pub health: String,
    /// Commit hash
    pub commit: String,
    /// Creation date
    pub created_at: String,
    /// Tags associated with this bookmark
    pub tags: Vec<String>,
    /// Whether the panel is focused
    pub focused: bool,
    /// Last rendered area
    pub last_area: std::cell::Cell<Rect>,
}

impl RightPane {
    /// Create a new right pane.
    pub fn new(db: &Database) -> Self {
        let mut pane = Self {
            steps: TabbedPanel::new_steps_info(db),
            details: DetailsPanel::new(),
            steps_data: Vec::new(),
            focused: RightPaneFocus::Steps,
            pager_total: 0,
            pager_current: 0,
            last_area: std::cell::Cell::new(Rect::default()),
            info_config: SectionConfig::new(7, 13),
            active_tour_name: None,
            active_bookmark_id: None,
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
            let markdown =
                self.generate_bookmark_markdown(&step.bookmark, step.resolution.as_ref());
            if let Some(md_panel) = self.steps.get_markdown_mut() {
                md_panel.set_markdown(markdown);
            }

            // Update Query tab
            if let Some(query_preview) = self.steps.get_query_preview_mut() {
                query_preview.set_code(step.bookmark.query.clone());
                query_preview.set_extension("scm".to_string());
            }

            // Update Details panel
            self.details.set_bookmark(&step.bookmark, step.resolution.as_ref());
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

    /// Generate markdown for a bookmark.
    pub fn generate_bookmark_markdown(&self, bm: &Bookmark, res: Option<&Resolution>) -> String {
        let mut md = String::new();
        md.push_str(&format!("# Bookmark: {}\n\n", &bm.id[..8.min(bm.id.len())]));

        md.push_str("## Metadata\n\n");
        md.push_str("| Property | Value |\n");
        md.push_str("|----------|-------|\n");
        md.push_str(&format!("| **File** | {} |\n", escape_markdown(&bm.file_path)));
        md.push_str(&format!("| **Language** | {} |\n", bm.language));
        md.push_str(&format!("| **Health** | {} |\n", bm.health));
        md.push_str(&format!("| **Created** | {} |\n", bm.created_at));

        if let Some(ref author) = bm.created_by {
            md.push_str(&format!("| **Author** | {} |\n", escape_markdown(author)));
        }

        if let Some(res) = res {
            if let Some(ref commit) = res.commit_hash {
                md.push_str(&format!("| **Commit** | `{}` |\n", &commit[..8.min(commit.len())]));
            }
            if let Some(ref lr) = res.line_range {
                md.push_str(&format!("| **Lines** | {} |\n", lr));
            }
        }

        md.push('\n');

        if !bm.tags.is_empty() {
            md.push_str("## Tags\n\n");
            for tag in &bm.tags {
                md.push_str(&format!("- #{} \n", tag));
            }
            md.push('\n');
        }

        if !bm.annotations.is_empty() {
            md.push_str("## Notes\n\n");
            for ann in &bm.annotations {
                if let Some(ref notes) = ann.notes {
                    md.push_str(&format!("> {}\n\n", notes));
                }
            }
        }

        md
    }

    /// Render the right pane.
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        self.last_area.set(area);

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

        // Render details
        self.details.render(chunks[2], buf);
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
                let info_area = self.details.last_area();
                if col >= info_area.x
                    && col < info_area.x + info_area.width
                    && row >= info_area.y
                    && row < info_area.y + info_area.height
                {
                    self.focus_details();
                }
            }
        }

        // Forward to focused component first
        let handled = match self.focused {
            RightPaneFocus::Steps => self.steps.handle_event(event),
            RightPaneFocus::Details => false, // Details doesn't handle events
        };

        if handled {
            return true;
        }

        // Handle navigation within right pane if not handled by components
        if let Event::Key(key) = event {
            match key.code {
                ratatui::crossterm::event::KeyCode::Left
                | ratatui::crossterm::event::KeyCode::Char('h')
                    if self.focused == RightPaneFocus::Steps =>
                {
                    self.pager_current = self.pager_current.saturating_sub(1);
                    self.update_preview();
                    return true;
                }
                ratatui::crossterm::event::KeyCode::Right
                | ratatui::crossterm::event::KeyCode::Char('l')
                    if self.focused == RightPaneFocus::Steps =>
                {
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
}

impl DetailsPanel {
    /// Create a new details panel.
    pub fn new() -> Self {
        Self {
            id: String::new(),
            author: String::new(),
            health: String::new(),
            commit: String::new(),
            created_at: String::new(),
            tags: Vec::new(),
            focused: false,
            last_area: std::cell::Cell::new(Rect::default()),
        }
    }

    /// Set bookmark data.
    pub fn set_bookmark(&mut self, bm: &Bookmark, res: Option<&Resolution>) {
        self.id = bm.id[..8.min(bm.id.len())].to_string();
        self.author = bm.created_by.clone().unwrap_or_else(|| "Unknown".to_string());
        self.health = bm.health.to_string();
        self.created_at = bm.created_at.to_string();
        self.tags = bm.tags.iter().map(|t| format!("#{}", t)).collect();
        self.commit = res
            .and_then(|r| r.commit_hash.as_ref())
            .map(|c| c[..8.min(c.len())].to_string())
            .unwrap_or_else(|| "N/A".to_string());
    }
    /// Get the last rendered area.
    pub fn last_area(&self) -> Rect {
        self.last_area.get()
    }

    /// Render the details panel.
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        self.last_area.set(area);
        // Render border
        let border_style = if self.focused {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let block = ratatui::widgets::Block::bordered()
            .border_type(ratatui::widgets::BorderType::Rounded)
            .title("Details")
            .title_style(Style::default().bold())
            .border_style(border_style);

        let inner = block.inner(area);
        block.render(area, buf);

        if self.id.is_empty() {
            return;
        }

        // Build info content
        let mut info_lines = vec![
            Line::from(vec![
                Span::styled("ID:      ", Style::default().fg(Color::DarkGray)),
                Span::styled(&self.id, Style::default().fg(Color::Yellow)),
            ]),
            Line::from(vec![
                Span::styled("Author:  ", Style::default().fg(Color::DarkGray)),
                Span::styled(&self.author, Style::default().fg(Color::Cyan)),
            ]),
            Line::from(vec![
                Span::styled("Health:  ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    &self.health,
                    Style::default().fg(match self.health.as_str() {
                        "Active" | "Healthy" => Color::Green,
                        "Error" | "Stale" => Color::Red,
                        _ => Color::Yellow,
                    }),
                ),
            ]),
            Line::from(vec![
                Span::styled("Commit:  ", Style::default().fg(Color::DarkGray)),
                Span::styled(&self.commit, Style::default().fg(Color::Magenta)),
            ]),
            Line::from(vec![
                Span::styled("Created: ", Style::default().fg(Color::DarkGray)),
                Span::styled(&self.created_at, Style::default().fg(Color::Gray)),
            ]),
        ];

        // Add tags line
        if !self.tags.is_empty() {
            let mut tag_spans = vec![Span::styled("Tags: ", Style::default().fg(Color::DarkGray))];
            for (i, tag) in self.tags.iter().enumerate() {
                if i > 0 {
                    tag_spans.push(Span::raw(" "));
                }
                let color = match i % 4 {
                    0 => Color::Cyan,
                    1 => Color::Magenta,
                    2 => Color::Yellow,
                    _ => Color::Green,
                };
                tag_spans.push(Span::styled(tag, Style::default().fg(color)));
            }
            info_lines.push(Line::from(tag_spans));
        }

        let paragraph =
            Paragraph::new(info_lines).wrap(Wrap { trim: false }).alignment(Alignment::Left);

        paragraph.render(inner, buf);
    }

    /// Set focus state.
    pub fn set_focus(&mut self, focused: bool) {
        self.focused = focused;
    }
}
