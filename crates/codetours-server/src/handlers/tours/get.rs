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
use crate::web::{filters, NavItem};

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
    pub preview_lines: Option<String>,
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
    pub is_drifted: bool,
    pub bookmarks: Vec<BookmarkView>,
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
    pub note: String,
    pub language: String,
    pub preview_lines: String,
    pub highlighted: String,
    pub has_query: bool,
    pub query: Option<String>,
    pub query_highlighted: Option<String>,
    pub tags: Vec<String>,
    pub comment_count: usize,
    pub comments: Vec<CommentView>,
    pub has_notes: bool,
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
                        "SELECT id, name, description, repo_url, published_at, created_by FROM collections 
                     WHERE id = ?1 AND visibility IS NOT NULL"
                    } else {
                        "SELECT id, name, description, repo_url, published_at, created_by FROM collections 
                     WHERE id = ?1 AND visibility = 'public'"
                    };

                    let (coll_id, name, description, repo_url, published_at, author) = conn
                        .query_row(sql, [&id_clone], |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, Option<String>>(2)?,
                                row.get::<_, Option<String>>(3)?,
                                row.get::<_, String>(4)?,
                                row.get::<_, Option<String>>(5)?,
                            ))
                        })
                        .map_err(|e| {
                            tracing::error!("Database error fetching tour: {:?}", e);
                            match e {
                                rusqlite::Error::QueryReturnedNoRows => StatusCode::NOT_FOUND,
                                _ => StatusCode::INTERNAL_SERVER_ERROR,
                            }
                        })?;

                    // 2. Get bookmarks and join with latest resolution
                    let mut stmt = conn
                        .prepare(
                            "SELECT b.id, b.file_path, r.line_range, r.headline, r.preview_lines, b.language, b.query
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
                                row.get::<_, Option<String>>(4)?, // preview_lines
                                row.get::<_, String>(5)?, // language
                                row.get::<_, String>(6)?, // query
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
                    for (i, (bid, file, range, head, preview, lang, q)) in bookmarks_data.into_iter().enumerate() {
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

                        bookmarks.push((bid, file, range, head, preview, String::new(), lang, q, tags, comments, i + 1));
                    }

                    Ok::<(String, String, Option<String>, Option<String>, String, Option<String>, Vec<_>), StatusCode>((
                        coll_id, name, description, repo_url, published_at, author, bookmarks
                    ))
                })
                .await;

            match result {
                Ok(Ok((coll_id, name, description, repo_url, published_at, author, bookmarks_data))) => {
                    if format == ResponseFormat::Json {
                        let bookmarks = bookmarks_data.into_iter().map(|(bid, file, range, head, preview, _, _, _, _, _, _)| {
                            BookmarkDetail {
                                id: bid,
                                file_path: file,
                                line_range: range,
                                snapshot: head.map(|h| ResolutionSnapshot {
                                    headline: Some(h),
                                    preview_lines: preview,
                                }),
                            }
                        }).collect();
                        (StatusCode::OK, Json(TourDetail {
                            tour_id: coll_id,
                            title: name,
                            description,
                            repo_url,
                            published_at,
                            bookmarks,
                        })).into_response()
                    } else {
                        let bookmarks = bookmarks_data.into_iter().map(|(bid, file, range, head, preview, notes, lang, q, tags, comments, ordinal)| {
                            let line_parsed = range.as_ref()
                                .and_then(|r| {
                                    r.split('-')
                                        .next()
                                        .and_then(|s| s.parse::<usize>().ok())
                                })
                                .unwrap_or(1);

                            let raw_preview = preview.unwrap_or_default();
                            let highlighted = (*crate::highlight::highlight(&lang, &raw_preview)).clone();

                            // Highlight the query using lisp syntax (tree-sitter queries use lisp/scheme-like syntax)
                            let query_highlighted = if !q.is_empty() {
                                Some((*crate::highlight::highlight("lisp", &q)).clone())
                            } else {
                                None
                            };

                            BookmarkView {
                                id_short: bid[..8].to_string(),
                                id: bid,
                                ordinal,
                                headline: head.unwrap_or_else(|| "No headline".to_string()),
                                file_path: file,
                                line_range: range.unwrap_or_else(|| "L1".to_string()),
                                line: line_parsed,
                                note: notes,
                                language: lang,
                                preview_lines: raw_preview,
                                highlighted,
                                has_query: !q.is_empty(),
                                query: if q.is_empty() { None } else { Some(q) },
                                query_highlighted,
                                tags,
                                comment_count: comments.len(),
                                comments,
                                has_notes: false, // TODO: bookmark_annotations check
                            }
                        }).collect();

                        let template = TourDetailTemplate {
                            nav: NavItem::Tours,
                            tour: TourDetailView {
                                id: coll_id,
                                title: name,
                                description,
                                author: author.unwrap_or_else(|| "anonymous".to_string()),
                                published_at_relative: published_at, // TODO: relative
                                is_drifted: false, // TODO: drift check
                                bookmarks,
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
