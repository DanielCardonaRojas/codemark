//! Event handling for the TUI.
//!
//! This module provides types and utilities for handling terminal events
//! including keyboard input, mouse events, and terminal resize events.

use codemark_core::engine::bookmark::{Bookmark, Collection};
use codemark_core::engine::resolution::LiveUIStatus;
use ratatui::crossterm::event::{Event as CrosstermEvent, KeyEvent, MouseEvent};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// Events that can be handled by the TUI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A key press event.
    Key(KeyEvent),
    /// A mouse event.
    Mouse(MouseEvent),
    /// A terminal resize event.
    Resize(u16, u16),
    /// A tick event (used for periodic updates).
    Tick,
    /// A focus gained event.
    FocusGained,
    /// A focus lost event.
    FocusLost,
    /// A paste event (clipboard content).
    Paste(String),
    /// Search results returned from background task.
    SearchResults { request_id: u64, bookmarks: Vec<Bookmark> },
    /// Collection search results returned from background task, paired with each
    /// collection's bookmark count.
    CollectionSearchResults { request_id: u64, collections: Vec<(Collection, usize)> },
    /// Search failed with an error message.
    SearchError { request_id: u64, msg: String },
    /// Heal operation completed with a notification message.
    HealComplete(String, bool), // (message, success)
    /// Sync operation (push/pull) completed with a notification message.
    SyncComplete(String, bool), // (message, success)
    /// Remote tours listing completed (tours, repos scope used for the request —
    /// the comma-joined `owner/name` set, for discarding stale responses).
    RemoteToursLoaded(Vec<codemark_core::sync::RemoteTourSummary>, Option<String>),
    /// Remote tours listing failed.
    RemoteToursFetchError(String),
    /// Background live health resolution results for bookmark list dots.
    /// Each entry is (bookmark_id, live_status).
    LiveHealthBatch(Vec<(String, LiveUIStatus)>),
    /// A bookmark preview finished resolving on a background task.
    /// `request_id` guards against applying stale results after the selection moved.
    PreviewReady {
        /// Monotonic id of the preview request this result answers.
        request_id: u64,
        /// Fully-computed preview, boxed to keep the enum small.
        payload: Box<crate::browser::PreviewPayload>,
    },
    /// A bookmark preview failed to resolve live (broken/missing); the UI falls
    /// back to the persisted resolution on the main thread.
    PreviewFailed {
        /// Monotonic id of the preview request this result answers.
        request_id: u64,
        /// Bookmark that failed to resolve.
        bookmark_id: String,
    },
    /// The Info/Details/Comments markdown for a collection step finished
    /// rendering on a background task. The code pane is updated inline on
    /// navigation; this carries only the expensive markdown so paging never
    /// blocks on git-ancestry projection + handlebars rendering.
    StepPreviewReady {
        /// Monotonic id of the render this result answers. Applied only if it
        /// still matches the right pane's current step-preview request, so an
        /// older render for the same bookmark can't overwrite a newer one after
        /// a reload or rapid re-navigation.
        request_id: u64,
        /// Bookmark the markdown was rendered for. A cheap secondary guard so a
        /// result for a step the user has already paged past is dropped.
        bookmark_id: String,
        /// Rendered Info/Details/Comments markdown, boxed to keep the enum small.
        markdown: Box<crate::browser::StepPreviewMarkdown>,
    },
}

impl Event {
    /// Check if this event should trigger a quit.
    pub fn is_quit(&self) -> bool {
        matches!(
            self,
            Event::Key(key) if key.code == ratatui::crossterm::event::KeyCode::Char('q')
                || key.code == ratatui::crossterm::event::KeyCode::Char('c')
                    && key.modifiers.contains(ratatui::crossterm::event::KeyModifiers::CONTROL)
        )
    }

    /// Check if this is a navigation event (up/down/left/right).
    pub fn is_navigation(&self) -> bool {
        matches!(
            self,
            Event::Key(key) if matches!(
                key.code,
                ratatui::crossterm::event::KeyCode::Up
                    | ratatui::crossterm::event::KeyCode::Down
                    | ratatui::crossterm::event::KeyCode::Left
                    | ratatui::crossterm::event::KeyCode::Right
                    | ratatui::crossterm::event::KeyCode::Char('j')
                    | ratatui::crossterm::event::KeyCode::Char('k')
                    | ratatui::crossterm::event::KeyCode::Char('h')
                    | ratatui::crossterm::event::KeyCode::Char('l')
            )
        )
    }

    /// Convert a crossterm event to our Event type.
    fn from_crossterm(event: CrosstermEvent) -> Option<Self> {
        match event {
            CrosstermEvent::Key(key) => Some(Event::Key(key)),
            CrosstermEvent::Mouse(mouse) => Some(Event::Mouse(mouse)),
            CrosstermEvent::Resize(width, height) => Some(Event::Resize(width, height)),
            CrosstermEvent::FocusGained => Some(Event::FocusGained),
            CrosstermEvent::FocusLost => Some(Event::FocusLost),
            CrosstermEvent::Paste(s) => Some(Event::Paste(s)),
        }
    }
}

/// Configuration for the event handler.
#[derive(Debug, Clone)]
pub struct EventHandlerConfig {
    /// The tick rate for generating Tick events.
    pub tick_rate: Duration,
    /// Whether to enable mouse events.
    pub enable_mouse: bool,
    /// Whether to enable paste events.
    pub enable_paste: bool,
    /// The channel capacity for events.
    pub channel_capacity: usize,
}

impl Default for EventHandlerConfig {
    fn default() -> Self {
        Self {
            tick_rate: Duration::from_millis(250),
            enable_mouse: true,
            enable_paste: true,
            channel_capacity: 1000,
        }
    }
}

impl EventHandlerConfig {
    /// Create a new event handler config.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the tick rate.
    pub fn tick_rate(mut self, tick_rate: Duration) -> Self {
        self.tick_rate = tick_rate;
        self
    }

    /// Enable or disable mouse events.
    pub fn enable_mouse(mut self, enable: bool) -> Self {
        self.enable_mouse = enable;
        self
    }

    /// Enable or disable paste events.
    pub fn enable_paste(mut self, enable: bool) -> Self {
        self.enable_paste = enable;
        self
    }

    /// Set the channel capacity.
    pub fn channel_capacity(mut self, capacity: usize) -> Self {
        self.channel_capacity = capacity;
        self
    }
}

/// A handle to the event handler that can be cloned across threads.
#[derive(Debug)]
pub struct EventHandler {
    _tx: Arc<mpsc::UnboundedSender<Event>>,
    // Optional receiver for EventHandler::new() users
    // Using Arc<Mutex<>> to allow cloning while preserving receiver access
    rx: Option<Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<Event>>>>,
}

// Clone implementation - the cloned handle shares the sender but not the receiver
// (only the original EventHandler created via new() can receive events)
impl Clone for EventHandler {
    fn clone(&self) -> Self {
        Self {
            _tx: Arc::clone(&self._tx),
            rx: None, // Clones don't get the receiver
        }
    }
}

impl EventHandler {
    /// Create a new event handler.
    pub fn new(config: EventHandlerConfig) -> anyhow::Result<Self> {
        let (tx, rx) = mpsc::unbounded_channel();

        // Enable mouse events if configured
        if config.enable_mouse {
            crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)?;
        }

        Ok(Self { _tx: Arc::new(tx), rx: Some(Arc::new(tokio::sync::Mutex::new(rx))) })
    }

    /// Get the next event.
    ///
    /// This method is async and will wait until an event is available.
    /// Note: Only works on the original EventHandler created via new(),
    /// not on clones. For better control, use EventHandler::with_receiver().
    pub async fn next(&self) -> anyhow::Result<Event> {
        if let Some(rx) = &self.rx {
            rx.lock().await.recv().await.ok_or_else(|| anyhow::anyhow!("Event channel closed"))
        } else {
            Err(anyhow::anyhow!(
                "Cannot receive events on cloned EventHandler. Use EventHandler::with_receiver() instead."
            ))
        }
    }

    /// Create a new event handler and return a receiver.
    pub fn with_receiver(
        config: EventHandlerConfig,
    ) -> anyhow::Result<(mpsc::Receiver<Event>, Self)> {
        let (tx, rx) = mpsc::channel(config.channel_capacity);

        // Enable mouse events if configured
        if config.enable_mouse {
            crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)?;
        }

        // Enable paste events if configured
        if config.enable_paste {
            crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste)?;
        }

        // Spawn the event loop task
        tokio::spawn(event_loop_with_sender(config.tick_rate, tx.clone()));

        // Create an unbounded channel for custom events that we can send from anywhere
        let (unbounded_tx, mut unbounded_rx) = mpsc::unbounded_channel();

        // Forward unbounded events to the main channel
        let tx_clone = tx.clone();
        tokio::spawn(async move {
            while let Some(event) = unbounded_rx.recv().await {
                if tx_clone.send(event).await.is_err() {
                    break;
                }
            }
        });

        let handler = Self { _tx: Arc::new(unbounded_tx), rx: None };

        Ok((rx, handler))
    }

    /// Send a custom event.
    pub fn send(&self, event: Event) -> anyhow::Result<()> {
        self._tx.send(event).map_err(|e| anyhow::anyhow!("Failed to send event: {}", e))
    }
}

/// Event loop that sends to an mpsc channel.
async fn event_loop_with_sender(tick_rate: Duration, tx: mpsc::Sender<Event>) {
    // Move blocking crossterm operations to a dedicated thread using spawn_blocking
    // This prevents blocking the async executor threads with terminal I/O
    let tx = std::sync::Arc::new(tx);
    let mut last_tick = std::time::Instant::now();

    loop {
        let timeout =
            tick_rate.checked_sub(last_tick.elapsed()).unwrap_or_else(|| Duration::from_secs(0));

        // Run the blocking crossterm poll/read in a separate thread
        let poll_result = tokio::task::spawn_blocking(move || {
            // crossterm::event::poll and crossterm::event::read are blocking calls
            if crossterm::event::poll(timeout).unwrap_or(false) {
                crossterm::event::read().ok()
            } else {
                None
            }
        })
        .await;

        match poll_result {
            Ok(Some(crossterm_event)) => {
                if let Some(event) = Event::from_crossterm(crossterm_event)
                    && tx.send(event).await.is_err()
                {
                    // Channel closed, exit the loop
                    return;
                }
                // Also send tick if the tick interval has elapsed, even when
                // input events are arriving (e.g. mouse events from mouse capture
                // can starve tick generation otherwise).
                if last_tick.elapsed() >= tick_rate {
                    if tx.send(Event::Tick).await.is_err() {
                        return;
                    }
                    last_tick = std::time::Instant::now();
                }
            }
            Ok(None) => {
                // Timeout occurred, send tick event if needed
                if last_tick.elapsed() >= tick_rate {
                    if tx.send(Event::Tick).await.is_err() {
                        // Channel closed, exit the loop
                        return;
                    }
                    last_tick = std::time::Instant::now();
                }
            }
            Err(_) => {
                // Spawn blocking task failed or was cancelled
                if tx.send(Event::Tick).await.is_err() {
                    return;
                }
                last_tick = std::time::Instant::now();
            }
        }
    }
}

/// A helper struct for event handling that provides common patterns.
#[derive(Debug)]
pub struct EventStream {
    receiver: mpsc::Receiver<Event>,
}

impl EventStream {
    /// Create a new event stream.
    pub fn new(config: EventHandlerConfig) -> anyhow::Result<Self> {
        let (receiver, _) = EventHandler::with_receiver(config)?;
        Ok(Self { receiver })
    }

    /// Create a new event stream and return the receiver directly.
    pub fn receiver(config: EventHandlerConfig) -> anyhow::Result<mpsc::Receiver<Event>> {
        let (receiver, _) = EventHandler::with_receiver(config)?;
        Ok(receiver)
    }

    /// Get the next event.
    pub async fn next(&mut self) -> Option<Event> {
        self.receiver.recv().await
    }

    /// Try to get the next event without blocking.
    pub fn try_next(&mut self) -> Option<Event> {
        self.receiver.try_recv().ok()
    }
}

/// Key bindings for common actions.
#[derive(Debug, Clone)]
pub struct KeyBindings {
    /// Quit key (default: 'q')
    pub quit: Vec<KeyCode>,
    /// Navigate up key (default: 'k' or Up)
    pub up: Vec<KeyCode>,
    /// Navigate down key (default: 'j' or Down)
    pub down: Vec<KeyCode>,
    /// Navigate left key (default: 'h' or Left)
    pub left: Vec<KeyCode>,
    /// Navigate right key (default: 'l' or Right)
    pub right: Vec<KeyCode>,
    /// Confirm/Select key (default: Enter)
    pub confirm: Vec<KeyCode>,
    /// Cancel key (default: Esc)
    pub cancel: Vec<KeyCode>,
}

// Re-export for convenience
pub use ratatui::crossterm::event::KeyCode;

impl Default for KeyBindings {
    fn default() -> Self {
        Self {
            quit: vec![KeyCode::Char('q')],
            up: vec![KeyCode::Char('k'), KeyCode::Up],
            down: vec![KeyCode::Char('j'), KeyCode::Down],
            left: vec![KeyCode::Char('h'), KeyCode::Left],
            right: vec![KeyCode::Char('l'), KeyCode::Right],
            confirm: vec![KeyCode::Enter],
            cancel: vec![KeyCode::Esc],
        }
    }
}

impl KeyBindings {
    /// Create new key bindings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if a key event matches a specific action.
    pub fn matches(&self, key: &KeyEvent, action: KeyAction) -> bool {
        let codes = match action {
            KeyAction::Quit => &self.quit,
            KeyAction::Up => &self.up,
            KeyAction::Down => &self.down,
            KeyAction::Left => &self.left,
            KeyAction::Right => &self.right,
            KeyAction::Confirm => &self.confirm,
            KeyAction::Cancel => &self.cancel,
        };

        codes.contains(&key.code)
    }
}

/// An action that can be triggered by a key binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    Quit,
    Up,
    Down,
    Left,
    Right,
    Confirm,
    Cancel,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_is_quit() {
        let q_event = Event::Key(KeyEvent::new(
            KeyCode::Char('q'),
            ratatui::crossterm::event::KeyModifiers::NONE,
        ));
        assert!(q_event.is_quit());

        let c_event = Event::Key(KeyEvent::new(
            KeyCode::Char('c'),
            ratatui::crossterm::event::KeyModifiers::CONTROL,
        ));
        assert!(c_event.is_quit());
    }

    #[test]
    fn test_event_is_navigation() {
        let up_event =
            Event::Key(KeyEvent::new(KeyCode::Up, ratatui::crossterm::event::KeyModifiers::NONE));
        assert!(up_event.is_navigation());

        let k_event = Event::Key(KeyEvent::new(
            KeyCode::Char('k'),
            ratatui::crossterm::event::KeyModifiers::NONE,
        ));
        assert!(k_event.is_navigation());

        let q_event = Event::Key(KeyEvent::new(
            KeyCode::Char('q'),
            ratatui::crossterm::event::KeyModifiers::NONE,
        ));
        assert!(!q_event.is_navigation());
    }

    #[test]
    fn test_key_bindings_default() {
        let bindings = KeyBindings::default();

        let up_key = KeyEvent::new(KeyCode::Up, ratatui::crossterm::event::KeyModifiers::NONE);
        let k_key =
            KeyEvent::new(KeyCode::Char('k'), ratatui::crossterm::event::KeyModifiers::NONE);

        assert!(bindings.matches(&up_key, KeyAction::Up));
        assert!(bindings.matches(&k_key, KeyAction::Up));
    }

    #[test]
    fn test_event_handler_config() {
        let config = EventHandlerConfig::new()
            .tick_rate(Duration::from_millis(100))
            .enable_mouse(false)
            .channel_capacity(500);

        assert_eq!(config.tick_rate, Duration::from_millis(100));
        assert!(!config.enable_mouse);
        assert_eq!(config.channel_capacity, 500);
    }
}
