//! GitHub API integration for repository access verification.

use crate::router::AppState;
use anyhow::Result;
use rusqlite::OptionalExtension;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// GitHub repository information from API.
#[derive(Debug, Deserialize)]
pub struct GitHubRepo {
    pub id: u64,
    pub name: String,
    pub full_name: String,
    pub owner: GitHubOwner,
    pub private: bool,
    pub permissions: GitHubPermissions,
}

#[derive(Debug, Deserialize)]
pub struct GitHubOwner {
    pub login: String,
    pub id: u64,
}

#[derive(Debug, Deserialize)]
pub struct GitHubPermissions {
    #[serde(default)]
    pub pull: bool,
    #[serde(default)]
    pub push: bool,
    #[serde(default)]
    pub admin: bool,
}

/// Cache entry for repo access checks.
#[derive(Debug, Clone)]
struct CacheEntry {
    has_access: bool,
    expires_at: chrono::DateTime<chrono::Utc>,
}

/// GitHub access verifier with caching.
pub struct GitHubVerifier {
    /// Cache of repo access checks: (owner, repo) -> CacheEntry
    cache: Arc<RwLock<HashMap<(String, String), CacheEntry>>>,
    /// Cache TTL in seconds (default: 5 minutes)
    cache_ttl_seconds: i64,
}

impl GitHubVerifier {
    /// Create a new verifier with default settings.
    pub fn new() -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            cache_ttl_seconds: 300, // 5 minutes
        }
    }

    /// Set the cache TTL.
    pub fn with_cache_ttl(mut self, seconds: i64) -> Self {
        self.cache_ttl_seconds = seconds;
        self
    }

    /// Parse a GitHub repository URL into (owner, repo).
    ///
    /// Supports:
    /// - https://github.com/owner/repo
    /// - git@github.com:owner/repo.git
    /// - owner/repo
    pub fn parse_repo_url(url: &str) -> Option<(String, String)> {
        let url = url.trim();

        // Handle https://github.com/owner/repo
        if let Some(parts) = url.strip_prefix("https://github.com/") {
            let parts: Vec<&str> = parts.split('/').collect();
            if parts.len() >= 2 {
                let repo = parts[1].strip_suffix(".git").unwrap_or(parts[1]);
                return Some((parts[0].to_string(), repo.to_string()));
            }
        }

        // Handle git@github.com:owner/repo.git
        if let Some(parts) = url.strip_prefix("git@github.com:") {
            let repo = parts.strip_suffix(".git").unwrap_or(parts);
            if let Some((owner, name)) = repo.split_once('/') {
                return Some((owner.to_string(), name.to_string()));
            }
        }

        // Handle owner/repo format (simple case, no .git suffix)
        if let Some((owner, repo)) = url.split_once('/')
            && !owner.contains('.')
            && !owner.contains(':')
        {
            let repo = repo.strip_suffix(".git").unwrap_or(repo);
            return Some((owner.to_string(), repo.to_string()));
        }

        None
    }

    /// Verify that the user has access to the repository.
    ///
    /// Returns Ok(true) if access is granted, Ok(false) if denied.
    pub async fn verify_access(
        &self,
        state: &AppState,
        repo_url: &str,
        user_id: &str,
    ) -> Result<bool, anyhow::Error> {
        let (owner, repo) = Self::parse_repo_url(repo_url)
            .ok_or_else(|| anyhow::anyhow!("Invalid repository URL: {}", repo_url))?;

        // Check cache first (include user_id in cache key)
        let cache_key = format!("{}:{}:read", user_id, owner);
        {
            let cache = self.cache.read().await;
            if let Some(entry) = cache.get(&(cache_key.clone(), repo.clone()))
                && entry.expires_at > chrono::Utc::now()
            {
                return Ok(entry.has_access);
            }
        }

        // Get the user's GitHub token from registry
        let user_id = user_id.to_string();
        let registry_conn = state.registry.get_conn().await?;
        let user_info = registry_conn
            .interact(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, github_token FROM users
                     WHERE id = ?1 AND github_token IS NOT NULL",
                )?;

                stmt.query_row([&user_id], |row| {
                    let id: String = row.get(0)?;
                    let token: String = row.get(1)?;
                    Ok((id, token))
                })
                .optional()
            })
            .await
            .map_err(|e| anyhow::anyhow!("Failed to query user: {}", e))?
            .map_err(|e| anyhow::anyhow!("Failed to execute query: {}", e))?;

        let (user_id_from_db, github_token) = user_info
            .ok_or_else(|| anyhow::anyhow!("No authenticated user with GitHub token found"))?;

        // Verify access via GitHub API
        let client = reqwest::Client::new();
        let api_url = format!("https://api.github.com/repos/{}/{}", owner, repo);

        let resp = client
            .get(&api_url)
            .header("Authorization", format!("Bearer {}", github_token))
            .header("User-Agent", "codetours-server")
            .header("Accept", "application/vnd.github+json")
            .send()
            .await?;

        let has_access = resp.status().is_success();

        // Cache the result
        let mut cache = self.cache.write().await;
        cache.insert(
            (cache_key, repo),
            CacheEntry {
                has_access,
                expires_at: chrono::Utc::now() + chrono::Duration::seconds(self.cache_ttl_seconds),
            },
        );

        tracing::info!(
            user_id = %user_id_from_db,
            repo_url = %repo_url,
            has_access = %has_access,
            "GitHub access check completed"
        );

        Ok(has_access)
    }

    /// Verify that the user has write access (push or admin) to the repository.
    ///
    /// Returns Ok(true) if write access is granted, Ok(false) if denied.
    pub async fn verify_write_access(
        &self,
        state: &AppState,
        repo_url: &str,
        user_id: &str,
    ) -> Result<bool, anyhow::Error> {
        let (owner, repo) = Self::parse_repo_url(repo_url)
            .ok_or_else(|| anyhow::anyhow!("Invalid repository URL: {}", repo_url))?;

        // Check cache first (include user_id in cache key since permissions vary by user)
        let cache_key = format!("{}:{}:write", user_id, owner);
        {
            let cache = self.cache.read().await;
            if let Some(entry) = cache.get(&(cache_key.clone(), repo.clone()))
                && entry.expires_at > chrono::Utc::now()
            {
                return Ok(entry.has_access);
            }
        }

        // Get the user's GitHub token from registry
        let user_id = user_id.to_string();
        let registry_conn = state.registry.get_conn().await?;
        let user_id_for_log = user_id.clone();
        let github_token = registry_conn
            .interact(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT github_token FROM users
                     WHERE id = ?1 AND github_token IS NOT NULL",
                )?;

                stmt.query_row([&user_id], |row| {
                    let token: String = row.get(0)?;
                    Ok(token)
                })
                .optional()
            })
            .await
            .map_err(|e| anyhow::anyhow!("Failed to query user: {}", e))?
            .map_err(|e| anyhow::anyhow!("Failed to execute query: {}", e))?
            .ok_or_else(|| anyhow::anyhow!("User not found or no GitHub token available"))?;

        // Verify write access via GitHub API
        let client = reqwest::Client::new();
        let api_url = format!("https://api.github.com/repos/{}/{}", owner, repo);

        let resp = client
            .get(&api_url)
            .header("Authorization", format!("Bearer {}", github_token))
            .header("User-Agent", "codetours-server")
            .header("Accept", "application/vnd.github+json")
            .send()
            .await?;

        if !resp.status().is_success() {
            // Cache the denial
            let mut cache = self.cache.write().await;
            cache.insert(
                (cache_key, repo.clone()),
                CacheEntry {
                    has_access: false,
                    expires_at: chrono::Utc::now() + chrono::Duration::seconds(self.cache_ttl_seconds),
                },
            );
            return Ok(false);
        }

        let repo_info: GitHubRepo = resp.json().await?;
        let has_write = repo_info.permissions.push || repo_info.permissions.admin;

        // Cache the result
        let mut cache = self.cache.write().await;
        cache.insert(
            (cache_key, repo),
            CacheEntry {
                has_access: has_write,
                expires_at: chrono::Utc::now() + chrono::Duration::seconds(self.cache_ttl_seconds),
            },
        );

        tracing::info!(
            user_id = %user_id_for_log,
            repo_url = %repo_url,
            has_write_access = %has_write,
            "GitHub write access check completed"
        );

        Ok(has_write)
    }

    /// Clear expired cache entries.
    pub async fn clean_cache(&self) {
        let mut cache = self.cache.write().await;
        let now = chrono::Utc::now();
        cache.retain(|_, entry| entry.expires_at > now);
    }
}

impl Default for GitHubVerifier {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_repo_url() {
        assert_eq!(
            GitHubVerifier::parse_repo_url("https://github.com/owner/repo"),
            Some(("owner".to_string(), "repo".to_string()))
        );

        assert_eq!(
            GitHubVerifier::parse_repo_url("git@github.com:owner/repo.git"),
            Some(("owner".to_string(), "repo".to_string()))
        );

        assert_eq!(
            GitHubVerifier::parse_repo_url("owner/repo"),
            Some(("owner".to_string(), "repo".to_string()))
        );

        assert_eq!(
            GitHubVerifier::parse_repo_url("https://github.com/owner/repo.git"),
            Some(("owner".to_string(), "repo".to_string()))
        );

        assert_eq!(GitHubVerifier::parse_repo_url("not-a-url"), None);
    }
}
