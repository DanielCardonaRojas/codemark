use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use codetours_server::{
    config::Config,
    github::GitHubVerifier,
    router::{AppState, router},
    storage::{StorageManager, registry::RegistryManager},
};
use serde_json::Value;
use std::sync::Arc;
use tempfile::tempdir;
use tower::ServiceExt;
use uuid::Uuid;

const DEV_TOKEN: &str = "dev-secret-not-for-prod";

async fn setup_app() -> (axum::Router, Arc<GitHubVerifier>, tempfile::TempDir) {
    let (router, _registry, github, temp_data) = setup_app_with_registry().await;
    (router, github, temp_data)
}

/// Like [`setup_app`] but also returns the registry so tests can seed users.
///
/// Returns the shared [`GitHubVerifier`] too: tests seed its caches (public-repo
/// visibility, per-user read/write access) so the handlers never make a live
/// GitHub call.
async fn setup_app_with_registry()
-> (axum::Router, Arc<RegistryManager>, Arc<GitHubVerifier>, tempfile::TempDir) {
    let mut config = Config::default();
    config.auth.dev_token = DEV_TOKEN.to_string();
    let temp_data = tempdir().unwrap();
    let storage =
        StorageManager::new(temp_data.path().to_path_buf(), config.storage.clone()).unwrap();
    let registry = Arc::new(RegistryManager::new(temp_data.path()).unwrap());
    let github = Arc::new(GitHubVerifier::new());

    let state = AppState {
        config: Arc::new(config),
        storage: Arc::new(storage),
        registry: registry.clone(),
        github: github.clone(),
    };
    (router(state), registry, github, temp_data)
}

/// Tests the full publish, list, get, and delete flow.
#[tokio::test]
async fn test_publish_list_get_delete_flow() {
    let (app, github, _tmp) = setup_app().await;
    let collection_id = Uuid::new_v4().to_string();

    // The tour names a repo; seed the verifier so publish (write check) and the
    // anonymous list (public-repo check) don't hit the network.
    github.seed_write_access("stub", "octo", "demo", true).await;
    github.seed_public_repo("octo", "demo", true).await;

    // 1. Create a dummy pack file
    let pack_path = _tmp.path().join("test.pack.sqlite");
    {
        let conn = rusqlite::Connection::open(&pack_path).unwrap();
        let sql = "PRAGMA user_version = 20;
             CREATE TABLE schema_meta (key TEXT PRIMARY KEY, value TEXT);
             INSERT INTO schema_meta (key, value) VALUES ('schema_version', '20');
             CREATE TABLE collections (id TEXT PRIMARY KEY, name TEXT, visibility TEXT, created_at TEXT, description TEXT, repo_url TEXT, created_branch TEXT, published_commit_sha TEXT, status TEXT, health TEXT, health_computed_at TEXT, published_at TEXT, updated_at TEXT, created_by TEXT);
             CREATE TABLE bookmarks (id TEXT PRIMARY KEY, file_path TEXT, query TEXT, language TEXT, created_at TEXT, content_hash TEXT, commit_hash TEXT, created_by TEXT, current_resolution_id TEXT);
             CREATE TABLE collection_bookmarks (collection_id TEXT, bookmark_id TEXT, position INTEGER, added_at TEXT);
             CREATE TABLE collection_tags (collection_id TEXT, tag TEXT, added_at TEXT, added_by TEXT);
             CREATE TABLE collection_links (id TEXT PRIMARY KEY, collection_id TEXT, kind TEXT, label TEXT, url TEXT, sort_order INTEGER, added_at TEXT, added_by TEXT);
             CREATE TABLE resolutions (id TEXT PRIMARY KEY, bookmark_id TEXT, resolved_at TEXT, health TEXT, method TEXT, headline TEXT, snapshot TEXT, commit_hash TEXT, match_count INTEGER, file_path TEXT, byte_range TEXT, line_range TEXT, content_hash TEXT, breadcrumbs TEXT, snapshot_top_padding INTEGER, snapshot_bottom_padding INTEGER);
             CREATE TABLE bookmark_annotations (id TEXT PRIMARY KEY, bookmark_id TEXT, added_at TEXT, added_by TEXT, notes TEXT, context TEXT, source TEXT);
             CREATE TABLE bookmark_tags (bookmark_id TEXT, tag TEXT, added_at TEXT, added_by TEXT);
             CREATE TABLE _pack_meta (pack_id TEXT PRIMARY KEY, protocol_version INTEGER, purpose TEXT, source_client TEXT, generated_at TEXT, notes TEXT);
             
             INSERT INTO collections (id, name, visibility, created_at, created_by, repo_url, status, health, published_at, updated_at) VALUES ('COL_ID', 'Test Tour', 'public', '2026-05-01T00:00:00Z', 'pack-author', 'git@github.com:octo/demo.git', 'ready', 'active', '2026-05-01T00:00:00Z', '2026-05-01T00:00:00Z');
             INSERT INTO bookmarks (id, file_path, query, language, created_at, current_resolution_id) VALUES ('BM_FLOW_1', 'src/main.rs', 'query', 'rust', '2026-05-01T00:00:00Z', 'RES_FLOW_1');
             -- Note: collections.created_by above is seeded but should NOT surface as
             -- the author, since the publisher (stub auth) is not in the registry.
             INSERT INTO collection_bookmarks (collection_id, bookmark_id, position, added_at) VALUES ('COL_ID', 'BM_FLOW_1', 0, '2026-05-01T00:00:00Z');
             INSERT INTO resolutions (id, bookmark_id, resolved_at, health, method, headline, line_range, snapshot, breadcrumbs) VALUES ('RES_FLOW_1', 'BM_FLOW_1', '2026-05-01T00:00:00Z', 'active', 'exact', 'headline', '10', 'snapshot_content', '[{\"line\": 1, \"text\": \"mod auth {\"}]');
             INSERT INTO _pack_meta (pack_id, protocol_version, purpose, source_client, generated_at) VALUES ('PACK_FLOW_1', 20, 'publish', 'test-client', '2026-05-01T00:00:00Z');"
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

    // 3. GET /tours (authorization-scoped list — repos is required)
    let req =
        Request::builder().method("GET").uri("/tours?repos=octo/demo").body(Body::empty()).unwrap();

    let response = app.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let body = ax_body_to_json(response).await;

    assert_eq!(status, StatusCode::OK);
    assert!(!body["tours"].as_array().unwrap().is_empty());
    assert_eq!(body["tours"][0]["tour_id"], collection_id);
    // The full repo_url is retained in the scoped list.
    assert_eq!(body["tours"][0]["repo_url"], "git@github.com:octo/demo.git");
    // The publisher (stub auth) is not a registered user, so the author cannot be
    // verified. We must NOT surface the pack's client-controlled created_by; the
    // author is null instead. (Happy-path resolution is covered by a dedicated test.)
    assert_eq!(body["tours"][0]["author"], Value::Null);

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
    assert_eq!(body["bookmarks"][0]["snapshot"]["snapshot"], "snapshot_content");
    assert_eq!(body["bookmarks"][0]["snapshot"]["sticky_lines"][0], "mod auth {");
    assert_eq!(body["bookmarks"][0]["snapshot"]["sticky_line_numbers"][0], 1);

    // 5. GET /tours/:id (Pack binary)
    let req = Request::builder()
        .method("GET")
        .uri(format!("/tours/{}?format=pack", collection_id))
        .header(header::ACCEPT, "application/vnd.codetours.pack+sqlite")
        .body(Body::empty())
        .unwrap();

    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/vnd.codetours.pack+sqlite"
    );

    // 6. Test Migration (Upload a pack with user_version = 1)
    let migration_collection_id = Uuid::new_v4().to_string();
    let migration_pack_path = _tmp.path().join("migration.pack.sqlite");
    {
        let conn = rusqlite::Connection::open(&migration_pack_path).unwrap();
        let sql = "PRAGMA user_version = 1;
             CREATE TABLE schema_meta (key TEXT PRIMARY KEY, value TEXT);
             INSERT INTO schema_meta (key, value) VALUES ('schema_version', '1');

             CREATE TABLE collections (id TEXT PRIMARY KEY, name TEXT NOT NULL UNIQUE, description TEXT, created_at TEXT NOT NULL, created_by TEXT);
             CREATE TABLE bookmarks (id TEXT PRIMARY KEY, query TEXT NOT NULL, language TEXT NOT NULL, file_path TEXT NOT NULL, content_hash TEXT, commit_hash TEXT, current_resolution_id TEXT, created_at TEXT NOT NULL, created_by TEXT, tags TEXT, notes TEXT, context TEXT);
             CREATE TABLE collection_bookmarks (collection_id TEXT NOT NULL REFERENCES collections(id) ON DELETE CASCADE, bookmark_id TEXT NOT NULL REFERENCES bookmarks(id) ON DELETE CASCADE, added_at TEXT NOT NULL, PRIMARY KEY (collection_id, bookmark_id));
             CREATE TABLE resolutions (id TEXT PRIMARY KEY, bookmark_id TEXT NOT NULL REFERENCES bookmarks(id) ON DELETE CASCADE, resolved_at TEXT NOT NULL, health TEXT, commit_hash TEXT, method TEXT NOT NULL, match_count INTEGER, file_path TEXT, byte_range TEXT, content_hash TEXT);
             CREATE TABLE _pack_meta (pack_id TEXT PRIMARY KEY, protocol_version INTEGER, purpose TEXT, source_client TEXT, generated_at TEXT, notes TEXT);
             
             INSERT INTO collections (id, name, created_at) VALUES ('COL_ID', 'Old Tour', '2026-05-01T00:00:00Z');
             INSERT INTO bookmarks (id, query, language, file_path, created_at, current_resolution_id) VALUES ('BM_MIG_1', 'query_mig', 'rust', 'src/mig.rs', '2026-05-01T00:00:00Z', 'RES_MIG_1');
             INSERT INTO collection_bookmarks (collection_id, bookmark_id, added_at) VALUES ('COL_ID', 'BM_MIG_1', '2026-05-01T00:00:00Z');
             INSERT INTO resolutions (id, bookmark_id, resolved_at, health, method) VALUES ('RES_MIG_1', 'BM_MIG_1', '2026-05-01T00:00:00Z', 'active', 'exact');
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

/// Tests that the author surfaced in tour listings is the publisher's verified
/// GitHub login resolved from the registry — overriding the pack's own
/// (client-controlled) created_by, not falling back to it.
#[tokio::test]
async fn test_publish_resolves_author_from_registry() {
    let (app, registry, github, _tmp) = setup_app_with_registry().await;
    let collection_id = Uuid::new_v4().to_string();

    github.seed_write_access("stub", "octo", "demo", true).await;
    github.seed_public_repo("octo", "demo", true).await;

    // Seed the registry with the user the stub auth resolves to ("stub"), giving
    // it a known GitHub login that should become the verified author.
    {
        let conn = registry.get_conn().await.unwrap();
        conn.interact(|conn| {
            codetours_server::storage::registry::upsert_user(
                conn,
                &codetours_server::storage::registry::UserUpsert {
                    id: "stub",
                    github_id: "999",
                    github_login: "verified-octocat",
                    github_token: None,
                },
            )
        })
        .await
        .unwrap()
        .unwrap();
    }

    // Build a pack whose collection carries a *different* created_by, to prove the
    // server-resolved login wins over the client-supplied value.
    let pack_path = _tmp.path().join("author.pack.sqlite");
    {
        let conn = rusqlite::Connection::open(&pack_path).unwrap();
        let sql = "PRAGMA user_version = 20;
             CREATE TABLE schema_meta (key TEXT PRIMARY KEY, value TEXT);
             INSERT INTO schema_meta (key, value) VALUES ('schema_version', '20');
             CREATE TABLE collections (id TEXT PRIMARY KEY, name TEXT, visibility TEXT, created_at TEXT, description TEXT, repo_url TEXT, created_branch TEXT, published_commit_sha TEXT, status TEXT, health TEXT, health_computed_at TEXT, published_at TEXT, updated_at TEXT, created_by TEXT);
             CREATE TABLE bookmarks (id TEXT PRIMARY KEY, file_path TEXT, query TEXT, language TEXT, created_at TEXT, content_hash TEXT, commit_hash TEXT, created_by TEXT, current_resolution_id TEXT);
             CREATE TABLE collection_bookmarks (collection_id TEXT, bookmark_id TEXT, position INTEGER, added_at TEXT);
             CREATE TABLE collection_tags (collection_id TEXT, tag TEXT, added_at TEXT, added_by TEXT);
             CREATE TABLE collection_links (id TEXT PRIMARY KEY, collection_id TEXT, kind TEXT, label TEXT, url TEXT, sort_order INTEGER, added_at TEXT, added_by TEXT);
             CREATE TABLE resolutions (id TEXT PRIMARY KEY, bookmark_id TEXT, resolved_at TEXT, health TEXT, method TEXT, headline TEXT, snapshot TEXT, commit_hash TEXT, match_count INTEGER, file_path TEXT, byte_range TEXT, line_range TEXT, content_hash TEXT, breadcrumbs TEXT, snapshot_top_padding INTEGER, snapshot_bottom_padding INTEGER);
             CREATE TABLE bookmark_annotations (id TEXT PRIMARY KEY, bookmark_id TEXT, added_at TEXT, added_by TEXT, notes TEXT, context TEXT, source TEXT);
             CREATE TABLE bookmark_tags (bookmark_id TEXT, tag TEXT, added_at TEXT, added_by TEXT);
             CREATE TABLE _pack_meta (pack_id TEXT PRIMARY KEY, protocol_version INTEGER, purpose TEXT, source_client TEXT, generated_at TEXT, notes TEXT);

             INSERT INTO collections (id, name, visibility, created_at, created_by, repo_url, status, health, published_at, updated_at) VALUES ('COL_ID', 'Authored Tour', 'public', '2026-05-01T00:00:00Z', 'spoofed-author', 'git@github.com:octo/demo.git', 'ready', 'active', '2026-05-01T00:00:00Z', '2026-05-01T00:00:00Z');
             INSERT INTO bookmarks (id, file_path, query, language, created_at, current_resolution_id) VALUES ('BM_AUTH_1', 'src/main.rs', 'query', 'rust', '2026-05-01T00:00:00Z', 'RES_AUTH_1');
             INSERT INTO collection_bookmarks (collection_id, bookmark_id, position, added_at) VALUES ('COL_ID', 'BM_AUTH_1', 0, '2026-05-01T00:00:00Z');
             INSERT INTO resolutions (id, bookmark_id, resolved_at, health, method, headline, line_range, snapshot, breadcrumbs) VALUES ('RES_AUTH_1', 'BM_AUTH_1', '2026-05-01T00:00:00Z', 'active', 'exact', 'headline', '10', 'snapshot', '[]');
             INSERT INTO _pack_meta (pack_id, protocol_version, purpose, source_client, generated_at) VALUES ('PACK_AUTH_1', 20, 'publish', 'test-client', '2026-05-01T00:00:00Z');"
             .replace("COL_ID", &collection_id);
        conn.execute_batch(&sql).unwrap();
    }
    let pack_bytes = std::fs::read(&pack_path).unwrap();

    // Publish (stub auth resolves to user "stub", seeded above).
    let req = Request::builder()
        .method("POST")
        .uri("/tours")
        .header("X-Tour-Token", DEV_TOKEN)
        .header(header::CONTENT_TYPE, "application/vnd.codetours.pack+sqlite")
        .body(Body::from(pack_bytes))
        .unwrap();
    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    // List and confirm the verified login is the author, not the pack's value.
    let req =
        Request::builder().method("GET").uri("/tours?repos=octo/demo").body(Body::empty()).unwrap();
    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = ax_body_to_json(response).await;
    let tour = body["tours"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["tour_id"] == collection_id)
        .expect("published tour should be listed");
    assert_eq!(tour["author"], "verified-octocat");
}

/// Tests that publishing without a token is rejected.
#[tokio::test]
async fn test_publish_unauthorized() {
    let (app, _github, _tmp) = setup_app().await;
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
    let (app, _github, _tmp) = setup_app().await;

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
    let (app, _github, _tmp) = setup_app().await;

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

/// Tests that republishing a tour with collection_tags does not fail with a UNIQUE constraint error.
#[tokio::test]
async fn test_republish_with_collection_tags() {
    let (app, _github, _tmp) = setup_app().await;
    let collection_id = Uuid::new_v4().to_string();

    let make_pack = |col_id: &str, bookmark_suffix: &str| {
        let pack_path = _tmp.path().join(format!("republish_{}.pack.sqlite", bookmark_suffix));
        let conn = rusqlite::Connection::open(&pack_path).unwrap();
        let sql = format!(
            "PRAGMA user_version = 20;
             CREATE TABLE schema_meta (key TEXT PRIMARY KEY, value TEXT);
             INSERT INTO schema_meta (key, value) VALUES ('schema_version', '20');
             CREATE TABLE collections (id TEXT PRIMARY KEY, name TEXT, visibility TEXT, created_at TEXT, description TEXT, repo_url TEXT, created_branch TEXT, published_commit_sha TEXT, status TEXT, health TEXT, health_computed_at TEXT, published_at TEXT, updated_at TEXT, created_by TEXT);
             CREATE TABLE bookmarks (id TEXT PRIMARY KEY, file_path TEXT, query TEXT, language TEXT, created_at TEXT, content_hash TEXT, commit_hash TEXT, created_by TEXT, current_resolution_id TEXT);
             CREATE TABLE collection_bookmarks (collection_id TEXT, bookmark_id TEXT, position INTEGER, added_at TEXT);
             CREATE TABLE collection_tags (collection_id TEXT, tag TEXT, added_at TEXT, added_by TEXT);
             CREATE TABLE collection_links (id TEXT PRIMARY KEY, collection_id TEXT, kind TEXT, label TEXT, url TEXT, sort_order INTEGER, added_at TEXT, added_by TEXT);
             CREATE TABLE resolutions (id TEXT PRIMARY KEY, bookmark_id TEXT, resolved_at TEXT, health TEXT, method TEXT, headline TEXT, snapshot TEXT, commit_hash TEXT, match_count INTEGER, file_path TEXT, byte_range TEXT, line_range TEXT, content_hash TEXT, breadcrumbs TEXT, snapshot_top_padding INTEGER, snapshot_bottom_padding INTEGER);
             CREATE TABLE bookmark_annotations (id TEXT PRIMARY KEY, bookmark_id TEXT, added_at TEXT, added_by TEXT, notes TEXT, context TEXT, source TEXT);
             CREATE TABLE bookmark_tags (bookmark_id TEXT, tag TEXT, added_at TEXT, added_by TEXT);
             CREATE TABLE _pack_meta (pack_id TEXT PRIMARY KEY, protocol_version INTEGER, purpose TEXT, source_client TEXT, generated_at TEXT, notes TEXT);

             INSERT INTO collections (id, name, visibility, created_at, status, health, published_at, updated_at) VALUES ('{col_id}', 'Tagged Tour', 'public', '2026-05-01T00:00:00Z', 'ready', 'active', '2026-05-01T00:00:00Z', '2026-05-01T00:00:00Z');
             INSERT INTO bookmarks (id, file_path, query, language, created_at, current_resolution_id) VALUES ('BM_{bookmark_suffix}', 'src/main.rs', 'query', 'rust', '2026-05-01T00:00:00Z', 'RES_{bookmark_suffix}');
             INSERT INTO collection_bookmarks (collection_id, bookmark_id, position, added_at) VALUES ('{col_id}', 'BM_{bookmark_suffix}', 0, '2026-05-01T00:00:00Z');
             INSERT INTO collection_tags (collection_id, tag, added_at, added_by) VALUES ('{col_id}', 'rust', '2026-05-01T00:00:00Z', 'test');
             INSERT INTO collection_tags (collection_id, tag, added_at, added_by) VALUES ('{col_id}', 'tutorial', '2026-05-01T00:00:00Z', 'test');
             INSERT INTO resolutions (id, bookmark_id, resolved_at, health, method, headline, line_range, snapshot, breadcrumbs) VALUES ('RES_{bookmark_suffix}', 'BM_{bookmark_suffix}', '2026-05-01T00:00:00Z', 'active', 'exact', 'headline', '10', 'snapshot_content', '[]');
             INSERT INTO _pack_meta (pack_id, protocol_version, purpose, source_client, generated_at) VALUES ('PACK_{bookmark_suffix}', 20, 'publish', 'test-client', '2026-05-01T00:00:00Z');",
        );
        conn.execute_batch(&sql).unwrap();
        std::fs::read(&pack_path).unwrap()
    };

    // First publish
    let pack_bytes = make_pack(&collection_id, "PUB1");
    let req = Request::builder()
        .method("POST")
        .uri("/tours")
        .header("X-Tour-Token", DEV_TOKEN)
        .header(header::CONTENT_TYPE, "application/vnd.codetours.pack+sqlite")
        .body(Body::from(pack_bytes))
        .unwrap();

    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    // Republish same collection (should succeed with 200, not 500)
    let pack_bytes = make_pack(&collection_id, "PUB2");
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
    assert_eq!(status, StatusCode::OK, "Republish failed: {:?}", body);
    assert_eq!(body["tour_id"], collection_id);
}

async fn ax_body_to_json(response: axum::response::Response) -> Value {
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

/// Builds a minimal v20 pack with one collection (given visibility/repo) and one
/// bookmark, returning its bytes. Keeps the new discovery/access tests concise.
fn build_pack(dir: &std::path::Path, col_id: &str, visibility: &str, repo_url: &str) -> Vec<u8> {
    let pack_path = dir.join(format!("{}.pack.sqlite", col_id));
    let conn = rusqlite::Connection::open(&pack_path).unwrap();
    let sql = format!(
        "PRAGMA user_version = 20;
         CREATE TABLE schema_meta (key TEXT PRIMARY KEY, value TEXT);
         INSERT INTO schema_meta (key, value) VALUES ('schema_version', '20');
         CREATE TABLE collections (id TEXT PRIMARY KEY, name TEXT, visibility TEXT, created_at TEXT, description TEXT, repo_url TEXT, created_branch TEXT, published_commit_sha TEXT, status TEXT, health TEXT, health_computed_at TEXT, published_at TEXT, updated_at TEXT, created_by TEXT);
         CREATE TABLE bookmarks (id TEXT PRIMARY KEY, file_path TEXT, query TEXT, language TEXT, created_at TEXT, content_hash TEXT, commit_hash TEXT, created_by TEXT, current_resolution_id TEXT);
         CREATE TABLE collection_bookmarks (collection_id TEXT, bookmark_id TEXT, position INTEGER, added_at TEXT);
         CREATE TABLE collection_tags (collection_id TEXT, tag TEXT, added_at TEXT, added_by TEXT);
         CREATE TABLE collection_links (id TEXT PRIMARY KEY, collection_id TEXT, kind TEXT, label TEXT, url TEXT, sort_order INTEGER, added_at TEXT, added_by TEXT);
         CREATE TABLE resolutions (id TEXT PRIMARY KEY, bookmark_id TEXT, resolved_at TEXT, health TEXT, method TEXT, headline TEXT, snapshot TEXT, commit_hash TEXT, match_count INTEGER, file_path TEXT, byte_range TEXT, line_range TEXT, content_hash TEXT, breadcrumbs TEXT, snapshot_top_padding INTEGER, snapshot_bottom_padding INTEGER);
         CREATE TABLE bookmark_annotations (id TEXT PRIMARY KEY, bookmark_id TEXT, added_at TEXT, added_by TEXT, notes TEXT, context TEXT, source TEXT);
         CREATE TABLE bookmark_tags (bookmark_id TEXT, tag TEXT, added_at TEXT, added_by TEXT);
         CREATE TABLE _pack_meta (pack_id TEXT PRIMARY KEY, protocol_version INTEGER, purpose TEXT, source_client TEXT, generated_at TEXT, notes TEXT);

         INSERT INTO collections (id, name, visibility, created_at, created_by, repo_url, status, health, published_at, updated_at) VALUES ('{col_id}', 'Tour {col_id}', '{visibility}', '2026-05-01T00:00:00Z', 'pack-author', '{repo_url}', 'ready', 'active', '2026-05-01T00:00:00Z', '2026-05-01T00:00:00Z');
         INSERT INTO bookmarks (id, file_path, query, language, created_at, current_resolution_id) VALUES ('BM_{col_id}', 'src/main.rs', 'query', 'rust', '2026-05-01T00:00:00Z', 'RES_{col_id}');
         INSERT INTO collection_bookmarks (collection_id, bookmark_id, position, added_at) VALUES ('{col_id}', 'BM_{col_id}', 0, '2026-05-01T00:00:00Z');
         INSERT INTO resolutions (id, bookmark_id, resolved_at, health, method, headline, line_range, snapshot, breadcrumbs) VALUES ('RES_{col_id}', 'BM_{col_id}', '2026-05-01T00:00:00Z', 'active', 'exact', 'headline', '10', 'snapshot_content', '[]');
         INSERT INTO _pack_meta (pack_id, protocol_version, purpose, source_client, generated_at) VALUES ('PACK_{col_id}', 20, 'publish', 'test-client', '2026-05-01T00:00:00Z');"
    );
    conn.execute_batch(&sql).unwrap();
    std::fs::read(&pack_path).unwrap()
}

async fn publish_pack(app: &axum::Router, pack_bytes: Vec<u8>) -> StatusCode {
    let req = Request::builder()
        .method("POST")
        .uri("/tours")
        .header("X-Tour-Token", DEV_TOKEN)
        .header(header::CONTENT_TYPE, "application/vnd.codetours.pack+sqlite")
        .body(Body::from(pack_bytes))
        .unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

/// `repos` is required: a bare `GET /tours` is a 400, not a broadcast directory.
#[tokio::test]
async fn test_list_requires_repos() {
    let (app, _github, _tmp) = setup_app().await;
    let req = Request::builder().method("GET").uri("/tours").body(Body::empty()).unwrap();
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = ax_body_to_json(response).await;
    assert_eq!(body["error"], "repos_required");
}

/// Anonymous callers see a public tour only when the named repo is itself public;
/// for a non-public repo the tour is *absent* (not a 403 — no existence leak).
#[tokio::test]
async fn test_anonymous_private_repo_tour_absent() {
    let (app, github, _tmp) = setup_app().await;
    let col_id = Uuid::new_v4().to_string();

    // Publish a (public-visibility) tour naming a repo we will treat as NOT public.
    github.seed_write_access("stub", "secret", "repo", true).await;
    let pack = build_pack(_tmp.path(), &col_id, "public", "git@github.com:secret/repo.git");
    assert_eq!(publish_pack(&app, pack).await, StatusCode::CREATED);

    // Repo is not public → anonymous list returns nothing for it.
    github.seed_public_repo("secret", "repo", false).await;
    let req = Request::builder()
        .method("GET")
        .uri("/tours?repos=secret/repo")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = ax_body_to_json(response).await;
    assert_eq!(body["total"], 0);
    assert!(body["tours"].as_array().unwrap().is_empty());

    // Once the repo is public, the same tour appears.
    github.seed_public_repo("secret", "repo", true).await;
    let req = Request::builder()
        .method("GET")
        .uri("/tours?repos=secret/repo")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(req).await.unwrap();
    let body = ax_body_to_json(response).await;
    assert_eq!(body["total"], 1);
    assert_eq!(body["tours"][0]["tour_id"], col_id);
}

/// A private tour's detail requires verified read access; otherwise it 404s
/// (not 403) for anonymous *and* authenticated-without-access callers.
#[tokio::test]
async fn test_private_tour_detail_requires_access() {
    let (app, github, _tmp) = setup_app().await;
    let col_id = Uuid::new_v4().to_string();

    github.seed_write_access("stub", "acme", "priv", true).await;
    let pack = build_pack(_tmp.path(), &col_id, "private", "git@github.com:acme/priv.git");
    assert_eq!(publish_pack(&app, pack).await, StatusCode::CREATED);

    // Anonymous → 404.
    let req = Request::builder()
        .method("GET")
        .uri(format!("/tours/{}", col_id))
        .body(Body::empty())
        .unwrap();
    assert_eq!(app.clone().oneshot(req).await.unwrap().status(), StatusCode::NOT_FOUND);

    // Authenticated without read access → 404 (seed denial to avoid a live call).
    github.seed_read_access("stub", "acme", "priv", false).await;
    let req = Request::builder()
        .method("GET")
        .uri(format!("/tours/{}", col_id))
        .header("X-Tour-Token", DEV_TOKEN)
        .body(Body::empty())
        .unwrap();
    assert_eq!(app.clone().oneshot(req).await.unwrap().status(), StatusCode::NOT_FOUND);

    // Authenticated *with* verified read access → 200.
    github.seed_read_access("stub", "acme", "priv", true).await;
    let req = Request::builder()
        .method("GET")
        .uri(format!("/tours/{}", col_id))
        .header("X-Tour-Token", DEV_TOKEN)
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = ax_body_to_json(response).await;
    assert_eq!(body["tour_id"], col_id);
}
