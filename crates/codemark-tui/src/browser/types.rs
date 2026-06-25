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

/// A modal confirmation dialog displayed over the browser layout.
///
/// While a dialog is active it captures all key input. The user moves between
/// the Cancel and Confirm buttons (←/→/Tab) and activates the focused one with
/// Enter; Esc cancels outright. Confirming runs the pending [`DialogAction`].
#[derive(Debug, Clone)]
pub struct ConfirmDialog {
    /// Title rendered in the dialog border.
    pub title: String,
    /// Message body explaining the consequence of confirming.
    pub message: String,
    /// The action performed when the user confirms.
    pub action: DialogAction,
    /// Which button currently has focus.
    pub selected: DialogButton,
}

impl ConfirmDialog {
    /// Create a dialog with the Cancel button focused by default, a safe
    /// default for destructive actions.
    pub fn new(title: impl Into<String>, message: impl Into<String>, action: DialogAction) -> Self {
        Self {
            title: title.into(),
            message: message.into(),
            action,
            selected: DialogButton::Cancel,
        }
    }
}

/// The two buttons of a [`ConfirmDialog`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogButton {
    /// Runs the pending action.
    Confirm,
    /// Dismisses the dialog without side effects.
    Cancel,
}

impl DialogButton {
    /// Switch focus to the other button.
    pub fn toggle(self) -> Self {
        match self {
            Self::Confirm => Self::Cancel,
            Self::Cancel => Self::Confirm,
        }
    }
}

/// An action awaiting user confirmation in a [`ConfirmDialog`].
#[derive(Debug, Clone)]
pub enum DialogAction {
    /// Delete the bookmark with the given id.
    DeleteBookmark(String),
    /// Delete the collection with the given id.
    ///
    /// Collection names are not uniquely constrained, so deletion is keyed on
    /// the stable id rather than the display name.
    DeleteCollection(String),
}

/// Target for healing operation.
#[derive(Debug, Clone)]
pub enum HealTarget {
    /// Heal a single bookmark by ID
    Bookmark(String),
    /// Heal all bookmarks in a collection by ID
    Collection(String),
}

/// A panel item that is currently showing an animated spinner.
#[derive(Debug, Clone)]
pub struct SpinningItem {
    /// The `user_data` value that identifies the item in its panel
    pub user_data_key: String,
    /// Index of the panel tab containing the item
    pub tab_index: usize,
    /// Tick count when this spinner was started (for minimum duration)
    pub start_tick: usize,
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

impl FocusArea {
    /// Check if this focus area can be resized with +/- keys.
    /// Left side panels and the main preview pane support resizing.
    pub fn is_resizable(self) -> bool {
        matches!(self, FocusArea::Panel1 | FocusArea::Panel2 | FocusArea::Panel3 | FocusArea::Main)
    }
}

/// Size mode for the left pane, similar to lazy git's panel sizing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LeftPaneSize {
    /// Regular size (40% width)
    #[default]
    Regular,
    /// Half width (50%), only focused left panel shown (right pane still visible at 50%)
    Half,
    /// Full window width (100%)
    Full,
}

impl LeftPaneSize {
    /// Cycle to the next larger size.
    pub fn increase(self) -> Self {
        match self {
            Self::Regular => Self::Half,
            Self::Half => Self::Full,
            Self::Full => Self::Regular, // Cycle back to regular
        }
    }

    /// Cycle to the next smaller size.
    pub fn decrease(self) -> Self {
        match self {
            Self::Regular => Self::Half, // Wrap to middle size
            Self::Half => Self::Regular,
            Self::Full => Self::Half,
        }
    }

    /// Get the width percentage for the left pane.
    pub fn left_width_percent(self) -> u16 {
        match self {
            Self::Regular => 40,
            Self::Half => 50,
            Self::Full => 100,
        }
    }

    /// Get the width percentage for the right pane.
    pub fn right_width_percent(self) -> Option<u16> {
        match self {
            Self::Regular => Some(60),
            Self::Half => Some(50), // Right pane at 50% in half mode
            Self::Full => None,     // Right pane is hidden in full mode
        }
    }
}

/// Size mode for the right pane (preview), allowing expansion to full screen.
///
/// Cycles: Regular → Full → PreviewOnly → Regular. Both expanded states take the
/// full window width (left pane hidden) and always keep the pagination widget
/// visible; they differ only in whether the details pane is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RightPaneSize {
    /// Regular size (shares space with left pane based on left_pane_size)
    #[default]
    Regular,
    /// Full window width (100%) - left pane hidden, preview + pagination + details
    Full,
    /// Full window width (100%) - left pane hidden, preview + pagination only
    /// (details pane hidden)
    PreviewOnly,
}

impl RightPaneSize {
    /// Cycle to the next layout (Regular → Full → PreviewOnly → Regular).
    pub fn increase(self) -> Self {
        match self {
            Self::Regular => Self::Full,
            Self::Full => Self::PreviewOnly,
            Self::PreviewOnly => Self::Regular,
        }
    }

    /// Cycle to the previous layout (Regular → PreviewOnly → Full → Regular).
    pub fn decrease(self) -> Self {
        match self {
            Self::Regular => Self::PreviewOnly,
            Self::PreviewOnly => Self::Full,
            Self::Full => Self::Regular,
        }
    }

    /// Check if the right pane should override the default layout.
    /// When true, the right pane takes full width regardless of left pane size.
    pub fn is_fullscreen(self) -> bool {
        matches!(self, Self::Full | Self::PreviewOnly)
    }

    /// Check if the details pane should be hidden (preview + pagination only).
    pub fn hides_details(self) -> bool {
        matches!(self, Self::PreviewOnly)
    }
}

/// Size mode for the details pane, allowing expansion like the preview pane.
///
/// Cycles: Regular → Half (full right-pane height) → Full (full screen).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DetailsPaneSize {
    /// Regular size (shares vertical space with the steps panel)
    #[default]
    Regular,
    /// Full right-pane height (steps/pager hidden, left pane still visible)
    Half,
    /// Full screen (left pane hidden, steps/pager hidden)
    Full,
}

impl DetailsPaneSize {
    /// Cycle to the next larger size.
    pub fn increase(self) -> Self {
        match self {
            Self::Regular => Self::Half,
            Self::Half => Self::Full,
            Self::Full => Self::Regular,
        }
    }

    /// Cycle to the next smaller size.
    pub fn decrease(self) -> Self {
        match self {
            Self::Regular => Self::Half,
            Self::Half => Self::Regular,
            Self::Full => Self::Half,
        }
    }

    /// Check if the details pane should take the full screen.
    pub fn is_fullscreen(self) -> bool {
        matches!(self, Self::Full)
    }

    /// Check if the details pane should take the full right-pane height.
    pub fn is_expanded(self) -> bool {
        matches!(self, Self::Half | Self::Full)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_left_pane_size_increase() {
        assert_eq!(LeftPaneSize::Regular.increase(), LeftPaneSize::Half);
        assert_eq!(LeftPaneSize::Half.increase(), LeftPaneSize::Full);
        assert_eq!(LeftPaneSize::Full.increase(), LeftPaneSize::Regular);
    }

    #[test]
    fn test_left_pane_size_decrease() {
        assert_eq!(LeftPaneSize::Regular.decrease(), LeftPaneSize::Half);
        assert_eq!(LeftPaneSize::Half.decrease(), LeftPaneSize::Regular);
        assert_eq!(LeftPaneSize::Full.decrease(), LeftPaneSize::Half);
    }

    #[test]
    fn test_left_pane_size_widths() {
        assert_eq!(LeftPaneSize::Regular.left_width_percent(), 40);
        assert_eq!(LeftPaneSize::Half.left_width_percent(), 50);
        assert_eq!(LeftPaneSize::Full.left_width_percent(), 100);

        assert_eq!(LeftPaneSize::Regular.right_width_percent(), Some(60));
        assert_eq!(LeftPaneSize::Half.right_width_percent(), Some(50));
        assert_eq!(LeftPaneSize::Full.right_width_percent(), None);
    }

    #[test]
    fn test_focus_area_is_resizable() {
        assert!(!FocusArea::Search.is_resizable());
        assert!(FocusArea::Panel1.is_resizable());
        assert!(FocusArea::Panel2.is_resizable());
        assert!(FocusArea::Panel3.is_resizable());
        assert!(FocusArea::Main.is_resizable());
        assert!(!FocusArea::Filter.is_resizable());
    }

    #[test]
    fn test_right_pane_size_increase() {
        assert_eq!(RightPaneSize::Regular.increase(), RightPaneSize::Full);
        assert_eq!(RightPaneSize::Full.increase(), RightPaneSize::PreviewOnly);
        assert_eq!(RightPaneSize::PreviewOnly.increase(), RightPaneSize::Regular);
    }

    #[test]
    fn test_right_pane_size_decrease() {
        assert_eq!(RightPaneSize::Regular.decrease(), RightPaneSize::PreviewOnly);
        assert_eq!(RightPaneSize::PreviewOnly.decrease(), RightPaneSize::Full);
        assert_eq!(RightPaneSize::Full.decrease(), RightPaneSize::Regular);
    }

    #[test]
    fn test_right_pane_size_is_fullscreen() {
        assert!(!RightPaneSize::Regular.is_fullscreen());
        assert!(RightPaneSize::Full.is_fullscreen());
        assert!(RightPaneSize::PreviewOnly.is_fullscreen());
    }

    #[test]
    fn test_right_pane_size_hides_details() {
        assert!(!RightPaneSize::Regular.hides_details());
        assert!(!RightPaneSize::Full.hides_details());
        assert!(RightPaneSize::PreviewOnly.hides_details());
    }

    #[test]
    fn test_details_pane_size_increase() {
        assert_eq!(DetailsPaneSize::Regular.increase(), DetailsPaneSize::Half);
        assert_eq!(DetailsPaneSize::Half.increase(), DetailsPaneSize::Full);
        assert_eq!(DetailsPaneSize::Full.increase(), DetailsPaneSize::Regular);
    }

    #[test]
    fn test_details_pane_size_decrease() {
        assert_eq!(DetailsPaneSize::Regular.decrease(), DetailsPaneSize::Half);
        assert_eq!(DetailsPaneSize::Half.decrease(), DetailsPaneSize::Regular);
        assert_eq!(DetailsPaneSize::Full.decrease(), DetailsPaneSize::Half);
    }

    #[test]
    fn test_details_pane_size_is_fullscreen() {
        assert!(!DetailsPaneSize::Regular.is_fullscreen());
        assert!(!DetailsPaneSize::Half.is_fullscreen());
        assert!(DetailsPaneSize::Full.is_fullscreen());
    }

    #[test]
    fn test_details_pane_size_is_expanded() {
        assert!(!DetailsPaneSize::Regular.is_expanded());
        assert!(DetailsPaneSize::Half.is_expanded());
        assert!(DetailsPaneSize::Full.is_expanded());
    }
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// All resolutions for this bookmark (for showing full resolution history)
    pub resolutions: Vec<Resolution>,
}

/// A fully-computed bookmark preview, produced off the UI thread.
///
/// All expensive work (live resolution, file read, markdown rendering) is done
/// when this is built, so applying it to the [`RightPane`](super::RightPane) is
/// just field assignment with no I/O. Sent back to the UI thread via
/// [`Event::PreviewReady`](crate::event::Event::PreviewReady).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewPayload {
    /// Bookmark this preview belongs to.
    pub bookmark_id: String,
    /// Resolved step (file path + line range + bookmark metadata).
    pub step: StepData,
    /// File contents for the code preview.
    pub code: String,
    /// File extension (for syntax highlighting).
    pub extension: String,
    /// Relative path shown in the preview header.
    pub relative_path: String,
    /// Rendered markdown for the Info tab.
    pub info_markdown: String,
    /// Rendered markdown for the Details pane.
    pub details_markdown: String,
    /// Rendered markdown for the Comments pane.
    pub comments_markdown: String,
    /// The bookmark's query (shown in the Query tab).
    pub query: String,
}

/// Escape special markdown characters.
pub fn escape_markdown(text: &str) -> String {
    text.replace('_', "\\_")
        .replace('*', "\\*")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('#', "\\#")
}
