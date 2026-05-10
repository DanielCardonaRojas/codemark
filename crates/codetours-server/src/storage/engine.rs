//! Storage engine for scatter-gather queries across multiple repository databases.

use super::registry_client::{create_repo_pool, RegistryClient};
use anyhow::{Context, Result};
use deadpool_sqlite::Pool;
use futures_util::stream::{FuturesUnordered, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Storage engine that manages connections to multiple repository databases.
#[derive(Clone)]
pub struct StorageEngine {
    /// Registry client for discovering repositories
    registry_client: RegistryClient,
    /// Connection pools for each repository database
    /// Maps repo_root to pool
    pools: Arc<RwLock<HashMap<String, RepoPool>>>,
}

/// A connection pool for a single repository.
struct RepoPool {
    db_path: std::path::PathBuf,
    pool: Pool,
    repo_owner: String,
    repo_name: String,
    origin_url: Option<String>,
}

/// A collection (tour) from a repository database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionEntry {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub repo_url: Option<String>,
    pub updated_at: String,
    pub created_by: Option<String>,
    pub created_branch: Option<String>,
    pub health: Option<String>,
    pub repo_root: String,
    pub repo_owner: String,
    pub repo_name: String,
}

/// Filter parameters for querying collections.
#[derive(Debug, Clone, Default)]
pub struct CollectionFilter {
    pub q: Option<String>,
    pub repo_url: Option<String>,
    pub branch: Option<String>,
    pub tag: Option<String>,
    pub visibility: Option<String>, // 'public', 'private', etc.
    pub status: Option<String>,     // 'ready', etc.
}

/// Paginated query result.
#[derive(Debug, Clone)]
pub struct PaginatedResult<T> {
    pub items: Vec<T>,
    pub total: usize,
    pub limit: usize,
    pub offset: usize,
}

impl StorageEngine {
    /// Create a new storage engine.
    pub fn new(registry_client: RegistryClient) -> Self {
        Self {
            registry_client,
            pools: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Refresh the list of repositories and their connection pools.
    pub async fn refresh(&self) -> Result<()> {
        // Refresh the registry
        self.registry_client.refresh().await?;

        // Get the list of known repositories
        let known_repos = self.registry_client.known_repos().await;

        tracing::info!("StorageEngine::refresh: got {} known repos from registry", known_repos.len());

        // Build new pools map
        let mut new_pools = HashMap::new();

        for entry in known_repos {
            let repo_root = entry.repo_root.to_string_lossy().to_string();
            let repo_owner = entry.repo_owner.clone();
            let repo_name = entry.repo_name.clone();

            tracing::info!("Creating pool for {}/{} at {:?}", repo_owner, repo_name, entry.db_path);

            match create_repo_pool(entry.db_path.clone()) {
                Ok(pool) => {
                    new_pools.insert(
                        repo_root.clone(),
                        RepoPool {
                            db_path: entry.db_path,
                            pool,
                            repo_owner: entry.repo_owner,
                            repo_name: entry.repo_name,
                            origin_url: entry.origin_url,
                        },
                    );
                    tracing::info!("Successfully created pool for {}/{}", repo_owner, repo_name);
                }
                Err(e) => {
                    tracing::warn!("Failed to create pool for {}: {}", repo_root, e);
                }
            }
        }

        *self.pools.write().await = new_pools;
        tracing::info!("Storage engine now managing {} repositories", self.pools.read().await.len());

        Ok(())
    }

    /// Query collections across all repositories (scatter-gather).
    pub async fn query_all_collections(
        &self,
        filter: CollectionFilter,
        limit: usize,
        offset: usize,
        sort: Option<&str>,
    ) -> Result<PaginatedResult<CollectionEntry>> {
        let pools = self.pools.read().await;
        tracing::info!("query_all_collections: {} pools to query, filter: {:?}", pools.len(), filter);

        // Scatter: Query all repositories concurrently
        let mut tasks = FuturesUnordered::new();

        for (repo_root, repo_pool) in pools.iter() {
            let filter = filter.clone();
            let pool = repo_pool.pool.clone();
            let repo_root = repo_root.clone();
            let repo_owner = repo_pool.repo_owner.clone();
            let repo_name = repo_pool.repo_name.clone();
            let origin_url = repo_pool.origin_url.clone();

            tracing::info!("Queueing query for repo: {}/{}", repo_owner, repo_name);

            tasks.push(tokio::task::spawn_blocking(move || {
                Self::query_single_repo(pool, filter, repo_root, repo_owner, repo_name, origin_url)
            }));
        }

        // Gather: Collect all results
        let mut all_collections = Vec::new();

        while let Some(result) = tasks.next().await {
            match result {
                Ok(Ok(entries)) => {
                    tracing::info!("Got {} collections from query", entries.len());
                    all_collections.extend(entries);
                }
                Ok(Err(e)) => {
                    tracing::warn!("Error querying repository: {:?}", e);
                }
                Err(e) => {
                    tracing::warn!("Task join error: {:?}", e);
                }
            }
        }

        tracing::info!("query_all_collections: total {} collections gathered", all_collections.len());

        // Apply sorting
        let sort_order = sort.unwrap_or("updated_at_desc");
        Self::sort_collections(&mut all_collections, sort_order);

        // Calculate total before pagination
        let total = all_collections.len();

        // Apply pagination
        let offset = offset.min(total);
        let limit = limit.min(total - offset);
        let items = all_collections.into_iter().skip(offset).take(limit).collect();

        Ok(PaginatedResult {
            items,
            total,
            limit,
            offset,
        })
    }

    /// Find a collection by ID across all repositories.
    pub async fn find_collection_by_id(&self, id: &str) -> Result<Option<CollectionEntry>> {
        let pools = self.pools.read().await;
        tracing::info!("find_collection_by_id: looking for id={} across {} pools", id, pools.len());

        // Query all repositories concurrently
        let mut tasks = FuturesUnordered::new();

        for (repo_root, repo_pool) in pools.iter() {
            let id = id.to_string();
            let pool = repo_pool.pool.clone();
            let repo_root = repo_root.clone();
            let repo_owner = repo_pool.repo_owner.clone();
            let repo_name = repo_pool.repo_name.clone();
            let origin_url = repo_pool.origin_url.clone();

            tasks.push(tokio::task::spawn_blocking(move || {
                Self::find_by_id_in_repo(pool, id.clone(), repo_root, repo_owner, repo_name, origin_url)
            }));
        }

        // Return the first match found
        while let Some(result) = tasks.next().await {
            match result {
                Ok(Ok(Some(entry))) => {
                    tracing::info!("find_collection_by_id: found collection {} in repo {}/{}", entry.id, entry.repo_owner, entry.repo_name);
                    return Ok(Some(entry));
                }
                Ok(Ok(None)) => continue,
                Ok(Err(e)) => {
                    tracing::warn!("Error querying repository: {:?}", e);
                }
                Err(e) => {
                    tracing::warn!("Task failed: {:?}", e);
                }
            }
        }

        tracing::warn!("find_collection_by_id: collection {} not found in any repository", id);
        Ok(None)
    }

    /// Get connection pool for a specific repository root.
    pub async fn get_pool_for_repo(&self, repo_root: &str) -> Option<Pool> {
        self.pools.read().await.get(repo_root).map(|p| p.pool.clone())
    }

    /// Get all repository roots managed by this engine.
    pub async fn repo_roots(&self) -> Vec<String> {
        self.pools.read().await.keys().cloned().collect()
    }

    /// Get the count of managed repositories.
    pub async fn repo_count(&self) -> usize {
        self.pools.read().await.len()
    }

    /// Get all managed repositories for filter dropdowns.
    /// Returns a list of (repo_owner, repo_name, origin_url) tuples.
    pub async fn all_repos(&self) -> Vec<(String, String, Option<String>)> {
        let pools = self.pools.read().await;
        tracing::info!("all_repos() called, pools.len() = {}", pools.len());
        pools.iter().map(|(_, p)| {
            tracing::info!("Returning repo: {}/{} (origin: {:?})", p.repo_owner, p.repo_name, p.origin_url);
            (p.repo_owner.clone(), p.repo_name.clone(), p.origin_url.clone())
        }).collect()
    }

    /// Query a single repository database.
    fn query_single_repo(
        pool: Pool,
        filter: CollectionFilter,
        repo_root: String,
        repo_owner: String,
        repo_name: String,
        origin_url: Option<String>,
    ) -> Result<Vec<CollectionEntry>> {
        // Use block_in_place to run the async pool.get() in a blocking context
        let conn = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(pool.get())
        })
        .context("Failed to get connection from pool")?;

        // Use block_in_place again for the interact call
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(conn.interact(move |conn| {
                // Build query
                let mut query_str = "
                    SELECT c.id, c.name, c.description, c.repo_url, c.updated_at, c.created_by, c.created_branch, c.health
                    FROM collections c
                    WHERE 1=1
                ".to_string();

                let mut where_clauses = Vec::new();
                let mut sql_params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

                if let Some(visibility) = &filter.visibility {
                    where_clauses.push("c.visibility = ?");
                    sql_params.push(Box::new(visibility.clone()));
                }

                if let Some(status) = &filter.status {
                    where_clauses.push("c.status = ?");
                    sql_params.push(Box::new(status.clone()));
                }

                if let Some(q) = &filter.q {
                    where_clauses.push("(c.name LIKE ? OR c.description LIKE ?)");
                    let pattern = format!("%{}%", q);
                    sql_params.push(Box::new(pattern.clone()));
                    sql_params.push(Box::new(pattern));
                }

                if let Some(repo) = &filter.repo_url {
                    where_clauses.push("c.repo_url = ?");
                    sql_params.push(Box::new(repo.clone()));
                }

                if let Some(branch) = &filter.branch {
                    where_clauses.push("c.created_branch = ?");
                    sql_params.push(Box::new(branch.clone()));
                }

                if let Some(tag) = &filter.tag {
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

                let mut stmt = conn.prepare(&query_str)?;
                let rows = stmt.query_map(rusqlite::params_from_iter(sql_params), |row| {
                    Ok((
                        row.get::<_, String>(0)?, // id
                        row.get::<_, String>(1)?, // name
                        row.get::<_, Option<String>>(2)?, // description
                        row.get::<_, Option<String>>(3)?, // repo_url
                        row.get::<_, Option<String>>(4)?, // updated_at - can be NULL
                        row.get::<_, Option<String>>(5)?, // created_by
                        row.get::<_, Option<String>>(6)?, // created_branch
                        row.get::<_, Option<String>>(7)?, // health
                    ))
                })?;

                let mut entries = Vec::new();
                for row in rows {
                    let (id, name, description, repo_url, updated_at, created_by, created_branch, health) = row?;
                    // Use created_at as fallback if updated_at is NULL
                    let display_date = updated_at.unwrap_or_else(|| "".to_string());
                    entries.push(CollectionEntry {
                        id,
                        name,
                        description,
                        repo_url: repo_url.or_else(|| origin_url.clone()),
                        updated_at: display_date,
                        created_by,
                        created_branch,
                        health,
                        repo_root: repo_root.clone(),
                        repo_owner: repo_owner.clone(),
                        repo_name: repo_name.clone(),
                    });
                }

                Ok::<Vec<CollectionEntry>, rusqlite::Error>(entries)
            }))
            .map_err(|e| anyhow::anyhow!("Interaction error: {}", e))?
            .context("Database query failed")
        })
    }

    /// Find a collection by ID in a single repository.
    fn find_by_id_in_repo(
        pool: Pool,
        id: String,  // Changed from &str to String to avoid lifetime issues
        repo_root: String,
        repo_owner: String,
        repo_name: String,
        origin_url: Option<String>,
    ) -> Result<Option<CollectionEntry>> {
        // Use block_in_place to run the async pool.get() in a blocking context
        let conn = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(pool.get())
        })
        .context("Failed to get connection from pool")?;

        // Use block_in_place again for the interact call
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(conn.interact(move |conn| {
                let query_str = "
                    SELECT c.id, c.name, c.description, c.repo_url, c.updated_at, c.created_by, c.created_branch, c.health
                    FROM collections c
                    WHERE c.id = ?1
                ";

                let result = conn.query_row(query_str, [&id], |row| {
                    Ok((
                        row.get::<_, String>(0)?, // id
                        row.get::<_, String>(1)?, // name
                        row.get::<_, Option<String>>(2)?, // description
                        row.get::<_, Option<String>>(3)?, // repo_url
                        row.get::<_, Option<String>>(4)?, // updated_at - can be NULL
                        row.get::<_, Option<String>>(5)?, // created_by
                        row.get::<_, Option<String>>(6)?, // created_branch
                        row.get::<_, Option<String>>(7)?, // health
                    ))
                });

                match result {
                    Ok((id, name, description, repo_url, updated_at, created_by, created_branch, health)) => {
                        // Use empty string as fallback for updated_at
                        let display_date = updated_at.unwrap_or_else(|| "".to_string());
                        Ok(Some(CollectionEntry {
                            id,
                            name,
                            description,
                            repo_url: repo_url.or_else(|| origin_url),
                            updated_at: display_date,
                            created_by,
                            created_branch,
                            health,
                            repo_root,
                            repo_owner,
                            repo_name,
                        }))
                    }
                    Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                    Err(e) => Err(e),
                }
            }))
            .map_err(|e| anyhow::anyhow!("Interaction error: {}", e))?
            .context("Database query failed")
        })
    }

    /// Sort collections by the specified order.
    fn sort_collections(collections: &mut Vec<CollectionEntry>, sort: &str) {
        match sort {
            "updated_at_asc" => {
                collections.sort_by(|a, b| a.updated_at.cmp(&b.updated_at));
            }
            "title_asc" => {
                collections.sort_by(|a, b| a.name.cmp(&b.name));
            }
            _ => {
                // Default: updated_at_desc
                collections.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collection_filter_default() {
        let filter = CollectionFilter::default();
        assert!(filter.q.is_none());
        assert!(filter.repo_url.is_none());
        assert!(filter.branch.is_none());
        assert!(filter.tag.is_none());
    }

    #[test]
    fn test_paginated_result() {
        let items = vec![1, 2, 3];
        let result = PaginatedResult {
            items: items.clone(),
            total: 10,
            limit: 3,
            offset: 0,
        };
        assert_eq!(result.items, items);
        assert_eq!(result.total, 10);
        assert_eq!(result.limit, 3);
        assert_eq!(result.offset, 0);
    }
}
