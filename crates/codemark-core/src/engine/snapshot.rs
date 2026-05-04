//! Logic for creating snapshots of a collection for publishing.

use crate::engine::bookmark::{
    Bookmark, BookmarkComment, BookmarkFilter, Collection, Resolution, Tag,
};
use crate::engine::resolution;
use crate::error::Result;
use crate::git::context as git_context;
use crate::parser::languages::{Language, ParseCache};
use crate::storage::db::Database;
use crate::vfs::FileProvider;
use chrono::Utc;

/// A bundle of resolved bookmarks and metadata for a collection.
pub struct SnapshotPayload {
    pub collection: Collection,
    pub bookmarks: Vec<Bookmark>,
    pub resolutions: Vec<Resolution>,
    pub tags: Vec<Tag>,
    pub comments: Vec<BookmarkComment>,
}

/// Build a fresh snapshot of a collection by resolving all its bookmarks.
pub async fn build_snapshot(
    db: &Database,
    collection_id: &str,
    padding: usize,
) -> Result<SnapshotPayload> {
    let collection = db.get_collection_by_id(collection_id)?.ok_or_else(|| {
        crate::error::Error::Input(format!("collection {collection_id} not found"))
    })?;

    let filter =
        BookmarkFilter { collection_id: Some(collection_id.to_string()), ..Default::default() };
    let bookmarks = db.list_bookmarks(&filter)?;
    let mut resolved_bookmarks = Vec::new();
    let mut resolutions = Vec::new();
    let mut all_tags = Vec::new();
    let mut all_comments = Vec::new();

    let provider = crate::vfs::LocalFileProvider;
    let head_commit =
        git_context::detect_context(&std::env::current_dir()?).and_then(|ctx| ctx.head_commit);

    for bm in bookmarks {
        let lang = bm
            .language
            .parse::<Language>()
            .map_err(|e| crate::error::Error::Input(e.to_string()))?;
        let mut cache = ParseCache::new(lang)?;
        let ts_lang = lang.tree_sitter_language();

        let result = resolution::resolve(&bm, &mut cache, &ts_lang, db.path(), &provider).await?;

        // Capture preview lines
        let abs_path = git_context::resolve_bookmark_file_path(&bm.file_path, db.path())?;
        let source = provider.read_file(&abs_path, None).await?;
        let preview = result.capture_preview(&source, padding);

        // Build the resolution record
        let res_id = uuid::Uuid::new_v4().to_string();
        let resolution = Resolution {
            id: res_id,
            bookmark_id: bm.id.clone(),
            resolved_at: Utc::now().to_rfc3339(),
            commit_hash: head_commit.clone(),
            method: result.method,
            match_count: Some(1),
            file_path: Some(bm.file_path.clone()),
            byte_range: Some(format!("{}-{}", result.byte_range.0, result.byte_range.1)),
            line_range: Some(format!("{}-{}", result.start_line + 1, result.end_line + 1)),
            content_hash: Some(result.content_hash),
            headline: bm
                .annotations
                .iter()
                .find_map(|a| a.notes.clone())
                .or(Some(result.matched_text.clone())),
            preview_lines: Some(preview),
        };

        // Fetch tags for this bookmark
        let tags = db.list_tags_for_bookmark(&bm.id)?;
        all_tags.extend(tags);

        // Comments
        all_comments.extend(bm.comments.clone());

        resolved_bookmarks.push(bm);
        resolutions.push(resolution);
    }

    Ok(SnapshotPayload {
        collection,
        bookmarks: resolved_bookmarks,
        resolutions,
        tags: all_tags,
        comments: all_comments,
    })
}
