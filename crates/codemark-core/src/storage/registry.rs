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

const REGISTRY_MIGRATION_001: &str =
    include_str!("../../../../registry_migrations/V1__multi_account.sql");

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
    if !path.exists()
        && let Some(old_path) = global_config_dir().map(|d| d.join("registry.db"))
        && old_path.exists()
    {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if let Err(rename_err) = std::fs::rename(&old_path, &path) {
            // rename(2) fails with EXDEV when src and dst are on different
            // filesystems. Fall back to copy + delete.
            std::fs::copy(&old_path, &path).map_err(|e| {
                Error::Operation(format!(
                    "Failed to move registry from {} to {}: rename failed ({}), copy also failed: {}",
                    old_path.display(),
                    path.display(),
                    rename_err,
                    e
                ))
            })?;
            let _ = std::fs::remove_file(&old_path);
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

    // Detect legacy v0 databases that need special migration
    if version == 0 {
        let has_servers: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='servers'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(false);

        if has_servers {
            migrate_v0_to_v1(conn)?;
            return Ok(());
        }
    }

    // Standard migration loop for fresh DBs and future upgrades
    let migrations: &[(i64, &str)] = &[(1, REGISTRY_MIGRATION_001)];

    for &(target_version, sql) in migrations {
        if version < target_version {
            conn.execute_batch(sql)?;
            set_registry_version(conn, target_version)?;
        }
    }

    Ok(())
}

/// Migrate from v0 (servers table) to v1 (accounts table).
fn migrate_v0_to_v1(conn: &Connection) -> Result<()> {
    conn.execute_batch("BEGIN TRANSACTION")?;

    let result = (|| -> Result<()> {
        // 1. Create accounts table (and indexes) from the V1 migration SQL.
        // The known_repos CREATE is a no-op here (table already exists from v0).
        conn.execute_batch(REGISTRY_MIGRATION_001)?;

        // 2. Migrate servers → accounts, using db_owner_email from known_repos as username when available.
        // Use a scalar subquery (not LEFT JOIN) to pick exactly one canonical identity per server,
        // avoiding duplicate (server_url, forge_kind, username) PK rows when multiple repos reference the same server.
        // Hardcode forge_kind = 'github' since v0 only supported GitHub.
        conn.execute_batch(
            "INSERT INTO accounts (server_url, forge_kind, username, email, token, is_default, last_used)
             SELECT s.url,
                    'github',
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
    pub forge_kind: String,
    pub username: String,
    pub email: Option<String>,
    pub token: String,
    pub is_default: bool,
    pub last_used: Option<String>,
}

/// Builder for upserting an account to the global registry.
pub struct AccountUpsert<'a> {
    pub server_url: &'a str,
    pub forge_kind: &'a str,
    pub username: &'a str,
    pub email: Option<&'a str>,
    pub token: &'a str,
    pub is_default: bool,
}

fn row_to_account(row: &rusqlite::Row) -> rusqlite::Result<Account> {
    Ok(Account {
        server_url: row.get(0)?,
        forge_kind: row.get(1)?,
        username: row.get(2)?,
        email: row.get(3)?,
        token: row.get(4)?,
        is_default: row.get(5)?,
        last_used: row.get(6)?,
    })
}

/// Register or update an account in the registry.
///
/// If an account with the same (server_url, username) already exists, its token,
/// email, is_default, and last_used are updated.
pub fn upsert_account(conn: &Connection, account: &AccountUpsert<'_>) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "INSERT INTO accounts (server_url, forge_kind, username, email, token, is_default, last_used)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(server_url, forge_kind, username) DO UPDATE SET
             email = COALESCE(excluded.email, accounts.email),
             token = excluded.token,
             is_default = excluded.is_default,
             last_used = excluded.last_used",
        params![
            account.server_url,
            account.forge_kind,
            account.username,
            account.email,
            account.token,
            account.is_default,
            now,
        ],
    )?;

    Ok(())
}

/// Get an account by server URL, forge kind, and username.
pub fn get_account(
    conn: &Connection,
    server_url: &str,
    forge_kind: &str,
    username: &str,
) -> Result<Option<Account>> {
    conn.query_row(
        "SELECT server_url, forge_kind, username, email, token, is_default, last_used
         FROM accounts WHERE server_url = ?1 AND forge_kind = ?2 AND username = ?3",
        params![server_url, forge_kind, username],
        row_to_account,
    )
    .optional()
    .map_err(|e| Error::Database(e.to_string()))
}

/// Get the default account for a server (highest is_default, most recent last_used).
pub fn get_default_account(conn: &Connection, server_url: &str) -> Result<Option<Account>> {
    conn.query_row(
        "SELECT server_url, forge_kind, username, email, token, is_default, last_used
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
            "SELECT server_url, forge_kind, username, email, token, is_default, last_used
             FROM accounts WHERE server_url = ?1
             ORDER BY is_default DESC, last_used DESC",
        )?;
        let accounts = stmt
            .query_map(params![url], row_to_account)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(accounts)
    } else {
        let mut stmt = conn.prepare(
            "SELECT server_url, forge_kind, username, email, token, is_default, last_used
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
pub fn delete_account(
    conn: &Connection,
    server_url: &str,
    username: Option<&str>,
    forge_kind: Option<&str>,
) -> Result<()> {
    match (username, forge_kind) {
        (Some(user), Some(forge)) => {
            conn.execute(
                "DELETE FROM accounts WHERE server_url = ?1 AND forge_kind = ?2 AND username = ?3",
                params![server_url, forge, user],
            )?;
        }
        (Some(user), None) => {
            conn.execute(
                "DELETE FROM accounts WHERE server_url = ?1 AND username = ?2",
                params![server_url, user],
            )?;
        }
        (None, Some(forge)) => {
            conn.execute(
                "DELETE FROM accounts WHERE server_url = ?1 AND forge_kind = ?2",
                params![server_url, forge],
            )?;
        }
        (None, None) => {
            conn.execute("DELETE FROM accounts WHERE server_url = ?1", params![server_url])?;
        }
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
        "SELECT server_url, forge_kind, username, email, token, is_default, last_used
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
/// 1. Exact username + forge_kind match → return token
/// 2. Email match → return token
/// 3. Default account (is_default DESC, last_used DESC) → return token
pub fn resolve_token(
    conn: &Connection,
    server_url: &str,
    identity_hint: Option<&str>,
    forge_kind: Option<&str>,
) -> Result<Option<String>> {
    if let Some(hint) = identity_hint {
        // Try exact username + forge_kind match when forge is known
        if let Some(forge) = forge_kind {
            if let Some(account) = get_account(conn, server_url, forge, hint)? {
                return Ok(Some(account.token));
            }
        } else {
            // Try username-only match across any forge_kind
            let username_match: Option<String> = conn
                .query_row(
                    "SELECT token FROM accounts WHERE server_url = ?1 AND username = ?2
                     ORDER BY is_default DESC, last_used DESC LIMIT 1",
                    params![server_url, hint],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| Error::Database(e.to_string()))?;
            if let Some(token) = username_match {
                return Ok(Some(token));
            }
        }

        // Try email match
        let email_match: Option<String> = conn
            .query_row(
                "SELECT token FROM accounts WHERE server_url = ?1 AND email = ?2
                     ORDER BY is_default DESC, last_used DESC LIMIT 1",
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

/// Reconcile a repository's location in the global registry.
///
/// The registry is keyed on `repo_root` (the local `repos.id` is *not* a durable
/// cross-database identity — it is regenerated whenever `.codemark/` is recreated, so
/// it cannot be used to recognize a moved repo). Instead, when a repo has moved we
/// recognize the moved-from row by its durable `(repo_owner, repo_name)` identity.
///
/// Behavior:
/// - If exactly one existing row shares this repo's `(repo_owner, repo_name)` at a
///   different path that no longer exists on disk, it is treated as the moved-from
///   row: its `server_url`/`default_username` are carried over to the new location and
///   the stale row is removed. Ambiguous cases (zero or multiple stale candidates) are
///   left untouched and the current path is simply registered.
/// - The current path is then UPSERTed (keyed on `repo_root`). Any other row carrying
///   the same `id` at a different path is removed first to avoid an `id` primary-key
///   collision.
///
/// Moved-from detection only applies to repos with an `origin_url`; a local-only repo
/// is indistinguishable from a brand-new one when its path changes, so its old entry is
/// left for `codemark repo prune` to clean up.
pub fn reconcile_repo(conn: &Connection, repo: &RepoUpsert<'_>) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();

    // Run the whole reconcile (predecessor delete, stale-row cleanup, and the final
    // upsert) in a single transaction. The predecessor DELETE is the only place the
    // inherited server_url/default_username are read from; without atomicity, a crash
    // after that DELETE but before the INSERT would durably lose them.
    let tx = conn.unchecked_transaction()?;

    // Identify a single unambiguous moved-from predecessor to inherit config from.
    let predecessor = find_move_predecessor(&tx, repo)?;
    let inherited_server_url = predecessor.as_ref().and_then(|p| p.server_url.clone());
    let inherited_default_username = predecessor.as_ref().and_then(|p| p.default_username.clone());
    if let Some(p) = &predecessor {
        tx.execute("DELETE FROM known_repos WHERE id = ?1", params![p.id])?;
    }

    // Clear any stale row occupying the target path under a different identity, and any
    // other row carrying this id at a different path, so the UPSERT below collides with
    // neither the UNIQUE(repo_root) constraint nor the id primary key.
    tx.execute(
        "DELETE FROM known_repos WHERE repo_root = ?1 AND id != ?2",
        params![repo.repo_root, repo.id],
    )?;
    tx.execute(
        "DELETE FROM known_repos WHERE id = ?1 AND repo_root != ?2",
        params![repo.id, repo.repo_root],
    )?;

    let server_url = repo.server_url.map(str::to_string).or(inherited_server_url);
    let default_username = repo.default_username.map(str::to_string).or(inherited_default_username);

    tx.execute(
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
            server_url,
            default_username,
        ],
    )?;

    tx.commit()?;
    Ok(())
}

/// Find the single unambiguous registry row that this repo moved away from.
///
/// A candidate is a row with the same `origin_url` and `(repo_owner, repo_name)` at a
/// different `repo_root` that no longer exists on disk. Constraining on `origin_url`
/// prevents matching an unrelated repo that merely shares the same owner/name (e.g. the
/// same `owner/name` on `github.com` vs. a private GitHub Enterprise host). Returns
/// `Some` only when there is exactly one such candidate; zero or multiple candidates are
/// ambiguous and yield `None` (the caller then just registers the current path rather
/// than guessing). Local-only repos (no `origin_url`) are never reconciled this way.
fn find_move_predecessor(conn: &Connection, repo: &RepoUpsert<'_>) -> Result<Option<KnownRepo>> {
    let Some(origin_url) = repo.origin_url else {
        return Ok(None);
    };

    let mut stmt = conn.prepare(
        "SELECT id, repo_owner, repo_name, origin_url, repo_root, db_owner_email, db_owner_name, detected_at, last_seen_at, server_url, default_username
         FROM known_repos WHERE repo_owner = ?1 AND repo_name = ?2 AND origin_url = ?3 AND repo_root != ?4",
    )?;
    let candidates = stmt
        .query_map(
            params![repo.repo_owner, repo.repo_name, origin_url, repo.repo_root],
            row_to_known_repo,
        )?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    let mut stale = candidates.into_iter().filter(|r| !r.repo_root.exists());
    match (stale.next(), stale.next()) {
        (Some(only), None) => Ok(Some(only)),
        _ => Ok(None),
    }
}

/// List registry entries whose `repo_root` no longer exists on disk.
pub fn find_stale_repos(conn: &Connection) -> Result<Vec<KnownRepo>> {
    Ok(list_repos(conn)?.into_iter().filter(|repo| !repo.repo_root.exists()).collect())
}

/// Remove registry entries whose `repo_root` no longer exists on disk.
///
/// Returns the list of removed repositories. This is an explicit, opt-in operation
/// (`codemark repo prune`); sync never prunes automatically, since a missing path may
/// be a temporarily unmounted volume rather than a deleted repository.
pub fn prune_repos(conn: &Connection) -> Result<Vec<KnownRepo>> {
    let stale = find_stale_repos(conn)?;
    // Delete all stale rows atomically so a mid-loop crash can't leave the prune
    // half-applied.
    let tx = conn.unchecked_transaction()?;
    for repo in &stale {
        tx.execute("DELETE FROM known_repos WHERE id = ?1", params![repo.id])?;
    }
    tx.commit()?;
    Ok(stale)
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

/// Update the default username for a repository.
pub fn set_default_username(
    conn: &Connection,
    repo_root: &str,
    username: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE known_repos SET default_username = ?1 WHERE repo_root = ?2",
        params![username, repo_root],
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
    fn test_set_default_username() {
        let conn = test_registry().unwrap();

        upsert_repo(
            &conn,
            &RepoUpsert {
                id: "test-id-3",
                repo_owner: "owner",
                repo_name: "repo",
                origin_url: None,
                repo_root: "/some/path",
                db_owner_email: "user@example.com",
                db_owner_name: None,
                server_url: None,
                default_username: None,
            },
        )
        .unwrap();

        // Initially None
        let repo = find_repo_by_root(&conn, "/some/path").unwrap().unwrap();
        assert_eq!(repo.default_username, None);

        // Set username
        set_default_username(&conn, "/some/path", Some("alice")).unwrap();
        let repo = find_repo_by_root(&conn, "/some/path").unwrap().unwrap();
        assert_eq!(repo.default_username, Some("alice".to_string()));

        // Clear username
        set_default_username(&conn, "/some/path", None).unwrap();
        let repo = find_repo_by_root(&conn, "/some/path").unwrap().unwrap();
        assert_eq!(repo.default_username, None);
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
    fn test_reconcile_repo_updates_path_on_move() {
        let conn = test_registry().unwrap();

        // Register a repo at its original (now nonexistent) path, with a server URL.
        // Use a different id than the one reconcile will supply, mirroring reality: the
        // registry id is a stale snapshot of a past local id and need not match the
        // current local repos.id.
        upsert_repo(
            &conn,
            &RepoUpsert {
                id: "old-registry-id",
                repo_owner: "owner",
                repo_name: "repo",
                origin_url: Some("https://github.com/owner/repo"),
                repo_root: "/old/path",
                db_owner_email: "owner@example.com",
                db_owner_name: Some("Owner"),
                server_url: None,
                default_username: None,
            },
        )
        .unwrap();
        set_server_url(&conn, "/old/path", Some("https://codemark.example.com")).unwrap();

        // The repo moves; reconcile from the new location with a *different* local id.
        // The moved-from row is matched by (repo_owner, repo_name), not id.
        reconcile_repo(
            &conn,
            &RepoUpsert {
                id: "current-local-id",
                repo_owner: "owner",
                repo_name: "repo",
                origin_url: Some("https://github.com/owner/repo"),
                repo_root: "/new/path",
                db_owner_email: "owner@example.com",
                db_owner_name: Some("Owner"),
                // The new path has no row yet, so no server_url is supplied; it must be
                // inherited from the moved-from row.
                server_url: None,
                default_username: None,
            },
        )
        .unwrap();

        // Exactly one row remains, at the new path, with server_url carried over and the
        // current local id adopted.
        let repos = list_repos(&conn).unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].id, "current-local-id");
        assert_eq!(repos[0].repo_root, PathBuf::from("/new/path"));
        assert_eq!(repos[0].server_url, Some("https://codemark.example.com".to_string()));
        assert!(find_repo_by_root(&conn, "/old/path").unwrap().is_none());
    }

    #[test]
    fn test_reconcile_repo_does_not_match_different_origin() {
        let conn = test_registry().unwrap();

        // A stale row with the same owner/name but a *different* origin (e.g. a private
        // GitHub Enterprise host vs github.com). It must NOT be treated as a predecessor.
        upsert_repo(
            &conn,
            &RepoUpsert {
                id: "enterprise-id",
                repo_owner: "owner",
                repo_name: "repo",
                origin_url: Some("https://github.example.com/owner/repo"),
                repo_root: "/gone/enterprise",
                db_owner_email: "owner@example.com",
                db_owner_name: None,
                server_url: None,
                default_username: None,
            },
        )
        .unwrap();
        set_server_url(&conn, "/gone/enterprise", Some("https://enterprise.internal")).unwrap();

        reconcile_repo(
            &conn,
            &RepoUpsert {
                id: "github-id",
                repo_owner: "owner",
                repo_name: "repo",
                origin_url: Some("https://github.com/owner/repo"),
                repo_root: "/current/path",
                db_owner_email: "owner@example.com",
                db_owner_name: None,
                server_url: None,
                default_username: None,
            },
        )
        .unwrap();

        // Both rows coexist; the unrelated enterprise server_url was NOT carried over.
        let repos = list_repos(&conn).unwrap();
        assert_eq!(repos.len(), 2);
        let current = find_repo_by_root(&conn, "/current/path").unwrap().unwrap();
        assert_eq!(current.server_url, None);
        assert!(find_repo_by_root(&conn, "/gone/enterprise").unwrap().is_some());
    }

    #[test]
    fn test_reconcile_repo_ambiguous_move_registers_current() {
        let conn = test_registry().unwrap();

        // Two existing rows share the same (owner, repo) at different nonexistent paths.
        for (id, root) in [("id-a", "/gone/a"), ("id-b", "/gone/b")] {
            upsert_repo(
                &conn,
                &RepoUpsert {
                    id,
                    repo_owner: "owner",
                    repo_name: "repo",
                    origin_url: Some("https://github.com/owner/repo"),
                    repo_root: root,
                    db_owner_email: "owner@example.com",
                    db_owner_name: None,
                    server_url: None,
                    default_username: None,
                },
            )
            .unwrap();
        }
        set_server_url(&conn, "/gone/a", Some("https://codemark.example.com")).unwrap();

        // Ambiguous: two stale candidates. Reconcile must just register the current path
        // without guessing which one moved (no carry-over, no deletion).
        reconcile_repo(
            &conn,
            &RepoUpsert {
                id: "id-current",
                repo_owner: "owner",
                repo_name: "repo",
                origin_url: Some("https://github.com/owner/repo"),
                repo_root: "/current/path",
                db_owner_email: "owner@example.com",
                db_owner_name: None,
                server_url: None,
                default_username: None,
            },
        )
        .unwrap();

        // All three rows coexist; the new one inherited no server_url.
        let repos = list_repos(&conn).unwrap();
        assert_eq!(repos.len(), 3);
        let current = find_repo_by_root(&conn, "/current/path").unwrap().unwrap();
        assert_eq!(current.id, "id-current");
        assert_eq!(current.server_url, None);
        // The stale rows remain for `prune` to clean up.
        assert!(find_repo_by_root(&conn, "/gone/a").unwrap().is_some());
        assert!(find_repo_by_root(&conn, "/gone/b").unwrap().is_some());
    }

    #[test]
    fn test_reconcile_repo_clears_stale_occupant_of_path() {
        let conn = test_registry().unwrap();

        // A stale row points at /shared/path under a different id.
        upsert_repo(
            &conn,
            &RepoUpsert {
                id: "stale-id",
                repo_owner: "owner",
                repo_name: "old",
                origin_url: None,
                repo_root: "/shared/path",
                db_owner_email: "owner@example.com",
                db_owner_name: None,
                server_url: None,
                default_username: None,
            },
        )
        .unwrap();

        // A different repo (new id) is now at that path.
        reconcile_repo(
            &conn,
            &RepoUpsert {
                id: "fresh-id",
                repo_owner: "owner",
                repo_name: "new",
                origin_url: None,
                repo_root: "/shared/path",
                db_owner_email: "owner@example.com",
                db_owner_name: None,
                server_url: None,
                default_username: None,
            },
        )
        .unwrap();

        let repos = list_repos(&conn).unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].id, "fresh-id");
        assert_eq!(repos[0].repo_name, "new");
    }

    #[test]
    fn test_prune_removes_only_nonexistent_paths() {
        let conn = test_registry().unwrap();

        // One repo at a real path (the temp dir), one at a bogus path.
        let real_dir = std::env::temp_dir();
        let real_root = real_dir.to_string_lossy().to_string();

        upsert_repo(
            &conn,
            &RepoUpsert {
                id: "real-id",
                repo_owner: "owner",
                repo_name: "real",
                origin_url: None,
                repo_root: &real_root,
                db_owner_email: "owner@example.com",
                db_owner_name: None,
                server_url: None,
                default_username: None,
            },
        )
        .unwrap();
        upsert_repo(
            &conn,
            &RepoUpsert {
                id: "gone-id",
                repo_owner: "owner",
                repo_name: "gone",
                origin_url: None,
                repo_root: "/definitely/not/a/real/path/codemark-test",
                db_owner_email: "owner@example.com",
                db_owner_name: None,
                server_url: None,
                default_username: None,
            },
        )
        .unwrap();

        // Dry-run reports the stale one without deleting.
        let stale = find_stale_repos(&conn).unwrap();
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].id, "gone-id");
        assert_eq!(list_repos(&conn).unwrap().len(), 2);

        // Prune removes only the nonexistent path.
        let removed = prune_repos(&conn).unwrap();
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].id, "gone-id");

        let remaining = list_repos(&conn).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, "real-id");
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
                forge_kind: "github",
                username: "alice",
                email: Some("alice@example.com"),
                token: "token123",
                is_default: true,
            },
        )
        .unwrap();

        let account =
            get_account(&conn, "https://codemark.example.com", "github", "alice").unwrap().unwrap();
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
                forge_kind: "github",
                username: "alice",
                email: Some("alice@example.com"),
                token: "new_token",
                is_default: true,
            },
        )
        .unwrap();
        let account =
            get_account(&conn, "https://codemark.example.com", "github", "alice").unwrap().unwrap();
        assert_eq!(account.token, "new_token");
    }

    #[test]
    fn test_list_accounts() {
        let conn = test_registry();

        upsert_account(
            &conn,
            &AccountUpsert {
                server_url: "https://server1.com",
                forge_kind: "github",
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
                forge_kind: "github",
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
                forge_kind: "github",
                username: "user",
                email: None,
                token: "token",
                is_default: true,
            },
        )
        .unwrap();
        assert!(get_account(&conn, "https://server.com", "github", "user").unwrap().is_some());

        delete_account(&conn, "https://server.com", Some("user"), None).unwrap();
        assert!(get_account(&conn, "https://server.com", "github", "user").unwrap().is_none());
    }

    #[test]
    fn test_delete_all_accounts_for_server() {
        let conn = test_registry();

        upsert_account(
            &conn,
            &AccountUpsert {
                server_url: "https://server.com",
                forge_kind: "github",
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
                forge_kind: "github",
                username: "bob",
                email: None,
                token: "token2",
                is_default: false,
            },
        )
        .unwrap();

        assert_eq!(list_accounts(&conn, Some("https://server.com")).unwrap().len(), 2);

        // Delete all accounts for server (username = None)
        delete_account(&conn, "https://server.com", None, None).unwrap();
        assert_eq!(list_accounts(&conn, Some("https://server.com")).unwrap().len(), 0);
    }

    #[test]
    fn test_resolve_token_by_username() {
        let conn = test_registry();

        upsert_account(
            &conn,
            &AccountUpsert {
                server_url: "https://server.com",
                forge_kind: "github",
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
                forge_kind: "github",
                username: "bob",
                email: Some("bob@example.com"),
                token: "bob_token",
                is_default: true,
            },
        )
        .unwrap();

        // Exact username match
        let token =
            resolve_token(&conn, "https://server.com", Some("alice"), Some("github")).unwrap();
        assert_eq!(token, Some("alice_token".to_string()));
    }

    #[test]
    fn test_resolve_token_by_username_no_forge() {
        let conn = test_registry();

        upsert_account(
            &conn,
            &AccountUpsert {
                server_url: "https://server.com",
                forge_kind: "github",
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
                forge_kind: "github",
                username: "bob",
                email: None,
                token: "bob_token",
                is_default: true,
            },
        )
        .unwrap();

        // Username hint without forge_kind still resolves to the correct account
        let token = resolve_token(&conn, "https://server.com", Some("alice"), None).unwrap();
        assert_eq!(token, Some("alice_token".to_string()));
    }

    #[test]
    fn test_resolve_token_by_email() {
        let conn = test_registry();

        upsert_account(
            &conn,
            &AccountUpsert {
                server_url: "https://server.com",
                forge_kind: "github",
                username: "alice",
                email: Some("alice@example.com"),
                token: "alice_token",
                is_default: false,
            },
        )
        .unwrap();

        // Email match
        let token =
            resolve_token(&conn, "https://server.com", Some("alice@example.com"), Some("github"))
                .unwrap();
        assert_eq!(token, Some("alice_token".to_string()));
    }

    #[test]
    fn test_resolve_token_default_fallback() {
        let conn = test_registry();

        upsert_account(
            &conn,
            &AccountUpsert {
                server_url: "https://server.com",
                forge_kind: "github",
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
                forge_kind: "github",
                username: "bob",
                email: None,
                token: "bob_token",
                is_default: true,
            },
        )
        .unwrap();

        // No hint → falls back to default (bob, is_default=true)
        let token = resolve_token(&conn, "https://server.com", None, None).unwrap();
        assert_eq!(token, Some("bob_token".to_string()));

        // Non-matching hint → also falls back to default
        let token = resolve_token(&conn, "https://server.com", Some("nonexistent"), Some("github"))
            .unwrap();
        assert_eq!(token, Some("bob_token".to_string()));
    }

    #[test]
    fn test_multiple_accounts_per_server() {
        let conn = test_registry();

        upsert_account(
            &conn,
            &AccountUpsert {
                server_url: "https://server.com",
                forge_kind: "github",
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
                forge_kind: "github",
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
        let matched =
            get_account(&conn, "https://server.com", "github", "user@example.com").unwrap();
        assert!(matched.is_some());
        assert_eq!(matched.unwrap().token, "old_token");

        // The server with no matching repo should use "default"
        let unmatched = get_account(&conn, "https://other.com", "github", "default").unwrap();
        assert!(unmatched.is_some());
        assert_eq!(unmatched.unwrap().token, "other_token");

        // The repo should have default_username set
        let repo = find_repo_by_root(&conn, "/path/to/repo").unwrap().unwrap();
        assert_eq!(repo.default_username, Some("user@example.com".to_string()));
    }
}
