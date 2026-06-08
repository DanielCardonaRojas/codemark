//! Global registry database for tracking known repositories across the filesystem.
//!
//! The registry is stored in the global config directory as `registry.db` and maintains
//! a cross-repository index of all projects that use codemark.
//!
//! # Security Note
//!
//! Server authentication tokens are stored in plain text in this database. This is a known
//! limitation that should be addressed in future versions by using the system keychain
//! (macOS Keychain, Windows Credential Manager, etc.) or encrypted storage.

use crate::config::{global_config_dir, global_data_dir};
use crate::error::{Error, Result};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::PathBuf;

/// Global registry database path.
///
/// Stores the registry in the data directory (`~/.local/share/codemark/registry.db`).
pub fn registry_path() -> Result<PathBuf> {
    let data_dir = global_data_dir()
        .ok_or_else(|| Error::Operation("Could not determine data directory".into()))?;
    Ok(data_dir.join("registry.db"))
}

/// Open or create the global registry database.
///
/// If the database does not exist at the new data directory but exists at the old
/// config directory path, it is moved automatically.
pub fn open_registry() -> Result<Connection> {
    let path = registry_path()?;

    // Migrate from old config-dir location if needed
    if !path.exists() {
        if let Some(old_path) = global_config_dir().map(|d| d.join("registry.db")) {
            if old_path.exists() {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::rename(&old_path, &path).map_err(|e| {
                    Error::Operation(format!(
                        "Failed to move registry from {} to {}: {}",
                        old_path.display(),
                        path.display(),
                        e
                    ))
                })?;
            }
        }
    }

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let conn = Connection::open(&path)?;

    // Enable WAL mode for better concurrent access
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;

    // Initialize schema if this is a new database
    init_schema(&conn)?;

    Ok(conn)
}

/// Read the registry schema version (PRAGMA user_version).
fn get_registry_version(conn: &Connection) -> Result<i64> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    Ok(version)
}

/// Set the registry schema version (PRAGMA user_version).
fn set_registry_version(conn: &Connection, version: i64) -> Result<()> {
    conn.execute_batch(&format!("PRAGMA user_version = {version}"))?;
    Ok(())
}

/// Initialize the registry database schema with versioned migrations.
fn init_schema(conn: &Connection) -> Result<()> {
    let version = get_registry_version(conn)?;

    if version == 0 {
        // Check if this is an existing v0 database (has `servers` table) or a fresh DB
        let has_servers: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='servers'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(false);

        if has_servers {
            migrate_v0_to_v1(conn)?;
        } else {
            create_v1_schema(conn)?;
        }
    }
    // Future migrations: if version == 1 { migrate_v1_to_v2(conn)?; }

    Ok(())
}

/// Create a fresh v1 schema (for new databases).
fn create_v1_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS known_repos (
            id              TEXT PRIMARY KEY,
            repo_owner      TEXT NOT NULL,
            repo_name       TEXT NOT NULL,
            origin_url      TEXT,
            repo_root       TEXT NOT NULL UNIQUE,
            db_owner_email  TEXT NOT NULL,
            db_owner_name   TEXT,
            detected_at     TEXT NOT NULL,
            last_seen_at    TEXT NOT NULL,
            server_url      TEXT,
            default_username TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_known_repos_origin ON known_repos(origin_url);
        CREATE INDEX IF NOT EXISTS idx_known_repos_root ON known_repos(repo_root);
        CREATE INDEX IF NOT EXISTS idx_known_repos_owner_name ON known_repos(repo_owner, repo_name);

        CREATE TABLE IF NOT EXISTS accounts (
            server_url      TEXT NOT NULL,
            username        TEXT NOT NULL,
            email           TEXT,
            token           TEXT NOT NULL,
            is_default      INTEGER NOT NULL DEFAULT 0,
            last_used       TEXT,
            PRIMARY KEY (server_url, username)
        );

        CREATE INDEX IF NOT EXISTS idx_accounts_server ON accounts(server_url);
        CREATE INDEX IF NOT EXISTS idx_accounts_email ON accounts(email);
        ",
    )?;

    set_registry_version(conn, 1)?;
    Ok(())
}

/// Migrate from v0 (servers table) to v1 (accounts table).
fn migrate_v0_to_v1(conn: &Connection) -> Result<()> {
    conn.execute_batch("BEGIN TRANSACTION")?;

    let result = (|| -> Result<()> {
        // 1. Create accounts table
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS accounts (
                server_url      TEXT NOT NULL,
                username        TEXT NOT NULL,
                email           TEXT,
                token           TEXT NOT NULL,
                is_default      INTEGER NOT NULL DEFAULT 0,
                last_used       TEXT,
                PRIMARY KEY (server_url, username)
            );

            CREATE INDEX IF NOT EXISTS idx_accounts_server ON accounts(server_url);
            CREATE INDEX IF NOT EXISTS idx_accounts_email ON accounts(email);
            ",
        )?;

        // 2. Migrate servers → accounts, using db_owner_email from known_repos as username when available.
        // Use a scalar subquery (not LEFT JOIN) to pick exactly one canonical identity per server,
        // avoiding duplicate (server_url, username) PK rows when multiple repos reference the same server.
        conn.execute_batch(
            "INSERT INTO accounts (server_url, username, email, token, is_default, last_used)
             SELECT s.url,
                    COALESCE(
                        (SELECT kr.db_owner_email FROM known_repos kr
                         WHERE kr.server_url = s.url ORDER BY kr.last_seen_at DESC LIMIT 1),
                        'default'
                    ),
                    (SELECT kr.db_owner_email FROM known_repos kr
                     WHERE kr.server_url = s.url ORDER BY kr.last_seen_at DESC LIMIT 1),
                    s.token,
                    1,
                    s.last_login
             FROM servers s
             WHERE s.token IS NOT NULL",
        )?;

        // 3. Add default_username column to known_repos
        conn.execute_batch("ALTER TABLE known_repos ADD COLUMN default_username TEXT")?;

        // 4. Set default_username from db_owner_email where server_url is set
        conn.execute_batch(
            "UPDATE known_repos SET default_username = db_owner_email WHERE server_url IS NOT NULL",
        )?;

        // 5. Drop the old servers table
        conn.execute_batch("DROP TABLE IF EXISTS servers")?;

        // 6. Set version
        set_registry_version(conn, 1)?;

        Ok(())
    })();

    match result {
        Ok(()) => {
            conn.execute_batch("COMMIT")?;
            Ok(())
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// Information about a known repository in the registry.
#[derive(Debug, Clone, serde::Serialize)]
pub struct KnownRepo {
    pub id: String,
    pub repo_owner: String,
    pub repo_name: String,
    pub origin_url: Option<String>,
    pub repo_root: PathBuf,
    pub db_owner_email: String,
    pub db_owner_name: Option<String>,
    pub detected_at: String,
    pub last_seen_at: String,
    pub server_url: Option<String>,
    pub default_username: Option<String>,
}

/// Builder for upserting a repository to the global registry.
pub struct RepoUpsert<'a> {
    pub id: &'a str,
    pub repo_owner: &'a str,
    pub repo_name: &'a str,
    pub origin_url: Option<&'a str>,
    pub repo_root: &'a str,
    pub db_owner_email: &'a str,
    pub db_owner_name: Option<&'a str>,
    pub server_url: Option<&'a str>,
    pub default_username: Option<&'a str>,
}

/// Account authentication information for multi-account identity support.
///
/// Each account represents a username+server pair, allowing multiple identities
/// per server for zero-switch publishing.
///
/// # Security Warning
///
/// The `token` field is stored in plain text in the registry database.
#[derive(Debug, Clone)]
pub struct Account {
    pub server_url: String,
    pub username: String,
    pub email: Option<String>,
    pub token: String,
    pub is_default: bool,
    pub last_used: Option<String>,
}

/// Builder for upserting an account to the global registry.
pub struct AccountUpsert<'a> {
    pub server_url: &'a str,
    pub username: &'a str,
    pub email: Option<&'a str>,
    pub token: &'a str,
    pub is_default: bool,
}

fn row_to_account(row: &rusqlite::Row) -> rusqlite::Result<Account> {
    Ok(Account {
        server_url: row.get(0)?,
        username: row.get(1)?,
        email: row.get(2)?,
        token: row.get(3)?,
        is_default: row.get(4)?,
        last_used: row.get(5)?,
    })
}

/// Register or update an account in the registry.
///
/// If an account with the same (server_url, username) already exists, its token,
/// email, is_default, and last_used are updated.
pub fn upsert_account(conn: &Connection, account: &AccountUpsert<'_>) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "INSERT INTO accounts (server_url, username, email, token, is_default, last_used)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(server_url, username) DO UPDATE SET
             email = COALESCE(excluded.email, accounts.email),
             token = excluded.token,
             is_default = excluded.is_default,
             last_used = excluded.last_used",
        params![
            account.server_url,
            account.username,
            account.email,
            account.token,
            account.is_default,
            now,
        ],
    )?;

    Ok(())
}

/// Get an account by server URL and username.
pub fn get_account(conn: &Connection, server_url: &str, username: &str) -> Result<Option<Account>> {
    conn.query_row(
        "SELECT server_url, username, email, token, is_default, last_used
         FROM accounts WHERE server_url = ?1 AND username = ?2",
        params![server_url, username],
        row_to_account,
    )
    .optional()
    .map_err(|e| Error::Database(e.to_string()))
}

/// Get the default account for a server (highest is_default, most recent last_used).
pub fn get_default_account(conn: &Connection, server_url: &str) -> Result<Option<Account>> {
    conn.query_row(
        "SELECT server_url, username, email, token, is_default, last_used
         FROM accounts WHERE server_url = ?1
         ORDER BY is_default DESC, last_used DESC
         LIMIT 1",
        params![server_url],
        row_to_account,
    )
    .optional()
    .map_err(|e| Error::Database(e.to_string()))
}

/// List all accounts, optionally filtered by server URL.
pub fn list_accounts(conn: &Connection, server_url: Option<&str>) -> Result<Vec<Account>> {
    if let Some(url) = server_url {
        let mut stmt = conn.prepare(
            "SELECT server_url, username, email, token, is_default, last_used
             FROM accounts WHERE server_url = ?1
             ORDER BY is_default DESC, last_used DESC",
        )?;
        let accounts = stmt
            .query_map(params![url], row_to_account)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(accounts)
    } else {
        let mut stmt = conn.prepare(
            "SELECT server_url, username, email, token, is_default, last_used
             FROM accounts ORDER BY server_url, is_default DESC, last_used DESC",
        )?;
        let accounts =
            stmt.query_map([], row_to_account)?.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(accounts)
    }
}

/// Delete an account. If username is None, deletes all accounts for the server.
///
/// Also clears `server_url` on any `known_repos` rows that referenced this server
/// when all accounts for the server are removed.
pub fn delete_account(conn: &Connection, server_url: &str, username: Option<&str>) -> Result<()> {
    if let Some(user) = username {
        conn.execute(
            "DELETE FROM accounts WHERE server_url = ?1 AND username = ?2",
            params![server_url, user],
        )?;
    } else {
        conn.execute("DELETE FROM accounts WHERE server_url = ?1", params![server_url])?;
    }

    // If no accounts remain for this server, clear dangling known_repos references
    let remaining: i64 = conn.query_row(
        "SELECT COUNT(*) FROM accounts WHERE server_url = ?1",
        params![server_url],
        |row| row.get(0),
    )?;
    if remaining == 0 {
        conn.execute(
            "UPDATE known_repos SET server_url = NULL WHERE server_url = ?1",
            params![server_url],
        )?;
    }

    Ok(())
}

/// Get the global default account across all servers (most recently used default).
pub fn get_global_default_account(conn: &Connection) -> Result<Option<Account>> {
    conn.query_row(
        "SELECT server_url, username, email, token, is_default, last_used
         FROM accounts
         ORDER BY is_default DESC, last_used DESC
         LIMIT 1",
        [],
        row_to_account,
    )
    .optional()
    .map_err(|e| Error::Database(e.to_string()))
}

/// Clear the is_default flag on all accounts for a server.
///
/// Call this before upserting a new account to ensure only the new account is marked default.
pub fn clear_default_account(conn: &Connection, server_url: &str) -> Result<()> {
    conn.execute("UPDATE accounts SET is_default = 0 WHERE server_url = ?1", params![server_url])?;
    Ok(())
}

/// Resolve the best token for a server given an optional identity hint.
///
/// Priority:
/// 1. Exact username match → return token
/// 2. Email match → return token
/// 3. Default account (is_default DESC, last_used DESC) → return token
pub fn resolve_token(
    conn: &Connection,
    server_url: &str,
    identity_hint: Option<&str>,
) -> Result<Option<String>> {
    if let Some(hint) = identity_hint {
        // Try exact username match
        if let Some(account) = get_account(conn, server_url, hint)? {
            return Ok(Some(account.token));
        }

        // Try email match
        let email_match: Option<String> = conn
            .query_row(
                "SELECT token FROM accounts WHERE server_url = ?1 AND email = ?2 LIMIT 1",
                params![server_url, hint],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| Error::Database(e.to_string()))?;

        if let Some(token) = email_match {
            return Ok(Some(token));
        }
    }

    // Fallback to default account
    Ok(get_default_account(conn, server_url)?.map(|a| a.token))
}

/// Register or update a repository in the global registry.
pub fn upsert_repo(conn: &Connection, repo: &RepoUpsert<'_>) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "INSERT INTO known_repos (id, repo_owner, repo_name, origin_url, repo_root, db_owner_email, db_owner_name, detected_at, last_seen_at, server_url, default_username)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT(repo_root) DO UPDATE SET
             id = excluded.id,
             repo_owner = excluded.repo_owner,
             repo_name = excluded.repo_name,
             origin_url = excluded.origin_url,
             db_owner_email = excluded.db_owner_email,
             db_owner_name = excluded.db_owner_name,
             last_seen_at = excluded.last_seen_at,
             server_url = COALESCE(excluded.server_url, known_repos.server_url),
             default_username = COALESCE(excluded.default_username, known_repos.default_username)",
        params![
            repo.id,
            repo.repo_owner,
            repo.repo_name,
            repo.origin_url,
            repo.repo_root,
            repo.db_owner_email,
            repo.db_owner_name,
            now, // detected_at (only used on insert)
            now, // last_seen_at
            repo.server_url,
            repo.default_username,
        ],
    )?;

    Ok(())
}

/// Find a repository by owner/name (e.g., "owner/repo").
pub fn find_repo_by_owner_name(conn: &Connection, owner_name: &str) -> Result<Option<KnownRepo>> {
    let parts: Vec<&str> = owner_name.split('/').collect();
    if parts.len() != 2 {
        return Err(Error::Input(format!(
            "Invalid repo reference: '{owner_name}'. Expected format: 'owner/repo'"
        )));
    }

    match conn.query_row(
        "SELECT id, repo_owner, repo_name, origin_url, repo_root, db_owner_email, db_owner_name, detected_at, last_seen_at, server_url, default_username
         FROM known_repos WHERE repo_owner = ?1 AND repo_name = ?2",
        params![parts[0], parts[1]],
        row_to_known_repo,
    ) {
        Ok(repo) => Ok(Some(repo)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(Error::Database(e.to_string())),
    }
}

/// Find a repository by its local root path.
pub fn find_repo_by_root(conn: &Connection, root: &str) -> Result<Option<KnownRepo>> {
    match conn.query_row(
        "SELECT id, repo_owner, repo_name, origin_url, repo_root, db_owner_email, db_owner_name, detected_at, last_seen_at, server_url, default_username
         FROM known_repos WHERE repo_root = ?1",
        params![root],
        row_to_known_repo,
    ) {
        Ok(repo) => Ok(Some(repo)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(Error::Database(e.to_string())),
    }
}

/// List distinct repo owners from all known repositories.
pub fn list_repo_owners(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt =
        conn.prepare("SELECT DISTINCT repo_owner FROM known_repos ORDER BY repo_owner")?;
    let rows = stmt.query_map([], |row| row.get(0))?;
    let mut owners = Vec::new();
    for owner in rows {
        owners.push(owner?);
    }
    Ok(owners)
}

/// List all known repositories.
pub fn list_repos(conn: &Connection) -> Result<Vec<KnownRepo>> {
    let mut stmt = conn.prepare(
        "SELECT id, repo_owner, repo_name, origin_url, repo_root, db_owner_email, db_owner_name, detected_at, last_seen_at, server_url, default_username
         FROM known_repos ORDER BY repo_owner, repo_name"
    )?;

    let repos =
        stmt.query_map([], row_to_known_repo)?.collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(repos)
}

/// Update the server URL for a repository.
pub fn set_server_url(conn: &Connection, repo_root: &str, server_url: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE known_repos SET server_url = ?1 WHERE repo_root = ?2",
        params![server_url, repo_root],
    )?;

    Ok(())
}

/// Get the server URL for a repository by its root path.
pub fn get_server_url(conn: &Connection, repo_root: &str) -> Result<Option<String>> {
    let url: Option<String> = conn
        .query_row(
            "SELECT server_url FROM known_repos WHERE repo_root = ?1",
            params![repo_root],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(None);

    Ok(url)
}

/// Find a repository by its origin URL.
///
/// This function performs a normalized comparison to match repositories regardless
/// of the URL format used (HTTPS, SSH, owner/repo, etc.). It parses the input URL
/// to extract the owner and name, then queries by those fields.
pub fn find_repo_by_origin(conn: &Connection, origin_url: &str) -> Result<Option<KnownRepo>> {
    // Try to parse the URL to extract owner/name for flexible matching
    if let Some((owner, repo_name)) = crate::git::remote::parse_remote_url(origin_url) {
        return conn
            .query_row(
                "SELECT id, repo_owner, repo_name, origin_url, repo_root, db_owner_email, db_owner_name, detected_at, last_seen_at, server_url, default_username
                 FROM known_repos WHERE repo_owner = ?1 AND repo_name = ?2",
                params![owner, repo_name],
                row_to_known_repo,
            )
            .optional()
            .map_err(|e| Error::Database(e.to_string()));
    }

    // Fallback to exact match if parsing fails (e.g., non-GitHub URLs)
    conn.query_row(
        "SELECT id, repo_owner, repo_name, origin_url, repo_root, db_owner_email, db_owner_name, detected_at, last_seen_at, server_url, default_username
         FROM known_repos WHERE origin_url = ?1",
        params![origin_url],
        row_to_known_repo,
    )
    .optional()
    .map_err(|e| Error::Database(e.to_string()))
}

/// Resolve multiple repository references (owner/name) to their local paths.
///
/// Returns a vector of (repo_ref, local_path) pairs for repositories found in the registry.
/// Repositories not found are silently skipped (consistent with how missing --db paths are handled).
pub fn resolve_repos(conn: &Connection, repo_refs: &[String]) -> Result<Vec<(String, PathBuf)>> {
    let mut result = Vec::new();

    for repo_ref in repo_refs {
        let parts: Vec<&str> = repo_ref.split('/').collect();
        if parts.len() != 2 {
            eprintln!(
                "codemark: warning: invalid repo reference: '{}', expected format 'owner/name'",
                repo_ref
            );
            continue;
        }

        match conn.query_row(
            "SELECT repo_root FROM known_repos WHERE repo_owner = ?1 AND repo_name = ?2",
            params![parts[0], parts[1]],
            |row| row.get::<_, String>(0),
        ) {
            Ok(repo_root) => {
                result.push((repo_ref.clone(), PathBuf::from(repo_root)));
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                eprintln!("codemark: warning: repository '{}' not found in registry", repo_ref);
            }
            Err(e) => {
                eprintln!("codemark: warning: failed to resolve repository '{}': {}", repo_ref, e);
            }
        }
    }

    Ok(result)
}

fn row_to_known_repo(row: &rusqlite::Row) -> rusqlite::Result<KnownRepo> {
    Ok(KnownRepo {
        id: row.get(0)?,
        repo_owner: row.get(1)?,
        repo_name: row.get(2)?,
        origin_url: row.get(3)?,
        repo_root: PathBuf::from(row.get::<_, String>(4)?),
        db_owner_email: row.get(5)?,
        db_owner_name: row.get(6)?,
        detected_at: row.get(7)?,
        last_seen_at: row.get(8)?,
        server_url: row.get(9)?,
        default_username: row.get(10)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_registry() -> Result<Connection> {
        let conn = Connection::open_in_memory()?;
        init_schema(&conn)?;
        Ok(conn)
    }

    #[test]
    fn test_upsert_and_find() {
        let conn = test_registry().unwrap();

        upsert_repo(
            &conn,
            &RepoUpsert {
                id: "test-id-1",
                repo_owner: "owner",
                repo_name: "repo",
                origin_url: Some("https://github.com/owner/repo"),
                repo_root: "/path/to/repo",
                db_owner_email: "owner@example.com",
                db_owner_name: Some("Owner"),
                server_url: None,
                default_username: None,
            },
        )
        .unwrap();

        // Find by owner/name
        let repo = find_repo_by_owner_name(&conn, "owner/repo").unwrap().unwrap();
        assert_eq!(repo.repo_owner, "owner");
        assert_eq!(repo.repo_name, "repo");
        assert_eq!(repo.repo_root, PathBuf::from("/path/to/repo"));

        // Find by root
        let repo = find_repo_by_root(&conn, "/path/to/repo").unwrap().unwrap();
        assert_eq!(repo.id, "test-id-1");
    }

    #[test]
    fn test_set_server_url() {
        let conn = test_registry().unwrap();

        upsert_repo(
            &conn,
            &RepoUpsert {
                id: "test-id-2",
                repo_owner: "owner",
                repo_name: "repo",
                origin_url: None,
                repo_root: "/another/path",
                db_owner_email: "user@example.com",
                db_owner_name: None,
                server_url: None,
                default_username: None,
            },
        )
        .unwrap();

        set_server_url(&conn, "/another/path", Some("https://codemark.example.com")).unwrap();

        let url = get_server_url(&conn, "/another/path").unwrap().unwrap();
        assert_eq!(url, "https://codemark.example.com");
    }

    #[test]
    fn test_resolve_repos() {
        let conn = test_registry().unwrap();

        // Insert multiple repos
        upsert_repo(
            &conn,
            &RepoUpsert {
                id: "test-id-1",
                repo_owner: "facebook",
                repo_name: "react",
                origin_url: Some("https://github.com/facebook/react"),
                repo_root: "/dev/react",
                db_owner_email: "user@example.com",
                db_owner_name: None,
                server_url: None,
                default_username: None,
            },
        )
        .unwrap();

        upsert_repo(
            &conn,
            &RepoUpsert {
                id: "test-id-2",
                repo_owner: "acme",
                repo_name: "api",
                origin_url: Some("https://github.com/acme/api"),
                repo_root: "/work/api",
                db_owner_email: "user@example.com",
                db_owner_name: None,
                server_url: None,
                default_username: None,
            },
        )
        .unwrap();

        // Resolve multiple repos
        let repo_refs = vec!["facebook/react".to_string(), "acme/api".to_string()];
        let resolved = resolve_repos(&conn, &repo_refs).unwrap();

        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].0, "facebook/react");
        assert_eq!(resolved[0].1, PathBuf::from("/dev/react"));
        assert_eq!(resolved[1].0, "acme/api");
        assert_eq!(resolved[1].1, PathBuf::from("/work/api"));

        // Test with non-existent repo (should be skipped)
        let repo_refs = vec![
            "facebook/react".to_string(),
            "nonexistent/repo".to_string(),
            "acme/api".to_string(),
        ];
        let resolved = resolve_repos(&conn, &repo_refs).unwrap();
        assert_eq!(resolved.len(), 2); // Only the two existing repos
    }

    #[test]
    fn test_find_repo_by_origin() {
        let conn = test_registry().unwrap();

        upsert_repo(
            &conn,
            &RepoUpsert {
                id: "test-origin",
                repo_owner: "owner",
                repo_name: "repo",
                origin_url: Some("https://github.com/owner/repo.git"),
                repo_root: "/path/to/repo",
                db_owner_email: "owner@example.com",
                db_owner_name: None,
                server_url: None,
                default_username: None,
            },
        )
        .unwrap();

        // Find by origin URL
        let repo =
            find_repo_by_origin(&conn, "https://github.com/owner/repo.git").unwrap().unwrap();
        assert_eq!(repo.repo_owner, "owner");
        assert_eq!(repo.repo_name, "repo");

        // Non-existent origin URL
        assert!(
            find_repo_by_origin(&conn, "https://github.com/nonexistent/repo.git")
                .unwrap()
                .is_none()
        );

        // Test cross-format matching: repo stored with HTTPS URL can be found via SSH URL
        let repo = find_repo_by_origin(&conn, "git@github.com:owner/repo.git").unwrap().unwrap();
        assert_eq!(repo.repo_owner, "owner");
        assert_eq!(repo.repo_name, "repo");

        // Also test with owner/repo format (after normalization in tour.rs)
        let repo = find_repo_by_origin(&conn, "owner/repo").unwrap().unwrap();
        assert_eq!(repo.repo_owner, "owner");
        assert_eq!(repo.repo_name, "repo");
    }
}

#[cfg(test)]
mod account_tests {
    use super::*;

    fn test_registry() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        conn
    }

    #[test]
    fn test_account_upsert_and_get() {
        let conn = test_registry();

        upsert_account(
            &conn,
            &AccountUpsert {
                server_url: "https://codemark.example.com",
                username: "alice",
                email: Some("alice@example.com"),
                token: "token123",
                is_default: true,
            },
        )
        .unwrap();

        let account = get_account(&conn, "https://codemark.example.com", "alice").unwrap().unwrap();
        assert_eq!(account.server_url, "https://codemark.example.com");
        assert_eq!(account.username, "alice");
        assert_eq!(account.email, Some("alice@example.com".to_string()));
        assert_eq!(account.token, "token123");
        assert!(account.is_default);
        assert!(account.last_used.is_some());

        // Update with new token
        upsert_account(
            &conn,
            &AccountUpsert {
                server_url: "https://codemark.example.com",
                username: "alice",
                email: Some("alice@example.com"),
                token: "new_token",
                is_default: true,
            },
        )
        .unwrap();
        let account = get_account(&conn, "https://codemark.example.com", "alice").unwrap().unwrap();
        assert_eq!(account.token, "new_token");
    }

    #[test]
    fn test_list_accounts() {
        let conn = test_registry();

        upsert_account(
            &conn,
            &AccountUpsert {
                server_url: "https://server1.com",
                username: "user1",
                email: None,
                token: "token1",
                is_default: true,
            },
        )
        .unwrap();
        upsert_account(
            &conn,
            &AccountUpsert {
                server_url: "https://server2.com",
                username: "user2",
                email: None,
                token: "token2",
                is_default: true,
            },
        )
        .unwrap();

        let accounts = list_accounts(&conn, None).unwrap();
        assert_eq!(accounts.len(), 2);

        // Filter by server
        let accounts = list_accounts(&conn, Some("https://server1.com")).unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].username, "user1");
    }

    #[test]
    fn test_delete_account() {
        let conn = test_registry();

        upsert_account(
            &conn,
            &AccountUpsert {
                server_url: "https://server.com",
                username: "user",
                email: None,
                token: "token",
                is_default: true,
            },
        )
        .unwrap();
        assert!(get_account(&conn, "https://server.com", "user").unwrap().is_some());

        delete_account(&conn, "https://server.com", Some("user")).unwrap();
        assert!(get_account(&conn, "https://server.com", "user").unwrap().is_none());
    }

    #[test]
    fn test_delete_all_accounts_for_server() {
        let conn = test_registry();

        upsert_account(
            &conn,
            &AccountUpsert {
                server_url: "https://server.com",
                username: "alice",
                email: None,
                token: "token1",
                is_default: true,
            },
        )
        .unwrap();
        upsert_account(
            &conn,
            &AccountUpsert {
                server_url: "https://server.com",
                username: "bob",
                email: None,
                token: "token2",
                is_default: false,
            },
        )
        .unwrap();

        assert_eq!(list_accounts(&conn, Some("https://server.com")).unwrap().len(), 2);

        // Delete all accounts for server (username = None)
        delete_account(&conn, "https://server.com", None).unwrap();
        assert_eq!(list_accounts(&conn, Some("https://server.com")).unwrap().len(), 0);
    }

    #[test]
    fn test_resolve_token_by_username() {
        let conn = test_registry();

        upsert_account(
            &conn,
            &AccountUpsert {
                server_url: "https://server.com",
                username: "alice",
                email: Some("alice@example.com"),
                token: "alice_token",
                is_default: false,
            },
        )
        .unwrap();
        upsert_account(
            &conn,
            &AccountUpsert {
                server_url: "https://server.com",
                username: "bob",
                email: Some("bob@example.com"),
                token: "bob_token",
                is_default: true,
            },
        )
        .unwrap();

        // Exact username match
        let token = resolve_token(&conn, "https://server.com", Some("alice")).unwrap();
        assert_eq!(token, Some("alice_token".to_string()));
    }

    #[test]
    fn test_resolve_token_by_email() {
        let conn = test_registry();

        upsert_account(
            &conn,
            &AccountUpsert {
                server_url: "https://server.com",
                username: "alice",
                email: Some("alice@example.com"),
                token: "alice_token",
                is_default: false,
            },
        )
        .unwrap();

        // Email match
        let token = resolve_token(&conn, "https://server.com", Some("alice@example.com")).unwrap();
        assert_eq!(token, Some("alice_token".to_string()));
    }

    #[test]
    fn test_resolve_token_default_fallback() {
        let conn = test_registry();

        upsert_account(
            &conn,
            &AccountUpsert {
                server_url: "https://server.com",
                username: "alice",
                email: None,
                token: "alice_token",
                is_default: false,
            },
        )
        .unwrap();
        upsert_account(
            &conn,
            &AccountUpsert {
                server_url: "https://server.com",
                username: "bob",
                email: None,
                token: "bob_token",
                is_default: true,
            },
        )
        .unwrap();

        // No hint → falls back to default (bob, is_default=true)
        let token = resolve_token(&conn, "https://server.com", None).unwrap();
        assert_eq!(token, Some("bob_token".to_string()));

        // Non-matching hint → also falls back to default
        let token = resolve_token(&conn, "https://server.com", Some("nonexistent")).unwrap();
        assert_eq!(token, Some("bob_token".to_string()));
    }

    #[test]
    fn test_multiple_accounts_per_server() {
        let conn = test_registry();

        upsert_account(
            &conn,
            &AccountUpsert {
                server_url: "https://server.com",
                username: "personal",
                email: Some("me@personal.com"),
                token: "personal_token",
                is_default: false,
            },
        )
        .unwrap();
        upsert_account(
            &conn,
            &AccountUpsert {
                server_url: "https://server.com",
                username: "work",
                email: Some("me@work.com"),
                token: "work_token",
                is_default: true,
            },
        )
        .unwrap();

        let accounts = list_accounts(&conn, Some("https://server.com")).unwrap();
        assert_eq!(accounts.len(), 2);

        // Default should be "work" (is_default=true)
        let default = get_default_account(&conn, "https://server.com").unwrap().unwrap();
        assert_eq!(default.username, "work");
    }

    #[test]
    fn test_fresh_db_creates_v1() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();

        let version = get_registry_version(&conn).unwrap();
        assert_eq!(version, 1);

        // accounts table should exist
        let has_accounts: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='accounts'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(has_accounts);

        // servers table should NOT exist
        let has_servers: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='servers'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!has_servers);
    }

    #[test]
    fn test_migrate_v0_to_v1() {
        let conn = Connection::open_in_memory().unwrap();

        // Create v0 schema manually (the old schema without accounts)
        conn.execute_batch(
            "CREATE TABLE known_repos (
                id              TEXT PRIMARY KEY,
                repo_owner      TEXT NOT NULL,
                repo_name       TEXT NOT NULL,
                origin_url      TEXT,
                repo_root       TEXT NOT NULL UNIQUE,
                db_owner_email  TEXT NOT NULL,
                db_owner_name   TEXT,
                detected_at     TEXT NOT NULL,
                last_seen_at    TEXT NOT NULL,
                server_url      TEXT
            );

            CREATE TABLE servers (
                url             TEXT PRIMARY KEY,
                token           TEXT,
                last_login      TEXT
            );",
        )
        .unwrap();

        // Insert some v0 data
        conn.execute(
            "INSERT INTO servers (url, token, last_login) VALUES (?1, ?2, ?3)",
            params!["https://server.com", "old_token", "2024-01-01T00:00:00Z"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO known_repos (id, repo_owner, repo_name, origin_url, repo_root, db_owner_email, db_owner_name, detected_at, last_seen_at, server_url)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                "repo-1",
                "owner",
                "repo",
                "https://github.com/owner/repo",
                "/path/to/repo",
                "user@example.com",
                "User",
                "2024-01-01T00:00:00Z",
                "2024-01-01T00:00:00Z",
                "https://server.com",
            ],
        )
        .unwrap();

        // Also insert a server with no matching repo (should use "default" as username)
        conn.execute(
            "INSERT INTO servers (url, token, last_login) VALUES (?1, ?2, ?3)",
            params!["https://other.com", "other_token", "2024-01-01T00:00:00Z"],
        )
        .unwrap();

        // Also insert a server with NULL token (should be skipped)
        conn.execute(
            "INSERT INTO servers (url, token, last_login) VALUES (?1, ?2, ?3)",
            params!["https://notoken.com", Option::<String>::None, "2024-01-01T00:00:00Z"],
        )
        .unwrap();

        // Run migration
        init_schema(&conn).unwrap();

        // Verify version is 1
        let version = get_registry_version(&conn).unwrap();
        assert_eq!(version, 1);

        // servers table should be gone
        let has_servers: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='servers'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!has_servers);

        // accounts table should exist with migrated data
        let accounts = list_accounts(&conn, None).unwrap();
        // Only 2 accounts (the NULL-token server was skipped)
        assert_eq!(accounts.len(), 2);

        // The server with a matching repo should use db_owner_email as username
        let matched = get_account(&conn, "https://server.com", "user@example.com").unwrap();
        assert!(matched.is_some());
        assert_eq!(matched.unwrap().token, "old_token");

        // The server with no matching repo should use "default"
        let unmatched = get_account(&conn, "https://other.com", "default").unwrap();
        assert!(unmatched.is_some());
        assert_eq!(unmatched.unwrap().token, "other_token");

        // The repo should have default_username set
        let repo = find_repo_by_root(&conn, "/path/to/repo").unwrap().unwrap();
        assert_eq!(repo.default_username, Some("user@example.com".to_string()));
    }
}
