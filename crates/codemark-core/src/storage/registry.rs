//! Global registry database for tracking known repositories across the filesystem.
//!
//! The registry is stored at `~/.config/codemark/registry.db` and maintains
//! a cross-repository index of all projects that use codemark.

use crate::error::{Error, Result};
use rusqlite::{params, Connection};
use std::path::PathBuf;

/// Global registry database path.
pub fn registry_path() -> Result<PathBuf> {
    let config_dir = directories::ProjectDirs::from("com", "codemark", "codemark")
        .ok_or_else(|| Error::Operation("Could not determine config directory".into()))?;
    Ok(config_dir.config_dir().join("registry.db"))
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
    // Check if schema already exists by checking for known_repos table
    let exists: bool = conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name='known_repos'",
        [],
        |_| Ok(true),
    ).unwrap_or(false);

    if exists {
        return Ok(());
    }

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

/// Register or update a repository in the global registry.
pub fn upsert_repo(
    conn: &Connection,
    id: &str,
    repo_owner: &str,
    repo_name: &str,
    origin_url: Option<&str>,
    repo_root: &str,
    db_owner_email: &str,
    db_owner_name: Option<&str>,
    server_url: Option<&str>,
) -> Result<()> {
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
            id,
            repo_owner,
            repo_name,
            origin_url,
            repo_root,
            db_owner_email,
            db_owner_name,
            now,  // detected_at (only used on insert)
            now,  // last_seen_at
            server_url,
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

    let repo = conn.query_row(
        "SELECT id, repo_owner, repo_name, origin_url, repo_root, db_owner_email, db_owner_name, detected_at, last_seen_at, server_url
         FROM known_repos WHERE repo_owner = ?1 AND repo_name = ?2",
        params![parts[0], parts[1]],
        row_to_known_repo,
    ).ok();

    Ok(repo)
}

/// Find a repository by its local root path.
pub fn find_repo_by_root(conn: &Connection, root: &str) -> Result<Option<KnownRepo>> {
    let repo = conn.query_row(
        "SELECT id, repo_owner, repo_name, origin_url, repo_root, db_owner_email, db_owner_name, detected_at, last_seen_at, server_url
         FROM known_repos WHERE repo_root = ?1",
        params![root],
        row_to_known_repo,
    ).ok();

    Ok(repo)
}

/// List all known repositories.
pub fn list_repos(conn: &Connection) -> Result<Vec<KnownRepo>> {
    let mut stmt = conn.prepare(
        "SELECT id, repo_owner, repo_name, origin_url, repo_root, db_owner_email, db_owner_name, detected_at, last_seen_at, server_url
         FROM known_repos ORDER BY repo_owner, repo_name"
    )?;

    let repos = stmt.query_map([], row_to_known_repo)?
        .collect::<std::result::Result<Vec<_>, _>>()?;

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
    let url: Option<String> = conn.query_row(
        "SELECT server_url FROM known_repos WHERE repo_root = ?1",
        params![repo_root],
        |row| row.get(0),
    )?;

    Ok(url)
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
        let mut conn = Connection::open_in_memory()?;
        init_schema(&conn)?;
        Ok(conn)
    }

    #[test]
    fn test_upsert_and_find() {
        let conn = test_registry().unwrap();

        upsert_repo(
            &conn,
            "test-id-1",
            "owner",
            "repo",
            Some("https://github.com/owner/repo"),
            "/path/to/repo",
            "owner@example.com",
            Some("Owner"),
            None,
        ).unwrap();

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
            "test-id-2",
            "owner",
            "repo",
            None,
            "/another/path",
            "user@example.com",
            None,
            None,
        ).unwrap();

        set_server_url(&conn, "/another/path", Some("https://codemark.example.com")).unwrap();

        let url = get_server_url(&conn, "/another/path").unwrap().unwrap();
        assert_eq!(url, "https://codemark.example.com");
    }
}
