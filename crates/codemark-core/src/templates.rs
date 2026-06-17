//! Handlebars template support for markdown output.
//!
//! Templates are stored in `~/.config/codemark/templates/` directory and can be customized by users.
//! The default template is bundled into the binary at compile time from `templates/codemark_show.md`
//! in the project root.

use std::path::PathBuf;

use chrono::DateTime;
use handlebars::{Handlebars, HelperDef, HelperResult, Output, RenderContext, RenderErrorReason};
use serde::Serialize;

use crate::config::global_config_dir;
use crate::engine::bookmark::{Annotation, Bookmark, Collection, CollectionLink, Resolution};

/// Template context for rendering a single bookmark with its resolutions.
#[derive(Debug, Serialize)]
pub struct BookmarkTemplateContext {
    /// Short ID (first 8 chars)
    pub short_id: String,
    /// Full bookmark ID
    pub id: String,
    /// File path
    pub file_path: String,
    /// Just the filename
    pub file_name: String,
    /// Programming language
    pub language: String,
    /// Status as string
    pub status: String,
    /// UI status as string (projected based on current HEAD)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui_status: Option<String>,
    /// Tree-sitter query
    pub query: String,
    /// Creation timestamp
    pub created_at: String,
    /// Creator (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
    /// Git commit hash (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit_hash: Option<String>,
    /// Short commit hash (first 8 chars, optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub short_commit: Option<String>,
    /// Last resolution time (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_resolved_at: Option<String>,
    /// Resolution method as string (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution_method: Option<String>,
    /// When it became stale (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stale_since: Option<String>,
    /// Current resolution ID (optional)
    pub current_resolution_id: Option<String>,
    /// Code snapshot from the latest resolution (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
    /// Breadcrumbs for sticky headers
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub breadcrumbs: Vec<crate::engine::breadcrumbs::Breadcrumb>,
    /// Tags
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Annotations
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub annotations: Vec<AnnotationTemplateContext>,
    /// Comments (durable markdown discussion entries)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<CommentTemplateContext>,
    /// Resolution history
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub resolutions: Vec<ResolutionTemplateContext>,
}

/// Template context for an annotation.
#[derive(Debug, Serialize)]
pub struct AnnotationTemplateContext {
    /// When annotation was added
    pub added_at: String,
    /// Who added it (optional, defaults to "unknown")
    pub added_by: String,
    /// Source (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Notes (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// Code context snippet (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

/// Template context for a comment.
#[derive(Debug, Serialize)]
pub struct CommentTemplateContext {
    /// Comment author
    pub author: String,
    /// Markdown body of the comment
    pub body: String,
    /// When the comment was created
    pub created_at: String,
    /// Parent comment ID for threaded replies (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
}

/// Template context for a resolution.
#[derive(Debug, Serialize)]
pub struct ResolutionTemplateContext {
    /// Resolution ID
    pub id: String,
    /// When resolution occurred
    pub resolved_at: String,
    /// Resolution method
    pub method: String,
    /// Health status
    pub status: String,
    /// Whether this is the current resolution
    pub is_current: bool,
    /// Resolved file path (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    /// Line range (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_range: Option<String>,
    /// Line range with colon separator for tools (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_range_colon: Option<String>,
    /// Number of matches (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub match_count: Option<i32>,
    /// Resolution commit (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit_hash: Option<String>,
    /// Short commit hash (first 8 chars, optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub short_commit: Option<String>,
    /// Code snapshot at this resolution (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
    /// UI status computed for this resolution (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui_status: Option<String>,
    /// Whether this resolution is anchored
    pub is_anchored: bool,
}

impl BookmarkTemplateContext {
    /// Create a template context from a bookmark and its resolutions.
    pub fn from_bookmark(
        bm: &Bookmark,
        resolutions: &[Resolution],
        repo_path: &std::path::Path,
        current_head: Option<&str>,
    ) -> Self {
        let short_id = short_id(&bm.id).to_string();
        let file_name = file_name_of(&bm.file_path);

        let short_commit = bm.commit_hash.as_ref().map(|c| short_id_value(c));

        let latest_snapshot = resolutions.first().and_then(|r| r.snapshot.clone());

        let breadcrumbs = resolutions
            .first()
            .and_then(|r| r.breadcrumbs.as_ref())
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();

        BookmarkTemplateContext {
            short_id,
            id: bm.id.clone(),
            file_path: bm.file_path.clone(),
            file_name,
            language: bm.language.clone(),
            status: bm.health.to_string(),
            ui_status: bm.current_resolution_id.as_ref().and_then(|rid| {
                resolutions
                    .iter()
                    .find(|r| r.id == *rid)
                    .and_then(|resolution| {
                        crate::engine::projection::project_resolution_status(
                            resolution,
                            bm,
                            current_head,
                            repo_path,
                        )
                        .ok()
                    })
                    .map(|s| s.to_string())
            }),
            query: bm.query.clone(),
            created_at: bm.created_at.clone(),
            created_by: bm.created_by.clone(),
            commit_hash: bm.commit_hash.clone(),
            short_commit,
            last_resolved_at: bm.last_resolved_at.clone(),
            resolution_method: bm.resolution_method.map(|m| m.to_string()),
            stale_since: bm.stale_since.clone(),
            current_resolution_id: bm.current_resolution_id.clone(),
            snapshot: latest_snapshot,
            breadcrumbs,
            tags: bm.tags.clone(),
            annotations: bm
                .annotations
                .iter()
                .map(AnnotationTemplateContext::from_annotation)
                .collect(),
            comments: bm.comments.iter().map(CommentTemplateContext::from_comment).collect(),
            resolutions: resolutions
                .iter()
                .map(|r| {
                    ResolutionTemplateContext::from_resolution_projected(
                        r,
                        bm,
                        repo_path,
                        current_head,
                    )
                })
                .collect(),
        }
    }
}

impl CommentTemplateContext {
    /// Create a template context from a comment.
    fn from_comment(comment: &crate::engine::bookmark::BookmarkComment) -> Self {
        CommentTemplateContext {
            author: comment.author.clone(),
            body: comment.body.clone(),
            created_at: comment.created_at.clone(),
            parent_id: comment.parent_id.clone(),
        }
    }
}

impl AnnotationTemplateContext {
    /// Create a template context from an annotation.
    fn from_annotation(ann: &Annotation) -> Self {
        AnnotationTemplateContext {
            added_at: ann.added_at.clone(),
            added_by: ann.added_by.clone().unwrap_or_else(|| "unknown".to_string()),
            source: ann.source.clone(),
            notes: ann.notes.clone(),
            context: ann.context.clone(),
        }
    }
}

impl ResolutionTemplateContext {
    /// Create a template context from a resolution.
    fn from_resolution(r: &Resolution, current_resolution_id: Option<&str>) -> Self {
        let short_commit = r.commit_hash.as_ref().map(|c| short_id_value(c));

        ResolutionTemplateContext {
            id: r.id.clone(),
            resolved_at: r.resolved_at.clone(),
            method: r.method.to_string(),
            status: r.health.to_string(),
            is_current: current_resolution_id == Some(r.id.as_str()),
            file_path: r.file_path.clone(),
            line_range: r.line_range.clone(),
            line_range_colon: r.line_range.as_ref().map(|l| l.replace('-', ":")),
            match_count: r.match_count,
            commit_hash: r.commit_hash.clone(),
            short_commit,
            snapshot: r.snapshot.clone(),
            ui_status: None, // Will be computed or passed if needed
            is_anchored: !r.is_dirty,
        }
    }

    /// Create a template context from a resolution with projected UI status.
    ///
    /// Only the current resolution gets a projected `ui_status`; historical
    /// resolutions are left with `ui_status: None` because the projection
    /// algorithm is only meaningful relative to the bookmark's current pointer.
    pub fn from_resolution_projected(
        r: &Resolution,
        bm: &Bookmark,
        repo_path: &std::path::Path,
        current_head: Option<&str>,
    ) -> Self {
        let mut ctx = Self::from_resolution(r, bm.current_resolution_id.as_deref());
        if ctx.is_current
            && let Ok(status) =
                crate::engine::projection::project_resolution_status(r, bm, current_head, repo_path)
        {
            ctx.ui_status = Some(status.to_string());
        }
        ctx
    }
}

/// Template context for rendering a collection overview (live preview).
#[derive(Debug, Serialize)]
pub struct CollectionTemplateContext {
    /// Collection name
    pub name: String,
    /// Description (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Visibility as string
    pub visibility: String,
    /// Health status as string (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health: Option<String>,
    /// Creation timestamp
    pub created_at: String,
    /// Creator (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
    /// Branch the collection was created on (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Whether the collection has been published
    pub published: bool,
    /// Publish timestamp (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
    /// Source repository URL (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_url: Option<String>,
    /// Number of steps (bookmarks) in the collection
    pub step_count: usize,
    /// Tags
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// External links
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<CollectionLinkTemplateContext>,
    /// Steps (one per bookmark, in order)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<CollectionStepTemplateContext>,
}

/// Template context for a collection link.
#[derive(Debug, Serialize)]
pub struct CollectionLinkTemplateContext {
    /// Link kind as string (pr, issue, doc, ...)
    pub kind: String,
    /// Display label
    pub label: String,
    /// URL
    pub url: String,
}

/// Template context for a single collection step (bookmark).
#[derive(Debug, Serialize)]
pub struct CollectionStepTemplateContext {
    /// 1-based step number
    pub index: usize,
    /// Short bookmark ID (first 8 chars) for drilling into the bookmark
    pub id: String,
    /// File path
    pub file_path: String,
    /// Just the filename
    pub file_name: String,
    /// Programming language
    pub language: String,
    /// Human-readable summary of the bookmarked code (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// First annotation note, if any (the durable code explanation)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl CollectionTemplateContext {
    /// Create a template context from a collection, its bookmarks, tags, and links.
    pub fn from_collection(
        collection: &Collection,
        bookmarks: &[Bookmark],
        tags: Vec<String>,
        links: &[CollectionLink],
    ) -> Self {
        let steps = bookmarks
            .iter()
            .enumerate()
            .map(|(i, bm)| CollectionStepTemplateContext::from_bookmark(i + 1, bm))
            .collect();

        CollectionTemplateContext {
            name: collection.name.clone(),
            description: collection.description.clone(),
            visibility: collection.visibility.to_string(),
            health: collection.health.map(|h| h.to_string()),
            created_at: collection.created_at.clone(),
            created_by: collection.created_by.clone(),
            branch: collection.created_branch.clone(),
            published: collection.published_at.is_some(),
            published_at: collection.published_at.clone(),
            repo_url: collection.repo_url.clone(),
            step_count: bookmarks.len(),
            tags,
            links: links.iter().map(CollectionLinkTemplateContext::from_link).collect(),
            steps,
        }
    }
}

impl CollectionLinkTemplateContext {
    /// Create a template context from a collection link.
    fn from_link(link: &CollectionLink) -> Self {
        CollectionLinkTemplateContext {
            kind: link.kind.to_string(),
            label: link.label.clone(),
            url: link.url.clone(),
        }
    }
}

impl CollectionStepTemplateContext {
    /// Create a template context from a bookmark at the given 1-based position.
    fn from_bookmark(index: usize, bm: &Bookmark) -> Self {
        let file_name = file_name_of(&bm.file_path);

        let summary = bm
            .language
            .parse::<crate::parser::languages::Language>()
            .ok()
            .and_then(|lang| crate::query::summarizer::summarize_query(&bm.query, Some(lang)).ok())
            .and_then(|s| s.format());

        let note = bm.annotations.iter().find_map(|a| a.notes.clone());

        CollectionStepTemplateContext {
            index,
            id: short_id_value(&bm.id),
            file_path: bm.file_path.clone(),
            file_name,
            language: bm.language.clone(),
            summary,
            note,
        }
    }
}

/// Extract the file name from a path, handling both `/` and `\` separators
/// regardless of host OS (stored paths may use either separator).
fn file_name_of(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

/// Truncate a string to first 8 characters.
fn short_id_value(s: &str) -> String {
    s.chars().take(8).collect()
}

/// Truncate ID to first 8 characters.
pub fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

/// Escape special markdown characters in text.
fn escape_markdown(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => result.push_str("\\\\"),
            '`' => result.push_str("\\`"),
            '*' => result.push_str("\\*"),
            '_' => result.push_str("\\_"),
            '{' => result.push_str("\\{"),
            '}' => result.push_str("\\}"),
            '[' => result.push_str("\\["),
            ']' => result.push_str("\\]"),
            '(' => result.push_str("\\("),
            ')' => result.push_str("\\)"),
            '#' => result.push_str("\\#"),
            '+' => result.push_str("\\+"),
            '-' => result.push_str("\\-"),
            '.' => result.push_str("\\."),
            '!' => result.push_str("\\!"),
            '|' => result.push_str("\\|"),
            '<' => result.push_str("\\<"),
            '>' => result.push_str("\\>"),
            _ => result.push(c),
        }
    }
    result
}

/// Helper function for Handlebars to escape markdown.
#[derive(Clone)]
struct EscapeMarkdownHelper;

impl HelperDef for EscapeMarkdownHelper {
    fn call<'reg: 'rc, 'rc>(
        &self,
        h: &handlebars::Helper<'rc>,
        _r: &'reg Handlebars<'reg>,
        _ctx: &'rc handlebars::Context,
        _rc: &mut RenderContext<'reg, 'rc>,
        out: &mut dyn Output,
    ) -> HelperResult {
        let param = h.param(0).ok_or_else(|| {
            RenderErrorReason::Other("escape_markdown helper requires exactly one parameter".into())
        })?;
        let value = param.value().as_str().ok_or_else(|| {
            RenderErrorReason::Other("escape_markdown helper parameter must be a string".into())
        })?;
        out.write(&escape_markdown(value))?;
        Ok(())
    }
}

/// Helper function for Handlebars to truncate an ID to first 8 chars.
#[derive(Clone)]
struct TruncateHelper;

impl HelperDef for TruncateHelper {
    fn call<'reg: 'rc, 'rc>(
        &self,
        h: &handlebars::Helper<'rc>,
        _r: &'reg Handlebars<'reg>,
        _ctx: &'rc handlebars::Context,
        _rc: &mut RenderContext<'reg, 'rc>,
        out: &mut dyn Output,
    ) -> HelperResult {
        let param = h.param(0).ok_or_else(|| {
            RenderErrorReason::Other("truncate helper requires exactly one parameter".into())
        })?;
        let value = param.value().as_str().ok_or_else(|| {
            RenderErrorReason::Other("truncate helper parameter must be a string".into())
        })?;
        out.write(&short_id_value(value))?;
        Ok(())
    }
}

/// Helper function for Handlebars to format a date string.
/// Usage: {{format_date date_string "%Y-%m-%d %H:%M:%S"}}
#[derive(Clone)]
struct FormatDateHelper;

impl HelperDef for FormatDateHelper {
    fn call<'reg: 'rc, 'rc>(
        &self,
        h: &handlebars::Helper<'rc>,
        _r: &'reg Handlebars<'reg>,
        _ctx: &'rc handlebars::Context,
        _rc: &mut RenderContext<'reg, 'rc>,
        out: &mut dyn Output,
    ) -> HelperResult {
        let date_str = h.param(0).and_then(|p| p.value().as_str()).ok_or_else(|| {
            RenderErrorReason::Other("format_date helper requires a date string parameter".into())
        })?;

        let format_str = h.param(1).and_then(|p| p.value().as_str()).unwrap_or("%Y-%m-%d %H:%M:%S");

        // Try to parse the date string (expecting RFC3339 or similar)
        let formatted = if let Ok(dt) = DateTime::parse_from_rfc3339(date_str) {
            dt.format(format_str).to_string()
        } else if let Ok(dt) = DateTime::parse_from_str(date_str, "%Y-%m-%d %H:%M:%S") {
            dt.format(format_str).to_string()
        } else {
            // Fallback to original string if parsing fails
            date_str.to_string()
        };

        out.write(&formatted)?;
        Ok(())
    }
}

/// Get the templates directory.
///
/// Located alongside `config.toml` under the global config directory
/// (e.g. `$XDG_CONFIG_HOME/codemark/templates` or
/// `~/Library/Application Support/codemark/templates` on macOS).
pub fn templates_dir() -> Option<PathBuf> {
    global_config_dir().map(|d| d.join("templates"))
}

/// Template names
pub const SHOW_TEMPLATE: &str = "codemark_show.md";
pub const DETAILS_TEMPLATE: &str = "details_panel.md";
pub const COLLECTION_OVERVIEW_TEMPLATE: &str = "codemark_collection_overview.md";

/// Get the default markdown template for the `show` command.
pub fn default_show_template() -> &'static str {
    include_str!("../../../templates/codemark_show.md")
}

/// Get the default markdown template for the details panel.
pub fn default_details_template() -> &'static str {
    include_str!("../../../templates/details_panel.md")
}

/// Get the default markdown template for the collection overview panel.
pub fn default_collection_overview_template() -> &'static str {
    include_str!("../../../templates/codemark_collection_overview.md")
}

/// Get the default content for a given template name.
pub fn default_template_content(name: &str) -> Option<&'static str> {
    match name {
        SHOW_TEMPLATE => Some(default_show_template()),
        DETAILS_TEMPLATE => Some(default_details_template()),
        COLLECTION_OVERVIEW_TEMPLATE => Some(default_collection_overview_template()),
        _ => None,
    }
}

/// Create a Handlebars instance with all helpers registered.
pub fn create_handlebars_engine() -> Handlebars<'static> {
    let mut handlebars = Handlebars::new();

    // Don't escape HTML - we're generating markdown
    handlebars.register_escape_fn(|s| s.to_string());

    // Register custom helpers
    handlebars.register_helper("escape_markdown", Box::new(EscapeMarkdownHelper));
    handlebars.register_helper("truncate", Box::new(TruncateHelper));
    handlebars.register_helper("format_date", Box::new(FormatDateHelper));

    handlebars
}

/// Ensure default template files exist in the user's data directory.
/// Creates the template files if they don't already exist.
pub fn ensure_default_template_exists() {
    let templates_dir = match templates_dir() {
        Some(dir) => dir,
        None => return,
    };

    // Create templates directory if it doesn't exist
    if let Err(e) = std::fs::create_dir_all(&templates_dir) {
        eprintln!("Warning: Failed to create templates directory: {}", e);
        return;
    }

    for name in [SHOW_TEMPLATE, DETAILS_TEMPLATE, COLLECTION_OVERVIEW_TEMPLATE] {
        let template_path = templates_dir.join(name);
        if !template_path.exists()
            && let Some(content) = default_template_content(name)
            && let Err(e) = std::fs::write(&template_path, content)
        {
            eprintln!(
                "Warning: Failed to write default template to {}: {}",
                template_path.display(),
                e
            );
        }
    }
}

/// Load a template, falling back to the default if not found.
pub fn load_template(name: &str) -> String {
    let templates_dir = templates_dir();
    let template_path = templates_dir.map(|d| d.join(name));

    if let Some(path) = template_path
        && let Ok(content) = std::fs::read_to_string(path)
    {
        return content;
    }

    default_template_content(name).unwrap_or("").to_string()
}

/// Load the show template (backward compatibility).
pub fn load_show_template() -> String {
    load_template(SHOW_TEMPLATE)
}

/// Render a bookmark with its resolutions using a specific template.
pub fn render_template(
    template_name: &str,
    bm: &Bookmark,
    resolutions: &[Resolution],
    repo_path: &std::path::Path,
    current_head: Option<&str>,
) -> Result<String, handlebars::RenderError> {
    let handlebars = create_handlebars_engine();
    let template = load_template(template_name);
    let context = BookmarkTemplateContext::from_bookmark(bm, resolutions, repo_path, current_head);
    handlebars.render_template(&template, &context)
}

/// Render a collection overview using the collection overview template,
/// loading it from disk (or the bundled default).
pub fn render_collection_overview(
    collection: &Collection,
    bookmarks: &[Bookmark],
    tags: Vec<String>,
    links: &[CollectionLink],
) -> Result<String, handlebars::RenderError> {
    let template = load_template(COLLECTION_OVERVIEW_TEMPLATE);
    render_collection_overview_with_template(&template, collection, bookmarks, tags, links)
}

/// Render a collection overview using a caller-supplied template string.
///
/// Lets callers that cache the template (e.g. the TUI right pane) avoid a disk
/// read on every render while still sharing context construction + engine setup.
pub fn render_collection_overview_with_template(
    template: &str,
    collection: &Collection,
    bookmarks: &[Bookmark],
    tags: Vec<String>,
    links: &[CollectionLink],
) -> Result<String, handlebars::RenderError> {
    let handlebars = create_handlebars_engine();
    let context = CollectionTemplateContext::from_collection(collection, bookmarks, tags, links);
    handlebars.render_template(template, &context)
}

/// Render a bookmark with its resolutions using the show template (backward compatibility).
pub fn render_show_template(
    bm: &Bookmark,
    resolutions: &[Resolution],
    repo_path: &std::path::Path,
    current_head: Option<&str>,
) -> Result<String, handlebars::RenderError> {
    render_template(SHOW_TEMPLATE, bm, resolutions, repo_path, current_head)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::bookmark::{Bookmark, BookmarkHealth, Resolution, ResolutionMethod};
    use crate::git::context as git_context;
    use std::path::Path;

    #[test]
    fn test_render_collection_overview() {
        use crate::engine::bookmark::{
            Annotation, Collection, CollectionHealth, CollectionLink, CollectionLinkKind,
            Visibility,
        };

        let collection = Collection {
            id: "col-1".to_string(),
            name: "Auth Flow".to_string(),
            description: Some("How login works end to end".to_string()),
            visibility: Visibility::Private,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            created_by: Some("alice".to_string()),
            created_branch: Some("main".to_string()),
            published_at: None,
            published_commit_sha: None,
            repo_url: None,
            repo_id: None,
            status: None,
            health: Some(CollectionHealth::Active),
            health_computed_at: None,
            updated_at: None,
            imported_from_url: None,
        };

        let bm = Bookmark {
            id: "bm-1abcdef0".to_string(),
            query: "(function_definition name: (identifier) @target)".to_string(),
            language: "rust".to_string(),
            file_path: "src/auth/login.rs".to_string(),
            content_hash: None,
            commit_hash: None,
            health: BookmarkHealth::Active,
            resolution_method: None,
            last_resolved_at: None,
            stale_since: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            created_by: None,
            current_resolution_id: None,
            repo_id: None,
            tags: vec![],
            annotations: vec![Annotation {
                id: "ann-1".to_string(),
                bookmark_id: "bm-1abcdef0".to_string(),
                added_at: "2024-01-01T00:00:00Z".to_string(),
                added_by: None,
                notes: Some("Login entry point".to_string()),
                context: None,
                source: None,
            }],
            comments: vec![],
        };

        let links = vec![CollectionLink {
            id: "link-1".to_string(),
            collection_id: "col-1".to_string(),
            kind: CollectionLinkKind::Pr,
            label: "Add login".to_string(),
            url: "https://example.com/pr/1".to_string(),
            sort_order: 0,
            added_at: "2024-01-01T00:00:00Z".to_string(),
            added_by: None,
        }];

        // Render against the compiled-in default template (not any on-disk cache)
        // so the assertions are deterministic regardless of the user's config dir.
        let result = render_collection_overview_with_template(
            default_collection_overview_template(),
            &collection,
            std::slice::from_ref(&bm),
            vec!["auth".to_string(), "demo".to_string()],
            &links,
        );
        assert!(result.is_ok(), "Overview rendering failed: {:?}", result.err());
        let output = result.unwrap();

        assert!(output.contains("# Auth Flow"));
        assert!(output.contains("How login works end to end"));
        assert!(output.contains("| **Steps** | 1 |"));
        assert!(output.contains("| **Branch** | main |"));
        assert!(output.contains("| **Visibility** | private |"));
        assert!(output.contains("| **Health** | active |"));
        // Tags rendered inline with # prefix
        assert!(output.contains("#auth"));
        assert!(output.contains("#demo"));
        // Link rendered with kind
        assert!(output.contains("**pr**"));
        assert!(output.contains("https://example.com/pr/1"));
        // Step list shows the filename, the short bookmark ID, and the first note
        assert!(output.contains("login.rs"));
        assert!(output.contains("bm-1abcd"), "step should show short bookmark id; got:\n{output}");
        assert!(output.contains("Login entry point"), "step should show first note; got:\n{output}");
    }

    #[test]
    fn test_escape_markdown() {
        // Underscore is escaped in markdown
        assert_eq!(escape_markdown("hello_world"), "hello\\_world");
        assert_eq!(escape_markdown("test `code`"), "test \\`code\\`");
        assert_eq!(escape_markdown("*bold*"), "\\*bold\\*");
        // Characters that don't need escaping
        assert_eq!(escape_markdown("hello world"), "hello world");
        assert_eq!(escape_markdown("test/value"), "test/value");
    }

    #[test]
    fn test_short_id() {
        assert_eq!(short_id_value("abcdef1234567890"), "abcdef12");
        assert_eq!(short_id_value("short"), "short");
    }

    #[test]
    fn test_render_show_template() {
        let bm = Bookmark {
            id: "abcdef1234567890".to_string(),
            query: "(function_definition name: (identifier) @name)".to_string(),
            language: "rust".to_string(),
            file_path: "/path/to/file.rs".to_string(),
            content_hash: None,
            commit_hash: Some("commit1234567890".to_string()),
            health: BookmarkHealth::Active,
            resolution_method: Some(ResolutionMethod::Exact),
            last_resolved_at: Some("2024-01-01T00:00:00Z".to_string()),
            stale_since: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            created_by: Some("user".to_string()),
            current_resolution_id: None,
            repo_id: None,
            tags: vec!["tag1".to_string(), "tag2".to_string()],
            annotations: vec![],
            comments: vec![],
        };

        let is_dirty = !git_context::is_clean(Path::new(".")).unwrap_or(true);
        let resolutions = vec![Resolution {
            id: "res1".to_string(),
            bookmark_id: bm.id.clone(),
            resolved_at: "2024-01-01T00:00:00Z".to_string(),
            health: BookmarkHealth::Active,
            commit_hash: Some("commit1234567890".to_string()),
            method: ResolutionMethod::Exact,
            match_count: Some(1),
            file_path: Some("/path/to/file.rs".to_string()),
            byte_range: None,
            line_range: Some("10-20".to_string()),
            content_hash: None,
            headline: None,
            snapshot: Some("fn main() {\n    println!(\"Hello\");\n}".to_string()),
            breadcrumbs: None,
            is_dirty,
        }];

        // Verify snapshot is present in resolution
        assert!(resolutions[0].snapshot.is_some(), "Snapshot should be Some");
        assert_eq!(
            resolutions[0].snapshot.as_ref().unwrap(),
            "fn main() {\n    println!(\"Hello\");\n}"
        );

        let result = render_show_template(&bm, &resolutions, Path::new("."), None);
        assert!(result.is_ok(), "Template rendering failed: {:?}", result.err());
        let output = result.unwrap();
        assert!(output.contains("# Bookmark: abcdef12"));
        assert!(output.contains("| **File** | /path/to/file.rs |"));
        assert!(output.contains("| **Language** | rust |"));
        assert!(output.contains("| **Status** | active |"));
        assert!(output.contains("## Tags"));
        // Tags are rendered inline with # prefix
        assert!(output.contains("#tag1"));
        assert!(output.contains("#tag2"));
        // Resolution status should be present
        assert!(output.contains("active"));
    }

    #[test]
    fn test_render_show_template_includes_comments() {
        let bm = Bookmark {
            id: "abcdef1234567890".to_string(),
            query: "(function_definition) @t".to_string(),
            language: "rust".to_string(),
            file_path: "/path/to/file.rs".to_string(),
            content_hash: None,
            commit_hash: None,
            health: BookmarkHealth::Active,
            resolution_method: Some(ResolutionMethod::Exact),
            last_resolved_at: None,
            stale_since: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            created_by: Some("agent".to_string()),
            current_resolution_id: None,
            repo_id: None,
            tags: vec![],
            annotations: vec![],
            comments: vec![crate::engine::bookmark::BookmarkComment {
                id: "c1".to_string(),
                bookmark_id: "abcdef1234567890".to_string(),
                author: "agent".to_string(),
                body: "**Ticket ABC-123**: investigate".to_string(),
                created_at: "2024-01-02T00:00:00Z".to_string(),
                parent_id: None,
            }],
        };

        let ctx = BookmarkTemplateContext::from_bookmark(&bm, &[], Path::new("."), None);
        assert_eq!(ctx.comments.len(), 1, "comments should be mapped into the context");

        let handlebars = create_handlebars_engine();
        let template = default_show_template();
        let output = handlebars.render_template(template, &ctx).unwrap();
        assert!(output.contains("## Comments"), "missing Comments section:\n{output}");
        assert!(output.contains("**Ticket ABC-123**: investigate"), "missing comment body");
        assert!(output.contains("*— agent,"), "missing comment attribution");
    }

    #[test]
    fn test_template_ui_status_healthy_at_head() {
        // When a bookmark has a current resolution at HEAD with is_dirty=false
        // and health=Active, the projected ui_status should be "healthy".
        let tmp = std::env::temp_dir()
            .join(format!("codemark_test_tpl_healthy_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();

        // Create a two-commit repo so we have a known HEAD
        let run = |args: &[&str]| {
            let status =
                std::process::Command::new("git").args(args).current_dir(&tmp).status().unwrap();
            assert!(status.success());
        };
        run(&["init"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "test"]);
        std::fs::write(tmp.join("file.rs"), "fn main() {}").unwrap();
        run(&["add", "file.rs"]);
        run(&["commit", "-m", "initial"]);
        let head = String::from_utf8(
            std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(&tmp)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();

        let bm = Bookmark {
            id: "bm-1".to_string(),
            query: "test".to_string(),
            language: "rust".to_string(),
            file_path: "file.rs".to_string(),
            content_hash: None,
            commit_hash: None,
            health: BookmarkHealth::Active,
            resolution_method: None,
            last_resolved_at: None,
            stale_since: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            created_by: None,
            current_resolution_id: Some("res-1".to_string()),
            repo_id: None,
            tags: vec![],
            annotations: vec![],
            comments: vec![],
        };

        let resolution = Resolution {
            id: "res-1".to_string(),
            bookmark_id: "bm-1".to_string(),
            resolved_at: "2024-01-01T00:00:00Z".to_string(),
            health: BookmarkHealth::Active,
            commit_hash: Some(head.clone()),
            method: ResolutionMethod::Exact,
            match_count: Some(1),
            file_path: Some("file.rs".to_string()),
            byte_range: None,
            line_range: None,
            content_hash: None,
            headline: None,
            snapshot: None,
            breadcrumbs: None,
            is_dirty: false,
        };

        let ctx = BookmarkTemplateContext::from_bookmark(&bm, &[resolution], &tmp, Some(&head));
        assert_eq!(ctx.ui_status, Some("healthy".to_string()));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_template_ui_status_none_without_resolution_id() {
        // When bookmark has no current_resolution_id, ui_status should be None
        let bm = Bookmark {
            id: "bm-2".to_string(),
            query: "test".to_string(),
            language: "rust".to_string(),
            file_path: "file.rs".to_string(),
            content_hash: None,
            commit_hash: None,
            health: BookmarkHealth::Active,
            resolution_method: None,
            last_resolved_at: None,
            stale_since: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            created_by: None,
            current_resolution_id: None,
            repo_id: None,
            tags: vec![],
            annotations: vec![],
            comments: vec![],
        };

        let ctx = BookmarkTemplateContext::from_bookmark(&bm, &[], Path::new("."), None);
        assert_eq!(ctx.ui_status, None);
    }

    #[test]
    fn test_template_ui_status_none_when_resolution_missing() {
        // When bookmark points to a resolution ID that doesn't exist in the list
        let bm = Bookmark {
            id: "bm-3".to_string(),
            query: "test".to_string(),
            language: "rust".to_string(),
            file_path: "file.rs".to_string(),
            content_hash: None,
            commit_hash: None,
            health: BookmarkHealth::Active,
            resolution_method: None,
            last_resolved_at: None,
            stale_since: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            created_by: None,
            current_resolution_id: Some("nonexistent-res".to_string()),
            repo_id: None,
            tags: vec![],
            annotations: vec![],
            comments: vec![],
        };

        let ctx = BookmarkTemplateContext::from_bookmark(&bm, &[], Path::new("."), None);
        assert_eq!(ctx.ui_status, None);
    }

    #[test]
    fn test_template_ui_status_drifted() {
        // When health=Drifted at HEAD, ui_status should be "drifted"
        let tmp = std::env::temp_dir()
            .join(format!("codemark_test_tpl_drifted_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();

        let run = |args: &[&str]| {
            let status =
                std::process::Command::new("git").args(args).current_dir(&tmp).status().unwrap();
            assert!(status.success());
        };
        run(&["init"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "test"]);
        std::fs::write(tmp.join("file.rs"), "fn main() {}").unwrap();
        run(&["add", "file.rs"]);
        run(&["commit", "-m", "initial"]);
        let head = String::from_utf8(
            std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(&tmp)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();

        let bm = Bookmark {
            id: "bm-4".to_string(),
            query: "test".to_string(),
            language: "rust".to_string(),
            file_path: "file.rs".to_string(),
            content_hash: None,
            commit_hash: None,
            health: BookmarkHealth::Drifted,
            resolution_method: None,
            last_resolved_at: None,
            stale_since: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            created_by: None,
            current_resolution_id: Some("res-4".to_string()),
            repo_id: None,
            tags: vec![],
            annotations: vec![],
            comments: vec![],
        };

        let resolution = Resolution {
            id: "res-4".to_string(),
            bookmark_id: "bm-4".to_string(),
            resolved_at: "2024-01-01T00:00:00Z".to_string(),
            health: BookmarkHealth::Drifted,
            commit_hash: Some(head.clone()),
            method: ResolutionMethod::Relaxed,
            match_count: Some(1),
            file_path: Some("file.rs".to_string()),
            byte_range: None,
            line_range: None,
            content_hash: None,
            headline: None,
            snapshot: None,
            breadcrumbs: None,
            is_dirty: false,
        };

        let ctx = BookmarkTemplateContext::from_bookmark(&bm, &[resolution], &tmp, Some(&head));
        assert_eq!(ctx.ui_status, Some("drifted".to_string()));

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
