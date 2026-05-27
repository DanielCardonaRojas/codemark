use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

// --- Enums ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BookmarkHealth {
    Active,
    Drifted,
    Stale,
    Archived,
}

impl fmt::Display for BookmarkHealth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BookmarkHealth::Active => write!(f, "active"),
            BookmarkHealth::Drifted => write!(f, "drifted"),
            BookmarkHealth::Stale => write!(f, "stale"),
            BookmarkHealth::Archived => write!(f, "archived"),
        }
    }
}

impl FromStr for BookmarkHealth {
    type Err = crate::error::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "active" => Ok(BookmarkHealth::Active),
            "drifted" => Ok(BookmarkHealth::Drifted),
            "stale" => Ok(BookmarkHealth::Stale),
            "archived" => Ok(BookmarkHealth::Archived),
            _ => Err(crate::error::Error::Input(format!("invalid bookmark health: {s}"))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollectionHealth {
    Active,
    Drifted,
    Stale,
}

impl fmt::Display for CollectionHealth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CollectionHealth::Active => write!(f, "active"),
            CollectionHealth::Drifted => write!(f, "drifted"),
            CollectionHealth::Stale => write!(f, "stale"),
        }
    }
}

impl FromStr for CollectionHealth {
    type Err = crate::error::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "active" => Ok(CollectionHealth::Active),
            "drifted" => Ok(CollectionHealth::Drifted),
            "stale" => Ok(CollectionHealth::Stale),
            _ => Err(crate::error::Error::Input(format!("invalid collection health: {s}"))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UIStatus {
    /// Is Current + active - Perfect match at current HEAD
    Healthy,
    /// Is Current + drifted - Found at HEAD but code changed
    Drifted,
    /// Is Current + stale - Not found at current HEAD
    Broken,
    /// Past + active - Was perfect match in previous commit
    Verified,
    /// Past + drifted - Was partial match in previous commit
    Outdated,
    /// Future + Any - Recorded at commit ahead of current HEAD
    Future,
}

impl fmt::Display for UIStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UIStatus::Healthy => write!(f, "healthy"),
            UIStatus::Drifted => write!(f, "drifted"),
            UIStatus::Broken => write!(f, "broken"),
            UIStatus::Verified => write!(f, "verified"),
            UIStatus::Outdated => write!(f, "outdated"),
            UIStatus::Future => write!(f, "future"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollectionLinkKind {
    Pr,
    Issue,
    Doc,
    Discussion,
    Dashboard,
    Repo,
    Tour,
    Other,
}

impl CollectionLinkKind {
    pub const ALL_VARIANTS: &'static [&'static str] =
        &["pr", "issue", "doc", "discussion", "dashboard", "repo", "tour", "other"];
}

impl fmt::Display for CollectionLinkKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CollectionLinkKind::Pr => write!(f, "pr"),
            CollectionLinkKind::Issue => write!(f, "issue"),
            CollectionLinkKind::Doc => write!(f, "doc"),
            CollectionLinkKind::Discussion => write!(f, "discussion"),
            CollectionLinkKind::Dashboard => write!(f, "dashboard"),
            CollectionLinkKind::Repo => write!(f, "repo"),
            CollectionLinkKind::Tour => write!(f, "tour"),
            CollectionLinkKind::Other => write!(f, "other"),
        }
    }
}

impl FromStr for CollectionLinkKind {
    type Err = crate::error::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pr" => Ok(CollectionLinkKind::Pr),
            "issue" => Ok(CollectionLinkKind::Issue),
            "doc" => Ok(CollectionLinkKind::Doc),
            "discussion" => Ok(CollectionLinkKind::Discussion),
            "dashboard" => Ok(CollectionLinkKind::Dashboard),
            "repo" => Ok(CollectionLinkKind::Repo),
            "tour" => Ok(CollectionLinkKind::Tour),
            "other" => Ok(CollectionLinkKind::Other),
            _ => Err(crate::error::Error::Input(format!("invalid collection link kind: {s}"))),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    Public,
    Private,
}

impl fmt::Display for Visibility {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Visibility::Public => write!(f, "public"),
            Visibility::Private => write!(f, "private"),
        }
    }
}

impl FromStr for Visibility {
    type Err = crate::error::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "public" => Ok(Visibility::Public),
            "private" => Ok(Visibility::Private),
            _ => Err(crate::error::Error::Input(format!("invalid visibility: {s}"))),
        }
    }
}

// --- Domain structs ---

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookmarkComment {
    pub id: String,
    pub bookmark_id: String,
    pub author: String,
    pub body: String,
    pub created_at: String,
    pub parent_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Annotation {
    pub id: String,
    pub bookmark_id: String,
    pub added_at: String,
    pub added_by: Option<String>,
    pub notes: Option<String>,
    pub context: Option<String>,
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tag {
    pub bookmark_id: String,
    pub tag: String,
    pub added_at: String,
    pub added_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectionTag {
    pub collection_id: String,
    pub tag: String,
    pub added_at: String,
    pub added_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectionLink {
    pub id: String,
    pub collection_id: String,
    pub kind: CollectionLinkKind,
    pub label: String,
    pub url: String,
    pub sort_order: i32,
    pub added_at: String,
    pub added_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bookmark {
    pub id: String,
    pub query: String,
    pub language: String,
    pub file_path: String,
    pub content_hash: Option<String>,
    pub commit_hash: Option<String>,
    pub health: BookmarkHealth,
    pub resolution_method: Option<ResolutionMethod>,
    pub last_resolved_at: Option<String>,
    pub stale_since: Option<String>,
    pub created_at: String,
    pub created_by: Option<String>,
    pub current_resolution_id: Option<String>,
    pub repo_id: Option<String>,
    // Computed UI status (not stored in database)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui_status: Option<String>,
    // Aggregated from related tables
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub annotations: Vec<Annotation>,
    #[serde(default)]
    pub comments: Vec<BookmarkComment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolution {
    pub id: String,
    pub bookmark_id: String,
    pub resolved_at: String,
    pub health: BookmarkHealth,
    pub commit_hash: Option<String>,
    pub method: ResolutionMethod,
    pub match_count: Option<i32>,
    pub file_path: Option<String>,
    pub byte_range: Option<String>,
    pub line_range: Option<String>,
    pub content_hash: Option<String>,
    pub headline: Option<String>,
    pub snapshot: Option<String>,
    pub breadcrumbs: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Collection {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub visibility: Visibility,
    pub created_at: String,
    pub created_by: Option<String>,
    pub created_branch: Option<String>,
    pub published_at: Option<String>,
    pub published_commit_sha: Option<String>,
    pub repo_url: Option<String>,
    pub repo_id: Option<String>,
    pub status: Option<String>,
    pub health: Option<CollectionHealth>,
    pub health_computed_at: Option<String>,
    pub updated_at: Option<String>,
    pub imported_from_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repo {
    pub id: String,
    pub repo_owner: String,
    pub repo_name: String,
    pub origin_url: Option<String>,
    pub repo_root: String,
    pub db_owner_email: String,
    pub db_owner_name: Option<String>,
    pub detected_at: String,
}

// --- Filter ---

#[derive(Debug, Default)]
pub struct BookmarkFilter {
    pub tag: Option<String>,
    pub health: Option<Vec<BookmarkHealth>>,
    pub file_path: Option<String>,
    pub language: Option<String>,
    pub created_by: Option<String>,
    pub collection: Option<String>,
    pub collection_id: Option<String>,
    pub limit: Option<usize>,
}

impl BookmarkFilter {
    pub fn health(mut self, health: Vec<BookmarkHealth>) -> Self {
        self.health = Some(health);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bookmark_health_round_trip() {
        for health in [
            BookmarkHealth::Active,
            BookmarkHealth::Drifted,
            BookmarkHealth::Stale,
            BookmarkHealth::Archived,
        ] {
            let s = health.to_string();
            let parsed: BookmarkHealth = s.parse().unwrap();
            assert_eq!(health, parsed);
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
    fn invalid_health_returns_error() {
        assert!("bogus".parse::<BookmarkHealth>().is_err());
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
            health: BookmarkHealth::Active,
            resolution_method: Some(ResolutionMethod::Exact),
            last_resolved_at: None,
            stale_since: None,
            created_at: "2026-04-01T00:00:00Z".into(),
            created_by: None,
            current_resolution_id: None,
            repo_id: None,
            ui_status: None,
            tags: vec!["auth".into()],
            annotations: vec![],
            comments: vec![],
        };
        let json = serde_json::to_string(&bookmark).unwrap();
        let parsed: Bookmark = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "test-id");
        assert_eq!(parsed.health, BookmarkHealth::Active);
    }
}
