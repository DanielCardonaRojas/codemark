use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use codetours_server::{
    config::Config,
    router::{AppState, router},
    storage::StorageManager,
};
use std::sync::Arc;
use tempfile::tempdir;
use tower::ServiceExt;

const DEV_TOKEN: &str = "dev-secret-not-for-prod";

async fn setup_app() -> (axum::Router, tempfile::TempDir) {
    let mut config = Config::default();
    config.auth.dev_token = DEV_TOKEN.to_string();
    let temp_data = tempdir().unwrap();
    let storage =
        StorageManager::new(temp_data.path().to_path_buf(), config.storage.clone()).unwrap();

    let state = AppState {
        config: Arc::new(config),
        storage: Arc::new(storage),
        storage_engine: None,
    };
    (router(state), temp_data)
}

#[tokio::test]
async fn test_html_smoke() {
    let (app, _tmp) = setup_app().await;

    // Home redirects to /tours
    let req = Request::builder().uri("/").body(Body::empty()).unwrap();
    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);

    // GET /tours returns HTML
    let req = Request::builder()
        .uri("/tours")
        .header(header::ACCEPT, "text/html")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers();
    assert!(headers.get(header::CONTENT_SECURITY_POLICY).is_some(), "CSP header missing");

    // GET /my-tours returns HTML
    let req = Request::builder()
        .uri("/my-tours")
        .header(header::ACCEPT, "text/html")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // GET /config returns HTML
    let req = Request::builder()
        .uri("/config")
        .header(header::ACCEPT, "text/html")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // GET /tours/invalid returns 404 (not 5xx)
    let req = Request::builder()
        .uri("/tours/not-a-tour")
        .header(header::ACCEPT, "text/html")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
