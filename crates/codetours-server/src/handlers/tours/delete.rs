use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use crate::auth::AuthContext;
use crate::router::AppState;
use crate::pack_cache::PackCache;

pub async fn handler(
    State(state): State<AppState>,
    auth: AuthContext,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if !auth.is_authenticated() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let storage = state.storage.clone();
    let id_clone = id.clone();
    
    let result = storage.get_conn().await.unwrap().interact(move |conn| {
        let tx = conn.transaction()?;

        // 1. Check if it exists
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM collections WHERE id = ?1)",
            [&id_clone],
            |row| row.get(0)
        )?;

        if !exists {
            return Ok::<_, rusqlite::Error>(false);
        }

        // 2. Capture orphan candidates (bookmark IDs from this collection)
        // We'll use a temp table to avoid borrow checker issues with stmt
        tx.execute("CREATE TEMP TABLE _delete_candidates AS SELECT bookmark_id FROM collection_bookmarks WHERE collection_id = ?1", [&id_clone])?;

        // 3. Delete membership rows
        tx.execute("DELETE FROM collection_bookmarks WHERE collection_id = ?1", [&id_clone])?;

        // 4. Delete collection row
        tx.execute("DELETE FROM collections WHERE id = ?1", [&id_clone])?;

        // 5. Delete orphan bookmarks
        tx.execute(
            "DELETE FROM bookmarks 
             WHERE id IN (SELECT bookmark_id FROM _delete_candidates)
             AND id NOT IN (SELECT bookmark_id FROM collection_bookmarks)",
            []
        )?;

        tx.execute("DROP TABLE _delete_candidates", [])?;
        tx.commit()?;
        Ok(true)
    }).await;

    match result {
        Ok(Ok(true)) => {
            // Delete from cache
            let cache = PackCache::new(state.config.data_dir.clone());
            let _ = cache.delete_pack(&id).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(Ok(false)) => StatusCode::NOT_FOUND.into_response(),
        _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
