//! Tab selection component for switching between views.

use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Widget, Wrap},
};

/// Panel 3 tabs (Tours/Collections/Bookmarks).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel3Tab {
    Tours = 0,
    Collections = 1,
    Bookmarks = 2,
}

impl Panel3Tab {
    /// Get the index of this tab.
    pub fn index(self) -> usize {
        self as usize
    }

    /// Get all tabs in order.
    pub fn all() -> &'static [Panel3Tab] {
        &[Panel3Tab::Tours, Panel3Tab::Collections, Panel3Tab::Bookmarks]
    }

    /// Get the tab label.
    pub fn label(self) -> &'static str {
        match self {
            Panel3Tab::Tours => "Tours",
            Panel3Tab::Collections => "Collections",
            Panel3Tab::Bookmarks => "Bookmarks",
        }
    }

    /// Try to convert an index to a Panel3Tab.
    pub fn from_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(Panel3Tab::Tours),
            1 => Some(Panel3Tab::Collections),
            2 => Some(Panel3Tab::Bookmarks),
            _ => None,
        }
    }
}

/// Panel 2 tabs (Tags/Branches).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel2Tab {
    Tags = 0,
    Branches = 1,
}

impl Panel2Tab {
    /// Get the index of this tab.
    pub fn index(self) -> usize {
        self as usize
    }

    /// Get all tabs in order.
    pub fn all() -> &'static [Panel2Tab] {
        &[Panel2Tab::Tags, Panel2Tab::Branches]
    }

    /// Get the tab label.
    pub fn label(self) -> &'static str {
        match self {
            Panel2Tab::Tags => "Tags",
            Panel2Tab::Branches => "Branches",
        }
    }

    /// Try to convert an index to a Panel2Tab.
    pub fn from_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(Panel2Tab::Tags),
            1 => Some(Panel2Tab::Branches),
            _ => None,
        }
    }
}

/// A single tab.
#[derive(Debug, Clone)]
pub struct Tab {
    /// The tab label
    label: String,
    /// Optional badge text (e.g., item count)
    badge_text: Option<String>,
}

impl Tab {
    /// Create a new tab.
    pub fn new(label: impl Into<String>) -> Self {
        Self { label: label.into(), badge_text: None }
    }

    /// Set a badge for this tab.
    pub fn badge(mut self, badge: impl Into<String>) -> Self {
        self.badge_text = Some(badge.into());
        self
    }

    /// Get the tab label.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Get the badge text.
    pub fn badge_text(&self) -> Option<&str> {
        self.badge_text.as_deref()
    }

    /// Render this tab as a Line with its index number.
    fn render(&self, index: usize, selected: bool, focused: bool) -> Line<'_> {
        let base_style = if selected && focused {
            Style::default().fg(Color::Black).bg(Color::Green)
        } else if selected {
            Style::default().fg(Color::Green).bg(Color::DarkGray)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let mut spans = vec![
            Span::styled(" ", base_style),
            Span::styled(format!("{}:", index + 1), base_style),
            Span::styled(&self.label, base_style.add_modifier(ratatui::style::Modifier::BOLD)),
        ];

        if let Some(badge) = &self.badge_text {
            spans.push(Span::styled(format!(" ({badge})"), base_style));
        }

        spans.push(Span::styled(" ", base_style));

        Line::from(spans)
    }
}

/// Tab selection component.
#[derive(Debug, Clone)]
pub struct TabSelection {
    /// Available tabs
    tabs: Vec<Tab>,
    /// Currently selected tab index
    selected: usize,
    /// Whether the tabs are focused
    focused: bool,
}

impl TabSelection {
    /// Create a new tab selection.
    pub fn new(tabs: Vec<Tab>) -> Self {
        Self { tabs, selected: 0, focused: false }
    }

    /// Get the selected tab index.
    pub fn selected_index(&self) -> usize {
        self.selected
    }

    /// Set the selected tab index.
    pub fn set_selected(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.selected = index;
        }
    }

    /// Select the next tab.
    pub fn next(&mut self) {
        if !self.tabs.is_empty() {
            self.selected = (self.selected + 1) % self.tabs.len();
        }
    }

    /// Select the previous tab.
    pub fn previous(&mut self) {
        if !self.tabs.is_empty() {
            self.selected =
                if self.selected == 0 { self.tabs.len() - 1 } else { self.selected - 1 };
        }
    }

    /// Get the number of tabs.
    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    /// Check if there are no tabs.
    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    /// Render the tabs.
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        let mut lines = Vec::new();

        for (i, tab) in self.tabs.iter().enumerate() {
            let selected = i == self.selected;
            lines.push(tab.render(i, selected, self.focused));
        }

        // Join tabs with spacing
        let mut spans = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw(" "));
            }
            spans.extend(line.spans.clone());
        }

        let paragraph = ratatui::widgets::Paragraph::new(Line::from(spans))
            .wrap(Wrap { trim: false })
            .alignment(Alignment::Left);

        paragraph.render(area, buf);
    }

    /// Render tabs as a Title for use with Block borders (inline with border).
    pub fn render_as_titles(&self, focused: bool) -> Line<'_> {
        let mut spans = Vec::new();

        for (i, tab) in self.tabs.iter().enumerate() {
            let selected = i == self.selected;
            let style = if selected && focused {
                Style::default().fg(Color::Black).bg(Color::Green)
            } else if selected {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            if i > 0 {
                spans.push(Span::styled(" ", Style::default()));
            }
            spans.push(Span::styled(format!("{}:", i + 1), style));
            spans.push(Span::styled(
                tab.label(),
                style.add_modifier(ratatui::style::Modifier::BOLD),
            ));

            if let Some(badge) = tab.badge_text() {
                spans.push(Span::styled(format!(" ({badge})"), style));
            }
            spans.push(Span::styled(" ", style));
        }

        Line::from(spans).alignment(Alignment::Left)
    }

    /// Set focus state.
    pub fn set_focus(&mut self, focused: bool) {
        self.focused = focused;
    }
}

impl Default for TabSelection {
    fn default() -> Self {
        Self::new(vec![Tab::new("Tab 1"), Tab::new("Tab 2")])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tab_creation() {
        let tab = Tab::new("Test").badge("5");
        assert_eq!(tab.label(), "Test");
        assert_eq!(tab.badge_text(), Some("5"));
    }

    #[test]
    fn test_tab_selection() {
        let mut tabs = TabSelection::new(vec![Tab::new("Local"), Tab::new("Remote")]);

        assert_eq!(tabs.selected_index(), 0);

        tabs.next();
        assert_eq!(tabs.selected_index(), 1);

        tabs.next();
        assert_eq!(tabs.selected_index(), 0); // Wrapped

        tabs.previous();
        assert_eq!(tabs.selected_index(), 1); // Wrapped
    }

    #[test]
    fn test_tab_selection_empty() {
        let tabs = TabSelection::new(vec![]);
        assert!(tabs.is_empty());
        assert_eq!(tabs.len(), 0);
    }
}
