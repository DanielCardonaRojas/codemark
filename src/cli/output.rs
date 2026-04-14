use std::io::{self, Write};

use comfy_table::{Cell, Table};
use is_terminal::IsTerminal;
use serde::Serialize;

use crate::engine::bookmark::{Bookmark, Resolution};

/// Helper function to get the first annotation's notes from a bookmark.
fn get_first_note(bm: &Bookmark) -> &str {
    bm.annotations.first().and_then(|a| a.notes.as_deref()).unwrap_or("")
}

/// Resolved output mode for a command invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputMode {
    Table,
    Line,
    Tv, // Television format: includes line numbers for preview.offset
    Json,
    Markdown,
    Custom(String),
}

impl OutputMode {
    /// Determine the output mode from CLI flags and TTY detection.
    ///
    /// Priority: --json > --format > auto-detect (TTY=table, pipe=line).
    pub fn resolve(json_flag: bool, format_flag: Option<&str>) -> Self {
        Self::resolve_with_default(json_flag, format_flag, false)
    }

    /// Determine the output mode with an option to default to JSON.
    ///
    /// When default_json is true and no flags are provided, JSON is used
    /// instead of TTY-based auto-detection.
    ///
    /// Priority: --json > --format > default_json (if true) > auto-detect (TTY=table, pipe=line).
    pub fn resolve_with_default(
        json_flag: bool,
        format_flag: Option<&str>,
        default_json: bool,
    ) -> Self {
        if json_flag {
            return OutputMode::Json;
        }
        match format_flag {
            Some("table") => OutputMode::Table,
            Some("line") => OutputMode::Line,
            Some("tv") => OutputMode::Tv,
            Some("json") => OutputMode::Json,
            Some("markdown") => OutputMode::Markdown,
            Some(template) => OutputMode::Custom(template.to_string()),
            None => {
                if default_json {
                    OutputMode::Json
                } else if io::stdout().is_terminal() {
                    OutputMode::Table
                } else {
                    OutputMode::Line
                }
            }
        }
    }
}

/// Standard JSON envelope for all command output.
#[derive(Debug, Serialize)]
pub struct JsonEnvelope<T: Serialize> {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Write a JSON success response to stdout.
pub fn write_json_success<T: Serialize>(data: &T) -> io::Result<()> {
    let envelope = JsonEnvelope { success: true, data: Some(data), error: None };
    let mut stdout = io::stdout().lock();
    serde_json::to_writer_pretty(&mut stdout, &envelope)?;
    writeln!(stdout)?;
    Ok(())
}

/// Write raw JSON to stdout (not wrapped in envelope).
pub fn write_json<T: Serialize>(data: &T) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    serde_json::to_writer_pretty(&mut stdout, data)?;
    writeln!(stdout)?;
    Ok(())
}

/// Write a JSON error response to stdout.
pub fn write_json_error(message: &str) -> io::Result<()> {
    let envelope: JsonEnvelope<()> =
        JsonEnvelope { success: false, data: None, error: Some(message.to_string()) };
    let mut stdout = io::stdout().lock();
    serde_json::to_writer_pretty(&mut stdout, &envelope)?;
    writeln!(stdout)?;
    Ok(())
}

/// Write a "not implemented" stub response in the appropriate output mode.
pub fn write_not_implemented(mode: &OutputMode, command_name: &str) -> io::Result<()> {
    match mode {
        OutputMode::Json => write_json_error(&format!("{command_name}: not implemented")),
        _ => {
            let mut stderr = io::stderr().lock();
            writeln!(stderr, "codemark {command_name}: not yet implemented")?;
            Ok(())
        }
    }
}

// --- Bookmark output formatting ---

/// Short ID prefix for display (first 8 chars).
pub fn short_id(id: &str) -> &str {
    &id[..id.len().min(8)]
}

/// Write a list of bookmarks in the appropriate output mode.
pub fn write_bookmarks(mode: &OutputMode, bookmarks: &[Bookmark]) -> io::Result<()> {
    match mode {
        OutputMode::Json => write_json_success(&bookmarks),
        OutputMode::Table => write_bookmarks_table(bookmarks),
        OutputMode::Line => write_bookmarks_line(bookmarks),
        OutputMode::Tv => {
            fn no_line(_: &str) -> Option<usize> {
                None
            }
            write_bookmarks_tv(bookmarks, &no_line)
        }
        OutputMode::Markdown => write_bookmarks_table(bookmarks),
        OutputMode::Custom(template) => write_bookmarks_custom(bookmarks, template),
    }
}

/// Write bookmarks in television format with line numbers.
pub fn write_bookmarks_with_line<F>(
    mode: &OutputMode,
    bookmarks: &[Bookmark],
    get_line_fn: F,
) -> io::Result<()>
where
    F: Fn(&str) -> Option<usize>,
{
    match mode {
        OutputMode::Tv => write_bookmarks_tv(bookmarks, get_line_fn),
        _ => write_bookmarks(mode, bookmarks),
    }
}

fn write_bookmarks_table(bookmarks: &[Bookmark]) -> io::Result<()> {
    if bookmarks.is_empty() {
        println!("No bookmarks found.");
        return Ok(());
    }
    let mut table = Table::new();
    table.set_header(vec!["ID", "File", "Status", "Tags", "Note", "Last Resolved"]);

    for bm in bookmarks {
        let tags = bm.tags.join(", ");
        let note = get_first_note(bm).chars().take(40).collect::<String>();
        let resolved = bm.last_resolved_at.as_deref().unwrap_or("-");
        table.add_row(vec![
            Cell::new(short_id(&bm.id)),
            Cell::new(&bm.file_path),
            Cell::new(bm.status.to_string()),
            Cell::new(tags),
            Cell::new(note),
            Cell::new(resolved),
        ]);
    }
    println!("{table}");
    Ok(())
}

fn write_bookmarks_line(bookmarks: &[Bookmark]) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    for bm in bookmarks {
        let tags = bm.tags.join(",");
        let note = get_first_note(bm);
        // Format: <id>\t<file>:<line>\t<status>\t<tags>\t<note>
        // Line is unknown without resolution, so we use file path only
        writeln!(
            stdout,
            "{}\t{}\t{}\t{}\t{}",
            short_id(&bm.id),
            bm.file_path,
            bm.status,
            tags,
            note
        )?;
    }
    Ok(())
}

/// Write bookmarks in television format with line numbers.
/// Format: <id>\t<file>\t<line>\t<status>\t<tags>\t<note>
/// This requires database access to fetch line numbers from resolutions.
/// The line number is the center of the line range for better preview positioning.
pub fn write_bookmarks_tv(
    bookmarks: &[Bookmark],
    get_center_line: impl Fn(&str) -> Option<usize>,
) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    for bm in bookmarks {
        let tags = bm.tags.join(",");
        let note = get_first_note(bm);
        let short = short_id(&bm.id);
        let line = get_center_line(short).unwrap_or(0);

        writeln!(
            stdout,
            "{}\t{}\t{}\t{}\t{}\t{}",
            short, bm.file_path, line, bm.status, tags, note
        )?;
    }
    Ok(())
}

fn write_bookmarks_custom(bookmarks: &[Bookmark], template: &str) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    for bm in bookmarks {
        let line = template
            .replace("{id}", short_id(&bm.id))
            .replace("{file}", &bm.file_path)
            .replace("{status}", &bm.status.to_string())
            .replace("{tags}", &bm.tags.join(","))
            .replace("{note}", get_first_note(bm))
            .replace("{query}", &bm.query);
        writeln!(stdout, "{line}")?;
    }
    Ok(())
}

// --- Source-annotated output for multi-db queries ---

/// A bookmark annotated with its source database label.
pub struct AnnotatedBookmark<'a> {
    pub source: &'a str,
    pub bookmark: &'a Bookmark,
}

/// Write annotated bookmarks (multi-db results with source labels).
pub fn write_annotated_bookmarks(
    mode: &OutputMode,
    bookmarks: &[AnnotatedBookmark],
) -> io::Result<()> {
    match mode {
        OutputMode::Json => {
            let items: Vec<serde_json::Value> = bookmarks
                .iter()
                .map(|ab| {
                    let mut val = serde_json::to_value(ab.bookmark).unwrap_or_default();
                    if let Some(obj) = val.as_object_mut() {
                        obj.insert("source".to_string(), serde_json::json!(ab.source));
                    }
                    val
                })
                .collect();
            write_json_success(&items)
        }
        OutputMode::Table => {
            if bookmarks.is_empty() {
                println!("No bookmarks found.");
                return Ok(());
            }
            let mut table = Table::new();
            table.set_header(vec!["Source", "ID", "File", "Status", "Tags", "Note"]);
            for ab in bookmarks {
                let bm = ab.bookmark;
                let tags = bm.tags.join(", ");
                let note = get_first_note(bm).chars().take(35).collect::<String>();
                table.add_row(vec![
                    Cell::new(ab.source),
                    Cell::new(short_id(&bm.id)),
                    Cell::new(&bm.file_path),
                    Cell::new(bm.status.to_string()),
                    Cell::new(tags),
                    Cell::new(note),
                ]);
            }
            println!("{table}");
            Ok(())
        }
        OutputMode::Line => {
            let mut stdout = io::stdout().lock();
            for ab in bookmarks {
                let bm = ab.bookmark;
                let tags = bm.tags.join(",");
                let note = get_first_note(bm);
                writeln!(
                    stdout,
                    "{}\t{}\t{}\t{}\t{}\t{}",
                    ab.source,
                    short_id(&bm.id),
                    bm.file_path,
                    bm.status,
                    tags,
                    note
                )?;
            }
            Ok(())
        }
        OutputMode::Tv => {
            // For multi-db, fall back to line format without line numbers
            // (line numbers would require db access per bookmark)
            let mut stdout = io::stdout().lock();
            for ab in bookmarks {
                let bm = ab.bookmark;
                let tags = bm.tags.join(",");
                let note = get_first_note(bm);
                // Format: source\tid\tfile\tline\tstatus\ttags\tnote
                // Use 0 for line number (no scroll) in multi-db case
                writeln!(
                    stdout,
                    "{}\t{}\t{}\t0\t{}\t{}\t{}",
                    ab.source,
                    short_id(&bm.id),
                    bm.file_path,
                    bm.status,
                    tags,
                    note
                )?;
            }
            Ok(())
        }
        OutputMode::Custom(template) => {
            let mut stdout = io::stdout().lock();
            for ab in bookmarks {
                let bm = ab.bookmark;
                let line = template
                    .replace("{source}", ab.source)
                    .replace("{id}", short_id(&bm.id))
                    .replace("{file}", &bm.file_path)
                    .replace("{status}", &bm.status.to_string())
                    .replace("{tags}", &bm.tags.join(","))
                    .replace("{note}", get_first_note(bm))
                    .replace("{query}", &bm.query);
                writeln!(stdout, "{line}")?;
            }
            Ok(())
        }
        OutputMode::Markdown => {
            // For multi-db markdown output, fall back to table format
            let mut table = Table::new();
            table.set_header(vec!["Source", "ID", "File", "Status", "Tags", "Note"]);
            for ab in bookmarks {
                let bm = ab.bookmark;
                let tags = bm.tags.join(", ");
                let note = get_first_note(bm).chars().take(35).collect::<String>();
                table.add_row(vec![
                    Cell::new(ab.source),
                    Cell::new(short_id(&bm.id)),
                    Cell::new(&bm.file_path),
                    Cell::new(bm.status.to_string()),
                    Cell::new(tags),
                    Cell::new(note),
                ]);
            }
            println!("{table}");
            Ok(())
        }
    }
}

/// Write a single-line success message.
pub fn write_success(mode: &OutputMode, message: &str) -> io::Result<()> {
    match mode {
        OutputMode::Json => write_json_success(&serde_json::json!({ "message": message })),
        OutputMode::Markdown => {
            println!("{message}");
            Ok(())
        }
        _ => {
            println!("{message}");
            Ok(())
        }
    }
}

// --- Markdown output for show command ---

/// Write a bookmark with its resolutions in markdown format.
pub fn write_bookmark_markdown(bm: &Bookmark, resolutions: &[Resolution]) -> io::Result<()> {
    let mut stdout = io::stdout().lock();

    // Title
    writeln!(stdout, "# Bookmark: {}", short_id(&bm.id))?;
    writeln!(stdout)?;

    // Metadata section
    writeln!(stdout, "## Metadata")?;
    writeln!(stdout)?;
    writeln!(stdout, "| Property | Value |")?;
    writeln!(stdout, "|----------|-------|")?;
    writeln!(stdout, "| **File** | {} |", escape_markdown(&bm.file_path))?;
    writeln!(stdout, "| **Language** | {} |", bm.language)?;
    writeln!(stdout, "| **Status** | {} |", bm.status)?;
    writeln!(stdout, "| **Created** | {} |", bm.created_at)?;
    if let Some(ref created_by) = bm.created_by {
        writeln!(stdout, "| **Author** | {} |", escape_markdown(created_by))?;
    }
    if let Some(ref resolved) = bm.last_resolved_at {
        writeln!(stdout, "| **Last Resolved** | {} |", resolved)?;
    }
    if let Some(ref method) = bm.resolution_method {
        writeln!(stdout, "| **Resolution Method** | {} |", method)?;
    }
    if let Some(ref commit) = bm.commit_hash {
        writeln!(stdout, "| **Commit** | `{}` |", &commit[..commit.len().min(8)])?;
    }
    if let Some(ref stale_since) = bm.stale_since {
        writeln!(stdout, "| **Stale Since** | {} |", stale_since)?;
    }
    writeln!(stdout)?;

    // Query section
    writeln!(stdout, "## Tree-sitter Query")?;
    writeln!(stdout)?;
    writeln!(stdout, "```scheme")?;
    for line in bm.query.lines() {
        writeln!(stdout, "{}", line)?;
    }
    writeln!(stdout, "```")?;
    writeln!(stdout)?;

    // Tags section
    if !bm.tags.is_empty() {
        writeln!(stdout, "## Tags")?;
        writeln!(stdout)?;
        for tag in &bm.tags {
            writeln!(stdout, "- `{}`", escape_markdown(tag))?;
        }
        writeln!(stdout)?;
    }

    // Annotations section
    if !bm.annotations.is_empty() {
        writeln!(stdout, "## Annotations")?;
        writeln!(stdout)?;
        for ann in &bm.annotations {
            // Header with author and timestamp
            let source = ann.source.as_deref().unwrap_or("unknown");
            let added_by = ann.added_by.as_deref().unwrap_or("unknown");
            writeln!(stdout, "### {}", added_by)?;
            writeln!(stdout, "*{}* added: {}", source, ann.added_at)?;
            writeln!(stdout)?;

            // Notes
            if let Some(ref notes) = ann.notes {
                writeln!(stdout, "{}", escape_markdown(notes))?;
                writeln!(stdout)?;
            }

            // Context
            if let Some(ref context) = ann.context {
                writeln!(stdout, "{}", escape_markdown(context))?;
                writeln!(stdout)?;
            }
        }
    }

    // Resolution history section
    if !resolutions.is_empty() {
        writeln!(stdout, "## Resolution History")?;
        writeln!(stdout)?;
        writeln!(stdout, "| Time | Method | File | Lines | Matches | Commit |")?;
        writeln!(stdout, "|------|--------|------|-------|---------|--------|")?;
        for r in resolutions {
            let file = r.file_path.as_deref().unwrap_or("-");
            let line_range = r.line_range.as_deref().unwrap_or("-");
            let match_count = r.match_count.map_or("-".to_string(), |c| c.to_string());
            let commit = r
                .commit_hash
                .as_deref()
                .map(|c| format!("`{}`", &c[..c.len().min(8)]))
                .unwrap_or_else(|| "-".to_string());
            writeln!(
                stdout,
                "| {} | {} | {} | {} | {} | {} |",
                r.resolved_at, r.method, file, line_range, match_count, commit
            )?;
        }
        writeln!(stdout)?;
    }

    Ok(())
}

// --- Heal output formatting ---

/// Output for a single bookmark update during heal.
#[derive(Debug, Serialize)]
pub struct HealUpdate {
    pub bookmark_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution_id: Option<String>,
    pub name: String,
    pub file_path: String,
    pub previous_status: String,
    pub new_status: String,
    pub resolution_method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_location: Option<ByteLocation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_location: Option<ByteLocation>,
}

/// Byte location range for resolution.
#[derive(Debug, Serialize)]
pub struct ByteLocation {
    pub start_byte: usize,
    pub end_byte: usize,
}

impl ByteLocation {
    /// Parse from a "start:end" string format stored in the database.
    pub fn from_str(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() == 2 {
            let start = parts[0].parse::<usize>().ok()?;
            let end = parts[1].parse::<usize>().ok()?;
            Some(ByteLocation { start_byte: start, end_byte: end })
        } else {
            None
        }
    }
}

/// Summary output for the heal command.
#[derive(Debug, Serialize)]
pub struct HealOutput {
    pub total_processed: usize,
    pub skipped: usize,
    pub updates: Vec<HealUpdate>,
}

/// Write heal output in the appropriate mode.
pub fn write_heal_output(mode: &OutputMode, output: &HealOutput) -> io::Result<()> {
    match mode {
        OutputMode::Json => write_json_success(output),
        OutputMode::Table => write_heal_table(output),
        OutputMode::Line => write_heal_line(output),
        _ => {
            // Fallback to table format for other modes
            write_heal_table(output)
        }
    }
}

fn write_heal_table(output: &HealOutput) -> io::Result<()> {
    let updated_count = output.updates.iter().filter(|u| u.previous_status != u.new_status).count();
    let unchanged_count =
        output.updates.iter().filter(|u| u.previous_status == u.new_status).count();

    println!(
        "Healed {} bookmarks: {} updated, {} unchanged, {} skipped",
        output.total_processed, updated_count, unchanged_count, output.skipped
    );
    Ok(())
}

fn write_heal_line(output: &HealOutput) -> io::Result<()> {
    let updated_count = output.updates.iter().filter(|u| u.previous_status != u.new_status).count();
    let unchanged_count =
        output.updates.iter().filter(|u| u.previous_status == u.new_status).count();

    let mut stdout = io::stdout().lock();
    writeln!(
        stdout,
        "total={}:updated={}:unchanged={}:skipped={}",
        output.total_processed, updated_count, unchanged_count, output.skipped
    )?;
    Ok(())
}

/// Escape special markdown characters in text.
fn escape_markdown(text: &str) -> String {
    // Escape characters that have special meaning in markdown:
    // \ ` * _ { } [ ] ( ) # + - . ! | < >
    // But be careful not to escape within code blocks
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
