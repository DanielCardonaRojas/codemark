//! base16 / base24 color scheme support.
//!
//! A base16 scheme defines sixteen palette colors (`base00`..`base0F`) with
//! well-known semantic roles (`base08` = red, `base0A` = yellow, `base0B` =
//! green, `base0D` = blue, …). Unlike a `.tmTheme` — which only describes syntax
//! scopes — a scheme is a complete palette, so we can drive *both* the TUI chrome
//! ([`Palette`]) and the code-preview highlighting ([`Base16Scheme::to_syntect_theme`])
//! from a single source. That is what makes the whole TUI cohesive.
//!
//! See <https://github.com/tinted-theming/home> for the format and scheme
//! ecosystem (Catppuccin, Dracula, Gruvbox, Tokyo Night, Rosé Pine, …).

use std::str::FromStr;

use ratatui::style::Color;
use syntect::highlighting::{
    Color as SyntectColor, ScopeSelectors, StyleModifier, Theme, ThemeItem, ThemeSettings,
};

use super::Palette;

/// A parsed base16/base24 scheme: the sixteen `base00`..`base0F` colors as RGB.
///
/// base24 schemes (which add `base10`..`base17`) parse fine — the extra colors
/// are simply ignored, since the first sixteen carry every role we map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Base16Scheme {
    /// Display name (`name`/`scheme` field), used as the registry key.
    pub name: String,
    /// `base00`..`base0F` as `(r, g, b)`.
    colors: [(u8, u8, u8); 16],
}

impl Base16Scheme {
    /// Parse a scheme from base16 YAML. Supports both the current format
    /// (`palette:` submap) and the legacy format (top-level `base00:` keys), and
    /// hex values with or without a leading `#`.
    pub fn from_yaml(input: &str) -> Result<Self, String> {
        let value: serde_yaml::Value =
            serde_yaml::from_str(input).map_err(|e| format!("invalid YAML: {e}"))?;
        let root = value.as_mapping().ok_or("scheme is not a YAML mapping")?;

        // Colors live under `palette:` in the modern format, or at the top level
        // in the legacy format.
        let palette = root.get("palette").and_then(|v| v.as_mapping()).unwrap_or(root);

        let name = root
            .get("name")
            .or_else(|| root.get("scheme"))
            .and_then(|v| v.as_str())
            .unwrap_or("base16")
            .to_string();

        let mut colors = [(0u8, 0u8, 0u8); 16];
        for (i, slot) in colors.iter_mut().enumerate() {
            // Keys are `base00`..`base0F`; accept either case for the hex digit.
            let upper = format!("base{i:02X}");
            let lower = format!("base{i:02x}");
            let hex = palette
                .get(upper.as_str())
                .or_else(|| palette.get(lower.as_str()))
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("missing color `{upper}`"))?;
            *slot = parse_hex(hex).ok_or_else(|| format!("invalid hex `{hex}` for `{upper}`"))?;
        }

        Ok(Self { name, colors })
    }

    /// Map the scheme to the TUI chrome [`Palette`] using the standard base16
    /// role assignments, so status colors, accents, and borders all come from
    /// the same palette as the code preview.
    pub fn palette(&self) -> Palette {
        let c = |i: usize| rgb(self.colors[i]);
        Palette {
            emphasis: c(0x05), // default foreground
            dim: c(0x03),      // comments / line highlight
            gray: c(0x04),     // dark foreground
            accent: c(0x0D),   // functions / links (blue)
            success: c(0x0B),  // green
            warning: c(0x0A),  // yellow
            error: c(0x08),    // red
            info: c(0x0D),     // blue
            marker: c(0x0E),   // keywords (magenta/purple)
            inverse: c(0x00),  // background, for text on accent fills
            // Health cues trust the canonical base16 slots (the scheme author
            // deliberately assigns these), giving a real red/yellow/green.
            health_good: c(0x0B), // green
            health_warn: c(0x0A), // yellow
            health_bad: c(0x08),  // red
        }
    }

    /// Build a syntect highlighting theme from the scheme, following the base16
    /// "textmate" scope-to-color conventions. This drives the code preview from
    /// the same palette as the chrome.
    pub fn to_syntect_theme(&self) -> Theme {
        let c = |i: usize| syntect_color(self.colors[i]);

        let settings = ThemeSettings {
            background: Some(c(0x00)),
            foreground: Some(c(0x05)),
            caret: Some(c(0x05)),
            line_highlight: Some(c(0x01)),
            selection: Some(c(0x02)),
            gutter: Some(c(0x01)),
            gutter_foreground: Some(c(0x03)),
            ..ThemeSettings::default()
        };

        // (scope selector, base index). Standard base16 textmate mapping.
        const SCOPES: &[(&str, usize)] = &[
            ("comment, punctuation.definition.comment", 0x03),
            ("string, constant.other.symbol", 0x0B),
            ("constant.numeric, constant.language", 0x09),
            ("constant.character, constant.character.escape", 0x0C),
            ("keyword, storage.modifier, keyword.operator.logical", 0x0E),
            ("storage.type", 0x0A),
            ("entity.name.function, support.function, meta.function-call", 0x0D),
            ("entity.name.type, entity.name.class, support.type, support.class", 0x0A),
            ("variable, variable.parameter, meta.definition.variable", 0x08),
            ("entity.name.tag", 0x08),
            ("entity.other.attribute-name", 0x0A),
            ("keyword.operator, punctuation", 0x05),
            ("invalid, invalid.illegal", 0x08),
        ];

        let scopes = SCOPES
            .iter()
            .filter_map(|(scope, idx)| {
                ScopeSelectors::from_str(scope).ok().map(|sel| ThemeItem {
                    scope: sel,
                    style: StyleModifier {
                        foreground: Some(c(*idx)),
                        background: None,
                        font_style: None,
                    },
                })
            })
            .collect();

        Theme { name: Some(self.name.clone()), author: None, settings, scopes }
    }
}

/// Parse a `#rrggbb` or `rrggbb` hex string into `(r, g, b)`.
fn parse_hex(hex: &str) -> Option<(u8, u8, u8)> {
    let hex = hex.strip_prefix('#').unwrap_or(hex);
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some((r, g, b))
}

fn rgb((r, g, b): (u8, u8, u8)) -> Color {
    Color::Rgb(r, g, b)
}

fn syntect_color((r, g, b): (u8, u8, u8)) -> SyntectColor {
    SyntectColor { r, g, b, a: 0xFF }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOCHA: &str = include_str!("../../themes/catppuccin-mocha.yaml");

    #[test]
    fn parses_modern_palette_format() {
        let scheme = Base16Scheme::from_yaml(MOCHA).unwrap();
        assert_eq!(scheme.name, "Catppuccin Mocha");
        // base08 is Catppuccin Mocha red (#f38ba8).
        assert_eq!(scheme.colors[0x08], (0xf3, 0x8b, 0xa8));
    }

    #[test]
    fn parses_legacy_top_level_format() {
        // Older base16 schemes put colors at the top level and may use `scheme:`.
        let mut yaml = String::from("scheme: \"Legacy\"\n");
        for i in 0..16 {
            yaml.push_str(&format!("base{i:02X}: \"00112{i:x}\"\n"));
        }
        let scheme = Base16Scheme::from_yaml(&yaml).unwrap();
        assert_eq!(scheme.name, "Legacy");
        assert_eq!(scheme.colors[0x00], (0x00, 0x11, 0x20));
    }

    #[test]
    fn palette_uses_base16_role_mapping() {
        let scheme = Base16Scheme::from_yaml(MOCHA).unwrap();
        let p = scheme.palette();
        assert_eq!(p.error, Color::Rgb(0xf3, 0x8b, 0xa8)); // base08 red
        assert_eq!(p.success, Color::Rgb(0xa6, 0xe3, 0xa1)); // base0B green
        assert_eq!(p.warning, Color::Rgb(0xf9, 0xe2, 0xaf)); // base0A yellow
        assert_eq!(p.accent, Color::Rgb(0x89, 0xb4, 0xfa)); // base0D blue
        assert_eq!(p.emphasis, Color::Rgb(0xcd, 0xd6, 0xf4)); // base05 text
    }

    #[test]
    fn syntect_theme_carries_palette_background_and_scopes() {
        let scheme = Base16Scheme::from_yaml(MOCHA).unwrap();
        let theme = scheme.to_syntect_theme();
        assert_eq!(theme.name.as_deref(), Some("Catppuccin Mocha"));
        assert_eq!(
            theme.settings.background,
            Some(SyntectColor { r: 0x1e, g: 0x1e, b: 0x2e, a: 0xFF })
        );
        assert!(!theme.scopes.is_empty(), "expected scope rules to be generated");
    }

    #[test]
    fn rejects_invalid_scheme() {
        assert!(Base16Scheme::from_yaml("not: a scheme").is_err());
        assert!(Base16Scheme::from_yaml("palette:\n  base00: \"zzzzzz\"\n").is_err());
    }
}
