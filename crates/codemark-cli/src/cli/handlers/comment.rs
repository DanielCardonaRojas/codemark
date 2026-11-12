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

use super::{find_bookmark, now_iso, open_all_dbs, open_db_for_write};

pub async fn handle_comment(cli: &Cli, mode: &OutputMode, args: &CommentArgs) -> Result<()> {
    match &args.command {
        CommentCommand::Add(a) => handle_add(cli, mode, a).await,
        CommentCommand::Reply(a) => handle_reply(cli, mode, a).await,
        CommentCommand::List(a) => handle_list(cli, mode, a).await,
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
    let comment_range = super::parse_range(range, &source)?;
    comment_anchor::generate_comment_anchor_from_source(
        &source,
        &live.language,
        bookmark_range,
        comment_range,
    )
}

async fn handle_add(cli: &Cli, mode: &OutputMode, args: &CommentAddArgs) -> Result<()> {
    let db = open_db_for_write(cli)?;
    let bm = find_bookmark(&db, &args.bookmark_id)?;
    let mut comment = BookmarkComment {
        created_at: now_iso(),
        ..BookmarkComment::new(&bm.id, &args.created_by, &args.body)
    };
    if let Some(ref range) = args.range {
        comment = comment.with_anchor(anchor_for_range(&bm, &db, range).await?);
    }
    db.insert_comment(&comment)?;
    tracing::debug!(target: "codemark::comments", id = %comment.id, anchored = comment.is_anchored(), "comment added");

    let view = CommentView::fresh(&comment);
    write_one(mode, &view, "Added comment")
}

async fn handle_reply(cli: &Cli, mode: &OutputMode, args: &CommentReplyArgs) -> Result<()> {
    let db = open_db_for_write(cli)?;
    let parent = find_comment(&db, &args.comment_id)?;
    // Threads are one level deep: a reply to a reply joins the root's thread.
    let root_id = parent.parent_id.clone().unwrap_or(parent.id.clone());
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

fn handle_rm(cli: &Cli, mode: &OutputMode, args: &CommentRmArgs) -> Result<()> {
    let db = open_db_for_write(cli)?;
    let comment = find_comment(&db, &args.comment_id)?;
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
    let db = open_db_for_write(cli)?;
    let comment = find_comment(&db, &args.comment_id)?;
    if comment.parent_id.is_some() {
        return Err(Error::Input(
            "replies share their root comment's anchor; re-anchor the root comment instead".into(),
        ));
    }
    let bm = find_bookmark(&db, &comment.bookmark_id)?;
    let anchor = match args.range {
        Some(ref range) if !args.detach => Some(anchor_for_range(&bm, &db, range).await?),
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

fn find_comment(db: &Database, id: &str) -> Result<BookmarkComment> {
    db.get_comment_by_prefix(id)?.ok_or_else(|| Error::Input(format!("comment not found: {id}")))
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
    let roots: Vec<&BookmarkComment> = comments.iter().filter(|c| c.parent_id.is_none()).collect();
    let anchors: Vec<(Option<&str>, Option<&str>)> =
        roots.iter().map(|c| (c.query.as_deref(), c.content_hash.as_deref())).collect();

    let (states, source) =
        if roots.iter().any(|c| c.is_anchored()) {
            let live = resolve_live(bm, db.path()).await?;
            match live.source {
                Some(source) => {
                    let range = live.range;
                    let states = comment_anchor::resolve_comment_anchors_from_source(
                        &source,
                        &live.language,
                        range,
                        &anchors,
                    )?;
                    (states, Some(source))
                }
                None => (
                    anchors
                        .iter()
                        .map(|(q, _)| {
                            if q.is_some() { AnchorState::Suspended } else { AnchorState::Bookmark }
                        })
                        .collect(),
                    None,
                ),
            }
        } else {
            (vec![AnchorState::Bookmark; roots.len()], None)
        };

    Ok(roots
        .into_iter()
        .zip(states)
        .map(|(root, state)| {
            let mut view = CommentView::new(root, state, source.as_deref());
            view.replies = comments
                .iter()
                .filter(|c| c.parent_id.as_deref() == Some(root.id.as_str()))
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
            out.push_str(&format!("  - **{}** · {}: {}\n", r.author, r.created_at, r.body.trim()));
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
    fn hint_lines_parses_one_indexed_hints() {
        assert_eq!(hint_lines(Some("L5-L7: let x = 1;")), Some((4, 6)));
        assert_eq!(hint_lines(Some("L1-L1: a: b")), Some((0, 0)));
        assert_eq!(hint_lines(Some("garbage")), None);
        assert_eq!(hint_lines(None), None);
    }
}
