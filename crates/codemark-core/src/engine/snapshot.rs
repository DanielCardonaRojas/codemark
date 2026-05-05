//! Logic for creating snapshots of a collection for publishing.
use crate::config::Config;
use crate::engine::bookmark::{
    Bookmark, BookmarkComment, BookmarkFilter, Collection, CollectionHealth, CollectionLink,
    CollectionTag, Resolution, Tag,
};
use crate::engine::resolution;
use crate::error::Result;
use crate::git::context as git_context;
use crate::parser::languages::{Language, ParseCache};
use crate::storage::db::Database;
use crate::vfs::FileProvider;
use chrono::Utc;
use std::collections::HashMap;
use std::path::Path;

/// A bundle of resolved bookmarks and metadata for a collection.
pub struct SnapshotPayload {
    pub collection: Collection,
    pub bookmarks: Vec<Bookmark>,
    pub resolutions: Vec<Resolution>,
    pub tags: Vec<Tag>,
    pub comments: Vec<BookmarkComment>,
    pub collection_tags: Vec<CollectionTag>,
    pub collection_links: Vec<CollectionLink>,
}

/// Build a fresh snapshot of a collection by resolving all its bookmarks.
pub async fn build_snapshot(
    db: &Database,
    collection_id: &str,
    project_root: &Path,
    padding: usize,
    config: &Config,
) -> Result<SnapshotPayload> {
    let mut collection = db.get_collection_by_id(collection_id)?.ok_or_else(|| {
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
    let head_commit = git_context::detect_context(project_root).and_then(|ctx| ctx.head_commit);

    for mut bm in bookmarks {
        let lang = bm
            .language
            .parse::<Language>()
            .map_err(|e| crate::error::Error::Input(e.to_string()))?;
        let mut cache = ParseCache::new(lang)?;
        let ts_lang = lang.tree_sitter_language();

        let result =
            resolution::resolve(&bm, &mut cache, &ts_lang, project_root, &provider).await?;

        // Capture preview lines
        let abs_path = git_context::resolve_bookmark_file_path(&bm.file_path, project_root)?;
        let source = provider.read_file(&abs_path, None).await?;
        let preview = result.capture_preview(&source, padding);

        // Update bookmark health based on resolution
        let hash_matches = bm.content_hash.as_ref() == Some(&result.content_hash);
        let days_since =
            crate::engine::health::days_since_resolution(bm.last_resolved_at.as_deref());
        bm.health = crate::engine::health::transition(
            bm.health,
            result.method,
            hash_matches,
            days_since,
            config.health.stale_days(),
        );
        bm.resolution_method = Some(result.method);
        bm.last_resolved_at = Some(Utc::now().to_rfc3339());

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
            content_hash: Some(result.content_hash.clone()),
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

    // Collection links
    let collection_links = db.list_links_for_collection(collection_id)?;

    // Collection tags with auto-populate logic
    let mut collection_tags = db.list_tags_for_collection(collection_id)?;
    let has_authored_tags =
        collection_tags.iter().any(|t| t.added_by.as_deref() != Some("__auto__"));

    if !has_authored_tags {
        // Auto-populate from top-3 bookmark tags
        let mut tag_counts = HashMap::new();
        for tag in &all_tags {
            *tag_counts.entry(&tag.tag).or_insert(0) += 1;
        }
        let mut sorted_tags: Vec<(&String, i32)> = tag_counts.into_iter().collect();
        sorted_tags.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));

        let now = Utc::now().to_rfc3339();
        let take_n = config.publish.autopopulate_tags_count();
        for (tag_name, _) in sorted_tags.into_iter().take(take_n) {
            collection_tags.push(CollectionTag {
                collection_id: collection_id.to_string(),
                tag: tag_name.clone(),
                added_at: now.clone(),
                added_by: Some("__auto__".to_string()),
            });
        }
    }

    // Compute collection health from the resolved bookmarks (not from DB)
    // Rule: stale > drifted > active. Archived bookmarks are excluded.
    use crate::engine::bookmark::BookmarkHealth;
    let health = if resolved_bookmarks.is_empty() {
        None
    } else {
        let has_stale = resolved_bookmarks.iter().any(|bm| bm.health == BookmarkHealth::Stale);
        let has_drifted = resolved_bookmarks.iter().any(|bm| bm.health == BookmarkHealth::Drifted);
        if has_stale {
            Some(CollectionHealth::Stale)
        } else if has_drifted {
            Some(CollectionHealth::Drifted)
        } else {
            Some(CollectionHealth::Active)
        }
    };
    collection.health = health;
    collection.health_computed_at = Some(Utc::now().to_rfc3339());

    Ok(SnapshotPayload {
        collection,
        bookmarks: resolved_bookmarks,
        resolutions,
        tags: all_tags,
        comments: all_comments,
        collection_tags,
        collection_links,
    })
}
