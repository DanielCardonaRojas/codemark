use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
};
use rinja::Template;
use serde::Serialize;
use tokio::fs;

use crate::auth::AuthContext;
use crate::pack_cache::PackCache;
use crate::router::AppState;
use crate::web::negotiation::{Negotiated, ResponseFormat};
use crate::web::{NavItem, filters};

/// Detailed information about a tour, including its bookmarks (JSON API).
#[derive(Debug, Serialize)]
pub struct TourDetail {
    pub tour_id: String,
    pub title: String,
    pub description: Option<String>,
    pub repo_url: Option<String>,
    pub published_at: String,
    pub bookmarks: Vec<BookmarkDetail>,
}

/// Information about a single bookmark within a tour (JSON API).
#[derive(Debug, Serialize)]
pub struct BookmarkDetail {
    pub id: String,
    pub file_path: String,
    pub line_range: Option<String>,
    pub snapshot: Option<ResolutionSnapshot>,
}

/// A snapshot of a bookmark's resolution at a point in time (JSON API).
#[derive(Debug, Serialize)]
pub struct ResolutionSnapshot {
    pub headline: Option<String>,
    /// Exact snapshot showing only the target node code (no padding).
    pub snapshot: Option<String>,
    /// Sticky headers (breadcrumbs) representing structural context.
    pub sticky_lines: Vec<String>,
    /// Corresponding line numbers for the sticky headers.
    pub sticky_line_numbers: Vec<usize>,
}

#[derive(Template)]
#[template(path = "tours/detail.html")]
pub struct TourDetailTemplate {
    pub nav: NavItem,
    pub tour: TourDetailView,
    pub host: String,
}

#[derive(Serialize)]
pub struct TourDetailView {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub author: String,
    pub published_at_relative: String,
    pub health: Option<String>,
    pub health_class: String,
    pub health_label: String,
    pub health_computed_at: Option<String>,
    pub is_drifted: bool,
    pub bookmarks: Vec<BookmarkView>,
    pub links: Vec<LinkView>,
}

#[derive(Serialize)]
pub struct LinkView {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub url: String,
    pub icon: String,
}

#[derive(Serialize)]
pub struct BookmarkView {
    pub id: String,
    pub id_short: String,
    pub ordinal: usize,
    pub headline: String,
    pub file_path: String,
    pub line_range: String,
    pub line: usize,
    pub health: String,
    pub health_class: String,
    pub note: String,
    pub language: String,
    pub preview_lines: String,
    pub highlighted: String,
    /// The highlighted code with sticky lines prepended at the top
    pub highlighted_with_sticky: String,
    /// Line numbers for the sticky lines only
    pub sticky_line_numbers: Vec<usize>,
    pub has_query: bool,
    pub query: Option<String>,
    pub query_highlighted: Option<String>,
    pub tags: Vec<String>,
    pub comment_count: usize,
    pub comments: Vec<CommentView>,
    pub has_notes: bool,
    pub breadcrumbs: Vec<codemark_core::engine::breadcrumbs::Breadcrumb>,
    pub snippet_start: usize,
}

impl BookmarkView {
    /// Check if a given line number (absolute, 1-based) is a breadcrumb line.
    pub fn is_breadcrumb_line(&self, line_num: &usize) -> bool {
        self.breadcrumbs.iter().any(|bc| bc.line == *line_num)
    }

    /// Get the breadcrumb text for a given line number, if it exists.
    pub fn breadcrumb_text(&self, line_num: &usize) -> Option<String> {
        self.breadcrumbs.iter()
            .find(|bc| bc.line == *line_num)
            .map(|bc| bc.text.clone())
    }
}

#[derive(Serialize)]
pub struct CommentView {
    pub author: String,
    pub author_initial: String,
    pub body: String,
    pub created_at_relative: String,
}

/// Handler for GET /tours/:id. Returns tour details in HTML, JSON or binary pack format.
pub async fn handler(
    State(state): State<AppState>,
    auth: AuthContext,
    headers: HeaderMap,
    Negotiated(format): Negotiated,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost:8080")
        .to_string();

    match format {
        ResponseFormat::Pack => {
            // Check visibility/auth before serving the pack
            let storage = state.storage.clone();
            let id_clone = id.clone();
            let is_auth = auth.is_authenticated();

            let conn = match storage.get_conn().await {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!("Failed to acquire DB connection: {}", e);
                    return StatusCode::SERVICE_UNAVAILABLE.into_response();
                }
            };

            let allowed = conn
                .interact(move |conn| {
                    let sql = if is_auth {
                        "SELECT EXISTS(SELECT 1 FROM collections WHERE id = ?1 AND visibility IS NOT NULL)"
                    } else {
                        "SELECT EXISTS(SELECT 1 FROM collections WHERE id = ?1 AND visibility = 'public')"
                    };
                    conn.query_row(sql, [&id_clone], |row| row.get::<_, bool>(0))
                })
                .await;

            let allowed = match allowed {
                Ok(Ok(allowed)) => allowed,
                _ => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            };

            if !allowed {
                return StatusCode::NOT_FOUND.into_response();
            }

            let cache = PackCache::new(state.config.data_dir.clone());
            let pack_path = cache.get_pack_path(&id);

            if let Some(path) = pack_path.filter(|p| p.exists()) {
                match fs::read(&path).await {
                    Ok(bytes) => {
                        return (
                            [(header::CONTENT_TYPE, "application/vnd.codetours.pack+sqlite")],
                            bytes,
                        )
                            .into_response();
                    }
                    Err(e) => {
                        tracing::error!("Failed to read pack from cache: {}", e);
                        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                    }
                }
            }
            StatusCode::NOT_FOUND.into_response()
        }
        ResponseFormat::Html | ResponseFormat::Json => {
            let storage = state.storage.clone();
            let is_auth = auth.is_authenticated();

            let conn = match storage.get_conn().await {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!("Failed to acquire DB connection: {}", e);
                    return StatusCode::SERVICE_UNAVAILABLE.into_response();
                }
            };

            let id_clone = id.clone();
            let result = conn
                .interact(move |conn| {
                    // 1. Get collection
                    let sql = if is_auth {
                        "SELECT id, name, description, repo_url, published_at, created_by, health, health_computed_at FROM collections
                     WHERE id = ?1 AND visibility IS NOT NULL"
                    } else {
                        "SELECT id, name, description, repo_url, published_at, created_by, health, health_computed_at FROM collections
                     WHERE id = ?1 AND visibility = 'public'"
                    };

                    let (coll_id, name, description, repo_url, published_at, author, health, health_computed_at) = conn
                        .query_row(sql, [&id_clone], |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, Option<String>>(2)?,
                                row.get::<_, Option<String>>(3)?,
                                row.get::<_, String>(4)?,
                                row.get::<_, Option<String>>(5)?,
                                row.get::<_, Option<String>>(6)?,
                                row.get::<_, Option<String>>(7)?,
                            ))
                        })
                        .map_err(|e| {
                            tracing::error!("Database error fetching tour: {:?}", e);
                            match e {
                                rusqlite::Error::QueryReturnedNoRows => StatusCode::NOT_FOUND,
                                _ => StatusCode::INTERNAL_SERVER_ERROR,
                            }
                        })?;

                    // 2. Get links
                    let mut stmt = conn.prepare("SELECT id, kind, label, url FROM collection_links WHERE collection_id = ? ORDER BY sort_order ASC, added_at ASC")
                        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
                    let links = stmt.query_map([&id_clone], |row| {
                        let kind: String = row.get(1)?;
                        let icon = match kind.as_str() {
                            "pr" => "git-pull-request",
                            "issue" => "alert-circle",
                            "doc" => "file-text",
                            "discussion" => "message-square",
                            "dashboard" => "layout",
                            "repo" => "github",
                            "tour" => "book-open",
                            _ => "link",
                        }.to_string();

                        Ok(LinkView {
                            id: row.get(0)?,
                            kind,
                            label: row.get(2)?,
                            url: row.get(3)?,
                            icon,
                        })
                    })
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

                    // 3. Get bookmarks and join with latest resolution
                    let mut stmt = conn
                        .prepare(
                            "SELECT b.id, b.file_path, r.line_range, r.headline, r.snapshot, b.language, b.query, b.health, r.breadcrumbs
                     FROM collection_bookmarks cb
                     JOIN bookmarks b ON cb.bookmark_id = b.id
                     LEFT JOIN resolutions r ON r.id = (
                         SELECT id FROM resolutions
                         WHERE bookmark_id = b.id
                         ORDER BY resolved_at DESC LIMIT 1
                     )
                     WHERE cb.collection_id = ?1
                     ORDER BY cb.position ASC",
                        )
                        .map_err(|e| {
                            tracing::error!("Failed to prepare bookmarks query: {:?}", e);
                            StatusCode::INTERNAL_SERVER_ERROR
                        })?;

                    let bookmarks_data = stmt
                        .query_map([&id_clone], |row| {
                            Ok((
                                row.get::<_, String>(0)?, // id
                                row.get::<_, String>(1)?, // file_path
                                row.get::<_, Option<String>>(2)?, // line_range
                                row.get::<_, Option<String>>(3)?, // headline
                                row.get::<_, Option<String>>(4)?, // snapshot (was preview_lines)
                                row.get::<_, String>(5)?, // language
                                row.get::<_, String>(6)?, // query
                                row.get::<_, String>(7)?, // health
                                row.get::<_, Option<String>>(8)?, // breadcrumbs
                            ))
                        })
                        .map_err(|e| {
                            tracing::error!("Failed to execute bookmarks query: {:?}", e);
                            StatusCode::INTERNAL_SERVER_ERROR
                        })?
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(|e| {
                            tracing::error!("Failed to collect bookmarks data: {:?}", e);
                            StatusCode::INTERNAL_SERVER_ERROR
                        })?;

                    let mut bookmarks = Vec::new();
                    for (i, (bid, file, range, head, snapshot, lang, q, b_health, bcs)) in bookmarks_data.into_iter().enumerate() {
                        let bid: String = bid;
                        let file: String = file;
                        let range: Option<String> = range;
                        let head: Option<String> = head;
                        let snapshot: Option<String> = snapshot;
                        let lang: String = lang;
                        let q: String = q;
                        let b_health: String = b_health;
                        let bcs: Option<String> = bcs;

                        let tags: Vec<String> = conn.prepare("SELECT tag FROM bookmark_tags WHERE bookmark_id = ?")
                            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
                            .query_map([&bid], |row| row.get(0))
                            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
                            .collect::<rusqlite::Result<Vec<String>>>()
                            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

                        let comments: Vec<CommentView> = conn.prepare("SELECT author, body, created_at FROM bookmark_comments WHERE bookmark_id = ? ORDER BY created_at ASC")
                            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
                            .query_map([&bid], |row| {
                                let author: String = row.get(0)?;
                                Ok(CommentView {
                                    author_initial: author.chars().next().unwrap_or('?').to_uppercase().to_string(),
                                    author,
                                    body: row.get(1)?,
                                    created_at_relative: row.get(2)?, // TODO: relative
                                })
                            })
                            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
                            .collect::<rusqlite::Result<Vec<_>>>()
                            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

                        let breadcrumbs: Vec<codemark_core::engine::breadcrumbs::Breadcrumb> = bcs.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();

                        bookmarks.push((bid, file, range, head, snapshot, String::new(), lang, q, tags, comments, i + 1, b_health, breadcrumbs));
                    }

                    Ok::<(String, String, Option<String>, Option<String>, String, Option<String>, Option<String>, Option<String>, Vec<LinkView>, Vec<_>), StatusCode>((
                        coll_id, name, description, repo_url, published_at, author, health, health_computed_at, links, bookmarks
                    ))
                })
                .await;

            match result {
                Ok(Ok((
                    coll_id,
                    name,
                    description,
                    repo_url,
                    published_at,
                    author,
                    health,
                    health_computed_at,
                    links,
                    bookmarks_data,
                ))) => {
                    if format == ResponseFormat::Json {
                        let bookmarks = bookmarks_data
                            .into_iter()
                            .map(|(bid, file, range, head, snapshot, _, _, _, _, _, _, _, breadcrumbs)| {
                                let sticky_lines: Vec<String> =
                                    breadcrumbs.iter().map(|b| b.text.clone()).collect();
                                let sticky_line_numbers: Vec<usize> =
                                    breadcrumbs.iter().map(|b| b.line).collect();

                                let snapshot_data =
                                    if head.is_some() || snapshot.is_some() || !sticky_lines.is_empty() {
                                        Some(ResolutionSnapshot {
                                            headline: head,
                                            snapshot,
                                            sticky_lines,
                                            sticky_line_numbers,
                                        })
                                    } else {
                                        None
                                    };
                                BookmarkDetail {
                                    id: bid,
                                    file_path: file,
                                    line_range: range,
                                    snapshot: snapshot_data,
                                }
                            })
                            .collect();
                        (
                            StatusCode::OK,
                            Json(TourDetail {
                                tour_id: coll_id,
                                title: name,
                                description,
                                repo_url,
                                published_at,
                                bookmarks,
                            }),
                        )
                            .into_response()
                    } else {
                        let (health_class, health_label) = match health.as_deref() {
                            Some("active") => ("healthy", "Ready"),
                            Some("drifted") => ("drifted", "Drifted"),
                            Some("stale") => ("stale", "Stale"),
                            _ => ("healthy", "Ready"),
                        };

                        let is_drifted = health.as_deref() == Some("drifted")
                            || health.as_deref() == Some("stale");

                        let bookmarks = bookmarks_data
                            .into_iter()
                            .map(
                                |(
                                    bid,
                                    file,
                                    range,
                                    head,
                                    snapshot,
                                    notes,
                                    lang,
                                    q,
                                    tags,
                                    comments,
                                    ordinal,
                                    b_health,
                                    breadcrumbs,
                                )| {
                                    let (target_start, target_end) = if let Some(r) = &range {
                                        let mut parts = r.split('-');
                                        let start = parts.next().and_then(|s| s.parse::<usize>().ok()).unwrap_or(1);
                                        let end = parts.next().and_then(|s| s.parse::<usize>().ok()).unwrap_or(start);
                                        (start, end)
                                    } else {
                                        (1, 1)
                                    };

                                    let raw_preview = snapshot.unwrap_or_default();

                                    // Calculate target range relative to the snippet (usually 5 lines of context above)
                                    let snippet_start = target_start.saturating_sub(5).max(1);
                                    let rel_start = target_start - snippet_start + 1;
                                    let rel_end = target_end - snippet_start + 1;
                                    let target_range_in_snippet = Some((rel_start, rel_end));

                                    let highlighted =
                                        (*crate::highlight::highlight(&lang, &raw_preview, target_range_in_snippet)).clone();

                                    // Highlight the query using lisp syntax
                                    let query_highlighted = if !q.is_empty() {
                                        Some((*crate::highlight::highlight("lisp", &q, None)).clone())
                                    } else {
                                        None
                                    };

                                    let b_health_class = match b_health.as_str() {
                                        "active" => "healthy",
                                        "drifted" => "drifted",
                                        "stale" => "stale",
                                        "archived" => "archived",
                                        _ => "healthy",
                                    }
                                    .to_string();

                                    // Build highlighted code with sticky lines prepended
                                    // The sticky lines are the breadcrumb lines shown at the top
                                    let sticky_line_numbers: Vec<usize> = breadcrumbs.iter().map(|bc| bc.line).collect();
                                    let mut highlighted_with_sticky = String::new();

                                    // Highlight and prepend each sticky line
                                    for bc in &breadcrumbs {
                                        let sticky_html = (*crate::highlight::highlight(&lang, &bc.text, None)).clone();
                                        highlighted_with_sticky.push_str(&sticky_html);
                                    }

                                    // Append the original highlighted code
                                    highlighted_with_sticky.push_str(&highlighted);

                                    BookmarkView {
                                        id_short: bid[..8].to_string(),
                                        id: bid,
                                        ordinal,
                                        headline: head.unwrap_or_else(|| "No headline".to_string()),
                                        file_path: file,
                                        line_range: range.unwrap_or_else(|| "L1".to_string()),
                                        line: target_start,
                                        health: b_health,
                                        health_class: b_health_class,
                                        note: notes,
                                        language: lang,
                                        preview_lines: raw_preview,
                                        highlighted,
                                        highlighted_with_sticky,
                                        sticky_line_numbers,
                                        has_query: !q.is_empty(),
                                        query: if q.is_empty() { None } else { Some(q) },
                                        query_highlighted,
                                        tags,
                                        comment_count: comments.len(),
                                        comments,
                                        has_notes: false, // TODO: bookmark_annotations check
                                        breadcrumbs,
                                        snippet_start,
                                    }
                                },
                            )
                            .collect();

                        let template = TourDetailTemplate {
                            nav: NavItem::Tours,
                            tour: TourDetailView {
                                id: coll_id,
                                title: name,
                                description,
                                author: author.unwrap_or_else(|| "anonymous".to_string()),
                                published_at_relative: published_at, // TODO: relative
                                health: health.clone(),
                                health_class: health_class.to_string(),
                                health_label: health_label.to_string(),
                                health_computed_at,
                                is_drifted,
                                bookmarks,
                                links,
                            },
                            host,
                        };
                        template.into_response()
                    }
                }
                Ok(Err(status)) => status.into_response(),
                Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
    }
}
