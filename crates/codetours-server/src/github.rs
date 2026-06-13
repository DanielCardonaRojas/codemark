//! GitHub API integration for repository access verification.

use crate::router::AppState;
use rusqlite::OptionalExtension;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Error type for GitHub verification operations.
#[derive(Debug, thiserror::Error)]
pub enum GitHubVerifyError {
    /// The user has no GitHub token linked to their account.
    #[error("No GitHub token linked to user account")]
    NoGitHubToken,

    /// The repository URL is invalid.
    #[error("Invalid repository URL: {0}")]
    InvalidRepoUrl(String),

    /// Failed to query the registry for user information.
    #[error("Failed to query user registry: {0}")]
    RegistryQuery(String),

    /// GitHub API request failed.
    #[error("GitHub API error: {0}")]
    GitHubApi(String),

    /// Internal error.
    #[error("Internal error: {0}")]
    Internal(String),
}

/// Result type for GitHub verification operations.
pub type VerifyResult<T> = std::result::Result<T, GitHubVerifyError>;

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
    ) -> VerifyResult<bool> {
        let (owner, repo) = Self::parse_repo_url(repo_url)
            .ok_or_else(|| GitHubVerifyError::InvalidRepoUrl(repo_url.to_string()))?;

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
        let registry_conn = state.registry.get_conn().await.map_err(|e| {
            GitHubVerifyError::RegistryQuery(format!("Failed to get registry connection: {}", e))
        })?;
        let user_info = registry_conn
            .interact(move |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT id, github_token FROM users
                     WHERE id = ?1 AND github_token IS NOT NULL",
                    )
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;

                stmt.query_row([&user_id], |row| {
                    let id: String = row.get(0)?;
                    let token: String = row.get(1)?;
                    Ok((id, token))
                })
                .optional()
            })
            .await
            .map_err(|e| {
                GitHubVerifyError::RegistryQuery(format!("Failed to execute query: {}", e))
            })?
            .map_err(|e| {
                GitHubVerifyError::RegistryQuery(format!("Registry interact failed: {}", e))
            })?;

        let (user_id_from_db, github_token) = user_info.ok_or(GitHubVerifyError::NoGitHubToken)?;

        // Verify access via GitHub API
        let client = reqwest::Client::new();
        let api_url = format!("https://api.github.com/repos/{}/{}", owner, repo);

        let resp = client
            .get(&api_url)
            .header("Authorization", format!("Bearer {}", github_token))
            .header("User-Agent", "codetours-server")
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .map_err(|e| {
                GitHubVerifyError::GitHubApi(format!("Failed to call GitHub API: {}", e))
            })?;

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
            target: "codemark::auth",
            user_id = %user_id_from_db,
            repo_url = %repo_url,
            has_access = %has_access,
            "GitHub access check completed"
        );

        Ok(has_access)
    }

    /// Verify that a repository is **public**, using an unauthenticated
    /// `GET /repos/{owner}/{repo}` call.
    ///
    /// This is the shared repo-visibility check from the abuse-protection model:
    /// public repo metadata is anonymously readable, so a `200` means the repo is
    /// public; a `404` (what GitHub returns for private/nonexistent repos to an
    /// unauthenticated caller) means it is not visible. No user token is used, so
    /// the result is cached per `(owner, repo)` and shared across all anonymous
    /// callers.
    ///
    /// Returns `Ok(true)` if the repo is public, `Ok(false)` otherwise. On a
    /// network/transport error it fails closed (`Ok(false)`): we never reveal a
    /// repo we could not confirm is public.
    ///
    /// Only **authoritative** answers are cached — `200` (public) and `404`
    /// (private/nonexistent). Transient responses (rate-limit `429`, `5xx`) and
    /// transport errors fail closed for the current request but are **not**
    /// cached, so a blip doesn't hide a genuinely-public repo's tours for the
    /// whole TTL; the next request retries.
    pub async fn verify_public_repo(&self, owner: &str, repo: &str) -> VerifyResult<bool> {
        // Cache key namespaced so it can't collide with per-user access entries.
        let cache_key = format!("anon:public:{}", owner);
        {
            let cache = self.cache.read().await;
            if let Some(entry) = cache.get(&(cache_key.clone(), repo.to_string()))
                && entry.expires_at > chrono::Utc::now()
            {
                return Ok(entry.has_access);
            }
        }

        let client = reqwest::Client::new();
        let api_url = format!("https://api.github.com/repos/{}/{}", owner, repo);

        // (is_public, authoritative): only authoritative results are cached.
        let (is_public, authoritative) = match client
            .get(&api_url)
            .header("User-Agent", "codetours-server")
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
        {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    (true, true)
                } else if status == reqwest::StatusCode::NOT_FOUND {
                    (false, true)
                } else {
                    // 429 / 5xx / etc.: non-authoritative. Fail closed, don't cache.
                    tracing::warn!(
                        target: "codemark::auth",
                        owner = %owner,
                        repo = %repo,
                        status = %status,
                        "Public-repo check got a non-authoritative status; not caching"
                    );
                    (false, false)
                }
            }
            Err(e) => {
                // Fail closed: an unreachable GitHub must not expose a repo, and
                // must not poison the cache with a transient failure.
                tracing::warn!(
                    target: "codemark::auth",
                    owner = %owner,
                    repo = %repo,
                    error = %e,
                    "Public-repo visibility check failed; treating repo as not public"
                );
                (false, false)
            }
        };

        if authoritative {
            let mut cache = self.cache.write().await;
            cache.insert(
                (cache_key, repo.to_string()),
                CacheEntry {
                    has_access: is_public,
                    expires_at: chrono::Utc::now()
                        + chrono::Duration::seconds(self.cache_ttl_seconds),
                },
            );
        }

        tracing::debug!(target: "codemark::auth", owner = %owner, repo = %repo, is_public, "Public-repo visibility check completed");

        Ok(is_public)
    }

    /// Verify that the user has write access (push or admin) to the repository.
    ///
    /// Returns Ok(true) if write access is granted, Ok(false) if denied.
    pub async fn verify_write_access(
        &self,
        state: &AppState,
        repo_url: &str,
        user_id: &str,
    ) -> VerifyResult<bool> {
        let (owner, repo) = Self::parse_repo_url(repo_url)
            .ok_or_else(|| GitHubVerifyError::InvalidRepoUrl(repo_url.to_string()))?;

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
        let registry_conn = state.registry.get_conn().await.map_err(|e| {
            GitHubVerifyError::RegistryQuery(format!("Failed to get registry connection: {}", e))
        })?;
        let user_id_for_log = user_id.clone();
        let github_token = registry_conn
            .interact(move |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT github_token FROM users
                     WHERE id = ?1 AND github_token IS NOT NULL",
                    )
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;

                stmt.query_row([&user_id], |row| {
                    let token: String = row.get(0)?;
                    Ok(token)
                })
                .optional()
            })
            .await
            .map_err(|e| {
                GitHubVerifyError::RegistryQuery(format!("Failed to execute query: {}", e))
            })?
            .map_err(|e| {
                GitHubVerifyError::RegistryQuery(format!("Registry interact failed: {}", e))
            })?
            .ok_or(GitHubVerifyError::NoGitHubToken)?;

        // Verify write access via GitHub API
        let client = reqwest::Client::new();
        let api_url = format!("https://api.github.com/repos/{}/{}", owner, repo);

        let resp = client
            .get(&api_url)
            .header("Authorization", format!("Bearer {}", github_token))
            .header("User-Agent", "codetours-server")
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .map_err(|e| {
                GitHubVerifyError::GitHubApi(format!("Failed to call GitHub API: {}", e))
            })?;

        if !resp.status().is_success() {
            // Cache the denial
            let mut cache = self.cache.write().await;
            cache.insert(
                (cache_key, repo.clone()),
                CacheEntry {
                    has_access: false,
                    expires_at: chrono::Utc::now()
                        + chrono::Duration::seconds(self.cache_ttl_seconds),
                },
            );
            return Ok(false);
        }

        let repo_info: GitHubRepo = resp.json().await.map_err(|e| {
            GitHubVerifyError::GitHubApi(format!("Failed to parse GitHub response: {}", e))
        })?;
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
            target: "codemark::auth",
            user_id = %user_id_for_log,
            repo_url = %repo_url,
            has_write_access = %has_write,
            "GitHub write access check completed"
        );

        Ok(has_write)
    }

    /// Seed the public-repo visibility cache with a known result.
    ///
    /// Primarily for tests and offline/dev setups: lets callers avoid a live
    /// GitHub call by pre-populating the same cache entry `verify_public_repo`
    /// reads. The entry uses the configured TTL.
    pub async fn seed_public_repo(&self, owner: &str, repo: &str, is_public: bool) {
        let cache_key = format!("anon:public:{}", owner);
        let mut cache = self.cache.write().await;
        cache.insert(
            (cache_key, repo.to_string()),
            CacheEntry {
                has_access: is_public,
                expires_at: chrono::Utc::now() + chrono::Duration::seconds(self.cache_ttl_seconds),
            },
        );
    }

    /// Seed the per-`(user, repo)` read-access cache with a known result.
    ///
    /// Companion to [`Self::seed_public_repo`] for the authenticated path: lets
    /// tests pre-populate the same entry `verify_access` reads.
    pub async fn seed_read_access(&self, user_id: &str, owner: &str, repo: &str, has_access: bool) {
        let cache_key = format!("{}:{}:read", user_id, owner);
        let mut cache = self.cache.write().await;
        cache.insert(
            (cache_key, repo.to_string()),
            CacheEntry {
                has_access,
                expires_at: chrono::Utc::now() + chrono::Duration::seconds(self.cache_ttl_seconds),
            },
        );
    }

    /// Seed the per-`(user, repo)` write-access cache with a known result.
    ///
    /// Companion to [`Self::seed_read_access`] for the publish path.
    pub async fn seed_write_access(
        &self,
        user_id: &str,
        owner: &str,
        repo: &str,
        has_access: bool,
    ) {
        let cache_key = format!("{}:{}:write", user_id, owner);
        let mut cache = self.cache.write().await;
        cache.insert(
            (cache_key, repo.to_string()),
            CacheEntry {
                has_access,
                expires_at: chrono::Utc::now() + chrono::Duration::seconds(self.cache_ttl_seconds),
            },
        );
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
