use crate::router::AppState;
use crate::static_assets;
use axum::{
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};

pub async fn handler(State(state): State<AppState>, Path(path): Path<String>) -> impl IntoResponse {
    let (content, content_type) = match path.as_str() {
        "app.css" => (static_assets::APP_CSS.to_string(), "text/css"),
        "htmx.min.js" => (static_assets::HTMX_JS.to_string(), "application/javascript"),
        "htmx-intersect.js" => {
            (static_assets::HX_INTERSECT_JS.to_string(), "application/javascript")
        }
        "app.js" => (static_assets::APP_JS.to_string(), "application/javascript"),
        "theme.css" => {
            if let Some(theme_path) = &state.config.ui.theme_css {
                if let Ok(css) = tokio::fs::read_to_string(theme_path).await {
                    return Response::builder()
                        .header(header::CONTENT_TYPE, "text/css")
                        .header(header::CACHE_CONTROL, "public, max-age=60")
                        .body(css)
                        .unwrap()
                        .into_response();
                }
            }
            return StatusCode::NO_CONTENT.into_response();
        }
        _ => return StatusCode::NOT_FOUND.into_response(),
    };

    Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
        .body(content)
        .unwrap()
        .into_response()
}
