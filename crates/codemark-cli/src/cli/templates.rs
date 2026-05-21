//! Handlebars template support for markdown output.
//!
//! This module re-exports the template rendering functionality from codemark-core.
//! Templates are stored in `~/.config/codemark/templates/` directory and can be customized by users.
//!
//! # Re-exports
//!
//! All template rendering types and functions are re-exported from `codemark_core::templates`:
//!
//! - `BookmarkTemplateContext` - Template context for rendering bookmarks
//! - `AnnotationTemplateContext` - Template context for annotations
//! - `ResolutionTemplateContext` - Template context for resolutions
//! - `render_show_template()` - Render a bookmark with its resolutions
//! - `load_show_template()` - Load the show template from disk or default
//! - `default_show_template()` - Get the default bundled template
//! - `ensure_default_template_exists()` - Ensure default template is on disk
//! - `create_handlebars_engine()` - Create a configured Handlebars instance
//! - `templates_dir()` - Get the templates directory path

// Re-export all template rendering functionality from codemark_core
pub use codemark_core::templates::{
    ensure_default_template_exists, load_show_template, render_show_template, BookmarkTemplateContext,
    AnnotationTemplateContext, ResolutionTemplateContext,
};
