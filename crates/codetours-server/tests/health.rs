use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use codetours_server::{
    config::Config,
    router::{AppState, router},
    storage::{StorageManager, registry::RegistryManager},
};
use serde_json::Value;
use std::sync::Arc;
use tempfile::tempdir;
use tower::ServiceExt; // for oneshot

async fn setup_app() -> (axum::Router, tempfile::TempDir) {
    let config = Config::default();
    let temp_data = tempdir().unwrap();
    let storage =
        StorageManager::new(temp_data.path().to_path_buf(), config.storage.clone()).unwrap();
    let registry = RegistryManager::new(temp_data.path()).unwrap();

    let state = AppState {
        config: Arc::new(config),
        storage: Arc::new(storage),
        registry: Arc::new(registry),
        github: Arc::new(codetours_server::github::GitHubVerifier::new()),
    };
    (router(state), temp_data)
}

#[tokio::test]
async fn test_health() {
    let (app, _tmp) = setup_app().await;

    let response =
        app.oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap()).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    // Test 2: response includes X-Request-Id header (uuid format)
    let request_id = response.headers().get("X-Request-Id");
    assert!(request_id.is_some(), "Response should have X-Request-Id header");
    let request_id_str = request_id.unwrap().to_str().unwrap();
    assert!(uuid::Uuid::parse_str(request_id_str).is_ok(), "X-Request-Id should be a valid UUID");

    // Test 1: GET /health returns 200 with status: "ok" and a non-empty version
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(body["status"], "ok");
    assert!(!body["version"].as_str().unwrap().is_empty());
    assert!(body["boot_time"].as_str().is_some());
}

#[tokio::test]
async fn test_404() {
    let (app, _tmp) = setup_app().await;

    let response = app
        .oneshot(Request::builder().uri("/nonexistent").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_request_id_generation() {
    let (app, _tmp) = setup_app().await;

    // Test 3: a request without an X-Request-Id header gets a server-generated one
    let response =
        app.oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap()).await.unwrap();

    let request_id = response.headers().get("X-Request-Id");
    assert!(request_id.is_some());
}
