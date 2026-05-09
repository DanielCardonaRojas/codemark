use crate::router::AppState;
use crate::storage::CollectionFilter;
use crate::web::negotiation::{Negotiated, ResponseFormat};
use crate::web::{NavItem, filters};
use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use rinja::Template;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

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

/// Tour data with step count and tags for rendering.
struct TourWithData {
    id: String,
    name: String,
    description: Option<String>,
    repo_url: Option<String>,
    updated_at: String,
    author: Option<String>,
    branch: Option<String>,
    health: Option<String>,
    step_count: usize,
    tags: Vec<String>,
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

    // Use storage engine if available (registry mode), otherwise use single DB
    let result = if let Some(engine) = &state.storage_engine {
        query_with_engine(engine, &params, limit, offset).await
    } else {
        query_single_db(&state, &params, limit, offset).await
    };

    match result {
        Ok((tours_data, total, repos, branches, tags)) => {
            if format == ResponseFormat::Pack {
                // Pack format is only supported for individual tours, not lists
                return StatusCode::NOT_IMPLEMENTED.into_response();
            }
            if format == ResponseFormat::Json {
                let tours = tours_data
                    .into_iter()
                    .map(|t| {
                        let repo_name = t.repo_url
                            .as_ref()
                            .and_then(|u| u.split('/').next_back().map(|s| s.to_string()));
                        TourSummary {
                            tour_id: t.id.clone(),
                            title: t.name,
                            description: t.description,
                            repo_url: t.repo_url,
                            repo: repo_name,
                            updated_at: t.updated_at,
                            health: t.health,
                            url: format!("/tours/{}", t.id),
                        }
                    })
                    .collect();
                (StatusCode::OK, Json(ListToursResponse { tours, total, limit, offset }))
                    .into_response()
            } else {
                let tours = tours_data
                    .into_iter()
                    .map(|t| {
                        let (status_class, health_label) = match t.health.as_deref() {
                            Some("active") => ("healthy", "ACTIVE"),
                            Some("drifted") => ("drifted", "DRIFTED"),
                            Some("stale") => ("stale", "STALE"),
                            _ => ("healthy", "ACTIVE"),
                        };

                        let updated_date =
                            if t.updated_at.len() >= 10 { t.updated_at[..10].to_string() } else { t.updated_at.clone() };

                        let repo_name = t.repo_url
                            .as_ref()
                            .and_then(|u| u.split('/').next_back().map(|s| s.to_string()));

                        TourView {
                            id: t.id,
                            title: t.name,
                            description: t.description.unwrap_or_default(),
                            health: t.health.clone(),
                            health_label: health_label.to_string(),
                            status_class: status_class.to_string(),
                            updated_at_relative: updated_date,
                            author: t.author.unwrap_or_else(|| "anonymous".to_string()),
                            repo: repo_name,
                            branch: t.branch.unwrap_or_else(|| "main".to_string()),
                            step_count: t.step_count,
                            tags: t.tags,
                        }
                    })
                    .collect();

                if hx_request {
                    ToursListPartialTemplate { tours }.into_response()
                } else {
                    tracing::info!("Rendering template with {} repos, {} branches, {} tags", repos.len(), branches.len(), tags.len());
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
        Err(e) => {
            tracing::error!("Error querying tours: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Query using the storage engine (scatter-gather mode).
async fn query_with_engine(
    engine: &crate::storage::StorageEngine,
    params: &ListToursParams,
    limit: usize,
    offset: usize,
) -> anyhow::Result<(Vec<TourWithData>, usize, Vec<String>, Vec<String>, Vec<String>)> {
    // In registry mode, show all collections (not just public/ready)
    // This is useful for local development where you want to see everything
    let filter = CollectionFilter {
        q: params.q.clone(),
        repo_url: params.repo_url.clone(),
        branch: params.branch.clone(),
        tag: params.tag.clone(),
        visibility: None,  // Show all, not just public
        status: None,      // Show all, not just ready
    };

    let result = engine
        .query_all_collections(filter, limit + offset, 0, params.sort.as_deref())
        .await?;

    // For scatter-gather, we need to get step counts and tags from each repo's database
    // This is expensive, so we'll return default values for now and optimize later
    let tours_with_data: Vec<TourWithData> = result.items.into_iter().map(|entry| {
        TourWithData {
            id: entry.id,
            name: entry.name,
            description: entry.description,
            repo_url: entry.repo_url,
            updated_at: entry.updated_at,
            author: entry.created_by,
            branch: entry.created_branch,
            health: entry.health,
            step_count: 0, // TODO: Fetch from repo DB
            tags: vec![],   // TODO: Fetch from repo DB
        }
    }).collect();

    // Get filter options from the engine
    let mut repos_set = HashSet::new();
    let mut branches_set = HashSet::new();
    let mut tags_set = HashSet::new();

    // Get all repos from the engine for the filter dropdown
    let all_repos = engine.all_repos().await;
    tracing::info!("Found {} repos from engine for filter dropdown", all_repos.len());
    for (owner, name, origin_url) in all_repos {
        // Use origin_url if available (e.g., https://github.com/owner/repo)
        // Otherwise construct from owner/name
        let repo_display = origin_url.unwrap_or_else(|| format!("{}/{}", owner, name));
        tracing::info!("Adding repo to filter: {} (owner={}, name={})", repo_display, owner, name);
        repos_set.insert(repo_display);
    }
    tracing::info!("Final repos_set size: {}", repos_set.len());

    // Also collect from tour results for branches and tags
    for tour in &tours_with_data {
        if let Some(branch) = &tour.branch {
            branches_set.insert(branch.clone());
        }
        for tag in &tour.tags {
            tags_set.insert(tag.clone());
        }
    }

    let mut repos: Vec<String> = repos_set.into_iter().collect();
    repos.sort(); // Sort alphabetically
    let mut branches: Vec<String> = branches_set.into_iter().collect();
    branches.sort();
    let mut tags: Vec<String> = tags_set.into_iter().collect();
    tags.sort();

    Ok((tours_with_data, result.total, repos, branches, tags))
}

/// Query using single database (legacy mode).
async fn query_single_db(
    state: &AppState,
    params: &ListToursParams,
    limit: usize,
    offset: usize,
) -> anyhow::Result<(Vec<TourWithData>, usize, Vec<String>, Vec<String>, Vec<String>)> {
    let storage = state.storage.clone();
    let conn = storage.get_conn().await?;
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

            sql_params.push(Box::new(limit + offset)); // Fetch extra for offset
            sql_params.push(Box::new(0)); // Offset handled in-memory

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
            let mut count_query = "
                SELECT COUNT(*) FROM collections c
                WHERE c.visibility = 'public' AND c.status = 'ready'
            ".to_string();

            let mut count_params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

            if let Some(q) = &params_clone.q {
                count_query.push_str(" AND (c.name LIKE ? OR c.description LIKE ?)");
                let pattern = format!("%{}%", q);
                count_params.push(Box::new(pattern.clone()));
                count_params.push(Box::new(pattern));
            }

            if let Some(repo) = &params_clone.repo_url {
                count_query.push_str(" AND c.repo_url = ?");
                count_params.push(Box::new(repo.clone()));
            }

            if let Some(branch) = &params_clone.branch {
                count_query.push_str(" AND c.created_branch = ?");
                count_params.push(Box::new(branch.clone()));
            }

            if let Some(tag) = &params_clone.tag {
                count_query.push_str(" AND EXISTS (
                    SELECT 1 FROM collection_tags ct
                    WHERE ct.collection_id = c.id AND ct.tag = ?
                )");
                count_params.push(Box::new(tag.clone()));
            }

            let total: usize = conn.query_row(&count_query, rusqlite::params_from_iter(count_params), |row| row.get(0))?;

            // 4. Get filter options (Repos, Branches, Tags) - use same published-ready scope as main query
            let repos: Vec<String> = conn.prepare("SELECT DISTINCT repo_url FROM collections WHERE repo_url IS NOT NULL AND visibility = 'public' AND status = 'ready'")?
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()?;

            let branches: Vec<String> = conn.prepare("SELECT DISTINCT created_branch FROM collections WHERE created_branch IS NOT NULL AND visibility = 'public' AND status = 'ready'")?
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()?;

            let tags: Vec<String> = conn.prepare("
                SELECT DISTINCT ct.tag
                FROM collection_tags ct
                INNER JOIN collections c ON c.id = ct.collection_id
                WHERE c.visibility = 'public' AND c.status = 'ready'
            ")?
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()?;

            Ok::<(Vec<_>, usize, Vec<String>, Vec<String>, Vec<String>), rusqlite::Error>((tours, total, repos, branches, tags))
        })
        .await
        .map_err(|e| anyhow::anyhow!("Interaction error: {}", e))??;

    // Convert to TourWithData and apply offset
    let offset = offset.min(result.0.len());
    let limit = limit.min(result.0.len() - offset);
    let tours_data: Vec<TourWithData> = result.0.into_iter()
        .skip(offset)
        .take(limit)
        .map(|(id, name, desc, repo, updated, author, branch, health, step_count, tags)| {
            TourWithData {
                id,
                name,
                description: desc,
                repo_url: repo,
                updated_at: updated,
                author,
                branch,
                health,
                step_count,
                tags,
            }
        })
        .collect();

    Ok((tours_data, result.1, result.2, result.3, result.4))
}
