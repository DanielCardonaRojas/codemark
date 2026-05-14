//! Logic for creating and reading portable SQLite "packs" for sharing collections.

pub mod inspector;
pub mod reader;
pub mod writer;

pub use inspector::{PackError, PackInfo, inspect, pre_inspect};
pub use reader::PackReader;
pub use writer::Packer;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::bookmark::{Bookmark, Collection, Visibility, BookmarkHealth, ResolutionMethod};
    use crate::engine::snapshot::SnapshotPayload;
    use crate::storage::db::Database;
    use tempfile::tempdir;
    use uuid::Uuid;

    #[tokio::test]
    async fn test_pack_roundtrip() {
        let tmp = tempdir().unwrap();
        let db_path = tmp.path().join("source.db");
        let db = Database::create(&db_path).unwrap();

        let col_id = Uuid::new_v4().to_string();
        let bm_id = Uuid::new_v4().to_string();
        
        let payload = SnapshotPayload {
            collection: Collection {
                id: col_id.clone(),
                name: "Test Pack".to_string(),
                description: Some("Description".to_string()),
                visibility: Visibility::Public,
                created_at: "2024-01-01T00:00:00Z".to_string(),
                created_by: None,
                created_branch: None,
                published_at: None,
                published_commit_sha: None,
                repo_url: None,
                repo_id: None,
                status: None,
                health: None,
                health_computed_at: None,
                updated_at: None,
                imported_from_url: None,
            },
            bookmarks: vec![Bookmark {
                id: bm_id.clone(),
                query: "fn main".to_string(),
                language: "rust".to_string(),
                file_path: "src/main.rs".to_string(),
                content_hash: None,
                commit_hash: None,
                health: BookmarkHealth::Active,
                resolution_method: Some(ResolutionMethod::Exact),
                last_resolved_at: None,
                stale_since: None,
                created_at: "2024-01-01T00:00:00Z".to_string(),
                created_by: None,
                current_resolution_id: None,
                repo_id: None,
                tags: vec![],
                annotations: vec![],
                comments: vec![],
            }],
            resolutions: vec![crate::engine::bookmark::Resolution {
                id: Uuid::new_v4().to_string(),
                bookmark_id: bm_id.clone(),
                resolved_at: "2024-01-01T00:00:00Z".to_string(),
                health: BookmarkHealth::Active,
                commit_hash: Some("abc".to_string()),
                method: ResolutionMethod::Exact,
                match_count: Some(1),
                file_path: Some("src/main.rs".to_string()),
                byte_range: Some("0-10".to_string()),
                line_range: Some("1-1".to_string()),
                content_hash: Some("hash".to_string()),
                headline: Some("main".to_string()),
                snapshot: None,
                breadcrumbs: None,
            }],
            tags: vec![crate::engine::bookmark::Tag {
                bookmark_id: bm_id.clone(),
                tag: "important".to_string(),
                added_at: "2024-01-01T00:00:00Z".to_string(),
                added_by: None,
            }],
            comments: vec![crate::engine::bookmark::BookmarkComment {
                id: Uuid::new_v4().to_string(),
                bookmark_id: bm_id.clone(),
                author: "Alice".to_string(),
                body: "Nice code".to_string(),
                created_at: "2024-01-01T00:00:00Z".to_string(),
                parent_id: None,
            }],
            collection_tags: vec![],
            collection_links: vec![],
        };

        let packer = Packer::new(&db);
        let pack_path = tmp.path().join("test.sqlite");
        let result_path = packer.create_pack_from_snapshot(&payload, &pack_path).unwrap();

        assert!(result_path.exists());

        let decompressed_path = tmp.path().join("decompressed.sqlite");
        {
            let mut decoder = zstd::stream::read::Decoder::new(std::fs::File::open(&result_path).unwrap()).unwrap();
            let mut out = std::fs::File::create(&decompressed_path).unwrap();
            std::io::copy(&mut decoder, &mut out).unwrap();
        }

        let reader = PackReader::open(&decompressed_path).unwrap();
        let tours = reader.tours().unwrap();
        assert_eq!(tours.len(), 1);

        let bookmarks = reader.bookmarks_for_collection(&col_id).unwrap();
        assert_eq!(bookmarks.len(), 1);
        assert_eq!(bookmarks[0].id, bm_id);

        let resolutions = reader.resolutions(&bm_id).unwrap();
        assert_eq!(resolutions.len(), 1);
        assert_eq!(resolutions[0].commit_hash, Some("abc".to_string()));

        // Check tags and comments via raw SQL since PackReader might not have helpers for them yet
        let tags: i64 = reader.conn().query_row("SELECT COUNT(*) FROM bookmark_tags", [], |r| r.get(0)).unwrap();
        assert_eq!(tags, 1);

        let comments: i64 = reader.conn().query_row("SELECT COUNT(*) FROM bookmark_comments", [], |r| r.get(0)).unwrap();
        assert_eq!(comments, 1);
    }
}
