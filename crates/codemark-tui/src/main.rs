//! Codemark TUI - Terminal interface for Codemark.
//!
//! This is the binary entry point for the TUI application.

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::time::Duration;

use codemark_tui::{
    component::{Panel, PanelItem},
    event::{Event, EventHandlerConfig},
    layout::helpers,
    state::{AppMode, AppState, FocusManager},
    ui::{self, NotificationType},
};

/// Main entry point for the TUI application.
#[tokio::main]
async fn main() -> Result<()> {
    // Setup panic handler to restore terminal on panic
    setup_panic_handler();

    // Create and run the app
    let result = run_app().await;

    // Ensure terminal is restored
    restore_terminal();

    result
}

/// Run the main application.
async fn run_app() -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app state
    let mut state = AppState::new();

    // Create demo panels
    let left_panel = Panel::new("Navigation")
        .items(vec![
            PanelItem::new("Bookmarks").secondary_text("12 items"),
            PanelItem::new("Collections").secondary_text("5 collections"),
            PanelItem::new("Recent").secondary_text("3 items"),
            PanelItem::new("Settings").secondary_text(""),
            PanelItem::new("Help").secondary_text("Press ?"),
        ])
        .bordered(true);

    let main_panel = Panel::new("Bookmarks")
        .items(vec![
            PanelItem::new("src/main.rs").secondary_text("fn main").metadata("Rust"),
            PanelItem::new("src/lib.rs").secondary_text("pub fn process").metadata("Rust"),
            PanelItem::new("tests/test.rs").secondary_text("test case").metadata("Rust"),
            PanelItem::new("README.md").secondary_text("documentation").metadata("Markdown"),
            PanelItem::new("Cargo.toml").secondary_text("config").metadata("TOML"),
            PanelItem::new("src/cli.rs").secondary_text("fn parse_args").metadata("Rust"),
            PanelItem::new("src/config.rs").secondary_text("struct Config").metadata("Rust"),
        ])
        .bordered(true);

    let right_panel = Panel::new("Details")
        .items(vec![
            PanelItem::new("File:").secondary_text("src/main.rs"),
            PanelItem::new("Function:").secondary_text("fn main"),
            PanelItem::new("Lines:").secondary_text("1-25"),
            PanelItem::new("Language:").secondary_text("Rust"),
            PanelItem::new("Tags:").secondary_text("entry-point"),
            PanelItem::new("Created:").secondary_text("2024-01-15"),
        ])
        .bordered(true);

    // Create the main layout
    let mut layout = helpers::three_panel(left_panel, main_panel, right_panel);

    // Setup focus manager
    let mut focus_manager = FocusManager::new();
    focus_manager.add("left");
    focus_manager.add("main");
    focus_manager.add("right");

    // Setup event receiver
    let (mut event_rx, _) = codemark_tui::event::EventHandler::with_receiver(
        EventHandlerConfig::default().tick_rate(Duration::from_millis(100)),
    )?;

    // Notification state
    let mut notification: Option<(String, NotificationType)> = None;

    // Main loop
    let mut show_help = false;

    while state.is_running() {
        // Draw the UI
        terminal.draw(|f| {
            let size = f.area();

            if show_help {
                // Draw help overlay
                let bindings = ui::common_key_bindings();
                let help_width = 50.min(size.width.saturating_sub(4));
                let help_height = (bindings.len() as u16 + 4).min(size.height.saturating_sub(4));

                let help_area = ratatui::layout::Rect {
                    x: (size.width - help_width) / 2,
                    y: (size.height - help_height) / 2,
                    width: help_width,
                    height: help_height,
                };

                // Draw main layout in background
                let chunks = ratatui::layout::Layout::default()
                    .direction(ratatui::layout::Direction::Vertical)
                    .constraints([
                        ratatui::layout::Constraint::Min(0),
                        ratatui::layout::Constraint::Length(1),
                    ])
                    .split(size);

                layout.render(chunks[0], f.buffer_mut());

                ui::render_status_bar(
                    chunks[1],
                    f.buffer_mut(),
                    state.mode(),
                    notification.as_ref().map(|(msg, _)| msg.as_str()),
                );

                // Draw help overlay
                ui::render_help_panel(help_area, f.buffer_mut(), &bindings);
            } else {
                // Draw normal UI
                let chunks = ratatui::layout::Layout::default()
                    .direction(ratatui::layout::Direction::Vertical)
                    .constraints([
                        ratatui::layout::Constraint::Min(0),
                        ratatui::layout::Constraint::Length(1),
                    ])
                    .split(size);

                layout.render(chunks[0], f.buffer_mut());

                ui::render_status_bar(
                    chunks[1],
                    f.buffer_mut(),
                    state.mode(),
                    notification.as_ref().map(|(msg, _)| msg.as_str()),
                );
            }

            // Draw notification if present
            if let Some((msg, notification_type)) = &notification {
                let notif_area = ratatui::layout::Rect {
                    x: 0,
                    y: size.height.saturating_sub(1),
                    width: size.width,
                    height: 1,
                };
                ui::render_notification(notif_area, f.buffer_mut(), msg, *notification_type);
            }
        })?;

        // Handle events
        if let Ok(event) = event_rx.try_recv() {
            match &event {
                Event::Key(key) => {
                    match state.mode() {
                        AppMode::Normal => {
                            // Handle global key bindings
                            match key.code {
                                event::KeyCode::Char('q') => {
                                    state.quit();
                                }
                                event::KeyCode::Char('?') => {
                                    show_help = !show_help;
                                }
                                event::KeyCode::Char(':') => {
                                    state.set_mode(AppMode::Command);
                                }
                                event::KeyCode::Tab => {
                                    focus_manager.next();
                                    notification = Some((
                                        format!("Focus: {}", focus_manager.focused().unwrap_or("none")),
                                        NotificationType::Info,
                                    ));
                                }
                                event::KeyCode::BackTab => {
                                    focus_manager.previous();
                                    notification = Some((
                                        format!("Focus: {}", focus_manager.focused().unwrap_or("none")),
                                        NotificationType::Info,
                                    ));
                                }
                                event::KeyCode::Esc => {
                                    notification = None;
                                }
                                _ => {
                                    // Pass to layout
                                    layout.handle_event(&event);
                                }
                            }
                        }
                        AppMode::Command => {
                            if key.code == event::KeyCode::Esc {
                                state.set_mode(AppMode::Normal);
                            }
                        }
                        _ => {
                            if key.code == event::KeyCode::Esc {
                                state.set_mode(AppMode::Normal);
                            }
                        }
                    }
                }
                Event::Resize(width, height) => {
                    state.set_size(*width, *height);
                }
                _ => {}
            }

            // Let state handle the event too
            state.handle_event(&event);
        }

        // Small sleep to prevent busy-waiting
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    Ok(())
}

/// Setup a panic handler to restore the terminal.
fn setup_panic_handler() {
    std::panic::set_hook(Box::new(|panic_info| {
        restore_terminal();
        eprintln!("Panic: {}", panic_info);
    }));
}

/// Restore terminal state.
fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(
        io::stdout(),
        LeaveAlternateScreen,
        DisableMouseCapture
    );
}
