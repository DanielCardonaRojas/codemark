use crossterm::event::KeyCode;
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Clear, Paragraph, Widget, Wrap, List, ListItem, ListState},
};
use codemark_core::config::global_config_dir;
use crate::theme::ThemeRegistry;

pub enum SettingsAction {
    Handled,
    Unhandled,
    ThemeChanged,
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
    pub const ALL: &'static [SettingsTab] = &[SettingsTab::Configuration, SettingsTab::Theme, SettingsTab::About];

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
#[derive(Debug)]
pub struct SettingsOverlay {
    visible: bool,
    tab: SettingsTab,
    theme_registry: ThemeRegistry,
    available_themes: Vec<String>,
    selected_theme: usize,
    saved_theme: Option<String>,
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
                    table.get("tui")
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
        let selected_theme = available_themes.iter()
            .position(|t| Some(t) == saved_theme.as_ref())
            .unwrap_or_else(|| {
                // If not found, see if we can find the fallback theme
                available_themes.iter()
                    .position(|t| t == crate::theme::FALLBACK_THEME)
                    .unwrap_or(0)
            });

        Self { 
            visible: false, 
            tab: SettingsTab::Configuration,
            theme_registry,
            available_themes,
            selected_theme,
            saved_theme,
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
        self.selected_theme = self.available_themes.iter()
            .position(|t| Some(t) == self.saved_theme.as_ref())
            .unwrap_or_else(|| {
                self.available_themes.iter()
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

    fn save_selected_theme(&mut self) {
        if let Some(theme_name) = self.available_themes.get(self.selected_theme).cloned() {
            if let Some(config_dir) = global_config_dir() {
                let path = config_dir.join("config.toml");
                let content = std::fs::read_to_string(&path).unwrap_or_default();
                let mut table = content.parse::<toml::Table>().unwrap_or_default();

                if let Some(toml::Value::Table(tui)) = table.get_mut("tui") {
                    tui.insert("theme".to_string(), toml::Value::String(theme_name.clone()));
                } else {
                    let mut tui_table = toml::Table::new();
                    tui_table.insert("theme".to_string(), toml::Value::String(theme_name.clone()));
                    table.insert("tui".to_string(), toml::Value::Table(tui_table));
                }

                if let Ok(new_content) = toml::to_string(&table) {
                    let _ = std::fs::write(&path, new_content);
                }
            }
            self.saved_theme = Some(theme_name);
        }
    }

    /// Handle a key while the overlay is open.
    pub fn handle_key(&mut self, code: KeyCode) -> SettingsAction {
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
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if self.tab == SettingsTab::Theme {
                    if self.selected_theme > 0 {
                        self.selected_theme -= 1;
                        self.apply_preview_theme();
                        return SettingsAction::ThemeChanged;
                    }
                }
            }
            KeyCode::Enter => {
                if self.tab == SettingsTab::Theme {
                    self.save_selected_theme();
                }
            }
            _ => {}
        }
        SettingsAction::Handled
    }

    /// Render the overlay as a centered modal over `area`.
    pub fn render(&self, area: Rect, buf: &mut Buffer, config: &[(&'static str, String)]) {
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
            SettingsTab::Configuration => render_configuration(chunks[0], buf, config),
            SettingsTab::Theme => render_theme(chunks[0], buf, &self.available_themes, self.selected_theme),
            SettingsTab::About => render_about(chunks[0], buf),
        }

        let hint = match self.tab {
            SettingsTab::Theme => {
                Line::from(vec![
                    Span::styled("j/k", Style::default().fg(palette.accent).bold()),
                    Span::styled(" preview theme    ", Style::default().fg(palette.dim)),
                    Span::styled("Enter", Style::default().fg(palette.accent).bold()),
                    Span::styled(" save    ", Style::default().fg(palette.dim)),
                    Span::styled("Esc", Style::default().fg(palette.accent).bold()),
                    Span::styled(" close/revert", Style::default().fg(palette.dim)),
                ])
                .alignment(Alignment::Center)
            }
            _ => {
                Line::from(vec![
                    Span::styled("[ / ]", Style::default().fg(palette.accent).bold()),
                    Span::styled(" switch tabs    ", Style::default().fg(palette.dim)),
                    Span::styled("Esc", Style::default().fg(palette.accent).bold()),
                    Span::styled(" close", Style::default().fg(palette.dim)),
                ])
                .alignment(Alignment::Center)
            }
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

