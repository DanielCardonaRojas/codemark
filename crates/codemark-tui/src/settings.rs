//! Tabbed settings overlay for the TUI.
//!
//! Reached with `,`, this modal popup groups the app's *global* state — the
//! things that don't change with focus — into tabs the user switches with
//! `[`/`]` (or ←/→). It is deliberately separate from the contextual
//! keybindings cheat sheet (`?`, see [`crate::ui::render_help_panel`]): those
//! are ephemeral "what can I do right now", these are static "how is the app
//! set up".
//!
//! This module owns the overlay's visibility and selected-tab state plus its
//! rendering; the data the Configuration tab shows is supplied by the caller so
//! the overlay stays decoupled from the browser layout.
//!
//! Adding a pane is a matter of adding a [`SettingsTab`] variant (and an entry
//! in [`SettingsTab::ALL`]) plus a render arm — the tab bar, navigation, and
//! dispatch all derive from that list.

use crossterm::event::KeyCode;
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Clear, Paragraph, Widget, Wrap},
};

/// The tabs available in the settings overlay, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    /// Important paths: database, registry, config, data/templates/models dirs,
    /// logs, plus the embeddings model and theme.
    Configuration,
    /// Version and project links (repository, support).
    About,
}

impl SettingsTab {
    /// Every tab, in the order they appear in the tab bar.
    pub const ALL: &'static [SettingsTab] = &[SettingsTab::Configuration, SettingsTab::About];

    /// The label shown for this tab in the tab bar.
    fn title(self) -> &'static str {
        match self {
            SettingsTab::Configuration => "Configuration",
            SettingsTab::About => "About",
        }
    }

    /// This tab's position in [`SettingsTab::ALL`].
    fn index(self) -> usize {
        Self::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }

    /// The tab at `index`, wrapping around the ends.
    fn at(index: usize) -> SettingsTab {
        Self::ALL[index % Self::ALL.len()]
    }
}

/// Modal, tabbed settings overlay: owns its visibility and selected-tab state.
#[derive(Debug)]
pub struct SettingsOverlay {
    visible: bool,
    tab: SettingsTab,
}

impl Default for SettingsOverlay {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsOverlay {
    /// Create a hidden overlay defaulting to the Configuration tab.
    pub fn new() -> Self {
        Self { visible: false, tab: SettingsTab::Configuration }
    }

    /// Whether the overlay is currently shown.
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// The currently selected tab.
    pub fn tab(&self) -> SettingsTab {
        self.tab
    }

    /// Toggle visibility (the `,` entry point).
    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }

    /// Hide the overlay.
    pub fn hide(&mut self) {
        self.visible = false;
    }

    fn next_tab(&mut self) {
        self.tab = SettingsTab::at(self.tab.index() + 1);
    }

    fn prev_tab(&mut self) {
        // + len keeps the index non-negative before the modulo in `at`.
        self.tab = SettingsTab::at(self.tab.index() + SettingsTab::ALL.len() - 1);
    }

    /// Handle a key while the overlay is open.
    ///
    /// The overlay is modal, so this returns `true` for every key (it swallows
    /// all input while visible). `Esc`/`,` close it; `[`/`]` (and the arrow /
    /// tab equivalents) switch tabs.
    pub fn handle_key(&mut self, code: KeyCode) -> bool {
        match code {
            KeyCode::Esc | KeyCode::Char(',') => self.hide(),
            KeyCode::Char(']') | KeyCode::Right | KeyCode::Tab => self.next_tab(),
            KeyCode::Char('[') | KeyCode::Left | KeyCode::BackTab => self.prev_tab(),
            _ => {}
        }
        true
    }

    /// Render the overlay as a centered modal over `area`.
    ///
    /// `config` feeds the Configuration tab (a list of labeled paths/settings),
    /// passed in so the overlay needn't know about the layout.
    pub fn render(&self, area: Rect, buf: &mut Buffer, config: &[(&'static str, String)]) {
        let popup = centered_rect(area, 0.55, 0.7);
        Widget::render(Clear, popup, buf);

        let palette = crate::theme::palette();

        // The tab bar lives inline in the block's top border, matching the
        // panel chrome used elsewhere in the TUI.
        let mut title_spans: Vec<Span> = vec![Span::raw(" ")];
        for (i, tab) in SettingsTab::ALL.iter().enumerate() {
            if i > 0 {
                title_spans.push(Span::styled(" · ", Style::default().fg(palette.gray)));
            }
            let style = if *tab == self.tab {
                Style::default().fg(palette.accent).bold()
            } else {
                Style::default().fg(palette.dim)
            };
            title_spans.push(Span::styled(tab.title(), style));
        }

        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(palette.accent))
            .title(Line::from(title_spans))
            .title_alignment(Alignment::Left);
        let inner = block.inner(popup);
        block.render(popup, buf);

        // Reserve the last row for a navigation hint.
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(1)])
            .split(inner);

        match self.tab {
            SettingsTab::Configuration => render_configuration(chunks[0], buf, config),
            SettingsTab::About => render_about(chunks[0], buf),
        }

        let hint = Line::from(vec![
            Span::styled("[ / ]", Style::default().fg(palette.accent).bold()),
            Span::styled(" switch tabs    ", Style::default().fg(palette.dim)),
            Span::styled("Esc", Style::default().fg(palette.accent).bold()),
            Span::styled(" close", Style::default().fg(palette.dim)),
        ])
        .alignment(Alignment::Center);
        Paragraph::new(hint).render(chunks[1], buf);
    }
}

/// Render the Configuration tab: a `label : value` list of important paths and
/// settings, with labels aligned to the widest one.
fn render_configuration(area: Rect, buf: &mut Buffer, rows: &[(&'static str, String)]) {
    let palette = crate::theme::palette();
    let label_width = rows.iter().map(|(label, _)| label.len()).max().unwrap_or(0);

    let mut text = Text::default();
    for (label, value) in rows {
        text.push_line(Line::from(vec![
            Span::styled(
                format!(" {label:<label_width$}  "),
                Style::default().fg(palette.accent).bold(),
            ),
            Span::styled(value.clone(), Style::default().fg(palette.emphasis)),
        ]));
    }
    Paragraph::new(text).wrap(Wrap { trim: false }).render(area, buf);
}

/// Render the About tab: version and project links.
fn render_about(area: Rect, buf: &mut Buffer) {
    let palette = crate::theme::palette();

    let label = |text: &str| Span::styled(format!(" {text:<12}  "), Style::default().fg(palette.accent).bold());
    let value = |text: &str| Span::styled(text.to_string(), Style::default().fg(palette.emphasis));

    let mut text = Text::default();
    text.push_line(Line::from(vec![
        Span::raw(" "),
        Span::styled("Codemark", Style::default().fg(palette.emphasis).bold()),
        Span::styled(
            "  — structural code bookmarks that survive refactoring",
            Style::default().fg(palette.dim),
        ),
    ]));
    text.push_line(Line::raw(""));
    text.push_line(Line::from(vec![label("Version"), value(crate::VERSION)]));
    text.push_line(Line::from(vec![label("Repository"), value(env!("CARGO_PKG_REPOSITORY"))]));
    text.push_line(Line::from(vec![
        label("Support"),
        value("https://github.com/sponsors/DanielCardonaRojas"),
    ]));

    Paragraph::new(text).wrap(Wrap { trim: false }).render(area, buf);
}

/// Compute a rectangle centered in `area`, sized to the given fraction of its
/// width and height.
fn centered_rect(area: Rect, width_pct: f32, height_pct: f32) -> Rect {
    let width = ((area.width as f32 * width_pct) as u16).clamp(0, area.width);
    let height = ((area.height as f32 * height_pct) as u16).clamp(0, area.height);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_hidden_on_configuration_tab() {
        let overlay = SettingsOverlay::new();
        assert!(!overlay.is_visible());
        assert_eq!(overlay.tab(), SettingsTab::Configuration);
    }

    #[test]
    fn toggle_flips_visibility() {
        let mut overlay = SettingsOverlay::new();
        overlay.toggle();
        assert!(overlay.is_visible());
        overlay.toggle();
        assert!(!overlay.is_visible());
    }

    #[test]
    fn navigation_keys_cycle_tabs_and_wrap() {
        let mut overlay = SettingsOverlay::new();
        overlay.toggle();

        // Forward wraps from the last tab back to the first.
        for _ in 0..SettingsTab::ALL.len() {
            overlay.handle_key(KeyCode::Char(']'));
        }
        assert_eq!(overlay.tab(), SettingsTab::Configuration);

        // Backward from the first tab wraps to the last.
        overlay.handle_key(KeyCode::Char('['));
        assert_eq!(overlay.tab(), *SettingsTab::ALL.last().unwrap());
    }

    #[test]
    fn esc_and_comma_close_overlay() {
        let mut overlay = SettingsOverlay::new();

        overlay.toggle();
        assert!(overlay.handle_key(KeyCode::Esc));
        assert!(!overlay.is_visible());

        overlay.toggle();
        assert!(overlay.handle_key(KeyCode::Char(',')));
        assert!(!overlay.is_visible());
    }

    #[test]
    fn handle_key_is_modal_and_swallows_all_keys() {
        let mut overlay = SettingsOverlay::new();
        overlay.toggle();
        // An unrelated key is still consumed while the overlay is open.
        assert!(overlay.handle_key(KeyCode::Char('q')));
        assert!(overlay.is_visible());
    }
}
