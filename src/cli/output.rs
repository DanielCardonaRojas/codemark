use std::io::{self, Write};

use comfy_table::{Cell, Table};
use is_terminal::IsTerminal;
use serde::Serialize;

use crate::engine::bookmark::Bookmark;

/// Resolved output mode for a command invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputMode {
    Table,
    Line,
    Json,
    Custom(String),
}

impl OutputMode {
    /// Determine the output mode from CLI flags and TTY detection.
    ///
    /// Priority: --json > --format > auto-detect (TTY=table, pipe=line).
    pub fn resolve(json_flag: bool, format_flag: Option<&str>) -> Self {
        if json_flag {
            return OutputMode::Json;
        }
        match format_flag {
            Some("table") => OutputMode::Table,
            Some("line") => OutputMode::Line,
            Some("json") => OutputMode::Json,
            Some(template) => OutputMode::Custom(template.to_string()),
            None => {
                if io::stdout().is_terminal() {
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
    let envelope = JsonEnvelope {
        success: true,
        data: Some(data),
        error: None,
    };
    let mut stdout = io::stdout().lock();
    serde_json::to_writer_pretty(&mut stdout, &envelope)?;
    writeln!(stdout)?;
    Ok(())
}

/// Write a JSON error response to stdout.
pub fn write_json_error(message: &str) -> io::Result<()> {
    let envelope: JsonEnvelope<()> = JsonEnvelope {
        success: false,
        data: None,
        error: Some(message.to_string()),
    };
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
        OutputMode::Custom(template) => write_bookmarks_custom(bookmarks, template),
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
        let note = bm
            .notes
            .as_deref()
            .unwrap_or("")
            .chars()
            .take(40)
            .collect::<String>();
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
        let note = bm.notes.as_deref().unwrap_or("");
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

fn write_bookmarks_custom(bookmarks: &[Bookmark], template: &str) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    for bm in bookmarks {
        let line = template
            .replace("{id}", short_id(&bm.id))
            .replace("{file}", &bm.file_path)
            .replace("{status}", &bm.status.to_string())
            .replace("{tags}", &bm.tags.join(","))
            .replace("{note}", bm.notes.as_deref().unwrap_or(""))
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
            table.set_header(vec![
                "Source", "ID", "File", "Status", "Tags", "Note",
            ]);
            for ab in bookmarks {
                let bm = ab.bookmark;
                let tags = bm.tags.join(", ");
                let note = bm.notes.as_deref().unwrap_or("").chars().take(35).collect::<String>();
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
                let note = bm.notes.as_deref().unwrap_or("");
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
                    .replace("{note}", bm.notes.as_deref().unwrap_or(""))
                    .replace("{query}", &bm.query);
                writeln!(stdout, "{line}")?;
            }
            Ok(())
        }
    }
}

/// Write a single-line success message.
pub fn write_success(mode: &OutputMode, message: &str) -> io::Result<()> {
    match mode {
        OutputMode::Json => {
            write_json_success(&serde_json::json!({ "message": message }))
        }
        _ => {
            println!("{message}");
            Ok(())
        }
    }
}
