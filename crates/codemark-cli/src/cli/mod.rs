pub mod handlers;
pub mod output;
pub mod templates;

use std::path::PathBuf;

use clap::{CommandFactory, Parser, Subcommand, ValueEnum};

/// Build the CLI command struct.
/// This is used by both the main binary and the man page generation in build.rs.
#[allow(dead_code)]
pub fn build_cli() -> clap::Command {
    Cli::command()
}

/// A structural bookmarking system for code using tree-sitter queries.
///
/// Codemark stores tree-sitter queries that identify code by AST shape.
/// Bookmarks self-heal through layered resolution and integrate with
/// fzf, television, and other Unix tools.
#[derive(Debug, Parser)]
#[command(name = "codemark", version, about, long_about = None)]
pub struct Cli {
    /// Database location; repeatable for cross-repo queries
    #[arg(
        long,
        global = true,
        env = "CODEMARK_DB",
        value_delimiter = if cfg!(windows) { ';' } else { ':' }
    )]
    pub db: Vec<PathBuf>,

    /// Repository reference (owner/name) for identity-based discovery; repeatable
    #[arg(long, global = true)]
    pub repo: Vec<String>,

    /// Output format: json (default), table, line, markdown (or a custom template)
    #[arg(long, global = true, env = "CODEMARK_FORMAT")]
    pub format: Option<String>,

    /// Enable debug-level logging to stderr
    #[arg(long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Initialize a new codemark repository
    Init,

    /// Create a bookmark from a file and byte range
    Add(AddArgs),

    /// Create a bookmark by matching a code snippet from stdin against a file
    #[command(name = "add-from-snippet", hide = true)]
    AddFromSnippet(AddFromSnippetArgs),

    /// Create a bookmark from a file and a raw tree-sitter query
    #[command(name = "add-from-query", hide = true)]
    AddFromQuery(AddFromQueryArgs),

    /// Resolve a bookmark to its current file location
    #[command(name = "resolve", hide = true)]
    Resolve(ResolveArgs),

    /// Display bookmark details and code preview
    Show(ShowArgs),

    /// Show the current location of a bookmark (file, line, byte range)
    #[command(name = "preview", hide = true)]
    Preview(PreviewArgs),

    /// Remove bookmarks by ID
    Remove(RemoveArgs),

    /// List bookmarks with optional filters
    List(ListArgs),

    /// Full-text search across notes and context
    Search(SearchArgs),

    /// Update metadata for an existing bookmark
    Edit(EditArgs),

    /// Add notes, context, or tags to an existing bookmark
    #[command(name = "annotate", hide = true)]
    Annotate(AnnotateArgs),

    /// Open a bookmarked file in your editor
    Open(OpenArgs),

    /// Manage collections and remote sync (tours)
    Tour(TourArgs),

    /// Publish a local collection to a Codetours server
    #[command(name = "publish", hide = true)]
    Publish(PublishArgs),

    /// Manage named groups of bookmarks
    #[command(name = "collection", hide = true)]
    Collection(CollectionArgs),

    /// Database health and maintenance
    Health(HealthArgs),

    /// Heal bookmarks by resolving and updating their status
    #[command(name = "heal", hide = true)]
    Heal(HealArgs),

    /// Print a summary of bookmark health
    #[command(name = "status", hide = true)]
    Status,

    /// Import, export, and indexing
    Data(DataArgs),

    /// Rebuild semantic search embeddings for all bookmarks
    #[command(name = "reindex", hide = true)]
    Reindex(ReindexArgs),

    /// Export bookmarks to stdout
    #[command(name = "export", hide = true)]
    Export(ExportArgs),

    /// Import bookmarks from a JSON file
    #[command(name = "import", hide = true)]
    Import(ImportArgs),

    /// Remove old archived bookmarks
    #[command(name = "gc", hide = true)]
    Gc(GcArgs),

    /// Show bookmarks affected by recent changes
    #[command(name = "diff", hide = true)]
    Diff(DiffArgs),

    /// Generate shell completions
    Completions(CompletionsArgs),

    /// Pull a tour from a Codetours server
    #[command(name = "pull", hide = true)]
    Pull(PullArgs),

    /// Manage authentication and login
    Auth(AuthArgs),

    /// Manage repository registry and server configuration
    Repo(RepoArgs),
}

// --- Subcommand argument structs ---

#[derive(Debug, clap::Args)]
pub struct AddArgs {
    /// Path to the file (relative or absolute) — required unless using --snippet
    #[arg(long)]
    pub file: Option<PathBuf>,

    /// Line range (42, 42-67, 10:5-12:20) or byte range with b prefix (b1024-1280)
    #[arg(long, conflicts_with = "snippet", conflicts_with = "query", conflicts_with = "hunk")]
    pub range: Option<String>,

    /// Git diff hunk header (@@ -a,b +c,d @@) to derive line range
    #[arg(long, conflicts_with = "snippet", conflicts_with = "query", conflicts_with = "range")]
    pub hunk: Option<String>,

    /// Language identifier; auto-detected from file extension if omitted
    #[arg(long)]
    pub lang: Option<String>,

    /// Tag label; repeatable for multiple tags
    #[arg(long)]
    pub tag: Vec<String>,

    /// Semantic annotation; repeatable for multiple notes
    #[arg(long)]
    pub note: Vec<String>,

    /// Agent context at time of bookmarking
    #[arg(long)]
    pub context: Option<String>,

    /// Who created this bookmark (defaults to "user")
    #[arg(long, default_value = "user")]
    pub created_by: String,

    /// Preview what would be bookmarked without saving
    #[arg(long)]
    pub dry_run: bool,

    /// Add bookmark to this collection (auto-creates if it doesn't exist)
    #[arg(long)]
    pub collection: Option<String>,

    /// Match code from stdin against the file (reads from stdin)
    #[arg(long, conflicts_with = "range", conflicts_with = "hunk", conflicts_with = "query")]
    pub snippet: bool,

    /// Use a raw tree-sitter query to create the bookmark
    #[arg(long, conflicts_with = "range", conflicts_with = "hunk", conflicts_with = "snippet")]
    pub query: Option<String>,
}

/// Original AddArgs struct for backward compatibility with existing handlers.
/// This is the non-optional version used by bookmark::handle_add.
#[derive(Debug, clap::Args)]
pub struct AddArgsOriginal {
    /// Path to the file (relative or absolute)
    #[arg(long)]
    pub file: PathBuf,

    /// Line range (42, 42-67, 10:5-12:20) or byte range with b prefix (b1024-1280)
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

    /// Semantic annotation; repeatable for multiple notes
    #[arg(long)]
    pub note: Vec<String>,

    /// Agent context at time of bookmarking
    #[arg(long)]
    pub context: Option<String>,

    /// Who created this bookmark (defaults to "user")
    #[arg(long, default_value = "user")]
    pub created_by: String,

    /// Preview what would be bookmarked without saving
    #[arg(long)]
    pub dry_run: bool,

    /// Add bookmark to this collection (auto-creates if it doesn't exist)
    #[arg(long)]
    pub collection: Option<String>,
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

    /// Semantic annotation; repeatable for multiple notes
    #[arg(long)]
    pub note: Vec<String>,

    /// Agent context
    #[arg(long)]
    pub context: Option<String>,

    /// Who created this bookmark (defaults to "user")
    #[arg(long, default_value = "user")]
    pub created_by: String,

    /// Preview what would be bookmarked without saving
    #[arg(long)]
    pub dry_run: bool,

    /// Add bookmark to this collection (auto-creates if it doesn't exist)
    #[arg(long)]
    pub collection: Option<String>,
}

#[derive(Debug, clap::Args)]
pub struct AddFromQueryArgs {
    /// Path to the file (relative or absolute)
    #[arg(long)]
    pub file: PathBuf,

    /// Raw tree-sitter S-expression query
    #[arg(long)]
    pub query: String,

    /// Language identifier; auto-detected from file extension if omitted
    #[arg(long)]
    pub lang: Option<String>,

    /// Tag label; repeatable for multiple tags
    #[arg(long)]
    pub tag: Vec<String>,

    /// Semantic annotation; repeatable for multiple notes
    #[arg(long)]
    pub note: Vec<String>,

    /// Agent context at time of bookmarking
    #[arg(long)]
    pub context: Option<String>,

    /// Who created this bookmark (defaults to "user")
    #[arg(long, default_value = "user")]
    pub created_by: String,

    /// Preview what would be bookmarked without saving
    #[arg(long)]
    pub dry_run: bool,

    /// Add bookmark to this collection (auto-creates if it doesn't exist)
    #[arg(long)]
    pub collection: Option<String>,
}

#[derive(Debug, clap::Args)]
pub struct ResolveArgs {
    /// Bookmark ID (full UUID or unambiguous prefix)
    pub id: Option<String>,

    /// Filter by tag
    #[arg(long)]
    pub tag: Option<String>,

    /// Filter by health (default: active,drifted)
    #[arg(long)]
    pub health: Option<String>,

    /// Deprecated: use --health
    #[arg(long, hide = true)]
    pub status: Option<String>,

    /// Filter by file path
    #[arg(long)]
    pub file: Option<PathBuf>,

    /// Filter by language (swift, rust, typescript, python)
    #[arg(long)]
    pub lang: Option<String>,

    /// Filter by collection
    #[arg(long, env = "CODEMARK_COLLECTION_FILTER")]
    pub collection: Option<String>,

    /// Preview what would be resolved without storing anything
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, clap::Args)]
pub struct ShowArgs {
    /// Bookmark ID (full UUID or unambiguous prefix)
    pub id: String,

    /// Show only the file location (was 'resolve' behavior)
    #[arg(long)]
    pub location: bool,

    /// Hide code preview (show metadata only) — not yet implemented
    #[arg(long, hide = true)]
    pub no_preview: bool,
}

#[derive(Debug, clap::Args)]
pub struct RemoveArgs {
    /// Bookmark IDs to remove (full UUID or unambiguous prefix)
    #[arg(required = true)]
    pub ids: Vec<String>,
}

#[derive(Debug, clap::Args)]
pub struct HealArgs {
    /// Heal only bookmarks for this file
    #[arg(long)]
    pub file: Option<PathBuf>,

    /// Filter by language (swift, rust, typescript, python)
    #[arg(long)]
    pub lang: Option<String>,

    /// Filter by health (active,drifted,stale,archived)
    #[arg(long)]
    pub health: Option<String>,

    /// Deprecated: use --health
    #[arg(long, hide = true)]
    pub status: Option<String>,

    /// Archive bookmarks stale beyond grace period
    #[arg(long)]
    pub auto_archive: bool,

    /// Days before stale bookmarks are archived
    #[arg(long, default_value = "7")]
    pub archive_after: u32,

    /// Filter by collection
    #[arg(long, env = "CODEMARK_COLLECTION_FILTER")]
    pub collection: Option<String>,

    /// Skip recording resolution history (only update status)
    #[arg(long)]
    pub validate_only: bool,

    /// Bypass commit ancestry check (allow healing backward in history)
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, clap::Args)]
pub struct ListArgs {
    /// Filter by tag
    #[arg(long)]
    pub tag: Option<String>,

    /// Filter by health (default: active,drifted)
    #[arg(long)]
    pub health: Option<String>,

    /// Deprecated: use --health
    #[arg(long, hide = true)]
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
    #[arg(long, env = "CODEMARK_COLLECTION_FILTER")]
    pub collection: Option<String>,

    /// Filter by collection ID
    #[arg(long, env = "CODEMARK_COLLECTION_ID_FILTER")]
    pub collection_id: Option<String>,

    /// Additional databases to query (can be specified multiple times)
    #[arg(long, value_name = "PATH")]
    pub add_db: Vec<String>,

    /// Repository reference (owner/name) for identity-based discovery; repeatable
    #[arg(long, value_name = "OWNER/NAME")]
    pub repo: Vec<String>,

    /// Filter by database owner email
    #[arg(long)]
    pub user_email: Option<String>,

    /// Filter by repository owner
    #[arg(long)]
    pub repo_owner: Option<String>,

    /// Custom line format template (placeholders: {ID}, {FILE}, {FILENAME}, {LINE}, {OFFSET}, {STATUS}, {TAGS}, {NOTE}, {CONTEXT}, {QUERY})
    #[arg(long)]
    pub line_format: Option<String>,
}

#[derive(Debug, clap::Args)]
pub struct PreviewArgs {
    /// Bookmark ID (full UUID or unambiguous prefix)
    pub id: String,

    /// Preview the file as it was when the bookmark was created
    #[arg(long)]
    at_creation: bool,

    /// Preview the file at a specific git commit
    #[arg(long)]
    at_commit: Option<String>,

    /// Use a specific resolution (by ID) instead of the latest
    #[arg(long)]
    resolution_id: Option<String>,

    /// Output raw file content to stdout instead of JSON metadata
    #[arg(long)]
    pub raw: bool,

    /// Include ASCII breadcrumbs in the preview
    #[arg(long)]
    pub breadcrumbs: bool,
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

    /// Use semantic search (vector embeddings)
    #[arg(long)]
    pub semantic: bool,

    /// Maximum results to return (default: 20)
    #[arg(long, default_value = "20")]
    pub limit: usize,

    /// Filter by language (swift, rust, typescript, python)
    #[arg(long)]
    pub lang: Option<String>,

    /// Filter by health (active,drifted,stale,archived)
    #[arg(long)]
    pub health: Option<String>,

    /// Deprecated: use --health
    #[arg(long, hide = true)]
    pub status: Option<String>,

    /// Filter by author (user, agent, or any identifier)
    #[arg(long)]
    pub author: Option<String>,

    /// Filter by collection
    #[arg(long, env = "CODEMARK_COLLECTION_FILTER")]
    pub collection: Option<String>,

    /// Additional databases to query (can be specified multiple times)
    #[arg(long, value_name = "PATH")]
    pub add_db: Vec<String>,

    /// Repository reference (owner/name) for identity-based discovery; repeatable
    #[arg(long, value_name = "OWNER/NAME")]
    pub repo: Vec<String>,

    /// Filter by database owner email
    #[arg(long)]
    pub user_email: Option<String>,

    /// Filter by repository owner
    #[arg(long)]
    pub repo_owner: Option<String>,

    /// Custom line format template (placeholders: {ID}, {FILE}, {FILENAME}, {LINE}, {OFFSET}, {STATUS}, {TAGS}, {NOTE}, {CONTEXT}, {QUERY})
    #[arg(long)]
    pub line_format: Option<String>,
}

#[derive(Debug, clap::Args)]
pub struct ReindexArgs {
    /// Only reindex bookmarks for this language
    #[arg(long)]
    pub lang: Option<String>,

    /// Only reindex this collection
    #[arg(long, env = "CODEMARK_COLLECTION_FILTER")]
    pub collection: Option<String>,

    /// Show progress while reindexing
    #[arg(long, short = 'v')]
    pub verbose: bool,
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

    /// Delete a collection (bookmarks are kept by default)
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

    /// Manage tags for a collection
    Tag(CollectionTagArgs),

    /// Manage links for a collection
    Link(CollectionLinkArgs),
}

#[derive(Debug, clap::Args)]
pub struct CollectionCreateArgs {
    /// Collection name (lowercase, alphanumeric, hyphens)
    pub name: String,

    /// Human-readable description
    #[arg(long)]
    pub description: Option<String>,

    /// Tag labels to add; repeatable
    #[arg(long)]
    pub tag: Vec<String>,

    /// Links to add (format: "url,label"); repeatable
    #[arg(long)]
    pub link: Vec<String>,

    /// Notes to add; repeatable
    #[arg(long)]
    pub note: Vec<String>,
}

#[derive(Debug, clap::Args)]
pub struct CollectionDeleteArgs {
    /// Collection name
    pub name: String,

    /// Delete all bookmarks in the collection as well
    #[arg(long)]
    pub with_bookmarks: bool,
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

    /// Tag labels to add to the collection; repeatable
    #[arg(long)]
    pub tag: Vec<String>,

    /// Links to add to the collection (format: "url,label"); repeatable
    #[arg(long)]
    pub link: Vec<String>,

    /// Notes to add to the collection; repeatable
    #[arg(long)]
    pub note: Vec<String>,
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

    /// Custom line format template (placeholders: {ID}, {NAME}, {COUNT}, {DESCRIPTION}, {CREATED})
    #[arg(long)]
    pub line_format: Option<String>,
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
pub struct CollectionTagArgs {
    #[command(subcommand)]
    pub command: CollectionTagCommand,
}

#[derive(Debug, Subcommand)]
pub enum CollectionTagCommand {
    /// Add tags to a collection
    Add(CollectionTagAddArgs),

    /// Remove tags from a collection
    Rm(CollectionTagRmArgs),

    /// List tags for a collection
    List(CollectionTagListArgs),
}

#[derive(Debug, clap::Args)]
pub struct CollectionTagAddArgs {
    /// Collection name
    pub name: String,

    /// Tag labels to add
    #[arg(required = true)]
    pub tags: Vec<String>,
}

#[derive(Debug, clap::Args)]
pub struct CollectionTagRmArgs {
    /// Collection name
    pub name: String,

    /// Tag labels to remove
    #[arg(required = true)]
    pub tags: Vec<String>,
}

#[derive(Debug, clap::Args)]
pub struct CollectionTagListArgs {
    /// Collection name
    pub name: String,

    /// Include auto-populated tags (default: hide)
    #[arg(long)]
    pub include_auto: bool,
}

#[derive(Debug, clap::Args)]
pub struct CollectionLinkArgs {
    #[command(subcommand)]
    pub command: CollectionLinkCommand,
}

#[derive(Debug, Subcommand)]
pub enum CollectionLinkCommand {
    /// Add a link to a collection
    Add(CollectionLinkAddArgs),

    /// Remove a link from a collection
    Rm(CollectionLinkRmArgs),

    /// List links for a collection
    List(CollectionLinkListArgs),

    /// Reorder links for a collection
    Reorder(CollectionLinkReorderArgs),
}

#[derive(Debug, clap::Args)]
pub struct CollectionLinkAddArgs {
    /// Collection name
    pub name: String,

    /// Link URL
    #[arg(long)]
    pub url: String,

    /// Link label
    #[arg(long)]
    pub label: String,

    /// Link kind (pr, issue, doc, discussion, dashboard, repo, tour, other)
    #[arg(long, default_value = "other")]
    pub kind: String,

    /// Position to insert at (0-indexed)
    #[arg(long)]
    pub position: Option<i32>,
}

#[derive(Debug, clap::Args)]
pub struct CollectionLinkRmArgs {
    /// Collection name
    pub name: String,

    /// Link ID to remove
    #[arg(long)]
    pub id: String,
}

#[derive(Debug, clap::Args)]
pub struct CollectionLinkListArgs {
    /// Collection name
    pub name: String,
}

#[derive(Debug, clap::Args)]
pub struct CollectionLinkReorderArgs {
    /// Collection name
    pub name: String,

    /// Ordered list of link IDs
    #[arg(required = true)]
    pub ids: Vec<String>,
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

    /// Filter by health
    #[arg(long)]
    pub health: Option<String>,

    /// Deprecated: use --health
    #[arg(long, hide = true)]
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

#[derive(Debug, clap::Args)]
pub struct EditArgs {
    /// Bookmark ID (full UUID or unambiguous prefix)
    pub id: String,

    /// Semantic annotation to add; repeatable for multiple notes
    #[arg(long)]
    pub note: Vec<String>,

    /// Agent context to add
    #[arg(long)]
    pub context: Option<String>,

    /// Tag label to add; repeatable
    #[arg(long)]
    pub tag: Vec<String>,
}

#[derive(Debug, clap::Args)]
pub struct AnnotateArgs {
    /// Bookmark ID (full UUID or unambiguous prefix)
    pub id: String,

    /// Semantic annotation to add; repeatable for multiple notes
    #[arg(long)]
    pub note: Vec<String>,

    /// Agent context to add
    #[arg(long)]
    pub context: Option<String>,

    /// Tag label to add; repeatable for multiple tags
    #[arg(long)]
    pub tag: Vec<String>,

    /// Who is adding this annotation (defaults to "user")
    #[arg(long, default_value = "user")]
    pub added_by: String,

    /// Source of this annotation (defaults to "cli")
    #[arg(long, default_value = "cli")]
    pub source: String,
}

#[derive(Debug, clap::Args)]
pub struct OpenArgs {
    /// Bookmark ID (full UUID or unambiguous prefix)
    pub id: String,
}

#[derive(Debug, clap::Args)]
pub struct PublishArgs {
    /// Local collection name to publish
    pub collection: String,

    /// Server URL or name from config
    #[arg(long)]
    pub server: Option<String>,

    /// API token (overrides config)
    #[arg(long)]
    pub token: Option<String>,

    /// Visibility of the tour (public or private)
    #[arg(long, default_value = "private")]
    pub visibility: String,

    /// Title of the tour (defaults to collection name)
    #[arg(long)]
    pub title: Option<String>,

    /// Description of the tour
    #[arg(long)]
    pub description: Option<String>,

    /// Build the pack and print its path, but do not upload
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, clap::Args)]
pub struct PullArgs {
    /// Tour URL or ID
    pub tour: String,

    /// Server URL or name (required if tour is a bare ID)
    #[arg(long)]
    pub server: Option<String>,

    /// API token
    #[arg(long)]
    pub token: Option<String>,

    /// Save the tour locally as a collection. If no name is provided, uses the tour's title.
    #[arg(long, num_args = 0..=1, default_missing_value = "")]
    pub save: Option<String>,
}

#[derive(Debug, clap::Args)]
pub struct TourArgs {
    #[command(subcommand)]
    pub command: TourCommand,
}

#[derive(Debug, Subcommand)]
pub enum TourCommand {
    /// List local collections and remote tours
    List(TourListArgs),

    /// Create a new local collection
    Create(CollectionCreateArgs),

    /// Add bookmarks to a collection
    Add(CollectionAddArgs),

    /// Remove bookmarks from a collection
    Remove(CollectionRemoveArgs),

    /// Delete a collection (bookmarks are kept by default)
    Delete(CollectionDeleteArgs),

    /// Show bookmarks in a collection
    Show(CollectionShowArgs),

    /// Publish a collection to Codetours
    Push(PublishArgs),

    /// Pull a tour from Codetours
    Pull(PullArgs),

    /// Show changes since last publish
    Diff(DiffArgs),

    /// Batch-resolve all bookmarks in a collection
    Resolve(CollectionResolveArgs),

    /// Reorder bookmarks in a collection
    Reorder(CollectionReorderArgs),

    /// Manage tags for a collection
    Tag(CollectionTagArgs),

    /// Manage links for a collection
    Link(CollectionLinkArgs),

    /// Add notes, tags, or links to a collection
    Annotate(CollectionAnnotateArgs),
}

#[derive(Debug, clap::Args)]
pub struct TourListArgs {
    /// Server URL or name (for remote tours)
    #[arg(long)]
    pub server: Option<String>,

    /// Filter by repository URL
    #[arg(long)]
    pub repo: Option<String>,

    /// Maximum results to return
    #[arg(long, default_value = "50")]
    pub limit: usize,

    /// Offset for pagination
    #[arg(long, default_value = "0")]
    pub offset: usize,

    /// Custom line format template (placeholders: {ID}, {NAME}, {COUNT}, {DESCRIPTION}, {CREATED})
    #[arg(long)]
    pub line_format: Option<String>,
}

#[derive(Debug, clap::Args)]
pub struct CollectionAnnotateArgs {
    /// Collection name
    pub name: String,

    /// Tag labels to add
    #[arg(long)]
    pub tag: Vec<String>,

    /// Links to add (format: "url,label")
    #[arg(long)]
    pub link: Vec<String>,

    /// Notes to add
    #[arg(long)]
    pub note: Vec<String>,
}

// === Health Namespace ===

#[derive(Debug, clap::Args)]
pub struct HealthArgs {
    #[command(subcommand)]
    pub command: HealthCommand,
}

#[derive(Debug, Subcommand)]
pub enum HealthCommand {
    /// Print health summary
    Status,

    /// Run resolution/validation pass
    Check(HealArgs),

    /// Remove old archived bookmarks
    Gc(GcArgs),
}

// === Data Namespace ===

#[derive(Debug, clap::Args)]
pub struct DataArgs {
    #[command(subcommand)]
    pub command: DataCommand,
}

#[derive(Debug, Subcommand)]
pub enum DataCommand {
    /// Export bookmarks to JSON/CSV
    Export(ExportArgs),

    /// Import bookmarks from JSON
    Import(ImportArgs),

    /// Rebuild semantic search embeddings
    Reindex(ReindexArgs),
}

/// Arguments for the `codemark repo` subcommand.
#[derive(Debug, clap::Args)]
pub struct RepoArgs {
    #[command(subcommand)]
    pub command: RepoCommand,
}

#[derive(Debug, Subcommand)]
pub enum RepoCommand {
    /// List all known repositories in the global registry
    List,

    /// Show details for a repository (current or by owner/name)
    #[command(name = "show")]
    ShowRepo(RepoShowArgs),

    /// Set the server URL for a repository
    #[command(name = "set-server")]
    SetServer(RepoSetServerArgs),
}

#[derive(Debug, clap::Args)]
pub struct RepoShowArgs {
    /// Repository reference (owner/name) - defaults to current repo
    pub repo: Option<String>,
}

#[derive(Debug, clap::Args)]
pub struct RepoSetServerArgs {
    /// Repository reference (owner/name) - defaults to current repo
    pub repo: Option<String>,

    /// Server URL (e.g., https://codemark.example.com)
    pub server: String,
}

/// Arguments for the `codemark auth` subcommand.
#[derive(Debug, clap::Args)]
pub struct AuthArgs {
    #[command(subcommand)]
    pub command: AuthCommand,
}

#[derive(Debug, Subcommand)]
pub enum AuthCommand {
    /// Login to a Codetours server
    Login(AuthLoginArgs),

    /// Logout from a server
    Logout(AuthLogoutArgs),

    /// List authenticated servers
    List,
}

#[derive(Debug, clap::Args)]
pub struct AuthLoginArgs {
    /// Server URL (e.g., https://codemark.example.com)
    pub server: String,

    /// Auth token (if you have one already)
    #[arg(long)]
    pub token: Option<String>,

    /// Open browser for OAuth flow (default: true)
    #[arg(long, default_value = "true")]
    pub browser: bool,

    /// Use device flow for login (default: false)
    #[arg(long, default_value = "false")]
    pub device: bool,
}

#[derive(Debug, clap::Args)]
pub struct AuthLogoutArgs {
    /// Server URL (e.g., https://codemark.example.com)
    pub server: String,
}
