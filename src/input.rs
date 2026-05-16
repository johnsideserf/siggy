//! Slash-command parsing.
//!
//! Translates the user's typed input buffer into an [`InputAction`] enum
//! consumed by the event loop. Slash commands (`/join`, `/part`, `/quit`,
//! `/sidebar`, `/help`, ...) live in [`COMMANDS`] alongside their
//! autocomplete metadata.

/// Metadata for a slash command (used for autocomplete + help)
pub struct CommandInfo {
    pub name: &'static str,
    pub alias: &'static str,
    pub args: &'static str,
    pub description: &'static str,
}

pub const COMMANDS: &[CommandInfo] = &[
    CommandInfo {
        name: "/join",
        alias: "/j",
        args: "<name>",
        description: "Switch to a conversation",
    },
    CommandInfo {
        name: "/part",
        alias: "/p",
        args: "",
        description: "Leave current conversation",
    },
    CommandInfo {
        name: "/delete",
        alias: "",
        args: "",
        description: "Delete current conversation",
    },
    CommandInfo {
        name: "/sidebar",
        alias: "/sb",
        args: "",
        description: "Toggle sidebar",
    },
    CommandInfo {
        name: "/bell",
        alias: "",
        args: "[type]",
        description: "Toggle notifications (direct/group)",
    },
    CommandInfo {
        name: "/mute",
        alias: "",
        args: "[duration]",
        description: "Mute/unmute (e.g. 1h, 8h, 1d, 1w)",
    },
    CommandInfo {
        name: "/block",
        alias: "",
        args: "",
        description: "Block current contact/group",
    },
    CommandInfo {
        name: "/unblock",
        alias: "",
        args: "",
        description: "Unblock current contact/group",
    },
    CommandInfo {
        name: "/attach",
        alias: "/a",
        args: "",
        description: "Attach a file",
    },
    CommandInfo {
        name: "/paste",
        alias: "/pa",
        args: "",
        description: "Paste from clipboard (text, image, or file)",
    },
    CommandInfo {
        name: "/search",
        alias: "/s",
        args: "<query>",
        description: "Search messages",
    },
    CommandInfo {
        name: "/contacts",
        alias: "/c",
        args: "",
        description: "Browse contacts",
    },
    CommandInfo {
        name: "/settings",
        alias: "",
        args: "",
        description: "Open settings",
    },
    CommandInfo {
        name: "/disappearing",
        alias: "/dm",
        args: "<duration>",
        description: "Set disappearing timer (off/30s/5m/1h/1d/1w)",
    },
    CommandInfo {
        name: "/group",
        alias: "/g",
        args: "",
        description: "Group management",
    },
    CommandInfo {
        name: "/theme",
        alias: "/t",
        args: "",
        description: "Change color theme",
    },
    CommandInfo {
        name: "/poll",
        alias: "",
        args: "\"question\" \"opt1\" \"opt2\" [--single]",
        description: "Create a poll",
    },
    CommandInfo {
        name: "/verify",
        alias: "/v",
        args: "",
        description: "Verify contact identity",
    },
    CommandInfo {
        name: "/profile",
        alias: "",
        args: "",
        description: "Edit your Signal profile",
    },
    CommandInfo {
        name: "/about",
        alias: "",
        args: "",
        description: "About siggy",
    },
    CommandInfo {
        name: "/keybindings",
        alias: "/kb",
        args: "",
        description: "Configure keybindings",
    },
    CommandInfo {
        name: "/emoji",
        alias: "/e",
        args: "[search]",
        description: "Open emoji picker",
    },
    CommandInfo {
        name: "/export",
        alias: "",
        args: "[n]",
        description: "Export chat history to text file",
    },
    CommandInfo {
        name: "/help",
        alias: "/h",
        args: "",
        description: "Show help",
    },
    CommandInfo {
        name: "/lock",
        alias: "",
        args: "",
        description: "Lock the session (requires passphrase to resume)",
    },
    CommandInfo {
        name: "/lock-reset",
        alias: "",
        args: "",
        description: "Change the lock passphrase",
    },
    CommandInfo {
        name: "/quit",
        alias: "/q",
        args: "",
        description: "Exit siggy",
    },
];

/// Parsed user input — either a command or plain text to send
#[derive(Debug, PartialEq)]
pub enum InputAction {
    /// Send text to the current conversation
    SendText(String),
    /// Switch to a conversation by name/number
    Join(String),
    /// Leave current conversation (go back to no selection)
    Part,
    /// Delete the current conversation (with confirmation)
    DeleteConversation,
    /// Quit the application
    Quit,
    /// Lock the session immediately (boss key as a command).
    Lock,
    /// Begin the passphrase-change flow (locks the screen, prompts for current).
    LockReset,
    /// Toggle sidebar visibility
    ToggleSidebar,
    /// Toggle terminal bell notifications (None = both, Some("direct"/"group") = specific)
    ToggleBell(Option<String>),
    /// Mute/unmute the current conversation (None = toggle permanent, Some = timed)
    Mute(Option<String>),
    /// Block the current contact/group
    Block,
    /// Unblock the current contact/group
    Unblock,
    /// Show help text
    Help,
    /// Open settings overlay
    Settings,
    /// Open contacts overlay
    Contacts,
    /// Open file browser to attach a file
    Attach,
    /// Paste clipboard contents (image, file path, or text)
    Paste,
    /// Search messages in current (or all) conversations
    Search(String),
    /// Set disappearing message timer (raw duration string)
    SetDisappearing(String),
    /// Open group management menu
    Group,
    /// Open theme picker
    Theme,
    /// Create a poll
    Poll {
        question: String,
        options: Vec<String>,
        allow_multiple: bool,
    },
    /// Show identity verification overlay
    Verify,
    /// Edit Signal profile
    Profile,
    /// Show about overlay
    About,
    /// Open keybindings overlay
    Keybindings,
    /// Open the emoji picker overlay (optional initial search filter)
    Emoji(String),
    /// Export chat history to a text file (optional: last N messages)
    Export(Option<usize>),
    /// Unknown command
    Unknown(String),
}

/// Parse a line of input into an action
pub fn parse_input(input: &str) -> InputAction {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return InputAction::SendText(String::new());
    }

    if !trimmed.starts_with('/') {
        // Handle :q and :quit as vim-style quit aliases
        if trimmed == ":q" || trimmed == ":quit" {
            return InputAction::Quit;
        }
        return InputAction::SendText(trimmed.to_string());
    }

    let mut parts = trimmed.splitn(2, ' ');
    let cmd = parts.next().unwrap_or("");
    let arg = parts.next().unwrap_or("").trim().to_string();

    match cmd {
        "/join" | "/j" => {
            if arg.is_empty() {
                InputAction::Unknown("/join requires a contact or group name".to_string())
            } else {
                InputAction::Join(arg)
            }
        }
        "/part" | "/p" => InputAction::Part,
        "/delete" => InputAction::DeleteConversation,
        "/quit" | "/q" => InputAction::Quit,
        "/lock" => InputAction::Lock,
        "/lock-reset" => InputAction::LockReset,
        "/sidebar" | "/sb" => InputAction::ToggleSidebar,
        "/bell" | "/notify" => {
            if arg.is_empty() {
                InputAction::ToggleBell(None)
            } else {
                InputAction::ToggleBell(Some(arg))
            }
        }
        "/mute" => {
            if arg.is_empty() {
                InputAction::Mute(None)
            } else {
                InputAction::Mute(Some(arg))
            }
        }
        "/block" => InputAction::Block,
        "/unblock" => InputAction::Unblock,
        "/attach" | "/a" => InputAction::Attach,
        "/paste" | "/pa" => InputAction::Paste, // /p taken by /part, /pa is the initialism
        "/search" | "/s" => {
            if arg.is_empty() {
                InputAction::Unknown("/search requires a query".to_string())
            } else {
                InputAction::Search(arg)
            }
        }
        "/contacts" | "/c" => InputAction::Contacts,
        "/settings" => InputAction::Settings,
        "/disappearing" | "/dm" => {
            if arg.is_empty() {
                InputAction::Unknown(
                    "/disappearing requires a duration (e.g. off, 30s, 5m, 1h, 1d, 1w)".to_string(),
                )
            } else {
                InputAction::SetDisappearing(arg)
            }
        }
        "/group" | "/g" => InputAction::Group,
        "/theme" | "/t" => InputAction::Theme,
        "/poll" => match parse_poll_args(&arg) {
            Some((question, options, allow_multiple)) if options.len() >= 2 => InputAction::Poll {
                question,
                options,
                allow_multiple,
            },
            _ => InputAction::Unknown(
                "Usage: /poll \"question\" \"option1\" \"option2\" [--single]".into(),
            ),
        },
        "/emoji" | "/e" => InputAction::Emoji(arg),
        "/verify" | "/v" => InputAction::Verify,
        "/profile" => InputAction::Profile,
        "/about" => InputAction::About,
        "/keybindings" | "/kb" => InputAction::Keybindings,
        "/export" => {
            if arg.is_empty() {
                InputAction::Export(None)
            } else {
                match arg.parse::<usize>() {
                    Ok(n) => InputAction::Export(Some(n)),
                    Err(_) => InputAction::Unknown(
                        "/export takes an optional number (e.g. /export 100)".to_string(),
                    ),
                }
            }
        }
        "/help" | "/h" => InputAction::Help,
        _ => InputAction::Unknown(format!("Unknown command: {cmd}")),
    }
}

/// Parse `/poll` arguments: extract quoted strings and `--single` flag.
/// Returns (question, options, allow_multiple) or None on parse failure.
fn parse_poll_args(input: &str) -> Option<(String, Vec<String>, bool)> {
    let mut parts: Vec<String> = Vec::new();
    let mut allow_multiple = true;
    let mut chars = input.chars().peekable();

    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        if c == '"' {
            // Quoted string
            chars.next(); // skip opening quote
            let mut s = String::new();
            loop {
                match chars.next() {
                    Some('\\') => {
                        if let Some(escaped) = chars.next() {
                            s.push(escaped);
                        }
                    }
                    Some('"') => break,
                    Some(ch) => s.push(ch),
                    None => break,
                }
            }
            parts.push(s);
        } else {
            // Unquoted token
            let mut s = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                s.push(c);
                chars.next();
            }
            if s == "--single" {
                allow_multiple = false;
            } else {
                parts.push(s);
            }
        }
    }

    if parts.len() < 3 {
        // Need at least question + 2 options
        return None;
    }
    let question = parts.remove(0);
    Some((question, parts, allow_multiple))
}

/// Format seconds as a compact duration: "30s", "5m", "1h", "1d", "1w".
pub fn format_compact_duration(seconds: i64) -> String {
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else if seconds < 86400 {
        format!("{}h", seconds / 3600)
    } else if seconds < 604800 {
        format!("{}d", seconds / 86400)
    } else {
        format!("{}w", seconds / 604800)
    }
}

/// Parse a human-readable duration string into seconds.
/// Returns Ok(seconds) or Err(message) for invalid input.
pub fn parse_duration_to_seconds(s: &str) -> Result<i64, String> {
    let s = s.trim().to_lowercase();
    if s == "off" || s == "0" {
        return Ok(0);
    }
    // Try parsing as number + suffix
    let (num_str, multiplier) = if let Some(n) = s.strip_suffix('s') {
        (n, 1i64)
    } else if let Some(n) = s.strip_suffix('m') {
        (n, 60)
    } else if let Some(n) = s.strip_suffix('h') {
        (n, 3600)
    } else if let Some(n) = s.strip_suffix('d') {
        (n, 86400)
    } else if let Some(n) = s.strip_suffix('w') {
        (n, 604800)
    } else {
        return Err(format!("Invalid duration: {s}. Use off/30s/5m/1h/1d/1w/4w"));
    };
    match num_str.parse::<i64>() {
        Ok(n) if n > 0 => n
            .checked_mul(multiplier)
            .ok_or_else(|| format!("Duration too large: {s}")),
        _ => Err(format!("Invalid duration: {s}. Use off/30s/5m/1h/1d/1w/4w")),
    }
}

/// Format remaining mute time compactly for sidebar display.
/// Uses the largest unit that yields a value >= 1: `~Nw`, `~Nd`, `~Nh`, `~Nm`.
/// For less than 60 seconds returns `~<1m`.
pub fn format_mute_remaining(seconds: i64) -> String {
    if seconds < 60 {
        "~<1m".to_string()
    } else if seconds < 3600 {
        format!("~{}m", seconds / 60)
    } else if seconds < 86400 {
        format!("~{}h", seconds / 3600)
    } else if seconds < 604800 {
        format!("~{}d", seconds / 86400)
    } else {
        format!("~{}w", seconds / 604800)
    }
}

/// Replace `:shortcode:` patterns in text with their corresponding emoji.
/// Unrecognized shortcodes are left as-is.
pub fn replace_shortcodes(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find(':') {
        result.push_str(&rest[..start]);
        let after_colon = &rest[start + 1..];
        if let Some(end) = after_colon.find(':') {
            let candidate = &after_colon[..end];
            if !candidate.is_empty()
                && candidate
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '+')
                && let Some(emoji) = emojis::get_by_shortcode(candidate)
            {
                result.push_str(emoji.as_str());
                rest = &after_colon[end + 1..];
                continue;
            }
            // Not a valid shortcode — emit the colon and continue
            result.push(':');
            rest = after_colon;
        } else {
            // No closing colon found
            result.push(':');
            rest = after_colon;
        }
    }
    result.push_str(rest);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    // --- No-arg commands: 19 cases → 1 parameterized test ---

    #[rstest]
    #[case("/part", InputAction::Part)]
    #[case("/p", InputAction::Part)]
    #[case("/delete", InputAction::DeleteConversation)]
    #[case("/quit", InputAction::Quit)]
    #[case("/q", InputAction::Quit)]
    #[case("/sidebar", InputAction::ToggleSidebar)]
    #[case("/sb", InputAction::ToggleSidebar)]
    #[case("/mute", InputAction::Mute(None))]
    #[case("/settings", InputAction::Settings)]
    #[case("/attach", InputAction::Attach)]
    #[case("/a", InputAction::Attach)]
    #[case("/paste", InputAction::Paste)]
    #[case("/pa", InputAction::Paste)]
    #[case("/contacts", InputAction::Contacts)]
    #[case("/c", InputAction::Contacts)]
    #[case("/help", InputAction::Help)]
    #[case("/h", InputAction::Help)]
    #[case("/block", InputAction::Block)]
    #[case("/unblock", InputAction::Unblock)]
    #[case("/group", InputAction::Group)]
    #[case("/g", InputAction::Group)]
    #[case("/verify", InputAction::Verify)]
    #[case("/v", InputAction::Verify)]
    #[case("/profile", InputAction::Profile)]
    #[case("/about", InputAction::About)]
    #[case("/keybindings", InputAction::Keybindings)]
    #[case("/kb", InputAction::Keybindings)]
    #[case("/bell", InputAction::ToggleBell(None))]
    #[case("/emoji", InputAction::Emoji("".to_string()))]
    #[case("/e", InputAction::Emoji("".to_string()))]
    fn command_returns_expected_action(#[case] input: &str, #[case] expected: InputAction) {
        assert_eq!(parse_input(input), expected);
    }

    // --- Commands with arguments ---

    #[rstest]
    #[case("/join Alice", InputAction::Join("Alice".to_string()))]
    #[case("/j +1234567890", InputAction::Join("+1234567890".to_string()))]
    #[case("/search hello", InputAction::Search("hello".to_string()))]
    #[case("/s world", InputAction::Search("world".to_string()))]
    #[case("/disappearing 30s", InputAction::SetDisappearing("30s".to_string()))]
    #[case("/dm off", InputAction::SetDisappearing("off".to_string()))]
    #[case("/bell direct", InputAction::ToggleBell(Some("direct".to_string())))]
    #[case("/notify group", InputAction::ToggleBell(Some("group".to_string())))]
    #[case("/mute 2h", InputAction::Mute(Some("2h".to_string())))]
    #[case("/mute 1d", InputAction::Mute(Some("1d".to_string())))]
    #[case("/emoji smile", InputAction::Emoji("smile".to_string()))]
    #[case("/e rocket", InputAction::Emoji("rocket".to_string()))]
    fn command_with_argument(#[case] input: &str, #[case] expected: InputAction) {
        assert_eq!(parse_input(input), expected);
    }

    // --- Commands that require an argument but didn't get one ---

    #[rstest]
    #[case("/join")]
    #[case("/search")]
    #[case("/disappearing")]
    fn command_without_required_arg_returns_unknown(#[case] input: &str) {
        let InputAction::Unknown(s) = parse_input(input) else {
            panic!("expected Unknown for {input}");
        };
        assert!(
            s.contains("requires"),
            "error for {input} should mention 'requires': {s}"
        );
    }

    // --- SendText variants ---

    #[rstest]
    #[case("hello world", "hello world")]
    #[case("", "")]
    #[case("   ", "")]
    #[case("  hello  ", "hello")]
    fn send_text_variants(#[case] input: &str, #[case] expected: &str) {
        let InputAction::SendText(s) = parse_input(input) else {
            panic!("expected SendText for {input:?}");
        };
        assert_eq!(s, expected);
    }

    // --- Unknown command ---

    #[test]
    fn unknown_command() {
        let InputAction::Unknown(s) = parse_input("/foo") else {
            panic!("expected Unknown")
        };
        assert!(s.contains("/foo"));
    }

    // --- Duration parser: valid cases ---

    #[rstest]
    #[case("off", 0)]
    #[case("0", 0)]
    #[case("30s", 30)]
    #[case("5m", 300)]
    #[case("1h", 3600)]
    #[case("8h", 28800)]
    #[case("1d", 86400)]
    #[case("1w", 604800)]
    #[case("4w", 2419200)]
    fn duration_parser_valid(#[case] input: &str, #[case] expected: i64) {
        assert_eq!(parse_duration_to_seconds(input).unwrap(), expected);
    }

    // --- Duration parser: invalid cases ---

    #[rstest]
    #[case("abc")]
    #[case("")]
    #[case("0s")]
    #[case("-1h")]
    fn duration_parser_invalid(#[case] input: &str) {
        assert!(
            parse_duration_to_seconds(input).is_err(),
            "expected error for {input:?}"
        );
    }

    // --- Mute remaining formatter ---

    #[rstest]
    #[case(30, "~<1m")]
    #[case(59, "~<1m")]
    #[case(60, "~1m")]
    #[case(90, "~1m")]
    #[case(3599, "~59m")]
    #[case(3600, "~1h")]
    #[case(5400, "~1h")]
    #[case(7200, "~2h")]
    #[case(86400, "~1d")]
    #[case(172800, "~2d")]
    #[case(604800, "~1w")]
    #[case(1209600, "~2w")]
    fn format_mute_remaining_cases(#[case] seconds: i64, #[case] expected: &str) {
        assert_eq!(format_mute_remaining(seconds), expected);
    }

    // --- Poll command ---

    #[test]
    fn poll_command_basic() {
        let result = parse_input(r#"/poll "What for lunch?" "Pizza" "Sushi""#);
        match result {
            InputAction::Poll {
                question,
                options,
                allow_multiple,
            } => {
                assert_eq!(question, "What for lunch?");
                assert_eq!(options, vec!["Pizza", "Sushi"]);
                assert!(allow_multiple);
            }
            other => panic!("expected Poll, got {other:?}"),
        }
    }

    #[test]
    fn poll_command_single_select() {
        let result = parse_input(r#"/poll "Q" "A" "B" --single"#);
        match result {
            InputAction::Poll {
                allow_multiple,
                options,
                ..
            } => {
                assert!(!allow_multiple);
                assert_eq!(options, vec!["A", "B"]);
            }
            other => panic!("expected Poll, got {other:?}"),
        }
    }

    #[test]
    fn poll_command_too_few_options() {
        let result = parse_input(r#"/poll "Q" "A""#);
        assert!(matches!(result, InputAction::Unknown(_)));
    }

    #[test]
    fn poll_command_no_args() {
        let result = parse_input("/poll");
        assert!(matches!(result, InputAction::Unknown(_)));
    }

    // --- Shortcode replacement ---

    #[rstest]
    #[case(":+1:", "\u{1f44d}")]
    #[case(":thumbsup:", "\u{1f44d}")]
    #[case(":rocket:", "\u{1f680}")]
    #[case("hello :+1: world", "hello \u{1f44d} world")]
    #[case(":+1: hello :rocket:", "\u{1f44d} hello \u{1f680}")]
    #[case("", "")]
    #[case("no colons here", "no colons here")]
    fn shortcode_replacement(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(replace_shortcodes(input), expected);
    }

    #[test]
    fn shortcode_unknown_left_as_is() {
        assert_eq!(
            replace_shortcodes(":not_a_real_emoji_xyz:"),
            ":not_a_real_emoji_xyz:"
        );
    }

    #[test]
    fn shortcode_unclosed_colon() {
        assert_eq!(replace_shortcodes("hello :world"), "hello :world");
    }

    #[test]
    fn shortcode_with_spaces_not_replaced() {
        assert_eq!(replace_shortcodes(":has spaces:"), ":has spaces:");
    }
}
