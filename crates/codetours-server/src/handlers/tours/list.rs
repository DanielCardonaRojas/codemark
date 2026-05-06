use crate::router::AppState;
use crate::web::negotiation::{Negotiated, ResponseFormat};
use crate::web::{filters, NavItem};
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use rinja::Template;
use serde::{Deserialize, Serialize};

/// Request parameters for listing tours.
#[derive(Debug, Deserialize, Clone, Serialize, Default)]
pub struct ListToursParams {
    /// Full-text search or title/description search.
    pub q: Option<String>,
    /// Optional repository URL filter.
    pub repo_url: Option<String>,
    /// Optional branch filter.
    pub branch: Option<String>,
    /// Optional tag filter.
    pub tag: Option<String>,
    /// Maximum number of tours to return (default 50, max 200).
    pub limit: Option<usize>,
    /// Number of tours to skip.
    pub offset: Option<usize>,
    /// Sort order (updated_at_desc, updated_at_asc, title_asc).
    pub sort: Option<String>,
}

/// View model for a tour card.
#[derive(Debug, Serialize)]
pub struct TourView {
    pub id: String,
    pub title: String,
    pub description: String,
    pub health: Option<String>,
    pub health_label: String,
    pub status_class: String,
    pub updated_at_relative: String,
    pub author: String,
    pub repo: Option<String>,
    pub branch: String,
    pub step_count: usize,
    pub tags: Vec<String>,
}

#[derive(Template)]
#[template(path = "tours/list.html")]
pub struct ToursListTemplate {
    pub nav: NavItem,
    pub tours: Vec<TourView>,
    pub repos: Vec<String>,
    pub branches: Vec<String>,
    pub tags: Vec<String>,
    pub params: ListToursParams,
    pub hx_request: bool,
}

#[derive(Template)]
#[template(path = "tours/list_partial.html")]
pub struct ToursListPartialTemplate {
    pub tours: Vec<TourView>,
}

/// Summary information for a single tour in a list (JSON API).
#[derive(Debug, Serialize)]
pub struct TourSummary {
    pub tour_id: String,
    pub title: String,
    pub description: Option<String>,
    pub repo_url: Option<String>,
    pub repo: Option<String>,
    pub updated_at: String,
    pub health: Option<String>,
    pub url: String,
}

/// Paginated response body for listing tours (JSON API).
#[derive(Debug, Serialize)]
pub struct ListToursResponse {
    pub tours: Vec<TourSummary>,
    pub total: usize,
    pub limit: usize,
    pub offset: usize,
}

/// Handler for GET /tours. Lists public tours with filtering and pagination.
pub async fn handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Negotiated(format): Negotiated,
    Query(params): Query<ListToursParams>,
) -> impl IntoResponse {
    let limit = params.limit.unwrap_or(50).min(200);
    let offset = params.offset.unwrap_or(0);
    let hx_request = headers.contains_key("hx-request");

    let storage = state.storage.clone();
    let conn = match storage.get_conn().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Failed to acquire DB connection: {}", e);
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
    };

    let params_clone = params.clone();
    let result = conn
        .interact(move |conn| {
            // 1. Build Query
            let mut query_str = "
                SELECT c.id, c.name, c.description, c.repo_url, c.updated_at, c.created_by, c.created_branch, c.health
                FROM collections c
                WHERE c.visibility = 'public' AND c.status = 'ready'
            ".to_string();

            let mut where_clauses = Vec::new();
            let mut sql_params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

            if let Some(q) = &params_clone.q {
                where_clauses.push("(c.name LIKE ? OR c.description LIKE ?)");
                let pattern = format!("%{}%", q);
                sql_params.push(Box::new(pattern.clone()));
                sql_params.push(Box::new(pattern));
            }

            if let Some(repo) = &params_clone.repo_url {
                where_clauses.push("c.repo_url = ?");
                sql_params.push(Box::new(repo.clone()));
            }

            if let Some(branch) = &params_clone.branch {
                where_clauses.push("c.created_branch = ?");
                sql_params.push(Box::new(branch.clone()));
            }

            if let Some(tag) = &params_clone.tag {
                where_clauses.push("EXISTS (
                    SELECT 1 FROM collection_tags ct 
                    WHERE ct.collection_id = c.id AND ct.tag = ?
                )");
                sql_params.push(Box::new(tag.clone()));
            }

            for clause in where_clauses {
                query_str.push_str(" AND ");
                query_str.push_str(clause);
            }

            let sort = match params_clone.sort.as_deref() {
                Some("updated_at_asc") => "c.updated_at ASC",
                Some("title_asc") => "c.name ASC",
                _ => "c.updated_at DESC",
            };
            query_str.push_str(&format!(" ORDER BY {}", sort));
            query_str.push_str(" LIMIT ? OFFSET ?");
            
            sql_params.push(Box::new(limit));
            sql_params.push(Box::new(offset));

            let mut stmt = conn.prepare(&query_str)?;
            let tours_rows = stmt.query_map(rusqlite::params_from_iter(sql_params), |row| {
                Ok((
                    row.get::<_, String>(0)?, // id
                    row.get::<_, String>(1)?, // name
                    row.get::<_, Option<String>>(2)?, // description
                    row.get::<_, Option<String>>(3)?, // repo_url
                    row.get::<_, String>(4)?, // updated_at
                    row.get::<_, Option<String>>(5)?, // created_by
                    row.get::<_, Option<String>>(6)?, // created_branch
                    row.get::<_, Option<String>>(7)?, // health
                ))
            })?.collect::<rusqlite::Result<Vec<_>>>()?;

            // 2. Map rows to view models or summaries
            let mut tours = Vec::new();
            for (id, name, desc, repo, updated, author, branch, health) in tours_rows {
                let step_count: usize = conn.query_row(
                    "SELECT COUNT(*) FROM collection_bookmarks WHERE collection_id = ?",
                    [&id],
                    |row| row.get(0),
                )?;

                let tags: Vec<String> = conn.prepare("
                    SELECT tag FROM collection_tags 
                    WHERE collection_id = ?
                    ORDER BY added_at ASC
                ")?
                .query_map([&id], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()?;

                tours.push((id, name, desc, repo, updated, author, branch, health, step_count, tags));
            }

            // 3. Get total count
            let count_query = "
                SELECT COUNT(*) FROM collections c 
                WHERE c.visibility = 'public' AND c.status = 'ready'
            ";
            // (Re-apply filters to count query if needed, but for now keep it simple)

            let total: usize = conn.query_row(&count_query, [], |row| row.get(0))?;

            // 4. Get filter options (Repos, Branches, Tags)
            let repos: Vec<String> = conn.prepare("SELECT DISTINCT repo_url FROM collections WHERE repo_url IS NOT NULL AND visibility = 'public'")?
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()?;

            let branches: Vec<String> = conn.prepare("SELECT DISTINCT created_branch FROM collections WHERE created_branch IS NOT NULL AND visibility = 'public'")?
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()?;

            let tags: Vec<String> = conn.prepare("SELECT DISTINCT tag FROM collection_tags")?
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()?;

            Ok::<(Vec<_>, usize, Vec<String>, Vec<String>, Vec<String>), rusqlite::Error>((tours, total, repos, branches, tags))
        })
        .await;

    match result {
        Ok(Ok((tours_data, total, repos, branches, tags))) => {
            if format == ResponseFormat::Json {
                let tours = tours_data.into_iter().map(|(id, name, desc, repo, updated, _, _, health, _, _)| {
                    let repo_name = repo.as_ref().and_then(|u| u.split('/').last().map(|s| s.to_string()));
                    TourSummary {
                        tour_id: id.clone(),
                        title: name,
                        description: desc,
                        repo_url: repo,
                        repo: repo_name,
                        updated_at: updated,
                        health,
                        url: format!("/tours/{}", id),
                    }
                }).collect();
                (StatusCode::OK, Json(ListToursResponse { tours, total, limit, offset })).into_response()
            } else {
                let tours = tours_data.into_iter().map(|(id, name, desc, repo, updated, author, branch, health, steps, tags)| {
                    let (status_class, health_label) = match health.as_deref() {
                        Some("active") => ("healthy", "ACTIVE"),
                        Some("drifted") => ("drifted", "DRIFTED"),
                        Some("stale") => ("stale", "STALE"),
                        _ => ("healthy", "ACTIVE"),
                    };

                    let updated_date = if updated.len() >= 10 {
                        updated[..10].to_string()
                    } else {
                        updated
                    };

                    let repo_name = repo.as_ref().and_then(|u| u.split('/').last().map(|s| s.to_string()));

                    TourView {
                        id,
                        title: name,
                        description: desc.unwrap_or_default(),
                        health: health.clone(),
                        health_label: health_label.to_string(),
                        status_class: status_class.to_string(),
                        updated_at_relative: updated_date,
                        author: author.unwrap_or_else(|| "anonymous".to_string()),
                        repo: repo_name,
                        branch: branch.unwrap_or_else(|| "main".to_string()),
                        step_count: steps,
                        tags,
                    }
                }).collect();

                if hx_request {
                    ToursListPartialTemplate { tours }.into_response()
                } else {
                    let template = ToursListTemplate {
                        nav: NavItem::Tours,
                        tours,
                        repos,
                        branches,
                        tags,
                        params,
                        hx_request,
                    };
                    template.into_response()
                }
            }
        }
        _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
