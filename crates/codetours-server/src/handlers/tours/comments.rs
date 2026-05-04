use crate::{auth::AuthContext, router::AppState};
use crate::handlers::tours::get::CommentView;
use axum::{
    extract::{Path, State, Form},
    http::StatusCode,
    response::IntoResponse,
};
use rinja::Template;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct CommentForm {
    pub content: String,
}

#[derive(Template)]
#[template(path = "tours/_comment_bubble.html")]
pub struct CommentBubbleTemplate {
    pub comment: CommentView,
}

pub async fn create_handler(
    State(state): State<AppState>,
    auth: AuthContext,
    Path((_tour_id, bookmark_id)): Path<(String, String)>,
    Form(form): Form<CommentForm>,
) -> impl IntoResponse {
    let author = auth.current_user(&state.config).unwrap_or_else(|| "Anonymous".to_string());
    let body = form.content.trim();

    if body.is_empty() || body.len() > 4000 {
        return (StatusCode::BAD_REQUEST, "Comment must be between 1 and 4000 characters.").into_response();
    }

    let db = match state.storage.get_conn().await {
        Ok(t) => t,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let author_clone = author.clone();
    let body_clone = body.to_string();
    let bid_clone = bookmark_id.clone();
    let comment_id = uuid::Uuid::new_v4().to_string();
    let created_at = chrono::Utc::now().to_rfc3339();

    let result = db.interact(move |conn| {
        conn.execute(
            "INSERT INTO bookmark_comments (id, bookmark_id, author, body, created_at) VALUES (?, ?, ?, ?, ?)",
            [&comment_id, &bid_clone, &author_clone, &body_clone, &created_at],
        )
    }).await;

    match result {
        Ok(Ok(_)) => {
            let author_initial = author.chars().next().unwrap_or('?').to_uppercase().to_string();
            let view = CommentView {
                author,
                author_initial,
                body: body.to_string(),
                created_at_relative: "just now".to_string(),
            };
            CommentBubbleTemplate { comment: view }.into_response()
        }
        _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}