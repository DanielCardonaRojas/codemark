//! Global registry database for tracking known repositories across the filesystem.
//!
//! The registry is stored in the global config directory as `registry.db` and maintains
//! a cross-repository index of all projects that use codemark.

use crate::config::global_config_dir;
use crate::error::{Error, Result};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::PathBuf;

/// Global registry database path.
pub fn registry_path() -> Result<PathBuf> {
    let config_dir = global_config_dir()
        .ok_or_else(|| Error::Operation("Could not determine config directory".into()))?;
    Ok(config_dir.join("registry.db"))
}

/// Open or create the global registry database.
pub fn open_registry() -> Result<Connection> {
    let path = registry_path()?;

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

/// Initialize the registry database schema.
fn init_schema(conn: &Connection) -> Result<()> {
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
            server_url      TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_known_repos_origin ON known_repos(origin_url);
        CREATE INDEX IF NOT EXISTS idx_known_repos_root ON known_repos(repo_root);
        CREATE INDEX IF NOT EXISTS idx_known_repos_owner_name ON known_repos(repo_owner, repo_name);

        CREATE TABLE IF NOT EXISTS servers (
            url             TEXT PRIMARY KEY,
                token           TEXT,
                last_login      TEXT
        );
        ",
    )?;

    Ok(())
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
}

/// Register or update a repository in the global registry.
pub fn upsert_repo(conn: &Connection, repo: &RepoUpsert<'_>) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "INSERT INTO known_repos (id, repo_owner, repo_name, origin_url, repo_root, db_owner_email, db_owner_name, detected_at, last_seen_at, server_url)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(repo_root) DO UPDATE SET
             id = excluded.id,
             repo_owner = excluded.repo_owner,
             repo_name = excluded.repo_name,
             origin_url = excluded.origin_url,
             db_owner_email = excluded.db_owner_email,
             db_owner_name = excluded.db_owner_name,
             last_seen_at = excluded.last_seen_at,
             server_url = COALESCE(excluded.server_url, known_repos.server_url)",
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
        "SELECT id, repo_owner, repo_name, origin_url, repo_root, db_owner_email, db_owner_name, detected_at, last_seen_at, server_url
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
        "SELECT id, repo_owner, repo_name, origin_url, repo_root, db_owner_email, db_owner_name, detected_at, last_seen_at, server_url
         FROM known_repos WHERE repo_root = ?1",
        params![root],
        row_to_known_repo,
    ) {
        Ok(repo) => Ok(Some(repo)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(Error::Database(e.to_string())),
    }
}

/// List all known repositories.
pub fn list_repos(conn: &Connection) -> Result<Vec<KnownRepo>> {
    let mut stmt = conn.prepare(
        "SELECT id, repo_owner, repo_name, origin_url, repo_root, db_owner_email, db_owner_name, detected_at, last_seen_at, server_url
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
pub fn find_repo_by_origin(conn: &Connection, origin_url: &str) -> Result<Option<KnownRepo>> {
    conn.query_row(
        "SELECT id, repo_owner, repo_name, origin_url, repo_root, db_owner_email, db_owner_name, detected_at, last_seen_at, server_url
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
}

/// Server authentication information.
#[derive(Debug, Clone)]
pub struct Server {
    pub url: String,
    pub token: Option<String>,
    pub last_login: Option<String>,
}

/// Register or update a server in the registry.
///
/// If a server with the same URL already exists and `token` is `None`,
/// the existing token is preserved (only `last_login` is updated).
pub fn upsert_server(conn: &Connection, url: &str, token: Option<&str>) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "INSERT INTO servers (url, token, last_login)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(url) DO UPDATE SET
             token = COALESCE(excluded.token, servers.token),
             last_login = excluded.last_login",
        params![url, token, now],
    )?;

    Ok(())
}

/// Get a server by URL.
pub fn get_server(conn: &Connection, url: &str) -> Result<Option<Server>> {
    conn.query_row(
        "SELECT url, token, last_login FROM servers WHERE url = ?1",
        params![url],
        |row| Ok(Server { url: row.get(0)?, token: row.get(1)?, last_login: row.get(2)? }),
    )
    .optional()
    .map_err(|e| Error::Database(e.to_string()))
}

/// List all servers.
pub fn list_servers(conn: &Connection) -> Result<Vec<Server>> {
    let mut stmt = conn.prepare("SELECT url, token, last_login FROM servers ORDER BY url")?;

    let servers = stmt
        .query_map([], |row| {
            Ok(Server { url: row.get(0)?, token: row.get(1)?, last_login: row.get(2)? })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(servers)
}

/// Delete a server by URL.
pub fn delete_server(conn: &Connection, url: &str) -> Result<()> {
    conn.execute("DELETE FROM servers WHERE url = ?1", params![url])?;

    Ok(())
}

#[cfg(test)]
mod server_tests {
    use super::*;

    fn test_registry() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        conn
    }

    #[test]
    fn test_server_upsert_and_get() {
        let conn = test_registry();

        upsert_server(&conn, "https://codemark.example.com", Some("token123")).unwrap();

        let server = get_server(&conn, "https://codemark.example.com").unwrap().unwrap();
        assert_eq!(server.url, "https://codemark.example.com");
        assert_eq!(server.token, Some("token123".to_string()));
        assert!(server.last_login.is_some());

        // Update with new token
        upsert_server(&conn, "https://codemark.example.com", Some("new_token")).unwrap();
        let server = get_server(&conn, "https://codemark.example.com").unwrap().unwrap();
        assert_eq!(server.token, Some("new_token".to_string()));
    }

    #[test]
    fn test_list_servers() {
        let conn = test_registry();

        upsert_server(&conn, "https://server1.com", Some("token1")).unwrap();
        upsert_server(&conn, "https://server2.com", Some("token2")).unwrap();

        let servers = list_servers(&conn).unwrap();
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[0].url, "https://server1.com");
        assert_eq!(servers[1].url, "https://server2.com");
    }

    #[test]
    fn test_delete_server() {
        let conn = test_registry();

        upsert_server(&conn, "https://server.com", Some("token")).unwrap();
        assert!(get_server(&conn, "https://server.com").unwrap().is_some());

        delete_server(&conn, "https://server.com").unwrap();
        assert!(get_server(&conn, "https://server.com").unwrap().is_none());
    }
}
