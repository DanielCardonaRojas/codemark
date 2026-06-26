use crate::theme::ThemeRegistry;
use codemark_core::config::global_config_dir;
use crossterm::event::KeyCode;
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Clear, List, ListItem, ListState, Paragraph, Widget, Wrap},
};

#[derive(Debug, PartialEq, Eq)]
pub enum SettingsAction {
    Handled,
    Unhandled,
    ThemeChanged,
    /// Surface a transient message to the user (e.g. a failed save or open).
    Notify(String),
}

/// The tabs available in the settings overlay, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    /// Important paths: database, registry, config, data/templates/models dirs,
    /// logs, plus the embeddings model and theme.
    Configuration,
    /// Theme selector.
    Theme,
    /// Version and project links (repository, support).
    About,
}

impl SettingsTab {
    /// Every tab, in the order they appear in the tab bar.
    pub const ALL: &'static [SettingsTab] =
        &[SettingsTab::Configuration, SettingsTab::Theme, SettingsTab::About];

    /// The label shown for this tab in the tab bar.
    fn title(self) -> &'static str {
        match self {
            SettingsTab::Configuration => "Configuration",
            SettingsTab::Theme => "Theme",
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
pub struct SettingsOverlay {
    visible: bool,
    tab: SettingsTab,
    theme_registry: ThemeRegistry,
    available_themes: Vec<String>,
    selected_theme: usize,
    saved_theme: Option<String>,
    selected_about_link: usize,
    selected_config_row: usize,
}

impl Default for SettingsOverlay {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsOverlay {
    /// Create a hidden overlay defaulting to the Configuration tab.
    pub fn new() -> Self {
        let theme_registry = ThemeRegistry::new();
        let available_themes = theme_registry.available();

        let saved_theme = match global_config_dir() {
            Some(config_dir) => {
                let path = config_dir.join("config.toml");
                let content = std::fs::read_to_string(&path).unwrap_or_default();
                if let Ok(table) = content.parse::<toml::Table>() {
                    table
                        .get("tui")
                        .and_then(|tui| tui.as_table())
                        .and_then(|tui| tui.get("theme"))
                        .and_then(|theme| theme.as_str())
                        .map(|s| s.to_string())
                } else {
                    None
                }
            }
            None => None,
        };

        // Determine the selected theme index from the saved theme, or use default
        let selected_theme = available_themes
            .iter()
            .position(|t| Some(t) == saved_theme.as_ref())
            .unwrap_or_else(|| {
                // If not found, see if we can find the fallback theme
                available_themes.iter().position(|t| t == crate::theme::FALLBACK_THEME).unwrap_or(0)
            });

        Self {
            visible: false,
            tab: SettingsTab::Configuration,
            theme_registry,
            available_themes,
            selected_theme,
            saved_theme,
            selected_about_link: 0,
            selected_config_row: 0,
        }
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
    pub fn toggle(&mut self) -> SettingsAction {
        self.visible = !self.visible;
        if !self.visible {
            return self.hide();
        }
        SettingsAction::Handled
    }

    /// Hide the overlay and revert theme if un-saved.
    pub fn hide(&mut self) -> SettingsAction {
        self.visible = false;

        // Revert to saved theme when closing, returning ThemeChanged if it differs
        // from what's currently active (previewed).
        let current_preview = self.available_themes.get(self.selected_theme).cloned();

        // Reset selected index
        self.selected_theme = self
            .available_themes
            .iter()
            .position(|t| Some(t) == self.saved_theme.as_ref())
            .unwrap_or_else(|| {
                self.available_themes
                    .iter()
                    .position(|t| t == crate::theme::FALLBACK_THEME)
                    .unwrap_or(0)
            });

        let actual_saved = self.available_themes.get(self.selected_theme).cloned();

        if current_preview != actual_saved {
            self.apply_preview_theme();
            return SettingsAction::ThemeChanged;
        }
        SettingsAction::Handled
    }

    fn next_tab(&mut self) {
        self.tab = SettingsTab::at(self.tab.index() + 1);
    }

    fn prev_tab(&mut self) {
        // + len keeps the index non-negative before the modulo in `at`.
        self.tab = SettingsTab::at(self.tab.index() + SettingsTab::ALL.len() - 1);
    }

    fn apply_preview_theme(&self) {
        if let Some(theme_name) = self.available_themes.get(self.selected_theme) {
            let (theme, palette) = self.theme_registry.resolve_full(theme_name);
            crate::theme::set_palette(palette);
            crate::component::code_preview::set_default_theme(theme);
        }
    }

    /// Persist the selected theme to the global `config.toml`'s `tui.theme` key.
    ///
    /// Edits the file in place with `toml_edit` so existing comments and
    /// formatting are preserved, and creates the config directory if it doesn't
    /// exist yet. `saved_theme` is only updated once the write actually
    /// succeeds; on failure an `Err(message)` is returned for the caller to
    /// surface, so a discarded error can't leave the in-memory state claiming a
    /// theme is persisted when it isn't.
    fn save_selected_theme(&mut self) -> Result<(), String> {
        let Some(theme_name) = self.available_themes.get(self.selected_theme).cloned() else {
            return Ok(());
        };
        let Some(config_dir) = global_config_dir() else {
            return Err("Could not resolve the global config directory".to_string());
        };

        std::fs::create_dir_all(&config_dir)
            .map_err(|e| format!("Failed to create config directory: {e}"))?;

        let path = config_dir.join("config.toml");
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        let mut doc = content
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| format!("Failed to parse config.toml: {e}"))?;

        doc["tui"]["theme"] = toml_edit::value(theme_name.clone());

        std::fs::write(&path, doc.to_string())
            .map_err(|e| format!("Failed to write config.toml: {e}"))?;

        self.saved_theme = Some(theme_name);
        Ok(())
    }

    /// Handle a key while the overlay is open.
    pub fn handle_key(
        &mut self,
        code: KeyCode,
        config: &[(&'static str, String, Option<String>)],
    ) -> SettingsAction {
        match code {
            KeyCode::Esc | KeyCode::Char(',') => {
                return self.hide();
            }
            KeyCode::Char(']') | KeyCode::Right | KeyCode::Tab => self.next_tab(),
            KeyCode::Char('[') | KeyCode::Left | KeyCode::BackTab => self.prev_tab(),
            KeyCode::Char('j') | KeyCode::Down => {
                if self.tab == SettingsTab::Theme {
                    let max = self.available_themes.len().saturating_sub(1);
                    if self.selected_theme < max {
                        self.selected_theme += 1;
                        self.apply_preview_theme();
                        return SettingsAction::ThemeChanged;
                    }
                } else if self.tab == SettingsTab::About {
                    if self.selected_about_link < 1 {
                        self.selected_about_link += 1;
                    }
                } else if self.tab == SettingsTab::Configuration {
                    let max = config.len().saturating_sub(1);
                    if self.selected_config_row < max {
                        self.selected_config_row += 1;
                    }
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if self.tab == SettingsTab::Theme {
                    if self.selected_theme > 0 {
                        self.selected_theme -= 1;
                        self.apply_preview_theme();
                        return SettingsAction::ThemeChanged;
                    }
                } else if self.tab == SettingsTab::About {
                    if self.selected_about_link > 0 {
                        self.selected_about_link -= 1;
                    }
                } else if self.tab == SettingsTab::Configuration && self.selected_config_row > 0 {
                    self.selected_config_row -= 1;
                }
            }
            KeyCode::Enter => {
                if self.tab == SettingsTab::Theme {
                    if let Err(msg) = self.save_selected_theme() {
                        return SettingsAction::Notify(msg);
                    }
                } else if self.tab == SettingsTab::About {
                    let url = if self.selected_about_link == 0 {
                        env!("CARGO_PKG_REPOSITORY")
                    } else {
                        "https://github.com/sponsors/DanielCardonaRojas"
                    };
                    if let Err(e) = open::that(url) {
                        return SettingsAction::Notify(format!("Failed to open {url}: {e}"));
                    }
                } else if self.tab == SettingsTab::Configuration
                    && let Some((_, _, Some(path))) = config.get(self.selected_config_row)
                    && let Err(e) = open::that(path)
                {
                    return SettingsAction::Notify(format!("Failed to open {path}: {e}"));
                }
            }
            _ => {}
        }
        SettingsAction::Handled
    }

    /// Render the overlay as a centered modal over `area`.
    pub fn render(
        &self,
        area: Rect,
        buf: &mut Buffer,
        config: &[(&'static str, String, Option<String>)],
    ) {
        let popup = centered_rect(area, 0.55, 0.7);
        Widget::render(Clear, popup, buf);

        let palette = crate::theme::palette();

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

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(1)])
            .split(inner);

        match self.tab {
            SettingsTab::Configuration => {
                render_configuration(chunks[0], buf, config, self.selected_config_row)
            }
            SettingsTab::Theme => {
                render_theme(chunks[0], buf, &self.available_themes, self.selected_theme)
            }
            SettingsTab::About => render_about(chunks[0], buf, self.selected_about_link),
        }

        let hint = match self.tab {
            SettingsTab::Theme => Line::from(vec![
                Span::styled("j/k", Style::default().fg(palette.accent).bold()),
                Span::styled(" preview theme    ", Style::default().fg(palette.dim)),
                Span::styled("Enter", Style::default().fg(palette.accent).bold()),
                Span::styled(" save    ", Style::default().fg(palette.dim)),
                Span::styled("Esc", Style::default().fg(palette.accent).bold()),
                Span::styled(" close/revert", Style::default().fg(palette.dim)),
            ])
            .alignment(Alignment::Center),
            SettingsTab::About | SettingsTab::Configuration => Line::from(vec![
                Span::styled("j/k", Style::default().fg(palette.accent).bold()),
                Span::styled(" select link    ", Style::default().fg(palette.dim)),
                Span::styled("Enter", Style::default().fg(palette.accent).bold()),
                Span::styled(" open link    ", Style::default().fg(palette.dim)),
                Span::styled("[ / ]", Style::default().fg(palette.accent).bold()),
                Span::styled(" switch tabs    ", Style::default().fg(palette.dim)),
                Span::styled("Esc", Style::default().fg(palette.accent).bold()),
                Span::styled(" close", Style::default().fg(palette.dim)),
            ])
            .alignment(Alignment::Center),
        };
        Paragraph::new(hint).render(chunks[1], buf);
    }
}

fn render_theme(area: Rect, buf: &mut Buffer, themes: &[String], selected: usize) {
    let palette = crate::theme::palette();
    let items: Vec<ListItem> = themes
        .iter()
        .enumerate()
        .map(|(i, name)| {
            if i == selected {
                ListItem::new(Line::from(vec![
                    Span::styled("  \u{276f} ", Style::default().fg(palette.marker).bold()),
                    Span::styled(name.to_string(), Style::default().fg(palette.emphasis).bold()),
                ]))
            } else {
                ListItem::new(Line::from(vec![
                    Span::raw("    "),
                    Span::styled(name.to_string(), Style::default().fg(palette.dim)),
                ]))
            }
        })
        .collect();

    let list = List::new(items);
    let mut state = ListState::default();
    state.select(Some(selected));

    ratatui::widgets::StatefulWidget::render(list, area, buf, &mut state);
}

/// Render the Configuration tab: a `label : value` list of important paths and
/// settings, with labels aligned to the widest one.
fn render_configuration(
    area: Rect,
    buf: &mut Buffer,
    rows: &[(&'static str, String, Option<String>)],
    selected_row: usize,
) {
    let palette = crate::theme::palette();
    let label_width = rows.iter().map(|(label, _, _)| label.len()).max().unwrap_or(0);

    let mut text = Text::default();
    for (i, (label, value, has_path)) in rows.iter().enumerate() {
        let is_selected = i == selected_row;
        let is_link = has_path.is_some();

        let marker = if is_selected {
            Span::styled("\u{276f} ", Style::default().fg(palette.marker).bold())
        } else {
            Span::raw("  ")
        };

        // The label colour is constant; only the value/marker reflect selection.
        let label_style = Style::default().fg(palette.accent).bold();

        let value_style = if is_selected {
            if is_link {
                Style::default().fg(palette.inverse).bg(palette.accent).bold()
            } else {
                Style::default().fg(palette.emphasis).bold()
            }
        } else if is_link {
            Style::default().fg(palette.emphasis).underlined()
        } else {
            Style::default().fg(palette.emphasis)
        };

        text.push_line(Line::from(vec![
            marker,
            Span::styled(format!("{label:<label_width$}  "), label_style),
            Span::styled(value.clone(), value_style),
        ]));
    }
    Paragraph::new(text).wrap(Wrap { trim: false }).render(area, buf);
}

/// Render the About tab: version and project links.
fn render_about(area: Rect, buf: &mut Buffer, selected_link: usize) {
    let palette = crate::theme::palette();

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

    // Version is not a link
    text.push_line(Line::from(vec![
        Span::raw("  "),
        Span::styled(format!("{:<10}  ", "Version"), Style::default().fg(palette.accent).bold()),
        Span::styled(crate::VERSION, Style::default().fg(palette.emphasis)),
    ]));

    let links = [
        ("Repository", env!("CARGO_PKG_REPOSITORY")),
        ("Support", "https://github.com/sponsors/DanielCardonaRojas"),
    ];

    for (i, (name, url)) in links.iter().enumerate() {
        let value_style = if i == selected_link {
            Style::default().fg(palette.inverse).bg(palette.accent).bold()
        } else {
            Style::default().fg(palette.emphasis).underlined()
        };

        let marker = if i == selected_link {
            Span::styled("\u{276f} ", Style::default().fg(palette.marker).bold())
        } else {
            Span::raw("  ")
        };

        let link_label =
            Span::styled(format!("{name:<10}  "), Style::default().fg(palette.accent).bold());

        text.push_line(Line::from(vec![
            marker,
            link_label,
            Span::styled(url.to_string(), value_style),
        ]));
    }

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
            assert_eq!(overlay.handle_key(KeyCode::Char(']'), &[]), SettingsAction::Handled);
        }
        assert_eq!(overlay.tab(), SettingsTab::Configuration);

        // Backward from the first tab wraps to the last.
        assert_eq!(overlay.handle_key(KeyCode::Char('['), &[]), SettingsAction::Handled);
        assert_eq!(overlay.tab(), *SettingsTab::ALL.last().unwrap());
    }

    #[test]
    fn esc_and_comma_close_overlay() {
        let mut overlay = SettingsOverlay::new();

        overlay.toggle();
        assert_eq!(overlay.handle_key(KeyCode::Esc, &[]), SettingsAction::Handled);
        assert!(!overlay.is_visible());

        overlay.toggle();
        assert_eq!(overlay.handle_key(KeyCode::Char(','), &[]), SettingsAction::Handled);
        assert!(!overlay.is_visible());
    }

    #[test]
    fn handle_key_is_modal_and_swallows_all_keys() {
        let mut overlay = SettingsOverlay::new();
        overlay.toggle();
        // An unrelated key is still consumed while the overlay is open.
        assert_eq!(overlay.handle_key(KeyCode::Char('q'), &[]), SettingsAction::Handled);
        assert!(overlay.is_visible());
    }
}
