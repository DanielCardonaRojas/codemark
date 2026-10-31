//! Health status indicator shown next to a panel item.

use ratatui::style::Color;

/// Health status indicator for an item based on the projected UI status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    /// 🟢 Healthy
    Healthy,
    /// 🟡 Unanchored (Healthy)
    UnanchoredHealthy,
    /// 🟡 Drifted
    Drifted,
    /// 🟠 Unanchored (Drifting)
    UnanchoredDrifting,
    /// 🔴 Broken
    Broken,
    /// 🔴 Broken (Unanchored)
    BrokenUnanchored,
    /// ⚪ Verified (Historical)
    Verified,
    /// ⚪ Outdated (Historical)
    Outdated,
    /// 🔵 Future
    Future,
    /// ⚪ No grammar installed for this bookmark's language.
    ///
    /// Not a health state of the bookmark at all: the query was never run,
    /// because nothing can parse the file. Shown apart from `Broken` because
    /// the two call for opposite responses — a broken bookmark may need
    /// re-anchoring, while this one needs `codemark languages add` and is then
    /// fine. Conflating them makes a healthy collection look rotten to anyone
    /// who has not installed the language, which is the normal case for a
    /// shared tour.
    GrammarMissing,
    /// Unknown/Gray - gray
    Unknown,
}

impl From<codemark_core::engine::resolution::LiveUIStatus> for HealthStatus {
    fn from(status: codemark_core::engine::resolution::LiveUIStatus) -> Self {
        use codemark_core::engine::resolution::LiveUIStatus;
        match status {
            LiveUIStatus::Healthy => HealthStatus::Healthy,
            LiveUIStatus::Drifted => HealthStatus::Drifted,
            LiveUIStatus::Broken => HealthStatus::Broken,
        }
    }
}

impl From<codemark_core::engine::projection::UIStatus> for HealthStatus {
    fn from(status: codemark_core::engine::projection::UIStatus) -> Self {
        use codemark_core::engine::projection::UIStatus;
        match status {
            UIStatus::Healthy => HealthStatus::Healthy,
            UIStatus::UnanchoredHealthy => HealthStatus::UnanchoredHealthy,
            UIStatus::Drifted => HealthStatus::Drifted,
            UIStatus::UnanchoredDrifting => HealthStatus::UnanchoredDrifting,
            UIStatus::Broken => HealthStatus::Broken,
            UIStatus::BrokenUnanchored => HealthStatus::BrokenUnanchored,
            UIStatus::Verified => HealthStatus::Verified,
            UIStatus::Outdated => HealthStatus::Outdated,
            UIStatus::Future => HealthStatus::Future,
        }
    }
}

impl HealthStatus {
    /// Get the color for this health status.
    pub(crate) fn color(&self) -> Color {
        match self {
            HealthStatus::Healthy => crate::theme::palette().health_good,
            HealthStatus::UnanchoredHealthy => crate::theme::palette().health_warn,
            HealthStatus::Drifted => crate::theme::palette().health_warn,
            HealthStatus::UnanchoredDrifting => Color::Rgb(255, 165, 0), // Orange
            HealthStatus::Broken | HealthStatus::BrokenUnanchored => {
                crate::theme::palette().health_bad
            }
            HealthStatus::Verified => crate::theme::palette().health_good,
            HealthStatus::Outdated => crate::theme::palette().health_warn,
            HealthStatus::Unknown => crate::theme::palette().dim,
            // Dim rather than red: nothing is wrong with the bookmark, the
            // reader simply cannot see it yet.
            HealthStatus::GrammarMissing => crate::theme::palette().dim,
            HealthStatus::Future => crate::theme::palette().info,
        }
    }

    /// Get the symbol for this health status.
    pub(super) fn symbol(&self) -> &'static str {
        match self {
            HealthStatus::Verified | HealthStatus::Outdated => "○", // Unfilled circle for historical statuses
            _ => "●", // Filled dot for all other statuses
        }
    }

    /// Short label for this status, reflecting the live-preview model: a
    /// bookmark's query is applied live to the files on disk, so there are
    /// only four outcomes — an exact match, a drifted (partial) match, no
    /// match at all, or an error (file missing, parse failure, …). Drawn as
    /// knockout text on the preview-pane border.
    pub(crate) fn label(&self) -> &'static str {
        match self {
            // Exact match — the query resolved perfectly (possibly uncommitted
            // or from a historical/future commit, but the code is there).
            HealthStatus::Healthy
            | HealthStatus::UnanchoredHealthy
            | HealthStatus::Verified
            | HealthStatus::Future => "Exact",
            // Drifted match — the query resolved, but the code has changed.
            HealthStatus::Drifted | HealthStatus::UnanchoredDrifting | HealthStatus::Outdated => {
                "Drifted"
            }
            // Unmatched — the query does not resolve at all.
            HealthStatus::Broken | HealthStatus::BrokenUnanchored => "Unmatched",
            // The language's grammar is not installed, so the query never ran.
            HealthStatus::GrammarMissing => "No grammar",
            // Error — file missing, parse failure, or not yet resolved.
            HealthStatus::Unknown => "Error",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ui_status_to_health_status_exhaustive() {
        use codemark_core::engine::projection::UIStatus;

        assert_eq!(HealthStatus::from(UIStatus::Healthy), HealthStatus::Healthy);
        assert_eq!(
            HealthStatus::from(UIStatus::UnanchoredHealthy),
            HealthStatus::UnanchoredHealthy
        );
        assert_eq!(HealthStatus::from(UIStatus::Drifted), HealthStatus::Drifted);
        assert_eq!(
            HealthStatus::from(UIStatus::UnanchoredDrifting),
            HealthStatus::UnanchoredDrifting
        );
        assert_eq!(HealthStatus::from(UIStatus::Broken), HealthStatus::Broken);
        assert_eq!(HealthStatus::from(UIStatus::BrokenUnanchored), HealthStatus::BrokenUnanchored);
        assert_eq!(HealthStatus::from(UIStatus::Verified), HealthStatus::Verified);
        assert_eq!(HealthStatus::from(UIStatus::Outdated), HealthStatus::Outdated);
        assert_eq!(HealthStatus::from(UIStatus::Future), HealthStatus::Future);
    }

    /// A missing grammar must not read as a broken bookmark.
    ///
    /// Receiving a shared tour for a language you have not installed is the
    /// normal case, not an edge case — so showing "Unmatched" would tell every
    /// such reader their bookmarks had rotted. See codemark#257.
    #[test]
    fn a_missing_grammar_is_labelled_apart_from_unmatched() {
        assert_eq!(HealthStatus::GrammarMissing.label(), "No grammar");
        assert_ne!(HealthStatus::GrammarMissing.label(), HealthStatus::Broken.label());
        assert_ne!(HealthStatus::GrammarMissing.label(), HealthStatus::Unknown.label());

        // Dim, not red: nothing is wrong with the bookmark.
        assert_eq!(HealthStatus::GrammarMissing.color(), HealthStatus::Unknown.color());
        assert_ne!(HealthStatus::GrammarMissing.color(), HealthStatus::Broken.color());
    }

    #[test]
    fn test_health_status_colors() {
        use ratatui::style::Color;

        assert_eq!(HealthStatus::Healthy.color(), Color::Green);
        assert_eq!(HealthStatus::UnanchoredHealthy.color(), Color::Yellow);
        assert_eq!(HealthStatus::Drifted.color(), Color::Yellow);
        assert_eq!(HealthStatus::UnanchoredDrifting.color(), Color::Rgb(255, 165, 0));
        assert_eq!(HealthStatus::Broken.color(), Color::Red);
        assert_eq!(HealthStatus::BrokenUnanchored.color(), Color::Red);
        assert_eq!(HealthStatus::Verified.color(), Color::Green);
        assert_eq!(HealthStatus::Outdated.color(), Color::Yellow);
        assert_eq!(HealthStatus::Unknown.color(), Color::DarkGray);
        assert_eq!(HealthStatus::Future.color(), Color::Blue);
    }

    #[test]
    fn test_health_status_symbols() {
        // Verified and Outdated statuses use an unfilled circle (historical)
        assert_eq!(HealthStatus::Verified.symbol(), "○");
        assert_eq!(HealthStatus::Outdated.symbol(), "○");
        // All other statuses use a filled dot
        assert_eq!(HealthStatus::Healthy.symbol(), "●");
        assert_eq!(HealthStatus::UnanchoredHealthy.symbol(), "●");
        assert_eq!(HealthStatus::Drifted.symbol(), "●");
        assert_eq!(HealthStatus::Broken.symbol(), "●");
    }

    #[test]
    fn test_health_status_labels() {
        // Live-preview model collapses the granular UI statuses into four
        // outcomes the user can actually distinguish.
        assert_eq!(HealthStatus::Healthy.label(), "Exact");
        assert_eq!(HealthStatus::UnanchoredHealthy.label(), "Exact");
        assert_eq!(HealthStatus::Verified.label(), "Exact");
        assert_eq!(HealthStatus::Future.label(), "Exact");
        assert_eq!(HealthStatus::Drifted.label(), "Drifted");
        assert_eq!(HealthStatus::UnanchoredDrifting.label(), "Drifted");
        assert_eq!(HealthStatus::Outdated.label(), "Drifted");
        assert_eq!(HealthStatus::Broken.label(), "Unmatched");
        assert_eq!(HealthStatus::BrokenUnanchored.label(), "Unmatched");
        assert_eq!(HealthStatus::Unknown.label(), "Error");
    }
}
