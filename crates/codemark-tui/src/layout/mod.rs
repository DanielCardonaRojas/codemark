//! Layout management for organizing components in the terminal.
//!
//! The layout system provides flexible ways to arrange components
//! in rows, columns, and grids with automatic sizing.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};

use crate::component::Component;
use crate::component::SizeConstraints;

/// Direction for layout arrangement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutDirection {
    /// Arrange components horizontally (left to right)
    Horizontal,
    /// Arrange components vertically (top to bottom)
    Vertical,
}

impl From<LayoutDirection> for Direction {
    fn from(dir: LayoutDirection) -> Self {
        match dir {
            LayoutDirection::Horizontal => Direction::Horizontal,
            LayoutDirection::Vertical => Direction::Vertical,
        }
    }
}

/// A layout manager that arranges multiple components.
///
/// Layouts can be nested to create complex arrangements similar to
/// lazy git's flexible panel system.
pub struct LayoutManager {
    /// The direction of this layout
    direction: LayoutDirection,
    /// The components in this layout
    children: Vec<LayoutChild>,
    /// Gap between components in cells
    gap: u16,
    /// Alignment for this layout
    alignment: Alignment,
}

/// A child in a layout - either a component or another nested layout.
pub enum LayoutChild {
    /// A boxed component
    Component(Box<dyn Component>),
    /// A nested layout
    Layout(Box<LayoutManager>),
    /// A spacer that takes up fixed space
    Spacer(u16),
    /// A flexible spacer that takes up remaining space
    Flex,
}

impl LayoutChild {
    /// Create a component child.
    pub fn component<C: Component + 'static>(component: C) -> Self {
        Self::Component(Box::new(component))
    }

    /// Create a nested layout child.
    pub fn layout(layout: LayoutManager) -> Self {
        Self::Layout(Box::new(layout))
    }

    /// Create a fixed spacer.
    pub fn spacer(size: u16) -> Self {
        Self::Spacer(size)
    }

    /// Create a flexible spacer.
    pub fn flex() -> Self {
        Self::Flex
    }

    /// Get the size constraints for this child.
    fn constraints(&self) -> SizeConstraints {
        match self {
            Self::Component(c) => c.size_constraints(),
            Self::Layout(l) => l.size_constraints(),
            Self::Spacer(_) => SizeConstraints::default(),
            Self::Flex => SizeConstraints::default(),
        }
    }
}

impl LayoutManager {
    /// Create a new horizontal layout.
    pub fn horizontal() -> Self {
        Self {
            direction: LayoutDirection::Horizontal,
            children: Vec::new(),
            gap: 0,
            alignment: Alignment::Left,
        }
    }

    /// Create a new vertical layout.
    pub fn vertical() -> Self {
        Self {
            direction: LayoutDirection::Vertical,
            children: Vec::new(),
            gap: 0,
            alignment: Alignment::Center,
        }
    }

    /// Add a child to this layout.
    pub fn child(mut self, child: LayoutChild) -> Self {
        self.children.push(child);
        self
    }

    /// Add multiple children to this layout.
    pub fn children(mut self, children: impl IntoIterator<Item = LayoutChild>) -> Self {
        self.children.extend(children);
        self
    }

    /// Set the gap between components.
    pub fn gap(mut self, gap: u16) -> Self {
        self.gap = gap;
        self
    }

    /// Set the alignment for this layout.
    pub fn alignment(mut self, alignment: Alignment) -> Self {
        self.alignment = alignment;
        self
    }

    /// Get the combined size constraints for all children.
    pub fn size_constraints(&self) -> SizeConstraints {
        self.children.iter().fold(SizeConstraints::default(), |acc, child| {
            let c = child.constraints();
            match self.direction {
                LayoutDirection::Horizontal => SizeConstraints {
                    min_width: acc.min_width.saturating_add(c.min_width),
                    min_height: acc.min_height.max(c.min_height),
                    ..Default::default()
                },
                LayoutDirection::Vertical => SizeConstraints {
                    min_width: acc.min_width.max(c.min_width),
                    min_height: acc.min_height.saturating_add(c.min_height),
                    ..Default::default()
                },
            }
        })
    }

    /// Calculate constraints for layout children.
    fn calculate_constraints(&self) -> Vec<Constraint> {
        self.children
            .iter()
            .map(|child| match child {
                LayoutChild::Spacer(size) => Constraint::Length(*size),
                LayoutChild::Flex => Constraint::Min(0),
                _ => Constraint::Min(0),
            })
            .collect()
    }

    /// Split an area into chunks for each child.
    fn split_area(&self, area: Rect) -> Vec<Rect> {
        let constraints = self.calculate_constraints();

        // Create the layout with gap consideration
        if self.gap > 0 && !self.children.is_empty() {
            // We need to account for gaps by reducing available space
            let _total_gap = self.gap.saturating_mul(self.children.len().saturating_sub(1) as u16);
            let layout = Layout::default()
                .direction(self.direction.into())
                .constraints(constraints.as_slice())
                .margin(0);

            let raw_chunks = layout.split(area);
            let mut result = Vec::with_capacity(raw_chunks.len());

            for (i, chunk) in raw_chunks.iter().enumerate() {
                let mut adjusted = *chunk;
                if i > 0 {
                    // Shift position to account for gap (don't also reduce size here)
                    let offset = self.gap;
                    if self.direction == LayoutDirection::Horizontal {
                        adjusted.x = adjusted.x.saturating_add(offset);
                    } else {
                        adjusted.y = adjusted.y.saturating_add(offset);
                    }
                }
                if i < raw_chunks.len() - 1 {
                    // Reduce size to account for trailing gap
                    if self.direction == LayoutDirection::Horizontal {
                        adjusted.width = adjusted.width.saturating_sub(self.gap);
                    } else {
                        adjusted.height = adjusted.height.saturating_sub(self.gap);
                    }
                }
                result.push(adjusted);
            }
            result
        } else {
            Layout::default()
                .direction(self.direction.into())
                .constraints(constraints.as_slice())
                .split(area)
                .to_vec()
        }
    }

    /// Render the layout and all its children.
    pub fn render(&self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        let chunks = self.split_area(area);

        for (child, chunk) in self.children.iter().zip(chunks.iter()) {
            match child {
                LayoutChild::Component(component) => {
                    component.render(*chunk, buf);
                }
                LayoutChild::Layout(layout) => {
                    layout.render(*chunk, buf);
                }
                LayoutChild::Spacer(_) | LayoutChild::Flex => {
                    // Spacers render nothing
                }
            }
        }
    }

    /// Handle events for children, propagating to focused component.
    ///
    /// Returns true if the event was handled by any child.
    pub fn handle_event(&mut self, event: &crate::event::Event) -> bool {
        for child in &mut self.children {
            match child {
                LayoutChild::Component(component) => {
                    if component.handle_event(event) {
                        return true;
                    }
                }
                LayoutChild::Layout(layout) => {
                    if layout.handle_event(event) {
                        return true;
                    }
                }
                LayoutChild::Spacer(_) | LayoutChild::Flex => {}
            }
        }
        false
    }

    /// Set focus state for all children.
    pub fn set_focus(&mut self, focused: bool) {
        for child in &mut self.children {
            match child {
                LayoutChild::Component(component) => {
                    component.set_focus(focused);
                }
                LayoutChild::Layout(layout) => {
                    layout.set_focus(focused);
                }
                LayoutChild::Spacer(_) | LayoutChild::Flex => {}
            }
        }
    }

    /// Focus a specific child by index.
    pub fn focus_child(&mut self, index: usize) {
        for (i, child) in self.children.iter_mut().enumerate() {
            let should_focus = i == index;
            match child {
                LayoutChild::Component(component) => {
                    component.set_focus(should_focus);
                }
                LayoutChild::Layout(layout) => {
                    layout.set_focus(should_focus);
                }
                LayoutChild::Spacer(_) | LayoutChild::Flex => {}
            }
        }
    }
}

impl Default for LayoutManager {
    fn default() -> Self {
        Self::horizontal()
    }
}

/// A split layout that divides space into specific proportions.
///
/// This is useful for creating layouts like "sidebar takes 30%, main content takes 70%".
pub struct SplitLayout {
    /// Direction of the split
    direction: LayoutDirection,
    /// Constraints for each section
    constraints: Vec<Constraint>,
    /// Children for each section
    children: Vec<LayoutChild>,
}

impl SplitLayout {
    /// Create a new horizontal split layout.
    pub fn horizontal(constraints: Vec<Constraint>) -> Self {
        Self { direction: LayoutDirection::Horizontal, constraints, children: Vec::new() }
    }

    /// Create a new vertical split layout.
    pub fn vertical(constraints: Vec<Constraint>) -> Self {
        Self { direction: LayoutDirection::Vertical, constraints, children: Vec::new() }
    }

    /// Add a child to a specific section.
    pub fn child(mut self, index: usize, child: LayoutChild) -> Self {
        // Ensure we have enough slots
        while self.children.len() <= index {
            self.children.push(LayoutChild::Flex);
        }
        self.children[index] = child;
        self
    }

    /// Render the split layout.
    pub fn render(&self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        if self.constraints.is_empty() {
            return;
        }

        let layout = Layout::default()
            .direction(self.direction.into())
            .constraints(self.constraints.as_slice())
            .split(area);

        for (child, chunk) in self.children.iter().zip(layout.iter()) {
            match child {
                LayoutChild::Component(component) => {
                    component.render(*chunk, buf);
                }
                LayoutChild::Layout(inner_layout) => {
                    inner_layout.render(*chunk, buf);
                }
                LayoutChild::Spacer(_) | LayoutChild::Flex => {}
            }
        }
    }

    /// Handle events for children.
    pub fn handle_event(&mut self, event: &crate::event::Event) -> bool {
        for child in &mut self.children {
            match child {
                LayoutChild::Component(component) => {
                    if component.handle_event(event) {
                        return true;
                    }
                }
                LayoutChild::Layout(layout) => {
                    if layout.handle_event(event) {
                        return true;
                    }
                }
                LayoutChild::Spacer(_) | LayoutChild::Flex => {}
            }
        }
        false
    }

    /// Set focus state for all children.
    pub fn set_focus(&mut self, focused: bool) {
        for child in &mut self.children {
            match child {
                LayoutChild::Component(component) => {
                    component.set_focus(focused);
                }
                LayoutChild::Layout(layout) => {
                    layout.set_focus(focused);
                }
                LayoutChild::Spacer(_) | LayoutChild::Flex => {}
            }
        }
    }

    /// Focus a specific section by index.
    pub fn focus_section(&mut self, index: usize) {
        for (i, child) in self.children.iter_mut().enumerate() {
            let should_focus = i == index;
            match child {
                LayoutChild::Component(component) => {
                    component.set_focus(should_focus);
                }
                LayoutChild::Layout(layout) => {
                    layout.set_focus(should_focus);
                }
                LayoutChild::Spacer(_) | LayoutChild::Flex => {}
            }
        }
    }
}

/// Helper functions for common layout patterns.
pub mod helpers {
    use super::*;

    /// Create a typical lazy-git style layout with sidebar and main panel.
    pub fn sidebar_layout(
        sidebar: impl Component + 'static,
        main: impl Component + 'static,
    ) -> LayoutManager {
        LayoutManager::horizontal()
            .gap(1)
            .children(vec![LayoutChild::component(sidebar), LayoutChild::component(main)])
    }

    /// Create a three-panel layout (left, middle, right).
    pub fn three_panel(
        left: impl Component + 'static,
        middle: impl Component + 'static,
        right: impl Component + 'static,
    ) -> LayoutManager {
        LayoutManager::horizontal().gap(1).children(vec![
            LayoutChild::component(left),
            LayoutChild::component(middle),
            LayoutChild::component(right),
        ])
    }

    /// Create a layout with top header and bottom content.
    pub fn header_content(
        header: impl Component + 'static,
        content: impl Component + 'static,
    ) -> LayoutManager {
        LayoutManager::vertical()
            .gap(1)
            .children(vec![LayoutChild::component(header), LayoutChild::component(content)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_layout_direction_conversion() {
        assert_eq!(Direction::from(LayoutDirection::Horizontal), Direction::Horizontal);
        assert_eq!(Direction::from(LayoutDirection::Vertical), Direction::Vertical);
    }

    #[test]
    fn test_layout_manager_creation() {
        let layout = LayoutManager::horizontal().gap(1);
        assert_eq!(layout.direction, LayoutDirection::Horizontal);
        assert_eq!(layout.gap, 1);
    }

    #[test]
    fn test_layout_child_variants() {
        use crate::component::Label;

        let component = LayoutChild::component(Label::new("test"));
        let spacer = LayoutChild::spacer(5);
        let flex = LayoutChild::flex();

        match component {
            LayoutChild::Component(_) => {}
            _ => panic!("Expected component"),
        }

        match spacer {
            LayoutChild::Spacer(s) => assert_eq!(s, 5),
            _ => panic!("Expected spacer"),
        }

        match flex {
            LayoutChild::Flex => {}
            _ => panic!("Expected flex"),
        }
    }
}
