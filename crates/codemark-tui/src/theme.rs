//! Hybrid theme loader for the TUI.
//!
//! Two kinds of theme are supported:
//!
//! - **base16/base24 schemes** ([`base16`]) — a full palette that drives *both*
//!   the code preview and the chrome ([`Palette`]) from one source, for a fully
//!   cohesive UI. This is the recommended path. Resolve via
//!   [`ThemeRegistry::resolve_full`].
//! - **`.tmTheme` themes** — syntax-only; the preview is themed fully and the
//!   chrome palette is approximated from the theme via [`Palette::from_theme`].
//!
//! Both kinds are resolved from these sources, in priority order (first match
//! wins): the user directory (`~/.config/codemark/themes`), then themes embedded
//! at compile time, then the `syntect-assets` bundle (Dracula, Nord, Gruvbox, …),
//! then [`FALLBACK_THEME`].
//!
//! This mirrors the bundle-plus-user-directory pattern already used for
//! templates in `codemark-core` (`templates.rs`).

pub mod base16;

use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::OnceLock;

use codemark_core::config::global_config_dir;
use ratatui::style::Color;
use syntect::highlighting::{Color as SyntectColor, Highlighter, Theme, ThemeSet};
use syntect::parsing::ScopeStack;
use syntect_assets::assets::HighlightingAssets;

use base16::Base16Scheme;

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

/// base16/base24 schemes embedded at compile time. Unlike [`EMBEDDED_THEMES`], a
/// scheme drives *both* the code preview and the chrome palette from one source,
/// giving a fully cohesive theme. Each entry is `(name, yaml)`.
const EMBEDDED_SCHEMES: &[(&str, &str)] = &[
    ("Catppuccin Mocha", include_str!("../themes/catppuccin-mocha.yaml")),
    ("Everforest Dark", include_str!("../themes/everforest-dark.yaml")),
];

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

/// Materialize the embedded extra themes ([`EMBEDDED_THEMES`]) into [`themes_dir`]
/// so users have real `.tmTheme` files to copy or tweak, mirroring
/// `ensure_default_template_exists` in `codemark-core`.
///
/// Best-effort: existing files are left untouched and any failure is logged
/// rather than propagated. No-op while [`EMBEDDED_THEMES`] is empty.
pub fn ensure_default_themes_exist() {
    if EMBEDDED_THEMES.is_empty() {
        return;
    }
    let Some(dir) = themes_dir() else { return };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!(target: "codemark::theme", "failed to create themes dir {}: {e}", dir.display());
        return;
    }
    for (name, bytes) in EMBEDDED_THEMES {
        let path = dir.join(format!("{name}.tmTheme"));
        if !path.exists()
            && let Err(e) = std::fs::write(&path, bytes)
        {
            tracing::warn!(target: "codemark::theme", "failed to write default theme {}: {e}", path.display());
        }
    }
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
    /// base16/base24 schemes (embedded + user `.yaml`/`.yml`), keyed by name.
    /// User schemes override embedded ones with the same name.
    schemes: BTreeMap<String, Base16Scheme>,
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
            .as_ref()
            .filter(|d| d.is_dir())
            .and_then(|d| ThemeSet::load_from_folder(d).ok())
            .unwrap_or_default();

        Self {
            assets: HighlightingAssets::from_binary(),
            embedded: load_embedded(),
            user,
            schemes: load_schemes(dir.as_deref()),
        }
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

    /// Resolve `name` to both the code-preview [`Theme`] and the chrome
    /// [`Palette`], so the whole TUI can be themed from a single source.
    ///
    /// A base16/base24 scheme drives both from one palette (fully cohesive);
    /// otherwise the name resolves to a `.tmTheme` via [`resolve`](Self::resolve)
    /// and the palette is derived from it with [`Palette::from_theme`].
    pub fn resolve_full(&self, name: &str) -> (Theme, Palette) {
        if let Some(scheme) = self.schemes.get(name) {
            return (scheme.to_syntect_theme(), scheme.palette());
        }
        let theme = self.resolve(name);
        let palette = Palette::from_theme(&theme);
        (theme, palette)
    }

    /// All theme names known to the registry, deduplicated and sorted. Useful for
    /// a theme picker or `--list-themes`-style output.
    pub fn available(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .schemes
            .keys()
            .cloned()
            .chain(self.user.themes.keys().cloned())
            .chain(self.embedded.themes.keys().cloned())
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
                tracing::warn!(target: "codemark::theme", "failed to parse embedded theme {name:?}: {e}");
            }
        }
    }
    set
}

/// Load base16 schemes: embedded ([`EMBEDDED_SCHEMES`]) first, then user
/// `.yaml`/`.yml` files from `dir` (which override embedded ones of the same
/// name). A scheme that fails to parse is logged and skipped.
fn load_schemes(dir: Option<&std::path::Path>) -> BTreeMap<String, Base16Scheme> {
    let mut schemes = BTreeMap::new();

    for (name, yaml) in EMBEDDED_SCHEMES {
        match Base16Scheme::from_yaml(yaml) {
            Ok(scheme) => {
                schemes.insert((*name).to_owned(), scheme);
            }
            Err(e) => {
                tracing::warn!(target: "codemark::theme", "failed to parse embedded scheme {name:?}: {e}")
            }
        }
    }

    if let Some(dir) = dir.filter(|d| d.is_dir())
        && let Ok(entries) = std::fs::read_dir(dir)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            let is_yaml = path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("yaml") || e.eq_ignore_ascii_case("yml"));
            if !is_yaml {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()).map(str::to_owned) else {
                continue;
            };
            match std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|s| Base16Scheme::from_yaml(&s))
            {
                Ok(scheme) => {
                    schemes.insert(stem, scheme);
                }
                Err(e) => {
                    tracing::warn!(target: "codemark::theme", "failed to load scheme {}: {e}", path.display())
                }
            }
        }
    }

    schemes
}

// ---------------------------------------------------------------------------
// Chrome palette
// ---------------------------------------------------------------------------

/// Semantic colors for the TUI "chrome" — everything outside the syntax-
/// highlighted code preview: borders, titles, status icons, secondary text, etc.
///
/// Field defaults reproduce the TUI's original hardcoded ANSI colors, so an
/// unset theme renders exactly as before. [`Palette::from_theme`] derives the
/// whole palette — structural *and* status roles — from a syntect theme by
/// reading its global settings and a handful of conventional TextMate scopes,
/// so a `.tmTheme` themes the entire chrome (a base16 scheme does the same, more
/// precisely, via [`base16::Base16Scheme::palette`]). A role whose scope a theme
/// doesn't define keeps its ANSI default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// Emphasized foreground text. Default: white.
    pub emphasis: Color,
    /// Muted/secondary text, borders, scrollbars. Default: dark gray.
    pub dim: Color,
    /// Neutral gray. Default: gray.
    pub gray: Color,
    /// Accent for icons, metadata, and links. Default: cyan.
    pub accent: Color,
    /// Success / healthy / active focus. Default: green.
    pub success: Color,
    /// Warning / drifted / in-progress. Default: yellow.
    pub warning: Color,
    /// Error / broken. Default: red.
    pub error: Color,
    /// Informational. Default: blue.
    pub info: Color,
    /// Bookmark range / selection marker. Default: magenta.
    pub marker: Color,
    /// Foreground drawn on inverted/highlighted backgrounds. Default: black.
    pub inverse: Color,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            emphasis: Color::White,
            dim: Color::DarkGray,
            gray: Color::Gray,
            accent: Color::Cyan,
            success: Color::Green,
            warning: Color::Yellow,
            error: Color::Red,
            info: Color::Blue,
            marker: Color::Magenta,
            inverse: Color::Black,
        }
    }
}

impl Palette {
    /// Derive the full chrome palette from a syntect theme.
    ///
    /// Structural roles come from the global settings (`emphasis` from the
    /// foreground, `gray` from a foreground/background blend) and conventional
    /// scopes (`dim` from comments, `accent` from functions). Status roles map to
    /// the scopes that carry each base16-style hue: `error` ← `invalid` (red),
    /// `success` ← `string` (green), `warning` ← numeric constants (yellow/orange),
    /// `marker` ← `keyword` (purple). `info` follows `accent` (both are the
    /// "blue/function" role, matching the base16 mapping). Any role whose scope a
    /// theme doesn't define keeps its ANSI default.
    pub fn from_theme(theme: &Theme) -> Self {
        let hl = Highlighter::new(theme);
        let mut palette = Palette::default();

        // Structural roles from the global settings.
        if let Some(c) = theme.settings.foreground.and_then(to_rgb) {
            palette.emphasis = c;
        }
        if let (Some(fg), Some(bg)) = (theme.settings.foreground, theme.settings.background) {
            palette.gray = mix(fg, bg, 0.5);
        }
        if let Some(c) = theme.settings.background.and_then(to_rgb) {
            palette.inverse = c;
        }

        // Roles from conventional TextMate scopes (first match wins).
        if let Some(c) = first_scope_color(&hl, &["comment", "punctuation.definition.comment"]) {
            palette.dim = c;
        }
        if let Some(c) =
            first_scope_color(&hl, &["entity.name.function", "support.function", "keyword"])
        {
            palette.accent = c;
        }
        palette.info = palette.accent;
        if let Some(c) = first_scope_color(&hl, &["string", "string.quoted"]) {
            palette.success = c;
        }
        if let Some(c) = first_scope_color(&hl, &["constant.numeric", "constant.other", "constant"])
        {
            palette.warning = c;
        }
        if let Some(c) = first_scope_color(&hl, &["invalid.illegal", "invalid"]) {
            palette.error = c;
        }
        if let Some(c) = first_scope_color(&hl, &["keyword", "storage.type", "storage.modifier"]) {
            palette.marker = c;
        }

        palette
    }
}

/// Convert a syntect color to a ratatui RGB color, treating fully transparent
/// colors (alpha 0, syntect's "unset" sentinel) as absent.
fn to_rgb(c: SyntectColor) -> Option<Color> {
    (c.a != 0).then_some(Color::Rgb(c.r, c.g, c.b))
}

/// Foreground color a theme assigns to the first of `scopes` it defines.
fn first_scope_color(highlighter: &Highlighter, scopes: &[&str]) -> Option<Color> {
    scopes.iter().find_map(|s| scope_color(highlighter, s))
}

/// Foreground color a theme assigns to a TextMate scope (e.g. "comment").
fn scope_color(highlighter: &Highlighter, scope: &str) -> Option<Color> {
    let stack = ScopeStack::from_str(scope).ok()?;
    to_rgb(highlighter.style_for_stack(stack.as_slice()).foreground)
}

/// Linearly blend two syntect colors (`t` = 0.0 → `a`, 1.0 → `b`) into a ratatui
/// RGB color. Used to synthesize a muted tone from a theme's foreground and
/// background.
fn mix(a: SyntectColor, b: SyntectColor, t: f32) -> Color {
    let lerp = |x: u8, y: u8| (x as f32 * (1.0 - t) + y as f32 * t).round() as u8;
    Color::Rgb(lerp(a.r, b.r), lerp(a.g, b.g), lerp(a.b, b.b))
}

/// Process-wide chrome palette. Set once from config via [`set_palette`];
/// otherwise the ANSI-based [`Palette::default`].
static PALETTE: OnceLock<Palette> = OnceLock::new();

/// Set the process-wide chrome palette. Call once at startup (alongside the
/// preview theme), before the UI is built. A no-op if already set.
pub fn set_palette(palette: Palette) {
    let _ = PALETTE.set(palette);
}

/// The active chrome palette, defaulting to the ANSI [`Palette::default`].
pub fn palette() -> Palette {
    *PALETTE.get_or_init(Palette::default)
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
    fn palette_default_matches_original_ansi_colors() {
        let p = Palette::default();
        assert_eq!(p.dim, Color::DarkGray);
        assert_eq!(p.accent, Color::Cyan);
        assert_eq!(p.success, Color::Green);
        assert_eq!(p.error, Color::Red);
    }

    #[test]
    fn palette_from_theme_themes_every_role() {
        let theme = default_theme();
        let p = Palette::from_theme(&theme);
        // Every role that has a reliable source (global settings or a near-
        // universal scope) is now theme-derived rather than ANSI.
        for role in [p.emphasis, p.dim, p.gray, p.accent, p.info, p.success, p.inverse] {
            assert!(matches!(role, Color::Rgb(..)), "expected themed RGB, got {role:?}");
        }
        // info mirrors accent (both are the base16 "blue/function" role).
        assert_eq!(p.info, p.accent);
    }

    #[test]
    fn resolve_full_uses_base16_scheme_for_both_theme_and_palette() {
        let registry = registry_with_no_user_dir();
        let (theme, palette) = registry.resolve_full("Catppuccin Mocha");
        // Both come from the scheme: preview background and chrome error color
        // are the Catppuccin Mocha palette, not ANSI/fallback.
        assert_eq!(theme.name.as_deref(), Some("Catppuccin Mocha"));
        assert_eq!(palette.error, Color::Rgb(0xf3, 0x8b, 0xa8));
        assert_eq!(palette.success, Color::Rgb(0xa6, 0xe3, 0xa1));
    }

    #[test]
    fn resolve_full_falls_back_to_tmtheme_palette_for_non_scheme() {
        let registry = registry_with_no_user_dir();
        let (theme, palette) = registry.resolve_full("Dracula");
        assert_eq!(theme.name.as_deref(), Some("Dracula"));
        // tmTheme path: the palette is derived from the theme (RGB), not ANSI.
        assert!(matches!(palette.emphasis, Color::Rgb(..)));
        assert!(matches!(palette.success, Color::Rgb(..)));
    }

    #[test]
    fn scheme_name_appears_in_available() {
        let registry = registry_with_no_user_dir();
        assert!(registry.available().contains(&"Catppuccin Mocha".to_string()));
        assert!(registry.available().contains(&"Everforest Dark".to_string()));
    }

    #[test]
    fn resolve_full_uses_everforest_scheme_for_both_theme_and_palette() {
        let registry = registry_with_no_user_dir();
        let (theme, palette) = registry.resolve_full("Everforest Dark");
        assert_eq!(theme.name.as_deref(), Some("Everforest Dark"));
        assert_eq!(palette.error, Color::Rgb(0xe6, 0x7e, 0x80)); // base08 red
        assert_eq!(palette.success, Color::Rgb(0xa7, 0xc0, 0x80)); // base0B green
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
