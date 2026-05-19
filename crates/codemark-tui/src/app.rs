//! Main application struct.
//!
//! The App struct ties together all the components of the TUI application
//! and manages the main event loop.

use std::io::{self, Stdout};
use std::time::Duration;

use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

use crate::component::{Panel, PanelItem};
use crate::event::{Event, EventHandler, EventHandlerConfig};
use crate::layout::{LayoutManager, helpers};
use crate::state::{AppState, FocusManager};
use crate::ui::{self, NotificationType};

/// The main application struct.
///
/// App manages the terminal UI, event handling, and component lifecycle.
pub struct App {
    /// The terminal
    terminal: Terminal<CrosstermBackend<Stdout>>,
    /// The application state
    state: AppState,
    /// The main layout
    layout: LayoutManager,
    /// Focus manager for component focus
    focus_manager: FocusManager,
    /// Current notification message (if any)
    notification: Option<(String, NotificationType)>,
    /// Whether help is currently displayed
    show_help: bool,
}

impl App {
    /// Create a new application instance.
    pub fn new() -> anyhow::Result<Self> {
        // Setup terminal
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;

        // Create initial layout with demo panels
        let layout = Self::create_default_layout();

        let mut app = Self {
            terminal,
            state: AppState::new(),
            layout,
            focus_manager: FocusManager::new(),
            notification: None,
            show_help: false,
        };

        // Setup focus manager
        app.focus_manager.add("left_panel");
        app.focus_manager.add("main_panel");
        app.focus_manager.add("right_panel");

        Ok(app)
    }

    /// Create the default layout for the application.
    fn create_default_layout() -> LayoutManager {
        // Create demo panels
        let left_panel = Panel::new("Navigation")
            .items(vec![
                PanelItem::new("Bookmarks").secondary_text("12 items"),
                PanelItem::new("Collections").secondary_text("5 collections"),
                PanelItem::new("Recent").secondary_text("3 items"),
                PanelItem::new("Settings").secondary_text(""),
            ])
            .bordered(true);

        let main_panel = Panel::new("Bookmarks")
            .items(vec![
                PanelItem::new("src/main.rs").secondary_text("fn main").metadata("Rust"),
                PanelItem::new("src/lib.rs").secondary_text("pub fn process").metadata("Rust"),
                PanelItem::new("tests/test.rs").secondary_text("test case").metadata("Rust"),
                PanelItem::new("README.md").secondary_text("documentation").metadata("Markdown"),
                PanelItem::new("Cargo.toml").secondary_text("config").metadata("TOML"),
            ])
            .bordered(true);

        let right_panel = Panel::new("Details")
            .items(vec![
                PanelItem::new("File:").secondary_text("src/main.rs"),
                PanelItem::new("Function:").secondary_text("fn main"),
                PanelItem::new("Lines:").secondary_text("1-25"),
                PanelItem::new("Language:").secondary_text("Rust"),
                PanelItem::new("Tags:").secondary_text("entry-point"),
            ])
            .bordered(true);

        helpers::three_panel(left_panel, main_panel, right_panel)
    }

    /// Run the application main loop.
    pub fn run(&mut self) -> anyhow::Result<()> {
        let (_event_handler, _) = EventHandler::with_receiver(EventHandlerConfig::default())?;

        // Note: In a real implementation, we'd get the receiver properly
        // For this scaffold, we'll do a simple version

        self.state.set_mode(crate::state::AppMode::Normal);

        // Initial focus
        if let Some(focused) = self.focus_manager.focused() {
            self.state.set_focus(focused);
        }

        // Main loop
        while self.state.is_running() {
            // Clone values needed for rendering
            let show_help = self.show_help;
            let layout = &self.layout;
            let state = &self.state;
            let notification = self.notification.clone();

            // Render
            self.terminal.draw(|f| {
                if show_help {
                    Self::render_help_static(f, layout);
                } else {
                    Self::render_main_static(f, layout, state);
                }

                // Render notification if present
                if let Some((msg, notification_type)) = &notification {
                    let size = f.area();
                    let notification_area = ratatui::layout::Rect {
                        x: 0,
                        y: size.height.saturating_sub(1),
                        width: size.width,
                        height: 1,
                    };
                    ui::render_notification(
                        notification_area,
                        f.buffer_mut(),
                        msg,
                        *notification_type,
                    );
                }
            })?;

            // Handle events
            // In a real implementation, we'd poll the event receiver here
            // For this scaffold, we'll do a simple tick-based approach

            std::thread::sleep(Duration::from_millis(100));

            // For demo purposes, we'll just check if we should quit
            // Real event handling would go here
        }

        Ok(())
    }

    /// Static version of render_main for use in draw closure.
    fn render_main_static(f: &mut ratatui::Frame, layout: &LayoutManager, state: &AppState) {
        let size = f.area();

        // Split into main content and status bar
        let chunks = ratatui::layout::Layout::default()
            .direction(ratatui::layout::Direction::Vertical)
            .constraints([
                ratatui::layout::Constraint::Min(0),
                ratatui::layout::Constraint::Length(1),
            ])
            .split(size);

        // Render the main layout
        layout.render(chunks[0], f.buffer_mut());

        // For the static version in the demo, we use placeholder bindings
        let bindings = vec![ui::KeyBinding::new("?", "Help"), ui::KeyBinding::new("q", "Quit")];

        // Render status bar
        ui::render_status_bar(
            chunks[1],
            f.buffer_mut(),
            state.mode(),
            &bindings,
            None,
            state.get_string("search_query"),
        );
    }

    /// Static version of render_help for use in draw closure.
    fn render_help_static(f: &mut ratatui::Frame, _layout: &LayoutManager) {
        let size = f.area();

        let bindings = ui::common_key_bindings();

        // Calculate help panel dimensions
        let width = 50.min(size.width.saturating_sub(4));
        let height = (bindings.len() as u16 + 4).min(size.height.saturating_sub(4));

        let help_area = ratatui::layout::Rect {
            x: (size.width - width) / 2,
            y: (size.height - height) / 2,
            width,
            height,
        };

        ui::render_help_panel(help_area, f.buffer_mut(), &bindings);
    }

    /// Handle an event.
    pub fn handle_event(&mut self, event: &Event) -> anyhow::Result<bool> {
        // Handle application-level events first
        if self.state.handle_event(event) {
            return Ok(true);
        }

        // Handle mode-specific events
        if self.state.mode() == crate::state::AppMode::Normal
            && let Event::Key(key) = event
        {
            match key.code {
                ratatui::crossterm::event::KeyCode::Char('?') => {
                    self.show_help = !self.show_help;
                    return Ok(true);
                }
                ratatui::crossterm::event::KeyCode::Tab => {
                    self.focus_manager.cycle_next();
                    if let Some(focused) = self.focus_manager.focused() {
                        self.state.set_focus(focused);
                        self.update_focus();
                    }
                    return Ok(true);
                }
                ratatui::crossterm::event::KeyCode::BackTab => {
                    self.focus_manager.previous();
                    if let Some(focused) = self.focus_manager.focused() {
                        self.state.set_focus(focused);
                        self.update_focus();
                    }
                    return Ok(true);
                }
                _ => {}
            }
        }

        // Handle events in layout components
        if self.layout.handle_event(event) {
            return Ok(true);
        }

        Ok(false)
    }

    /// Update focus state based on current focus.
    fn update_focus(&mut self) {
        let focused_id = self.focus_manager.focused().map(|s| s.to_owned());

        // Update focus in the layout
        self.layout.set_focus(false);

        // Set focus on the currently focused component
        if let Some(id) = focused_id {
            self.focus_component(&id);
        }
    }

    /// Focus a specific component by ID.
    fn focus_component(&mut self, _id: &str) {
        // This would traverse the layout and set focus on the matching component
        // For the scaffold, we'll do a simple implementation
        self.layout.set_focus(true);
    }

    /// Show a notification.
    pub fn notify(&mut self, message: impl Into<String>, notification_type: NotificationType) {
        self.notification = Some((message.into(), notification_type));
    }

    /// Clear any active notification.
    pub fn clear_notification(&mut self) {
        self.notification = None;
    }

    /// Quit the application.
    pub fn quit(&mut self) {
        self.state.quit();
    }
}

impl Drop for App {
    fn drop(&mut self) {
        // Restore terminal state
        disable_raw_mode().ok();
        execute!(self.terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture).ok();
    }
}

/// Builder for creating an App with custom configuration.
pub struct AppBuilder {
    tick_rate: Duration,
    enable_mouse: bool,
}

impl Default for AppBuilder {
    fn default() -> Self {
        Self { tick_rate: Duration::from_millis(250), enable_mouse: true }
    }
}

impl AppBuilder {
    /// Create a new app builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the tick rate.
    pub fn tick_rate(mut self, tick_rate: Duration) -> Self {
        self.tick_rate = tick_rate;
        self
    }

    /// Enable or disable mouse support.
    pub fn enable_mouse(mut self, enable: bool) -> Self {
        self.enable_mouse = enable;
        self
    }

    /// Build the app.
    pub fn build(self) -> anyhow::Result<App> {
        let _ = self.tick_rate; // Would be used in event handler config
        let _ = self.enable_mouse; // Would be used in event handler config
        App::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_builder() {
        let builder = AppBuilder::new().tick_rate(Duration::from_millis(100)).enable_mouse(false);

        assert_eq!(builder.tick_rate, Duration::from_millis(100));
        assert!(!builder.enable_mouse);
    }

    #[test]
    fn test_focus_manager() {
        let mut manager = FocusManager::new();
        manager.add("panel1");
        manager.add("panel2");

        assert_eq!(manager.focused(), Some("panel1"));

        manager.cycle_next();
        assert_eq!(manager.focused(), Some("panel2"));
    }
}
