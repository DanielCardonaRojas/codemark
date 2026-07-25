//! Component module providing the base trait for all UI components.
//!
//! Components are the building blocks of the TUI. Each component is responsible
//! for rendering itself to a specific area of the terminal and handling events.

pub mod code_preview;
pub mod markdown_panel;
pub mod panel;

// Re-export types for convenience
pub use code_preview::CodePreview;
pub use codemark_core::sort::SortMethod;
pub use markdown_panel::MarkdownPanel;
pub use panel::{HealthStatus, Panel, PanelItem, SyncDirection};

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::Line,
    widgets::Widget,
};

use crate::event::Event;

/// Base trait for all UI components.
///
/// Components are composable and can be nested to create complex layouts.
/// Each component is responsible for its own rendering and event handling.
pub trait Component {
    /// Render the component to the given buffer.
    ///
    /// This method is called by the framework during the render phase.
    /// Components should only draw within the bounds of the provided area.
    fn render(&self, area: Rect, buf: &mut Buffer);

    /// Handle an event and return whether the event was handled.
    ///
    /// Returning `true` indicates the event was handled and should not
    /// propagate to other components. Returning `false` allows the event
    /// to bubble up to parent components.
    fn handle_event(&mut self, event: &Event) -> bool;

    /// Get the component's focus state.
    ///
    /// Focused components receive keyboard events first.
    fn focused(&self) -> bool {
        false
    }

    /// Set the component's focus state.
    ///
    /// Components should visually indicate their focus state when rendering.
    fn set_focus(&mut self, focused: bool);

    /// Get the component's preferred size constraints.
    ///
    /// This can be used by layout managers to allocate space.
    fn size_constraints(&self) -> SizeConstraints {
        SizeConstraints::default()
    }
}

/// Size constraints for a component.
///
/// Layout managers use these constraints to determine how much space
/// to allocate to each component.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SizeConstraints {
    /// Minimum width (0 = no minimum)
    pub min_width: u16,
    /// Minimum height (0 = no minimum)
    pub min_height: u16,
    /// Maximum width (0 = no maximum)
    pub max_width: u16,
    /// Maximum height (0 = no maximum)
    pub max_height: u16,
}

impl SizeConstraints {
    /// Create new size constraints.
    pub fn new(min_width: u16, min_height: u16, max_width: u16, max_height: u16) -> Self {
        Self { min_width, min_height, max_width, max_height }
    }

    /// Create constraints with only minimum sizes.
    pub fn min(min_width: u16, min_height: u16) -> Self {
        Self { min_width, min_height, ..Default::default() }
    }

    /// Create constraints with only maximum sizes.
    pub fn max(max_width: u16, max_height: u16) -> Self {
        Self { max_width, max_height, ..Default::default() }
    }

    /// Check if a given size satisfies the constraints.
    pub fn satisfies(&self, width: u16, height: u16) -> bool {
        let width_ok = self.min_width == 0 || width >= self.min_width;
        let height_ok = self.min_height == 0 || height >= self.min_height;
        let max_width_ok = self.max_width == 0 || width <= self.max_width;
        let max_height_ok = self.max_height == 0 || height <= self.max_height;
        width_ok && height_ok && max_width_ok && max_height_ok
    }
}

/// A simple text label component.
///
/// Labels are non-interactive components that display static text.
#[derive(Debug, Clone)]
pub struct Label {
    text: String,
    style: Style,
}

impl Label {
    /// Create a new label with the given text.
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into(), style: Style::default() }
    }

    /// Set the label's style.
    pub fn style(mut self, style: Style) -> Self {
        self.style = style;
        self
    }

    /// Get the label's text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Set the label's text.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
    }
}

impl Component for Label {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        Line::raw(&self.text).style(self.style).render(area, buf);
    }

    fn handle_event(&mut self, _event: &Event) -> bool {
        // Labels don't handle events
        false
    }

    fn focused(&self) -> bool {
        // Labels can't be focused
        false
    }

    fn set_focus(&mut self, _focused: bool) {
        // Labels can't be focused
    }
}

/// A pager component that shows horizontal dots for pagination.
#[derive(Debug, Clone)]
pub struct Pager {
    /// Total number of pages
    pub total: usize,
    /// Currently active page (0-indexed)
    pub current: usize,
    /// Per-page health status, used to color each dot. When empty, dots fall
    /// back to the accent (current) / dim (others) styling.
    health: Vec<HealthStatus>,
    /// Last rendered area
    last_area: std::cell::Cell<Rect>,
}

impl Pager {
    /// Create a new pager.
    pub fn new(total: usize, current: usize) -> Self {
        Self {
            total,
            current,
            health: Vec::new(),
            last_area: std::cell::Cell::new(Rect::default()),
        }
    }

    /// Attach per-page health statuses so each dot is colored by the health of
    /// the step it represents.
    pub fn with_health(mut self, health: Vec<HealthStatus>) -> Self {
        self.health = health;
        self
    }
}

impl Component for Pager {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.last_area.set(area);
        if self.total == 0 {
            return;
        }

        use ratatui::layout::Alignment;
        use ratatui::text::Span;
        use ratatui::widgets::Paragraph;

        let mut spans = Vec::with_capacity(self.total * 2);
        for i in 0..self.total {
            if i > 0 {
                spans.push(Span::raw(" "));
            }
            let is_current = i == self.current;
            // Every dot — filled and unfilled — is colored by its step's health.
            // Only when no health is supplied do we fall back to accent (current)
            // / dim (others). The current page stays a filled, bold dot so it
            // remains distinguishable even when a neighbor shares its color.
            let color = self.health.get(i).map(|h| h.color()).unwrap_or_else(|| {
                if is_current {
                    crate::theme::palette().accent
                } else {
                    crate::theme::palette().dim
                }
            });
            let glyph = if is_current { "●" } else { "○" };
            let mut style = Style::default().fg(color);
            if is_current {
                style = style.add_modifier(Modifier::BOLD);
            }
            spans.push(Span::styled(glyph, style));
        }

        let p = Paragraph::new(Line::from(spans)).alignment(Alignment::Center);
        p.render(area, buf);

        // Show the current page index (1-based) over the total, right-aligned.
        let index = Span::styled(
            format!("{} / {}", self.current + 1, self.total),
            Style::default().fg(crate::theme::palette().dim),
        );
        Paragraph::new(Line::from(index)).alignment(Alignment::Right).render(area, buf);
    }

    fn handle_event(&mut self, event: &Event) -> bool {
        if let Event::Mouse(mouse) = event
            && let ratatui::crossterm::event::MouseEventKind::Down(
                ratatui::crossterm::event::MouseButton::Left,
            ) = mouse.kind
        {
            let area = self.last_area.get();
            if mouse.column >= area.x
                && mouse.column < area.x + area.width
                && mouse.row >= area.y
                && mouse.row < area.y + area.height
            {
                // Simple logic: clicking left half goes back, right half goes forward
                let center_x = area.x + area.width / 2;
                if mouse.column < center_x {
                    self.current = self.current.saturating_sub(1);
                } else if self.current + 1 < self.total {
                    self.current += 1;
                }
                return true;
            }
        }
        false
    }

    fn set_focus(&mut self, _focused: bool) {}
}

/// A simple spacer component that takes up space but renders nothing.
///
/// Useful for creating gaps between components in layouts.
#[derive(Debug, Clone, Copy)]
pub struct Spacer {
    _width: u16,
    _height: u16,
}

impl Spacer {
    /// Create a new spacer with the given dimensions.
    pub fn new(width: u16, height: u16) -> Self {
        Self { _width: width, _height: height }
    }

    /// Create a horizontal spacer (width only).
    pub fn horizontal(width: u16) -> Self {
        Self { _width: width, _height: 1 }
    }

    /// Create a vertical spacer (height only).
    pub fn vertical(height: u16) -> Self {
        Self { _width: 1, _height: height }
    }
}

impl Default for Spacer {
    fn default() -> Self {
        Self::new(1, 1)
    }
}

impl Component for Spacer {
    fn render(&self, _area: Rect, _buf: &mut Buffer) {
        // Spacer renders nothing
    }

    fn handle_event(&mut self, _event: &Event) -> bool {
        false
    }

    fn focused(&self) -> bool {
        false
    }

    fn set_focus(&mut self, _focused: bool) {}
}
