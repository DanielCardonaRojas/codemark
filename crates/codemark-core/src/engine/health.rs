use crate::engine::bookmark::{BookmarkStatus, ResolutionMethod};

/// Determine the new bookmark status based on resolution outcome and whether
/// the content hash matches.
pub fn transition(
    _current: BookmarkStatus,
    method: ResolutionMethod,
    hash_matches: bool,
) -> BookmarkStatus {
    match method {
        ResolutionMethod::Exact if hash_matches => BookmarkStatus::Active,
        ResolutionMethod::Exact => BookmarkStatus::Drifted,
        ResolutionMethod::Relaxed => BookmarkStatus::Drifted,
        ResolutionMethod::HashFallback => BookmarkStatus::Drifted,
        ResolutionMethod::Failed => BookmarkStatus::Stale,
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
            transition(BookmarkStatus::Drifted, ResolutionMethod::Exact, true),
            BookmarkStatus::Active
        );
        assert_eq!(
            transition(BookmarkStatus::Stale, ResolutionMethod::Exact, true),
            BookmarkStatus::Active
        );
    }

    #[test]
    fn exact_without_hash_match_returns_drifted() {
        assert_eq!(
            transition(BookmarkStatus::Active, ResolutionMethod::Exact, false),
            BookmarkStatus::Drifted
        );
    }

    #[test]
    fn relaxed_returns_drifted() {
        assert_eq!(
            transition(BookmarkStatus::Active, ResolutionMethod::Relaxed, true),
            BookmarkStatus::Drifted
        );
    }

    #[test]
    fn hash_fallback_returns_drifted() {
        assert_eq!(
            transition(BookmarkStatus::Active, ResolutionMethod::HashFallback, false),
            BookmarkStatus::Drifted
        );
    }

    #[test]
    fn failed_returns_stale() {
        assert_eq!(
            transition(BookmarkStatus::Active, ResolutionMethod::Failed, false),
            BookmarkStatus::Stale
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
