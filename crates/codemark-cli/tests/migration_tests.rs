use codemark_cli::cli::{Cli, Command, PullArgs, handlers};
use codemark_core::engine::bookmark::{Bookmark, Collection, Visibility};
use codemark_core::storage::db::Database;
use codetours_server::{
    config::Config as ServerConfig,
    router::{AppState, router},
    storage::{StorageManager, registry::RegistryManager},
};
use std::sync::Arc;
use tempfile::tempdir;
use tokio::net::TcpListener;
use uuid::Uuid;

async fn start_test_server(data_dir: std::path::PathBuf) -> (String, tokio::task::JoinHandle<()>) {
    let mut config = ServerConfig::default();
    config.auth.dev_token = "test-token".to_string();
    config.data_dir = data_dir.clone();

    let storage = StorageManager::new(data_dir.clone(), config.storage.clone()).unwrap();
    let registry = RegistryManager::new(&data_dir).unwrap();
    let state = AppState {
        config: Arc::new(config),
        storage: Arc::new(storage),
        registry: Arc::new(registry),
    };

    let app = router(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);

    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (url, handle)
}

#[tokio::test]
async fn test_pull_migration_forward() {
    let server_dir = tempdir().unwrap();
    let cli_dir = tempdir().unwrap();
    let db_path = cli_dir.path().join("codemark.db");

    // 1. Start server
    let (server_url, _server_handle) = start_test_server(server_dir.path().to_path_buf()).await;

    // 2. Setup local CLI DB with a collection and publish it
    let _repo = git2::Repository::init(cli_dir.path()).unwrap();
    let db = Database::create(&db_path).unwrap();
    let col_id = Uuid::new_v4().to_string();
    let collection = Collection {
        id: col_id.clone(),
        name: "test-collection".to_string(),
        description: None,
        visibility: Visibility::Public,
        created_at: "2026-01-01T00:00:00Z".to_string(),
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
    };
    db.insert_collection(&collection).unwrap();

    let bookmark = Bookmark {
        id: Uuid::new_v4().to_string(),
        query: "fn main".to_string(),
        language: "rust".to_string(),
        file_path: "src/main.rs".to_string(),
        content_hash: None,
        commit_hash: None,
        health: codemark_core::engine::bookmark::BookmarkHealth::Active,
        resolution_method: None,
        last_resolved_at: None,
        stale_since: None,
        created_at: "2024-01-01T00:00:00Z".to_string(),
        created_by: None,
        current_resolution_id: None,
        repo_id: None,
        tags: vec![],
        annotations: vec![],
        comments: vec![],
    };
    db.insert_bookmark(&bookmark).unwrap();
    db.add_to_collection(&col_id, &[bookmark.id]).unwrap();

    let src_dir = cli_dir.path().join("src");
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::write(src_dir.join("main.rs"), "fn main() {}").unwrap();

    let publish_cli = Cli {
        db: vec![db_path.clone()],
        repo: vec![],
        format: None,
        verbose: true,
        command: Command::Publish(codemark_cli::cli::PublishArgs {
            collection: "test-collection".to_string(),
            server: Some(server_url.clone()),
            token: Some("test-token".to_string()),
            visibility: "public".to_string(),
            title: None,
            description: None,
            dry_run: false,
        }),
    };
    handlers::dispatch(&publish_cli).await.unwrap();

    // 3. Overwrite the pack on server with an OLD version (v1)
    let pack_path = server_dir.path().join("pack-cache").join("tours").join(format!("{}.sqlite", col_id));
    assert!(pack_path.exists(), "Pack should exist after publish");

    {
        let v1_pack_path = server_dir.path().join("v1.sqlite");
        let conn = rusqlite::Connection::open(&v1_pack_path).unwrap();
        conn.execute_batch("
            PRAGMA user_version = 1;
            CREATE TABLE schema_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            INSERT INTO schema_meta (key, value) VALUES ('schema_version', '1');
            CREATE TABLE collections (id TEXT PRIMARY KEY, name TEXT NOT NULL UNIQUE, description TEXT, created_at TEXT NOT NULL, created_by TEXT);
            CREATE TABLE bookmarks (id TEXT PRIMARY KEY, query TEXT NOT NULL, language TEXT NOT NULL, file_path TEXT NOT NULL, content_hash TEXT, commit_hash TEXT, created_at TEXT NOT NULL, created_by TEXT, current_resolution_id TEXT, tags TEXT, notes TEXT, context TEXT);
            CREATE TABLE collection_bookmarks (collection_id TEXT NOT NULL REFERENCES collections(id) ON DELETE CASCADE, bookmark_id TEXT NOT NULL REFERENCES bookmarks(id) ON DELETE CASCADE, added_at TEXT NOT NULL, PRIMARY KEY (collection_id, bookmark_id));
            CREATE TABLE resolutions (id TEXT PRIMARY KEY, bookmark_id TEXT NOT NULL REFERENCES bookmarks(id) ON DELETE CASCADE, resolved_at TEXT NOT NULL, health TEXT NOT NULL, commit_hash TEXT, method TEXT NOT NULL, match_count INTEGER, file_path TEXT, byte_range TEXT, content_hash TEXT);
            CREATE TABLE _pack_meta (pack_id TEXT PRIMARY KEY, protocol_version INTEGER, purpose TEXT, source_client TEXT, generated_at TEXT, notes TEXT);
            
            INSERT INTO collections (id, name, created_at) VALUES ('COL_V1', 'Old Tour', '2024-01-01T00:00:00Z');
            INSERT INTO bookmarks (id, query, language, file_path, created_at) VALUES ('BM1', 'fn main', 'rust', 'src/main.rs', '2024-01-01T00:00:00Z');
            INSERT INTO collection_bookmarks (collection_id, bookmark_id, added_at) VALUES ('COL_V1', 'BM1', '2024-01-01T00:00:00Z');
            INSERT INTO resolutions (id, bookmark_id, resolved_at, health, method) VALUES ('RES1', 'BM1', '2024-01-01T00:00:00Z', 'active', 'exact');
            INSERT INTO _pack_meta (pack_id, protocol_version, purpose, source_client, generated_at) VALUES ('P1', 1, 'publish', 'test', '2024-01-01T00:00:00Z');
        ").unwrap();
        drop(conn);

        let mut encoder = zstd::stream::write::Encoder::new(std::fs::File::create(&pack_path).unwrap(), 0).unwrap();
        let mut source = std::fs::File::open(&v1_pack_path).unwrap();
        std::io::copy(&mut source, &mut encoder).unwrap();
        encoder.finish().unwrap();
    }

    // 4. Run Pull
    let pull_cli = Cli {
        db: vec![db_path.clone()],
        repo: vec![],
        format: None,
        verbose: true,
        command: Command::Pull(PullArgs {
            tour: format!("{}/tours/{}", server_url, col_id),
            server: None,
            token: Some("test-token".to_string()),
            save: Some("migrated-collection".to_string()),
        }),
    };

    handlers::dispatch(&pull_cli).await.unwrap();

    // 5. Verify migrated collection in local DB
    let db = Database::open(&db_path).unwrap();
    let col = db.get_collection_by_name("migrated-collection").unwrap().expect("migrated collection not found");
    assert_eq!(col.name, "migrated-collection");
    assert!(col.imported_from_url.is_some());
    assert_eq!(col.visibility, Visibility::Private); // Migrated default from V10
}
