use crate::handlers::tours::get::LinkView;
use crate::{auth::AuthContext, router::AppState};
use axum::{
    extract::{Form, Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::Utc;
use rinja::Template;
use serde::Deserialize;
use uuid::Uuid;

#[derive(Deserialize)]
pub struct AddLinkForm {
    pub label: String,
    pub url: String,
    pub kind: String,
}

#[derive(Template)]
#[template(path = "tours/_link_item.html")]
pub struct LinkItemTemplate {
    pub link: LinkView,
}

pub async fn add_handler(
    State(state): State<AppState>,
    auth: AuthContext,
    Path(tour_id): Path<String>,
    Form(form): Form<AddLinkForm>,
) -> impl IntoResponse {
    let label = form.label.trim();
    let url = form.url.trim();
    let kind = form.kind.trim();

    if label.is_empty() || url.is_empty() {
        return (StatusCode::BAD_REQUEST, "Label and URL are required.").into_response();
    }

    // Reject URLs whose scheme is not http(s) to avoid javascript:/data: injection.
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return (StatusCode::BAD_REQUEST, "URL must start with http:// or https://")
            .into_response();
    }

    let db = match state.storage.get_conn().await {
        Ok(t) => t,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let id = Uuid::new_v4().to_string();
    let id_clone = id.clone();
    let tour_id_clone = tour_id.clone();
    let label_clone = label.to_string();
    let url_clone = url.to_string();
    let kind_clone = kind.to_string();
    let added_at = Utc::now().to_rfc3339();
    let added_by = auth.current_user(&state.config).unwrap_or_else(|| "Anonymous".to_string());

    let result = db.interact(move |conn| {
        conn.execute(
            "INSERT INTO collection_links (id, collection_id, kind, label, url, sort_order, added_at, added_by) 
             VALUES (?, ?, ?, ?, ?, (SELECT COALESCE(MAX(sort_order), -1) + 1 FROM collection_links WHERE collection_id = ?), ?, ?)",
            rusqlite::params![
                &id_clone,
                &tour_id_clone,
                &kind_clone,
                &label_clone,
                &url_clone,
                &tour_id_clone,
                &added_at,
                &added_by,
            ],
        )
    }).await;

    match result {
        Ok(Ok(_)) => {
            let icon = match kind {
                "pr" => "git-pull-request",
                "issue" => "alert-circle",
                "doc" => "file-text",
                "discussion" => "message-square",
                "dashboard" => "layout",
                "repo" => "github",
                "tour" => "book-open",
                _ => "link",
            }
            .to_string();

            let view = LinkView {
                id,
                kind: kind.to_string(),
                label: label.to_string(),
                url: url.to_string(),
                icon,
            };
            LinkItemTemplate { link: view }.into_response()
        }
        _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
