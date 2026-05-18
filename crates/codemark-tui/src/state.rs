//! Application state management.
//!
//! This module provides the state management system for the TUI application,
//! including modes, focus management, and data that persists between renders.

use std::collections::HashMap;

use crate::event::Event;

/// The overall state of the application.
///
/// AppState manages global application state including the current mode,
/// focus state, and any data that needs to persist across renders.
#[derive(Debug, Clone)]
pub struct AppState {
    /// The current application mode.
    mode: AppMode,
    /// Whether the application is currently running.
    running: bool,
    /// The currently focused component ID.
    focus: Option<String>,
    /// Custom state data for components.
    data: HashMap<String, StateData>,
    /// Terminal dimensions.
    size: (u16, u16),
}

/// The current mode of the application.
///
/// Modes determine how events are handled and what is displayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AppMode {
    /// Normal mode - default browsing mode.
    Normal,
    /// Insert mode - for entering text.
    Insert,
    /// Command mode - for entering commands.
    Command,
    /// Search mode - for searching.
    Search,
    /// Help mode - showing help.
    Help,
    /// Confirm mode - confirming an action.
    Confirm,
}

impl Default for AppMode {
    fn default() -> Self {
        Self::Normal
    }
}

impl AppMode {
    /// Check if this is a mode that accepts text input.
    pub fn is_input_mode(&self) -> bool {
        matches!(self, Self::Insert | Self::Command | Self::Search | Self::Confirm)
    }

    /// Get the mode indicator string for display in the UI.
    pub fn indicator(&self) -> &'static str {
        match self {
            Self::Normal => "NORMAL",
            Self::Insert => "INSERT",
            Self::Command => "COMMAND",
            Self::Search => "SEARCH",
            Self::Help => "HELP",
            Self::Confirm => "CONFIRM",
        }
    }
}

/// Data that can be stored in the state map.
#[derive(Debug, Clone)]
pub enum StateData {
    /// String data.
    String(String),
    /// Integer data.
    Integer(i64),
    /// Boolean data.
    Boolean(bool),
    /// List of strings.
    StringList(Vec<String>),
    /// Custom serializable data (as JSON string).
    Custom(String),
}

impl StateData {
    /// Create new string data.
    pub fn string(value: impl Into<String>) -> Self {
        Self::String(value.into())
    }

    /// Create new integer data.
    pub fn integer(value: i64) -> Self {
        Self::Integer(value)
    }

    /// Create new boolean data.
    pub fn boolean(value: bool) -> Self {
        Self::Boolean(value)
    }

    /// Create new string list data.
    pub fn string_list(value: Vec<String>) -> Self {
        Self::StringList(value)
    }

    /// Get as string if possible.
    pub fn as_string(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }

    /// Get as integer if possible.
    pub fn as_integer(&self) -> Option<i64> {
        match self {
            Self::Integer(i) => Some(*i),
            _ => None,
        }
    }

    /// Get as boolean if possible.
    pub fn as_boolean(&self) -> Option<bool> {
        match self {
            Self::Boolean(b) => Some(*b),
            _ => None,
        }
    }

    /// Get as string list if possible.
    pub fn as_string_list(&self) -> Option<&[String]> {
        match self {
            Self::StringList(list) => Some(list),
            _ => None,
        }
    }
}

impl AppState {
    /// Create a new application state.
    pub fn new() -> Self {
        Self {
            mode: AppMode::default(),
            running: true,
            focus: None,
            data: HashMap::new(),
            size: (80, 24),
        }
    }

    /// Get the current mode.
    pub fn mode(&self) -> AppMode {
        self.mode
    }

    /// Set the current mode.
    pub fn set_mode(&mut self, mode: AppMode) {
        self.mode = mode;
    }

    /// Check if the application is running.
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Stop the application.
    pub fn quit(&mut self) {
        self.running = false;
    }

    /// Get the currently focused component ID.
    pub fn focus(&self) -> Option<&str> {
        self.focus.as_deref()
    }

    /// Set the focused component.
    pub fn set_focus(&mut self, id: impl Into<String>) {
        self.focus = Some(id.into());
    }

    /// Clear the focus.
    pub fn clear_focus(&mut self) {
        self.focus = None;
    }

    /// Get the terminal size.
    pub fn size(&self) -> (u16, u16) {
        self.size
    }

    /// Set the terminal size.
    pub fn set_size(&mut self, width: u16, height: u16) {
        self.size = (width, height);
    }

    /// Get state data for a key.
    pub fn get(&self, key: &str) -> Option<&StateData> {
        self.data.get(key)
    }

    /// Set state data for a key.
    pub fn set(&mut self, key: impl Into<String>, value: StateData) {
        self.data.insert(key.into(), value);
    }

    /// Remove state data for a key.
    pub fn remove(&mut self, key: &str) -> Option<StateData> {
        self.data.remove(key)
    }

    /// Get a string value.
    pub fn get_string(&self, key: &str) -> Option<&str> {
        self.get(key)?.as_string()
    }

    /// Set a string value.
    pub fn set_string(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.set(key, StateData::String(value.into()));
    }

    /// Get an integer value.
    pub fn get_integer(&self, key: &str) -> Option<i64> {
        self.get(key)?.as_integer()
    }

    /// Set an integer value.
    pub fn set_integer(&mut self, key: impl Into<String>, value: i64) {
        self.set(key, StateData::Integer(value));
    }

    /// Get a boolean value.
    pub fn get_boolean(&self, key: &str) -> Option<bool> {
        self.get(key)?.as_boolean()
    }

    /// Set a boolean value.
    pub fn set_boolean(&mut self, key: impl Into<String>, value: bool) {
        self.set(key, StateData::Boolean(value));
    }

    /// Handle an event and update state if needed.
    ///
    /// Returns true if the event was handled.
    pub fn handle_event(&mut self, event: &Event) -> bool {
        match event {
            Event::Resize(width, height) => {
                self.set_size(*width, *height);
                true
            }
            Event::Key(key) => {
                // Handle mode-specific key bindings
                match self.mode {
                    AppMode::Normal => {
                        // Check for quit
                        if key.code == ratatui::crossterm::event::KeyCode::Char('q') {
                            self.quit();
                            return true;
                        }
                        // Check for mode switches
                        match key.code {
                            ratatui::crossterm::event::KeyCode::Char(':') => {
                                self.set_mode(AppMode::Command);
                                true
                            }
                            ratatui::crossterm::event::KeyCode::Char('/') => {
                                self.set_mode(AppMode::Search);
                                self.set_string("filter_buffer", "");
                                true
                            }
                            ratatui::crossterm::event::KeyCode::Char('i') => {
                                self.set_mode(AppMode::Insert);
                                true
                            }
                            ratatui::crossterm::event::KeyCode::Esc => {
                                // Clear active filter in Normal mode
                                self.set_string("active_filter", "");
                                true
                            }
                            _ => false,
                        }
                    }
                    AppMode::Command | AppMode::Search | AppMode::Insert => {
                        // In input modes, Esc returns to normal
                        if key.code == ratatui::crossterm::event::KeyCode::Esc {
                            if self.mode == AppMode::Search {
                                self.set_string("filter_buffer", "");
                                // We don't clear active_filter here, so user can cancel a new search
                                // while keeping the previous results.
                            }
                            self.set_mode(AppMode::Normal);
                            true
                        } else if key.code == ratatui::crossterm::event::KeyCode::Enter {
                            if self.mode == AppMode::Search {
                                let buffer =
                                    self.get_string("filter_buffer").unwrap_or("").to_string();
                                self.set_string("active_filter", buffer);
                                self.set_mode(AppMode::Normal);
                                true
                            } else {
                                false
                            }
                        } else if let ratatui::crossterm::event::KeyCode::Char(c) = key.code {
                            if self.mode == AppMode::Search {
                                let mut query =
                                    self.get_string("filter_buffer").unwrap_or("").to_string();
                                query.push(c);
                                self.set_string("filter_buffer", query);
                                true
                            } else {
                                false
                            }
                        } else if key.code == ratatui::crossterm::event::KeyCode::Backspace {
                            if self.mode == AppMode::Search {
                                let mut query =
                                    self.get_string("filter_buffer").unwrap_or("").to_string();
                                query.pop();
                                self.set_string("filter_buffer", query);
                                true
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

/// A trait for types that can be stored in the application state.
pub trait Storeable: Sized {
    /// Convert this value to StateData.
    fn to_data(&self) -> StateData;

    /// Try to convert from StateData.
    fn from_data(data: &StateData) -> Option<Self>;
}

impl Storeable for String {
    fn to_data(&self) -> StateData {
        StateData::String(self.clone())
    }

    fn from_data(data: &StateData) -> Option<Self> {
        data.as_string().map(ToString::to_string)
    }
}

impl Storeable for i64 {
    fn to_data(&self) -> StateData {
        StateData::Integer(*self)
    }

    fn from_data(data: &StateData) -> Option<Self> {
        data.as_integer()
    }
}

impl Storeable for bool {
    fn to_data(&self) -> StateData {
        StateData::Boolean(*self)
    }

    fn from_data(data: &StateData) -> Option<Self> {
        data.as_boolean()
    }
}

impl Storeable for Vec<String> {
    fn to_data(&self) -> StateData {
        StateData::StringList(self.clone())
    }

    fn from_data(data: &StateData) -> Option<Self> {
        data.as_string_list().map(ToOwned::to_owned)
    }
}

/// A helper struct for managing focus across multiple components.
#[derive(Debug, Clone)]
pub struct FocusManager {
    /// The component IDs in focus order.
    components: Vec<String>,
    /// The currently focused index.
    focused: Option<usize>,
}

impl FocusManager {
    /// Create a new focus manager.
    pub fn new() -> Self {
        Self { components: Vec::new(), focused: None }
    }

    /// Add a component to the focus order.
    pub fn add(&mut self, id: impl Into<String>) {
        let id = id.into();
        if !self.components.contains(&id) {
            self.components.push(id);
            if self.focused.is_none() {
                self.focused = Some(0);
            }
        }
    }

    /// Remove a component from the focus order.
    pub fn remove(&mut self, id: &str) {
        if let Some(pos) = self.components.iter().position(|x| x == id) {
            self.components.remove(pos);
            // Adjust focused index if needed
            if let Some(focused) = self.focused {
                if focused >= self.components.len() {
                    self.focused = if self.components.is_empty() {
                        None
                    } else {
                        Some(self.components.len() - 1)
                    };
                } else if pos == focused {
                    self.focused = if self.components.is_empty() {
                        None
                    } else {
                        Some(focused.min(self.components.len() - 1))
                    };
                }
            }
        }
    }

    /// Get the currently focused component ID.
    pub fn focused(&self) -> Option<&str> {
        self.focused.and_then(|i| self.components.get(i)).map(String::as_str)
    }

    /// Set the focused component by ID.
    pub fn set_focused(&mut self, id: &str) -> bool {
        if let Some(pos) = self.components.iter().position(|x| x == id) {
            self.focused = Some(pos);
            true
        } else {
            false
        }
    }

    /// Focus the next component.
    pub fn next(&mut self) -> bool {
        if self.components.is_empty() {
            return false;
        }
        let len = self.components.len();
        self.focused = Some(self.focused.map_or(0, |i| (i + 1) % len));
        true
    }

    /// Focus the previous component.
    pub fn previous(&mut self) -> bool {
        if self.components.is_empty() {
            return false;
        }
        let len = self.components.len();
        self.focused = Some(self.focused.map_or(0, |i| if i == 0 { len - 1 } else { i - 1 }));
        true
    }

    /// Check if a specific component is focused.
    pub fn is_focused(&self, id: &str) -> bool {
        self.focused() == Some(id)
    }

    /// Get all component IDs.
    pub fn components(&self) -> &[String] {
        &self.components
    }
}

impl Default for FocusManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_mode_is_input_mode() {
        assert!(!AppMode::Normal.is_input_mode());
        assert!(!AppMode::Help.is_input_mode());
        assert!(AppMode::Insert.is_input_mode());
        assert!(AppMode::Command.is_input_mode());
        assert!(AppMode::Search.is_input_mode());
        assert!(AppMode::Confirm.is_input_mode());
    }

    #[test]
    fn test_app_mode_indicator() {
        assert_eq!(AppMode::Normal.indicator(), "NORMAL");
        assert_eq!(AppMode::Insert.indicator(), "INSERT");
        assert_eq!(AppMode::Command.indicator(), "COMMAND");
    }

    #[test]
    fn test_app_state_creation() {
        let state = AppState::new();
        assert_eq!(state.mode(), AppMode::Normal);
        assert!(state.is_running());
        assert_eq!(state.size(), (80, 24));
    }

    #[test]
    fn test_app_state_quit() {
        let mut state = AppState::new();
        state.quit();
        assert!(!state.is_running());
    }

    #[test]
    fn test_app_state_data() {
        let mut state = AppState::new();

        state.set_string("test_key", "test_value");
        assert_eq!(state.get_string("test_key"), Some("test_value"));

        state.set_integer("count", 42);
        assert_eq!(state.get_integer("count"), Some(42));

        state.set_boolean("flag", true);
        assert_eq!(state.get_boolean("flag"), Some(true));
    }

    #[test]
    fn test_focus_manager() {
        let mut manager = FocusManager::new();

        manager.add("component1");
        manager.add("component2");
        manager.add("component3");

        assert_eq!(manager.focused(), Some("component1"));

        manager.next();
        assert_eq!(manager.focused(), Some("component2"));

        manager.next();
        assert_eq!(manager.focused(), Some("component3"));

        manager.next(); // Should wrap
        assert_eq!(manager.focused(), Some("component1"));

        manager.previous();
        assert_eq!(manager.focused(), Some("component3"));

        assert!(manager.is_focused("component3"));
        assert!(!manager.is_focused("component1"));
    }

    #[test]
    fn test_focus_manager_remove() {
        let mut manager = FocusManager::new();

        manager.add("component1");
        manager.add("component2");
        manager.add("component3");

        manager.remove("component2");

        assert_eq!(manager.components().len(), 2);
        assert!(manager.next()); // Move to component3
        assert_eq!(manager.focused(), Some("component3"));
    }
}
