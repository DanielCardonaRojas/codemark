use axum::{
    body::Body,
    http::{header, Request, StatusCode},
};
use codetours_server::{
    config::Config,
    router::{AppState, router},
    storage::StorageManager,
};
use serde_json::Value;
use std::sync::Arc;
use tempfile::tempdir;
use tower::ServiceExt;
use uuid::Uuid;

const DEV_TOKEN: &str = "dev-secret-not-for-prod";

async fn setup_app() -> (axum::Router, tempfile::TempDir) {
    let mut config = Config::default();
    config.auth.dev_token = DEV_TOKEN.to_string();
    let temp_data = tempdir().unwrap();
    let storage =
        StorageManager::new(temp_data.path().to_path_buf(), config.storage.clone()).unwrap();

    let state = AppState { config: Arc::new(config), storage: Arc::new(storage) };
    (router(state), temp_data)
}

/// Tests the full publish, list, get, and delete flow.
#[tokio::test]
async fn test_publish_list_get_delete_flow() {
    let (app, _tmp) = setup_app().await;
    let collection_id = Uuid::new_v4().to_string();

    // 1. Create a dummy pack file
    let pack_path = _tmp.path().join("test.pack.sqlite");
    {
        let conn = rusqlite::Connection::open(&pack_path).unwrap();
        let sql = "PRAGMA user_version = 12;
             CREATE TABLE collections (id TEXT PRIMARY KEY, name TEXT, visibility TEXT, created_at TEXT, description TEXT, repo_url TEXT, created_branch TEXT, published_commit_sha TEXT, status TEXT, published_at TEXT, updated_at TEXT, created_by TEXT);
             CREATE TABLE bookmarks (id TEXT PRIMARY KEY, file_path TEXT, query TEXT, language TEXT, created_at TEXT, content_hash TEXT, commit_hash TEXT, status TEXT, resolution_method TEXT, last_resolved_at TEXT, stale_since TEXT, created_by TEXT);
             CREATE TABLE collection_bookmarks (collection_id TEXT, bookmark_id TEXT, position INTEGER, added_at TEXT);
             CREATE TABLE resolutions (id TEXT PRIMARY KEY, bookmark_id TEXT, resolved_at TEXT, method TEXT, headline TEXT, preview_lines TEXT, commit_hash TEXT, match_count INTEGER, file_path TEXT, byte_range TEXT, line_range TEXT, content_hash TEXT);
             CREATE TABLE bookmark_annotations (id TEXT PRIMARY KEY, bookmark_id TEXT, added_at TEXT, added_by TEXT, notes TEXT, context TEXT, source TEXT);
             CREATE TABLE bookmark_tags (bookmark_id TEXT, tag TEXT, added_at TEXT, added_by TEXT);
             CREATE TABLE _pack_meta (pack_id TEXT PRIMARY KEY, protocol_version INTEGER, purpose TEXT, source_client TEXT, generated_at TEXT, notes TEXT);
             
             INSERT INTO collections (id, name, visibility, created_at, status, published_at, updated_at) VALUES ('COL_ID', 'Test Tour', 'public', '2026-05-01T00:00:00Z', 'ready', '2026-05-01T00:00:00Z', '2026-05-01T00:00:00Z');
             INSERT INTO bookmarks (id, file_path, query, language, created_at, status) VALUES ('BM_FLOW_1', 'src/main.rs', 'query', 'rust', '2026-05-01T00:00:00Z', 'active');
             INSERT INTO collection_bookmarks (collection_id, bookmark_id, position, added_at) VALUES ('COL_ID', 'BM_FLOW_1', 0, '2026-05-01T00:00:00Z');
             INSERT INTO resolutions (id, bookmark_id, resolved_at, method, headline, preview_lines, line_range) VALUES ('RES_FLOW_1', 'BM_FLOW_1', '2026-05-01T00:00:00Z', 'exact', 'headline', 'preview', '10');
             INSERT INTO _pack_meta (pack_id, protocol_version, purpose, source_client, generated_at) VALUES ('PACK_FLOW_1', 12, 'publish', 'test-client', '2026-05-01T00:00:00Z');"
             .replace("COL_ID", &collection_id);
        conn.execute_batch(&sql).unwrap();
    }

    let pack_bytes = std::fs::read(&pack_path).unwrap();

    // 2. POST /tours (Publish)
    let req = Request::builder()
        .method("POST")
        .uri("/tours")
        .header("X-Tour-Token", DEV_TOKEN)
        .header(header::CONTENT_TYPE, "application/vnd.codetours.pack+sqlite")
        .body(Body::from(pack_bytes))
        .unwrap();

    let response = app.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let body = ax_body_to_json(response).await;
    
    assert_eq!(status, StatusCode::CREATED, "Response body: {:?}", body);
    assert_eq!(body["tour_id"], collection_id);

    // 3. GET /tours (List)
    let req = Request::builder()
        .method("GET")
        .uri("/tours")
        .body(Body::empty())
        .unwrap();

    let response = app.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let body = ax_body_to_json(response).await;

    assert_eq!(status, StatusCode::OK);
    assert!(body["tours"].as_array().unwrap().len() >= 1);
    assert_eq!(body["tours"][0]["tour_id"], collection_id);

    // 4. GET /tours/:id (Detail JSON)
    let req = Request::builder()
        .method("GET")
        .uri(format!("/tours/{}", collection_id))
        .body(Body::empty())
        .unwrap();

    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    
    let body = ax_body_to_json(response).await;
    assert_eq!(body["tour_id"], collection_id);
    assert_eq!(body["bookmarks"].as_array().unwrap().len(), 1);
    assert_eq!(body["bookmarks"][0]["snapshot"]["headline"], "headline");

    // 5. GET /tours/:id (Pack binary)
    let req = Request::builder()
        .method("GET")
        .uri(format!("/tours/{}?format=pack", collection_id))
        .header(header::ACCEPT, "application/vnd.codetours.pack+sqlite")
        .body(Body::empty())
        .unwrap();

    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers().get(header::CONTENT_TYPE).unwrap(), "application/vnd.codetours.pack+sqlite");

    // 6. Test Migration (Upload a pack with user_version = 1)
    let migration_collection_id = Uuid::new_v4().to_string();
    let migration_pack_path = _tmp.path().join("migration.pack.sqlite");
    {
        let conn = rusqlite::Connection::open(&migration_pack_path).unwrap();
        let sql = "PRAGMA user_version = 1;
             CREATE TABLE schema_meta (key TEXT PRIMARY KEY, value TEXT);
             INSERT INTO schema_meta (key, value) VALUES ('schema_version', '1');

             CREATE TABLE collections (id TEXT PRIMARY KEY, name TEXT NOT NULL UNIQUE, description TEXT, created_at TEXT NOT NULL, created_by TEXT);
             CREATE TABLE bookmarks (id TEXT PRIMARY KEY, query TEXT NOT NULL, language TEXT NOT NULL, file_path TEXT NOT NULL, content_hash TEXT, commit_hash TEXT, status TEXT NOT NULL DEFAULT 'active', resolution_method TEXT, last_resolved_at TEXT, stale_since TEXT, created_at TEXT NOT NULL, created_by TEXT, tags TEXT, notes TEXT, context TEXT);
             CREATE TABLE collection_bookmarks (collection_id TEXT NOT NULL REFERENCES collections(id) ON DELETE CASCADE, bookmark_id TEXT NOT NULL REFERENCES bookmarks(id) ON DELETE CASCADE, added_at TEXT NOT NULL, PRIMARY KEY (collection_id, bookmark_id));
             CREATE TABLE resolutions (id TEXT PRIMARY KEY, bookmark_id TEXT NOT NULL REFERENCES bookmarks(id) ON DELETE CASCADE, resolved_at TEXT NOT NULL, commit_hash TEXT, method TEXT NOT NULL, match_count INTEGER, file_path TEXT, byte_range TEXT, content_hash TEXT);
             CREATE TABLE _pack_meta (pack_id TEXT PRIMARY KEY, protocol_version INTEGER, purpose TEXT, source_client TEXT, generated_at TEXT, notes TEXT);
             
             INSERT INTO collections (id, name, created_at) VALUES ('COL_ID', 'Old Tour', '2026-05-01T00:00:00Z');
             INSERT INTO bookmarks (id, query, language, file_path, created_at) VALUES ('BM_MIG_1', 'query_mig', 'rust', 'src/mig.rs', '2026-05-01T00:00:00Z');
             INSERT INTO collection_bookmarks (collection_id, bookmark_id, added_at) VALUES ('COL_ID', 'BM_MIG_1', '2026-05-01T00:00:00Z');
             INSERT INTO resolutions (id, bookmark_id, resolved_at, method) VALUES ('RES_MIG_1', 'BM_MIG_1', '2026-05-01T00:00:00Z', 'exact');
             INSERT INTO _pack_meta (pack_id, protocol_version, purpose, source_client, generated_at) VALUES ('PACK_MIG_1', 1, 'publish', 'test-client', '2026-05-01T00:00:00Z');"
             .replace("COL_ID", &migration_collection_id);
        conn.execute_batch(&sql).unwrap();
    }
    let migration_pack_bytes = std::fs::read(&migration_pack_path).unwrap();
    let req = Request::builder()
        .method("POST")
        .uri("/tours")
        .header("X-Tour-Token", DEV_TOKEN)
        .header(header::CONTENT_TYPE, "application/vnd.codetours.pack+sqlite")
        .body(Body::from(migration_pack_bytes))
        .unwrap();

    let response = app.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let body = ax_body_to_json(response).await;
    assert_eq!(status, StatusCode::CREATED, "Migration response body: {:?}", body);

    // 7. DELETE /tours/:id (Delete)
    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/tours/{}", collection_id))
        .header("X-Tour-Token", DEV_TOKEN)
        .body(Body::empty())
        .unwrap();

    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // 8. Verify deletion
    let req = Request::builder()
        .method("GET")
        .uri(format!("/tours/{}", collection_id))
        .body(Body::empty())
        .unwrap();

    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// Tests that publishing without a token is rejected.
#[tokio::test]
async fn test_publish_unauthorized() {
    let (app, _tmp) = setup_app().await;
    let req = Request::builder()
        .method("POST")
        .uri("/tours")
        .header("X-Tour-Token", "wrong-token")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// Tests that a malicious pack with a view is rejected.
#[tokio::test]
async fn test_publish_malicious_pack() {
    let (app, _tmp) = setup_app().await;
    
    // Create pack with a view (disallowed)
    let pack_path = _tmp.path().join("malicious.pack.sqlite");
    {
        let conn = rusqlite::Connection::open(&pack_path).unwrap();
        conn.execute_batch(
            "PRAGMA user_version = 12;
             CREATE TABLE collections (id TEXT PRIMARY KEY, name TEXT, visibility TEXT);
             CREATE VIEW my_view AS SELECT * FROM collections;
             CREATE TABLE _pack_meta (pack_id TEXT PRIMARY KEY, protocol_version INTEGER, purpose TEXT, source_client TEXT, generated_at TEXT);
             INSERT INTO _pack_meta VALUES ('P_MAL', 12, 'publish', 'C', 'T');"
        ).unwrap();
    }
    
    let pack_bytes = std::fs::read(&pack_path).unwrap();
    let req = Request::builder()
        .method("POST")
        .uri("/tours")
        .header("X-Tour-Token", DEV_TOKEN)
        .header(header::CONTENT_TYPE, "application/vnd.codetours.pack+sqlite")
        .body(Body::from(pack_bytes))
        .unwrap();

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    
    let body = ax_body_to_json(response).await;
    assert_eq!(body["error"], "disallowed_schema_item");
}

/// Tests that a pack exceeding the size limit is rejected.
#[tokio::test]
async fn test_publish_too_large() {
    let (app, _tmp) = setup_app().await;
    
    // 6MB payload (limit is 5MB)
    let large_body = vec![0u8; 6 * 1024 * 1024];
    
    let req = Request::builder()
        .method("POST")
        .uri("/tours")
        .header("X-Tour-Token", DEV_TOKEN)
        .body(Body::from(large_body))
        .unwrap();

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

async fn ax_body_to_json(response: axum::response::Response) -> Value {
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}
