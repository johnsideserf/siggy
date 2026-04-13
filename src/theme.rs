use ratatui::style::Color;
use serde::{Deserialize, Serialize};

/// A complete color theme for the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Theme {
    pub name: String,

    // Base
    #[serde(with = "color_serde")]
    pub bg: Color,
    #[serde(with = "color_serde")]
    pub bg_selected: Color,
    #[serde(with = "color_serde")]
    pub fg: Color,
    #[serde(with = "color_serde")]
    pub fg_secondary: Color,
    #[serde(with = "color_serde")]
    pub fg_muted: Color,

    // Accent
    #[serde(with = "color_serde")]
    pub accent: Color,
    #[serde(with = "color_serde")]
    pub accent_secondary: Color,

    // Status indicators
    #[serde(with = "color_serde")]
    pub success: Color,
    #[serde(with = "color_serde")]
    pub error: Color,
    #[serde(with = "color_serde")]
    pub warning: Color,

    // Messages
    #[serde(with = "color_serde")]
    pub sender_self: Color,
    #[serde(with = "color_array_serde")]
    pub sender_palette: [Color; 8],
    #[serde(with = "color_serde")]
    pub link: Color,
    #[serde(with = "color_serde")]
    pub mention: Color,
    #[serde(with = "color_serde")]
    pub quote: Color,
    #[serde(with = "color_serde")]
    pub system_msg: Color,
    #[serde(with = "color_serde")]
    pub msg_selected_bg: Color,

    // Input box
    #[serde(with = "color_serde")]
    pub input_insert: Color,
    #[serde(with = "color_serde")]
    pub input_normal: Color,

    // Status bar
    #[serde(with = "color_serde")]
    pub statusbar_bg: Color,
    #[serde(with = "color_serde")]
    pub statusbar_fg: Color,

    // Receipt status colors
    #[serde(with = "color_serde")]
    pub receipt_failed: Color,
    #[serde(with = "color_serde")]
    pub receipt_sending: Color,
    #[serde(with = "color_serde")]
    pub receipt_sent: Color,
    #[serde(with = "color_serde")]
    pub receipt_delivered: Color,
    #[serde(with = "color_serde")]
    pub receipt_read: Color,
    #[serde(with = "color_serde")]
    pub receipt_viewed: Color,
}

// ---------------------------------------------------------------------------
// Built-in themes
// ---------------------------------------------------------------------------

/// The default theme matching the original hardcoded colors.
/// Uses named 16-color values so it adapts to the terminal palette.
pub fn default_theme() -> Theme {
    Theme {
        name: "Default".into(),
        bg: Color::Black,
        bg_selected: Color::DarkGray,
        fg: Color::White,
        fg_secondary: Color::Gray,
        fg_muted: Color::DarkGray,
        accent: Color::Cyan,
        accent_secondary: Color::Yellow,
        success: Color::Green,
        error: Color::Red,
        warning: Color::Yellow,
        sender_self: Color::Green,
        sender_palette: [
            Color::Cyan,
            Color::Magenta,
            Color::Yellow,
            Color::Blue,
            Color::LightRed,
            Color::LightGreen,
            Color::LightCyan,
            Color::LightMagenta,
        ],
        link: Color::Blue,
        mention: Color::Cyan,
        quote: Color::DarkGray,
        system_msg: Color::DarkGray,
        msg_selected_bg: Color::Indexed(236),
        input_insert: Color::Cyan,
        input_normal: Color::Yellow,
        statusbar_bg: Color::DarkGray,
        statusbar_fg: Color::White,
        receipt_failed: Color::Red,
        receipt_sending: Color::DarkGray,
        receipt_sent: Color::DarkGray,
        receipt_delivered: Color::White,
        receipt_read: Color::Green,
        receipt_viewed: Color::Cyan,
    }
}

fn catppuccin_mocha() -> Theme {
    Theme {
        name: "Catppuccin Mocha".into(),
        bg: Color::Rgb(30, 30, 46),                  // base
        bg_selected: Color::Rgb(69, 71, 90),         // surface1
        fg: Color::Rgb(205, 214, 244),               // text
        fg_secondary: Color::Rgb(166, 173, 200),     // subtext0
        fg_muted: Color::Rgb(108, 112, 134),         // overlay0
        accent: Color::Rgb(203, 166, 247),           // mauve
        accent_secondary: Color::Rgb(249, 226, 175), // yellow
        success: Color::Rgb(166, 227, 161),          // green
        error: Color::Rgb(243, 139, 168),            // red
        warning: Color::Rgb(249, 226, 175),          // yellow
        sender_self: Color::Rgb(166, 227, 161),      // green
        sender_palette: [
            Color::Rgb(137, 180, 250), // blue
            Color::Rgb(245, 194, 231), // pink
            Color::Rgb(249, 226, 175), // yellow
            Color::Rgb(116, 199, 236), // sapphire
            Color::Rgb(243, 139, 168), // red
            Color::Rgb(166, 227, 161), // green
            Color::Rgb(148, 226, 213), // teal
            Color::Rgb(203, 166, 247), // mauve
        ],
        link: Color::Rgb(137, 180, 250),         // blue
        mention: Color::Rgb(203, 166, 247),      // mauve
        quote: Color::Rgb(108, 112, 134),        // overlay0
        system_msg: Color::Rgb(108, 112, 134),   // overlay0
        msg_selected_bg: Color::Rgb(49, 50, 68), // surface0
        input_insert: Color::Rgb(203, 166, 247), // mauve
        input_normal: Color::Rgb(249, 226, 175), // yellow
        statusbar_bg: Color::Rgb(24, 24, 37),    // mantle
        statusbar_fg: Color::Rgb(205, 214, 244), // text
        receipt_failed: Color::Rgb(243, 139, 168),
        receipt_sending: Color::Rgb(108, 112, 134),
        receipt_sent: Color::Rgb(108, 112, 134),
        receipt_delivered: Color::Rgb(205, 214, 244),
        receipt_read: Color::Rgb(166, 227, 161),
        receipt_viewed: Color::Rgb(137, 180, 250),
    }
}

fn catppuccin_latte() -> Theme {
    Theme {
        name: "Catppuccin Latte".into(),
        bg: Color::Rgb(239, 241, 245),              // base
        bg_selected: Color::Rgb(188, 192, 204),     // surface1
        fg: Color::Rgb(76, 79, 105),                // text
        fg_secondary: Color::Rgb(108, 111, 133),    // subtext0
        fg_muted: Color::Rgb(140, 143, 161),        // overlay0
        accent: Color::Rgb(136, 57, 239),           // mauve
        accent_secondary: Color::Rgb(223, 142, 29), // yellow
        success: Color::Rgb(64, 160, 43),           // green
        error: Color::Rgb(210, 15, 57),             // red
        warning: Color::Rgb(223, 142, 29),          // yellow
        sender_self: Color::Rgb(64, 160, 43),       // green
        sender_palette: [
            Color::Rgb(30, 102, 245),  // blue
            Color::Rgb(234, 118, 203), // pink
            Color::Rgb(223, 142, 29),  // yellow
            Color::Rgb(32, 159, 181),  // sapphire
            Color::Rgb(210, 15, 57),   // red
            Color::Rgb(64, 160, 43),   // green
            Color::Rgb(23, 146, 153),  // teal
            Color::Rgb(136, 57, 239),  // mauve
        ],
        link: Color::Rgb(30, 102, 245),             // blue
        mention: Color::Rgb(136, 57, 239),          // mauve
        quote: Color::Rgb(140, 143, 161),           // overlay0
        system_msg: Color::Rgb(140, 143, 161),      // overlay0
        msg_selected_bg: Color::Rgb(204, 208, 218), // surface0
        input_insert: Color::Rgb(136, 57, 239),     // mauve
        input_normal: Color::Rgb(223, 142, 29),     // yellow
        statusbar_bg: Color::Rgb(230, 233, 239),    // mantle
        statusbar_fg: Color::Rgb(76, 79, 105),      // text
        receipt_failed: Color::Rgb(210, 15, 57),
        receipt_sending: Color::Rgb(140, 143, 161),
        receipt_sent: Color::Rgb(140, 143, 161),
        receipt_delivered: Color::Rgb(76, 79, 105),
        receipt_read: Color::Rgb(64, 160, 43),
        receipt_viewed: Color::Rgb(30, 102, 245),
    }
}

fn dracula() -> Theme {
    Theme {
        name: "Dracula".into(),
        bg: Color::Rgb(40, 42, 54),                  // background
        bg_selected: Color::Rgb(68, 71, 90),         // current line
        fg: Color::Rgb(248, 248, 242),               // foreground
        fg_secondary: Color::Rgb(189, 147, 249),     // purple (secondary info)
        fg_muted: Color::Rgb(98, 114, 164),          // comment
        accent: Color::Rgb(189, 147, 249),           // purple
        accent_secondary: Color::Rgb(241, 250, 140), // yellow
        success: Color::Rgb(80, 250, 123),           // green
        error: Color::Rgb(255, 85, 85),              // red
        warning: Color::Rgb(241, 250, 140),          // yellow
        sender_self: Color::Rgb(80, 250, 123),       // green
        sender_palette: [
            Color::Rgb(139, 233, 253), // cyan
            Color::Rgb(255, 121, 198), // pink
            Color::Rgb(241, 250, 140), // yellow
            Color::Rgb(189, 147, 249), // purple
            Color::Rgb(255, 85, 85),   // red
            Color::Rgb(80, 250, 123),  // green
            Color::Rgb(255, 184, 108), // orange
            Color::Rgb(139, 233, 253), // cyan (alt)
        ],
        link: Color::Rgb(139, 233, 253),      // cyan
        mention: Color::Rgb(255, 121, 198),   // pink
        quote: Color::Rgb(98, 114, 164),      // comment
        system_msg: Color::Rgb(98, 114, 164), // comment
        msg_selected_bg: Color::Rgb(55, 57, 69),
        input_insert: Color::Rgb(189, 147, 249), // purple
        input_normal: Color::Rgb(241, 250, 140), // yellow
        statusbar_bg: Color::Rgb(33, 34, 44),
        statusbar_fg: Color::Rgb(248, 248, 242),
        receipt_failed: Color::Rgb(255, 85, 85),
        receipt_sending: Color::Rgb(98, 114, 164),
        receipt_sent: Color::Rgb(98, 114, 164),
        receipt_delivered: Color::Rgb(248, 248, 242),
        receipt_read: Color::Rgb(80, 250, 123),
        receipt_viewed: Color::Rgb(139, 233, 253),
    }
}

fn nord() -> Theme {
    Theme {
        name: "Nord".into(),
        bg: Color::Rgb(46, 52, 64),                  // nord0 polar night
        bg_selected: Color::Rgb(67, 76, 94),         // nord2
        fg: Color::Rgb(236, 239, 244),               // nord6 snow storm
        fg_secondary: Color::Rgb(216, 222, 233),     // nord4
        fg_muted: Color::Rgb(76, 86, 106),           // nord3
        accent: Color::Rgb(136, 192, 208),           // nord8 frost
        accent_secondary: Color::Rgb(235, 203, 139), // nord13 aurora yellow
        success: Color::Rgb(163, 190, 140),          // nord14 aurora green
        error: Color::Rgb(191, 97, 106),             // nord11 aurora red
        warning: Color::Rgb(235, 203, 139),          // nord13 aurora yellow
        sender_self: Color::Rgb(163, 190, 140),      // nord14
        sender_palette: [
            Color::Rgb(136, 192, 208), // nord8
            Color::Rgb(180, 142, 173), // nord15 purple
            Color::Rgb(235, 203, 139), // nord13
            Color::Rgb(129, 161, 193), // nord9
            Color::Rgb(191, 97, 106),  // nord11
            Color::Rgb(163, 190, 140), // nord14
            Color::Rgb(143, 188, 187), // nord7
            Color::Rgb(208, 135, 112), // nord12 orange
        ],
        link: Color::Rgb(129, 161, 193),         // nord9
        mention: Color::Rgb(180, 142, 173),      // nord15
        quote: Color::Rgb(76, 86, 106),          // nord3
        system_msg: Color::Rgb(76, 86, 106),     // nord3
        msg_selected_bg: Color::Rgb(59, 66, 82), // nord1
        input_insert: Color::Rgb(136, 192, 208), // nord8
        input_normal: Color::Rgb(235, 203, 139), // nord13
        statusbar_bg: Color::Rgb(59, 66, 82),    // nord1
        statusbar_fg: Color::Rgb(236, 239, 244), // nord6
        receipt_failed: Color::Rgb(191, 97, 106),
        receipt_sending: Color::Rgb(76, 86, 106),
        receipt_sent: Color::Rgb(76, 86, 106),
        receipt_delivered: Color::Rgb(236, 239, 244),
        receipt_read: Color::Rgb(163, 190, 140),
        receipt_viewed: Color::Rgb(136, 192, 208),
    }
}

fn gruvbox_dark() -> Theme {
    Theme {
        name: "Gruvbox Dark".into(),
        bg: Color::Rgb(40, 40, 40),                 // bg
        bg_selected: Color::Rgb(80, 73, 69),        // bg2
        fg: Color::Rgb(235, 219, 178),              // fg
        fg_secondary: Color::Rgb(189, 174, 147),    // fg3
        fg_muted: Color::Rgb(124, 111, 100),        // bg4
        accent: Color::Rgb(254, 128, 25),           // orange
        accent_secondary: Color::Rgb(250, 189, 47), // yellow
        success: Color::Rgb(184, 187, 38),          // green
        error: Color::Rgb(251, 73, 52),             // red
        warning: Color::Rgb(250, 189, 47),          // yellow
        sender_self: Color::Rgb(184, 187, 38),      // green
        sender_palette: [
            Color::Rgb(131, 165, 152), // aqua
            Color::Rgb(211, 134, 155), // purple
            Color::Rgb(250, 189, 47),  // yellow
            Color::Rgb(69, 133, 136),  // dark aqua
            Color::Rgb(251, 73, 52),   // red
            Color::Rgb(184, 187, 38),  // green
            Color::Rgb(254, 128, 25),  // orange
            Color::Rgb(142, 192, 124), // bright green
        ],
        link: Color::Rgb(131, 165, 152),         // aqua
        mention: Color::Rgb(211, 134, 155),      // purple
        quote: Color::Rgb(124, 111, 100),        // bg4
        system_msg: Color::Rgb(124, 111, 100),   // bg4
        msg_selected_bg: Color::Rgb(60, 56, 54), // bg1
        input_insert: Color::Rgb(254, 128, 25),  // orange
        input_normal: Color::Rgb(250, 189, 47),  // yellow
        statusbar_bg: Color::Rgb(50, 48, 47),    // bg0_h
        statusbar_fg: Color::Rgb(235, 219, 178), // fg
        receipt_failed: Color::Rgb(251, 73, 52),
        receipt_sending: Color::Rgb(124, 111, 100),
        receipt_sent: Color::Rgb(124, 111, 100),
        receipt_delivered: Color::Rgb(235, 219, 178),
        receipt_read: Color::Rgb(184, 187, 38),
        receipt_viewed: Color::Rgb(131, 165, 152),
    }
}

fn mirc_dark() -> Theme {
    Theme {
        name: "mIRC Dark".into(),
        bg: Color::Black,
        bg_selected: Color::DarkGray,
        fg: Color::White,
        fg_secondary: Color::Gray,
        fg_muted: Color::DarkGray,
        accent: Color::LightGreen,
        accent_secondary: Color::LightYellow,
        success: Color::LightGreen,
        error: Color::LightRed,
        warning: Color::LightYellow,
        sender_self: Color::LightGreen,
        sender_palette: [
            Color::LightCyan,
            Color::LightMagenta,
            Color::LightYellow,
            Color::LightBlue,
            Color::LightRed,
            Color::LightGreen,
            Color::Cyan,
            Color::Magenta,
        ],
        link: Color::LightBlue,
        mention: Color::LightCyan,
        quote: Color::DarkGray,
        system_msg: Color::DarkGray,
        msg_selected_bg: Color::Indexed(236),
        input_insert: Color::LightGreen,
        input_normal: Color::LightYellow,
        statusbar_bg: Color::DarkGray,
        statusbar_fg: Color::White,
        receipt_failed: Color::LightRed,
        receipt_sending: Color::DarkGray,
        receipt_sent: Color::DarkGray,
        receipt_delivered: Color::White,
        receipt_read: Color::LightGreen,
        receipt_viewed: Color::LightCyan,
    }
}

fn mirc_light() -> Theme {
    Theme {
        name: "mIRC Light".into(),
        bg: Color::White,
        bg_selected: Color::Gray,
        fg: Color::Black,
        fg_secondary: Color::DarkGray,
        fg_muted: Color::Gray,
        accent: Color::Green,
        accent_secondary: Color::Yellow,
        success: Color::Green,
        error: Color::Red,
        warning: Color::Yellow,
        sender_self: Color::Green,
        sender_palette: [
            Color::Cyan,
            Color::Magenta,
            Color::Blue,
            Color::Red,
            Color::Green,
            Color::Yellow,
            Color::DarkGray,
            Color::Blue,
        ],
        link: Color::Blue,
        mention: Color::Magenta,
        quote: Color::Gray,
        system_msg: Color::Gray,
        msg_selected_bg: Color::Indexed(254),
        input_insert: Color::Green,
        input_normal: Color::Yellow,
        statusbar_bg: Color::Gray,
        statusbar_fg: Color::Black,
        receipt_failed: Color::Red,
        receipt_sending: Color::Gray,
        receipt_sent: Color::Gray,
        receipt_delivered: Color::Black,
        receipt_read: Color::Green,
        receipt_viewed: Color::Cyan,
    }
}

// ---------------------------------------------------------------------------
// Theme discovery
// ---------------------------------------------------------------------------

fn builtin_themes() -> Vec<Theme> {
    vec![
        default_theme(),
        catppuccin_mocha(),
        catppuccin_latte(),
        dracula(),
        nord(),
        gruvbox_dark(),
        mirc_dark(),
        mirc_light(),
    ]
}

/// Load custom themes from `~/.config/siggy/themes/*.toml`.
pub fn load_custom_themes() -> Vec<Theme> {
    let dir = match dirs::config_dir() {
        Some(d) => d.join("siggy").join("themes"),
        None => return Vec::new(),
    };
    if !dir.is_dir() {
        return Vec::new();
    }
    let mut themes = Vec::new();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) => {
            crate::debug_log::logf(format_args!("custom themes dir read error: {e}"));
            return Vec::new();
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(contents) => match toml::from_str::<Theme>(&contents) {
                Ok(theme) => themes.push(theme),
                Err(e) => {
                    crate::debug_log::logf(format_args!(
                        "custom theme parse error {}: {e}",
                        path.display()
                    ));
                }
            },
            Err(e) => {
                crate::debug_log::logf(format_args!(
                    "custom theme read error {}: {e}",
                    path.display()
                ));
            }
        }
    }
    themes
}

/// All available themes: built-ins followed by custom themes.
pub fn all_themes() -> Vec<Theme> {
    let mut themes = builtin_themes();
    themes.extend(load_custom_themes());
    themes
}

/// Find a theme by name. Falls back to Default if not found.
pub fn find_theme(name: &str) -> Theme {
    all_themes()
        .into_iter()
        .find(|t| t.name == name)
        .unwrap_or_else(default_theme)
}

// ---------------------------------------------------------------------------
// Color serde helpers
// ---------------------------------------------------------------------------

mod color_serde {
    use ratatui::style::Color;
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(color: &Color, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&super::color_to_string(color))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Color, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        super::string_to_color(&s).map_err(serde::de::Error::custom)
    }
}

mod color_array_serde {
    use ratatui::style::Color;
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(colors: &[Color; 8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeSeq;
        let mut seq = serializer.serialize_seq(Some(8))?;
        for c in colors {
            seq.serialize_element(&super::color_to_string(c))?;
        }
        seq.end()
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<[Color; 8], D::Error>
    where
        D: Deserializer<'de>,
    {
        let strings: Vec<String> = Vec::deserialize(deserializer)?;
        if strings.len() != 8 {
            return Err(serde::de::Error::custom(format!(
                "expected 8 colors, got {}",
                strings.len()
            )));
        }
        let mut colors = [Color::Reset; 8];
        for (i, s) in strings.iter().enumerate() {
            colors[i] = super::string_to_color(s).map_err(serde::de::Error::custom)?;
        }
        Ok(colors)
    }
}

fn color_to_string(color: &Color) -> String {
    match color {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Indexed(i) => format!("indexed({i})"),
        Color::Reset => "reset".into(),
        Color::Black => "black".into(),
        Color::Red => "red".into(),
        Color::Green => "green".into(),
        Color::Yellow => "yellow".into(),
        Color::Blue => "blue".into(),
        Color::Magenta => "magenta".into(),
        Color::Cyan => "cyan".into(),
        Color::Gray => "gray".into(),
        Color::DarkGray => "dark_gray".into(),
        Color::LightRed => "light_red".into(),
        Color::LightGreen => "light_green".into(),
        Color::LightYellow => "light_yellow".into(),
        Color::LightBlue => "light_blue".into(),
        Color::LightMagenta => "light_magenta".into(),
        Color::LightCyan => "light_cyan".into(),
        Color::White => "white".into(),
    }
}

fn string_to_color(s: &str) -> Result<Color, String> {
    let s = s.trim();
    // Hex: #rrggbb
    if let Some(hex) = s.strip_prefix('#') {
        if hex.len() == 6 {
            let r = u8::from_str_radix(&hex[0..2], 16).map_err(|e| format!("bad hex red: {e}"))?;
            let g =
                u8::from_str_radix(&hex[2..4], 16).map_err(|e| format!("bad hex green: {e}"))?;
            let b = u8::from_str_radix(&hex[4..6], 16).map_err(|e| format!("bad hex blue: {e}"))?;
            return Ok(Color::Rgb(r, g, b));
        }
        return Err(format!("hex color must be 6 digits: {s}"));
    }
    // Indexed: indexed(N)
    if let Some(inner) = s.strip_prefix("indexed(").and_then(|s| s.strip_suffix(')')) {
        let i: u8 = inner.parse().map_err(|e| format!("bad index: {e}"))?;
        return Ok(Color::Indexed(i));
    }
    // Named
    match s.to_lowercase().as_str() {
        "reset" => Ok(Color::Reset),
        "black" => Ok(Color::Black),
        "red" => Ok(Color::Red),
        "green" => Ok(Color::Green),
        "yellow" => Ok(Color::Yellow),
        "blue" => Ok(Color::Blue),
        "magenta" => Ok(Color::Magenta),
        "cyan" => Ok(Color::Cyan),
        "gray" | "grey" => Ok(Color::Gray),
        "dark_gray" | "dark_grey" | "darkgray" | "darkgrey" => Ok(Color::DarkGray),
        "light_red" | "lightred" => Ok(Color::LightRed),
        "light_green" | "lightgreen" => Ok(Color::LightGreen),
        "light_yellow" | "lightyellow" => Ok(Color::LightYellow),
        "light_blue" | "lightblue" => Ok(Color::LightBlue),
        "light_magenta" | "lightmagenta" => Ok(Color::LightMagenta),
        "light_cyan" | "lightcyan" => Ok(Color::LightCyan),
        "white" => Ok(Color::White),
        _ => Err(format!("unknown color: {s}")),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[test]
    fn default_theme_has_correct_name() {
        assert_eq!(default_theme().name, "Default");
    }

    #[test]
    fn all_builtin_themes_have_unique_names() {
        let themes = all_themes();
        let mut names: Vec<&str> = themes.iter().map(|t| t.name.as_str()).collect();
        let len = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), len, "duplicate theme names found");
    }

    #[test]
    fn find_theme_returns_default_for_unknown() {
        let t = find_theme("nonexistent");
        assert_eq!(t.name, "Default");
    }

    #[rstest]
    #[case(Color::Rgb(205, 214, 244), "#cdd6f4")]
    #[case(Color::Cyan, "cyan")]
    #[case(Color::Indexed(236), "indexed(236)")]
    fn color_serde_roundtrip(#[case] color: Color, #[case] expected_str: &str) {
        let s = color_to_string(&color);
        assert_eq!(s, expected_str);
        let c = string_to_color(&s).unwrap();
        assert_eq!(c, color);
    }

    #[test]
    fn theme_toml_roundtrip() {
        let theme = default_theme();
        let toml_str = toml::to_string_pretty(&theme).unwrap();
        let parsed: Theme = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.name, theme.name);
        assert_eq!(parsed.bg, theme.bg);
        assert_eq!(parsed.sender_palette, theme.sender_palette);
        assert_eq!(parsed.receipt_viewed, theme.receipt_viewed);
    }
}
