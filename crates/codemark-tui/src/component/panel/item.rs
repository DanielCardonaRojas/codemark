//! An individual item displayed within a [`Panel`](super::Panel), plus its
//! builder API and line-rendering logic.

use ratatui::{
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
};

use codemark_core::sort::Sortable;

use super::health::HealthStatus;
use super::path::shorten_path;

/// An item in a panel.
#[derive(Debug, Clone)]
pub struct PanelItem {
    /// The primary text to display
    text: String,
    /// Optional emphasized text rendered bold (shown after primary text)
    emphasis_text: Option<String>,
    /// Optional secondary text (shown in a different color)
    secondary_text: Option<String>,
    /// Optional metadata associated with this item
    metadata: Option<String>,
    /// Optional icon (NERD font symbol)
    icon: Option<String>,
    /// Render the icon in the normal foreground color instead of the accent color.
    plain_icon: bool,
    /// Health status indicator
    health: Option<HealthStatus>,
    /// Primary text color
    text_color: Option<Color>,
    /// Whether the item has a trailing checkmark (e.g., published tour)
    checkmark: bool,
    /// Sync direction indicator for tours (push/pull)
    sync_direction: Option<SyncDirection>,
    /// Whether this item is currently active (e.g., active workspace)
    active: bool,
    /// Whether the item is published to a server (shows cloud upload icon)
    published: bool,
    /// Optional spinner text shown at the very end of the item
    spinner_text: Option<String>,
    /// When true, the primary `text` is treated as a file path that is
    /// compressed at render time to fit the available width of the pane.
    compress_path: bool,
    /// Optional creation timestamp (ISO-8601), used as the key for date-based
    /// [`SortMethod`](codemark_core::sort::SortMethod) ordering. ISO strings
    /// sort chronologically as plain text.
    created_at: Option<String>,
    /// Optional hidden user data (e.g., database ID)
    pub user_data: Option<String>,
    /// Optional repository display name this item belongs to (multi-repo view).
    repo_name: Option<String>,
    /// Optional repository root path key this item belongs to (multi-repo view).
    repo_root: Option<String>,
}

/// Sync direction indicator for tours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncDirection {
    /// Tour can be pushed up (local changes need to be published)
    Push,
    /// Tour can be pulled down (remote updates available)
    Pull,
    /// Tour is in sync
    Synced,
}

impl PanelItem {
    /// Create a new panel item.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            emphasis_text: None,
            secondary_text: None,
            metadata: None,
            icon: None,
            plain_icon: false,
            health: Some(HealthStatus::Unknown),
            text_color: None,
            checkmark: false,
            sync_direction: None,
            active: false,
            published: false,
            spinner_text: None,
            compress_path: false,
            created_at: None,
            user_data: None,
            repo_name: None,
            repo_root: None,
        }
    }

    /// Set the creation timestamp used for date-based sorting (see
    /// [`SortMethod`](codemark_core::sort::SortMethod)).
    pub fn created_at(mut self, created_at: impl Into<String>) -> Self {
        self.created_at = Some(created_at.into());
        self
    }

    /// Set the icon.
    pub fn icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Render the icon in the normal foreground color instead of the accent color.
    pub fn plain_icon(mut self) -> Self {
        self.plain_icon = true;
        self
    }

    /// Set the emphasized text (rendered bold, after primary text).
    pub fn emphasis(mut self, text: impl Into<String>) -> Self {
        self.emphasis_text = Some(text.into());
        self
    }

    /// Set hidden user data.
    pub fn user_data(mut self, data: impl Into<String>) -> Self {
        self.user_data = Some(data.into());
        self
    }

    /// Set whether this item is active.
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// Set whether the item is published.
    pub fn published(mut self, published: bool) -> Self {
        self.published = published;
        self
    }

    /// Get the item text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Get the secondary text value.
    pub fn get_secondary_text(&self) -> Option<&str> {
        self.secondary_text.as_deref()
    }

    /// Tag this item with the repository it belongs to (display name + root path key).
    pub fn repo(mut self, name: impl Into<String>, root: impl Into<String>) -> Self {
        self.repo_name = Some(name.into());
        self.repo_root = Some(root.into());
        self
    }

    /// Get the repository display name this item belongs to, if any.
    pub fn repo_name(&self) -> Option<&str> {
        self.repo_name.as_deref()
    }

    /// Get the repository root path key this item belongs to, if any.
    pub fn repo_root(&self) -> Option<&str> {
        self.repo_root.as_deref()
    }

    /// Whether this item is currently active (selected/activated).
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Set the secondary text.
    pub fn secondary_text(mut self, text: impl Into<String>) -> Self {
        self.secondary_text = Some(text.into());
        self
    }

    /// Update the secondary text in place.
    pub fn set_secondary_text(&mut self, text: impl Into<String>) {
        self.secondary_text = Some(text.into());
    }

    /// Set the active flag in place.
    pub(super) fn set_active(&mut self, active: bool) {
        self.active = active;
    }

    /// Update the spinner text in place. `None` clears it.
    pub(super) fn set_spinner_text(&mut self, text: Option<String>) {
        self.spinner_text = text;
    }

    /// Update the health status in place. `None` hides the indicator.
    pub(super) fn set_health(&mut self, health: Option<HealthStatus>) {
        self.health = health;
    }

    /// Whether this item matches the (already lowercased) filter `query`,
    /// testing the primary text and the emphasis text.
    pub(super) fn matches_query(&self, query: &str) -> bool {
        self.text.to_lowercase().contains(query)
            || self.emphasis_text.as_ref().is_some_and(|e| e.to_lowercase().contains(query))
    }

    /// Set the metadata.
    pub fn metadata(mut self, metadata: impl Into<String>) -> Self {
        self.metadata = Some(metadata.into());
        self
    }

    /// Set the health status.
    pub fn health(mut self, health: HealthStatus) -> Self {
        self.health = Some(health);
        self
    }

    /// Set the health status to None (hide the indicator).
    pub fn no_health(mut self) -> Self {
        self.health = None;
        self
    }

    /// Set the primary text color.
    pub fn color(mut self, color: Color) -> Self {
        self.text_color = Some(color);
        self
    }

    /// Set whether to show a trailing checkmark (e.g., for published tours).
    pub fn checkmark(mut self, checkmark: bool) -> Self {
        self.checkmark = checkmark;
        self
    }

    /// Set the sync direction indicator for tours.
    pub fn sync_direction(mut self, direction: Option<SyncDirection>) -> Self {
        self.sync_direction = direction;
        self
    }

    /// Mark the primary text as a file path that should be compressed at render
    /// time to fit the available width of the pane. The full, untruncated path
    /// is kept in `text` so the displayed length can grow or shrink as the pane
    /// is resized (e.g. when cycling through left-pane layouts).
    pub fn compressible_path(mut self) -> Self {
        self.compress_path = true;
        self
    }

    /// Total display width consumed by the spans rendered *after* the primary
    /// text. Used to compute how much horizontal space a compressible path may
    /// occupy. Each optional segment is preceded by a single space separator,
    /// mirroring the layout in [`Self::to_line`].
    fn trailing_width(&self) -> usize {
        use unicode_width::UnicodeWidthStr;

        let mut width = 0;

        if let Some(emphasis) = &self.emphasis_text {
            width += 1 + emphasis.width();
        }
        if let Some(secondary) = &self.secondary_text {
            width += 1 + secondary.width();
        }
        if let Some(metadata) = &self.metadata {
            width += 1 + metadata.width();
        }
        if matches!(self.sync_direction, Some(SyncDirection::Push) | Some(SyncDirection::Pull)) {
            width += 2; // separator + arrow
        }
        if self.checkmark || self.active {
            width += 2; // separator + check
        }
        if self.published {
            width += 2; // separator + cloud icon
        }
        if let Some(spinner) = &self.spinner_text {
            width += 1 + spinner.width();
        }

        width
    }

    /// Render this item as a Line, compressing a path-style primary text to fit
    /// `available_width` (the inner content width of the pane) when applicable.
    pub(super) fn to_line(&self, selected: bool, focused: bool, available_width: u16) -> Line<'_> {
        use unicode_width::UnicodeWidthStr;

        let mut spans = Vec::new();

        // Add padding prefix for alignment
        spans.push(Span::raw("  "));

        // Add health status indicator if present
        if let Some(health) = self.health {
            spans.push(Span::styled(health.symbol(), Style::default().fg(health.color())));
            spans.push(Span::raw(" "));
        }

        // Add icon if present
        if let Some(icon) = &self.icon
            && !icon.is_empty()
        {
            let icon_style = if self.plain_icon {
                Style::default()
            } else {
                Style::default().fg(crate::theme::palette().accent)
            };
            spans.push(Span::styled(icon, icon_style));
            spans.push(Span::raw(" "));
        }

        // Add primary text
        let primary_style = if let Some(color) = self.text_color {
            Style::default().fg(color)
        } else {
            Style::default()
        };
        if self.compress_path {
            // Reserve the width consumed by the leading spans (padding, health,
            // icon) and the trailing spans (emphasis, secondary, metadata, …),
            // then give the path whatever width remains in the pane.
            let leading: usize = spans.iter().map(|s| s.content.width()).sum();
            let budget = (available_width as usize)
                .saturating_sub(leading)
                .saturating_sub(self.trailing_width());
            let short = shorten_path(&self.text, budget);
            spans.push(Span::styled(short, primary_style));
        } else {
            spans.push(Span::styled(&self.text, primary_style));
        }

        // Trailing spans begin here. Any span added below must be mirrored in
        // `trailing_width()` so a compressible path reserves the right amount of
        // space; `test_compress_path_fits_with_all_trailing_segments` guards
        // against drift between the two.
        //
        // Add emphasized text if present (bold)
        if let Some(emphasis) = &self.emphasis_text {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(emphasis, Style::default().add_modifier(Modifier::BOLD)));
        }

        // Add secondary text if present
        if let Some(secondary) = &self.secondary_text {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(secondary, Style::default().fg(crate::theme::palette().dim)));
        }

        // Add metadata if present
        if let Some(metadata) = &self.metadata {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(metadata, Style::default().fg(crate::theme::palette().accent)));
        }

        // Add sync direction arrow for tours (omit if synced)
        if let Some(direction) = &self.sync_direction {
            match direction {
                SyncDirection::Synced => {
                    // Don't show anything when synced
                }
                _ => {
                    spans.push(Span::raw(" "));
                    let (arrow, color) = match direction {
                        SyncDirection::Push => ("↑", crate::theme::palette().accent),
                        SyncDirection::Pull => ("↓", crate::theme::palette().warning),
                        SyncDirection::Synced => unreachable!(),
                    };
                    spans.push(Span::styled(arrow, Style::default().fg(color)));
                }
            }
        }
        // Add trailing checkmark if enabled or active
        if self.checkmark || self.active {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                "✓",
                Style::default().fg(crate::theme::palette().success).add_modifier(Modifier::BOLD),
            ));
        }

        // Add cloud upload icon if published to server (pushed)
        if self.published {
            spans.push(Span::raw(" "));
            spans.push(Span::styled("☁", Style::default().fg(crate::theme::palette().accent)));
        }

        // Add spinner at the very end
        if let Some(spinner) = &self.spinner_text {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(spinner, Style::default().fg(crate::theme::palette().warning)));
        }

        let mut line = Line::from(spans);
        if selected && focused {
            line = line.bold();
        }
        line
    }
}

impl Sortable for PanelItem {
    /// Order by the emphasized text (a bookmark's symbol identifier) when
    /// present, otherwise the primary text (a collection's name).
    fn sort_name(&self) -> &str {
        self.emphasis_text.as_deref().unwrap_or(&self.text)
    }

    fn sort_timestamp(&self) -> Option<&str> {
        self.created_at.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_panel_item_creation() {
        let item = PanelItem::new("test").secondary_text("secondary").metadata("meta");
        assert_eq!(item.text, "test");
        assert_eq!(item.secondary_text, Some("secondary".to_string()));
        assert_eq!(item.metadata, Some("meta".to_string()));
    }

    #[test]
    fn panel_item_carries_repo() {
        let item = PanelItem::new("x").repo("codemark", "/home/u/codemark");
        assert_eq!(item.repo_name(), Some("codemark"));
        assert_eq!(item.repo_root(), Some("/home/u/codemark"));
    }

    #[test]
    fn panel_item_repo_defaults_none() {
        let item = PanelItem::new("x");
        assert_eq!(item.repo_name(), None);
        assert_eq!(item.repo_root(), None);
    }

    #[test]
    fn test_compressible_path_grows_with_width() {
        // A compressible path item shows more of the path as the pane widens.
        let path = "crates/codemark-tui/src/component/panel.rs";
        let item = PanelItem::new(path).no_health().compressible_path();

        let narrow = item.to_line(false, false, 20).to_string();
        let wide = item.to_line(false, false, 80).to_string();

        // Narrow rendering is compressed; wide rendering shows the full path.
        assert!(narrow.trim().len() < path.len());
        assert!(wide.contains(path));
        assert!(wide.trim_end().len() >= narrow.trim_end().len());
    }

    #[test]
    fn test_compress_path_fits_with_all_trailing_segments() {
        // A fully-populated compressible item must never render wider than the
        // available width. If a trailing span is added to `to_line` without a
        // matching entry in `trailing_width`, the path budget is over-estimated
        // and this assertion fails — catching drift between the two sites.
        let mut item = PanelItem::new("crates/codemark-tui/src/component/panel.rs")
            .compressible_path()
            .icon("")
            .emphasis("identifier")
            .secondary_text("secondary")
            .metadata("alice")
            .health(HealthStatus::Healthy)
            .sync_direction(Some(SyncDirection::Push))
            .checkmark(true)
            .published(true);
        item.spinner_text = Some("⠋".to_string());

        // Widths comfortably larger than the fixed trailing content, so the path
        // (not the trailing segments) is the limiting factor.
        for width in [50u16, 65, 80] {
            let line = item.to_line(false, false, width);
            assert!(
                line.width() <= width as usize,
                "rendered width {} exceeded available {}",
                line.width(),
                width
            );
        }
    }
}
