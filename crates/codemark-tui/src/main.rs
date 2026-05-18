//! Codemark TUI - Terminal interface for Codemark.
//!
//! This is the binary entry point for the TUI application.

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::io;
use std::time::Duration;

use codemark_tui::{
    browser::BrowserLayout,
    event::{Event, EventHandlerConfig},
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

    // Initialize Codemark Core Database
    use codemark_core::storage::Workspace;
    let db = Workspace::open_primary()?;

    // Create the browser layout
    let mut layout = BrowserLayout::new(db);

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
                    &layout.get_status_bindings(),
                    Some(layout.get_status_metadata()),
                    state.get_string("filter_buffer"),
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
                    &layout.get_status_bindings(),
                    Some(layout.get_status_metadata()),
                    state.get_string("filter_buffer"),
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

        // Handle events (blocking wait for next event)
        if let Some(event) = event_rx.recv().await {
            let mut events = vec![event];

            // Drain any other pending events to process them all before next draw
            while let Ok(ev) = event_rx.try_recv() {
                events.push(ev);
            }

            for event in events {
                let mut handled = false;

                match &event {
                    Event::Key(key) => {
                        match state.mode() {
                            AppMode::Normal => {
                                // Handle global key bindings
                                match key.code {
                                    event::KeyCode::Char('q') => {
                                        state.quit();
                                        handled = true;
                                    }
                                    event::KeyCode::Char('?') => {
                                        show_help = !show_help;
                                        handled = true;
                                    }
                                    event::KeyCode::Esc => {
                                        if show_help {
                                            show_help = false;
                                            handled = true;
                                        } else if notification.is_some() {
                                            notification = None;
                                            handled = true;
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            AppMode::Command => {
                                if key.code == event::KeyCode::Esc {
                                    state.set_mode(AppMode::Normal);
                                    handled = true;
                                }
                            }
                            _ => {
                                if key.code == event::KeyCode::Esc {
                                    state.set_mode(AppMode::Normal);
                                    handled = true;
                                }
                            }
                        }
                    }
                    Event::Resize(width, height) => {
                        state.set_size(*width, *height);
                    }
                    _ => {}
                }

                // If not handled by global keys, pass to layout (includes mouse events)
                if !handled {
                    layout.handle_event(&event);
                }

                // Let state handle the event too (captures keys for Search mode)
                state.handle_event(&event);

                // Update filter based on active_filter (committed via Enter)
                let query = state.get_string("active_filter").unwrap_or("");
                layout.apply_filter(query);
            }
        }
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
    let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
}
