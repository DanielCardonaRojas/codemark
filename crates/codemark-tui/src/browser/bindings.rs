use crate::browser::right_pane::INFO_TAB_INDEX;
use crate::browser::{BrowserLayout, FocusArea, Panel3Tab};
use crate::ui::KeyBinding;

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
        let mut bindings = vec![
            ("q", "Quit"),
            ("?", "Toggle help"),
            ("Tab", "Cycle focus"),
            ("Esc", "Back/Cancel"),
            ("/", "Search"),
            ("[", "Previous tab"),
            ("]", "Next tab"),
        ];

        match self.focus {
            FocusArea::Search => {
                bindings.push(("Enter", "Search"));
            }
            FocusArea::Panel1 => {
                bindings.push(("Enter", "Select repo"));
                bindings.push(("+", "Increase pane"));
                bindings.push(("_", "Decrease pane"));
            }
            FocusArea::Panel2 => {
                bindings.push(("Enter", "Toggle filter"));
                bindings.push(("Space", "Toggle filter"));
                bindings.push(("+", "Increase pane"));
                bindings.push(("_", "Decrease pane"));
            }
            FocusArea::Panel3 => {
                bindings.push(("+", "Increase pane"));
                bindings.push(("_", "Decrease pane"));
                match Panel3Tab::from_index(self.left_pane.panel3.tabs.selected_index()) {
                    Some(Panel3Tab::Tours) => {
                        bindings.push(("Enter", "Open tour"));
                        bindings.push(("p", "Pull tour"));
                        bindings.push(("P", "Push tour"));
                        bindings.push(("H", "Heal all"));
                    }
                    Some(Panel3Tab::Collections) => {
                        bindings.push(("Enter", "Open collection"));
                        bindings.push(("d", "Delete collection"));
                        bindings.push(("H", "Heal all"));
                        bindings.push(("Ctrl+O", "Copy ID"));
                    }
                    Some(Panel3Tab::Bookmarks) => {
                        bindings.push(("Enter", "Preview bookmark"));
                        bindings.push(("o", "Open in editor"));
                        bindings.push(("d", "Delete bookmark"));
                        bindings.push(("H", "Heal"));
                        bindings.push(("Ctrl+O", "Copy ID"));
                    }
                    None => {}
                }
            }
            FocusArea::Main => {
                bindings.push(("Enter", "Go to step"));
                bindings.push(("o", "Open in editor"));
                bindings.push(("←/h", "Previous step"));
                bindings.push(("→/l", "Next step"));
                bindings.push(("H", "Heal"));
                bindings.push(("↑", "Focus steps"));
                bindings.push(("↓", "Focus details"));
                bindings.push(("+/-", "Toggle fullscreen"));
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

        bindings
    }

    /// Get context-aware bindings as KeyBinding structs.
    fn get_context_bindings(&self) -> Vec<KeyBinding> {
        let mut bindings = vec![
            KeyBinding::new("/", "Filter"),
            KeyBinding::new("?", "Help"),
            KeyBinding::new("q", "Quit"),
        ];

        match self.focus {
            FocusArea::Search => {
                bindings.insert(0, KeyBinding::new("Enter", "Search"));
            }
            FocusArea::Panel1 => {
                // Repos / Accounts
                bindings.insert(0, KeyBinding::new("Enter", "Select"));
                bindings.insert(1, KeyBinding::new("+/-", "Resize"));
            }
            FocusArea::Panel2 => {
                // Tags / Branches
                bindings.insert(0, KeyBinding::new("Enter", "Filter"));
                bindings.insert(1, KeyBinding::new("+/-", "Resize"));
            }
            FocusArea::Panel3 => {
                // Tours / Collections / Bookmarks
                match Panel3Tab::from_index(self.left_pane.panel3.tabs.selected_index()) {
                    Some(Panel3Tab::Tours) => {
                        bindings.insert(0, KeyBinding::new("p", "Pull"));
                        bindings.insert(1, KeyBinding::new("P", "Push"));
                        bindings.insert(2, KeyBinding::new("H", "Heal all"));
                        bindings.insert(3, KeyBinding::new("+/-", "Resize"));
                    }
                    Some(Panel3Tab::Collections) => {
                        bindings.insert(0, KeyBinding::new("Enter", "Open"));
                        bindings.insert(1, KeyBinding::new("d", "Delete"));
                        bindings.insert(2, KeyBinding::new("H", "Heal all"));
                        bindings.insert(3, KeyBinding::new("Ctrl+O", "Copy ID"));
                        bindings.insert(4, KeyBinding::new("+/-", "Resize"));
                    }
                    Some(Panel3Tab::Bookmarks) => {
                        bindings.insert(0, KeyBinding::new("o", "Open"));
                        bindings.insert(1, KeyBinding::new("Enter", "Preview"));
                        bindings.insert(2, KeyBinding::new("d", "Delete"));
                        bindings.insert(3, KeyBinding::new("H", "Heal"));
                        bindings.insert(4, KeyBinding::new("Ctrl+O", "Copy ID"));
                        bindings.insert(5, KeyBinding::new("+/-", "Resize"));
                    }
                    None => {
                        bindings.insert(0, KeyBinding::new("+/-", "Resize"));
                    }
                }
            }
            FocusArea::Main => {
                bindings.insert(0, KeyBinding::new("+/-", "Toggle fullscreen"));
                bindings.insert(1, KeyBinding::new("Enter", "Select Step"));
                bindings.insert(2, KeyBinding::new("o", "Open File"));
                bindings.insert(3, KeyBinding::new("H", "Heal"));
                let insert_pos = if self.right_pane.steps.tabs.selected_index() == INFO_TAB_INDEX {
                    bindings.insert(4, KeyBinding::new("Ctrl+O", "Copy markdown"));
                    5
                } else {
                    4
                };
                bindings.insert(insert_pos, KeyBinding::new("Esc", "Back to Tours"));
            }
            FocusArea::Filter => {}
        }

        bindings
    }
}
