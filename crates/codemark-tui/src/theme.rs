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
use std::sync::RwLock;

use codemark_core::config::global_config_dir;
use ratatui::style::Color;
use syntect::highlighting::{Color as SyntectColor, Highlighter, Theme, ThemeSet};
use syntect::parsing::ScopeStack;
use syntect_assets::assets::HighlightingAssets;

use base16::Base16Scheme;

/// Fallback theme name. Always present in the `syntect-assets` binary data, so
/// resolution can never fail to produce a usable theme.
pub const FALLBACK_THEME: &str = "OneHalfDark";

/// Background color used to highlight the selected row across panes (the list
/// [`Panel`](crate::component::Panel) and the [`CodePreview`](crate::component::CodePreview)
/// selected line). A light gray-blue from the One Dark palette, kept the same
/// whether or not the pane is focused.
pub const SELECTION_BG: Color = Color::Rgb(62, 68, 81);

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

    /// Names of the base16/base24 schemes only, sorted. Unlike the syntect
    /// `.tmTheme` entries in [`available`], a scheme drives the *entire* TUI
    /// chrome (not just the code preview), so these are the themes worth
    /// showcasing in a per-theme screenshot gallery.
    pub fn scheme_names(&self) -> Vec<String> {
        // BTreeMap keys are already sorted.
        self.schemes.keys().cloned().collect()
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
    /// Health "good" cue (healthy). A real green in every theme. Default: green.
    ///
    /// Dedicated to the health indicator and resolved to an actual green/yellow/
    /// red hue per theme (see [`Palette::from_theme`]), so the health state reads
    /// consistently regardless of what the theme assigns to `success`/`warning`/
    /// `error`. Falls back to the ANSI default when a theme has no such hue.
    pub health_good: Color,
    /// Health "warn" cue (drifted). A real yellow in every theme. Default: yellow.
    pub health_warn: Color,
    /// Health "bad" cue (broken). A real red in every theme. Default: red.
    pub health_bad: Color,
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
            health_good: Color::Green,
            health_warn: Color::Yellow,
            health_bad: Color::Red,
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

        // Health cues are resolved separately from the roles above: a `.tmTheme`
        // carries no palette, so we search a pool of syntax-scope colors for a
        // real red/yellow/green hue rather than trusting any single scope (which
        // may be off-hue). A target with no qualifying candidate keeps its ANSI
        // default.
        let candidates = health_candidates(&hl, theme);
        let HealthHues { good, warn, bad } = resolve_health_hues(&candidates);
        if let Some(c) = good {
            palette.health_good = c;
        }
        if let Some(c) = warn {
            palette.health_warn = c;
        }
        if let Some(c) = bad {
            palette.health_bad = c;
        }

        palette
    }
}

// ---------------------------------------------------------------------------
// Health-color hue search
// ---------------------------------------------------------------------------

/// Target hues (degrees on the color wheel) the health cues snap to.
const HUE_RED: f32 = 0.0;
const HUE_YELLOW: f32 = 50.0;
const HUE_GREEN: f32 = 130.0;

/// The nearest theme candidate for each health role, or `None` where no
/// candidate qualified (the caller keeps its ANSI fallback).
struct HealthHues {
    good: Option<Color>,
    warn: Option<Color>,
    bad: Option<Color>,
}

/// A candidate must be within this many degrees of a target hue to qualify.
const HUE_TOLERANCE: f32 = 40.0;
/// Reject washed-out candidates: anything less saturated than this is treated as
/// a gray with no meaningful hue.
const SATURATION_FLOOR: f32 = 0.25;
/// Reject near-black and near-white candidates, which read poorly as a cue.
const LIGHTNESS_MIN: f32 = 0.20;
const LIGHTNESS_MAX: f32 = 0.85;

/// Collect the pool of colors a `.tmTheme` might offer as a red/yellow/green,
/// drawn from common syntax scopes plus the global foreground.
fn health_candidates(hl: &Highlighter, theme: &Theme) -> Vec<Color> {
    const SCOPES: &[&str] = &[
        "string",
        "constant.numeric",
        "constant.language",
        "keyword",
        "entity.name.function",
        "entity.name.type",
        "variable",
        "support.type",
        "invalid",
    ];
    let mut colors: Vec<Color> = SCOPES.iter().filter_map(|s| scope_color(hl, s)).collect();
    if let Some(fg) = theme.settings.foreground.and_then(to_rgb) {
        colors.push(fg);
    }
    colors.sort_by_key(|c| match c {
        Color::Rgb(r, g, b) => (*r, *g, *b),
        _ => (0, 0, 0),
    });
    colors.dedup();
    colors
}

/// Assign each candidate to the health role (green/yellow/red) whose target hue
/// it is *nearest* to, then keep the closest candidate for each role.
///
/// Assigning to the single nearest target — rather than testing each role
/// independently — is what keeps the three cues distinct. The red and yellow
/// acceptance windows overlap in the orange band (≈10°–40°), so an independent
/// test would let one orange candidate fill *both* `bad` and `warn` and collapse
/// the distinction. Here that candidate lands in exactly one role.
///
/// A candidate is ignored entirely if it fails the saturation/lightness floors
/// or if even its nearest target is beyond [`HUE_TOLERANCE`]; a role with no
/// assigned candidate stays `None` so the caller keeps its ANSI fallback.
fn resolve_health_hues(candidates: &[Color]) -> HealthHues {
    // Ordered [green, yellow, red] to match the destructuring in the caller;
    // ties in `min_by` resolve to the earlier target, deterministically.
    let targets = [HUE_GREEN, HUE_YELLOW, HUE_RED];
    let mut best: [Option<(Color, f32)>; 3] = [None, None, None];

    for &c in candidates {
        let Color::Rgb(r, g, b) = c else { continue };
        let (h, s, l) = rgb_to_hsl(r, g, b);
        if s < SATURATION_FLOOR || !(LIGHTNESS_MIN..=LIGHTNESS_MAX).contains(&l) {
            continue;
        }
        let (idx, dist) = targets
            .iter()
            .enumerate()
            .map(|(i, &t)| (i, hue_distance(h, t)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .expect("targets is non-empty");
        if dist > HUE_TOLERANCE {
            continue;
        }
        if best[idx].is_none_or(|(_, d)| dist < d) {
            best[idx] = Some((c, dist));
        }
    }

    HealthHues {
        good: best[0].map(|(c, _)| c),
        warn: best[1].map(|(c, _)| c),
        bad: best[2].map(|(c, _)| c),
    }
}

/// Shortest angular distance between two hues, in degrees (0..=180).
fn hue_distance(a: f32, b: f32) -> f32 {
    let d = (a - b).abs() % 360.0;
    if d > 180.0 { 360.0 - d } else { d }
}

/// Convert an RGB color to HSL: hue in degrees (0..360), saturation and
/// lightness in 0.0..=1.0.
fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;

    let l = (max + min) / 2.0;
    if delta == 0.0 {
        return (0.0, 0.0, l);
    }

    let s = delta / (1.0 - (2.0 * l - 1.0).abs());
    let mut h = if max == r {
        60.0 * (((g - b) / delta) % 6.0)
    } else if max == g {
        60.0 * (((b - r) / delta) + 2.0)
    } else {
        60.0 * (((r - g) / delta) + 4.0)
    };
    if h < 0.0 {
        h += 360.0;
    }
    (h, s, l)
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
static PALETTE: RwLock<Option<Palette>> = RwLock::new(None);

/// Set the process-wide chrome palette.
pub fn set_palette(palette: Palette) {
    if let Ok(mut guard) = PALETTE.write() {
        *guard = Some(palette);
    }
}

/// The active chrome palette, defaulting to the ANSI [`Palette::default`].
pub fn palette() -> Palette {
    if let Ok(guard) = PALETTE.read()
        && let Some(p) = *guard
    {
        return p;
    }
    Palette::default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use syntect::highlighting::{ScopeSelectors, StyleModifier, ThemeItem, ThemeSettings};

    /// Build a syntect [`Theme`] whose listed scopes carry the given RGB
    /// foregrounds, with a neutral-gray default foreground so unmatched scopes
    /// don't introduce spurious colored candidates. Used to drive the health-cue
    /// hue search without depending on a real `.tmTheme`.
    fn theme_with_scopes(scopes: &[(&str, (u8, u8, u8))]) -> Theme {
        let items = scopes
            .iter()
            .filter_map(|(sel, (r, g, b))| {
                ScopeSelectors::from_str(sel).ok().map(|scope| ThemeItem {
                    scope,
                    style: StyleModifier {
                        foreground: Some(SyntectColor { r: *r, g: *g, b: *b, a: 0xFF }),
                        background: None,
                        font_style: None,
                    },
                })
            })
            .collect();
        let settings = ThemeSettings {
            foreground: Some(SyntectColor { r: 0x80, g: 0x80, b: 0x80, a: 0xFF }),
            background: Some(SyntectColor { r: 0x10, g: 0x10, b: 0x10, a: 0xFF }),
            ..ThemeSettings::default()
        };
        Theme { name: Some("Test".into()), author: None, settings, scopes: items }
    }

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

    #[test]
    fn palette_default_health_is_ansi() {
        let p = Palette::default();
        assert_eq!(p.health_good, Color::Green);
        assert_eq!(p.health_warn, Color::Yellow);
        assert_eq!(p.health_bad, Color::Red);
    }

    #[test]
    fn base16_scheme_health_uses_canonical_slots() {
        let registry = registry_with_no_user_dir();
        let (_theme, palette) = registry.resolve_full("Catppuccin Mocha");
        assert_eq!(palette.health_good, Color::Rgb(0xa6, 0xe3, 0xa1)); // base0B green
        assert_eq!(palette.health_warn, Color::Rgb(0xf9, 0xe2, 0xaf)); // base0A yellow
        assert_eq!(palette.health_bad, Color::Rgb(0xf3, 0x8b, 0xa8)); // base08 red
    }

    #[test]
    fn from_theme_finds_red_yellow_green_by_hue() {
        // String is green, numeric is yellow, invalid is red — regardless of the
        // conventional role each scope plays, the health cues snap to the hue.
        let theme = theme_with_scopes(&[
            ("string", (0x00, 0xC8, 0x00)),           // green (hue 120)
            ("constant.numeric", (0xC8, 0xC8, 0x00)), // yellow (hue 60)
            ("invalid", (0xC8, 0x00, 0x00)),          // red (hue 0)
        ]);
        let p = Palette::from_theme(&theme);
        assert_eq!(p.health_good, Color::Rgb(0x00, 0xC8, 0x00));
        assert_eq!(p.health_warn, Color::Rgb(0xC8, 0xC8, 0x00));
        assert_eq!(p.health_bad, Color::Rgb(0xC8, 0x00, 0x00));
    }

    #[test]
    fn from_theme_falls_back_when_no_hue_qualifies() {
        // Only grayscale scope colors: no candidate has a usable hue, so every
        // health cue keeps its ANSI default.
        let theme = theme_with_scopes(&[
            ("string", (0x88, 0x88, 0x88)),
            ("keyword", (0x55, 0x55, 0x55)),
            ("invalid", (0xBB, 0xBB, 0xBB)),
        ]);
        let p = Palette::from_theme(&theme);
        assert_eq!(p.health_good, Color::Green);
        assert_eq!(p.health_warn, Color::Yellow);
        assert_eq!(p.health_bad, Color::Red);
    }

    #[test]
    fn from_theme_orange_candidate_fills_only_one_role() {
        // Orange (hue ~30°) sits in the overlap of the red and yellow acceptance
        // windows. It must fill exactly one role (yellow, its nearest target) —
        // never both — so the warn and bad cues stay distinct. The unfilled role
        // keeps its ANSI fallback.
        let theme = theme_with_scopes(&[("string", (0xFF, 0x80, 0x00))]); // hue 30
        let p = Palette::from_theme(&theme);
        assert_eq!(p.health_warn, Color::Rgb(0xFF, 0x80, 0x00));
        assert_eq!(p.health_bad, Color::Red, "orange must not also fill the bad role");
    }

    #[test]
    fn from_theme_rejects_desaturated_candidate() {
        // A green-*hued* but washed-out candidate is below the saturation floor,
        // so it's rejected and health_good falls back to ANSI green.
        let theme = theme_with_scopes(&[("string", (0x6E, 0x82, 0x6E))]);
        let p = Palette::from_theme(&theme);
        assert_eq!(p.health_good, Color::Green);
    }

    #[test]
    fn rgb_to_hsl_matches_known_values() {
        // Pure primaries land on their canonical hues.
        assert!((rgb_to_hsl(255, 0, 0).0 - 0.0).abs() < 0.5);
        assert!((rgb_to_hsl(0, 255, 0).0 - 120.0).abs() < 0.5);
        assert!((rgb_to_hsl(0, 0, 255).0 - 240.0).abs() < 0.5);
        // Gray has zero saturation.
        assert!(rgb_to_hsl(0x80, 0x80, 0x80).1 < f32::EPSILON);
    }

    #[test]
    fn hue_distance_wraps_around_the_wheel() {
        assert!((hue_distance(350.0, 10.0) - 20.0).abs() < 0.5);
        assert!((hue_distance(10.0, 350.0) - 20.0).abs() < 0.5);
        assert!((hue_distance(0.0, 180.0) - 180.0).abs() < 0.5);
    }
}
