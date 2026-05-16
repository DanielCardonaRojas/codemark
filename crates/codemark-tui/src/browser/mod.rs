//! Browser layout for the Codemark TUI.
//!
//! This module provides the main browser layout with a left sidebar
//! containing search, repos, and tours, and a right main content area.

mod search;
mod tabs;

pub use search::SearchBar;
pub use tabs::{Tab, TabSelection};

use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Paragraph, Wrap, Widget},
};

use crate::component::{Component, HealthStatus, Panel, PanelItem, SyncDirection};
use crate::event::Event;
use crate::ui::KeyBinding;

/// The main browser layout.
///
/// Splits the screen vertically with a left sidebar (40%) and right main area (60%).
/// Each section has numbered tabs that can be cycled with `[` and `]`.
pub struct BrowserLayout {
    /// Left sidebar components
    left_pane: LeftPane,
    /// Right main content area
    right_pane: RightPane,
    /// Current focus area
    focus: FocusArea,
}

/// Right pane containing Steps and Tour Info sections.
struct RightPane {
    /// Steps panel showing tour steps
    steps: Panel,
    /// Tour info panel showing metadata
    tour_info: TourInfo,
    /// Currently focused section
    focused: RightPaneFocus,
}

/// Focus areas within the right pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RightPaneFocus {
    Steps,
    TourInfo,
}

/// Tour info component displaying tour metadata.
struct TourInfo {
    /// Tour ID
    id: String,
    /// Branch name
    branch: String,
    /// Author
    author: String,
    /// Step count
    step_count: usize,
    /// Description
    description: Option<String>,
    /// Whether the panel is focused
    focused: bool,
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

/// A tabbed panel component with multiple content panels.
struct TabbedPanel {
    /// Tab selection
    tabs: TabSelection,
    /// Content panels for each tab
    panels: Vec<Panel>,
    /// Currently focused
    focused: bool,
}

impl BrowserLayout {
    /// Create a new browser layout.
    pub fn new() -> Self {
        Self {
            left_pane: LeftPane::new(),
            right_pane: RightPane::new(),
            focus: FocusArea::Search,
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
                    0 => { // Tours
                        bindings.insert(0, KeyBinding::new("p", "Pull"));
                        bindings.insert(1, KeyBinding::new("P", "Push"));
                    }
                    1 => { // Collections
                        bindings.insert(0, KeyBinding::new("Enter", "Open"));
                    }
                    2 => { // Bookmarks
                        bindings.insert(0, KeyBinding::new("o", "Open"));
                        bindings.insert(1, KeyBinding::new("Enter", "Preview"));
                    }
                    _ => {}
                }
            }
            FocusArea::Main => {
                bindings.insert(0, KeyBinding::new("Enter", "Select Step"));
                bindings.insert(1, KeyBinding::new("o", "Open File"));
            }
        }

        bindings
    }

    /// Get current filters/metadata for the status bar.
    pub fn get_status_metadata(&self) -> Line {
        vec![
            Span::styled("Repo: ", Style::default().fg(Color::DarkGray)),
            Span::styled("codemark", Style::default().fg(Color::Cyan)),
            Span::styled(" | ", Style::default().fg(Color::Gray)),
            Span::styled("Branch: ", Style::default().fg(Color::DarkGray)),
            Span::styled("main", Style::default().fg(Color::Yellow)),
            Span::styled(" | ", Style::default().fg(Color::Gray)),
            Span::styled("#ui", Style::default().fg(Color::Magenta)),
        ].into()
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
                self.right_pane.steps.set_filter(query);
            }
            _ => {}
        }
    }

    /// Set the focus area.
    pub fn set_focus(&mut self, focus: FocusArea) {
        self.focus = focus;
        self.update_focus_state();
    }

    /// Cycle to the next focusable area.
    pub fn next_focus(&mut self) {
        self.focus = match self.focus {
            FocusArea::Search => FocusArea::Panel1,
            FocusArea::Panel1 => FocusArea::Panel2,
            FocusArea::Panel2 => FocusArea::Panel3,
            FocusArea::Panel3 => FocusArea::Main,
            FocusArea::Main => FocusArea::Search,
        };
        self.update_focus_state();
    }

    /// Cycle to the previous focusable area.
    pub fn previous_focus(&mut self) {
        self.focus = match self.focus {
            FocusArea::Search => FocusArea::Main,
            FocusArea::Main => FocusArea::Panel3,
            FocusArea::Panel3 => FocusArea::Panel2,
            FocusArea::Panel2 => FocusArea::Panel1,
            FocusArea::Panel1 => FocusArea::Search,
        };
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
        match self.focus {
            FocusArea::Search => self.left_pane.search.handle_event(event),
            FocusArea::Panel1 => self.left_pane.panel1.handle_event(event),
            FocusArea::Panel2 => self.left_pane.panel2.handle_event(event),
            FocusArea::Panel3 => self.left_pane.panel3.handle_event(event),
            FocusArea::Main => self.right_pane.handle_event(event),
        }
    }
}

impl LeftPane {
    /// Create a new left pane.
    fn new() -> Self {
        Self {
            search: SearchBar::new(),
            panel1: TabbedPanel::new_repos_accounts(),
            panel2: TabbedPanel::new_tags_branches(),
            panel3: TabbedPanel::new_tours_collections_bookmarks(),
            panel1_config: SectionConfig::new(4, 6),
            panel2_config: SectionConfig::new(4, 8),
        }
    }

    /// Render the left pane.
    fn render(&self, area: Rect, buf: &mut Buffer) {
        // Calculate heights based on focus
        let p1_height = if self.panel1.focused {
            self.panel1_config.max
        } else {
            self.panel1_config.min
        };

        let p2_height = if self.panel2.focused {
            self.panel2_config.max
        } else {
            self.panel2_config.min
        };

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

impl TabbedPanel {
    /// Get the currently active panel for modification.
    pub fn active_panel_mut(&mut self) -> Option<&mut Panel> {
        let active_index = self.tabs.selected_index();
        self.panels.get_mut(active_index)
    }

    /// Create panel 1 with Repos/Accounts tabs.
    fn new_repos_accounts() -> Self {
        let repos = Panel::new("")
            .items(vec![
                PanelItem::new("dcardona/fix_authentication"),
                PanelItem::new("dcardona/codemark"),
                PanelItem::new("anthropics/claude-code"),
            ])
            .bordered(false);

        let accounts = Panel::new("")
            .items(vec![
                PanelItem::new("GitHub"),
                PanelItem::new("GitLab"),
                PanelItem::new("Bitbucket"),
            ])
            .bordered(false);

        let tabs = TabSelection::new(vec![
            Tab::new("Repos").badge("3"),
            Tab::new("Accounts").badge("3"),
        ]);

        Self {
            tabs,
            panels: vec![repos, accounts],
            focused: false,
        }
    }

    /// Create panel 2 with Tags/Branches tabs.
    fn new_tags_branches() -> Self {
        let tags = Panel::new("")
            .items(vec![
                PanelItem::new("#ui").no_health().color(Color::Cyan),
                PanelItem::new("#backend").no_health().color(Color::Magenta),
                PanelItem::new("#authentication").no_health().color(Color::Yellow),
                PanelItem::new("#mylibrary").no_health().color(Color::Green),
            ])
            .bordered(false);

        let branches = Panel::new("")
            .items(vec![
                PanelItem::new("main"),
                PanelItem::new("develop"),
                PanelItem::new("feature/auth-tui"),
                PanelItem::new("fix/lifetime-error"),
            ])
            .bordered(false);

        let tabs = TabSelection::new(vec![
            Tab::new("Tags").badge("4"),
            Tab::new("Branches").badge("4"),
        ]);

        Self {
            tabs,
            panels: vec![tags, branches],
            focused: false,
        }
    }

    /// Create panel 3 with Tours/Collections/Bookmarks tabs.
    fn new_tours_collections_bookmarks() -> Self {
        let tours = Panel::new("")
            .items(vec![
                PanelItem::new("Authentication flow").health(HealthStatus::Healthy).sync_direction(Some(SyncDirection::Push)),
                PanelItem::new("Onboarding").health(HealthStatus::Healthy).sync_direction(Some(SyncDirection::Pull)),
                PanelItem::new("API tutorial").health(HealthStatus::Warning).sync_direction(None), // No indicator when synced
                PanelItem::new("Rust patterns").health(HealthStatus::Error).sync_direction(Some(SyncDirection::Push)),
                PanelItem::new("TCA basics").health(HealthStatus::Unknown).sync_direction(Some(SyncDirection::Pull)),
            ])
            .bordered(false);

        let collections = Panel::new("")
            .items(vec![
                PanelItem::new("My Collection").health(HealthStatus::Healthy).checkmark(true),
                PanelItem::new("Shared Tours").health(HealthStatus::Healthy).checkmark(true),
                PanelItem::new("Favorites").health(HealthStatus::Warning),
            ])
            .bordered(false);

        let bookmarks = Panel::new("")
            .items(vec![
                PanelItem::new("Login Component").health(HealthStatus::Healthy),
                PanelItem::new("API Handler").health(HealthStatus::Error),
                PanelItem::new("Auth Middleware").health(HealthStatus::Unknown),
            ])
            .bordered(false);

        let tabs = TabSelection::new(vec![
            Tab::new("Tours").badge("5"),
            Tab::new("Collections").badge("3"),
            Tab::new("Bookmarks").badge("3"),
        ]);

        Self {
            tabs,
            panels: vec![tours, collections, bookmarks],
            focused: false,
        }
    }

    /// Render the tabbed panel.
    fn render(&self, area: Rect, buf: &mut Buffer) {
        // Build the tab titles as a single line for the border
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
        if let Some(panel) = self.panels.get(active_index) {
            if !panel.is_empty() {
                let current = panel.selected_index().map_or(0, |i| i + 1);
                let total = panel.len();
                let indicator = format!(" {current} of {total} ");
                let indicator_width = indicator.len() as u16;

                // Position on bottom border, aligned right (shift left by 1 to avoid corner)
                let x = area.right().saturating_sub(indicator_width + 1);
                let y = area.bottom() - 1;

                if x > area.left() {
                    // Render the indicator by modifying the border cells
                    for (i, c) in indicator.chars().enumerate() {
                        let cx = x + i as u16;
                        if let Some(cell) = buf.cell_mut((cx, y)) {
                            cell.set_char(c);
                            cell.set_fg(Color::DarkGray);
                        }
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

impl RightPane {
    /// Create a new right pane.
    fn new() -> Self {
        Self {
            steps: Panel::new("Steps")
                .items(vec![
                    PanelItem::new("Step 1:").secondary_text("Login button"),
                    PanelItem::new("Step 2:").secondary_text("Network call"),
                    PanelItem::new("Step 3:").secondary_text("Validation"),
                ])
                .bordered(true),
            tour_info: TourInfo::new(),
            focused: RightPaneFocus::Steps,
        }
    }

    /// Render the right pane.
    fn render(&self, area: Rect, buf: &mut Buffer) {
        // Split vertically: steps (flex), tour info (fixed height)
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(0),
                Constraint::Length(8),
            ])
            .split(area);

        // Render steps
        self.steps.render(chunks[0], buf);

        // Render tour info
        self.tour_info.render(chunks[1], buf);
    }

    /// Handle an event.
    fn handle_event(&mut self, event: &Event) -> bool {
        // Forward to focused component first
        let handled = match self.focused {
            RightPaneFocus::Steps => self.steps.handle_event(event),
            RightPaneFocus::TourInfo => false, // Tour info doesn't handle events
        };

        if handled {
            return true;
        }

        // Handle tab switching within right pane
        if let Event::Key(key) = event {
            match key.code {
                ratatui::crossterm::event::KeyCode::Down => {
                    if self.focused == RightPaneFocus::Steps {
                        self.focused = RightPaneFocus::TourInfo;
                        self.steps.set_focus(false);
                        self.tour_info.set_focus(true);
                        return true;
                    }
                }
                ratatui::crossterm::event::KeyCode::Up => {
                    if self.focused == RightPaneFocus::TourInfo {
                        self.focused = RightPaneFocus::Steps;
                        self.tour_info.set_focus(false);
                        self.steps.set_focus(true);
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
            RightPaneFocus::TourInfo => self.tour_info.set_focus(focused),
        }
    }

    /// Focus the steps section.
    pub fn focus_steps(&mut self) {
        self.focused = RightPaneFocus::Steps;
        self.steps.set_focus(true);
        self.tour_info.set_focus(false);
    }

    /// Focus the tour info section.
    pub fn focus_tour_info(&mut self) {
        self.focused = RightPaneFocus::TourInfo;
        self.tour_info.set_focus(true);
        self.steps.set_focus(false);
    }
}

impl TourInfo {
    /// Create a new tour info panel.
    fn new() -> Self {
        Self {
            id: "a2nsg-k2est".to_string(),
            branch: "main".to_string(),
            author: "Claude Code".to_string(),
            step_count: 5,
            description: Some("Authentication flow tour".to_string()),
            focused: false,
        }
    }

    /// Set tour data.
    pub fn set_tour(&mut self, id: impl Into<String>, branch: impl Into<String>, author: impl Into<String>, step_count: usize) {
        self.id = id.into();
        self.branch = branch.into();
        self.author = author.into();
        self.step_count = step_count;
    }

    /// Set description.
    pub fn set_description(&mut self, description: Option<String>) {
        self.description = description;
    }

    /// Render the tour info panel.
    fn render(&self, area: Rect, buf: &mut Buffer) {
        // Render border
        let border_style = if self.focused {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let block = ratatui::widgets::Block::bordered()
            .border_type(ratatui::widgets::BorderType::Rounded)
            .title("Tour info")
            .title_style(Style::default().bold())
            .border_style(border_style);

        let inner = block.inner(area);
        block.render(area, buf);

        // Build info content
        let info_lines = vec![
            Line::from(vec![
                Span::styled("Branch: ", Style::default().fg(Color::DarkGray)),
                Span::styled(&self.branch, Style::default().fg(Color::Cyan)),
            ]),
            Line::from(vec![
                Span::styled("Author: ", Style::default().fg(Color::DarkGray)),
                Span::styled(&self.author, Style::default().fg(Color::Cyan)),
            ]),
            Line::from(vec![
                Span::styled("Steps: ", Style::default().fg(Color::DarkGray)),
                Span::styled(format!("{}", self.step_count), Style::default().fg(Color::Cyan)),
            ]),
            Line::from(vec![
                Span::styled("Id: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    self.id.clone(),
                    Style::default().fg(Color::Yellow).dim(),
                ),
            ]),
        ];

        let paragraph = Paragraph::new(info_lines)
            .wrap(Wrap { trim: false })
            .alignment(Alignment::Left);

        paragraph.render(inner, buf);
    }

    /// Set focus state.
    fn set_focus(&mut self, focused: bool) {
        self.focused = focused;
    }
}

impl Default for BrowserLayout {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for BrowserLayout {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.render(area, buf);
    }

    fn handle_event(&mut self, event: &Event) -> bool {
        // Handle focus cycling and number shortcuts
        if let Event::Key(key) = event {
            match key.code {
                ratatui::crossterm::event::KeyCode::Tab => {
                    self.next_focus();
                    return true;
                }
                ratatui::crossterm::event::KeyCode::BackTab => {
                    self.previous_focus();
                    return true;
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
                    self.right_pane.focus_tour_info();
                    self.update_focus_state();
                    return true;
                }
                _ => {}
            }
        }

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
