use crate::web::NavItem;
use crate::{auth::AuthContext, router::AppState, storage::CollectionFilter};
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
    // Use storage engine if available (registry mode), otherwise use single DB
    let result = if let Some(engine) = &state.storage_engine {
        query_with_engine(engine, &query.tab).await
    } else {
        query_single_db(&state, &query.tab).await
    };

    match result {
        Ok((published_count, draft_count, tours)) => MyToursTemplate {
            active_tab: query.tab,
            published_count,
            draft_count,
            tours,
            nav: crate::web::NavItem::MyTours,
        }
        .into_response(),
        Err(e) => {
            tracing::error!("Error querying my tours: {}", e);
            axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Query using the storage engine (scatter-gather mode).
async fn query_with_engine(
    engine: &crate::storage::StorageEngine,
    tab: &str,
) -> anyhow::Result<(usize, usize, Vec<MyTourCard>)> {
    let is_published_filter = tab == "published";

    // Query for published tours
    let published_filter = CollectionFilter {
        visibility: Some("public".to_string()),
        status: Some("ready".to_string()),
        ..Default::default()
    };

    let published_result = engine
        .query_all_collections(published_filter, 1000, 0, None)
        .await?;

    // Query for draft tours (everything not published)
    // For simplicity, we'll query all and filter in-memory
    let all_filter = CollectionFilter::default();
    let all_result = engine
        .query_all_collections(all_filter, 1000, 0, None)
        .await?;

    let published_count = published_result.total;
    let draft_count = all_result.total - published_count;

    let tours = if is_published_filter {
        published_result.items
    } else {
        // Filter out published tours
        all_result.items
            .into_iter()
            .filter(|t| {
                !(t.repo_url.as_deref() == Some("public") || t.health.as_deref() == Some("ready"))
            })
            .collect()
    };

    let tour_cards = tours
        .into_iter()
        .map(|entry| {
            let repo = entry
                .repo_url
                .as_ref()
                .and_then(|u| u.split('/').next_back().map(|s| s.to_string()));

            MyTourCard {
                id: entry.id,
                title: entry.name,
                description: entry.description,
                is_published: is_published_filter,
                created_at: entry.updated_at.clone(),
                updated_at: entry.updated_at,
                step_count: 0, // TODO: Fetch from repo DB
                repo,
            }
        })
        .collect();

    Ok((published_count, draft_count, tour_cards))
}

/// Query using single database (legacy mode).
async fn query_single_db(
    state: &AppState,
    tab: &str,
) -> anyhow::Result<(usize, usize, Vec<MyTourCard>)> {
    let is_published_filter = tab == "published";
    let db = state.storage.get_conn().await?;

    let result = db.interact(move |conn| {
        // Count all collections (single-tenant mode)
        // Published = visibility = 'public' AND status = 'ready' (canonical definition from tours/list.rs)
        let published_count: usize = conn.query_row(
            "SELECT COUNT(*) FROM collections WHERE visibility = 'public' AND status = 'ready'",
            [],
            |row: &rusqlite::Row| row.get(0),
        ).unwrap_or(0);

        // Drafts = everything not matching the published condition
        let draft_count: usize = conn.query_row(
            "SELECT COUNT(*) FROM collections WHERE NOT (visibility = 'public' AND status = 'ready')",
            [],
            |row: &rusqlite::Row| row.get(0),
        ).unwrap_or(0);

        let sql = if is_published_filter {
            "SELECT id, name, description, created_at, updated_at, repo_url,
            (SELECT COUNT(*) FROM collection_bookmarks WHERE collection_id = collections.id) as step_count
            FROM collections
            WHERE visibility = 'public' AND status = 'ready'
            ORDER BY created_at DESC"
        } else {
            "SELECT id, name, description, created_at, updated_at, repo_url,
            (SELECT COUNT(*) FROM collection_bookmarks WHERE collection_id = collections.id) as step_count
            FROM collections
            WHERE NOT (visibility = 'public' AND status = 'ready')
            ORDER BY created_at DESC"
        };

        let mut stmt = conn.prepare(sql)?;

        let tours = stmt.query_map([], |row: &rusqlite::Row| {
            let repo_url: Option<String> = row.get(5)?;
            let repo = repo_url.as_ref().and_then(|u| u.split('/').next_back().map(|s| s.to_string()));

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
        })?.collect::<rusqlite::Result<Vec<_>>>()?;

        Ok::<(usize, usize, Vec<MyTourCard>), rusqlite::Error>((published_count, draft_count, tours))
    }).await.map_err(|e| anyhow::anyhow!("Interaction error: {}", e))??;

    Ok(result)
}
