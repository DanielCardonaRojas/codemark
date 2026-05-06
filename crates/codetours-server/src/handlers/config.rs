use crate::web::NavItem;
use crate::{auth::AuthContext, router::AppState};
use axum::{
    extract::{Form, State},
    http::StatusCode,
    response::IntoResponse,
};
use rinja::Template;
use serde::Deserialize;

#[derive(Template)]
#[template(path = "config/index.html")]
pub struct ConfigTemplate {
    pub nav: NavItem,
    pub repos: Vec<RepoView>,
}

pub struct RepoView {
    pub name: String,
    pub path: String,
    pub connected: bool,
}

#[derive(Deserialize)]
pub struct PrefsForm {
    pub theme: String,
    pub font: String,
}

pub async fn page_handler(State(state): State<AppState>, _auth: AuthContext) -> impl IntoResponse {
    let db = match state.storage.get_conn().await {
        Ok(t) => t,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let repos = db
        .interact(|conn| {
            let mut stmt = conn
                .prepare(
                    "
            SELECT DISTINCT repo_url 
            FROM collections 
            WHERE repo_url IS NOT NULL AND repo_url != ''
        ",
                )
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

            let urls = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

            let mut repos = Vec::new();
            for url in urls {
                let name = url.split('/').next_back().unwrap_or(&url).to_string();
                repos.push(RepoView { name, path: url, connected: true });
            }
            Ok::<_, StatusCode>(repos)
        })
        .await
        .unwrap_or(Err(StatusCode::INTERNAL_SERVER_ERROR));

    match repos {
        Ok(repos) => ConfigTemplate { nav: NavItem::Config, repos }.into_response(),
        Err(e) => e.into_response(),
    }
}

pub async fn prefs_handler(
    State(_state): State<AppState>,
    _auth: AuthContext,
    Form(_form): Form<PrefsForm>,
) -> impl IntoResponse {
    StatusCode::OK.into_response()
}

pub async fn stub_handler() -> impl IntoResponse {
    (StatusCode::NOT_IMPLEMENTED, "This feature will be available in Phase 10 or later.")
        .into_response()
}
