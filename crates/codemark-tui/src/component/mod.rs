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

        // The `n / N` index sits at the right edge. Reserve its width (plus a
        // one-column gap) on *both* sides of the row so the dot window stays
        // centered in the true middle of the row instead of being shoved
        // left by the right-aligned index — otherwise a full row of dots looks
        // longer on the left and shorter on the right. Size the reservation
        // from the *widest* possible index (`N / N`) rather than the current
        // page's text so the visible window size stays constant as you page —
        // otherwise the block boundaries would shift when the current page's
        // digit count changes.
        let index_text = format!("{} / {}", self.current + 1, self.total);
        let index_width = (self.total.to_string().len() * 2 + 3) as u16;
        let reserve = index_width.saturating_add(1);
        let dots_width = area.width.saturating_sub(reserve.saturating_mul(2)) as usize;

        // Each dot occupies two columns (glyph + trailing space) except the
        // last, so `v` dots span `2v - 1` columns. The window holds as many dots
        // as the reserved space fits; the index already conveys the exact
        // position within the full collection. When the row is too narrow to
        // hold even one dot we render none — forcing a lone dot would only put
        // it under the right-aligned index, which would overwrite it and leave
        // the current-page indicator garbled.
        let fits = dots_width.saturating_add(1) / 2;
        // `NonZeroUsize` (rather than a plain `> 0` guard) keeps the divisions
        // below infallible without a `checked_div` dance.
        if let Some(visible) = std::num::NonZeroUsize::new(self.total.min(fits)) {
            let visible = visible.get();
            // Page the window in fixed blocks: the current dot moves freely from
            // the left edge to the right edge of its block, and only when it
            // crosses an edge does the window slide to the next block, resetting
            // the current dot to the left edge. (Contrast with pinning the dot to
            // the middle, which slides on every move.) A short final block — when
            // `total` isn't divisible by `visible` — keeps that reset: its dots
            // just fill the left of the window and leave the right empty.
            let start = (self.current / visible) * visible;
            let end = (start + visible).min(self.total);

            let mut spans = Vec::with_capacity(visible * 2);
            for i in start..end {
                if i > start {
                    spans.push(Span::raw(" "));
                }
                let is_current = i == self.current;
                // Every dot — filled and unfilled — is colored by its step's
                // health. Only when no health is supplied do we fall back to
                // accent (current) / dim (others). The current page stays a
                // filled, bold dot so it remains distinguishable even when a
                // neighbor shares its color.
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

            // Left-align the dots within a centered slot sized for a *full*
            // window (`2 * visible - 1` columns), not the current line. Centering
            // the line itself would re-center a shorter final block and shift the
            // whole window sideways; a fixed full-width slot instead keeps every
            // block's dots in the same columns, with a short block simply leaving
            // its right end empty.
            let full_width = (visible * 2 - 1) as u16;
            let offset = area.width.saturating_sub(full_width) / 2;
            let dots_area = Rect { x: area.x + offset, width: full_width, ..area };
            Paragraph::new(Line::from(spans)).alignment(Alignment::Left).render(dots_area, buf);
        }

        // Show the current page index (1-based) over the total, right-aligned.
        let index = Span::styled(index_text, Style::default().fg(crate::theme::palette().dim));
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

#[cfg(test)]
mod pager_tests {
    use super::*;
    use ratatui::layout::Rect;

    /// Render a pager to a fresh buffer and return the single row as a string.
    fn render_row(total: usize, current: usize, width: u16) -> String {
        let area = Rect::new(0, 0, width, 1);
        let mut buf = Buffer::empty(area);
        Pager::new(total, current).render(area, &mut buf);
        (0..width).map(|x| buf[(x, 0)].symbol()).collect()
    }

    fn is_dot(c: char) -> bool {
        c == '●' || c == '○'
    }

    /// Zero-based position of the filled (current) dot among the visible dots.
    fn filled_dot_position(row: &str) -> usize {
        row.chars().filter(|c| is_dot(*c)).position(|c| c == '●').expect("a filled dot")
    }

    #[test]
    fn window_size_is_bounded_by_available_width_and_the_index() {
        // The window holds only as many dots as fit once the `N / N` index is
        // reserved on both sides, not all 100 pages.
        let row = render_row(100, 5, 80);
        let dots = row.chars().filter(|c| is_dot(*c)).count();
        // 80 cols - 2*(9-wide "100 / 100" reservation + gap) = 60 -> 30 dots fit.
        assert_eq!(dots, 30, "row: {row:?}");

        // A narrower row fits fewer dots.
        let narrow = render_row(100, 5, 40);
        let narrow_dots = narrow.chars().filter(|c| is_dot(*c)).count();
        assert!(narrow_dots < dots, "narrow: {narrow:?}");
        assert!(row.contains("6 / 100"), "index should show the true position; row: {row:?}");
    }

    #[test]
    fn shows_every_dot_when_they_all_fit() {
        let row = render_row(3, 1, 80);
        let dots = row.chars().filter(|c| is_dot(*c)).count();
        assert_eq!(dots, 3, "row: {row:?}");
    }

    #[test]
    fn selection_moves_freely_within_a_block_before_the_window_slides() {
        // Within a block the current dot advances one position per page without
        // moving the window, so its position tracks the page.
        let width = 80u16;
        let visible = render_row(100, 0, width).chars().filter(|c| is_dot(*c)).count();

        // First and last page of the opening block keep the same window but move
        // the filled dot from the left edge to the right edge.
        assert_eq!(filled_dot_position(&render_row(100, 0, width)), 0);
        assert_eq!(filled_dot_position(&render_row(100, 3, width)), 3);
        assert_eq!(filled_dot_position(&render_row(100, visible - 1, width)), visible - 1);

        // Crossing the right edge slides to the next block, putting the current
        // dot back at the left edge.
        assert_eq!(filled_dot_position(&render_row(100, visible, width)), 0);
    }

    #[test]
    fn dot_window_never_overlaps_the_index() {
        // The rightmost dot must sit left of where the `n / N` index begins so
        // the two never collide, even on a narrow row.
        let width = 40u16;
        let row = render_row(100, 50, width);
        let last_dot_col = row.char_indices().rfind(|(_, c)| is_dot(*c)).unwrap().0;
        let index_col = row.find(|c: char| c.is_ascii_digit()).unwrap();
        assert!(last_dot_col < index_col, "dots must not overrun the index; row: {row:?}");
    }

    #[test]
    fn too_narrow_a_row_shows_the_index_but_no_dots() {
        // With no room for dots the pager must not force a lone dot (it would be
        // overwritten by the right-aligned index); only the index renders.
        let row = render_row(100, 50, 12);
        assert_eq!(row.chars().filter(|c| is_dot(*c)).count(), 0, "row: {row:?}");
        assert!(row.contains("51 / 100"), "index should still render; row: {row:?}");
    }

    #[test]
    fn final_partial_block_aligns_and_resets_without_jumping() {
        // 100 pages / 30-per-window leaves a 10-page tail. The final block holds
        // only those 10 dots, but they must render in the same leftmost columns
        // as a full mid-collection block — so the window doesn't shift sideways —
        // and paging into it must reset the current dot to the left edge rather
        // than jumping backward within an overlapping window.
        let width = 80u16;
        let dot_cols = |current: usize| -> Vec<usize> {
            render_row(100, current, width)
                .char_indices()
                .filter(|(_, c)| is_dot(*c))
                .map(|(i, _)| i)
                .collect()
        };
        let mid = dot_cols(50);
        let tail = dot_cols(99);
        assert_eq!(mid.len(), 30, "mid block should be full width: {mid:?}");
        assert_eq!(tail.len(), 10, "final block holds the 10-page tail: {tail:?}");
        assert_eq!(tail, mid[..tail.len()], "tail must be column-aligned with full blocks");

        // Crossing from the right edge of one block into the next resets the
        // current dot to the left edge — a forward move never jumps it backward.
        assert_eq!(filled_dot_position(&render_row(100, 89, width)), 29, "right edge of a block");
        assert_eq!(filled_dot_position(&render_row(100, 90, width)), 0, "left edge of final block");
    }
}
