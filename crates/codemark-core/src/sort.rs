//! Presentation-agnostic ordering for domain lists (bookmarks, collections, …).
//!
//! The [`SortMethod`] enum and the [`sort_by`] helper are shared across the TUI
//! and CLI so the two surfaces order lists identically. Types opt in by
//! implementing [`Sortable`], which exposes just the two keys ordering needs: a
//! display name (for alphabetical order) and a creation timestamp (for date
//! order). How a method is *presented* (a glyph, a flag name, …) is left to each
//! surface.

use crate::engine::bookmark::{Bookmark, Collection};
use std::cmp::Reverse;

/// How a list of [`Sortable`] items is ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortMethod {
    /// A → Z by display name (case-insensitive).
    AlphabeticalAsc,
    /// Z → A by display name (case-insensitive).
    AlphabeticalDesc,
    /// Most recently created first.
    DateNewest,
    /// Oldest created first.
    DateOldest,
}

impl SortMethod {
    /// The next method in the cycle (wraps back to the start). Drives the `S`
    /// shortcut / sort-glyph click in the TUI.
    pub fn next(self) -> Self {
        match self {
            SortMethod::AlphabeticalAsc => SortMethod::AlphabeticalDesc,
            SortMethod::AlphabeticalDesc => SortMethod::DateNewest,
            SortMethod::DateNewest => SortMethod::DateOldest,
            SortMethod::DateOldest => SortMethod::AlphabeticalAsc,
        }
    }

    /// Every method, in cycle order.
    pub fn all() -> [SortMethod; 4] {
        [
            SortMethod::AlphabeticalAsc,
            SortMethod::AlphabeticalDesc,
            SortMethod::DateNewest,
            SortMethod::DateOldest,
        ]
    }

    /// Human-readable label (help text, CLI flag descriptions, tooltips).
    pub fn label(self) -> &'static str {
        match self {
            SortMethod::AlphabeticalAsc => "Name A→Z",
            SortMethod::AlphabeticalDesc => "Name Z→A",
            SortMethod::DateNewest => "Newest",
            SortMethod::DateOldest => "Oldest",
        }
    }
}

/// A value that can be ordered by [`sort_by`].
pub trait Sortable {
    /// The display name used for alphabetical ordering. Compared
    /// case-insensitively, so implementations return it verbatim.
    fn sort_name(&self) -> &str;

    /// The ISO-8601 creation timestamp used for date ordering (such strings
    /// compare chronologically as plain text). Items returning `None` sort to
    /// the end regardless of direction.
    fn sort_timestamp(&self) -> Option<&str>;
}

/// Order `items` in place by `method`. Stable: items with equal keys keep their
/// relative (insertion) order, and undated items always trail the dated ones.
pub fn sort_by<T: Sortable>(items: &mut [T], method: SortMethod) {
    match method {
        SortMethod::AlphabeticalAsc => {
            items.sort_by_cached_key(|i| i.sort_name().to_lowercase());
        }
        SortMethod::AlphabeticalDesc => {
            items.sort_by_cached_key(|i| Reverse(i.sort_name().to_lowercase()));
        }
        // The leading `is_none` flag pins undated items last in both directions;
        // among dated items the timestamp orders them (reversed for "newest").
        SortMethod::DateNewest => {
            items.sort_by_cached_key(|i| {
                (i.sort_timestamp().is_none(), Reverse(i.sort_timestamp().map(str::to_owned)))
            });
        }
        SortMethod::DateOldest => {
            items.sort_by_cached_key(|i| {
                (i.sort_timestamp().is_none(), i.sort_timestamp().map(str::to_owned))
            });
        }
    }
}

impl Sortable for Bookmark {
    /// Bookmarks order by file path. (The TUI sorts by the symbol identifier via
    /// its own `Sortable` impl on the display item, since that requires a
    /// tree-sitter summary the domain struct doesn't carry.)
    fn sort_name(&self) -> &str {
        &self.file_path
    }

    fn sort_timestamp(&self) -> Option<&str> {
        Some(&self.created_at)
    }
}

impl Sortable for Collection {
    fn sort_name(&self) -> &str {
        &self.name
    }

    fn sort_timestamp(&self) -> Option<&str> {
        Some(&self.created_at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal fixture so the ordering tests don't depend on the full domain
    /// structs. `timestamp: None` exercises the undated-last behavior.
    struct Item {
        name: &'static str,
        timestamp: Option<&'static str>,
    }

    impl Sortable for Item {
        fn sort_name(&self) -> &str {
            self.name
        }
        fn sort_timestamp(&self) -> Option<&str> {
            self.timestamp
        }
    }

    fn fixture() -> Vec<Item> {
        vec![
            Item { name: "Banana", timestamp: Some("2024-01-01T00:00:00Z") },
            Item { name: "apple", timestamp: Some("2026-01-01T00:00:00Z") },
            Item { name: "cherry", timestamp: None },
            Item { name: "Avocado", timestamp: Some("2025-01-01T00:00:00Z") },
        ]
    }

    fn names(items: &[Item]) -> Vec<&str> {
        items.iter().map(|i| i.name).collect()
    }

    #[test]
    fn cycles_through_all_four_in_order() {
        assert_eq!(SortMethod::AlphabeticalAsc.next(), SortMethod::AlphabeticalDesc);
        assert_eq!(SortMethod::AlphabeticalDesc.next(), SortMethod::DateNewest);
        assert_eq!(SortMethod::DateNewest.next(), SortMethod::DateOldest);
        assert_eq!(SortMethod::DateOldest.next(), SortMethod::AlphabeticalAsc);
        assert_eq!(SortMethod::all().len(), 4);
    }

    #[test]
    fn alphabetical_is_case_insensitive() {
        let mut items = fixture();
        sort_by(&mut items, SortMethod::AlphabeticalAsc);
        assert_eq!(names(&items), vec!["apple", "Avocado", "Banana", "cherry"]);

        sort_by(&mut items, SortMethod::AlphabeticalDesc);
        assert_eq!(names(&items), vec!["cherry", "Banana", "Avocado", "apple"]);
    }

    #[test]
    fn date_orders_by_timestamp_with_undated_last() {
        let mut items = fixture();
        sort_by(&mut items, SortMethod::DateNewest);
        // 2026 (apple), 2025 (Avocado), 2024 (Banana), then the undated cherry.
        assert_eq!(names(&items), vec!["apple", "Avocado", "Banana", "cherry"]);

        sort_by(&mut items, SortMethod::DateOldest);
        assert_eq!(names(&items), vec!["Banana", "Avocado", "apple", "cherry"]);
    }

    #[test]
    fn sort_is_stable_for_equal_keys() {
        // Equal names keep insertion order (distinguished here by timestamp).
        let mut items = vec![
            Item { name: "dup", timestamp: Some("a") },
            Item { name: "dup", timestamp: Some("b") },
            Item { name: "dup", timestamp: Some("c") },
        ];
        sort_by(&mut items, SortMethod::AlphabeticalAsc);
        assert_eq!(
            items.iter().map(|i| i.timestamp.unwrap()).collect::<Vec<_>>(),
            vec!["a", "b", "c"]
        );
    }
}
