//! # Codemark TUI
//!
//! A terminal UI library for Codemark using ratatui.
//!
//! This library provides a composable component system for building
//! terminal user interfaces with a focus on keyboard-driven navigation
//! and flexible layouts (similar to lazy-git).
//!
//! ## Features
//!
//! - Composable component system with a base `Component` trait
//! - Scrollable panels for displaying lists
//! - Flexible layout management (rows, columns, splits)
//! - Event handling system for keyboard and mouse input
//! - Application state management
//! - Built-in UI components (labels, spacers, etc.)
//!
//! ## Example
//!
//! ```rust,no_run
//! use codemark_tui::{App, AppBuilder};
//!
//! fn main() -> anyhow::Result<()> {
//!     let mut app = AppBuilder::new().build()?;
//!     app.run()?;
//!     Ok(())
//! }
//! ```

pub mod app;
pub mod browser;
pub mod component;
pub mod event;
pub mod help;
pub mod layout;
pub mod logging;
pub mod state;
pub mod theme;
pub mod ui;

// Re-export commonly used types
pub use app::{App, AppBuilder};
pub use browser::{BrowserLayout, FocusArea, HealNotification, SearchBar, Tab, TabSelection};
pub use component::{
    Component, HealthStatus, Label, Panel, PanelItem, SizeConstraints, Spacer, SyncDirection,
};
pub use event::{Event, EventHandler, EventHandlerConfig, KeyBindings};
pub use help::{HelpOverlay, HelpTab};
pub use layout::{LayoutChild, LayoutManager, SplitLayout, helpers};
pub use state::{AppMode, AppState, FocusManager, StateData, Storeable};
pub use ui::{NotificationType, render_confirmation, render_help_panel, render_notification};

/// Version information
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Default tick rate for event polling
pub const DEFAULT_TICK_RATE: std::time::Duration = std::time::Duration::from_millis(250);
