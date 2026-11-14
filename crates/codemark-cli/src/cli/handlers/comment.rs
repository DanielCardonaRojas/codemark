//! `codemark comment`: whole-bookmark and anchored comments, with replies.
//!
//! An anchored comment points at one node inside its bookmark's deepest
//! landmark (see `codemark_core::engine::comment_anchor`). Agents pass a file
//! range and codemark generates the query; nobody writes tree-sitter by hand.

use std::path::Path;

use serde::Serialize;

use crate::cli::output::{OutputMode, write_json_success};
use crate::cli::*;
use codemark_core::engine::bookmark::{Bookmark, BookmarkComment, ResolutionMethod};
use codemark_core::engine::comment_anchor::{self, AnchorState};
use codemark_core::engine::resolution;
use codemark_core::error::{Error, Result};
use codemark_core::git::context as git_context;
use codemark_core::parser::languages::{Language, ParseCache};
use codemark_core::storage::Workspace;
use codemark_core::storage::db::Database;

use super::{now_iso, open_all_dbs};

pub async fn handle_comment(cli: &Cli, mode: &OutputMode, args: &CommentArgs) -> Result<()> {
    match &args.command {
        CommentCommand::Add(a) => handle_add(cli, mode, a).await,
        CommentCommand::Reply(a) => handle_reply(cli, mode, a).await,
        CommentCommand::List(a) => handle_list(cli, mode, a).await,
        CommentCommand::Edit(a) => handle_edit(cli, mode, a).await,
        CommentCommand::Rm(a) => handle_rm(cli, mode, a),
        CommentCommand::Reanchor(a) => handle_reanchor(cli, mode, a).await,
    }
}

/// A bookmark resolved against the working tree, with what anchoring needs.
struct LiveBookmark {
    /// The grammar that actually parsed the file (may be a fallback).
    language: Language,
    /// Current file contents; `None` when the file can't be read.
    source: Option<String>,
    /// The bookmark's resolved target; `None` when it did not resolve.
    range: Option<(usize, usize)>,
}

async fn resolve_live(bm: &Bookmark, db_path: &Path) -> Result<LiveBookmark> {
    let mut cache = ParseCache::new(bm.grammar()?)?;
    let language = cache.codemark_language().clone();
    let ts_lang = cache.language().clone();
    let provider = codemark_core::vfs::LocalFileProvider;

    let range = match resolution::resolve(bm, &mut cache, &ts_lang, db_path, &provider).await {
        Ok(r) if r.method != ResolutionMethod::Failed => Some(r.byte_range),
        Ok(_) => None,
        Err(e) => {
            tracing::debug!(target: "codemark::comments", bookmark = %bm.id, error = %e, "bookmark did not resolve");
            None
        }
    };
    let source = git_context::resolve_bookmark_file_path(&bm.file_path, db_path)
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok());
    Ok(LiveBookmark { language, source, range })
}

/// Generate an anchor for `range` (CLI range syntax) inside `bm`.
async fn anchor_for_range(
    bm: &Bookmark,
    db: &Database,
    range: &str,
) -> Result<comment_anchor::CommentAnchor> {
    let live = resolve_live(bm, db.path()).await?;
    let (Some(source), Some(bookmark_range)) = (live.source, live.range) else {
        return Err(Error::Resolution(format!(
            "bookmark {} does not resolve in the current code, so a comment can't be \
             anchored to it. Comment without --range, or heal the bookmark first.",
            short(&bm.id)
        )));
    };
    let comment_range = parse_comment_range(range, &source)?;
    comment_anchor::generate_comment_anchor_from_source(
        &source,
        &live.language,
        bookmark_range,
        comment_range,
    )
}

async fn handle_add(cli: &Cli, mode: &OutputMode, args: &CommentAddArgs) -> Result<()> {
    // Every subcommand searches the databases `--db` / `--repo` select and
    // writes to the one that owns the bookmark or comment.
    let dbs = open_all_dbs(cli)?;
    let (bm, db) = Workspace::find_bookmark_across(&dbs, &args.bookmark_id)?;
    let mut comment = BookmarkComment {
        created_at: now_iso(),
        ..BookmarkComment::new(&bm.id, &args.created_by, &args.body)
    };
    if let Some(ref range) = args.range {
        comment = comment.with_anchor(anchor_for_range(&bm, db, range).await?);
    }
    db.insert_comment(&comment)?;
    tracing::debug!(target: "codemark::comments", id = %comment.id, anchored = comment.is_anchored(), "comment added");

    let view = CommentView::fresh(&comment);
    write_one(mode, &view, "Added comment")
}

async fn handle_reply(cli: &Cli, mode: &OutputMode, args: &CommentReplyArgs) -> Result<()> {
    let dbs = open_all_dbs(cli)?;
    let (parent, db) = find_comment(&dbs, &args.comment_id)?;
    // Threads are one level deep: a reply to a reply (at any depth, as
    // imported threads can nest) joins the root's thread.
    let root_id = thread_root(db, &parent)?;
    let reply = BookmarkComment {
        created_at: now_iso(),
        parent_id: Some(root_id),
        ..BookmarkComment::new(&parent.bookmark_id, &args.created_by, &args.body)
    };
    db.insert_comment(&reply)?;
    let view = CommentView::fresh(&reply);
    write_one(mode, &view, "Added reply")
}

async fn handle_list(cli: &Cli, mode: &OutputMode, args: &CommentListArgs) -> Result<()> {
    // Comments are threads, not rows: JSON and markdown are the shapes that
    // carry replies. Say so rather than answer another format with markdown.
    if !matches!(mode, OutputMode::Json | OutputMode::Markdown) {
        return Err(Error::Input(
            "comment list supports --format json (default) or --format markdown".into(),
        ));
    }
    let dbs = open_all_dbs(cli)?;
    let (bm, db) = Workspace::find_bookmark_across(&dbs, &args.bookmark_id)?;
    let comments = db.list_comments_for_bookmark(&bm.id)?;
    let views = build_threads(&bm, db, &comments).await?;

    match mode {
        OutputMode::Json => write_json_success(&views)?,
        _ => print!("{}", render_markdown(&bm, &views)),
    }
    Ok(())
}

/// Parse a comment `--range`. Like `codemark add --range`, except that a
/// plain line range (`42-67`) includes its last line: the shared parser
/// reads `67` as the start of that line.
fn parse_comment_range(range: &str, source: &str) -> Result<(usize, usize)> {
    let lines = range.trim().split_once('-').and_then(|(start, end)| {
        Some((start.trim().parse::<usize>().ok()?, end.trim().parse::<usize>().ok()?))
    });
    match lines {
        Some((start, end)) => super::line_range_to_bytes(source, start, end),
        None => super::parse_range(range, source),
    }
}

/// Id of the root of `comment`'s thread, following `parent_id` up through
/// any depth. A parent missing from the database ends the walk there.
fn thread_root(db: &Database, comment: &BookmarkComment) -> Result<String> {
    let mut current = comment.clone();
    // Bounded so a malformed cycle can't loop forever.
    for _ in 0..1000 {
        match current.parent_id.as_deref().map(|id| db.get_comment(id)).transpose()?.flatten() {
            Some(parent) => current = parent,
            None => return Ok(current.id),
        }
    }
    Err(Error::Operation(format!("comment {} has a reply cycle", comment.id)))
}

/// Root id of each comment in `comments`, following `parent_id` within the
/// slice. Comments whose chain leaves the slice root at the last one found.
fn roots_by_id(comments: &[BookmarkComment]) -> std::collections::HashMap<&str, &str> {
    let by_id: std::collections::HashMap<&str, &BookmarkComment> =
        comments.iter().map(|c| (c.id.as_str(), c)).collect();
    comments
        .iter()
        .map(|c| {
            let mut root = c;
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

async fn handle_edit(cli: &Cli, mode: &OutputMode, args: &CommentEditArgs) -> Result<()> {
    let body = args.body.trim();
    if body.is_empty() {
        return Err(Error::Input("comment body must not be empty".into()));
    }
    let dbs = open_all_dbs(cli)?;
    let (comment, db) = find_comment(&dbs, &args.comment_id)?;
    db.update_comment_body(&comment.id, body)?;

    // Report where the comment points now, resolved like `comment list`,
    // not the hint stored when it was made.
    let bm = db
        .get_bookmark(&comment.bookmark_id)?
        .ok_or_else(|| Error::Input(format!("bookmark not found: {}", comment.bookmark_id)))?;
    let comments = db.list_comments_for_bookmark(&bm.id)?;
    let mut view = take_view(build_threads(&bm, db, &comments).await?, &comment.id)
        .ok_or_else(|| Error::Operation("comment vanished while editing".into()))?;
    view.replies.clear();
    write_one(mode, &view, "Edited comment")
}

/// The view of comment `id` among `views` and their replies, at any depth.
fn take_view(views: Vec<CommentView>, id: &str) -> Option<CommentView> {
    views.into_iter().find_map(|v| if v.id == id { Some(v) } else { take_view(v.replies, id) })
}

fn handle_rm(cli: &Cli, mode: &OutputMode, args: &CommentRmArgs) -> Result<()> {
    let dbs = open_all_dbs(cli)?;
    let (comment, db) = find_comment(&dbs, &args.comment_id)?;
    let removed = db.delete_comment_thread(&comment.id)?;
    let message = format!("Removed comment {} ({removed} with replies)", short(&comment.id));
    match mode {
        OutputMode::Json => write_json_success(&serde_json::json!({
            "id": comment.id,
            "removed": removed,
        }))?,
        _ => println!("{message}"),
    }
    Ok(())
}

async fn handle_reanchor(cli: &Cli, mode: &OutputMode, args: &CommentReanchorArgs) -> Result<()> {
    let dbs = open_all_dbs(cli)?;
    let (comment, db) = find_comment(&dbs, &args.comment_id)?;
    if comment.parent_id.is_some() {
        return Err(Error::Input(
            "replies share their root comment's anchor; re-anchor the root comment instead".into(),
        ));
    }
    let bm = db
        .get_bookmark(&comment.bookmark_id)?
        .ok_or_else(|| Error::Input(format!("bookmark not found: {}", comment.bookmark_id)))?;
    let anchor = match args.range {
        Some(ref range) if !args.detach => Some(anchor_for_range(&bm, db, range).await?),
        _ => None,
    };
    db.update_comment_anchor(&comment.id, anchor.as_ref())?;

    let updated = db
        .get_comment(&comment.id)?
        .ok_or_else(|| Error::Operation("comment vanished while re-anchoring".into()))?;
    let view = CommentView::fresh(&updated);
    write_one(
        mode,
        &view,
        if anchor.is_some() { "Re-anchored comment" } else { "Detached comment" },
    )
}

/// Find a comment by id or unambiguous prefix across `dbs`, with the
/// database that owns it.
fn find_comment<'a>(
    dbs: &'a [(String, Database)],
    id: &str,
) -> Result<(BookmarkComment, &'a Database)> {
    let mut found = None;
    for (label, db) in dbs {
        if let Some(comment) = db.get_comment_by_prefix(id)? {
            if found.is_some() {
                return Err(Error::Input(format!(
                    "comment id '{id}' matches in more than one repository (also '{label}'); \
                     use more characters or --repo"
                )));
            }
            found = Some((comment, db));
        }
    }
    found.ok_or_else(|| Error::Input(format!("comment not found: {id}")))
}

/// 0-indexed `(start, end)` lines from an `L12-L14: ...` hint.
fn hint_lines(hint: Option<&str>) -> Option<(usize, usize)> {
    let range = hint?.split(':').next()?;
    let (start, end) = range.split_once('-')?;
    let parse = |s: &str| s.trim_start_matches('L').parse::<usize>().ok()?.checked_sub(1);
    Some((parse(start)?, parse(end)?))
}

/// Resolve every root comment's anchor with one bookmark resolution and one
/// parse, and nest replies under their roots.
async fn build_threads(
    bm: &Bookmark,
    db: &Database,
    comments: &[BookmarkComment],
) -> Result<Vec<CommentView>> {
    let roots_of = roots_by_id(comments);
    // A comment is a root when its chain ends at itself: no parent, or a
    // parent that is gone. Either way its thread is listed, not dropped.
    let roots: Vec<&BookmarkComment> =
        comments.iter().filter(|c| roots_of.get(c.id.as_str()) == Some(&c.id.as_str())).collect();
    let anchors: Vec<(Option<&str>, Option<&str>)> =
        roots.iter().map(|c| (c.query.as_deref(), c.content_hash.as_deref())).collect();

    // A bookmark whose grammar can't load is as unresolvable as a broken one:
    // its anchored comments are suspended, and the listing still shows them.
    let live = if roots.iter().any(|c| c.is_anchored()) {
        match resolve_live(bm, db.path()).await {
            Ok(live) => Some(live),
            Err(e) => {
                tracing::debug!(target: "codemark::comments", bookmark = %bm.id, error = %e, "bookmark can't be resolved; anchors suspended");
                None
            }
        }
    } else {
        None
    };
    let (states, source) = if roots.iter().any(|c| c.is_anchored()) {
        match live.and_then(|l| l.source.map(|source| (source, l.language, l.range))) {
            Some((source, language, range)) => {
                let states = comment_anchor::resolve_comment_anchors_from_source(
                        &source,
                        &language,
                        range,
                        &anchors,
                    )
                    .unwrap_or_else(|e| {
                        tracing::debug!(target: "codemark::comments", error = %e, "anchors could not be resolved");
                        anchors
                            .iter()
                            .map(|(q, _)| {
                                if q.is_some() { AnchorState::Suspended } else { AnchorState::Bookmark }
                            })
                            .collect()
                    });
                (states, Some(source))
            }
            None => {
                (
                    anchors
                        .iter()
                        .map(|(q, _)| {
                            if q.is_some() { AnchorState::Suspended } else { AnchorState::Bookmark }
                        })
                        .collect(),
                    None,
                )
            }
        }
    } else {
        (vec![AnchorState::Bookmark; roots.len()], None)
    };

    Ok(roots
        .into_iter()
        .zip(states)
        .map(|(root, state)| {
            let mut view = CommentView::new(root, state, source.as_deref());
            // Every descendant, at any depth, oldest first under its root.
            view.replies = comments
                .iter()
                .filter(|c| {
                    c.id != root.id && roots_of.get(c.id.as_str()) == Some(&root.id.as_str())
                })
                .map(|c| CommentView::new(c, AnchorState::Bookmark, None))
                .collect();
            view
        })
        .collect())
}

/// One comment as reported by `comment list` / `add` / `reply` / `reanchor`.
#[derive(Debug, Serialize)]
struct CommentView {
    id: String,
    bookmark_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_id: Option<String>,
    author: String,
    created_at: String,
    body: String,
    /// `bookmark`, `anchored`, `orphaned` or `suspended`.
    anchor: &'static str,
    /// 1-indexed lines the anchor resolves to now.
    #[serde(skip_serializing_if = "Option::is_none")]
    lines: Option<Lines>,
    /// First line of the anchored code as it reads now.
    #[serde(skip_serializing_if = "Option::is_none")]
    snippet: Option<String>,
    /// Where the anchor pointed when it was made.
    #[serde(skip_serializing_if = "Option::is_none")]
    anchor_hint: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    replies: Vec<CommentView>,
}

#[derive(Debug, Serialize)]
struct Lines {
    start: usize,
    end: usize,
}

impl CommentView {
    /// A comment just written: its anchor was generated against the current
    /// code, so it is reported as anchored at its hint's lines without
    /// resolving again. `comment list` re-resolves.
    fn fresh(c: &BookmarkComment) -> Self {
        let mut view = Self::new(c, AnchorState::Bookmark, None);
        if c.is_anchored() {
            view.anchor = "anchored";
            view.lines = hint_lines(c.anchor_hint.as_deref())
                .map(|(start, end)| Lines { start: start + 1, end: end + 1 });
        }
        view
    }

    fn new(c: &BookmarkComment, state: AnchorState, source: Option<&str>) -> Self {
        let (lines, snippet) = match &state {
            AnchorState::Anchored(r) => (
                Some(Lines { start: r.start_line + 1, end: r.end_line + 1 }),
                source.and_then(|s| {
                    let text = s.get(r.byte_range.0..r.byte_range.1)?;
                    text.lines().map(str::trim).find(|l| !l.is_empty()).map(str::to_string)
                }),
            ),
            _ => (None, None),
        };
        CommentView {
            id: c.id.clone(),
            bookmark_id: c.bookmark_id.clone(),
            parent_id: c.parent_id.clone(),
            author: c.author.clone(),
            created_at: c.created_at.clone(),
            body: c.body.clone(),
            anchor: state.as_str(),
            lines,
            snippet,
            anchor_hint: c.anchor_hint.clone(),
            replies: Vec::new(),
        }
    }
}

fn write_one(mode: &OutputMode, view: &CommentView, verb: &str) -> Result<()> {
    match mode {
        OutputMode::Json => write_json_success(view)?,
        _ => {
            let place = match &view.lines {
                Some(l) => format!(" on lines {}-{}", l.start, l.end),
                None => String::new(),
            };
            println!("{verb} {}{place}", short(&view.id));
        }
    }
    Ok(())
}

/// Markdown for `comment list`: one section per thread, the anchored code
/// quoted above the body, replies indented under it.
fn render_markdown(bm: &Bookmark, views: &[CommentView]) -> String {
    let mut out = format!("# Comments on `{}` ({})\n\n", bm.file_path, short(&bm.id));
    if views.is_empty() {
        out.push_str("_No comments._\n");
        return out;
    }
    for (i, v) in views.iter().enumerate() {
        if i > 0 {
            out.push_str("---\n\n");
        }
        out.push_str(&format!("**{}** · {} · `{}`", v.author, v.created_at, short(&v.id)));
        match (v.anchor, &v.lines) {
            ("anchored", Some(l)) => out.push_str(&format!(" · lines {}-{}\n\n", l.start, l.end)),
            ("bookmark", _) => out.push_str(" · whole bookmark\n\n"),
            (state, _) => out.push_str(&format!(" · {state}\n\n")),
        }
        match (&v.snippet, &v.anchor_hint) {
            (Some(snippet), _) => out.push_str(&format!("> `{snippet}`\n\n")),
            (None, Some(hint)) if v.anchor != "bookmark" => {
                out.push_str(&format!("> was `{hint}`\n\n"))
            }
            _ => {}
        }
        out.push_str(&format!("{}\n\n", v.body.trim_end()));
        for r in &v.replies {
            // Continuation lines (and blank ones) stay inside the reply item.
            let body = r.body.trim().replace('\n', "\n    ");
            out.push_str(&format!("  - **{}** · {}: {}\n", r.author, r.created_at, body));
        }
        if !v.replies.is_empty() {
            out.push('\n');
        }
    }
    out
}

fn short(id: &str) -> &str {
    &id[..id.len().min(8)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comment_line_ranges_include_their_last_line() {
        let source = "a\nbb\nccc\n";
        // Lines 2-3 run from the start of `bb` to the end of `ccc\n`.
        assert_eq!(parse_comment_range("2-3", source).unwrap(), (2, 9));
        // Other forms keep the shared parser's meaning.
        assert_eq!(parse_comment_range("2", source).unwrap(), (2, 5));
        assert_eq!(parse_comment_range("b1-3", source).unwrap(), (1, 3));
        assert!(parse_comment_range("3-2", source).is_err());
    }

    #[test]
    fn roots_follow_parents_at_any_depth() {
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
        let roots = roots_by_id(&comments);
        assert_eq!(roots["r2"], "root", "a reply to a reply belongs to the root's thread");
        assert_eq!(roots["lost"], "lost", "a reply whose parent is gone becomes a root");
    }

    #[test]
    fn hint_lines_parses_one_indexed_hints() {
        assert_eq!(hint_lines(Some("L5-L7: let x = 1;")), Some((4, 6)));
        assert_eq!(hint_lines(Some("L1-L1: a: b")), Some((0, 0)));
        assert_eq!(hint_lines(Some("garbage")), None);
        assert_eq!(hint_lines(None), None);
    }
}
