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
    state::{AppMode, AppState},
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
    let db = Workspace::open_primary(None)?;

    // Setup event receiver
    let (mut event_rx, event_handler) = codemark_tui::event::EventHandler::with_receiver(
        EventHandlerConfig::default().tick_rate(Duration::from_millis(100)),
    )?;

    // Create the browser layout
    let mut layout = BrowserLayout::new(db, event_handler);

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
                let bindings = layout.get_help_bindings();
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
                // Convert &[(&str, &str)] to &[(&str, &str)] - compatible types
                let bindings_refs: Vec<(&str, &str)> =
                    bindings.iter().map(|(k, v)| (*k, *v)).collect();
                ui::render_help_panel(help_area, f.buffer_mut(), &bindings_refs);
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
                let mode_before = state.mode();

                match &event {
                    Event::Key(key) => {
                        match state.mode() {
                            AppMode::Normal => {
                                // Handle global key bindings (disabled when Search is focused)
                                let search_focused =
                                    layout.focus() == codemark_tui::browser::FocusArea::Search;
                                match key.code {
                                    event::KeyCode::Char('q') if !search_focused => {
                                        state.quit();
                                        handled = true;
                                    }
                                    event::KeyCode::Char('?') if !search_focused => {
                                        show_help = !show_help;
                                        handled = true;
                                    }
                                    event::KeyCode::Char('/') => {
                                        // Set the filter target based on current focus before entering filter mode
                                        let filter_target = match layout.focus() {
                                            codemark_tui::browser::FocusArea::Panel1 => "panel1",
                                            codemark_tui::browser::FocusArea::Panel2 => "panel2",
                                            codemark_tui::browser::FocusArea::Panel3 => "panel3",
                                            codemark_tui::browser::FocusArea::Main => "main",
                                            codemark_tui::browser::FocusArea::Search => "panel1", // Search filters Panel1
                                            codemark_tui::browser::FocusArea::Filter => "panel3",
                                        };
                                        state.set_string("filter_target", filter_target);
                                    }
                                    event::KeyCode::Esc => {
                                        if show_help {
                                            show_help = false;
                                            handled = true;
                                        } else if notification.is_some() {
                                            notification = None;
                                            handled = true;
                                        } else {
                                            // Clear the filter for the currently focused panel
                                            let filter_key = match layout.focus() {
                                                codemark_tui::browser::FocusArea::Panel1 => {
                                                    "active_filter_panel1"
                                                }
                                                codemark_tui::browser::FocusArea::Panel2 => {
                                                    "active_filter_panel2"
                                                }
                                                codemark_tui::browser::FocusArea::Panel3 => {
                                                    "active_filter_panel3"
                                                }
                                                codemark_tui::browser::FocusArea::Main => {
                                                    "active_filter_main"
                                                }
                                                codemark_tui::browser::FocusArea::Search => {
                                                    "active_filter_panel1"
                                                }
                                                codemark_tui::browser::FocusArea::Filter => {
                                                    "active_filter_panel3"
                                                }
                                            };
                                            state.set_string(filter_key, "");
                                            // Also clear the displayed filter_buffer for UI
                                            state.set_string("filter_buffer", "");
                                            handled = true;
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            AppMode::Command => {}
                            _ => {}
                        }
                    }
                    Event::Resize(width, height) => {
                        state.set_size(*width, *height);
                    }
                    _ => {}
                }

                // If not handled by global keys, pass to layout (includes mouse events)
                // Skip when help is shown to make help modal
                // In input modes (Command/Search/Insert), only pass mouse events to layout
                // so that Esc can be handled by state to exit the mode
                let is_input_mode =
                    matches!(state.mode(), AppMode::Command | AppMode::Search | AppMode::Insert);
                let is_mouse_event = matches!(event, Event::Mouse(_));
                if !handled && !show_help && (!is_input_mode || is_mouse_event) {
                    handled = layout.handle_event(&event);
                }

                // Let state handle the event too (captures keys for Search mode)
                // Skip when help is shown to make help modal
                // Skip if layout already handled the event (prevents keybinding conflicts)
                if !show_help && !handled {
                    state.handle_event(&event);
                }

                // Track mode transitions to manage focus
                let mode_after = state.mode();
                if mode_before != mode_after {
                    match (mode_before, mode_after) {
                        (AppMode::Normal, AppMode::Search) | (AppMode::Normal, AppMode::Insert) => {
                            // Entering filter/input mode
                            layout.enter_filter_mode();
                        }
                        (AppMode::Search, AppMode::Normal) | (AppMode::Insert, AppMode::Normal) => {
                            // Exiting filter/input mode
                            layout.exit_filter_mode();
                        }
                        _ => {}
                    }
                }

                // Update filter based on the focused panel's active_filter (live updated on each keystroke)
                // When in Search mode, use the filter_target to determine which panel's filter to apply.
                // Otherwise, use the current layout focus.
                let filter_key = if state.mode() == AppMode::Search {
                    // In Search mode, use the filter_target set when entering filter mode
                    let target = state.get_string("filter_target").unwrap_or("panel3");
                    format!("active_filter_{}", target)
                } else {
                    // In Normal mode, use the current layout focus
                    match layout.focus() {
                        codemark_tui::browser::FocusArea::Panel1 => "active_filter_panel1".to_string(),
                        codemark_tui::browser::FocusArea::Panel2 => "active_filter_panel2".to_string(),
                        codemark_tui::browser::FocusArea::Panel3 => "active_filter_panel3".to_string(),
                        codemark_tui::browser::FocusArea::Main => "active_filter_main".to_string(),
                        _ => "active_filter_panel3".to_string(),
                    }
                };
                let query = state.get_string(&filter_key).unwrap_or("");
                layout.apply_filter(query);

                // Handle external commands (e.g. Open in Editor)
                if let Some(cmd) = layout.take_pending_command() {
                    if cmd.should_wait {
                        // Terminal editor: exit TUI and replace process
                        restore_terminal();

                        #[cfg(unix)]
                        {
                            use std::os::unix::process::CommandExt;
                            let err =
                                std::process::Command::new(&cmd.program).args(&cmd.args).exec();

                            // If exec returns, it failed
                            eprintln!("Failed to run editor: {}", err);
                            std::process::exit(1);
                        }

                        #[cfg(not(unix))]
                        {
                            match std::process::Command::new(&cmd.program).args(&cmd.args).status()
                            {
                                Ok(status) if status.success() => std::process::exit(0),
                                Ok(status) => {
                                    let code = status.code().unwrap_or(1);
                                    eprintln!("Editor exited with status {}", code);
                                    std::process::exit(code);
                                }
                                Err(e) => {
                                    eprintln!("Failed to run editor: {}", e);
                                    std::process::exit(1);
                                }
                            }
                        }
                    } else {
                        // GUI editor: spawn in background
                        let status =
                            std::process::Command::new(&cmd.program).args(&cmd.args).spawn();

                        if let Err(e) = status {
                            notification = Some((
                                format!("Failed to spawn editor: {}", e),
                                NotificationType::Error,
                            ));
                        }
                    }
                }

                // Handle heal notifications
                if let Some(heal_notif) = layout.take_pending_notification() {
                    notification = Some((
                        heal_notif.message,
                        if heal_notif.success {
                            NotificationType::Info
                        } else {
                            NotificationType::Error
                        },
                    ));
                }
            }
        } else {
            // Event channel closed, quit the app
            state.quit();
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
