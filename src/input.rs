/// Metadata for a slash command (used for autocomplete + help)
pub struct CommandInfo {
    pub name: &'static str,
    pub alias: &'static str,
    pub args: &'static str,
    pub description: &'static str,
}

pub const COMMANDS: &[CommandInfo] = &[
    CommandInfo { name: "/join",     alias: "/j",  args: "<name>",  description: "Switch to a conversation" },
    CommandInfo { name: "/part",     alias: "/p",  args: "",        description: "Leave current conversation" },
    CommandInfo { name: "/sidebar",  alias: "/sb", args: "",        description: "Toggle sidebar" },
    CommandInfo { name: "/bell",     alias: "",    args: "[type]",  description: "Toggle notifications (direct/group)" },
    CommandInfo { name: "/mute",     alias: "",    args: "",        description: "Mute/unmute current chat" },
    CommandInfo { name: "/attach",   alias: "/a",  args: "",        description: "Attach a file" },
    CommandInfo { name: "/search",   alias: "/s",  args: "<query>", description: "Search messages" },
    CommandInfo { name: "/contacts", alias: "/c",  args: "",        description: "Browse contacts" },
    CommandInfo { name: "/settings", alias: "",    args: "",        description: "Open settings" },
    CommandInfo { name: "/disappearing", alias: "/dm", args: "<duration>", description: "Set disappearing timer (off/30s/5m/1h/1d/1w)" },
    CommandInfo { name: "/group",    alias: "/g",  args: "",        description: "Group management" },
    CommandInfo { name: "/help",     alias: "/h",  args: "",        description: "Show help" },
    CommandInfo { name: "/quit",     alias: "/q",  args: "",        description: "Exit signal-tui" },
];

/// Parsed user input — either a command or plain text to send
#[derive(Debug)]
pub enum InputAction {
    /// Send text to the current conversation
    SendText(String),
    /// Switch to a conversation by name/number
    Join(String),
    /// Leave current conversation (go back to no selection)
    Part,
    /// Quit the application
    Quit,
    /// Toggle sidebar visibility
    ToggleSidebar,
    /// Toggle terminal bell notifications (None = both, Some("direct"/"group") = specific)
    ToggleBell(Option<String>),
    /// Mute/unmute the current conversation
    ToggleMute,
    /// Show help text
    Help,
    /// Open settings overlay
    Settings,
    /// Open contacts overlay
    Contacts,
    /// Open file browser to attach a file
    Attach,
    /// Search messages in current (or all) conversations
    Search(String),
    /// Set disappearing message timer (raw duration string)
    SetDisappearing(String),
    /// Open group management menu
    Group,
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
        "/quit" | "/q" => InputAction::Quit,
        "/sidebar" | "/sb" => InputAction::ToggleSidebar,
        "/bell" | "/notify" => {
            if arg.is_empty() {
                InputAction::ToggleBell(None)
            } else {
                InputAction::ToggleBell(Some(arg))
            }
        }
        "/mute" => InputAction::ToggleMute,
        "/attach" | "/a" => InputAction::Attach,
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
                InputAction::Unknown("/disappearing requires a duration (e.g. off, 30s, 5m, 1h, 1d, 1w)".to_string())
            } else {
                InputAction::SetDisappearing(arg)
            }
        }
        "/group" | "/g" => InputAction::Group,
        "/help" | "/h" => InputAction::Help,
        _ => InputAction::Unknown(format!("Unknown command: {cmd}")),
    }
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
        Ok(n) if n > 0 => Ok(n * multiplier),
        _ => Err(format!("Invalid duration: {s}. Use off/30s/5m/1h/1d/1w/4w")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text() {
        let InputAction::SendText(s) = parse_input("hello world") else { panic!("expected SendText") };
        assert_eq!(s, "hello world");
    }

    #[test]
    fn empty_input() {
        let InputAction::SendText(s) = parse_input("") else { panic!("expected SendText") };
        assert!(s.is_empty());
    }

    #[test]
    fn whitespace_only() {
        let InputAction::SendText(s) = parse_input("   ") else { panic!("expected SendText") };
        assert!(s.is_empty());
    }

    #[test]
    fn trimmed_text() {
        let InputAction::SendText(s) = parse_input("  hello  ") else { panic!("expected SendText") };
        assert_eq!(s, "hello");
    }

    #[test]
    fn join_with_arg() {
        let InputAction::Join(s) = parse_input("/join Alice") else { panic!("expected Join") };
        assert_eq!(s, "Alice");
    }

    #[test]
    fn join_alias() {
        let InputAction::Join(s) = parse_input("/j +1234567890") else { panic!("expected Join") };
        assert_eq!(s, "+1234567890");
    }

    #[test]
    fn join_without_arg() {
        let InputAction::Unknown(s) = parse_input("/join") else { panic!("expected Unknown") };
        assert!(s.contains("requires"));
    }

    #[test]
    fn part_command() {
        assert!(matches!(parse_input("/part"), InputAction::Part));
    }

    #[test]
    fn part_alias() {
        assert!(matches!(parse_input("/p"), InputAction::Part));
    }

    #[test]
    fn quit_command() {
        assert!(matches!(parse_input("/quit"), InputAction::Quit));
    }

    #[test]
    fn quit_alias() {
        assert!(matches!(parse_input("/q"), InputAction::Quit));
    }

    #[test]
    fn sidebar_command() {
        assert!(matches!(parse_input("/sidebar"), InputAction::ToggleSidebar));
    }

    #[test]
    fn sidebar_alias() {
        assert!(matches!(parse_input("/sb"), InputAction::ToggleSidebar));
    }

    #[test]
    fn bell_no_arg() {
        let InputAction::ToggleBell(None) = parse_input("/bell") else { panic!("expected ToggleBell(None)") };
    }

    #[test]
    fn bell_with_arg() {
        let InputAction::ToggleBell(Some(s)) = parse_input("/bell direct") else { panic!("expected ToggleBell(Some)") };
        assert_eq!(s, "direct");
    }

    #[test]
    fn notify_alias() {
        let InputAction::ToggleBell(Some(s)) = parse_input("/notify group") else { panic!("expected ToggleBell(Some)") };
        assert_eq!(s, "group");
    }

    #[test]
    fn mute_command() {
        assert!(matches!(parse_input("/mute"), InputAction::ToggleMute));
    }

    #[test]
    fn settings_command() {
        assert!(matches!(parse_input("/settings"), InputAction::Settings));
    }

    #[test]
    fn attach_command() {
        assert!(matches!(parse_input("/attach"), InputAction::Attach));
    }

    #[test]
    fn attach_alias() {
        assert!(matches!(parse_input("/a"), InputAction::Attach));
    }

    #[test]
    fn contacts_command() {
        assert!(matches!(parse_input("/contacts"), InputAction::Contacts));
    }

    #[test]
    fn contacts_alias() {
        assert!(matches!(parse_input("/c"), InputAction::Contacts));
    }

    #[test]
    fn help_command() {
        assert!(matches!(parse_input("/help"), InputAction::Help));
    }

    #[test]
    fn help_alias() {
        assert!(matches!(parse_input("/h"), InputAction::Help));
    }

    #[test]
    fn search_with_query() {
        let InputAction::Search(s) = parse_input("/search hello") else { panic!("expected Search") };
        assert_eq!(s, "hello");
    }

    #[test]
    fn search_alias() {
        let InputAction::Search(s) = parse_input("/s world") else { panic!("expected Search") };
        assert_eq!(s, "world");
    }

    #[test]
    fn search_without_query() {
        let InputAction::Unknown(s) = parse_input("/search") else { panic!("expected Unknown") };
        assert!(s.contains("requires"));
    }

    #[test]
    fn unknown_command() {
        let InputAction::Unknown(s) = parse_input("/foo") else { panic!("expected Unknown") };
        assert!(s.contains("/foo"));
    }

    #[test]
    fn disappearing_command() {
        let InputAction::SetDisappearing(s) = parse_input("/disappearing 30s") else { panic!("expected SetDisappearing") };
        assert_eq!(s, "30s");
    }

    #[test]
    fn disappearing_alias() {
        let InputAction::SetDisappearing(s) = parse_input("/dm off") else { panic!("expected SetDisappearing") };
        assert_eq!(s, "off");
    }

    #[test]
    fn disappearing_without_arg() {
        let InputAction::Unknown(s) = parse_input("/disappearing") else { panic!("expected Unknown") };
        assert!(s.contains("requires"));
    }

    #[test]
    fn duration_parser_off() {
        assert_eq!(parse_duration_to_seconds("off").unwrap(), 0);
        assert_eq!(parse_duration_to_seconds("0").unwrap(), 0);
    }

    #[test]
    fn duration_parser_seconds() {
        assert_eq!(parse_duration_to_seconds("30s").unwrap(), 30);
    }

    #[test]
    fn duration_parser_minutes() {
        assert_eq!(parse_duration_to_seconds("5m").unwrap(), 300);
    }

    #[test]
    fn duration_parser_hours() {
        assert_eq!(parse_duration_to_seconds("1h").unwrap(), 3600);
        assert_eq!(parse_duration_to_seconds("8h").unwrap(), 28800);
    }

    #[test]
    fn duration_parser_days() {
        assert_eq!(parse_duration_to_seconds("1d").unwrap(), 86400);
    }

    #[test]
    fn duration_parser_weeks() {
        assert_eq!(parse_duration_to_seconds("1w").unwrap(), 604800);
        assert_eq!(parse_duration_to_seconds("4w").unwrap(), 2419200);
    }

    #[test]
    fn duration_parser_invalid() {
        assert!(parse_duration_to_seconds("abc").is_err());
        assert!(parse_duration_to_seconds("").is_err());
        assert!(parse_duration_to_seconds("0s").is_err());
        assert!(parse_duration_to_seconds("-1h").is_err());
    }

    #[test]
    fn group_command() {
        assert!(matches!(parse_input("/group"), InputAction::Group));
    }

    #[test]
    fn group_alias() {
        assert!(matches!(parse_input("/g"), InputAction::Group));
    }
}
