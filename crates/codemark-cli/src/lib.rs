pub mod cli;

/// Default Codetours server URL used when none is specified (e.g. `auth login`).
///
/// Re-exported from `codemark_core` so the constant has a single source of truth.
pub use codemark_core::DEFAULT_SERVER_URL;
