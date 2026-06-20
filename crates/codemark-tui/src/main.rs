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
use std::time::{Duration, Instant};

use codemark_tui::{
    browser::BrowserLayout,
    event::{Event, EventHandlerConfig},
    state::{AppMode, AppState},
    ui::{self, NotificationType},
};

/// Main entry point for the TUI application.
#[tokio::main]
async fn main() -> Result<()> {
    // `--list-themes` prints every known theme name (one per line) and exits,
    // without entering the alternate screen; `--list-schemes` prints only the
    // base16/base24 schemes (which re-theme the whole TUI chrome). Used by the
    // screenshot tooling to drive a per-theme capture loop, and handy for
    // discovering theme names.
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--list-themes" || a == "--list-schemes") {
        let registry = codemark_tui::theme::ThemeRegistry::new();
        let names = if args.iter().any(|a| a == "--list-schemes") {
            registry.scheme_names()
        } else {
            registry.available()
        };
        for name in names {
            println!("{name}");
        }
        return Ok(());
    }

    // Initialize file-based logging before anything else
    let _log_guard = codemark_tui::logging::init_logging();

    // Setup panic handler to restore terminal on panic
    setup_panic_handler();

    // Create and run the app
    let result = run_app().await;

    // Ensure terminal is restored
    restore_terminal();

    // Handle editor exit codes: drop the log guard first so buffered
    // tracing::error! entries are flushed before the process terminates.
    match result {
        Ok(Some(exit_code)) => {
            drop(_log_guard);
            std::process::exit(exit_code);
        }
        Ok(None) => Ok(()),
        Err(e) => {
            drop(_log_guard);
            Err(e)
        }
    }
}

/// Run the main application.
///
/// Returns `Ok(None)` for normal exit, `Ok(Some(code))` when the process
/// should terminate with a specific exit code (e.g. after spawning a terminal editor).
async fn run_app() -> Result<Option<i32>> {
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

    // Apply the configured TUI theme to code previews before any are created.
    // Theme is a user preference resolved from the layered config (global +
    // per-repo override) using the primary database's `.codemark` directory.
    codemark_tui::theme::ensure_default_themes_exist();
    let config = match db.path().parent() {
        Some(codemark_dir) => codemark_core::config::Config::load_layered(codemark_dir),
        None => codemark_core::config::Config::default(),
    };
    // Theme precedence: the CODEMARK_TUI_THEME env var (used by the screenshot
    // tooling and for ad-hoc previewing) overrides the layered config value.
    let theme_name = std::env::var("CODEMARK_TUI_THEME")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| config.tui.theme.clone());
    if let Some(theme_name) = theme_name.as_deref() {
        // Resolve the preview theme and the chrome palette from one source so the
        // whole TUI is cohesive (base16 schemes drive both from a single palette).
        let registry = codemark_tui::theme::ThemeRegistry::new();
        let (theme, palette) = registry.resolve_full(theme_name);
        codemark_tui::theme::set_palette(palette);
        codemark_tui::component::code_preview::set_default_theme(theme);
    }

    // Create the browser layout
    let mut layout = BrowserLayout::new(db, event_handler);

    // Notification state (message, type, when it was shown)
    let mut notification: Option<(String, NotificationType, Instant)> = None;
    let notification_duration = Duration::from_secs(2);

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
            if let Some((msg, notification_type, _)) = &notification {
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
                                            // Clear the active filter for the focused panel
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
                                            if state
                                                .get_string(filter_key)
                                                .is_some_and(|q| !q.is_empty())
                                            {
                                                state.set_string(filter_key, "");
                                                handled = true;
                                            }
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
                    Event::Tick => {
                        if let Some((_, _, shown_at)) = &notification
                            && shown_at.elapsed() >= notification_duration
                        {
                            notification = None;
                        }
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

                // A tab switch clears that pane's stored filter so it doesn't
                // carry over to the newly active tab (filters are pane-scoped
                // and applied to whichever tab is active).
                for target in layout.take_filter_targets_to_clear() {
                    state.set_string(format!("active_filter_{target}"), "");
                    state.set_string(format!("filter_buffer_{target}"), "");
                    if state.get_string("filter_target") == Some(target) {
                        state.set_string("filter_buffer", "");
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
                        codemark_tui::browser::FocusArea::Panel1 => {
                            "active_filter_panel1".to_string()
                        }
                        codemark_tui::browser::FocusArea::Panel2 => {
                            "active_filter_panel2".to_string()
                        }
                        codemark_tui::browser::FocusArea::Panel3 => {
                            "active_filter_panel3".to_string()
                        }
                        codemark_tui::browser::FocusArea::Main => "active_filter_main".to_string(),
                        codemark_tui::browser::FocusArea::Search => {
                            "active_filter_panel1".to_string()
                        }
                        codemark_tui::browser::FocusArea::Filter => {
                            "active_filter_panel3".to_string()
                        }
                    }
                };
                let query = state.get_string(&filter_key).unwrap_or("");
                layout.apply_filter(query);

                // Handle external commands (e.g. Open in Editor)
                if let Some(cmd) = layout.take_pending_command() {
                    tracing::info!(
                        target: "codemark::shell",
                        program = %cmd.program,
                        args = ?cmd.args,
                        wait = cmd.should_wait,
                        "spawning external command"
                    );

                    if cmd.should_wait {
                        // Terminal editor: exit TUI and replace process
                        restore_terminal();

                        #[cfg(unix)]
                        {
                            use std::os::unix::process::CommandExt;
                            let err =
                                std::process::Command::new(&cmd.program).args(&cmd.args).exec();

                            // If exec returns, it failed
                            tracing::error!(target: "codemark::shell", error = %err, "failed to exec editor");
                            eprintln!("Failed to run editor: {}", err);
                            return Ok(Some(1));
                        }

                        #[cfg(not(unix))]
                        {
                            match std::process::Command::new(&cmd.program).args(&cmd.args).status()
                            {
                                Ok(status) if status.success() => return Ok(Some(0)),
                                Ok(status) => {
                                    let code = status.code().unwrap_or(1);
                                    tracing::error!(target: "codemark::shell", code, "editor exited with non-zero status");
                                    eprintln!("Editor exited with status {}", code);
                                    return Ok(Some(code));
                                }
                                Err(e) => {
                                    tracing::error!(target: "codemark::shell", error = %e, "failed to spawn editor");
                                    eprintln!("Failed to run editor: {}", e);
                                    return Ok(Some(1));
                                }
                            }
                        }
                    } else {
                        // GUI editor: spawn in background
                        let status =
                            std::process::Command::new(&cmd.program).args(&cmd.args).spawn();

                        if let Err(e) = status {
                            tracing::error!(target: "codemark::shell", error = %e, "failed to spawn editor");
                            notification = Some((
                                format!("Failed to spawn editor: {}", e),
                                NotificationType::Error,
                                Instant::now(),
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
                        Instant::now(),
                    ));
                }
            }
        } else {
            // Event channel closed, quit the app
            state.quit();
        }
    }

    Ok(None)
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
