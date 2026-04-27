//! Repository metadata CRUD operations.

use rusqlite::Row;

use crate::engine::bookmark::Repo;
use crate::error::{Error, Result};
use crate::storage::db::Database;

impl Database {
    /// Insert or update repository metadata.
    /// Returns the repo ID.
    /// Returns the repo ID.
    pub fn upsert_repo(&self, repo: &Repo) -> Result<String> {
        // Try insert first
        let result = self.conn().execute(
            "INSERT INTO repos (id, repo_owner, repo_name, origin_url, repo_root, db_owner_email, db_owner_name, detected_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(origin_url) DO UPDATE SET
                repo_owner = excluded.repo_owner,
                repo_name = excluded.repo_name,
                repo_root = excluded.repo_root,
                db_owner_email = excluded.db_owner_email,
                db_owner_name = excluded.db_owner_name,
                detected_at = excluded.detected_at",
            (
                &repo.id,
                &repo.repo_owner,
                &repo.repo_name,
                &repo.origin_url,
                &repo.repo_root,
                &repo.db_owner_email,
                &repo.db_owner_name,
                &repo.detected_at,
            ),
        );

        match result {
            Ok(_) => Ok(repo.id.clone()),
            Err(e) => Err(Error::Database(format!("failed to upsert repo: {e}"))),
        }
    }

    /// Get repository by origin URL.
    pub fn get_repo_by_origin(&self, origin_url: &str) -> Result<Option<Repo>> {
        let mut stmt = self
            .conn()
            .prepare("SELECT id, repo_owner, repo_name, origin_url, repo_root, db_owner_email, db_owner_name, detected_at FROM repos WHERE origin_url = ?1")?;
        let mut rows = stmt.query_map([origin_url], row_to_repo)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// Get repository by ID.
    pub fn get_repo(&self, id: &str) -> Result<Option<Repo>> {
        let mut stmt = self
            .conn()
            .prepare("SELECT id, repo_owner, repo_name, origin_url, repo_root, db_owner_email, db_owner_name, detected_at FROM repos WHERE id = ?1")?;
        let mut rows = stmt.query_map([id], row_to_repo)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// Get the database owner email from any repo entry.
    /// Returns the first db_owner_email found, or None if repos table is empty.
    pub fn get_db_owner(&self) -> Result<Option<String>> {
        let mut stmt = self.conn().prepare("SELECT DISTINCT db_owner_email FROM repos LIMIT 1")?;
        let mut rows = stmt.query_map([], |row| row.get(0))?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// Get repository by repo_root.
    /// Used for local-only repos (no origin_url) to prevent duplicates.
    pub fn get_repo_by_root(&self, repo_root: &str) -> Result<Option<Repo>> {
        let mut stmt = self
            .conn()
            .prepare("SELECT id, repo_owner, repo_name, origin_url, repo_root, db_owner_email, db_owner_name, detected_at FROM repos WHERE repo_root = ?1")?;
        let mut rows = stmt.query_map([repo_root], row_to_repo)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// List all repositories.
    pub fn list_repos(&self) -> Result<Vec<Repo>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, repo_owner, repo_name, origin_url, repo_root, db_owner_email, db_owner_name, detected_at
             FROM repos ORDER BY repo_owner, repo_name",
        )?;
        let rows = stmt.query_map([], row_to_repo)?;
        let results: Vec<Repo> = rows.filter_map(|r| r.ok()).collect();
        Ok(results)
    }
}

fn row_to_repo(row: &Row) -> rusqlite::Result<Repo> {
    Ok(Repo {
        id: row.get(0)?,
        repo_owner: row.get(1)?,
        repo_name: row.get(2)?,
        origin_url: row.get(3)?,
        repo_root: row.get(4)?,
        db_owner_email: row.get(5)?,
        db_owner_name: row.get(6)?,
        detected_at: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn test_insert_and_get_repo() {
        let db = Database::open_in_memory().unwrap();

        let repo = Repo {
            id: Uuid::new_v4().to_string(),
            repo_owner: "testowner".to_string(),
            repo_name: "testrepo".to_string(),
            origin_url: Some("https://github.com/testowner/testrepo.git".to_string()),
            repo_root: "/tmp/test".to_string(),
            db_owner_email: "test@example.com".to_string(),
            db_owner_name: Some("Test User".to_string()),
            detected_at: "2024-01-01T00:00:00Z".to_string(),
        };

        let repo_id = db.upsert_repo(&repo).unwrap();
        assert_eq!(repo_id, repo.id);

        let retrieved = db.get_repo(&repo_id).unwrap().unwrap();
        assert_eq!(retrieved.repo_owner, "testowner");
        assert_eq!(retrieved.repo_name, "testrepo");
        assert_eq!(retrieved.db_owner_email, "test@example.com");
    }

    #[test]
    fn test_upsert_repo_updates_existing() {
        let db = Database::open_in_memory().unwrap();

        let id = Uuid::new_v4().to_string();
        let mut repo = Repo {
            id: id.clone(),
            repo_owner: "testowner".to_string(),
            repo_name: "testrepo".to_string(),
            origin_url: Some("https://github.com/testowner/testrepo.git".to_string()),
            repo_root: "/tmp/test".to_string(),
            db_owner_email: "test@example.com".to_string(),
            db_owner_name: Some("Test User".to_string()),
            detected_at: "2024-01-01T00:00:00Z".to_string(),
        };

        db.upsert_repo(&repo).unwrap();

        // Update with new owner name
        repo.db_owner_name = Some("Updated Name".to_string());
        db.upsert_repo(&repo).unwrap();

        let retrieved =
            db.get_repo_by_origin("https://github.com/testowner/testrepo.git").unwrap().unwrap();
        assert_eq!(retrieved.db_owner_name, Some("Updated Name".to_string()));
        // ID should remain the same
        assert_eq!(retrieved.id, id);
    }

    #[test]
    fn test_get_repo_by_origin() {
        let db = Database::open_in_memory().unwrap();

        let repo = Repo {
            id: Uuid::new_v4().to_string(),
            repo_owner: "testowner".to_string(),
            repo_name: "testrepo".to_string(),
            origin_url: Some("https://github.com/testowner/testrepo.git".to_string()),
            repo_root: "/tmp/test".to_string(),
            db_owner_email: "test@example.com".to_string(),
            db_owner_name: Some("Test User".to_string()),
            detected_at: "2024-01-01T00:00:00Z".to_string(),
        };

        db.upsert_repo(&repo).unwrap();

        let retrieved =
            db.get_repo_by_origin("https://github.com/testowner/testrepo.git").unwrap().unwrap();
        assert_eq!(retrieved.repo_owner, "testowner");

        // Test non-existent origin
        let not_found = db.get_repo_by_origin("https://example.com/nonexistent.git").unwrap();
        assert!(not_found.is_none());
    }

    #[test]
    fn test_get_repo_by_root() {
        let db = Database::open_in_memory().unwrap();

        let repo = Repo {
            id: Uuid::new_v4().to_string(),
            repo_owner: "testowner".to_string(),
            repo_name: "testrepo".to_string(),
            origin_url: None, // Local repo, no origin
            repo_root: "/tmp/test".to_string(),
            db_owner_email: "test@example.com".to_string(),
            db_owner_name: Some("Test User".to_string()),
            detected_at: "2024-01-01T00:00:00Z".to_string(),
        };

        db.upsert_repo(&repo).unwrap();

        let retrieved = db.get_repo_by_root("/tmp/test").unwrap().unwrap();
        assert_eq!(retrieved.db_owner_email, "test@example.com");

        // Test non-existent root
        let not_found = db.get_repo_by_root("/tmp/nonexistent").unwrap();
        assert!(not_found.is_none());
    }

    #[test]
    fn test_get_db_owner() {
        let db = Database::open_in_memory().unwrap();

        // No repos yet
        assert!(db.get_db_owner().unwrap().is_none());

        let repo = Repo {
            id: Uuid::new_v4().to_string(),
            repo_owner: "testowner".to_string(),
            repo_name: "testrepo".to_string(),
            origin_url: Some("https://github.com/testowner/testrepo.git".to_string()),
            repo_root: "/tmp/test".to_string(),
            db_owner_email: "test@example.com".to_string(),
            db_owner_name: Some("Test User".to_string()),
            detected_at: "2024-01-01T00:00:00Z".to_string(),
        };

        db.upsert_repo(&repo).unwrap();

        let owner = db.get_db_owner().unwrap().unwrap();
        assert_eq!(owner, "test@example.com");
    }
}
