use crate::{auth::AuthContext, router::AppState};
use crate::web::NavItem;
use axum::{
    extract::{Query, State},
    response::IntoResponse,
};
use rinja::Template;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct MyToursQuery {
    #[serde(default = "default_tab")]
    pub tab: String,
}

fn default_tab() -> String {
    "published".to_string()
}

#[derive(Template)]
#[template(path = "my_tours/list.html")]
pub struct MyToursTemplate {
    pub active_tab: String,
    pub published_count: usize,
    pub draft_count: usize,
    pub tours: Vec<MyTourCard>,
    pub nav: crate::web::NavItem,
}

pub struct MyTourCard {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub is_published: bool,
    pub created_at: String,
    pub updated_at: String,
    pub step_count: usize,
    pub repo: Option<String>,
}

pub async fn handler(
    State(state): State<AppState>,
    _auth: AuthContext,
    Query(query): Query<MyToursQuery>,
) -> impl IntoResponse {
    // In M2: single-tenant mode, show all tours for "My Tours"
    // Phase 6: filter by authenticated user_id
    let db = match state.storage.get_conn().await {
        Ok(t) => t,
        Err(_) => return axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let is_published_filter = query.tab == "published";

    let result = db.interact(move |conn| {
        // Count all collections (single-tenant mode)
        let published_count: usize = conn.query_row(
            "SELECT COUNT(*) FROM collections WHERE visibility IS NOT NULL",
            [],
            |row: &rusqlite::Row| row.get(0),
        ).unwrap_or(0);

        let draft_count: usize = conn.query_row(
            "SELECT COUNT(*) FROM collections WHERE visibility IS NULL",
            [],
            |row: &rusqlite::Row| row.get(0),
        ).unwrap_or(0);

        let sql = if is_published_filter {
            "SELECT id, name, description, created_at, created_at, repo_url,
            (SELECT COUNT(*) FROM collection_bookmarks WHERE collection_id = collections.id) as step_count
            FROM collections
            WHERE visibility IS NOT NULL
            ORDER BY created_at DESC"
        } else {
            "SELECT id, name, description, created_at, created_at, repo_url,
            (SELECT COUNT(*) FROM collection_bookmarks WHERE collection_id = collections.id) as step_count
            FROM collections
            WHERE visibility IS NULL
            ORDER BY created_at DESC"
        };

        let mut stmt = conn.prepare(sql).map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

        let tours = stmt.query_map([], |row: &rusqlite::Row| {
            let repo_url: Option<String> = row.get(5)?;
            let repo = repo_url.as_ref().and_then(|u| u.split('/').last().map(|s| s.to_string()));

            Ok(MyTourCard {
                id: row.get(0)?,
                title: row.get(1)?,
                description: row.get(2)?,
                is_published: is_published_filter,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
                repo,
                step_count: row.get(6)?,
            })
        }).map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?
        .collect::<Result<Vec<_>, _>>().map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

        Ok::<(usize, usize, Vec<MyTourCard>), axum::http::StatusCode>((published_count, draft_count, tours))
    }).await;

    match result {
        Ok(Ok((published_count, draft_count, tours))) => {
            MyToursTemplate {
                active_tab: query.tab,
                published_count,
                draft_count,
                tours,
                nav: crate::web::NavItem::MyTours,
            }.into_response()
        },
        _ => axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}