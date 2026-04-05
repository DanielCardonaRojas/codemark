pub mod handlers;
pub mod output;

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

/// A structural bookmarking system for code using tree-sitter queries.
///
/// Codemark stores tree-sitter queries that identify code by AST shape.
/// Bookmarks self-heal through layered resolution and integrate with
/// fzf, television, and other Unix tools.
#[derive(Debug, Parser)]
#[command(name = "codemark", version, about, long_about = None)]
pub struct Cli {
    /// Database location; repeatable for cross-repo queries
    #[arg(long, global = true)]
    pub db: Vec<PathBuf>,

    /// Output machine-readable JSON (shorthand for --format json)
    #[arg(long, global = true)]
    pub json: bool,

    /// Output format: table, line, json, or a custom template
    #[arg(long, global = true)]
    pub format: Option<String>,

    /// Enable debug-level logging to stderr
    #[arg(long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create a bookmark from a file and byte range
    Add(AddArgs),

    /// Create a bookmark by matching a code snippet from stdin against a file
    #[command(name = "add-from-snippet")]
    AddFromSnippet(AddFromSnippetArgs),

    /// Resolve a bookmark to its current file location
    Resolve(ResolveArgs),

    /// Display full details of a bookmark
    Show(ShowArgs),

    /// Remove bookmarks by ID
    Remove(RemoveArgs),

    /// Run resolution on bookmarks and update statuses
    Validate(ValidateArgs),

    /// Print a summary of bookmark health
    Status,

    /// List bookmarks with optional filters
    List(ListArgs),

    /// Show a syntax-highlighted preview of a bookmark's resolved code
    Preview(PreviewArgs),

    /// Full-text search across notes and context
    Search(SearchArgs),

    /// Manage named groups of bookmarks
    Collection(CollectionArgs),

    /// Show bookmarks affected by recent changes
    Diff(DiffArgs),

    /// Remove old archived bookmarks
    Gc(GcArgs),

    /// Export bookmarks to stdout
    Export(ExportArgs),

    /// Import bookmarks from a JSON file
    Import(ImportArgs),

    /// Generate shell completions
    Completions(CompletionsArgs),
}

// --- Subcommand argument structs ---

#[derive(Debug, clap::Args)]
pub struct AddArgs {
    /// Path to the file (relative or absolute)
    #[arg(long)]
    pub file: PathBuf,

    /// Line range (42 or 42:67) or byte range with b prefix (b1024:1280)
    #[arg(long, conflicts_with = "hunk")]
    pub range: Option<String>,

    /// Git diff hunk header (@@ -a,b +c,d @@) to derive line range
    #[arg(long, conflicts_with = "range")]
    pub hunk: Option<String>,

    /// Language identifier; auto-detected from file extension if omitted
    #[arg(long)]
    pub lang: Option<String>,

    /// Tag label; repeatable for multiple tags
    #[arg(long)]
    pub tag: Vec<String>,

    /// Semantic annotation
    #[arg(long)]
    pub note: Option<String>,

    /// Agent context at time of bookmarking
    #[arg(long)]
    pub context: Option<String>,

    /// Who created this bookmark (defaults to "user")
    #[arg(long, default_value = "user")]
    pub created_by: String,

    /// Preview what would be bookmarked without saving
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, clap::Args)]
pub struct AddFromSnippetArgs {
    /// Language identifier; auto-detected from file extension if omitted
    #[arg(long)]
    pub lang: Option<String>,

    /// File to search for the snippet in
    #[arg(long)]
    pub file: PathBuf,

    /// Tag label; repeatable
    #[arg(long)]
    pub tag: Vec<String>,

    /// Semantic annotation
    #[arg(long)]
    pub note: Option<String>,

    /// Agent context
    #[arg(long)]
    pub context: Option<String>,

    /// Who created this bookmark (defaults to "user")
    #[arg(long, default_value = "user")]
    pub created_by: String,

    /// Preview what would be bookmarked without saving
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, clap::Args)]
pub struct ResolveArgs {
    /// Bookmark ID (full UUID or unambiguous prefix)
    pub id: Option<String>,

    /// Filter by tag
    #[arg(long)]
    pub tag: Option<String>,

    /// Filter by status (default: active,drifted)
    #[arg(long)]
    pub status: Option<String>,

    /// Filter by file path
    #[arg(long)]
    pub file: Option<PathBuf>,

    /// Filter by language (swift, rust, typescript, python)
    #[arg(long)]
    pub lang: Option<String>,

    /// Filter by collection
    #[arg(long)]
    pub collection: Option<String>,
}

#[derive(Debug, clap::Args)]
pub struct ShowArgs {
    /// Bookmark ID (full UUID or unambiguous prefix)
    pub id: String,
}

#[derive(Debug, clap::Args)]
pub struct RemoveArgs {
    /// Bookmark IDs to remove (full UUID or unambiguous prefix)
    #[arg(required = true)]
    pub ids: Vec<String>,
}

#[derive(Debug, clap::Args)]
pub struct ValidateArgs {
    /// Validate only bookmarks for this file
    #[arg(long)]
    pub file: Option<PathBuf>,

    /// Filter by language (swift, rust, typescript, python)
    #[arg(long)]
    pub lang: Option<String>,

    /// Archive bookmarks stale beyond grace period
    #[arg(long)]
    pub auto_archive: bool,

    /// Days before stale bookmarks are archived
    #[arg(long, default_value = "7")]
    pub archive_after: u32,

    /// Filter by collection
    #[arg(long)]
    pub collection: Option<String>,
}

#[derive(Debug, clap::Args)]
pub struct ListArgs {
    /// Filter by tag
    #[arg(long)]
    pub tag: Option<String>,

    /// Filter by status (default: active,drifted)
    #[arg(long)]
    pub status: Option<String>,

    /// Filter by file path
    #[arg(long)]
    pub file: Option<PathBuf>,

    /// Filter by language (swift, rust, typescript, python)
    #[arg(long)]
    pub lang: Option<String>,

    /// Filter by author (user, agent, or any identifier)
    #[arg(long)]
    pub author: Option<String>,

    /// Maximum results to return
    #[arg(long)]
    pub limit: Option<usize>,

    /// Filter by collection
    #[arg(long)]
    pub collection: Option<String>,
}

#[derive(Debug, clap::Args)]
pub struct PreviewArgs {
    /// Bookmark ID or line-format string (extracts ID from first tab-separated field)
    pub id: String,

    /// Lines of context for plain text fallback (bat always shows full file)
    #[arg(long, default_value = "10")]
    pub context: u32,

    /// Disable syntax highlighting
    #[arg(long)]
    pub no_color: bool,

    /// Omit the metadata header
    #[arg(long)]
    pub no_metadata: bool,

    /// Show the stored tree-sitter query alongside the code
    #[arg(long)]
    pub show_query: bool,

    /// Preview the file at a specific git commit instead of the working copy
    #[arg(long, conflicts_with_all = ["at_creation", "resolution"])]
    pub at_commit: Option<String>,

    /// Preview the file as it was when the bookmark was created
    #[arg(long, conflicts_with_all = ["at_commit", "resolution"])]
    pub at_creation: bool,

    /// Preview the file as it was at a specific resolution (use resolution ID from `codemark show`)
    #[arg(long, conflicts_with_all = ["at_commit", "at_creation"])]
    pub resolution: Option<String>,
}

#[derive(Debug, clap::Args)]
pub struct SearchArgs {
    /// Search query (searches both notes and context)
    pub query: Option<String>,

    /// Search only in notes
    #[arg(long)]
    pub note: Option<String>,

    /// Search only in context
    #[arg(long)]
    pub context: Option<String>,

    /// Filter by language (swift, rust, typescript, python)
    #[arg(long)]
    pub lang: Option<String>,

    /// Filter by author (user, agent, or any identifier)
    #[arg(long)]
    pub author: Option<String>,

    /// Filter by collection
    #[arg(long)]
    pub collection: Option<String>,
}

#[derive(Debug, clap::Args)]
pub struct CollectionArgs {
    #[command(subcommand)]
    pub command: CollectionCommand,
}

#[derive(Debug, Subcommand)]
pub enum CollectionCommand {
    /// Create a new collection
    Create(CollectionCreateArgs),

    /// Delete a collection (bookmarks are kept)
    Delete(CollectionDeleteArgs),

    /// Add bookmarks to a collection
    Add(CollectionAddArgs),

    /// Remove bookmarks from a collection
    Remove(CollectionRemoveArgs),

    /// List all collections
    List(CollectionListArgs),

    /// Show bookmarks in a collection
    Show(CollectionShowArgs),

    /// Batch-resolve all bookmarks in a collection
    Resolve(CollectionResolveArgs),

    /// Reorder bookmarks in a collection
    Reorder(CollectionReorderArgs),
}

#[derive(Debug, clap::Args)]
pub struct CollectionCreateArgs {
    /// Collection name (lowercase, alphanumeric, hyphens)
    pub name: String,

    /// Human-readable description
    #[arg(long)]
    pub description: Option<String>,
}

#[derive(Debug, clap::Args)]
pub struct CollectionDeleteArgs {
    /// Collection name
    pub name: String,
}

#[derive(Debug, clap::Args)]
pub struct CollectionAddArgs {
    /// Collection name
    pub name: String,

    /// Bookmark IDs to add
    #[arg(required = true)]
    pub bookmark_ids: Vec<String>,

    /// Insert at this position (0-indexed); shifts existing items. Appends at end if omitted.
    #[arg(long)]
    pub at: Option<usize>,
}

#[derive(Debug, clap::Args)]
pub struct CollectionReorderArgs {
    /// Collection name
    pub name: String,

    /// Bookmark IDs in the desired order
    #[arg(required = true)]
    pub bookmark_ids: Vec<String>,
}

#[derive(Debug, clap::Args)]
pub struct CollectionRemoveArgs {
    /// Collection name
    pub name: String,

    /// Bookmark IDs to remove
    #[arg(required = true)]
    pub bookmark_ids: Vec<String>,
}

#[derive(Debug, clap::Args)]
pub struct CollectionListArgs {
    /// List collections containing this bookmark
    #[arg(long)]
    pub bookmark: Option<String>,
}

#[derive(Debug, clap::Args)]
pub struct CollectionShowArgs {
    /// Collection name
    pub name: String,
}

#[derive(Debug, clap::Args)]
pub struct CollectionResolveArgs {
    /// Collection name
    pub name: String,
}

#[derive(Debug, clap::Args)]
pub struct DiffArgs {
    /// Show bookmarks affected since this commit
    #[arg(long)]
    pub since: Option<String>,
}

#[derive(Debug, clap::Args)]
pub struct GcArgs {
    /// Remove archived bookmarks older than this duration (e.g., 30d, 2w, 6m)
    #[arg(long, default_value = "30d")]
    pub older_than: String,

    /// Show what would be removed without removing
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, clap::Args)]
pub struct ExportArgs {
    /// Export format (json or csv)
    #[arg(long = "export-format", default_value = "json")]
    pub export_format: ExportFormat,

    /// Filter by tag
    #[arg(long)]
    pub tag: Option<String>,

    /// Filter by status
    #[arg(long)]
    pub status: Option<String>,
}

#[derive(Debug, Clone, ValueEnum)]
pub enum ExportFormat {
    Json,
    Csv,
}

#[derive(Debug, clap::Args)]
pub struct ImportArgs {
    /// Path to JSON export file
    pub file: PathBuf,
}

#[derive(Debug, clap::Args)]
pub struct CompletionsArgs {
    /// Shell to generate completions for
    pub shell: clap_complete::Shell,
}
