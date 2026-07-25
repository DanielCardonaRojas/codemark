//! Codemark - A structural bookmarking system for code using tree-sitter queries.

/// Default Codetours server URL used when none is specified (e.g. `auth login`).
pub const DEFAULT_SERVER_URL: &str = "https://codetours.fly.dev";

pub mod config;
pub mod embeddings;
pub mod engine;
pub mod error;
pub mod git;
pub mod parser;
pub mod query;
pub mod sort;
pub mod storage;
pub mod sync;
pub mod templates;
pub mod vfs;
