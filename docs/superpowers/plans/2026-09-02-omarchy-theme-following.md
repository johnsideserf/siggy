# Omarchy Theme Following Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A user on Omarchy installs siggy, opens it, and it is already wearing their current Omarchy theme — no config, no flags — and it follows OS theme changes without a restart.

**Architecture:** A new `src/theme/omarchy.rs` reads `~/.local/state/omarchy/current/theme/colors.toml`, ports Omarchy's own key-resolution cascade, and maps the result onto siggy's existing `Theme` struct. The derived theme is injected into `all_themes()` under the name `"Omarchy"` when — and only when — Omarchy is detected, so it flows through the existing picker, `find_theme()`, and config-save paths with no special-casing. `default_theme()` becomes `"Omarchy"`, which self-heals to `Default` on non-Omarchy machines. Live following is a SIGUSR1 handler (matching Omarchy's `omarchy-restart-helix` convention) plus an mtime poll on the existing 10s sweep.

**Tech Stack:** Rust 2024, no new dependencies. `toml` and `dirs` are already direct dependencies; `tokio` is already present with `features = ["full"]`, which supplies `tokio::signal::unix` for the SIGUSR1 handler.

**Spec:** [Issue #697](https://github.com/johnsideserf/siggy/issues/697)

## Global Constraints

- **No new dependencies.** Everything needed is already in `Cargo.toml`.
- **No new `App` field.** `scripts/check-app-field-count.sh` is CI-enforced; this feature reuses the existing `app.theme` and `app.theme_picker`.
- **Must compile and pass clippy under both backends.** CI builds `signal-cli-backend` (default) and `--no-default-features --features native-backend` (`.github/workflows/ci.yml:109-111`). `cargo build --all-features` fails BY DESIGN — never use it to check. Nothing in this feature may reference `backend::`.
- **Never panic on malformed theme data.** Follow the `#487` precedent already in `src/theme.rs`: a bad colour is skipped and logged via `crate::debug_log::logf`, never propagated as a panic.
- **Non-Linux builds must be unaffected.** Gate the SIGUSR1 handler with `#[cfg(unix)]`; the file-reading path is portable and needs no gate.
- **No existing user's theme may change.** Verified mechanism: `theme` is `#[serde(default = "default_theme")]` (`src/config.rs:227`) so the default only fires on an *absent* key, and `Config::save()` (`src/config.rs:388`) always writes the key. Every existing config on disk already carries an explicit theme name.
- **Git workflow:** branch `feature/697-omarchy-theme`, never commit to master. Run `cargo clippy --tests -- -D warnings && cargo test` before pushing.

---

## Background: what we are porting

Verified against Omarchy `4.0.0.alpha`. The reference implementation is `/usr/share/omarchy/bin/omarchy-theme-color`, which every other Omarchy consumer (templates, OSC, tmux, GNOME) goes through. We reimplement it in Rust rather than shelling out, because a subprocess breaks over ssh and in bare ttys where the file probe still works.

Its cascade, in order, is reproduced in Task 2. The two subtleties worth stating up front:

1. **`accent` has no fallback in Omarchy's resolver.** `grep -n accent omarchy-theme-color` matches only a comment. All 22 stock themes define it, but a third-party theme need not, so siggy must supply its own fallback (`accent` → `blue` → `foreground`).
2. **Mode is derived, not always declared.** Precedence is `mode` key → legacy `theme_type` key → a `light.mode` file beside colors.toml → background luminance (`r+g+b > 382` means light) → `dark`.

---

## File Structure

| Action | File | Responsibility |
|--------|------|---------------|
| Rename | `src/theme.rs` → `src/theme/mod.rs` | Unchanged content; the move matches the repo's `dir/mod.rs` convention (`src/backend/`, `src/domain/`, `src/handlers/`, `src/signal/`, `src/ui/`) and makes room for a child module |
| Create | `src/theme/omarchy.rs` | Everything Omarchy-specific: locating `colors.toml`, parsing it, the resolver cascade, mode detection, and the mapping onto `Theme` |
| Modify | `src/theme/mod.rs` | `pub mod omarchy;`; inject the derived theme into `all_themes()` |
| Modify | `src/config.rs:250` | `default_theme()` returns `"Omarchy"` |
| Modify | `src/main.rs` | SIGUSR1 handler; mtime poll folded into the existing 10s sweep |
| Modify | `README.md` | Document the behaviour and the optional hook |

Everything Omarchy-shaped lives in `src/theme/omarchy.rs`. `mod.rs` gains three lines and knows nothing about `colors.toml`.

---

## Task 1: Parse colors.toml into a raw key/value map

Omarchy's `colors.toml` is TOML-shaped but is parsed by Omarchy itself with a hand-rolled line reader that tolerates things strict TOML does not. We use the real `toml` crate (already a dependency) and treat every value as a string, because values are not all colours — `mode = "dark"` is a keyword, and gradient values like `-45deg` appear in some themes.

**Files:**
- Create: `src/theme/omarchy.rs`
- Modify: `src/theme/mod.rs`
- Rename: `src/theme.rs` → `src/theme/mod.rs`

**Interfaces:**
- Consumes: nothing
- Produces: `pub(crate) fn parse_colors(contents: &str) -> HashMap<String, String>` — every top-level key in the document, values as raw strings with quotes stripped. Non-string scalars are stringified; tables and arrays are skipped.

- [ ] **Step 1: Move the file so the module has room for a child**

```bash
mkdir -p src/theme
git mv src/theme.rs src/theme/mod.rs
cargo test theme
```

Expected: PASS. This is a pure rename; `pub mod theme;` in `src/lib.rs` still resolves. Commit this on its own so the rename is reviewable separately from any content change.

```bash
git add -A && git commit -m "refactor(theme): move theme.rs to theme/mod.rs to make room for submodules"
```

- [ ] **Step 2: Declare the new module**

At the top of `src/theme/mod.rs`, directly below the existing `use` block:

```rust
pub mod omarchy;
```

- [ ] **Step 3: Write the failing test**

Create `src/theme/omarchy.rs` containing only this test module:

```rust
//! Follow the Omarchy desktop theme.
//!
//! Omarchy publishes the active theme's palette at
//! `~/.local/state/omarchy/current/theme/colors.toml`. This module reads it,
//! reproduces Omarchy's own key-resolution cascade (see
//! `/usr/share/omarchy/bin/omarchy-theme-color`), and maps the result onto
//! [`crate::theme::Theme`].

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_colors_reads_string_values() {
        let src = r#"
# Generated by Aether for Omarchy v4.
mode = "dark"
accent = "#ac6380"
background = "#000000"
"#;
        let map = parse_colors(src);
        assert_eq!(map.get("mode").map(String::as_str), Some("dark"));
        assert_eq!(map.get("accent").map(String::as_str), Some("#ac6380"));
        assert_eq!(map.get("background").map(String::as_str), Some("#000000"));
        assert_eq!(map.len(), 3);
    }

    #[test]
    fn parse_colors_ignores_tables_and_returns_empty_on_garbage() {
        let src = "accent = \"#ac6380\"\n[some_table]\nnested = \"x\"\n";
        let map = parse_colors(src);
        assert_eq!(map.get("accent").map(String::as_str), Some("#ac6380"));
        assert!(!map.contains_key("nested"));
        assert!(!map.contains_key("some_table"));

        assert!(parse_colors("this is not = = toml").is_empty());
    }
}
```

- [ ] **Step 4: Run the test to verify it fails**

Run: `cargo test theme::omarchy`
Expected: FAIL to compile — `cannot find function 'parse_colors' in this scope`.

- [ ] **Step 5: Write the minimal implementation**

Above the test module in `src/theme/omarchy.rs`:

```rust
use std::collections::HashMap;

/// Parse a `colors.toml` into a flat map of top-level string values.
///
/// Values are kept as strings rather than colours because not every value is
/// a colour: `mode` is a keyword, and some themes carry gradient specs like
/// `-45deg`. A document that does not parse yields an empty map -- the caller
/// then falls back to siggy's own default theme.
pub(crate) fn parse_colors(contents: &str) -> HashMap<String, String> {
    let table: toml::Table = match contents.parse() {
        Ok(t) => t,
        Err(e) => {
            crate::debug_log::logf(format_args!("omarchy colors.toml parse error: {e}"));
            return HashMap::new();
        }
    };
    table
        .into_iter()
        .filter_map(|(k, v)| match v {
            toml::Value::String(s) => Some((k, s)),
            toml::Value::Integer(_) | toml::Value::Float(_) | toml::Value::Boolean(_) => {
                Some((k, v.to_string()))
            }
            _ => None,
        })
        .collect()
}
```

- [ ] **Step 6: Run the test to verify it passes**

Run: `cargo test theme::omarchy`
Expected: PASS, 2 tests.

- [ ] **Step 7: Commit**

```bash
git add src/theme/omarchy.rs src/theme/mod.rs
git commit -m "feat(theme): parse Omarchy colors.toml into a raw key map (#697)"
```

---

## Task 2: Port Omarchy's resolver cascade

This is the correctness core. A sparse theme that defines only ANSI `colorN` names must still produce a full palette, exactly as Omarchy would.

**Files:**
- Modify: `src/theme/omarchy.rs`

**Interfaces:**
- Consumes: `parse_colors` from Task 1
- Produces:
  - `pub(crate) fn mix(start: &str, end: &str, amount: f64) -> String` — linear per-channel blend of two `#rrggbb` strings, returning `#rrggbb`
  - `pub(crate) fn resolve(map: &mut HashMap<String, String>, light_mode_file: bool)` — applies the full cascade in place, guaranteeing that `mode` and every semantic colour key is populated

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `src/theme/omarchy.rs`:

```rust
    fn resolved(src: &str) -> HashMap<String, String> {
        let mut map = parse_colors(src);
        resolve(&mut map, false);
        map
    }

    #[test]
    fn mix_blends_channels_and_rounds_half_up() {
        // Matches the awk in omarchy-theme-color: int(a*(1-t) + b*t + 0.5)
        assert_eq!(mix("#ffffff", "#000000", 0.5), "#808080");
        assert_eq!(mix("#ff0000", "#ffffff", 0.2), "#ff3333");
        assert_eq!(mix("#000000", "#000000", 0.25), "#000000");
    }

    #[test]
    fn legacy_ansi_only_theme_gets_a_full_semantic_palette() {
        // A pre-semantic theme defining only colorN must still resolve.
        let map = resolved(
            r#"
color0 = "#1e1e2e"
color1 = "#f38ba8"
color2 = "#a6e3a1"
color3 = "#f9e2af"
color4 = "#89b4fa"
color5 = "#cba6f7"
color6 = "#94e2d5"
color7 = "#cdd6f4"
color8 = "#585b70"
"#,
        );
        assert_eq!(map["background"], "#1e1e2e");
        assert_eq!(map["foreground"], "#cdd6f4");
        assert_eq!(map["red"], "#f38ba8");
        assert_eq!(map["magenta"], "#cba6f7");
        assert_eq!(map["muted"], "#585b70");
        assert_eq!(map["dark_foreground"], "#585b70");
        // bright_* derived by mixing 20% white when absent
        assert_eq!(map["bright_red"], mix("#f38ba8", "#ffffff", 0.2));
        // darker_background derived by mixing 50% black when absent
        assert_eq!(map["darker_background"], mix("#1e1e2e", "#000000", 0.5));
    }

    #[test]
    fn short_palette_aliases_are_accepted() {
        let map = resolved("bg = \"#101010\"\nfg = \"#f0f0f0\"\n");
        assert_eq!(map["background"], "#101010");
        assert_eq!(map["foreground"], "#f0f0f0");
    }

    #[test]
    fn purple_aliases_to_magenta() {
        let map = resolved("purple = \"#cba6f7\"\nbackground = \"#000000\"\n");
        assert_eq!(map["magenta"], "#cba6f7");
    }

    #[test]
    fn mode_precedence_explicit_key_wins() {
        let map = resolved("mode = \"light\"\nbackground = \"#000000\"\n");
        assert_eq!(map["mode"], "light");
    }

    #[test]
    fn mode_falls_back_to_legacy_theme_type() {
        let map = resolved("theme_type = \"light\"\nbackground = \"#000000\"\n");
        assert_eq!(map["mode"], "light");
    }

    #[test]
    fn mode_falls_back_to_light_mode_file() {
        let mut map = parse_colors("background = \"#000000\"\n");
        resolve(&mut map, true);
        assert_eq!(map["mode"], "light");
    }

    #[test]
    fn mode_falls_back_to_background_luminance() {
        // r+g+b > 382 is light, per omarchy-theme-color's resolve_theme_mode
        let dark = resolved("background = \"#000000\"\n");
        assert_eq!(dark["mode"], "dark");
        let light = resolved("background = \"#ffffff\"\n");
        assert_eq!(light["mode"], "light");
        // Exactly at the boundary (381) stays dark.
        let boundary = resolved("background = \"#7f7f7f\"\n");
        assert_eq!(boundary["mode"], "dark");
    }

    #[test]
    fn mode_defaults_to_dark_without_a_usable_background() {
        let map = resolved("accent = \"#ac6380\"\n");
        assert_eq!(map["mode"], "dark");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test theme::omarchy`
Expected: FAIL to compile — `cannot find function 'resolve'`, `cannot find function 'mix'`.

- [ ] **Step 3: Write the implementation**

Add to `src/theme/omarchy.rs`, above the test module:

```rust
/// Parse `#rrggbb` into its three channels. Returns `None` for anything else,
/// which is how non-colour values (keywords, gradient specs) stay out of the
/// arithmetic.
fn channels(hex: &str) -> Option<(f64, f64, f64)> {
    let h = hex.strip_prefix('#')?;
    if h.len() != 6 || !h.is_ascii() {
        return None;
    }
    let r = u8::from_str_radix(&h[0..2], 16).ok()?;
    let g = u8::from_str_radix(&h[2..4], 16).ok()?;
    let b = u8::from_str_radix(&h[4..6], 16).ok()?;
    Some((r as f64, g as f64, b as f64))
}

/// Linear per-channel blend, reproducing the awk in `omarchy-theme-color`:
/// `int(start * (1 - amount) + end * amount + 0.5)`. Returns `start`
/// unchanged if either operand is not a hex colour.
pub(crate) fn mix(start: &str, end: &str, amount: f64) -> String {
    let (Some(s), Some(e)) = (channels(start), channels(end)) else {
        return start.to_string();
    };
    let t = amount.clamp(0.0, 1.0);
    let blend = |a: f64, b: f64| (a * (1.0 - t) + b * t + 0.5) as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        blend(s.0, e.0),
        blend(s.1, e.1),
        blend(s.2, e.2)
    )
}

/// Set `key` from `from` only when `key` is absent. Mirrors
/// `alias_theme_color` in omarchy-theme-color: canonical names always win.
fn alias(map: &mut HashMap<String, String>, key: &str, from: &str) {
    if map.contains_key(key) {
        return;
    }
    if let Some(v) = map.get(from).cloned() {
        map.insert(key.to_string(), v);
    }
}

/// Set `key` to the first present candidate, if `key` is absent.
fn alias_any(map: &mut HashMap<String, String>, key: &str, from: &[&str]) {
    if map.contains_key(key) {
        return;
    }
    for candidate in from {
        if let Some(v) = map.get(*candidate).cloned() {
            map.insert(key.to_string(), v);
            return;
        }
    }
}

/// Set `key` to `mix(base, toward, amount)` if `key` is absent and `base` resolves.
fn derive(map: &mut HashMap<String, String>, key: &str, base: &str, toward: &str, amount: f64) {
    if map.contains_key(key) {
        return;
    }
    if let Some(b) = map.get(base).cloned() {
        map.insert(key.to_string(), mix(&b, toward, amount));
    }
}

/// Resolve `mode`, mirroring `resolve_theme_mode` in omarchy-theme-color.
fn resolve_mode(map: &mut HashMap<String, String>, light_mode_file: bool) {
    alias(map, "mode", "theme_type");
    if map.contains_key("mode") {
        return;
    }
    let mode = if light_mode_file {
        "light"
    } else if let Some(sum) = map
        .get("background")
        .and_then(|bg| channels(bg))
        .map(|(r, g, b)| r + g + b)
    {
        if sum > 382.0 { "light" } else { "dark" }
    } else {
        "dark"
    };
    map.insert("mode".to_string(), mode.to_string());
}

/// Apply Omarchy's full key-resolution cascade in place.
///
/// Ported from `resolve_theme_colors` in
/// `/usr/share/omarchy/bin/omarchy-theme-color` (Omarchy 4.0.0.alpha). The
/// ordering matters: short-name aliases first, then ANSI fallbacks, then
/// derived shades, then the ANSI back-fill.
pub(crate) fn resolve(map: &mut HashMap<String, String>, light_mode_file: bool) {
    // 1. Legacy short-name palette (bg/fg/...). Canonical names take precedence.
    const SHORT: [(&str, &str); 8] = [
        ("background", "bg"),
        ("dark_background", "dark_bg"),
        ("darker_background", "darker_bg"),
        ("lighter_background", "lighter_bg"),
        ("foreground", "fg"),
        ("dark_foreground", "dark_fg"),
        ("light_foreground", "light_fg"),
        ("bright_foreground", "bright_fg"),
    ];
    for (canonical, short) in SHORT {
        alias(map, canonical, short);
    }

    // 2. Themes predating the semantic palette may define only ANSI names.
    alias(map, "background", "color0");
    alias(map, "foreground", "color7");
    alias(map, "color0", "background");
    alias(map, "color7", "foreground");

    // 3. ANSI -> semantic.
    const ANSI: [(&str, &str); 12] = [
        ("red", "color1"),
        ("green", "color2"),
        ("yellow", "color3"),
        ("blue", "color4"),
        ("magenta", "color5"),
        ("cyan", "color6"),
        ("bright_red", "color9"),
        ("bright_green", "color10"),
        ("bright_yellow", "color11"),
        ("bright_blue", "color12"),
        ("bright_magenta", "color13"),
        ("bright_cyan", "color14"),
    ];
    for (semantic, ansi) in ANSI {
        alias(map, semantic, ansi);
    }
    alias(map, "magenta", "purple");
    alias(map, "bright_magenta", "bright_purple");

    // 4. Semantic fallbacks.
    alias_any(map, "light_foreground", &["color7", "foreground"]);
    alias_any(map, "bright_foreground", &["color15", "foreground"]);
    alias(map, "cursor", "bright_foreground");
    alias_any(map, "lighter_background", &["color0", "background"]);
    alias_any(map, "dark_foreground", &["color8", "foreground"]);
    alias_any(map, "muted", &["color8", "dark_foreground"]);
    alias_any(
        map,
        "selection",
        &["selection_background", "color8", "color0", "background"],
    );
    alias(map, "selection_background", "selection");
    alias(map, "selection_foreground", "bright_foreground");
    alias(map, "orange", "yellow");
    derive(map, "brown", "orange", "#000000", 0.5);

    // 5. Derived shades.
    derive(map, "dark_background", "background", "#000000", 0.25);
    derive(map, "darker_background", "background", "#000000", 0.5);
    for base in ["red", "yellow", "green", "cyan", "blue", "magenta"] {
        derive(map, &format!("bright_{base}"), base, "#ffffff", 0.2);
    }
    alias(map, "purple", "magenta");
    alias(map, "bright_purple", "bright_magenta");

    // 6. Back-fill ANSI names for completeness.
    const BACKFILL: [(&str, &str); 16] = [
        ("color0", "background"),
        ("color1", "red"),
        ("color2", "green"),
        ("color3", "yellow"),
        ("color4", "blue"),
        ("color5", "magenta"),
        ("color6", "cyan"),
        ("color7", "foreground"),
        ("color8", "muted"),
        ("color9", "bright_red"),
        ("color10", "bright_green"),
        ("color11", "bright_yellow"),
        ("color12", "bright_blue"),
        ("color13", "bright_magenta"),
        ("color14", "bright_cyan"),
        ("color15", "bright_foreground"),
    ];
    for (ansi, semantic) in BACKFILL {
        alias(map, ansi, semantic);
    }

    resolve_mode(map, light_mode_file);
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test theme::omarchy`
Expected: PASS, 11 tests.

- [ ] **Step 5: Commit**

```bash
git add src/theme/omarchy.rs
git commit -m "feat(theme): port Omarchy's colour resolver cascade (#697)"
```

---

## Task 3: Map the resolved palette onto siggy's Theme

**Files:**
- Modify: `src/theme/omarchy.rs`

**Interfaces:**
- Consumes: `resolve` and `parse_colors` from Tasks 1-2
- Produces: `pub(crate) fn theme_from_colors(map: &HashMap<String, String>, name: &str) -> Theme`

Design notes, so the mapping is not mistaken for arbitrary:

- **`bg` is `Color::Reset`, not `background`.** Omarchy terminals support transparency and blur; a TUI that paints an opaque background becomes a solid rectangle on an otherwise translucent desktop. Inheriting the terminal is both prettier and more correct, and the terminal has already been retinted by Omarchy.
- **Status bar colours go through `readable_on`.** `darker_background` is the natural status-bar background in a dark theme, but in a light theme Omarchy *derives* it as 50% black, giving a mid-grey bar. `readable_on` picks whichever of the foreground and background colours sits furthest from the bar in luminance. It must not assume the foreground is the lighter of the two — that holds in a dark theme and is exactly inverted in a light one.
- **`accent` gets a siggy-side fallback** (`accent` → `blue` → `foreground`), because Omarchy's resolver provides none.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module:

```rust
    use ratatui::style::Color;

    const AETHER: &str = r#"
mode = "dark"
accent = "#ac6380"
selection = "#1a1a1a"
muted = "#686163"
background = "#000000"
lighter_background = "#1a1a1a"
foreground = "#E7E6E5"
dark_foreground = "#adadac"
red = "#c47d75"
yellow = "#ffd3a1"
green = "#f7ac7a"
cyan = "#ffc569"
blue = "#ac6380"
magenta = "#e98897"
"#;

    fn aether_theme() -> Theme {
        let mut map = parse_colors(AETHER);
        resolve(&mut map, false);
        theme_from_colors(&map, "Omarchy")
    }

    #[test]
    fn maps_core_fields_from_the_palette() {
        let t = aether_theme();
        assert_eq!(t.name, "Omarchy");
        assert_eq!(t.accent, Color::Rgb(0xac, 0x63, 0x80));
        assert_eq!(t.fg, Color::Rgb(0xE7, 0xE6, 0xE5));
        assert_eq!(t.fg_muted, Color::Rgb(0x68, 0x61, 0x63));
        assert_eq!(t.bg_selected, Color::Rgb(0x1a, 0x1a, 0x1a));
        assert_eq!(t.error, Color::Rgb(0xc4, 0x7d, 0x75));
    }

    #[test]
    fn bg_stays_reset_to_preserve_terminal_transparency() {
        assert_eq!(aether_theme().bg, Color::Reset);
    }

    #[test]
    fn accent_falls_back_when_the_theme_omits_it() {
        // Omarchy's own resolver has no fallback for `accent`, so ours must.
        let mut map = parse_colors("background = \"#000000\"\nblue = \"#89b4fa\"\n");
        resolve(&mut map, false);
        let t = theme_from_colors(&map, "Omarchy");
        assert_eq!(t.accent, Color::Rgb(0x89, 0xb4, 0xfa));
    }

    #[test]
    fn statusbar_is_legible_in_both_modes() {
        let dark = aether_theme();
        assert_ne!(dark.statusbar_bg, dark.statusbar_fg);

        // A light theme inverts which of fg/background is the lighter colour.
        // darker_background resolves to a mid-grey here, so the legible choice
        // is the DARK foreground -- not the white background.
        let mut map = parse_colors("background = \"#ffffff\"\nforeground = \"#1a1a1a\"\n");
        resolve(&mut map, false);
        let light = theme_from_colors(&map, "Omarchy");
        assert_ne!(light.statusbar_bg, light.statusbar_fg);
        assert_eq!(
            light.statusbar_fg,
            Color::Rgb(0x1a, 0x1a, 0x1a),
            "a light theme must not get its own background as status-bar text"
        );
    }

    #[test]
    fn sender_palette_is_filled_with_eight_distinct_hues() {
        let t = aether_theme();
        assert_eq!(t.sender_palette.len(), 8);
        assert!(t.sender_palette.iter().all(|c| *c != Color::Reset));
    }

    #[test]
    fn an_empty_palette_still_produces_a_usable_theme() {
        let map = HashMap::new();
        let t = theme_from_colors(&map, "Omarchy");
        assert_eq!(t.name, "Omarchy");
        assert_eq!(t.bg, Color::Reset);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test theme::omarchy`
Expected: FAIL to compile — `cannot find function 'theme_from_colors'`.

- [ ] **Step 3: Write the implementation**

Add to `src/theme/omarchy.rs`:

```rust
use super::Theme;
use ratatui::style::Color;

/// Look up `key`, falling back through `alts`, and parse as a colour.
/// Anything unparseable yields `None` so the caller's default applies.
fn color(map: &HashMap<String, String>, key: &str, alts: &[&str]) -> Option<Color> {
    std::iter::once(key)
        .chain(alts.iter().copied())
        .find_map(|k| map.get(k))
        .and_then(|v| super::string_to_color(v).ok())
}

/// Relative luminance of a hex colour, 0.0-1.0. Used only to choose a
/// readable foreground; not a colour-science-grade figure.
fn luminance(c: Color) -> f64 {
    match c {
        Color::Rgb(r, g, b) => {
            (0.2126 * r as f64 + 0.7152 * g as f64 + 0.0722 * b as f64) / 255.0
        }
        _ => 0.5,
    }
}

/// Pick whichever of `a` / `b` contrasts more strongly against `bg`.
///
/// Deliberately makes no assumption about which argument is the lighter one:
/// in a dark theme the foreground is light and the background dark, and in a
/// light theme it is the other way round. Comparing luminance distance works
/// in both without a mode branch.
fn readable_on(bg: Color, a: Color, b: Color) -> Color {
    let d = |c: Color| (luminance(c) - luminance(bg)).abs();
    if d(a) >= d(b) { a } else { b }
}

/// Build a siggy [`Theme`] from a resolved Omarchy palette.
///
/// Every field falls back to the corresponding value in
/// [`super::default_theme`] when the palette cannot supply it, so a sparse or
/// broken `colors.toml` degrades instead of failing.
pub(crate) fn theme_from_colors(map: &HashMap<String, String>, name: &str) -> Theme {
    let d = super::default_theme();
    let get = |k: &str, alts: &[&str], fallback: Color| color(map, k, alts).unwrap_or(fallback);

    let fg = get("foreground", &[], d.fg);
    let accent = get("accent", &["blue", "foreground"], d.accent);
    let muted = get("muted", &["dark_foreground"], d.fg_muted);
    let selection = get("selection", &["lighter_background"], d.bg_selected);
    let statusbar_bg = get("darker_background", &["selection"], d.statusbar_bg);
    let background = get("background", &[], Color::Black);

    Theme {
        name: name.to_string(),

        // Reset, not `background`: painting an opaque bg would defeat the
        // terminal transparency/blur that Omarchy users commonly run.
        bg: Color::Reset,
        bg_selected: selection,
        fg,
        fg_secondary: get("dark_foreground", &["muted"], d.fg_secondary),
        fg_muted: muted,

        accent,
        accent_secondary: get("magenta", &["bright_magenta"], d.accent_secondary),

        success: get("green", &[], d.success),
        error: get("red", &[], d.error),
        warning: get("yellow", &["orange"], d.warning),

        sender_self: get("green", &[], d.sender_self),
        sender_palette: [
            get("cyan", &[], d.sender_palette[0]),
            get("magenta", &[], d.sender_palette[1]),
            get("yellow", &[], d.sender_palette[2]),
            get("blue", &[], d.sender_palette[3]),
            get("bright_red", &["red"], d.sender_palette[4]),
            get("bright_green", &["green"], d.sender_palette[5]),
            get("bright_cyan", &["cyan"], d.sender_palette[6]),
            get("bright_magenta", &["magenta"], d.sender_palette[7]),
        ],
        link: get("blue", &["accent"], d.link),
        mention: accent,
        quote: muted,
        system_msg: muted,
        msg_selected_bg: get("lighter_background", &["selection"], d.msg_selected_bg),

        input_insert: accent,
        input_normal: get("yellow", &["orange"], d.input_normal),

        statusbar_bg,
        statusbar_fg: readable_on(statusbar_bg, fg, background),

        receipt_failed: get("red", &[], d.receipt_failed),
        receipt_sending: muted,
        receipt_sent: get("dark_foreground", &["muted"], d.receipt_sent),
        receipt_delivered: fg,
        receipt_read: accent,
        receipt_viewed: get("magenta", &["accent"], d.receipt_viewed),
    }
}
```

`string_to_color` is currently a private free function in `src/theme/mod.rs`. Change its signature to `pub(crate) fn string_to_color(...)` so the child module can use it. Its behaviour is unchanged, and its existing `#487` regression tests continue to cover it.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test theme::omarchy`
Expected: PASS, 17 tests.

- [ ] **Step 5: Commit**

```bash
git add src/theme/omarchy.rs src/theme/mod.rs
git commit -m "feat(theme): map a resolved Omarchy palette onto siggy's Theme (#697)"
```

---

## Task 4: Discovery, integration, and the config default

This is the task that delivers "install, open, it matches".

**Files:**
- Modify: `src/theme/omarchy.rs`
- Modify: `src/theme/mod.rs`
- Modify: `src/config.rs:250`

**Interfaces:**
- Consumes: `theme_from_colors`, `parse_colors`, `resolve`
- Produces:
  - `pub const THEME_NAME: &str = "Omarchy";`
  - `pub fn theme_dir() -> Option<PathBuf>` — the current-theme directory if Omarchy is present
  - `pub fn current_theme() -> Option<Theme>` — the fully derived theme, or `None` when Omarchy is absent
  - `pub fn theme_name_path() -> Option<PathBuf>` — the `theme.name` file, for the mtime poll in Task 5

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module:

```rust
    #[test]
    fn theme_dir_prefers_the_xdg_state_path() {
        let tmp = std::env::temp_dir().join(format!("siggy-omarchy-{}", std::process::id()));
        let theme = tmp.join("omarchy/current/theme");
        std::fs::create_dir_all(&theme).unwrap();
        std::fs::write(theme.join("colors.toml"), "background = \"#000000\"\n").unwrap();

        let found = theme_dir_in(Some(tmp.clone()), None);
        assert_eq!(found.as_deref(), Some(theme.as_path()));

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn theme_dir_is_none_without_a_colors_file() {
        let tmp = std::env::temp_dir().join(format!("siggy-omarchy-empty-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        assert_eq!(theme_dir_in(Some(tmp.clone()), None), None);
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn a_theme_supplied_siggy_toml_overrides_the_derived_mapping() {
        let tmp = std::env::temp_dir().join(format!("siggy-omarchy-ovr-{}", std::process::id()));
        let theme = tmp.join("omarchy/current/theme");
        std::fs::create_dir_all(&theme).unwrap();
        std::fs::write(theme.join("colors.toml"), AETHER).unwrap();

        let template = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/themes/custom-theme-template.toml"
        ))
        .unwrap();
        std::fs::write(theme.join("siggy.toml"), template).unwrap();

        let t = current_theme_in(Some(tmp.clone()), None).unwrap();
        // The template names itself "My Theme"; we rename it so the picker and
        // the saved config key stay stable.
        assert_eq!(t.name, THEME_NAME);
        // ...but its colours, not the derived ones, are what we got.
        assert_ne!(t.accent, Color::Rgb(0xac, 0x63, 0x80));

        std::fs::remove_dir_all(&tmp).ok();
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test theme::omarchy`
Expected: FAIL to compile — `cannot find function 'theme_dir_in'`, `'current_theme_in'`, `THEME_NAME`.

- [ ] **Step 3: Write the implementation**

Add to `src/theme/omarchy.rs`:

```rust
use std::path::{Path, PathBuf};

/// The name the derived theme carries in the picker and in `config.toml`.
pub const THEME_NAME: &str = "Omarchy";

/// Locate the current-theme directory, given explicit roots. Split out from
/// [`theme_dir`] so tests can drive it without touching the real HOME.
///
/// `state_root` is `$XDG_STATE_HOME` (or `~/.local/state`); `config_root` is
/// `~/.config`, checked second for Omarchy installs predating v4, which kept
/// the current theme under `~/.config/omarchy/current/`.
fn theme_dir_in(state_root: Option<PathBuf>, config_root: Option<PathBuf>) -> Option<PathBuf> {
    [state_root, config_root]
        .into_iter()
        .flatten()
        .map(|root| root.join("omarchy").join("current").join("theme"))
        .find(|dir| dir.join("colors.toml").is_file())
}

/// The current Omarchy theme directory, or `None` when Omarchy is not present.
///
/// Detection is by file, not by `$OMARCHY_PATH`: the env var is exported into
/// the desktop session but is absent over ssh and in a bare tty, where the
/// files are still perfectly readable.
pub fn theme_dir() -> Option<PathBuf> {
    theme_dir_in(dirs::state_dir(), dirs::config_dir())
}

/// Path to Omarchy's `theme.name`, whose mtime is the change signal (Task 5).
pub fn theme_name_path() -> Option<PathBuf> {
    let dir = theme_dir()?;
    Some(dir.parent()?.join("theme.name"))
}

/// Build the theme from a known theme directory.
fn theme_from_dir(dir: &Path) -> Option<Theme> {
    // A theme may ship a hand-tuned siggy.toml; it wins over our mapping.
    // Rename it to THEME_NAME so the picker entry and the persisted config
    // key stay stable whatever the file calls itself.
    let override_path = dir.join("siggy.toml");
    if let Ok(contents) = std::fs::read_to_string(&override_path) {
        match toml::from_str::<Theme>(&contents) {
            Ok(mut t) => {
                t.name = THEME_NAME.to_string();
                return Some(t);
            }
            Err(e) => {
                crate::debug_log::logf(format_args!(
                    "omarchy siggy.toml parse error {}: {e}",
                    override_path.display()
                ));
            }
        }
    }

    let contents = std::fs::read_to_string(dir.join("colors.toml")).ok()?;
    let mut map = parse_colors(&contents);
    resolve(&mut map, dir.join("light.mode").is_file());
    Some(theme_from_colors(&map, THEME_NAME))
}

fn current_theme_in(state_root: Option<PathBuf>, config_root: Option<PathBuf>) -> Option<Theme> {
    theme_from_dir(&theme_dir_in(state_root, config_root)?)
}

/// The active Omarchy theme mapped onto a siggy [`Theme`], or `None` when
/// Omarchy is not installed. Re-reads from disk on every call, which is what
/// makes the reload path in `main.rs` a one-liner.
pub fn current_theme() -> Option<Theme> {
    theme_from_dir(&theme_dir()?)
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test theme::omarchy`
Expected: PASS, 20 tests.

- [ ] **Step 5: Write the failing integration tests**

Add to the `tests` module in `src/theme/mod.rs`:

```rust
    #[test]
    fn omarchy_theme_appears_in_all_themes_only_when_present() {
        let listed = all_themes().iter().any(|t| t.name == omarchy::THEME_NAME);
        assert_eq!(listed, omarchy::theme_dir().is_some());
    }

    #[test]
    fn find_theme_resolves_omarchy_when_present_and_falls_back_otherwise() {
        let t = find_theme(omarchy::THEME_NAME);
        if omarchy::theme_dir().is_some() {
            assert_eq!(t.name, omarchy::THEME_NAME);
        } else {
            assert_eq!(t.name, "Default");
        }
    }
```

And in `src/config.rs`'s test module:

```rust
    #[test]
    fn a_config_with_an_explicit_theme_is_not_overridden_by_the_new_default() {
        // Guards the promise that switching default_theme() to "Omarchy"
        // cannot change any existing user's theme: serde's default only fires
        // when the key is absent, and save() always writes the key.
        let cfg: Config = toml::from_str("theme = \"Nord\"\n").unwrap();
        assert_eq!(cfg.theme, "Nord");
    }

    #[test]
    fn a_config_without_a_theme_key_defaults_to_omarchy() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.theme, "Omarchy");
    }
```

- [ ] **Step 6: Run them to verify they fail**

Run: `cargo test theme:: && cargo test config::`
Expected: FAIL — `all_themes` does not yet include the Omarchy theme; `default_theme()` still returns `"Default"`.

- [ ] **Step 7: Wire it up**

In `src/theme/mod.rs`, replace the body of `all_themes()`:

```rust
/// All available themes: built-ins, the Omarchy theme when the desktop is
/// present, then custom themes.
pub fn all_themes() -> Vec<Theme> {
    let mut themes = builtin_themes();
    themes.extend(omarchy::current_theme());
    themes.extend(load_custom_themes());
    themes
}
```

`find_theme` needs no change: it already searches `all_themes()` and falls back to `default_theme()` for an unknown name, which is exactly the desired behaviour for `theme = "Omarchy"` on a non-Omarchy machine.

In `src/config.rs`, change `default_theme()`:

```rust
fn default_theme() -> String {
    // Fresh installs follow the desktop theme when one is detectable.
    // find_theme() falls back to the built-in Default when it is not, so this
    // is safe on every platform. Existing configs already carry an explicit
    // theme key and are unaffected -- serde defaults only fire on absence.
    crate::theme::omarchy::THEME_NAME.to_string()
}
```

- [ ] **Step 8: Run the full suite**

Run: `cargo test`
Expected: PASS. Note `all_builtin_themes_have_unique_names` also covers the new entry.

- [ ] **Step 9: Commit**

```bash
git add src/theme/ src/config.rs
git commit -m "feat(theme): follow the Omarchy desktop theme by default (#697)"
```

---

## Task 5: Follow theme changes without a restart

Two independent signals feed one reload. The mtime poll is the zero-touch path that works out of the box; SIGUSR1 makes it instant for users who install the hook.

We deliberately do **not** write into `~/.config/omarchy/hooks/` automatically. Silently installing files into another tool's config directory is presumptuous, and the poll already delivers the promised behaviour within 10 seconds. The hook is documented in the README as a one-line opt-in.

**Files:**
- Modify: `src/main.rs`

**Interfaces:**
- Consumes: `theme::omarchy::{THEME_NAME, current_theme, theme_name_path}`
- Produces: nothing consumed by later tasks

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `src/theme/mod.rs`:

```rust
    #[test]
    fn reload_only_replaces_the_theme_when_omarchy_is_active() {
        // Pinned themes must never be clobbered by a desktop theme change.
        let nord = find_theme("Nord");
        assert_eq!(maybe_reload_omarchy(&nord).map(|t| t.name), None);
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test theme::`
Expected: FAIL to compile — `cannot find function 'maybe_reload_omarchy'`.

- [ ] **Step 3: Implement the reload helper**

In `src/theme/mod.rs`:

```rust
/// Return a freshly-read Omarchy theme, but only if `current` is the Omarchy
/// theme. A user who pinned a specific theme keeps it across desktop theme
/// changes.
pub fn maybe_reload_omarchy(current: &Theme) -> Option<Theme> {
    if current.name != omarchy::THEME_NAME {
        return None;
    }
    omarchy::current_theme()
}
```

- [ ] **Step 4: Run it to verify it passes**

Run: `cargo test theme::`
Expected: PASS.

- [ ] **Step 5: Add the SIGUSR1 handler**

In `src/main.rs`, inside `run_app` before the event loop begins (near the other loop-local state around line 1650), add:

```rust
    // Omarchy signals a theme change the same way it does for helix and btop
    // (`pkill -USR1`). tokio's "full" feature already provides this; no new
    // dependency. Non-unix targets simply never fire.
    #[cfg(unix)]
    let mut theme_signal = {
        use tokio::signal::unix::{SignalKind, signal};
        signal(SignalKind::user_defined1()).ok()
    };

    // Fallback for users who have not installed the theme-set hook: notice a
    // theme change by the mtime of Omarchy's theme.name.
    let omarchy_name_path = theme::omarchy::theme_name_path();
    let mut omarchy_stamp = omarchy_name_path
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok());
    let mut last_theme_check = Instant::now();
```

- [ ] **Step 6: Fold the poll into the existing sweep**

Immediately after the existing expiry-sweep block in `src/main.rs` (currently at line 1818), add:

```rust
        // Follow Omarchy desktop theme changes (every 10s, alongside the
        // sweep above). A SIGUSR1 from the theme-set hook short-circuits this.
        let mut theme_changed = false;
        #[cfg(unix)]
        if let Some(sig) = theme_signal.as_mut() {
            // try_recv equivalent: poll without awaiting so the loop stays hot.
            if sig.try_recv().is_some() {
                theme_changed = true;
            }
        }
        if !theme_changed && last_theme_check.elapsed() >= Duration::from_secs(10) {
            last_theme_check = Instant::now();
            let stamp = omarchy_name_path
                .as_ref()
                .and_then(|p| std::fs::metadata(p).ok())
                .and_then(|m| m.modified().ok());
            if stamp != omarchy_stamp {
                omarchy_stamp = stamp;
                theme_changed = true;
            }
        }
        if theme_changed && let Some(t) = theme::maybe_reload_omarchy(&app.theme) {
            app.theme = t;
            app.theme_picker.available_themes = theme::all_themes();
            needs_redraw = true;
        }
```

`tokio::signal::unix::Signal` has no `try_recv`; if the compiler rejects the line above, use a `futures`-free poll instead:

```rust
        #[cfg(unix)]
        if let Some(sig) = theme_signal.as_mut() {
            // recv() is cancel-safe; poll it with a zero timeout so the event
            // loop never blocks on it.
            if tokio::time::timeout(Duration::ZERO, sig.recv()).await.is_ok() {
                theme_changed = true;
            }
        }
```

Use whichever compiles; prefer the `timeout` form if in doubt, and delete the other.

- [ ] **Step 7: Verify it builds and the suite passes**

Run: `cargo clippy --tests -- -D warnings && cargo test`
Expected: PASS with no warnings.

- [ ] **Step 8: Manual verification**

```bash
cargo run &
omarchy theme set tokyo-night
# within 10s, siggy's colours change with no restart
omarchy theme set guts-berserk-dark
```

Then test the instant path:

```bash
printf '#!/bin/bash\npkill -USR1 siggy\n' > ~/.config/omarchy/hooks/theme-set.d/siggy
chmod +x ~/.config/omarchy/hooks/theme-set.d/siggy
omarchy theme set tokyo-night
# siggy retints immediately
```

- [ ] **Step 9: Commit**

```bash
git add src/main.rs src/theme/mod.rs
git commit -m "feat(theme): follow Omarchy theme changes live via SIGUSR1 and mtime poll (#697)"
```

---

## Task 6: Verify against every stock theme, document, and ship

**Files:**
- Modify: `src/theme/omarchy.rs`
- Modify: `README.md`

- [ ] **Step 1: Write the corpus test**

The 22 stock themes in `/usr/share/omarchy/themes/` are the real-world corpus, including four light ones (`catppuccin-latte`, `flexoki-light`, `solitude`, `white`). The test must skip cleanly on machines without Omarchy so CI stays green.

Add to the `tests` module in `src/theme/omarchy.rs`:

```rust
    /// Every stock Omarchy theme must resolve to a complete, sane siggy theme.
    /// Skipped on machines without Omarchy installed, which includes CI.
    #[test]
    fn every_stock_theme_resolves() {
        let stock = Path::new("/usr/share/omarchy/themes");
        if !stock.is_dir() {
            eprintln!("skipping: Omarchy not installed");
            return;
        }
        let mut checked = 0;
        for entry in std::fs::read_dir(stock).unwrap().flatten() {
            let colors = entry.path().join("colors.toml");
            if !colors.is_file() {
                continue;
            }
            let contents = std::fs::read_to_string(&colors).unwrap();
            let mut map = parse_colors(&contents);
            assert!(!map.is_empty(), "{} parsed to nothing", colors.display());
            resolve(&mut map, entry.path().join("light.mode").is_file());

            for key in ["background", "foreground", "muted", "selection", "mode"] {
                assert!(map.contains_key(key), "{} missing {key}", colors.display());
            }

            let t = theme_from_colors(&map, THEME_NAME);
            assert_eq!(t.bg, Color::Reset);
            assert_ne!(t.statusbar_bg, t.statusbar_fg, "{}", colors.display());
            assert_ne!(t.fg, t.bg_selected, "{}", colors.display());
            assert!(t.sender_palette.iter().all(|c| *c != Color::Reset));
            checked += 1;
        }
        assert!(checked >= 20, "expected the full stock theme set, saw {checked}");
    }
```

- [ ] **Step 2: Run it**

Run: `cargo test theme::omarchy::tests::every_stock_theme_resolves -- --nocapture`
Expected: PASS, having checked 22 themes. If a light theme trips the `statusbar_bg != statusbar_fg` or `fg != bg_selected` assertion, adjust the fallbacks in `theme_from_colors` — that is the assertion earning its place, not a reason to weaken it.

- [ ] **Step 3: Verify under both backends**

```bash
cargo clippy --tests -- -D warnings
cargo test
cargo clippy --tests --no-default-features --features native-backend -- -D warnings
cargo test --no-default-features --features native-backend
```

Expected: all four PASS. Do **not** run `cargo build --all-features` — it fails by design.

- [ ] **Step 4: Document it**

Add to `README.md`, in the theming section:

```markdown
### Omarchy

On [Omarchy](https://omarchy.org), siggy follows your desktop theme out of the
box — a fresh install picks up whatever theme you are running, and switching
themes with `omarchy theme set` retints siggy within a few seconds without a
restart. Pick any other theme from `/theme` to pin it instead.

For instant retinting rather than within-10-seconds, install the theme-set hook:

    printf '#!/bin/bash\npkill -USR1 siggy\n' > ~/.config/omarchy/hooks/theme-set.d/siggy
    chmod +x ~/.config/omarchy/hooks/theme-set.d/siggy

Theme authors can override siggy's derived colours by shipping a `siggy.toml`
(in siggy's own theme format) alongside `colors.toml` in the theme directory.
```

- [ ] **Step 5: Commit, push, and open the PR**

```bash
git add src/theme/omarchy.rs README.md
git commit -m "test(theme): verify the Omarchy mapping across every stock theme (#697)"
git push -u origin feature/697-omarchy-theme
gh pr create --title "feat(theme): follow the Omarchy system theme by default (closes #697)" --body "..."
```

The PR body should state: no new dependencies, no new `App` field (ratchet untouched), verified under both backend feature sets, and that existing users' themes are provably unchanged.

---

## Self-Review Notes

**Spec coverage.** Every checkbox in issue #697 maps to a task: parsing and location (Task 1, 4), the resolver cascade including mode precedence and `colorN` aliases (Task 2), the field mapping and the theme-supplied `siggy.toml` override (Task 3), the config default and picker integration (Task 4), SIGUSR1 and the mtime poll (Task 5), the stock-theme corpus and both-backend verification (Task 6), README (Task 6).

**Two deliberate departures from the issue, to be reflected back into it:**

1. The issue proposed an `"Auto"` sentinel plus a separate `Auto (Omarchy — …)` picker entry. This plan uses a single theme named `"Omarchy"` instead. It is one concept rather than two, and it reuses `find_theme`, the picker, and `save_settings` with zero special-casing — `find_theme`'s existing unknown-name fallback already gives the correct non-Omarchy behaviour.
2. The issue listed auto-installing the `theme-set.d` hook on first run. This plan documents it as opt-in instead, because writing into another tool's config directory unprompted is not ours to do, and the 10s poll already satisfies the requirement.

**Type consistency.** `THEME_NAME` is used identically in `omarchy.rs`, `mod.rs`, and `config.rs`. `theme_from_colors(&map, name)` has the same signature at every call site. `theme_dir_in` / `current_theme_in` are the test-injectable forms of `theme_dir` / `current_theme`.

**Known loose end, resolved during execution rather than now:** Task 5 Step 6 offers two forms of the non-blocking signal poll because `tokio::signal::unix::Signal`'s exact non-async API surface should be confirmed against the pinned tokio rather than guessed. Whichever compiles is correct; delete the other.
