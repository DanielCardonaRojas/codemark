use codemark_cli::cli::{Cli, Command, PublishArgs, PullArgs, handlers};
use codemark_core::engine::bookmark::{Bookmark, Collection, Visibility};
use codemark_core::storage::db::Database;
use codetours_server::{
    config::Config as ServerConfig,
    router::{AppState, router},
    storage::StorageManager,
};
use std::sync::Arc;
use tempfile::tempdir;
use tokio::net::TcpListener;
use uuid::Uuid;

async fn start_test_server(data_dir: std::path::PathBuf) -> (String, tokio::task::JoinHandle<()>) {
    let mut config = ServerConfig::default();
    config.auth.dev_token = "test-token".to_string();
    config.data_dir = data_dir.clone();

    let storage = StorageManager::new(data_dir, config.storage.clone()).unwrap();
    let state = AppState { config: Arc::new(config), storage: Arc::new(storage) };

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
async fn test_cli_publish_pull_roundtrip() {
    let server_dir = tempdir().unwrap();
    let cli_dir = tempdir().unwrap();
    let db_path = cli_dir.path().join("codemark.db");

    // 1. Start server
    let (server_url, _server_handle) = start_test_server(server_dir.path().to_path_buf()).await;

    // 2. Setup local CLI DB with a collection
    // Initialize a git repo to help with path resolution
    let _repo = git2::Repository::init(cli_dir.path()).unwrap();

    let db = Database::create(&db_path).unwrap();
    let col_id = Uuid::new_v4().to_string();
    let collection = Collection {
        id: col_id.clone(),
        name: "test-collection".to_string(),
        description: Some("Test description".to_string()),
        visibility: Visibility::Public,
        created_at: "2026-01-01T00:00:00Z".to_string(),
        created_by: None,
        created_branch: None,
        published_at: None,
        published_commit_sha: None,
        repo_url: None,
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
        tags: vec![],
        annotations: vec![],
        comments: vec![],
    };
    db.insert_bookmark(&bookmark).unwrap();
    db.add_to_collection(&col_id, &[bookmark.id]).unwrap();

    // Create src/main.rs for resolution
    let src_dir = cli_dir.path().join("src");
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::write(src_dir.join("main.rs"), "fn main() {\n    println!(\"Hello\");\n}\n").unwrap();

    // 3. Run Publish
    let cli = Cli {
        db: vec![db_path.clone()],
        format: None,
        verbose: true,
        command: Command::Publish(PublishArgs {
            collection: "test-collection".to_string(),
            server: Some(server_url.clone()),
            token: Some("test-token".to_string()),
            visibility: "public".to_string(),
            title: None,
            description: None,
            dry_run: false,
        }),
    };

    handlers::dispatch(&cli).await.unwrap();

    // 4. Verify publication in server (via tour list)
    // We'll use the CLI to list tours
    let list_cli = Cli {
        db: vec![db_path.clone()],
        format: None,
        verbose: true,
        command: Command::Tour(codemark_cli::cli::TourArgs {
            command: codemark_cli::cli::TourCommand::List(codemark_cli::cli::TourListArgs {
                server: Some(server_url.clone()),
                repo: None,
                limit: 10,
                offset: 0,
                line_format: None,
            }),
        }),
    };
    handlers::dispatch(&list_cli).await.unwrap();

    // 5. Run Pull
    let pull_cli = Cli {
        db: vec![db_path.clone()],
        format: None,
        verbose: true,
        command: Command::Pull(PullArgs {
            tour: format!("{}/tours/{}", server_url, col_id),
            server: None,
            token: Some("test-token".to_string()),
            save: Some("pulled-collection".to_string()),
        }),
    };
    handlers::dispatch(&pull_cli).await.unwrap();

    // 6. Verify pulled collection
    let pulled_col = db
        .get_collection_by_name("pulled-collection")
        .unwrap()
        .expect("pulled collection not found");
    assert!(pulled_col.imported_from_url.is_some());

    let bookmarks = db.list_bookmarks_in_collection(&pulled_col.id).unwrap();
    assert_eq!(bookmarks.len(), 1);
    assert_eq!(bookmarks[0].query, "fn main");
}
