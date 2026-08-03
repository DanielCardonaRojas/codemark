use crate::browser::right_pane::INFO_TAB_INDEX;
use crate::browser::{BrowserLayout, ContentTab, FocusArea};
use crate::ui::{HIDDEN_BINDING_PRIORITY, KeyBinding};

impl BrowserLayout {
    /// Get context-aware keybindings for the status bar.
    pub fn get_status_bindings(&self) -> Vec<KeyBinding> {
        self.get_context_bindings()
    }

    /// Get context-aware keybindings for help display.
    ///
    /// Returns a list of (key, description) tuples suitable for display in the help panel.
    /// The bindings are contextual based on the current focus area and active tab.
    pub fn get_help_bindings(&self) -> Vec<(&'static str, &'static str)> {
        // A modal dialog only accepts button navigation/activation.
        if self.has_active_dialog() {
            return vec![("←/→", "Select button"), ("Enter", "Activate"), ("Esc", "Cancel")];
        }

        let mut bindings = vec![
            ("q", "Quit"),
            ("?", "Keybindings"),
            (",", "Settings"),
            ("Tab", "Cycle focus"),
            ("Esc", "Back/Cancel"),
            ("s", "Focus search"),
            ("/", "Filter"),
            ("[", "Previous tab"),
            ("]", "Next tab"),
        ];

        match self.focus {
            FocusArea::Search => {
                bindings.push(("Enter", "Search"));
                bindings.push(("Esc", "Clear"));
                // The FTS/semantic toggle only exists in semantic builds.
                #[cfg(feature = "semantic")]
                bindings.push(("Ctrl+S", "FTS / Sem"));
            }
            FocusArea::ContextPanel => {
                bindings.push(("Enter", "Select repo"));
                bindings.push(("+", "Increase pane"));
                bindings.push(("_", "Decrease pane"));
            }
            FocusArea::FiltersPanel => {
                bindings.push(("Enter", "Toggle filter"));
                bindings.push(("Space", "Toggle filter"));
                bindings.push(("+", "Increase pane"));
                bindings.push(("_", "Decrease pane"));
            }
            FocusArea::ContentPanel => {
                bindings.push(("+", "Increase pane"));
                bindings.push(("_", "Decrease pane"));
                match ContentTab::from_index(self.left_pane.content_panel.tabs.selected_index()) {
                    Some(ContentTab::Tours) => {
                        bindings.push(("Enter", "Open tour"));
                        bindings.push(("p", "Pull tour"));
                        bindings.push(("P", "Push tour"));
                        bindings.push(("H", "Heal all"));
                    }
                    Some(ContentTab::Collections) => {
                        bindings.push(("Enter", "Open collection"));
                        // Pushing requires sync credentials, so only advertise it
                        // when authenticated.
                        if self.logged_in {
                            bindings.push(("P", "Push collection"));
                        }
                        bindings.push(("d", "Delete collection"));
                        bindings.push(("H", "Heal all"));
                        bindings.push(("S", "Sort"));
                        bindings.push(("Ctrl+O", "Copy ID"));
                    }
                    Some(ContentTab::Bookmarks) => {
                        bindings.push(("Enter", "Preview bookmark"));
                        bindings.push(("o", "Open in editor"));
                        bindings.push(("d", "Delete bookmark"));
                        bindings.push(("H", "Heal"));
                        bindings.push(("S", "Sort"));
                        bindings.push(("Ctrl+O", "Copy ID"));
                    }
                    None => {}
                }
            }
            FocusArea::Main => {
                // On a collection/tour overview Enter enters the bookmarks flow;
                // otherwise it goes to the step or opens a focused link.
                if self.right_pane.overview_active {
                    bindings.push(("Enter", "Open collection / Open link"));
                } else {
                    bindings.push(("Enter", "Go to step / Open link"));
                }
                bindings.push(("o", "Open in editor"));
                // Links only exist in markdown panels, not the code preview.
                if self.right_pane.link_navigation_available() {
                    bindings.push(("n/N", "Cycle links"));
                }
                bindings.push(("←/h", "Previous step"));
                bindings.push(("→/l", "Next step"));
                bindings.push(("H", "Heal"));
                bindings.push(("↑", "Focus steps"));
                bindings.push(("↓", "Focus details"));
                bindings.push(("+/-", "Cycle layout"));
                // Only show Ctrl+O binding when Info tab is selected
                if self.right_pane.steps.tabs.selected_index() == INFO_TAB_INDEX {
                    bindings.push(("Ctrl+O", "Copy markdown"));
                }
            }
            FocusArea::Filter => {}
        }

        // Navigation keys (always available)
        bindings.push(("j/↓", "Move down"));
        bindings.push(("k/↑", "Move up"));
        // J/K scroll the preview from any pane, but in Search focus those keys
        // type into the query box instead (the handler guards on the same
        // condition), so only advertise them when they're actually live.
        if self.focus != FocusArea::Search {
            bindings.push(("J/K", "Scroll preview"));
        }

        bindings
    }

    /// Get context-aware bindings as KeyBinding structs.
    ///
    /// Bindings carry a relevance priority so the status bar can drop the least
    /// relevant ones when space is tight. The `?` Help binding is pinned so it
    /// is always shown — it's how users reach the exhaustive help popup.
    ///
    /// Priority bands (higher = more relevant, shown first):
    /// - 255 (`pinned`): always shown, never dropped (e.g. `?` Help)
    /// - 90..255: primary context action (the thing you most likely want here)
    /// - 70..90: secondary context actions
    /// - 50..70: common global actions (Filter, Quit)
    /// - 1..50: low-value extras (Resize, Copy ID)
    /// - 0: hidden from the bar entirely (e.g. Enter, which is universal)
    fn get_context_bindings(&self) -> Vec<KeyBinding> {
        // A modal dialog only accepts button navigation/activation.
        if self.has_active_dialog() {
            return vec![
                KeyBinding::new("←/→", "Select"),
                KeyBinding::new("Enter", "Activate"),
                KeyBinding::new("Esc", "Cancel"),
            ];
        }

        // Context-specific bindings first (most relevant), then globals.
        let mut bindings: Vec<KeyBinding> = Vec::new();

        match self.focus {
            FocusArea::Search => {
                // Enter is universal; hidden from the bar but kept for the help popup.
                bindings.push(
                    KeyBinding::new("Enter", "Search").with_priority(HIDDEN_BINDING_PRIORITY),
                );
                // Esc clears the query and restores the full list, but there is
                // no on-screen affordance for it, so advertise it as the primary
                // context action while the search bar holds focus.
                bindings.push(KeyBinding::new("Esc", "Clear").with_priority(95));
                // The FTS/semantic toggle only exists in semantic builds.
                #[cfg(feature = "semantic")]
                bindings.push(KeyBinding::new("Ctrl+S", "FTS / Sem").with_priority(90));
            }
            FocusArea::ContextPanel => {
                // Repos / Accounts
                bindings.push(
                    KeyBinding::new("Enter", "Select").with_priority(HIDDEN_BINDING_PRIORITY),
                );
                bindings.push(KeyBinding::new("+/-", "Resize").with_priority(20));
            }
            FocusArea::FiltersPanel => {
                // Tags / Branches
                bindings.push(
                    KeyBinding::new("Enter", "Filter").with_priority(HIDDEN_BINDING_PRIORITY),
                );
                bindings.push(KeyBinding::new("+/-", "Resize").with_priority(20));
            }
            FocusArea::ContentPanel => {
                // Tours / Collections / Bookmarks
                match ContentTab::from_index(self.left_pane.content_panel.tabs.selected_index()) {
                    Some(ContentTab::Tours) => {
                        bindings.push(KeyBinding::new("p", "Pull").with_priority(90));
                        bindings.push(KeyBinding::new("P", "Push").with_priority(85));
                        bindings.push(
                            KeyBinding::new("H", "Heal all").with_priority(HIDDEN_BINDING_PRIORITY),
                        );
                        bindings.push(KeyBinding::new("+/-", "Resize").with_priority(20));
                    }
                    Some(ContentTab::Collections) => {
                        bindings.push(
                            KeyBinding::new("Enter", "Open").with_priority(HIDDEN_BINDING_PRIORITY),
                        );
                        // Pushing requires sync credentials, so only advertise it
                        // when authenticated.
                        if self.logged_in {
                            bindings.push(KeyBinding::new("P", "Push").with_priority(85));
                        }
                        bindings.push(KeyBinding::new("d", "Delete").with_priority(75));
                        bindings.push(
                            KeyBinding::new("H", "Heal all").with_priority(HIDDEN_BINDING_PRIORITY),
                        );
                        bindings.push(KeyBinding::new("S", "Sort").with_priority(35));
                        bindings.push(KeyBinding::new("Ctrl+O", "Copy ID").with_priority(30));
                        bindings.push(KeyBinding::new("+/-", "Resize").with_priority(20));
                    }
                    Some(ContentTab::Bookmarks) => {
                        bindings.push(
                            KeyBinding::new("Enter", "Preview")
                                .with_priority(HIDDEN_BINDING_PRIORITY),
                        );
                        bindings.push(KeyBinding::new("o", "Open").with_priority(85));
                        bindings.push(KeyBinding::new("d", "Delete").with_priority(75));
                        bindings.push(
                            KeyBinding::new("H", "Heal").with_priority(HIDDEN_BINDING_PRIORITY),
                        );
                        bindings.push(KeyBinding::new("S", "Sort").with_priority(35));
                        bindings.push(KeyBinding::new("Ctrl+O", "Copy ID").with_priority(30));
                        bindings.push(KeyBinding::new("+/-", "Resize").with_priority(20));
                    }
                    None => {
                        bindings.push(KeyBinding::new("+/-", "Resize").with_priority(20));
                    }
                }
            }
            FocusArea::Main => {
                bindings.push(
                    KeyBinding::new("Enter", "Select Step").with_priority(HIDDEN_BINDING_PRIORITY),
                );
                bindings.push(KeyBinding::new("o", "Open File").with_priority(85));
                // Links only exist in markdown panels, not the code preview.
                if self.right_pane.link_navigation_available() {
                    bindings.push(KeyBinding::new("n/N", "Links").with_priority(40));
                }
                bindings.push(KeyBinding::new("H", "Heal").with_priority(HIDDEN_BINDING_PRIORITY));
                bindings.push(KeyBinding::new("Esc", "Back to Tours").with_priority(72));
                if self.right_pane.steps.tabs.selected_index() == INFO_TAB_INDEX {
                    bindings.push(KeyBinding::new("Ctrl+O", "Copy markdown").with_priority(30));
                }
                bindings.push(KeyBinding::new("+/-", "Cycle layout").with_priority(25));
            }
            FocusArea::Filter => {}
        }

        // Global bindings, always appended. `?` Help and `,` Settings are
        // pinned so the two overlays stay discoverable even on a narrow bar.
        // `s` Search is omitted while the bar is focused, where the key types.
        if self.focus != FocusArea::Search {
            bindings.push(KeyBinding::new("s", "Search").with_priority(58));
        }
        bindings.push(KeyBinding::new("/", "Filter").with_priority(60));
        bindings.push(KeyBinding::new("q", "Quit").with_priority(55));
        bindings.push(KeyBinding::new("?", "Help").pinned());
        bindings.push(KeyBinding::new(",", "Settings").pinned());

        bindings
    }
}
