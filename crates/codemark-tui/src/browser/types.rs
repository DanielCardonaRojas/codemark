use crate::component::{CodePreview, Component, MarkdownPanel, Panel};
use crate::event::Event;
use codemark_core::engine::bookmark::{Bookmark, Resolution};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

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

/// Result of a heal operation that can be displayed as a notification.
#[derive(Debug, Clone)]
pub struct HealNotification {
    /// Message to display
    pub message: String,
    /// Whether the operation was successful
    pub success: bool,
}

/// Target for healing operation.
#[derive(Debug, Clone)]
pub enum HealTarget {
    /// Heal a single bookmark by ID
    Bookmark(String),
    /// Heal all bookmarks in a collection by ID
    Collection(String),
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
    /// Bottom filter bar is focused
    Filter,
}

/// Configuration for a sidebar section's height.
#[derive(Debug, Clone, Copy)]
pub struct SectionConfig {
    /// Minimum height when unfocused
    pub min: u16,
    /// Maximum height when focused
    pub max: u16,
}

impl SectionConfig {
    pub fn new(min: u16, max: u16) -> Self {
        Self { min, max }
    }
}

/// Content for a tabbed panel.
pub enum TabContent {
    /// A list of items
    List(Panel),
    /// A code preview
    Preview(CodePreview),
    /// A markdown panel
    Markdown(MarkdownPanel),
}

impl TabContent {
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        match self {
            TabContent::List(p) => p.render(area, buf),
            TabContent::Preview(p) => p.render(area, buf),
            TabContent::Markdown(p) => p.render(area, buf),
        }
    }

    pub fn handle_event(&mut self, event: &Event) -> bool {
        match self {
            TabContent::List(p) => p.handle_event(event),
            TabContent::Preview(p) => p.handle_event(event),
            TabContent::Markdown(p) => p.handle_event(event),
        }
    }

    pub fn set_focus(&mut self, focused: bool) {
        match self {
            TabContent::List(p) => p.set_focus(focused),
            TabContent::Preview(p) => p.set_focus(focused),
            TabContent::Markdown(p) => p.set_focus(focused),
        }
    }

    /// Check for selection changes (for live preview).
    pub fn take_selection_change(&mut self) -> Option<String> {
        match self {
            TabContent::List(p) => p.take_selection_change(),
            _ => None,
        }
    }
}

/// Data for a single step in a tour.
pub struct StepData {
    /// Path to the file for this step
    pub file_path: String,
    /// Line number to jump to (0-indexed)
    pub line_number: usize,
    /// Optional end line number for range highlighting (0-indexed, inclusive)
    pub line_end: Option<usize>,
    /// Real bookmark data
    pub bookmark: Bookmark,
    /// Resolution data if available
    pub resolution: Option<Resolution>,
}

/// Escape special markdown characters.
pub fn escape_markdown(text: &str) -> String {
    text.replace('_', "\\_")
        .replace('*', "\\*")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('#', "\\#")
}
