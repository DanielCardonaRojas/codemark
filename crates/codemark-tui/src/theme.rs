//! Hybrid theme loader for the TUI.
//!
//! Resolves syntect `.tmTheme` themes from the following sources, in priority
//! order (first match wins):
//!
//! 1. **User directory** — `~/.config/codemark/themes/<name>.tmTheme`. Lets users
//!    drop in their own themes or override a bundled one.
//! 2. **Embedded extras** — popular themes that `syntect-assets` does not ship
//!    (e.g. Catppuccin, Tokyo Night), bundled into the binary at compile time.
//! 3. **`syntect-assets` bundled themes** — Dracula, Nord, Gruvbox, Solarized,
//!    Monokai, OneHalf, … (vendored from the bat project).
//! 4. **Fallback** — [`FALLBACK_THEME`], always available via `syntect-assets`.
//!
//! This mirrors the bundle-plus-user-directory pattern already used for
//! templates in `codemark-core` (`templates.rs`).

use std::io::Cursor;
use std::path::PathBuf;

use codemark_core::config::global_config_dir;
use syntect::highlighting::{Theme, ThemeSet};
use syntect_assets::assets::HighlightingAssets;

/// Fallback theme name. Always present in the `syntect-assets` binary data, so
/// resolution can never fail to produce a usable theme.
pub const FALLBACK_THEME: &str = "OneHalfDark";

/// Popular `.tmTheme` files not shipped by `syntect-assets`, embedded at compile
/// time. Each entry is `(name, bytes)`, where `name` is the key users reference
/// in config and `bytes` is the raw `.tmTheme` plist.
///
/// This list is intentionally empty until the theme files are vendored (a later
/// step); the loader handles an empty set gracefully. To add one:
///
/// ```ignore
/// ("Catppuccin Mocha", include_bytes!("../themes/catppuccin-mocha.tmTheme")),
/// ```
const EMBEDDED_THEMES: &[(&str, &[u8])] = &[];

/// The directory user-supplied themes are read from:
/// `<global config dir>/themes` (e.g. `~/.config/codemark/themes`).
pub fn themes_dir() -> Option<PathBuf> {
    global_config_dir().map(|d| d.join("themes"))
}

/// Resolve [`FALLBACK_THEME`] directly from bundled assets, without building a
/// full [`ThemeRegistry`] or touching the user directory. Used as the default
/// theme before an app-level theme has been applied.
pub fn default_theme() -> Theme {
    HighlightingAssets::from_binary().get_theme(FALLBACK_THEME).clone()
}

/// Resolves `.tmTheme` themes across the user directory, embedded extras, and
/// `syntect-assets`, with a guaranteed fallback.
///
/// Construct once at startup (loading the user directory touches the filesystem)
/// and reuse for the lifetime of the app.
pub struct ThemeRegistry {
    /// Themes vendored in the `syntect-assets` binary data.
    assets: HighlightingAssets,
    /// Themes embedded into our own binary ([`EMBEDDED_THEMES`]).
    embedded: ThemeSet,
    /// Themes loaded from the user's [`themes_dir`].
    user: ThemeSet,
}

impl ThemeRegistry {
    /// Build a registry, loading user themes from [`themes_dir`].
    pub fn new() -> Self {
        Self::with_user_dir(themes_dir())
    }

    /// Build a registry, loading user themes from `dir` (if it exists). Exposed
    /// for testing with a controlled directory.
    fn with_user_dir(dir: Option<PathBuf>) -> Self {
        let user = dir
            .filter(|d| d.is_dir())
            .and_then(|d| ThemeSet::load_from_folder(&d).ok())
            .unwrap_or_default();

        Self { assets: HighlightingAssets::from_binary(), embedded: load_embedded(), user }
    }

    /// Resolve `name` to a [`Theme`], walking the priority chain and falling back
    /// to [`FALLBACK_THEME`] if `name` is unknown.
    pub fn resolve(&self, name: &str) -> Theme {
        if let Some(theme) = self.user.themes.get(name) {
            return theme.clone();
        }
        if let Some(theme) = self.embedded.themes.get(name) {
            return theme.clone();
        }
        // `get_theme` silently falls back to a default when the name is unknown,
        // so only call it for names we know it has — otherwise route to our own
        // explicit fallback below.
        if self.assets.themes().any(|t| t == name) {
            return self.assets.get_theme(name).clone();
        }
        self.assets.get_theme(FALLBACK_THEME).clone()
    }

    /// All theme names known to the registry, deduplicated and sorted. Useful for
    /// a theme picker or `--list-themes`-style output.
    pub fn available(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .user
            .themes
            .keys()
            .chain(self.embedded.themes.keys())
            .cloned()
            .chain(self.assets.themes().map(str::to_owned))
            .collect();
        names.sort();
        names.dedup();
        names
    }
}

impl Default for ThemeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse [`EMBEDDED_THEMES`] into a [`ThemeSet`]. A theme that fails to parse is
/// skipped rather than panicking, so one malformed entry can't break the TUI.
fn load_embedded() -> ThemeSet {
    let mut set = ThemeSet::new();
    for (name, bytes) in EMBEDDED_THEMES {
        match ThemeSet::load_from_reader(&mut Cursor::new(*bytes)) {
            Ok(theme) => {
                set.themes.insert((*name).to_owned(), theme);
            }
            Err(e) => {
                tracing::warn!("failed to parse embedded theme {name:?}: {e}");
            }
        }
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal valid `.tmTheme` plist used to exercise the user-directory path
    /// without depending on any vendored theme files.
    const TEST_THEME: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>name</key>
    <string>Test Theme</string>
    <key>settings</key>
    <array>
        <dict>
            <key>settings</key>
            <dict>
                <key>background</key>
                <string>#FF0000</string>
                <key>foreground</key>
                <string>#FFFFFF</string>
            </dict>
        </dict>
    </array>
</dict>
</plist>
"#;

    fn registry_with_no_user_dir() -> ThemeRegistry {
        ThemeRegistry::with_user_dir(None)
    }

    #[test]
    fn resolves_bundled_theme() {
        let registry = registry_with_no_user_dir();
        let theme = registry.resolve("Dracula");
        assert_eq!(theme.name.as_deref(), Some("Dracula"));
    }

    #[test]
    fn unknown_theme_falls_back() {
        let registry = registry_with_no_user_dir();
        let fallback = registry.resolve(FALLBACK_THEME);
        let resolved = registry.resolve("does-not-exist-anywhere");
        // The fallback theme is returned verbatim for unknown names.
        assert_eq!(resolved.name, fallback.name);
    }

    #[test]
    fn user_dir_overrides_bundled() {
        let dir = tempfile::tempdir().unwrap();
        // Name the file after a bundled theme so we can prove the user copy wins.
        std::fs::write(dir.path().join("Dracula.tmTheme"), TEST_THEME).unwrap();

        let registry = ThemeRegistry::with_user_dir(Some(dir.path().to_path_buf()));
        let theme = registry.resolve("Dracula");

        // The user file is named "Dracula" (the file stem) but carries our test
        // theme's distinguishing red background, proving it shadowed the bundle.
        assert_eq!(
            theme.settings.background,
            Some(syntect::highlighting::Color { r: 0xFF, g: 0x00, b: 0x00, a: 0xFF })
        );
    }

    #[test]
    fn available_is_sorted_deduped_and_nonempty() {
        let registry = registry_with_no_user_dir();
        let names = registry.available();
        assert!(names.contains(&"Dracula".to_string()));

        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "available() must be sorted");

        let mut deduped = names.clone();
        deduped.dedup();
        assert_eq!(names.len(), deduped.len(), "available() must be deduped");
    }
}
