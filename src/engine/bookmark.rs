use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

// --- Enums ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BookmarkStatus {
    Active,
    Drifted,
    Stale,
    Archived,
}

impl fmt::Display for BookmarkStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BookmarkStatus::Active => write!(f, "active"),
            BookmarkStatus::Drifted => write!(f, "drifted"),
            BookmarkStatus::Stale => write!(f, "stale"),
            BookmarkStatus::Archived => write!(f, "archived"),
        }
    }
}

impl FromStr for BookmarkStatus {
    type Err = crate::error::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "active" => Ok(BookmarkStatus::Active),
            "drifted" => Ok(BookmarkStatus::Drifted),
            "stale" => Ok(BookmarkStatus::Stale),
            "archived" => Ok(BookmarkStatus::Archived),
            _ => Err(crate::error::Error::Input(format!("invalid bookmark status: {s}"))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionMethod {
    Exact,
    Relaxed,
    HashFallback,
    Failed,
}

impl fmt::Display for ResolutionMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolutionMethod::Exact => write!(f, "exact"),
            ResolutionMethod::Relaxed => write!(f, "relaxed"),
            ResolutionMethod::HashFallback => write!(f, "hash_fallback"),
            ResolutionMethod::Failed => write!(f, "failed"),
        }
    }
}

impl FromStr for ResolutionMethod {
    type Err = crate::error::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "exact" => Ok(ResolutionMethod::Exact),
            "relaxed" => Ok(ResolutionMethod::Relaxed),
            "hash_fallback" => Ok(ResolutionMethod::HashFallback),
            "failed" => Ok(ResolutionMethod::Failed),
            _ => Err(crate::error::Error::Input(format!("invalid resolution method: {s}"))),
        }
    }
}

// --- Domain structs ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Annotation {
    pub id: String,
    pub bookmark_id: String,
    pub added_at: String,
    pub added_by: Option<String>,
    pub notes: Option<String>,
    pub context: Option<String>,
    pub source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tag {
    pub bookmark_id: String,
    pub tag: String,
    pub added_at: String,
    pub added_by: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bookmark {
    pub id: String,
    pub query: String,
    pub language: String,
    pub file_path: String,
    pub content_hash: Option<String>,
    pub commit_hash: Option<String>,
    pub status: BookmarkStatus,
    pub resolution_method: Option<ResolutionMethod>,
    pub last_resolved_at: Option<String>,
    pub stale_since: Option<String>,
    pub created_at: String,
    pub created_by: Option<String>,
    // Aggregated from related tables
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub annotations: Vec<Annotation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Resolution {
    pub id: String,
    pub bookmark_id: String,
    pub resolved_at: String,
    pub commit_hash: Option<String>,
    pub method: ResolutionMethod,
    pub match_count: Option<i32>,
    pub file_path: Option<String>,
    pub byte_range: Option<String>,
    pub line_range: Option<String>,
    pub content_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Collection {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub created_at: String,
    pub created_by: Option<String>,
}

// --- Filter ---

#[derive(Debug, Default)]
pub struct BookmarkFilter {
    pub tag: Option<String>,
    pub status: Option<Vec<BookmarkStatus>>,
    pub file_path: Option<String>,
    pub language: Option<String>,
    pub created_by: Option<String>,
    pub collection: Option<String>,
    pub limit: Option<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bookmark_status_round_trip() {
        for status in [
            BookmarkStatus::Active,
            BookmarkStatus::Drifted,
            BookmarkStatus::Stale,
            BookmarkStatus::Archived,
        ] {
            let s = status.to_string();
            let parsed: BookmarkStatus = s.parse().unwrap();
            assert_eq!(status, parsed);
        }
    }

    #[test]
    fn resolution_method_round_trip() {
        for method in [
            ResolutionMethod::Exact,
            ResolutionMethod::Relaxed,
            ResolutionMethod::HashFallback,
            ResolutionMethod::Failed,
        ] {
            let s = method.to_string();
            let parsed: ResolutionMethod = s.parse().unwrap();
            assert_eq!(method, parsed);
        }
    }

    #[test]
    fn invalid_status_returns_error() {
        assert!("bogus".parse::<BookmarkStatus>().is_err());
    }

    #[test]
    fn bookmark_serializes_to_json() {
        let bookmark = Bookmark {
            id: "test-id".into(),
            query: "(function_declaration) @target".into(),
            language: "swift".into(),
            file_path: "src/main.swift".into(),
            content_hash: Some("sha256:abcd1234abcd1234".into()),
            commit_hash: None,
            status: BookmarkStatus::Active,
            resolution_method: Some(ResolutionMethod::Exact),
            last_resolved_at: None,
            stale_since: None,
            created_at: "2026-04-01T00:00:00Z".into(),
            created_by: None,
            tags: vec!["auth".into()],
            annotations: vec![],
        };
        let json = serde_json::to_string(&bookmark).unwrap();
        let parsed: Bookmark = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "test-id");
        assert_eq!(parsed.status, BookmarkStatus::Active);
    }
}
