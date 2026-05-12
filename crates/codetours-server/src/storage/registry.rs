//! Server registry database for managing users, tenants, and OAuth tokens.
//!
//! The registry is stored in `data/registry.sqlite` and serves as the central
//! source of truth for identity, organization, and access permissions.

use crate::error::{Result, ServerError};
use deadpool_sqlite::{Config as PoolConfig, Pool, Runtime};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::PathBuf;

/// Registry database path in the data directory.
pub fn registry_path(data_dir: &std::path::Path) -> PathBuf {
    data_dir.join("registry.sqlite")
}

/// Registry connection pool manager.
pub struct RegistryManager {
    pool: Pool,
}

impl RegistryManager {
    /// Create a new registry manager with connection pool.
    pub fn new(data_dir: &std::path::Path) -> Result<Self> {
        let path = registry_path(data_dir);

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ServerError::Registry(format!("Failed to create registry directory: {}", e))
            })?;
        }

        // Initialize the database file and schema
        let conn = Connection::open(&path).map_err(|e| {
            ServerError::Registry(format!("Failed to open registry database: {}", e))
        })?;

        // Enable WAL mode for better concurrent access
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
            .map_err(|e| ServerError::Registry(format!("Failed to enable pragmas: {}", e)))?;

        // Initialize schema
        init_schema(&conn)?;

        // Build connection pool
        let pool_config = PoolConfig::new(path);
        let pool = pool_config
            .builder(Runtime::Tokio1)
            .map_err(|e| ServerError::Registry(format!("Failed to create builder: {}", e)))?
            .max_size(5) // Small pool for auth operations
            .build()
            .map_err(|e| ServerError::Registry(format!("Failed to build pool: {}", e)))?;

        Ok(Self { pool })
    }

    /// Get a connection from the pool.
    pub async fn get_conn(&self) -> deadpool_sqlite::Object {
        self.pool
            .get()
            .await
            .map_err(|e| ServerError::Registry(format!("Failed to get connection: {}", e)))
            .unwrap()
    }

    /// Get the underlying pool.
    pub fn pool(&self) -> &Pool {
        &self.pool
    }
}

/// Initialize the registry database schema.
fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS users (
            id              TEXT PRIMARY KEY,
            github_id       TEXT UNIQUE NOT NULL,
            github_login    TEXT NOT NULL,
            github_token    TEXT,
            created_at      TEXT NOT NULL,
            last_login_at   TEXT
        );

        CREATE TABLE IF NOT EXISTS refresh_tokens (
            token           TEXT PRIMARY KEY,
            user_id         TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            expires_at      TEXT NOT NULL,
            created_at      TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS tenants (
            id              TEXT PRIMARY KEY,
            slug            TEXT UNIQUE NOT NULL,
            db_path         TEXT NOT NULL,
            github_id       TEXT,
            created_at      TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_refresh_tokens_user ON refresh_tokens(user_id);
        CREATE INDEX IF NOT EXISTS idx_tenants_github ON tenants(github_id);
        ",
    )
    .map_err(|e| ServerError::Registry(format!("Failed to initialize schema: {}", e)))?;

    Ok(())
}

/// User record from the registry.
#[derive(Debug, Clone)]
pub struct User {
    pub id: String,
    pub github_id: String,
    pub github_login: String,
    pub github_token: Option<String>,
    pub created_at: String,
    pub last_login_at: Option<String>,
}

/// Refresh token record from the registry.
#[derive(Debug, Clone)]
pub struct RefreshToken {
    pub token: String,
    pub user_id: String,
    pub expires_at: String,
    pub created_at: String,
}

/// Tenant record from the registry.
#[derive(Debug, Clone)]
pub struct Tenant {
    pub id: String,
    pub slug: String,
    pub db_path: String,
    pub github_id: Option<String>,
    pub created_at: String,
}

/// Builder for creating or updating a user.
pub struct UserUpsert<'a> {
    pub id: &'a str,
    pub github_id: &'a str,
    pub github_login: &'a str,
    pub github_token: Option<&'a str>,
}

/// Create or update a user in the registry.
pub fn upsert_user(conn: &Connection, user: &UserUpsert<'_>) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "INSERT INTO users (id, github_id, github_login, github_token, created_at, last_login_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(github_id) DO UPDATE SET
             id = excluded.id,
             github_login = excluded.github_login,
             github_token = COALESCE(excluded.github_token, users.github_token),
             last_login_at = excluded.last_login_at",
        params![user.id, user.github_id, user.github_login, user.github_token, now, now,],
    )
    .map_err(|e| ServerError::Registry(format!("Failed to upsert user: {}", e)))?;

    Ok(())
}

/// Find a user by their GitHub ID.
pub fn find_user_by_github_id(conn: &Connection, github_id: &str) -> Result<Option<User>> {
    conn.query_row(
        "SELECT id, github_id, github_login, github_token, created_at, last_login_at
         FROM users WHERE github_id = ?1",
        params![github_id],
        row_to_user,
    )
    .optional()
    .map_err(|e| ServerError::Registry(format!("Failed to find user: {}", e)))
}

/// Find a user by their internal ID.
pub fn find_user_by_id(conn: &Connection, id: &str) -> Result<Option<User>> {
    conn.query_row(
        "SELECT id, github_id, github_login, github_token, created_at, last_login_at
         FROM users WHERE id = ?1",
        params![id],
        row_to_user,
    )
    .optional()
    .map_err(|e| ServerError::Registry(format!("Failed to find user: {}", e)))
}

/// Store a refresh token.
pub fn store_refresh_token(conn: &Connection, token: &RefreshToken) -> Result<()> {
    conn.execute(
        "INSERT INTO refresh_tokens (token, user_id, expires_at, created_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![&token.token, &token.user_id, &token.expires_at, &token.created_at,],
    )
    .map_err(|e| ServerError::Registry(format!("Failed to store refresh token: {}", e)))?;

    Ok(())
}

/// Find a refresh token and its associated user.
pub fn find_refresh_token(conn: &Connection, token: &str) -> Result<Option<(RefreshToken, User)>> {
    conn.query_row(
        "SELECT rt.token, rt.user_id, rt.expires_at, rt.created_at,
                u.id, u.github_id, u.github_login, u.github_token, u.created_at, u.last_login_at
         FROM refresh_tokens rt
         JOIN users u ON rt.user_id = u.id
         WHERE rt.token = ?1",
        params![token],
        |row| {
            Ok((
                RefreshToken {
                    token: row.get(0)?,
                    user_id: row.get(1)?,
                    expires_at: row.get(2)?,
                    created_at: row.get(3)?,
                },
                User {
                    id: row.get(4)?,
                    github_id: row.get(5)?,
                    github_login: row.get(6)?,
                    github_token: row.get(7)?,
                    created_at: row.get(8)?,
                    last_login_at: row.get(9)?,
                },
            ))
        },
    )
    .optional()
    .map_err(|e| ServerError::Registry(format!("Failed to find refresh token: {}", e)))
}

/// Delete expired refresh tokens.
pub fn delete_expired_tokens(conn: &Connection) -> Result<usize> {
    let now = chrono::Utc::now().to_rfc3339();
    let count = conn
        .execute("DELETE FROM refresh_tokens WHERE expires_at < ?1", params![now])
        .map_err(|e| ServerError::Registry(format!("Failed to delete expired tokens: {}", e)))?;

    Ok(count)
}

/// Create a new tenant.
pub fn create_tenant(conn: &Connection, tenant: &Tenant) -> Result<()> {
    conn.execute(
        "INSERT INTO tenants (id, slug, db_path, github_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![&tenant.id, &tenant.slug, &tenant.db_path, &tenant.github_id, &tenant.created_at,],
    )
    .map_err(|e| ServerError::Registry(format!("Failed to create tenant: {}", e)))?;

    Ok(())
}

/// Find a tenant by slug.
pub fn find_tenant_by_slug(conn: &Connection, slug: &str) -> Result<Option<Tenant>> {
    conn.query_row(
        "SELECT id, slug, db_path, github_id, created_at
         FROM tenants WHERE slug = ?1",
        params![slug],
        row_to_tenant,
    )
    .optional()
    .map_err(|e| ServerError::Registry(format!("Failed to find tenant: {}", e)))
}

/// Find a tenant by GitHub ID (org/repo).
pub fn find_tenant_by_github_id(conn: &Connection, github_id: &str) -> Result<Option<Tenant>> {
    conn.query_row(
        "SELECT id, slug, db_path, github_id, created_at
         FROM tenants WHERE github_id = ?1",
        params![github_id],
        row_to_tenant,
    )
    .optional()
    .map_err(|e| ServerError::Registry(format!("Failed to find tenant by github_id: {}", e)))
}

fn row_to_user(row: &rusqlite::Row) -> rusqlite::Result<User> {
    Ok(User {
        id: row.get(0)?,
        github_id: row.get(1)?,
        github_login: row.get(2)?,
        github_token: row.get(3)?,
        created_at: row.get(4)?,
        last_login_at: row.get(5)?,
    })
}

fn row_to_tenant(row: &rusqlite::Row) -> rusqlite::Result<Tenant> {
    Ok(Tenant {
        id: row.get(0)?,
        slug: row.get(1)?,
        db_path: row.get(2)?,
        github_id: row.get(3)?,
        created_at: row.get(4)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_registry() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        conn
    }

    #[test]
    fn test_user_upsert_and_find() {
        let conn = test_registry();

        upsert_user(
            &conn,
            &UserUpsert {
                id: "user-1",
                github_id: "12345",
                github_login: "testuser",
                github_token: Some("ghp_token"),
            },
        )
        .unwrap();

        let user = find_user_by_github_id(&conn, "12345").unwrap().unwrap();
        assert_eq!(user.id, "user-1");
        assert_eq!(user.github_login, "testuser");
        assert_eq!(user.github_token, Some("ghp_token".to_string()));

        // Test update
        upsert_user(
            &conn,
            &UserUpsert {
                id: "user-1",
                github_id: "12345",
                github_login: "testuser",
                github_token: Some("ghp_new_token"),
            },
        )
        .unwrap();

        let user = find_user_by_github_id(&conn, "12345").unwrap().unwrap();
        assert_eq!(user.github_token, Some("ghp_new_token".to_string()));
    }

    #[test]
    fn test_refresh_token() {
        let conn = test_registry();

        // First create a user
        upsert_user(
            &conn,
            &UserUpsert {
                id: "user-1",
                github_id: "12345",
                github_login: "testuser",
                github_token: None,
            },
        )
        .unwrap();

        // Store a refresh token
        let token = RefreshToken {
            token: "refresh-123".to_string(),
            user_id: "user-1".to_string(),
            expires_at: "2099-01-01T00:00:00Z".to_string(),
            created_at: "2024-01-01T00:00:00Z".to_string(),
        };
        store_refresh_token(&conn, &token).unwrap();

        // Find the token
        let (found_token, user) = find_refresh_token(&conn, "refresh-123").unwrap().unwrap();
        assert_eq!(found_token.token, "refresh-123");
        assert_eq!(user.github_login, "testuser");
    }

    #[test]
    fn test_tenant() {
        let conn = test_registry();

        let tenant = Tenant {
            id: "tenant-1".to_string(),
            slug: "my-org".to_string(),
            db_path: "/data/tenants/my-org.db".to_string(),
            github_id: Some("org-123".to_string()),
            created_at: "2024-01-01T00:00:00Z".to_string(),
        };

        create_tenant(&conn, &tenant).unwrap();

        let found = find_tenant_by_slug(&conn, "my-org").unwrap().unwrap();
        assert_eq!(found.id, "tenant-1");
        assert_eq!(found.github_id, Some("org-123".to_string()));

        let found_by_github = find_tenant_by_github_id(&conn, "org-123").unwrap().unwrap();
        assert_eq!(found_by_github.slug, "my-org");
    }
}
