//! Browser layout for the Codemark TUI.
//!
//! This module provides the main browser layout with a left sidebar
//! containing search, repos, and tours, and a right main content area.

mod search;
mod tabs;

pub use search::SearchBar;
pub use tabs::{Tab, TabSelection};

use crate::component::{CodePreview, Component, HealthStatus, MarkdownPanel, Panel, PanelItem};
use crate::event::Event;
use crate::ui::KeyBinding;
use codemark_core::engine::bookmark::{Bookmark, BookmarkFilter, Resolution};
use codemark_core::storage::db::Database;
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Paragraph, Widget, Wrap},
};

/// An external command to be executed by the TUI host.
#[derive(Debug, Clone)]
pub struct ExternalCommand {
    /// The program to execute
    pub program: String,
    /// Arguments for the program
    pub args: Vec<String>,
    /// Whether the TUI should wait for this command to complete (terminal editors)
    pub should_wait: bool,
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
    /// Pending external command to be executed
    pending_command: Option<ExternalCommand>,
}

/// Areas that can be focused in the browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusArea {
    /// Search bar is focused
    Search,
    /// Tabbed panel 1 (Repos/Accounts) is focused
    Panel1,
    /// Tabbed panel 2 (Tags/Branches) is focused
    Panel2,
    /// Tabbed panel 3 (Tours/Collections/Bookmarks) is focused
    Panel3,
    /// Right main panel is focused
    Main,
}

/// Configuration for a sidebar section's height.
struct SectionConfig {
    /// Minimum height when unfocused
    min: u16,
    /// Maximum height when focused
    max: u16,
}

impl SectionConfig {
    fn new(min: u16, max: u16) -> Self {
        Self { min, max }
    }
}

/// Left sidebar containing search and three tabbed panels.
struct LeftPane {
    /// Search bar component
    search: SearchBar,
    /// First tabbed panel (Repos/Accounts)
    panel1: TabbedPanel,
    /// Second tabbed panel (Tags/Branches)
    panel2: TabbedPanel,
    /// Third tabbed panel (Tours/Collections/Bookmarks)
    panel3: TabbedPanel,
    /// Section height configurations
    panel1_config: SectionConfig,
    panel2_config: SectionConfig,
}

// @lat: [[tui-line-range-selection#StepData struct]]
/// Data for a single step in a tour.
struct StepData {
    /// Path to the file for this step
    file_path: String,
    /// Line number to jump to (0-indexed)
    line_number: usize,
    /// Optional end line number for range highlighting (0-indexed, inclusive)
    line_end: Option<usize>,
    /// Real bookmark data
    bookmark: Bookmark,
    /// Resolution data if available
    resolution: Option<Resolution>,
}

/// Right pane containing Steps and Details sections.
struct RightPane {
    /// Steps tabbed panel (Steps/Info)
    steps: TabbedPanel,
    /// Details panel showing bookmark metadata
    details: DetailsPanel,
    /// Data for each step in the current tour
    steps_data: Vec<StepData>,
    /// Currently focused section
    focused: RightPaneFocus,
    /// Pager total pages
    pager_total: usize,
    /// Pager current page
    pager_current: usize,
    /// Last rendered area
    last_area: std::cell::Cell<Rect>,
    /// Details height configuration
    info_config: SectionConfig,
}

/// Focus areas within the right pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RightPaneFocus {
    Steps,
    Details,
}

/// Details panel displaying bookmark metadata.
struct DetailsPanel {
    /// Bookmark ID (short)
    id: String,
    /// Author
    author: String,
    /// Health status
    health: String,
    /// Commit hash
    commit: String,
    /// Creation date
    created_at: String,
    /// Tags associated with this bookmark
    tags: Vec<String>,
    /// Whether the panel is focused
    focused: bool,
    /// Last rendered area
    last_area: std::cell::Cell<Rect>,
}

/// Content for a tabbed panel.
enum TabContent {
    /// A list of items
    List(Panel),
    /// A code preview
    Preview(CodePreview),
    /// A markdown panel
    Markdown(MarkdownPanel),
}

impl TabContent {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        match self {
            TabContent::List(p) => p.render(area, buf),
            TabContent::Preview(p) => p.render(area, buf),
            TabContent::Markdown(p) => p.render(area, buf),
        }
    }

    fn handle_event(&mut self, event: &Event) -> bool {
        match self {
            TabContent::List(p) => p.handle_event(event),
            TabContent::Preview(p) => p.handle_event(event),
            TabContent::Markdown(p) => p.handle_event(event),
        }
    }

    fn set_focus(&mut self, focused: bool) {
        match self {
            TabContent::List(p) => p.set_focus(focused),
            TabContent::Preview(p) => p.set_focus(focused),
            TabContent::Markdown(p) => p.set_focus(focused),
        }
    }
}

/// A tabbed panel component with multiple content panels.
struct TabbedPanel {
    /// Tab selection
    tabs: TabSelection,
    /// Content panels for each tab
    panels: Vec<TabContent>,
    /// Currently focused
    focused: bool,
    /// Last rendered area
    last_area: std::cell::Cell<Rect>,
}

/// Escape special markdown characters.
fn escape_markdown(text: &str) -> String {
    text.replace('_', "\\_")
        .replace('*', "\\*")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('#', "\\#")
}

impl BrowserLayout {
    /// Create a new browser layout.
    pub fn new(db: Database) -> Self {
        use codemark_core::storage::registry;
        let registry = registry::open_registry().expect("Failed to open global registry");

        let mut layout = Self {
            left_pane: LeftPane::new(&db, &registry),
            right_pane: RightPane::new(&db),
            focus: FocusArea::Panel3,
            db,
            registry,
            pending_command: None,
        };
        layout.update_focus_state();
        layout
    }

    /// Take the pending external command, if any.
    pub fn take_pending_command(&mut self) -> Option<ExternalCommand> {
        self.pending_command.take()
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

    /// Refresh all panels from the current active database.
    pub fn refresh_all_panels(&mut self) {
        // 1. Update Panel 1 Repos (in-place to preserve selection)
        let repo_items = TabbedPanel::build_repo_items(&self.db, &self.registry);
        if let Some(p) = self.left_pane.panel1.get_list_panel_mut(0) {
            p.set_items(repo_items);
        }

        // 2. Update Tags/Branches (in-place)
        let (tags, branches) = TabbedPanel::build_tags_branches_items(&self.db);
        if let Some(p) = self.left_pane.panel2.get_list_panel_mut(0) {
            p.set_items(tags);
        }
        if let Some(p) = self.left_pane.panel2.get_list_panel_mut(1) {
            p.set_items(branches);
        }

        // 3. Update Tours/Collections/Bookmarks (in-place)
        let (tours, collections, bookmarks) = TabbedPanel::build_panel3_items(&self.db);
        if let Some(p) = self.left_pane.panel3.get_list_panel_mut(0) {
            p.set_items(tours);
        }
        if let Some(p) = self.left_pane.panel3.get_list_panel_mut(1) {
            p.set_items(collections);
        }
        if let Some(p) = self.left_pane.panel3.get_list_panel_mut(2) {
            p.set_items(bookmarks);
        }

        // 4. Update Step previews (Right Pane)
        if let Ok(collections) = self.db.list_collections() {
            if let Some((first_tour, _)) = collections.first() {
                let name = first_tour.name.clone();
                self.right_pane.load_tour(&self.db, &name);
            } else {
                // Clear steps if no tours found
                self.right_pane.steps_data.clear();
                self.right_pane.pager_total = 0;
                self.right_pane.pager_current = 0;
            }
        }
    }

    /// Get the current focus area.
    pub fn focus(&self) -> FocusArea {
        self.focus
    }

    /// Get context-aware keybindings for the status bar.
    pub fn get_status_bindings(&self) -> Vec<KeyBinding> {
        let mut bindings = vec![
            KeyBinding::new("/", "Filter"),
            KeyBinding::new("?", "Help"),
            KeyBinding::new("q", "Quit"),
        ];

        match self.focus {
            FocusArea::Search => {
                bindings.insert(0, KeyBinding::new("Enter", "Search"));
            }
            FocusArea::Panel1 => {
                // Repos / Accounts
                bindings.insert(0, KeyBinding::new("Enter", "Select"));
            }
            FocusArea::Panel2 => {
                // Tags / Branches
                bindings.insert(0, KeyBinding::new("Enter", "Filter"));
            }
            FocusArea::Panel3 => {
                // Tours / Collections / Bookmarks
                let active_tab = self.left_pane.panel3.tabs.selected_index();
                match active_tab {
                    0 => {
                        // Tours
                        bindings.insert(0, KeyBinding::new("p", "Pull"));
                        bindings.insert(1, KeyBinding::new("P", "Push"));
                    }
                    1 => {
                        // Collections
                        bindings.insert(0, KeyBinding::new("Enter", "Open"));
                    }
                    2 => {
                        // Bookmarks
                        bindings.insert(0, KeyBinding::new("o", "Open"));
                        bindings.insert(1, KeyBinding::new("Enter", "Preview"));
                    }
                    _ => {}
                }
            }
            FocusArea::Main => {
                bindings.insert(0, KeyBinding::new("Enter", "Select Step"));
                bindings.insert(1, KeyBinding::new("o", "Open File"));
                bindings.insert(2, KeyBinding::new("Esc", "Back to Tours"));
            }
        }

        bindings
    }

    /// Get current filters/metadata for the status bar.
    pub fn get_status_metadata(&self) -> Line<'_> {
        vec![
            Span::styled("Repo: ", Style::default().fg(Color::DarkGray)),
            Span::styled("codemark", Style::default().fg(Color::Cyan)),
            Span::styled(" | ", Style::default().fg(Color::Gray)),
            Span::styled("Branch: ", Style::default().fg(Color::DarkGray)),
            Span::styled("main", Style::default().fg(Color::Yellow)),
            Span::styled(" | ", Style::default().fg(Color::Gray)),
            Span::styled("#ui", Style::default().fg(Color::Magenta)),
        ]
        .into()
    }

    /// Open the currently selected bookmark or tour step in the editor.
    pub fn open_in_editor(&mut self) {
        use codemark_core::config::Config;

        // For now, only support opening from the right pane (Main)
        let step = if self.focus == FocusArea::Main {
            self.right_pane.steps_data.get(self.right_pane.pager_current)
        } else {
            None
        };

        let Some(step) = step else {
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

        let command_template = if let Some(cmd) = config
            .open
            .get_command_for_extension(extension)
            .or(config.open.default.as_ref())
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

        if let Some(tokens) = shlex::split(&substituted) {
            if !tokens.is_empty() {
                let program = tokens[0].clone();
                let args = tokens[1..].to_vec();
                let program_name = std::path::Path::new(&program)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("");
                let should_wait = config.open.should_wait_for_editor(program_name);

                self.pending_command = Some(ExternalCommand {
                    program,
                    args,
                    should_wait,
                });
            }
        }
    }

    /// Apply a filter to the currently focused panel.
    pub fn apply_filter(&mut self, query: &str) {
        match self.focus {
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
                        Some(h) => match h.to_string().as_str() {
                            "Healthy" => HealthStatus::Healthy,
                            "Error" => HealthStatus::Error,
                            _ => HealthStatus::Warning,
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

            if let Some(TabContent::List(p)) = self.left_pane.panel3.panels.get_mut(0) {
                p.set_items(tour_items);
            }
            if let Some(TabContent::List(p)) = self.left_pane.panel3.panels.get_mut(1) {
                p.set_items(collection_items);
            }
        }

        // 2. Update Bookmarks (Panel 3, tab 2)
        if let Ok(bookmarks) = self.db.list_bookmarks(&BookmarkFilter::default()) {
            let filtered_items: Vec<PanelItem> = bookmarks
                .into_iter()
                .filter(|bm| {
                    let branch_match = active_branches.is_empty(); // Bookmarks don't have direct branch column in this version
                    let tag_match =
                        active_tags.is_empty() || bm.tags.iter().any(|t| active_tags.contains(t));
                    branch_match && tag_match
                })
                .map(|bm| {
                    PanelItem::new(bm.file_path)
                        .secondary_text(format!("L{}", bm.query))
                        .metadata(bm.created_by.unwrap_or_default())
                        .user_data(bm.id)
                })
                .collect();

            if let Some(TabContent::List(p)) = self.left_pane.panel3.panels.get_mut(2) {
                p.set_items(filtered_items);
            }
        }
    }

    /// Set the focus area.
    pub fn set_focus(&mut self, focus: FocusArea) {
        self.focus = focus;
        self.update_focus_state();
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
            }
            FocusArea::Main => {
                self.right_pane.set_focus(true);
            }
        }
    }

    /// Render the browser layout.
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        // Split vertically: 40% left, 60% right
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
            .split(area);

        // Render left pane
        self.left_pane.render(chunks[0], buf);

        // Render right pane
        self.right_pane.render(chunks[1], buf);
    }

    /// Handle an event.
    pub fn handle_event(&mut self, event: &Event) -> bool {
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
                ratatui::crossterm::event::KeyCode::Enter
                | ratatui::crossterm::event::KeyCode::Char(' ') => {
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
                        let active_tab = self.left_pane.panel3.tabs.selected_index();
                        if active_tab == 0 || active_tab == 1 {
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
                        } else if active_tab == 2 {
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
                    }
                }
                ratatui::crossterm::event::KeyCode::Tab => {
                    self.next_focus();
                    return true;
                }
                ratatui::crossterm::event::KeyCode::BackTab => {
                    self.previous_focus();
                    return true;
                }
                ratatui::crossterm::event::KeyCode::Esc => {
                    if self.focus == FocusArea::Main {
                        self.set_focus(FocusArea::Panel3);
                        return true;
                    }
                }
                // Number keys for direct section access
                ratatui::crossterm::event::KeyCode::Char('1') => {
                    self.focus = FocusArea::Search;
                    self.update_focus_state();
                    return true;
                }
                ratatui::crossterm::event::KeyCode::Char('2') => {
                    self.focus = FocusArea::Panel1;
                    self.update_focus_state();
                    return true;
                }
                ratatui::crossterm::event::KeyCode::Char('3') => {
                    self.focus = FocusArea::Panel2;
                    self.update_focus_state();
                    return true;
                }
                ratatui::crossterm::event::KeyCode::Char('4') => {
                    self.focus = FocusArea::Panel3;
                    self.update_focus_state();
                    return true;
                }
                ratatui::crossterm::event::KeyCode::Char('5') => {
                    self.focus = FocusArea::Main;
                    self.right_pane.focus_steps();
                    self.update_focus_state();
                    return true;
                }
                ratatui::crossterm::event::KeyCode::Char('6') => {
                    self.focus = FocusArea::Main;
                    self.right_pane.focus_details();
                    self.update_focus_state();
                    return true;
                }
                ratatui::crossterm::event::KeyCode::Char('o') => {
                    self.open_in_editor();
                    return true;
                }
                _ => {}
            }
        }

        // 3. Delegate to panes
        match event {
            Event::Mouse(_) => {
                self.left_pane.handle_event(event) || self.right_pane.handle_event(event)
            }
            Event::Key(_) => match self.focus {
                FocusArea::Search => self.left_pane.search.handle_event(event),
                FocusArea::Panel1 => self.left_pane.panel1.handle_event(event),
                FocusArea::Panel2 => self.left_pane.panel2.handle_event(event),
                FocusArea::Panel3 => self.left_pane.panel3.handle_event(event),
                FocusArea::Main => self.right_pane.handle_event(event),
            },
            _ => false,
        }
    }
}

impl LeftPane {
    /// Create a new left pane.
    fn new(db: &Database, registry: &rusqlite::Connection) -> Self {
        Self {
            search: SearchBar::new(),
            panel1: TabbedPanel::new_repos_accounts(db, registry),
            panel2: TabbedPanel::new_tags_branches(db),
            panel3: TabbedPanel::new_tours_collections_bookmarks(db),
            panel1_config: SectionConfig::new(4, 6),
            panel2_config: SectionConfig::new(4, 8),
        }
    }

    /// Render the left pane.
    fn render(&self, area: Rect, buf: &mut Buffer) {
        // Calculate heights based on focus
        let p1_height =
            if self.panel1.focused { self.panel1_config.max } else { self.panel1_config.min };

        let p2_height =
            if self.panel2.focused { self.panel2_config.max } else { self.panel2_config.min };

        // Split vertically: search (3 rows), panel1, panel2, panel3 (takes the rest)
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),         // Search bar
                Constraint::Length(p1_height), // Panel 1 (Repos/Accounts)
                Constraint::Length(p2_height), // Panel 2 (Tags/Branches)
                Constraint::Min(0),            // Panel 3 (Tours/Collections/Bookmarks)
            ])
            .split(area);

        // Render search
        self.search.render(chunks[0], buf);

        // Render panel 1
        self.panel1.render(chunks[1], buf);

        // Render panel 2
        self.panel2.render(chunks[2], buf);

        // Render panel 3
        self.panel3.render(chunks[3], buf);
    }

    /// Handle an event.
    fn handle_event(&mut self, event: &Event) -> bool {
        self.search.handle_event(event)
            || self.panel1.handle_event(event)
            || self.panel2.handle_event(event)
            || self.panel3.handle_event(event)
    }
}

impl RightPane {
    /// Create a new right pane.
    fn new(db: &Database) -> Self {
        let mut pane = Self {
            steps: TabbedPanel::new_steps_info(db),
            details: DetailsPanel::new(),
            steps_data: Vec::new(),
            focused: RightPaneFocus::Steps,
            pager_total: 0,
            pager_current: 0,
            last_area: std::cell::Cell::new(Rect::default()),
            info_config: SectionConfig::new(4, 10),
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
    fn update_preview(&mut self) {
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
                        parts.get(1).and_then(|s| s.parse::<usize>().ok())
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
                self.update_preview();
            }
        }
    }

    /// Load a tour and its steps from the database.
    pub fn load_tour(&mut self, db: &Database, tour_name: &str) {
        if let Ok(Some(collection)) = db.get_collection_by_name(tour_name)
            && let Ok(bookmarks) = db.list_bookmarks_in_collection(&collection.id)
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
                            parts.get(1).and_then(|s| s.parse::<usize>().ok())
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
                self.update_preview();
            }
        }
    }

    /// Generate markdown for a bookmark.
    fn generate_bookmark_markdown(&self, bm: &Bookmark, res: Option<&Resolution>) -> String {
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
    fn render(&self, area: Rect, buf: &mut Buffer) {
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
    fn handle_event(&mut self, event: &Event) -> bool {
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
                | ratatui::crossterm::event::KeyCode::Char('h') => {
                    if self.focused == RightPaneFocus::Steps {
                        self.pager_current = self.pager_current.saturating_sub(1);
                        self.update_preview();
                        return true;
                    }
                }
                ratatui::crossterm::event::KeyCode::Right
                | ratatui::crossterm::event::KeyCode::Char('l') => {
                    if self.focused == RightPaneFocus::Steps {
                        if self.pager_current + 1 < self.pager_total {
                            self.pager_current += 1;
                            self.update_preview();
                        }
                        return true;
                    }
                }
                ratatui::crossterm::event::KeyCode::Down => {
                    if self.focused == RightPaneFocus::Steps {
                        self.focus_details();
                        return true;
                    }
                }
                ratatui::crossterm::event::KeyCode::Up => {
                    if self.focused == RightPaneFocus::Details {
                        self.focus_steps();
                        return true;
                    }
                }
                _ => {}
            }
        }

        false
    }

    /// Set focus state.
    fn set_focus(&mut self, focused: bool) {
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

    /// Get the currently active markdown panel for modification.
    pub fn get_markdown_mut(&mut self) -> Option<&mut MarkdownPanel> {
        for panel in &mut self.panels {
            if let TabContent::Markdown(p) = panel {
                return Some(p);
            }
        }
        None
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
    fn build_repo_items(db: &Database, registry: &rusqlite::Connection) -> Vec<PanelItem> {
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
                    PanelItem::new(repo.repo_name)
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

    /// Build tags and branches items.
    fn build_tags_branches_items(db: &Database) -> (Vec<PanelItem>, Vec<PanelItem>) {
        let tags = match db.list_all_tags() {
            Ok(tags) if !tags.is_empty() => tags
                .into_iter()
                .map(|tag| PanelItem::new(format!("#{tag}")).no_health().color(Color::Cyan))
                .collect(),
            Ok(_) => vec![PanelItem::new("No tags found").no_health().color(Color::DarkGray)],
            Err(e) => vec![PanelItem::new(format!("Error: {e}")).no_health().color(Color::Red)],
        };

        let branches = match db.list_all_branches() {
            Ok(branches) if !branches.is_empty() => branches
                .into_iter()
                .map(|branch| PanelItem::new(branch).health(HealthStatus::Branch))
                .collect(),
            Ok(_) => vec![PanelItem::new("No branches found").no_health().color(Color::DarkGray)],
            Err(e) => vec![PanelItem::new(format!("Error: {e}")).no_health().color(Color::Red)],
        };

        (tags, branches)
    }

    /// Build tours, collections, and bookmarks items.
    fn build_panel3_items(db: &Database) -> (Vec<PanelItem>, Vec<PanelItem>, Vec<PanelItem>) {
        let mut collections_items = Vec::new();
        let mut tours_items = Vec::new();

        if let Ok(collections) = db.list_collections() {
            for (c, count) in collections {
                let health = match c.health {
                    Some(h) => match h {
                        codemark_core::engine::bookmark::CollectionHealth::Active => HealthStatus::Healthy,
                        codemark_core::engine::bookmark::CollectionHealth::Drifted => HealthStatus::Warning,
                        codemark_core::engine::bookmark::CollectionHealth::Stale => HealthStatus::Error,
                    },
                    None => HealthStatus::Unknown,
                };

                let is_published = c.published_at.is_some();
                let item = PanelItem::new(c.name)
                    .secondary_text(c.created_branch.unwrap_or_else(|| "main".to_string()))
                    .metadata(format!("{count} steps"))
                    .health(health)
                    .published(is_published);

                collections_items.push(item.clone());
                if is_published {
                    tours_items.push(item);
                }
            }
        }

        let bookmarks = match db.list_bookmarks(&BookmarkFilter::default()) {
            Ok(bookmarks) => bookmarks
                .into_iter()
                .map(|bm| {
                    PanelItem::new(bm.file_path)
                        .secondary_text(format!("L{}", bm.query))
                        .metadata(bm.created_by.unwrap_or_default())
                        .user_data(bm.id)
                })
                .collect(),
            Err(_) => Vec::new(),
        };

        (tours_items, collections_items, bookmarks)
    }

    /// Create panel 1 with Repos/Accounts tabs.
    fn new_repos_accounts(db: &Database, registry: &rusqlite::Connection) -> Self {
        let repos_items = TabbedPanel::build_repo_items(db, registry);
        let mut repos_panel = Panel::new("").bordered(false);
        repos_panel = repos_panel.items(repos_items);

        let accounts = Panel::new("")
            .items(vec![
                PanelItem::new("GitHub").no_health(),
                PanelItem::new("GitLab").no_health(),
                PanelItem::new("Bitbucket").no_health(),
            ])
            .bordered(false);

        let repos_count = repos_panel.len().to_string();
        let tabs = TabSelection::new(vec![
            Tab::new("Repos").badge(repos_count),
            Tab::new("Accounts").badge("3"),
        ]);

        Self {
            tabs,
            panels: vec![TabContent::List(repos_panel), TabContent::List(accounts)],
            focused: false,
            last_area: std::cell::Cell::new(Rect::default()),
        }
    }

    /// Create panel 2 with Tags/Branches tabs.
    fn new_tags_branches(db: &Database) -> Self {
        let (tags_items, branches_items) = TabbedPanel::build_tags_branches_items(db);
        let tags_panel = Panel::new("").multi_select(true).bordered(false).items(tags_items);
        let branches_panel = Panel::new("").bordered(false).items(branches_items);

        let tabs = TabSelection::new(vec![
            Tab::new("Tags").badge(tags_panel.len().to_string()),
            Tab::new("Branches").badge(branches_panel.len().to_string()),
        ]);

        Self {
            tabs,
            panels: vec![TabContent::List(tags_panel), TabContent::List(branches_panel)],
            focused: false,
            last_area: std::cell::Cell::new(Rect::default()),
        }
    }

    /// Create panel 3 with Tours/Collections/Bookmarks tabs.
    fn new_tours_collections_bookmarks(db: &Database) -> Self {
        let (tours_items, collections_items, bookmarks_items) = TabbedPanel::build_panel3_items(db);
        let tours_panel = Panel::new("").bordered(false).items(tours_items);
        let collections_panel = Panel::new("").bordered(false).items(collections_items);
        let bookmarks_panel = Panel::new("").bordered(false).items(bookmarks_items);

        let tabs = TabSelection::new(vec![
            Tab::new("Tours").badge(tours_panel.len().to_string()),
            Tab::new("Collections").badge(collections_panel.len().to_string()),
            Tab::new("Bookmarks").badge(bookmarks_panel.len().to_string()),
        ]);

        Self {
            tabs,
            panels: vec![
                TabContent::List(tours_panel),
                TabContent::List(collections_panel),
                TabContent::List(bookmarks_panel),
            ],
            focused: false,
            last_area: std::cell::Cell::new(Rect::default()),
        }
    }

    /// Create steps/info/query tabs for right pane.
    fn new_steps_info(_db: &Database) -> Self {
        use crate::component::CodePreview;
        let fixture_path = "tests/fixtures/rust/api_client.rs";
        let code = std::fs::read_to_string(fixture_path).unwrap_or_else(|_| {
            "Error: Could not load fixture tests/fixtures/rust/api_client.rs".to_string()
        });

        let mut preview = CodePreview::new(code, "rs");
        preview.jump_to_line(49); // Jump to line 50 (0-indexed)

        let info = MarkdownPanel::new();

        let query_preview = CodePreview::new("(node) @cap", "scm");

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
        }
    }

    /// Get the last rendered area.
    pub fn last_area(&self) -> Rect {
        self.last_area.get()
    }

    /// Render the tabbed panel.
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.last_area.set(area);
        let tab_titles = self.tabs.render_as_titles(self.focused);

        // Render outer border for the entire panel area with inline tabs
        let border_style = if self.focused {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let block = ratatui::widgets::Block::bordered()
            .border_type(ratatui::widgets::BorderType::Rounded)
            .border_style(border_style)
            .title(tab_titles)
            .title_style(border_style)
            .title_alignment(ratatui::layout::Alignment::Left);

        let inner = block.inner(area);
        block.render(area, buf);

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
                    Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::DarkGray)
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
    fn handle_event(&mut self, event: &Event) -> bool {
        // Check for tab switching with [ and ]
        if let Event::Key(key) = event {
            match key.code {
                ratatui::crossterm::event::KeyCode::Char(']') => {
                    self.tabs.next();
                    return true;
                }
                ratatui::crossterm::event::KeyCode::Char('[') => {
                    self.tabs.previous();
                    return true;
                }
                _ => {}
            }
        }

        // Forward to active panel
        let active_index = self.tabs.selected_index();
        if let Some(panel) = self.panels.get_mut(active_index) {
            panel.handle_event(event)
        } else {
            false
        }
    }

    /// Set focus state.
    fn set_focus(&mut self, focused: bool) {
        self.focused = focused;
        for panel in &mut self.panels {
            panel.set_focus(focused);
        }
    }
}

impl DetailsPanel {
    /// Create a new details panel.
    fn new() -> Self {
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
    fn render(&self, area: Rect, buf: &mut Buffer) {
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
    fn set_focus(&mut self, focused: bool) {
        self.focused = focused;
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
