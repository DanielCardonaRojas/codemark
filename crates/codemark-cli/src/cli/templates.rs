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
//! - `render_template()` - Render a bookmark using a specific template
//! - `load_show_template()` - Load the show template from disk or default
//! - `load_template()` - Load a specific template from disk or default
//! - `ensure_default_template_exists()` - Ensure default template is on disk
//! - `SHOW_TEMPLATE` - Constant name for the show template
//! - `DETAILS_TEMPLATE` - Constant name for the details panel template

// Re-export all template rendering functionality from codemark_core
pub use codemark_core::templates::{
    AnnotationTemplateContext, BookmarkTemplateContext, DETAILS_TEMPLATE,
    ResolutionTemplateContext, SHOW_TEMPLATE, create_handlebars_engine,
    ensure_default_template_exists, load_show_template, load_template, render_show_template,
    render_template,
};
