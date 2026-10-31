//! Handlebars template support for markdown output.
//!
//! Templates are stored in `~/.config/codemark/templates/` directory and can be customized by users.
//! The default template is bundled into the binary at compile time from `templates/codemark_show.md`
//! in the project root.

use std::collections::HashMap;
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
    /// The request this collection answers, kept so it can be recreated (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// `prompt` with a `>` prefix on every line, ready to drop into a markdown
    /// block quote. Pre-rendered here rather than in the template because a
    /// single `>` only quotes the first line: a prompt containing a blank line
    /// would push its later paragraphs out of the quote and make them read as
    /// separate collection content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_quoted: Option<String>,
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

/// Prefix every line of `text` with `>` so a multi-paragraph string renders as
/// one continuous markdown block quote.
///
/// Blank lines get a bare `>` rather than being left empty: an empty line
/// terminates a block quote, so the text after it would escape the quote and
/// render as ordinary body content.
fn quote_markdown_block(text: &str) -> String {
    text.lines()
        .map(|line| if line.is_empty() { ">".to_string() } else { format!("> {line}") })
        .collect::<Vec<_>>()
        .join("\n")
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
            prompt: collection.prompt.clone(),
            prompt_quoted: collection.prompt.as_deref().map(quote_markdown_block),
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

/// Wrap text in a markdown code span that renders it verbatim.
///
/// Source text is the wrong input for `escape_markdown`: a signature like
/// `impl<T> Service<T> {` would come back as backslash soup, and left bare its
/// `<T>` is parsed as inline HTML, which the TUI markdown reader drops — so the
/// displayed signature silently loses characters. A code span preserves it
/// exactly.
///
/// Per CommonMark, the fence must be longer than the longest backtick run in
/// the content, and content that starts or ends with a backtick needs padding
/// spaces (which the reader strips back out).
fn code_span(text: &str) -> String {
    let longest_run = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest_run + 1);
    // An empty span also needs the padding, or the two fences read as one
    // four-backtick literal rather than as a span.
    let pad =
        if text.is_empty() || text.starts_with('`') || text.ends_with('`') { " " } else { "" };
    format!("{fence}{pad}{text}{pad}{fence}")
}

/// Helper function for Handlebars to render text as a verbatim code span.
#[derive(Clone)]
struct CodeSpanHelper;

impl HelperDef for CodeSpanHelper {
    fn call<'reg: 'rc, 'rc>(
        &self,
        h: &handlebars::Helper<'rc>,
        _r: &'reg Handlebars<'reg>,
        _ctx: &'rc handlebars::Context,
        _rc: &mut RenderContext<'reg, 'rc>,
        out: &mut dyn Output,
    ) -> HelperResult {
        let param = h.param(0).ok_or_else(|| {
            RenderErrorReason::Other("code_span helper requires exactly one parameter".into())
        })?;
        let value = param.value().as_str().ok_or_else(|| {
            RenderErrorReason::Other("code_span helper parameter must be a string".into())
        })?;
        out.write(&code_span(value))?;
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

/// Where the hashes of codemark-written templates live.
///
/// One `<name> <hash>` line per template, recording what *codemark* wrote. A
/// file whose current content still hashes to its recorded value has not been
/// edited, so a newer default can replace it; anything else is the user's and
/// is left alone. See [`ensure_default_template_exists`].
const TEMPLATE_STAMP_FILE: &str = ".shipped-versions";

/// Stable, short digest of a template's bytes.
///
/// FNV-1a rather than `DefaultHasher`, whose output is explicitly not stable
/// across Rust releases — a stamp that changed with the toolchain would make
/// every template look edited. Not security-sensitive: this only distinguishes
/// "byte-identical to what we wrote" from "something else".
fn template_digest(content: &str) -> String {
    const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = FNV_OFFSET_BASIS;
    for byte in content.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("{hash:016x}")
}

/// Read the recorded digests, if any.
fn read_template_stamps(dir: &std::path::Path) -> HashMap<String, String> {
    let Ok(text) = std::fs::read_to_string(dir.join(TEMPLATE_STAMP_FILE)) else {
        return HashMap::new();
    };
    text.lines()
        .filter_map(|line| {
            let (name, hash) = line.split_once(' ')?;
            Some((name.to_string(), hash.trim().to_string()))
        })
        .collect()
}

/// Rewrite the digest file.
///
/// A failure here fails safe — a template whose digest went unrecorded looks
/// user-edited next time, so it is preserved rather than overwritten — but it
/// is not harmless: that template can then never be refreshed automatically,
/// and the user has to delete it by hand to pick up a new default. So the
/// error is reported rather than swallowed, while the run continues.
fn write_template_stamps(dir: &std::path::Path, stamps: &HashMap<String, String>) {
    let mut lines: Vec<String> =
        stamps.iter().map(|(name, hash)| format!("{name} {hash}")).collect();
    lines.sort();
    let path = dir.join(TEMPLATE_STAMP_FILE);
    if let Err(e) = write_atomic(&path, &(lines.join("\n") + "\n")) {
        tracing::warn!(
            target: "codemark::templates",
            path = %path.display(),
            error = %e,
            "failed to record template digests"
        );
        eprintln!(
            "Warning: Failed to record template versions to {}: {e}\n  \
             Templates will be treated as customized until this is fixed.",
            path.display()
        );
    }
}

/// Replace `path`'s contents in one step.
///
/// [`ensure_default_template_exists`] runs on every CLI start, so two codemark
/// processes can write these files concurrently — and now that an upgrade
/// *rewrites* existing templates, a plain `fs::write` lets a reader see a
/// truncated file: a half-written template renders wrong, and a half-written
/// stamp file makes untouched templates look edited. Writing to a sibling
/// temporary file and renaming over the destination makes the swap atomic.
///
/// The temp name includes the pid so concurrent writers don't clobber each
/// other's staging file.
///
/// **Symlinks are followed, not replaced.** `rename` would swap out the link
/// itself, so someone keeping a template symlinked into their dotfiles would
/// find the link quietly turned into a regular file and their managed copy
/// disconnected. `fs::write` followed the link, and that behaviour is
/// preserved by resolving the target first. A broken link has no target to
/// write through, so it is replaced.
fn write_atomic(path: &std::path::Path, contents: &str) -> std::io::Result<()> {
    let target = match std::fs::canonicalize(path) {
        Ok(resolved) => resolved,
        // Nothing there yet, or a dangling link: write the path as given.
        Err(_) => path.to_path_buf(),
    };

    let file_name = target.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let tmp = target.with_file_name(format!(".{file_name}.tmp.{}", std::process::id()));
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, &target).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Template names
pub const SHOW_TEMPLATE: &str = "codemark_show.md";
pub const DETAILS_TEMPLATE: &str = "details_panel.md";
pub const COLLECTION_OVERVIEW_TEMPLATE: &str = "codemark_collection_overview.md";
pub const COMMENTS_TEMPLATE: &str = "comments_panel.md";
pub const REMOTE_TOUR_OVERVIEW_TEMPLATE: &str = "codemark_remote_tour_overview.md";

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

/// Get the default markdown template for the comments panel.
pub fn default_comments_template() -> &'static str {
    include_str!("../../../templates/comments_panel.md")
}

/// Get the default markdown template for the remote tour overview panel.
pub fn default_remote_tour_overview_template() -> &'static str {
    include_str!("../../../templates/codemark_remote_tour_overview.md")
}

/// Get the default content for a given template name.
pub fn default_template_content(name: &str) -> Option<&'static str> {
    match name {
        SHOW_TEMPLATE => Some(default_show_template()),
        DETAILS_TEMPLATE => Some(default_details_template()),
        COLLECTION_OVERVIEW_TEMPLATE => Some(default_collection_overview_template()),
        COMMENTS_TEMPLATE => Some(default_comments_template()),
        REMOTE_TOUR_OVERVIEW_TEMPLATE => Some(default_remote_tour_overview_template()),
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
    handlebars.register_helper("code_span", Box::new(CodeSpanHelper));
    handlebars.register_helper("truncate", Box::new(TruncateHelper));
    handlebars.register_helper("format_date", Box::new(FormatDateHelper));

    handlebars
}

/// Every template codemark ships.
pub const ALL_TEMPLATES: &[&str] = &[
    SHOW_TEMPLATE,
    DETAILS_TEMPLATE,
    COLLECTION_OVERVIEW_TEMPLATE,
    COMMENTS_TEMPLATE,
    REMOTE_TOUR_OVERVIEW_TEMPLATE,
];

/// A template on disk that is older than the one this build ships, and that the
/// user has edited — so it cannot be refreshed automatically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleTemplate {
    pub name: String,
    pub path: PathBuf,
}

/// Write the shipped templates to the user's config directory, refreshing any
/// that are out of date.
///
/// Returns the templates that are stale *and* locally modified, so the caller
/// can tell the user rather than silently overwriting their work.
///
/// **Why this is not just "write if absent".** A template on disk always wins
/// in [`load_template`], so a copy written by an older codemark shadows the
/// compiled-in default forever. When a release adds a section — breadcrumbs,
/// say — every existing user keeps the old rendering and sees nothing new. The
/// file is not a cache they know about; it is a customization point they may
/// never have touched.
///
/// So each file falls into one of three cases, decided by the digest recorded
/// when codemark last wrote it:
///
/// | On disk | Action |
/// |---|---|
/// | absent | write it, record the digest |
/// | matches the recorded digest (untouched) | overwrite with the new default |
/// | differs (user edited it) | leave it, and report it |
/// | unreadable (not UTF-8, no permission) | leave it, and report it |
///
/// A file with no recorded digest is treated as edited. That is deliberate: it
/// is what every pre-existing install looks like, and guessing wrong in that
/// direction loses someone's customization. An *unreadable* file is treated the
/// same way and for the same reason: a failed read says nothing about whether
/// the content is ours, so it cannot be assumed replaceable.
pub fn ensure_default_template_exists() -> Vec<StaleTemplate> {
    let Some(templates_dir) = templates_dir() else {
        return Vec::new();
    };
    ensure_default_templates_in(&templates_dir)
}

/// [`ensure_default_template_exists`] against an explicit directory.
///
/// Split out so tests can drive the logic on a temp directory instead of
/// pointing `templates_dir()` at one through `XDG_CONFIG_HOME` — that variable
/// is process-global, so tests that set it interfere with every other test
/// reading a template.
fn ensure_default_templates_in(templates_dir: &std::path::Path) -> Vec<StaleTemplate> {
    if let Err(e) = std::fs::create_dir_all(templates_dir) {
        tracing::warn!(
            target: "codemark::templates",
            dir = %templates_dir.display(),
            error = %e,
            "failed to create templates directory"
        );
        eprintln!("Warning: Failed to create templates directory: {e}");
        return Vec::new();
    }

    let mut stamps = read_template_stamps(templates_dir);
    let mut stale = Vec::new();
    let mut stamps_changed = false;

    for name in ALL_TEMPLATES {
        let Some(shipped) = default_template_content(name) else { continue };
        let path = templates_dir.join(name);
        let shipped_digest = template_digest(shipped);

        // Only a missing file counts as absent. Any other read error — a
        // non-UTF-8 edit, a permission problem — means a file *is* there and
        // we cannot tell whose it is, so it is left alone and reported.
        // Treating it as absent would overwrite a customization silently,
        // which is exactly what this function exists to avoid.
        let write = match std::fs::read_to_string(&path) {
            // Nothing there: first run, or a template added by this release.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::debug!(
                    target: "codemark::templates",
                    template = %name,
                    "template absent, writing shipped default"
                );
                true
            }
            Err(e) => {
                tracing::warn!(
                    target: "codemark::templates",
                    template = %name,
                    path = %path.display(),
                    error = %e,
                    "template unreadable, preserving it"
                );
                eprintln!(
                    "Warning: Failed to read template {}: {e}\n  Leaving it as it is.",
                    path.display()
                );
                stale.push(StaleTemplate { name: (*name).to_string(), path: path.clone() });
                false
            }
            // Already current, whether or not the user wrote it themselves.
            Ok(current) if template_digest(&current) == shipped_digest => {
                if stamps.get(*name) != Some(&shipped_digest) {
                    tracing::debug!(
                        target: "codemark::templates",
                        template = %name,
                        "template already current, recording digest"
                    );
                    stamps.insert((*name).to_string(), shipped_digest.clone());
                    stamps_changed = true;
                }
                false
            }
            // Byte-identical to what codemark last wrote, so it is ours to
            // replace: the user never touched it and would otherwise be stuck
            // on an old rendering.
            Ok(current) if stamps.get(*name) == Some(&template_digest(&current)) => {
                tracing::debug!(
                    target: "codemark::templates",
                    template = %name,
                    "template untouched since codemark wrote it, refreshing"
                );
                true
            }
            // Edited, or written before stamps existed. Not ours to overwrite.
            Ok(_) => {
                tracing::debug!(
                    target: "codemark::templates",
                    template = %name,
                    path = %path.display(),
                    had_digest = stamps.contains_key(*name),
                    "template customized, preserving it and reporting as stale"
                );
                stale.push(StaleTemplate { name: (*name).to_string(), path: path.clone() });
                false
            }
        };

        if write {
            match write_atomic(&path, shipped) {
                Ok(()) => {
                    stamps.insert((*name).to_string(), shipped_digest);
                    stamps_changed = true;
                }
                Err(e) => {
                    tracing::warn!(
                        target: "codemark::templates",
                        template = %name,
                        path = %path.display(),
                        error = %e,
                        "failed to write default template"
                    );
                    eprintln!(
                        "Warning: Failed to write default template to {}: {e}",
                        path.display()
                    )
                }
            }
        }
    }

    if stamps_changed {
        write_template_stamps(templates_dir, &stamps);
    }

    stale
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

/// Template context for rendering a remote tour overview (pre-pull preview).
#[derive(Debug, Serialize)]
struct RemoteTourTemplateContext {
    title: String,
    author: Option<String>,
    repo_url: Option<String>,
    updated_at: Option<String>,
}

impl RemoteTourTemplateContext {
    fn from_summary(tour: &crate::sync::RemoteTourSummary) -> Self {
        // Normalize empty strings to None so the template's `{{#if}}` guards hide
        // the rows entirely rather than rendering blank cells.
        let non_empty = |s: &str| (!s.is_empty()).then(|| s.to_string());
        Self {
            title: tour.title.clone(),
            author: tour.author.as_deref().and_then(non_empty),
            repo_url: tour.repo_url.as_deref().and_then(non_empty),
            updated_at: non_empty(&tour.updated_at),
        }
    }
}

/// Render a remote tour overview, loading the template from disk (or the bundled
/// default). Used to preview a server-hosted tour before it is pulled locally.
pub fn render_remote_tour_overview(tour: &crate::sync::RemoteTourSummary) -> String {
    let template = load_template(REMOTE_TOUR_OVERVIEW_TEMPLATE);
    let handlebars = create_handlebars_engine();
    let context = RemoteTourTemplateContext::from_summary(tour);
    handlebars
        .render_template(&template, &context)
        .unwrap_or_else(|e| format!("# {}\n\nError rendering overview: {}", tour.title, e))
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
            prompt: None,
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
        // Step list shows the filename and the human-readable query summary
        assert!(output.contains("login.rs"));
        assert!(
            output.contains("— identifier"),
            "step should show the query summary; got:\n{output}"
        );
    }

    // @lat: [[tests#Collection Creation Prompt#Multi-Paragraph Prompt Stays Quoted]]
    #[test]
    fn multiline_prompt_renders_as_one_block_quote() {
        use crate::engine::bookmark::{Collection, Visibility};

        let collection = Collection {
            id: "col-1".to_string(),
            name: "Auth Flow".to_string(),
            description: None,
            visibility: Visibility::Private,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            created_by: None,
            created_branch: None,
            published_at: None,
            published_commit_sha: None,
            repo_url: None,
            repo_id: None,
            status: None,
            health: None,
            health_computed_at: None,
            updated_at: None,
            imported_from_url: None,
            // A blank line between paragraphs is the case that breaks a single
            // `>` prefix: the second paragraph would escape the quote.
            prompt: Some(
                "Trace the login path.\n\nFocus on the 2FA branch added in ENG-42.".to_string(),
            ),
        };

        let output = render_collection_overview_with_template(
            default_collection_overview_template(),
            &collection,
            &[],
            vec![],
            &[],
        )
        .expect("overview renders");

        assert!(output.contains("## Prompt"), "prompt section present; got:\n{output}");
        // Every line of the prompt is inside the quote, blank line included.
        assert!(output.contains("> Trace the login path."), "first paragraph; got:\n{output}");
        assert!(
            output.contains("> Focus on the 2FA branch added in ENG-42."),
            "second paragraph must stay quoted; got:\n{output}"
        );
        assert!(
            output.contains("> Trace the login path.\n>\n> Focus on the 2FA branch"),
            "paragraphs must form one continuous quote; got:\n{output}"
        );
        // No unquoted copy of the later paragraph leaked into the body.
        assert!(
            !output.contains("\nFocus on the 2FA branch"),
            "prompt text must not appear outside the quote; got:\n{output}"
        );
    }

    #[test]
    fn quote_markdown_block_prefixes_every_line() {
        assert_eq!(quote_markdown_block("one"), "> one");
        assert_eq!(quote_markdown_block("one\ntwo"), "> one\n> two");
        // Blank lines become a bare `>` so the quote isn't terminated.
        assert_eq!(quote_markdown_block("one\n\ntwo"), "> one\n>\n> two");
        assert_eq!(quote_markdown_block(""), "");
    }

    #[test]
    fn test_render_remote_tour_overview() {
        use crate::sync::RemoteTourSummary;

        let tour = RemoteTourSummary {
            tour_id: "tour-123".to_string(),
            title: "Onboarding Tour".to_string(),
            repo_url: Some("https://github.com/acme/widgets".to_string()),
            author: Some("octocat".to_string()),
            updated_at: "2024-05-01T12:30:00Z".to_string(),
        };

        let output = render_remote_tour_overview(&tour);
        assert!(output.contains("# Onboarding Tour"), "title heading; got:\n{output}");
        assert!(output.contains("| **Author** | octocat |"), "author row; got:\n{output}");
        // Repo URL renders as a markdown link; the visible label is markdown-escaped
        // (so the `.` is escaped) while the link target stays raw.
        assert!(
            output.contains(
                r"| **Repo** | [https://github\.com/acme/widgets](https://github.com/acme/widgets) |"
            ),
            "repo row; got:\n{output}"
        );
        assert!(output.contains("2024-05-01"), "formatted updated date; got:\n{output}");
    }

    #[test]
    fn test_render_remote_tour_overview_escapes_metadata() {
        use crate::sync::RemoteTourSummary;

        // Remote (untrusted) values containing markdown control characters must
        // be escaped so they can't break the heading/table layout.
        let tour = RemoteTourSummary {
            tour_id: "tour-789".to_string(),
            title: "Pipe | Tour".to_string(),
            repo_url: None,
            author: Some("a|b#c".to_string()),
            updated_at: String::new(),
        };

        let output = render_remote_tour_overview(&tour);
        assert!(output.contains(r"# Pipe \| Tour"), "escaped title; got:\n{output}");
        assert!(output.contains(r"a\|b\#c"), "escaped author; got:\n{output}");
    }

    #[test]
    fn test_render_remote_tour_overview_omits_empty_fields() {
        use crate::sync::RemoteTourSummary;

        let tour = RemoteTourSummary {
            tour_id: "tour-456".to_string(),
            title: "Bare Tour".to_string(),
            repo_url: None,
            author: Some(String::new()),
            updated_at: String::new(),
        };

        let output = render_remote_tour_overview(&tour);
        assert!(output.contains("# Bare Tour"));
        // Empty/None metadata must not render blank table rows.
        assert!(!output.contains("**Author**"), "empty author hidden; got:\n{output}");
        assert!(!output.contains("**Repo**"), "missing repo hidden; got:\n{output}");
        assert!(!output.contains("**Updated**"), "empty date hidden; got:\n{output}");
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

    /// Breadcrumb text is source code, so it must survive the markdown round
    /// trip verbatim. Left bare, a generic signature's `<T>` parses as inline
    /// HTML and the TUI reader drops it; run through `escape_markdown` instead,
    /// it comes back as backslash soup.
    #[test]
    fn test_code_span_preserves_source_verbatim() {
        // The motivating case: generics must not be read as HTML tags.
        assert_eq!(code_span("impl<T> Service<T> {"), "`impl<T> Service<T> {`");
        // Underscores and asterisks are emphasis markers outside a code span.
        assert_eq!(code_span("fn send_to(*ptr) {"), "`fn send_to(*ptr) {`");
        // A backtick in the source needs a longer fence than the longest run.
        assert_eq!(code_span("let s = \"a`b\";"), "``let s = \"a`b\";``");
        assert_eq!(code_span("x = ``y``;"), "```x = ``y``;```");
        // Leading/trailing backticks need padding spaces, which readers strip.
        assert_eq!(code_span("`quoted`"), "`` `quoted` ``");
        // Breadcrumb text is never empty in practice, but the helper must not
        // emit a bare "``" that reads as a literal instead of a span.
        assert_eq!(code_span(""), "`  `");
    }

    /// Pin the digest against published FNV-1a 64-bit vectors.
    ///
    /// A hash that is merely self-consistent is not enough: the first version
    /// of this used `0x1000_0000_01b3` — one zero too many — and every test
    /// still passed, because both sides of every comparison used the same
    /// wrong constant. It only showed up against an independent implementation.
    #[test]
    fn template_digest_is_fnv1a_64() {
        assert_eq!(template_digest(""), "cbf29ce484222325");
        assert_eq!(template_digest("a"), "af63dc4c8601ec8c");
        assert_eq!(template_digest("foobar"), "85944171f73967e8");
    }

    /// A template codemark wrote and the user never touched must be refreshed
    /// when a release changes it — otherwise a new section (breadcrumbs, say)
    /// is invisible to every existing install. See #256.
    #[test]
    fn an_untouched_template_is_refreshed() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("templates");

        // First run: writes the shipped templates and records their digests.
        assert!(ensure_default_templates_in(&dir).is_empty());
        let shipped = std::fs::read_to_string(dir.join(SHOW_TEMPLATE)).unwrap();

        // Age it exactly as an older codemark would have: the old content on
        // disk, and a stamp recording that codemark wrote that old content.
        let old = "# Bookmark: {{short_id}}\n\n## Metadata\n";
        std::fs::write(dir.join(SHOW_TEMPLATE), old).unwrap();
        let mut stamps = read_template_stamps(&dir);
        stamps.insert(SHOW_TEMPLATE.to_string(), template_digest(old));
        write_template_stamps(&dir, &stamps);

        let stale = ensure_default_templates_in(&dir);

        assert!(stale.is_empty(), "an untouched template should not be reported: {stale:?}");
        assert_eq!(
            std::fs::read_to_string(dir.join(SHOW_TEMPLATE)).unwrap(),
            shipped,
            "an untouched template must be refreshed to the shipped default"
        );
    }

    /// A template the user edited must never be overwritten, even when it is
    /// older than the shipped default — it is a documented customization
    /// point, and losing someone's work is worse than an old rendering. It is
    /// reported instead, so the staleness is at least visible.
    #[test]
    fn an_edited_template_is_preserved_and_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("templates");

        ensure_default_templates_in(&dir);

        let mine = "# My own heading {{short_id}}\n";
        std::fs::write(dir.join(SHOW_TEMPLATE), mine).unwrap();

        let stale = ensure_default_templates_in(&dir);

        assert_eq!(std::fs::read_to_string(dir.join(SHOW_TEMPLATE)).unwrap(), mine);
        assert!(
            stale.iter().any(|s| s.name == SHOW_TEMPLATE),
            "an edited template should be reported: {stale:?}"
        );
    }

    /// An install predating the digest file looks exactly like an edited one,
    /// and is treated as such. That is the deliberate direction to guess
    /// wrong in: a stale rendering is recoverable, a deleted customization is
    /// not.
    #[test]
    fn a_template_with_no_recorded_digest_is_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("templates");
        std::fs::create_dir_all(&dir).unwrap();

        let old = "# Bookmark: {{short_id}}\n";
        std::fs::write(dir.join(SHOW_TEMPLATE), old).unwrap();

        let stale = ensure_default_templates_in(&dir);

        assert_eq!(std::fs::read_to_string(dir.join(SHOW_TEMPLATE)).unwrap(), old);
        assert!(stale.iter().any(|s| s.name == SHOW_TEMPLATE), "{stale:?}");
    }

    /// A template that exists but cannot be read must not be mistaken for an
    /// absent one. `read_to_string` fails on a file the user edited in a
    /// non-UTF-8 encoding; treating that failure as "nothing there" would
    /// overwrite their customization with the shipped default — the one
    /// outcome this whole function exists to prevent.
    #[test]
    fn an_unreadable_template_is_preserved_and_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("templates");
        std::fs::create_dir_all(&dir).unwrap();

        // Invalid UTF-8: a lone continuation byte. Written as bytes because
        // no `&str` can express it.
        let mine: &[u8] = b"# My heading \xff\n";
        std::fs::write(dir.join(SHOW_TEMPLATE), mine).unwrap();

        let stale = ensure_default_templates_in(&dir);

        assert_eq!(
            std::fs::read(dir.join(SHOW_TEMPLATE)).unwrap(),
            mine,
            "an unreadable template must not be overwritten"
        );
        assert!(
            stale.iter().any(|s| s.name == SHOW_TEMPLATE),
            "an unreadable template should be reported: {stale:?}"
        );
    }

    /// A template symlinked into the user's dotfiles must stay a symlink.
    ///
    /// The atomic write renames a temp file over the destination, and `rename`
    /// replaces the *link* rather than following it — so refreshing an
    /// untouched template would turn the link into a regular file and silently
    /// disconnect the user's managed copy. `write_atomic` resolves the target
    /// first to keep the plain-`fs::write` behaviour.
    #[cfg(unix)]
    #[test]
    fn refreshing_an_untouched_template_preserves_a_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("templates");
        std::fs::create_dir_all(&dir).unwrap();

        // The user's real file, living outside the templates dir.
        let dotfiles = tmp.path().join("dotfiles");
        std::fs::create_dir_all(&dotfiles).unwrap();
        let real = dotfiles.join("my_show.md");
        let old = "# Bookmark: {{short_id}}\n";
        std::fs::write(&real, old).unwrap();

        let link = dir.join(SHOW_TEMPLATE);
        std::os::unix::fs::symlink(&real, &link).unwrap();

        // Record that codemark wrote the linked-through content, so the file
        // counts as untouched and is due a refresh.
        let mut stamps = read_template_stamps(&dir);
        stamps.insert(SHOW_TEMPLATE.to_string(), template_digest(old));
        write_template_stamps(&dir, &stamps);

        let stale = ensure_default_templates_in(&dir);

        assert!(stale.is_empty(), "an untouched symlinked template is not stale: {stale:?}");
        assert!(
            std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink(),
            "the symlink must survive a refresh"
        );
        assert_eq!(
            std::fs::read_to_string(&real).unwrap(),
            default_show_template(),
            "the refresh must write through the link to the user's file"
        );
    }

    /// The stamp file and refreshed templates are swapped in whole, so a
    /// concurrent reader never sees a truncated file. Checks the observable
    /// consequence: no staging file is left behind after a successful write.
    #[test]
    fn writes_leave_no_temporary_files_behind() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("templates");

        ensure_default_templates_in(&dir);

        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp."))
            .collect();
        assert!(leftovers.is_empty(), "temporary files left behind: {leftovers:?}");
    }

    #[test]
    fn test_render_show_template_includes_breadcrumbs() {
        let bm = Bookmark {
            id: "abcdef1234567890".to_string(),
            query: "(function_item name: (identifier) @name)".to_string(),
            language: "rust".to_string(),
            file_path: "auth.rs".to_string(),
            content_hash: None,
            commit_hash: None,
            health: BookmarkHealth::Active,
            resolution_method: Some(ResolutionMethod::Exact),
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

        let resolutions = vec![Resolution {
            id: "res1".to_string(),
            bookmark_id: bm.id.clone(),
            resolved_at: "2024-01-01T00:00:00Z".to_string(),
            health: BookmarkHealth::Active,
            commit_hash: None,
            method: ResolutionMethod::Exact,
            match_count: Some(1),
            file_path: Some("auth.rs".to_string()),
            byte_range: None,
            line_range: Some("9".to_string()),
            content_hash: None,
            headline: None,
            snapshot: None,
            breadcrumbs: Some(
                r#"[{"line":1,"text":"mod auth {"},{"line":6,"text":"    impl Service {"}]"#
                    .to_string(),
            ),
            is_dirty: false,
        }];

        // Render against the compiled-in default rather than `load_template`,
        // which prefers a cached copy under the user's config dir. A developer
        // with an older cache would otherwise see this pass or fail based on
        // ambient state. The cache-shadowing problem itself is real and is
        // handled separately — see the note in codemark#249.
        let handlebars = create_handlebars_engine();
        let context =
            BookmarkTemplateContext::from_bookmark(&bm, &resolutions, Path::new("."), None);
        let output = handlebars.render_template(default_show_template(), &context).unwrap();

        assert!(output.contains("## Context"), "breadcrumbs section missing:\n{output}");
        // Assert each line number beside its own source text. Checking for a
        // bare '1' would pass even if the template stopped rendering numbers
        // entirely, since the bookmark ID and dates contain that digit too.
        assert!(
            output.contains("- `1` `mod auth {`"),
            "outer breadcrumb missing or unnumbered:\n{output}"
        );
        assert!(
            output.contains("- `6` `    impl Service {`"),
            "inner breadcrumb missing or unnumbered:\n{output}"
        );
    }

    /// A bookmark with no breadcrumbs must not render an empty section header.
    /// `containers` fails silently, so an always-present "Context" heading would
    /// make a broken profile indistinguishable from a top-level bookmark.
    #[test]
    fn test_render_show_template_omits_empty_breadcrumbs() {
        let bm = Bookmark {
            id: "abcdef1234567890".to_string(),
            query: "(function_item) @f".to_string(),
            language: "rust".to_string(),
            file_path: "main.rs".to_string(),
            content_hash: None,
            commit_hash: None,
            health: BookmarkHealth::Active,
            resolution_method: Some(ResolutionMethod::Exact),
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

        let handlebars = create_handlebars_engine();
        let context = BookmarkTemplateContext::from_bookmark(&bm, &[], Path::new("."), None);
        let output = handlebars.render_template(default_show_template(), &context).unwrap();
        assert!(!output.contains("## Context"), "empty breadcrumbs rendered a header:\n{output}");
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
