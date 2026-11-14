//! Turn a bookmark's anchored comments into inline threads for the preview.
//!
//! Anchors resolve against the code the preview is showing, scoped to the
//! bookmark's deepest landmark (see `codemark_core::engine::comment_anchor`).
//! Only *anchored* threads appear inline: whole-bookmark comments stay in the
//! Comments tab, and orphaned or suspended ones have nowhere to sit.

use codemark_core::engine::bookmark::{Bookmark, BookmarkComment};
use codemark_core::engine::comment_anchor::{self, AnchorState};

use crate::component::{InlineComment, InlineThread};

/// Inline threads for `bm`, resolved against `code`.
///
/// `bookmark_lines` is the bookmark's live range (0-indexed, inclusive), or
/// `None` when it did not resolve, in which case no thread is shown.
///
/// `bookmark_bytes` is its exact byte range when known; otherwise the lines
/// stand in for it (see [`bookmark_span`]).
pub(crate) fn build_inline_threads(
    bm: &Bookmark,
    code: &str,
    bookmark_lines: Option<(usize, usize)>,
    bookmark_bytes: Option<(usize, usize)>,
) -> Vec<InlineThread> {
    let root_of = thread_roots(&bm.comments);
    let roots: Vec<&BookmarkComment> = bm
        .comments
        .iter()
        .filter(|c| root_of.get(c.id.as_str()) == Some(&c.id.as_str()) && c.is_anchored())
        .collect();
    let Some((start, end)) = bookmark_lines else { return Vec::new() };
    if roots.is_empty() {
        return Vec::new();
    }
    let Ok(language) = bm.grammar() else { return Vec::new() };
    let Some(bookmark_range) = bookmark_span(code, (start, end), bookmark_bytes) else {
        return Vec::new();
    };

    let anchors: Vec<(Option<&str>, Option<&str>)> =
        roots.iter().map(|c| (c.query.as_deref(), c.content_hash.as_deref())).collect();
    let states = match comment_anchor::resolve_comment_anchors_from_source(
        code,
        &language,
        Some(bookmark_range),
        &anchors,
    ) {
        Ok(states) => states,
        Err(e) => {
            tracing::debug!(target: "codemark::comments", error = %e, "inline anchors failed to resolve");
            return Vec::new();
        }
    };

    let now = chrono::Local::now();
    let comment = |c: &BookmarkComment| InlineComment {
        id: c.id.clone(),
        author: c.author.clone(),
        when: relative_time(&c.created_at, now),
        body: c.body.clone(),
    };
    roots
        .into_iter()
        .zip(states)
        .filter_map(|(root, state)| {
            let AnchorState::Anchored(range) = state else { return None };
            // Every descendant, at any depth (imported threads can nest).
            let replies = bm.comments.iter().filter(|c| {
                c.id != root.id && root_of.get(c.id.as_str()) == Some(&root.id.as_str())
            });
            Some(InlineThread {
                root_id: root.id.clone(),
                anchor_start: range.start_line,
                anchor_end: range.end_line,
                start_col: char_column(code, range.byte_range.0),
                end_col: char_column(code, range.byte_range.1),
                entries: std::iter::once(root).chain(replies).map(comment).collect(),
            })
        })
        .collect()
}

/// Root id of each comment, following `parent_id` within `comments`. A
/// comment whose parent is not in the slice is its own root.
fn thread_roots(comments: &[BookmarkComment]) -> std::collections::HashMap<&str, &str> {
    let by_id: std::collections::HashMap<&str, &BookmarkComment> =
        comments.iter().map(|c| (c.id.as_str(), c)).collect();
    comments
        .iter()
        .map(|c| {
            let mut root = c;
            // Bounded by the slice length, so a cycle can't loop forever.
            for _ in 0..comments.len() {
                match root.parent_id.as_deref().and_then(|p| by_id.get(p)) {
                    Some(parent) => root = parent,
                    None => break,
                }
            }
            (c.id.as_str(), root.id.as_str())
        })
        .collect()
}

/// Hash identifying a file's contents, tying a resolved byte range to the
/// source it was resolved against.
pub(crate) fn source_hash(code: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    code.hash(&mut hasher);
    hasher.finish()
}

/// `range` if it was resolved against exactly `code`, else `None` so the
/// caller falls back to the bookmark's lines.
pub(crate) fn trusted_range(
    code: &str,
    range: Option<(usize, usize)>,
    source: Option<u64>,
) -> Option<(usize, usize)> {
    range.filter(|_| source == Some(source_hash(code)))
}

/// Key for the threads [`build_inline_threads`] would produce: the code, the
/// bookmark's range, every comment field the bubbles show, and today's date
/// (relative times such as "today 14:05" change at midnight).
pub(crate) fn cache_key(
    bm: &Bookmark,
    code: &str,
    lines: Option<(usize, usize)>,
    bytes: Option<(usize, usize)>,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (&bm.id, &bm.language, bm.grammar_source.map(|g| g.as_str()), code, lines, bytes)
        .hash(&mut hasher);
    for c in &bm.comments {
        (&c.id, &c.parent_id, &c.query, &c.content_hash, &c.author, &c.body, &c.created_at)
            .hash(&mut hasher);
    }
    chrono::Local::now().date_naive().hash(&mut hasher);
    hasher.finish()
}

/// Char column of byte `offset` within its line.
fn char_column(code: &str, offset: usize) -> usize {
    let offset = offset.min(code.len());
    let line_start = code[..offset].rfind('\n').map_or(0, |i| i + 1);
    code[line_start..offset].chars().count()
}

/// A comment timestamp relative to `now`, in local time: `today 14:05`,
/// `yesterday 09:30`, `Nov 3` this year, else `2025-11-03`. Unparseable
/// timestamps are shown as stored.
pub(crate) fn relative_time(iso: &str, now: chrono::DateTime<chrono::Local>) -> String {
    let Ok(at) = chrono::DateTime::parse_from_rfc3339(iso) else { return iso.to_string() };
    let at = at.with_timezone(&chrono::Local);
    let days = (now.date_naive() - at.date_naive()).num_days();
    match days {
        0 => format!("today {}", at.format("%H:%M")),
        1 => format!("yesterday {}", at.format("%H:%M")),
        _ if at.format("%Y").to_string() == now.format("%Y").to_string() => {
            at.format("%b %-d").to_string()
        }
        _ => at.format("%Y-%m-%d").to_string(),
    }
}

/// The bookmark's byte range in `code`: the exact live-resolved range when
/// known and still inside `code`, else its lines with whitespace trimmed.
/// The exact range matters when one line holds several functions.
pub(crate) fn bookmark_span(
    code: &str,
    (start, end): (usize, usize),
    bytes: Option<(usize, usize)>,
) -> Option<(usize, usize)> {
    match bytes {
        Some((s, e)) if s < e && e <= code.len() => Some((s, e)),
        _ => trimmed_line_span(code, start, end),
    }
}

/// Byte span of lines `start..=end` (0-indexed) with surrounding whitespace
/// trimmed, so the span names the bookmarked node rather than its indentation.
pub(crate) fn trimmed_line_span(code: &str, start: usize, end: usize) -> Option<(usize, usize)> {
    let mut offset = 0;
    let mut span_start = None;
    let mut span_end = None;
    for (i, line) in code.split_inclusive('\n').enumerate() {
        if i == start {
            span_start = Some(offset);
        }
        offset += line.len();
        if i == end {
            span_end = Some(offset);
            break;
        }
    }
    let (mut s, mut e) = (span_start?, span_end?);
    let bytes = code.as_bytes();
    while s < e && bytes[s].is_ascii_whitespace() {
        s += 1;
    }
    while e > s && bytes[e - 1].is_ascii_whitespace() {
        e -= 1;
    }
    (s < e).then_some((s, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_span_trims_indentation_and_newline() {
        let code = "fn a() {\n    let x = 1;\n}\n";
        let (s, e) = trimmed_line_span(code, 1, 1).unwrap();
        assert_eq!(&code[s..e], "let x = 1;");
        let (s, e) = trimmed_line_span(code, 0, 2).unwrap();
        assert_eq!(&code[s..e], "fn a() {\n    let x = 1;\n}");
        assert_eq!(trimmed_line_span(code, 9, 9), None);
    }

    #[test]
    fn relative_time_abbreviates_recent_days() {
        use chrono::TimeZone;
        let now = chrono::Local.with_ymd_and_hms(2026, 11, 12, 15, 0, 0).unwrap();
        let at = |y, m, d, h, min| {
            chrono::Local.with_ymd_and_hms(y, m, d, h, min, 0).unwrap().to_rfc3339()
        };
        assert_eq!(relative_time(&at(2026, 11, 12, 9, 5), now), "today 09:05");
        assert_eq!(relative_time(&at(2026, 11, 11, 23, 59), now), "yesterday 23:59");
        assert_eq!(relative_time(&at(2026, 11, 3, 8, 0), now), "Nov 3");
        assert_eq!(relative_time(&at(2025, 11, 3, 8, 0), now), "2025-11-03");
        assert_eq!(relative_time("bad", now), "bad");
    }

    #[test]
    fn char_column_counts_chars_within_the_line() {
        let code = "ab\n  é x";
        assert_eq!(char_column(code, 0), 0);
        assert_eq!(char_column(code, 3), 0);
        assert_eq!(char_column(code, code.find('x').unwrap()), 4);
    }

    #[test]
    #[cfg_attr(not(feature = "wasm"), ignore = "needs the wasm feature to load registry grammars")]
    fn one_line_with_two_functions_scopes_to_the_bookmarked_one() {
        use codemark_core::engine::bookmark::BookmarkHealth;
        // Both functions share a line and an identical call. Whole lines would
        // scope the comment to both; the exact byte range keeps it in `b`.
        let code = "fn a() { ping(); } fn b() { ping(); }\nfn ping() {}\n";
        let b = {
            let start = code.find("fn b").unwrap();
            (start, code.find("}\n").unwrap() + 1)
        };
        let ping_in_b = {
            let start = code.rfind("ping();").unwrap();
            (start, start + "ping()".len())
        };
        let lang: codemark_core::parser::languages::Language = "rust".parse().unwrap();
        let anchor =
            comment_anchor::generate_comment_anchor_from_source(code, &lang, b, ping_in_b).unwrap();
        let bm = Bookmark {
            id: "bm".into(),
            query: "(function_item) @target".into(),
            language: "rust".into(),
            grammar_source: None,
            forked_from: None,
            branch: None,
            file_path: "lib.rs".into(),
            content_hash: None,
            commit_hash: None,
            health: BookmarkHealth::Active,
            resolution_method: None,
            last_resolved_at: None,
            stale_since: None,
            created_at: "2026-01-01T00:00:00Z".into(),
            created_by: None,
            current_resolution_id: None,
            repo_id: None,
            tags: Vec::new(),
            annotations: Vec::new(),
            comments: vec![BookmarkComment::new("bm", "claude", "why ping?").with_anchor(anchor)],
        };

        let threads = build_inline_threads(&bm, code, Some((0, 0)), Some(b));
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].start_col, ping_in_b.0, "anchored in `b`, not `a`");
    }

    #[test]
    fn bookmark_span_prefers_the_exact_range_when_it_fits() {
        let code = "fn a() {} fn b() {}\n";
        assert_eq!(bookmark_span(code, (0, 0), Some((10, 19))), Some((10, 19)));
        assert_eq!(bookmark_span(code, (0, 0), None), Some((0, 19)), "falls back to the line");
        assert_eq!(bookmark_span(code, (0, 0), Some((10, 999))), Some((0, 19)), "stale range");
    }

    #[test]
    fn thread_roots_reach_the_root_at_any_depth() {
        let c = |id: &str, parent: Option<&str>| BookmarkComment {
            id: id.into(),
            parent_id: parent.map(str::to_string),
            ..BookmarkComment::new("b", "a", "x")
        };
        let comments = vec![
            c("root", None),
            c("r1", Some("root")),
            c("r2", Some("r1")),
            c("lost", Some("gone")),
        ];
        let roots = thread_roots(&comments);
        assert_eq!(roots["r2"], "root");
        assert_eq!(roots["lost"], "lost", "a missing parent makes a root");
    }

    #[test]
    fn ranges_from_another_version_of_the_file_are_not_trusted() {
        let code = "fn a() {}\n";
        let hash = source_hash(code);
        assert_eq!(trusted_range(code, Some((0, 9)), Some(hash)), Some((0, 9)));
        assert_eq!(trusted_range("fn ab() {}\n", Some((0, 9)), Some(hash)), None, "file changed");
        assert_eq!(trusted_range(code, Some((0, 9)), None), None, "unknown source");
    }
}
