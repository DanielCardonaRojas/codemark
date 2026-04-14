//! Handlebars template support for markdown output.
//!
//! Templates are stored in `.codemark/templates/` directory and can be customized by users.
//! Default templates are embedded in the binary.

use std::path::PathBuf;

use handlebars::{Handlebars, HelperDef, HelperResult, Output, RenderContext, RenderErrorReason};
use serde::Serialize;

use crate::engine::bookmark::{Annotation, Bookmark, Resolution};

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
    /// Tags
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Annotations
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub annotations: Vec<AnnotationTemplateContext>,
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

/// Template context for a resolution.
#[derive(Debug, Serialize)]
pub struct ResolutionTemplateContext {
    /// When resolution occurred
    pub resolved_at: String,
    /// Resolution method
    pub method: String,
    /// Resolved file path (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    /// Line range (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_range: Option<String>,
    /// Number of matches (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub match_count: Option<i32>,
    /// Resolution commit (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit_hash: Option<String>,
    /// Short commit hash (first 8 chars, optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub short_commit: Option<String>,
}

impl BookmarkTemplateContext {
    /// Create a template context from a bookmark and its resolutions.
    pub fn from_bookmark(bm: &Bookmark, resolutions: &[Resolution]) -> Self {
        let short_id = crate::cli::output::short_id(&bm.id).to_string();
        let file_name = bm
            .file_path
            .rsplit('/')
            .next()
            .or_else(|| bm.file_path.rsplit('\\').next())
            .unwrap_or(&bm.file_path)
            .to_string();

        let short_commit = bm.commit_hash.as_ref().map(|c| short_id_value(c));

        BookmarkTemplateContext {
            short_id,
            id: bm.id.clone(),
            file_path: bm.file_path.clone(),
            file_name,
            language: bm.language.clone(),
            status: bm.status.to_string(),
            query: bm.query.clone(),
            created_at: bm.created_at.clone(),
            created_by: bm.created_by.clone(),
            commit_hash: bm.commit_hash.clone(),
            short_commit,
            last_resolved_at: bm.last_resolved_at.clone(),
            resolution_method: bm.resolution_method.map(|m| m.to_string()),
            stale_since: bm.stale_since.clone(),
            tags: bm.tags.clone(),
            annotations: bm
                .annotations
                .iter()
                .map(AnnotationTemplateContext::from_annotation)
                .collect(),
            resolutions: resolutions
                .iter()
                .map(ResolutionTemplateContext::from_resolution)
                .collect(),
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
    fn from_resolution(r: &Resolution) -> Self {
        let short_commit = r.commit_hash.as_ref().map(|c| short_id_value(c));

        ResolutionTemplateContext {
            resolved_at: r.resolved_at.clone(),
            method: r.method.to_string(),
            file_path: r.file_path.clone(),
            line_range: r.line_range.clone(),
            match_count: r.match_count,
            commit_hash: r.commit_hash.clone(),
            short_commit,
        }
    }
}

/// Truncate a string to first 8 characters.
fn short_id_value(s: &str) -> String {
    s.chars().take(8).collect()
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

/// Get the templates directory.
pub fn templates_dir() -> Option<PathBuf> {
    let project_dir = directories::ProjectDirs::from("", "codemark", "codemark")?;
    Some(project_dir.config_dir().join("templates"))
}

/// Get the default markdown template for the `show` command.
pub fn default_show_template() -> &'static str {
    r#"# Bookmark: {{short_id}}

## Metadata
| Property | Value |
|----------|-------|
| **File** | {{file_path}} |
| **Language** | {{language}} |
| **Status** | {{status}} |
| **Created** | {{created_at}} |
{{#if created_by}}| **Author** | {{escape_markdown created_by}} |{{/if}}
{{#if last_resolved_at}}| **Last Resolved** | {{last_resolved_at}} |{{/if}}
{{#if resolution_method}}| **Resolution Method** | {{resolution_method}} |{{/if}}
{{#if commit_hash}}| **Commit** | `{{short_commit}}` |{{/if}}
{{#if stale_since}}| **Stale Since** | {{stale_since}} |{{/if}}

## Tree-sitter Query
```scheme
{{query}}
```

{{#if tags}}
## Tags
{{#each tags}}
- `{{escape_markdown this}}`
{{/each}}
{{/if}}

{{#if annotations}}
## Annotations
{{#each annotations}}
### {{added_by}}
*{{source}}* added: {{added_at}}

{{#if notes}}{{escape_markdown notes}}{{/if}}

{{#if context}}
```
{{escape_markdown context}}
```
{{/if}}
{{/each}}
{{/if}}

{{#if resolutions}}
## Resolution History
| Time | Method | File | Lines | Matches | Commit |
|------|--------|------|-------|---------|--------|
{{#each resolutions}}
| {{resolved_at}} | {{method}} | {{file_path}} | {{line_range}} | {{match_count}} | {{#if commit_hash}}`{{short_commit}}`{{else}}-{{/if}} |
{{/each}}
{{/if}}
"#
}

/// Create a Handlebars instance with all helpers registered.
pub fn create_handlebars_engine() -> Handlebars<'static> {
    let mut handlebars = Handlebars::new();

    // Don't escape HTML - we're generating markdown
    handlebars.register_escape_fn(|s| s.to_string());

    // Register custom helpers
    handlebars.register_helper("escape_markdown", Box::new(EscapeMarkdownHelper));
    handlebars.register_helper("truncate", Box::new(TruncateHelper));

    handlebars
}

/// Ensure the default template file exists in the user's data directory.
/// Creates the template file if it doesn't already exist.
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

    let template_path = templates_dir.join("codemark_show.md");

    // Only write if file doesn't exist (don't overwrite user customizations)
    if !template_path.exists() {
        if let Err(e) = std::fs::write(&template_path, default_show_template()) {
            eprintln!(
                "Warning: Failed to write default template to {}: {}",
                template_path.display(),
                e
            );
        }
    }
}

/// Load a template, falling back to the default if not found.
pub fn load_show_template() -> String {
    let templates_dir = match templates_dir() {
        Some(dir) => dir,
        None => return default_show_template().to_string(),
    };

    let template_path = templates_dir.join("codemark_show.md");

    match std::fs::read_to_string(&template_path) {
        Ok(content) => content,
        Err(_) => default_show_template().to_string(),
    }
}

/// Render a bookmark with its resolutions using the show template.
pub fn render_show_template(
    bm: &Bookmark,
    resolutions: &[Resolution],
) -> Result<String, handlebars::RenderError> {
    let handlebars = create_handlebars_engine();
    let template = load_show_template();
    let context = BookmarkTemplateContext::from_bookmark(bm, resolutions);
    handlebars.render_template(&template, &context)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::bookmark::{BookmarkStatus, ResolutionMethod};

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
            status: BookmarkStatus::Active,
            resolution_method: Some(ResolutionMethod::Exact),
            last_resolved_at: Some("2024-01-01T00:00:00Z".to_string()),
            stale_since: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            created_by: Some("user".to_string()),
            tags: vec!["tag1".to_string(), "tag2".to_string()],
            annotations: vec![],
        };

        let resolutions = vec![Resolution {
            id: "res1".to_string(),
            bookmark_id: bm.id.clone(),
            resolved_at: "2024-01-01T00:00:00Z".to_string(),
            commit_hash: Some("commit1234567890".to_string()),
            method: ResolutionMethod::Exact,
            match_count: Some(1),
            file_path: Some("/path/to/file.rs".to_string()),
            byte_range: None,
            line_range: Some("10:20".to_string()),
            content_hash: None,
        }];

        let result = render_show_template(&bm, &resolutions);
        assert!(result.is_ok(), "Template rendering failed: {:?}", result.err());
        let output = result.unwrap();
        assert!(output.contains("# Bookmark: abcdef12"));
        assert!(output.contains("| **File** | /path/to/file.rs |"));
        assert!(output.contains("| **Language** | rust |"));
        assert!(output.contains("| **Status** | active |"));
        assert!(output.contains("## Tags"));
        assert!(output.contains("- `tag1`"));
        assert!(output.contains("- `tag2`"));
    }
}
