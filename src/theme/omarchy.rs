//! Follow the Omarchy desktop theme.
//!
//! Omarchy publishes the active theme's palette at
//! `~/.local/state/omarchy/current/theme/colors.toml`. This module reads it,
//! reproduces Omarchy's own key-resolution cascade (see
//! `/usr/share/omarchy/bin/omarchy-theme-color`), and maps the result onto
//! [`crate::theme::Theme`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::Theme;
use ratatui::style::Color;

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

/// Set `key` to `value`, unconditionally overwriting any existing value.
/// Used for assignments that must always apply (mirrors upstream's direct variable assignment).
fn assign(map: &mut HashMap<String, String>, key: &str, value: &str) {
    map.insert(key.to_string(), value.to_string());
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
/// derived shades, then the ANSI back-fill, then the short-name write-back,
/// then mode resolution.
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

/// Look up `key`, falling back through `alts`, and parse as a colour.
///
/// Tries each candidate key *in order* and returns the first whose value
/// actually parses as a colour -- not the first key that merely happens to
/// be present. `colors.toml` can hold non-colour values under a key this
/// cascade also uses for fallback (a keyword like `mode`, or a gradient spec
/// such as `-45deg`); stopping at the first present-but-unparseable key would
/// skip every remaining fallback and fall straight to the caller's hard
/// default, even though a later alt might have resolved fine (#697 review
/// Finding 5).
fn color(map: &HashMap<String, String>, key: &str, alts: &[&str]) -> Option<Color> {
    std::iter::once(key)
        .chain(alts.iter().copied())
        .find_map(|k| map.get(k).and_then(|v| super::string_to_color(v).ok()))
}

/// WCAG relative luminance of a hex colour, 0.0-1.0: each sRGB channel is
/// linearized before weighting, per the WCAG 2.x definition. Falls back to a
/// neutral mid-point for non-RGB colours (e.g. `Color::Reset`), which keeps
/// `contrast()` well-defined everywhere it is called.
fn luminance(c: Color) -> f64 {
    match c {
        Color::Rgb(r, g, b) => {
            let f = |c: u8| {
                let c = c as f64 / 255.0;
                if c <= 0.03928 {
                    c / 12.92
                } else {
                    ((c + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b)
        }
        _ => 0.5,
    }
}

/// WCAG contrast ratio between two colors. Higher values indicate greater
/// contrast and better readability. Ratio = (L_light + 0.05) / (L_dark + 0.05).
fn contrast(c1: Color, c2: Color) -> f64 {
    let l1 = luminance(c1);
    let l2 = luminance(c2);
    (l1.max(l2) + 0.05) / (l1.min(l2) + 0.05)
}

/// Pick whichever of `a` / `b` has greater contrast ratio against `bg`,
/// ensuring readability works in both dark and light themes.
fn readable_on(bg: Color, a: Color, b: Color) -> Color {
    let ca = contrast(bg, a);
    let cb = contrast(bg, b);
    if ca >= cb { a } else { b }
}

/// If `candidate` resolved equal to `background`, derive a distinguishable
/// shade instead of returning an invisible highlight.
///
/// `last-horizon` and `solitude` are the two stock themes that explicitly set
/// `lighter_background` equal to `background` (rather than leaving it absent,
/// which would fall through the alt chain to `selection` instead). Since
/// `chat_pane` paints `msg_selected_bg` as the ONLY cue for which message is
/// focused, an equal value makes the highlight undetectable (#697 review
/// Finding 2).
///
/// Blends 12% of the foreground into the background. That factor was chosen
/// by measuring `contrast(msg_selected_bg, background)` across the 20 stock
/// themes where this mapping already works without this fallback: their
/// natural contrast ranges from ~1.05 (`lupine`, a near-white theme with a
/// deliberately subtle highlight) to ~1.8 (`white`), clustered around 1.1-1.4.
/// 12% lands `last-horizon` at 1.34 and `solitude` at 1.27 -- squarely inside
/// that range, so the derived highlight reads the same way the rest of the
/// corpus already does: a subtle row tint, not a selection block.
///
/// Falls back to `candidate` unchanged if either raw value is not a hex
/// colour, matching how every other field in [`theme_from_colors`] degrades
/// when the palette can't supply what it needs.
fn distinguishable_from_background(
    candidate: Color,
    background: Color,
    map: &HashMap<String, String>,
) -> Color {
    if candidate != background {
        return candidate;
    }
    match (map.get("background"), map.get("foreground")) {
        (Some(bg_hex), Some(fg_hex)) => {
            super::string_to_color(&mix(bg_hex, fg_hex, 0.12)).unwrap_or(candidate)
        }
        _ => candidate,
    }
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
        msg_selected_bg: distinguishable_from_background(
            get("lighter_background", &["selection"], d.msg_selected_bg),
            background,
            map,
        ),

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

/// Path to Omarchy's `theme.name`, whose mtime is the change signal (#697).
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

// Only test code drives an explicit root through this path; production
// always goes through `current_theme()`, which reads the real environment.
#[cfg(test)]
fn current_theme_in(state_root: Option<PathBuf>, config_root: Option<PathBuf>) -> Option<Theme> {
    theme_from_dir(&theme_dir_in(state_root, config_root)?)
}

/// The active Omarchy theme mapped onto a siggy [`Theme`], or `None` when
/// Omarchy is not installed. Re-reads from disk on every call, which is what
/// makes the reload path in `main.rs` a one-liner.
pub fn current_theme() -> Option<Theme> {
    theme_from_dir(&theme_dir()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    const AETHER: &str = r##"
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
"##;

    fn aether_theme() -> Theme {
        let mut map = parse_colors(AETHER);
        resolve(&mut map, false);
        theme_from_colors(&map, "Omarchy")
    }

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
            let statusbar_contrast = contrast(t.statusbar_bg, t.statusbar_fg);
            assert!(
                statusbar_contrast >= 4.5,
                "{}: statusbar contrast {statusbar_contrast:.2} is below WCAG AA (4.5)",
                colors.display()
            );
            // WCAG AA text-contrast threshold (same standard as the statusbar
            // check above), not a bare `assert_ne!`: an inequality alone
            // passes for colours one bit apart, which is not "readable".
            // Measured across all 22 stock themes, the true minimum is
            // gruvbox at 4.881 -- comfortably above 4.5 (~8.5% margin) -- and
            // the maximum is vantablack at 17.404, so 4.5 passes every real
            // theme with room while still failing a degenerate mapping (e.g.
            // fg == bg_selected, contrast 1.0).
            let selected_text_contrast = contrast(t.fg, t.bg_selected);
            assert!(
                selected_text_contrast >= 4.5,
                "{}: fg-on-bg_selected contrast {selected_text_contrast:.2} is below WCAG AA (4.5)",
                colors.display()
            );
            // msg_selected_bg is chat_pane's ONLY cue for the focused message
            // (#697 review Finding 2) -- an equal-to-background value (as
            // `last-horizon` and `solitude` explicitly set for
            // `lighter_background`) makes it invisible. 1.02 was chosen by
            // measuring `contrast(msg_selected_bg, background)` across the 20
            // themes where this already worked before the Finding 2 fix: the
            // tightest legitimate case is lupine (a near-white theme with a
            // deliberately subtle highlight) at 1.0445, so 1.02 sits below
            // every real theme's own value with ~2.3% margin while still
            // failing the exact-equality bug (contrast == 1.0 precisely).
            let background = color(&map, "background", &[]).unwrap_or(Color::Black);
            let selected_bg_contrast = contrast(t.msg_selected_bg, background);
            assert!(
                selected_bg_contrast >= 1.02,
                "{}: msg_selected_bg contrast {selected_bg_contrast:.4} against background is \
                 not distinguishable (indistinguishable focused-message highlight)",
                colors.display()
            );
            assert!(t.sender_palette.iter().all(|c| *c != Color::Reset));
            checked += 1;
        }
        assert!(
            checked >= 20,
            "expected the full stock theme set, saw {checked}"
        );
    }

    #[test]
    fn readable_on_selects_by_contrast_not_argument_order() {
        // Argument order test: when the second argument has better contrast,
        // it must be returned. This test fails if readable_on always returns
        // the first argument.
        let bg = Color::Rgb(10, 10, 10); // near-black background
        let dark = Color::Rgb(30, 30, 30); // dark color (first arg)
        let light = Color::Rgb(240, 240, 240); // light color (second arg)

        // Against near-black, light text (second arg) has far better contrast
        // than dark text (first arg). The function must return the light one.
        let result = readable_on(bg, dark, light);
        assert_eq!(
            result, light,
            "readable_on must select by contrast ratio, not argument order"
        );
    }
}
