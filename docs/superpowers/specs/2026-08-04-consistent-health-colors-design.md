# Consistent health colors across themes

## Problem

The TUI health indicator conveys a bookmark's state with color: green = good,
yellow = caution, red = bad. Today those colors are not a reliable cue across
themes.

Two theme paths feed the chrome `Palette`:

- **base16/base24 schemes** (`theme/base16.rs`) — health roles come from
  canonical slots: `base08` (red), `base0A` (yellow), `base0B` (green). Reliable.
- **`.tmTheme` themes** (`Palette::from_theme` in `theme.rs`) — the palette is
  derived from syntax scopes: `success` from the *string* scope, `warning` from
  the *numeric constant* scope, `error` from the *invalid* scope. None of these
  are guaranteed to be a green/yellow/red hue. A theme with blue strings yields a
  blue "healthy" cue, so the health state reads inconsistently.

## Goal

Every supported theme must render the health indicator with a real red, yellow,
and green. When a theme has no qualifying hue, fall back to fixed defaults.

## Approach

Introduce **dedicated health colors** on the palette, resolved independently of
the general `success`/`warning`/`error` roles (which keep their scope-derived
meanings for other chrome). This guarantees the semantic cue for health without
disturbing chrome that legitimately uses the string/number/invalid colors.

### 1. Palette additions

Add three fields to `Palette` in `theme.rs`:

```rust
pub health_good: Color,  // green
pub health_warn: Color,  // yellow
pub health_bad:  Color,  // red
```

`Palette::default()` sets them to the ANSI trio (`Color::Green` / `Color::Yellow`
/ `Color::Red`), so the fallback path and existing rendering are unchanged.

### 2. Resolving the three colors per theme

**base16 schemes** — `Base16Scheme::palette` trusts the canonical slots:

```rust
health_good: c(0x0B),  // green
health_warn: c(0x0A),  // yellow
health_bad:  c(0x08),  // red
```

No hue validation: base16 authors deliberately choose these slots, and a
muted-but-intentional red still reads as "the bad one" in context.

**`.tmTheme` themes** — `Palette::from_theme` searches the theme for the nearest
real red/yellow/green:

1. Gather a candidate pool of colors from common syntax scopes (`string`,
   `constant.numeric`, `constant.language`, `keyword`, `entity.name.function`,
   `entity.name.type`, `variable`, `support.type`, `invalid`) plus the global
   foreground. Dedupe.
2. Convert each candidate to HSL.
3. For each target hue — **red ≈ 0°, yellow ≈ 50°, green ≈ 130°** — keep only
   candidates that clear a **saturation floor** (≈ 0.25) and sit in a
   **lightness band** (≈ 0.2–0.85), rejecting grays and near-black/near-white.
   Among candidates within a **±40° hue tolerance**, pick the minimum hue
   distance.
4. If a target has no qualifying candidate, that field keeps its
   `Palette::default()` value (ANSI).

The HSL conversion and hue-matching live in small private helpers in `theme.rs`,
used only by `from_theme`.

### 3. Rewiring the health indicator

In `component/panel/health.rs`, `HealthStatus::color()` switches the
red/yellow/green states from the general roles to the dedicated health fields:

| State | Before | After |
|---|---|---|
| `Healthy`, `Verified` | `.success` | `.health_good` |
| `UnanchoredHealthy`, `Drifted`, `Outdated` | `.warning` | `.health_warn` |
| `Broken`, `BrokenUnanchored` | `.error` | `.health_bad` |

Unchanged: `UnanchoredDrifting` → fixed orange, `Unknown` → `.dim` (gray),
`Future` → `.info` (blue). These are outside the red/yellow/green triad, and the
orange is already theme-independent.

Because `Palette::default()`'s health fields are ANSI Red/Yellow/Green, the
existing `test_health_status_colors` assertions still hold.

## Testing

- `Palette::default()` health fields are ANSI Red/Yellow/Green.
- **base16**: Catppuccin Mocha / Everforest resolve `health_good/warn/bad` to
  base0B/0A/08.
- **tmTheme hue search**: a synthetic theme with a clearly green string, red
  invalid, and yellow constant resolves the health fields to those hues.
- **Fallback**: a grayscale-only synthetic theme leaves all three health fields
  at ANSI.
- **Saturation floor**: a desaturated near-gray candidate in the green hue range
  is rejected in favor of the ANSI fallback.
- Existing `health.rs` color tests updated for the new field routing (values
  unchanged under the default palette).

## Out of scope

- Changing the `success`/`warning`/`error` roles or other chrome coloring.
- The orange (`UnanchoredDrifting`), gray (`Unknown`), and blue (`Future`) cues.
- Light/dark adaptation beyond what ANSI fallback already provides.
