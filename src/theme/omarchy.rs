//! Follow the Omarchy desktop theme.
//!
//! Omarchy publishes the active theme's palette at
//! `~/.local/state/omarchy/current/theme/colors.toml`. This module reads it,
//! reproduces Omarchy's own key-resolution cascade (see
//! `/usr/share/omarchy/bin/omarchy-theme-color`), and maps the result onto
//! [`crate::theme::Theme`].

use std::collections::HashMap;

/// Parse a `colors.toml` into a flat map of top-level string values.
///
/// Values are kept as strings rather than colours because not every value is
/// a colour: `mode` is a keyword, and some themes carry gradient specs like
/// `-45deg`. A document that does not parse yields an empty map -- the caller
/// then falls back to siggy's own default theme.
#[allow(dead_code)]
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

/// Parse `#rrggbb` into its three channels. Returns `None` for anything else,
/// which is how non-colour values (keywords, gradient specs) stay out of the
/// arithmetic.
#[allow(dead_code)]
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
#[allow(dead_code)]
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
#[allow(dead_code)]
fn alias(map: &mut HashMap<String, String>, key: &str, from: &str) {
    if map.contains_key(key) {
        return;
    }
    if let Some(v) = map.get(from).cloned() {
        map.insert(key.to_string(), v);
    }
}

/// Set `key` to the first present candidate, if `key` is absent.
#[allow(dead_code)]
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

/// Set `key` to `value`, unconditionally overwriting any existing value.
/// Used for assignments that must always apply (mirrors upstream's direct variable assignment).
#[allow(dead_code)]
fn assign(map: &mut HashMap<String, String>, key: &str, value: &str) {
    map.insert(key.to_string(), value.to_string());
}

/// Set `key` to `mix(base, toward, amount)` if `key` is absent and `base` resolves.
#[allow(dead_code)]
fn derive(map: &mut HashMap<String, String>, key: &str, base: &str, toward: &str, amount: f64) {
    if map.contains_key(key) {
        return;
    }
    if let Some(b) = map.get(base).cloned() {
        map.insert(key.to_string(), mix(&b, toward, amount));
    }
}

/// Resolve `mode`, mirroring `resolve_theme_mode` in omarchy-theme-color.
#[allow(dead_code)]
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
/// derived shades, then the ANSI back-fill, then the short-name write-back,
/// then mode resolution.
#[allow(dead_code)]
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
    // Fill background/foreground from colorN if absent; then unconditionally
    // overwrite colorN back with the resolved background/foreground.
    alias(map, "background", "color0");
    alias(map, "foreground", "color7");
    if let Some(bg) = map.get("background").cloned() {
        assign(map, "color0", &bg);
    }
    if let Some(fg) = map.get("foreground").cloned() {
        assign(map, "color7", &fg);
    }

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
    // Cursor is unconditionally set to bright_foreground (not an only-if-absent alias).
    if let Some(bf) = map.get("bright_foreground").cloned() {
        assign(map, "cursor", &bf);
    }
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

    // 7. Short-name write-back: unconditionally write canonical values back to
    // short names so consumers still using legacy names get the resolved colours.
    for (canonical, short) in SHORT {
        if let Some(v) = map.get(canonical).cloned() {
            assign(map, short, &v);
        }
    }

    resolve_mode(map, light_mode_file);
    // After mode is resolved, set legacy theme_type key for consumers still using it.
    if let Some(m) = map.get("mode").cloned() {
        assign(map, "theme_type", &m);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_colors_reads_string_values() {
        let src = r##"
# Generated by Aether for Omarchy v4.
mode = "dark"
accent = "#ac6380"
background = "#000000"
"##;
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
            r##"
color0 = "#1e1e2e"
color1 = "#f38ba8"
color2 = "#a6e3a1"
color3 = "#f9e2af"
color4 = "#89b4fa"
color5 = "#cba6f7"
color6 = "#94e2d5"
color7 = "#cdd6f4"
color8 = "#585b70"
"##,
        );
        assert_eq!(map["background"], "#1e1e2e");
        assert_eq!(map["foreground"], "#cdd6f4");
        assert_eq!(map["red"], "#f38ba8");
        assert_eq!(map["magenta"], "#cba6f7");
        assert_eq!(map["muted"], "#585b70");
        assert_eq!(map["dark_foreground"], "#585b70");
        // bright_* derived by mixing 20% white when absent.
        // bright_red = mix("#f38ba8", "#ffffff", 0.2):
        //   R: int(243*0.8 + 255*0.2 + 0.5) = int(245.9) = 245 = f5
        //   G: int(139*0.8 + 255*0.2 + 0.5) = int(162.7) = 162 = a2
        //   B: int(168*0.8 + 255*0.2 + 0.5) = int(185.9) = 185 = b9
        assert_eq!(map["bright_red"], "#f5a2b9");
        // darker_background derived by mixing 50% black when absent.
        // darker_background = mix("#1e1e2e", "#000000", 0.5):
        //   R: int(30*0.5 + 0*0.5 + 0.5) = int(15.5) = 15 = 0f
        //   G: int(30*0.5 + 0*0.5 + 0.5) = int(15.5) = 15 = 0f
        //   B: int(46*0.5 + 0*0.5 + 0.5) = int(23.5) = 23 = 17
        assert_eq!(map["darker_background"], "#0f0f17");
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

    #[test]
    fn short_names_and_theme_type_populated_after_resolve() {
        // After resolve(), short names (bg, fg, etc.) and theme_type must be populated.
        let map = resolved("background = \"#101010\"\nforeground = \"#f0f0f0\"\n");
        assert_eq!(map["bg"], "#101010");
        assert_eq!(map["fg"], "#f0f0f0");
        assert_eq!(map["theme_type"], map["mode"]);
    }

    #[test]
    fn conflicting_color0_forced_to_background() {
        // If a theme defines both background and a conflicting color0,
        // resolve() unconditionally overwrites color0 to match background.
        // This ensures lighter_background (which chains through color0) resolves correctly.
        let map = resolved("background = \"#101010\"\ncolor0 = \"#ffffff\"\n");
        // color0 is forced to background, not the theme's #ffffff
        assert_eq!(map["color0"], "#101010");
        // lighter_background uses color0, so it must also reflect the forced value
        assert_eq!(map["lighter_background"], "#101010");
    }

    #[test]
    fn cursor_unconditionally_set_to_bright_foreground() {
        // cursor must always be set to bright_foreground, even if theme defines cursor.
        let map = resolved("foreground = \"#ffffff\"\ncursor = \"#ff0000\"\n");
        // bright_foreground defaults to foreground when absent from theme
        assert_eq!(map["bright_foreground"], "#ffffff");
        // cursor is unconditionally overwritten, not kept as theme-supplied value
        assert_eq!(map["cursor"], "#ffffff");
    }
}
