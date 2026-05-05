use crate::engine::bookmark::{BookmarkHealth, ResolutionMethod};

/// Determine the new bookmark health based on resolution outcome and whether
/// the content hash matches.
pub fn transition(
    _current: BookmarkHealth,
    method: ResolutionMethod,
    hash_matches: bool,
) -> BookmarkHealth {
    match method {
        ResolutionMethod::Exact if hash_matches => BookmarkHealth::Active,
        ResolutionMethod::Exact => BookmarkHealth::Drifted,
        ResolutionMethod::Relaxed => BookmarkHealth::Drifted,
        ResolutionMethod::HashFallback => BookmarkHealth::Drifted,
        ResolutionMethod::Failed => BookmarkHealth::Stale,
    }
}

/// Determine if a stale bookmark should be auto-archived.
pub fn should_auto_archive(stale_since: &str, archive_after_days: u32) -> bool {
    let Ok(stale_dt) = chrono::DateTime::parse_from_rfc3339(stale_since) else {
        return false;
    };
    let now = chrono::Utc::now();
    let days_stale = (now - stale_dt.with_timezone(&chrono::Utc)).num_days();
    days_stale >= archive_after_days as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_with_hash_match_returns_active() {
        assert_eq!(
            transition(BookmarkHealth::Drifted, ResolutionMethod::Exact, true),
            BookmarkHealth::Active
        );
        assert_eq!(
            transition(BookmarkHealth::Stale, ResolutionMethod::Exact, true),
            BookmarkHealth::Active
        );
    }

    #[test]
    fn exact_without_hash_match_returns_drifted() {
        assert_eq!(
            transition(BookmarkHealth::Active, ResolutionMethod::Exact, false),
            BookmarkHealth::Drifted
        );
    }

    #[test]
    fn relaxed_returns_drifted() {
        assert_eq!(
            transition(BookmarkHealth::Active, ResolutionMethod::Relaxed, true),
            BookmarkHealth::Drifted
        );
    }

    #[test]
    fn hash_fallback_returns_drifted() {
        assert_eq!(
            transition(BookmarkHealth::Active, ResolutionMethod::HashFallback, false),
            BookmarkHealth::Drifted
        );
    }

    #[test]
    fn failed_returns_stale() {
        assert_eq!(
            transition(BookmarkHealth::Active, ResolutionMethod::Failed, false),
            BookmarkHealth::Stale
        );
    }

    #[test]
    fn auto_archive_after_threshold() {
        let old_date = "2026-03-01T00:00:00Z";
        assert!(should_auto_archive(old_date, 7));
    }

    #[test]
    fn no_auto_archive_before_threshold() {
        let recent = chrono::Utc::now().to_rfc3339();
        assert!(!should_auto_archive(&recent, 7));
    }
}
