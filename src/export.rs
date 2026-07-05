//! Conversation export rendering (#613).
//!
//! Pure formatting for `/export`: given a conversation name and its display
//! messages, produce the file contents in plain text, Markdown, or JSON.
//! `App::export_chat_history` owns file naming, control-char stripping, and
//! the actual write; everything here is side-effect free so each format can
//! be unit-tested on constructed messages.

use chrono::{DateTime, Local};

use crate::conversation_store::DisplayMessage;
// ExportFormat lives in `input` (with the `/export` parser) because `input`
// is part of the fuzz lib target and this module is not.
pub use crate::input::ExportFormat;

/// Render `messages` in the given format. `exported_at` is stamped into the
/// header (passed in rather than read from the clock so output is testable).
pub fn render(
    format: ExportFormat,
    conv_name: &str,
    messages: &[DisplayMessage],
    exported_at: DateTime<Local>,
) -> String {
    match format {
        ExportFormat::Text => render_text(conv_name, messages, exported_at),
        ExportFormat::Markdown => render_markdown(conv_name, messages, exported_at),
        ExportFormat::Json => render_json(conv_name, messages, exported_at),
    }
}

fn local_minute(ts: &DateTime<chrono::Utc>) -> String {
    ts.with_timezone(&Local)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

fn reactions_summary(msg: &DisplayMessage) -> Option<String> {
    if msg.reactions.is_empty() {
        return None;
    }
    Some(
        msg.reactions
            .iter()
            .map(|r| format!("{} ({})", r.emoji, r.sender))
            .collect::<Vec<_>>()
            .join(", "),
    )
}

fn render_text(
    conv_name: &str,
    messages: &[DisplayMessage],
    exported_at: DateTime<Local>,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("Chat export: {conv_name}\n"));
    out.push_str(&format!(
        "Exported: {}\n",
        exported_at.format("%Y-%m-%d %H:%M")
    ));
    out.push_str(&format!("Messages: {}\n", messages.len()));
    out.push_str(&"-".repeat(60));
    out.push('\n');

    for msg in messages {
        let time = local_minute(&msg.timestamp);
        if msg.is_system {
            out.push_str(&format!("[{time}] * {}\n", msg.body));
        } else {
            let prefix = if msg.is_edited { "(edited) " } else { "" };
            out.push_str(&format!("[{time}] <{}> {prefix}{}\n", msg.sender, msg.body));
            if let Some(ref q) = msg.quote {
                out.push_str(&format!("  > <{}> {}\n", q.author, q.body));
            }
            if let Some(reactions) = reactions_summary(msg) {
                out.push_str(&format!("  reactions: {reactions}\n"));
            }
        }
    }
    out
}

fn render_markdown(
    conv_name: &str,
    messages: &[DisplayMessage],
    exported_at: DateTime<Local>,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("# Chat export: {conv_name}\n\n"));
    out.push_str(&format!(
        "Exported {} - {} messages\n\n---\n",
        exported_at.format("%Y-%m-%d %H:%M"),
        messages.len()
    ));

    for msg in messages {
        let time = local_minute(&msg.timestamp);
        out.push('\n');
        if msg.is_system {
            out.push_str(&format!("*[{time}] {}*\n", msg.body));
            continue;
        }
        let edited = if msg.is_edited { " (edited)" } else { "" };
        out.push_str(&format!(
            "**[{time}] {}:**{edited} {}\n",
            msg.sender, msg.body
        ));
        if let Some(ref q) = msg.quote {
            out.push_str(&format!("> {}: {}\n", q.author, q.body));
        }
        if let Some(reactions) = reactions_summary(msg) {
            out.push_str(&format!("- reactions: {reactions}\n"));
        }
    }
    out
}

fn render_json(
    conv_name: &str,
    messages: &[DisplayMessage],
    exported_at: DateTime<Local>,
) -> String {
    let msgs: Vec<serde_json::Value> = messages
        .iter()
        .map(|msg| {
            serde_json::json!({
                "timestamp": msg.timestamp.to_rfc3339(),
                "timestamp_ms": msg.timestamp_ms,
                "sender": msg.sender,
                "sender_id": msg.sender_id,
                "body": msg.body,
                "system": msg.is_system,
                "edited": msg.is_edited,
                "deleted": msg.is_deleted,
                "quote": msg.quote.as_ref().map(|q| {
                    serde_json::json!({ "author": q.author, "body": q.body })
                }),
                "reactions": msg.reactions.iter().map(|r| {
                    serde_json::json!({ "emoji": r.emoji, "sender": r.sender })
                }).collect::<Vec<_>>(),
            })
        })
        .collect();

    let doc = serde_json::json!({
        "conversation": conv_name,
        "exported_at": exported_at.to_rfc3339(),
        "message_count": messages.len(),
        "messages": msgs,
    });
    // Pretty output for humans; still valid input for jq and scripts.
    serde_json::to_string_pretty(&doc).unwrap_or_else(|_| "{}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation_store::Quote;
    use crate::signal::types::{MessageStatus, Reaction};
    use chrono::TimeZone;

    fn msg(sender: &str, body: &str, ts_ms: i64) -> DisplayMessage {
        DisplayMessage {
            sender: sender.to_string(),
            timestamp: chrono::Utc.timestamp_millis_opt(ts_ms).unwrap(),
            body: body.to_string(),
            is_system: false,
            image_lines: None,
            image_path: None,
            status: Some(MessageStatus::Sent),
            timestamp_ms: ts_ms,
            reactions: Vec::new(),
            mention_ranges: Vec::new(),
            style_ranges: Vec::new(),
            body_raw: None,
            mentions: Vec::new(),
            quote: None,
            is_edited: false,
            is_deleted: false,
            is_pinned: false,
            sender_id: sender.to_string(),
            expires_in_seconds: 0,
            expiration_start_ms: 0,
            poll_data: None,
            poll_votes: Vec::new(),
            preview: None,
            preview_image_lines: None,
            preview_image_path: None,
        }
    }

    fn now() -> DateTime<Local> {
        Local.timestamp_millis_opt(1_700_000_000_000).unwrap()
    }

    #[test]
    fn from_arg_parses_formats_case_insensitively() {
        assert_eq!(ExportFormat::from_arg("txt"), Some(ExportFormat::Text));
        assert_eq!(ExportFormat::from_arg("TEXT"), Some(ExportFormat::Text));
        assert_eq!(ExportFormat::from_arg("md"), Some(ExportFormat::Markdown));
        assert_eq!(
            ExportFormat::from_arg("Markdown"),
            Some(ExportFormat::Markdown)
        );
        assert_eq!(ExportFormat::from_arg("json"), Some(ExportFormat::Json));
        assert_eq!(ExportFormat::from_arg("csv"), None);
        assert_eq!(ExportFormat::from_arg("100"), None);
    }

    #[test]
    fn text_includes_header_quote_and_reactions() {
        let mut m = msg("alice", "hello", 1_700_000_000_000);
        m.quote = Some(Quote {
            author: "bob".to_string(),
            body: "earlier".to_string(),
            timestamp_ms: 1,
            author_id: "+2".to_string(),
        });
        m.reactions = vec![Reaction {
            emoji: "\u{1F44D}".to_string(),
            sender: "bob".to_string(),
        }];
        let out = render(ExportFormat::Text, "Alice", &[m], now());
        assert!(out.starts_with("Chat export: Alice\n"));
        assert!(out.contains("Messages: 1\n"));
        assert!(out.contains("<alice> hello\n"));
        assert!(out.contains("  > <bob> earlier\n"));
        assert!(out.contains("  reactions: \u{1F44D} (bob)\n"));
    }

    #[test]
    fn text_marks_system_and_edited_messages() {
        let mut edited = msg("alice", "fixed", 1);
        edited.is_edited = true;
        let mut system = msg("", "group renamed", 2);
        system.is_system = true;
        let out = render(ExportFormat::Text, "c", &[edited, system], now());
        assert!(out.contains("<alice> (edited) fixed\n"));
        assert!(out.contains("] * group renamed\n"));
    }

    #[test]
    fn markdown_formats_messages_and_quotes() {
        let mut m = msg("alice", "hello", 1_700_000_000_000);
        m.quote = Some(Quote {
            author: "bob".to_string(),
            body: "earlier".to_string(),
            timestamp_ms: 1,
            author_id: "+2".to_string(),
        });
        let out = render(ExportFormat::Markdown, "Alice", &[m], now());
        assert!(out.starts_with("# Chat export: Alice\n"));
        assert!(out.contains("alice:** hello\n"));
        assert!(out.contains("> bob: earlier\n"));
    }

    #[test]
    fn json_round_trips_and_carries_fields() {
        let mut m = msg("alice", "hi \"there\"", 1_700_000_000_000);
        m.reactions = vec![Reaction {
            emoji: "\u{2764}".to_string(),
            sender: "you".to_string(),
        }];
        let out = render(ExportFormat::Json, "Alice", &[m], now());
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["conversation"], "Alice");
        assert_eq!(v["message_count"], 1);
        assert_eq!(v["messages"][0]["sender"], "alice");
        assert_eq!(v["messages"][0]["body"], "hi \"there\"");
        assert_eq!(v["messages"][0]["quote"], serde_json::Value::Null);
        assert_eq!(v["messages"][0]["reactions"][0]["emoji"], "\u{2764}");
    }

    #[test]
    fn json_escapes_control_characters() {
        let m = msg("alice", "evil\u{1b}[31mtext", 1);
        let out = render(ExportFormat::Json, "c", &[m], now());
        // Raw ESC must not appear; serde escapes it as a \u sequence.
        assert!(!out.contains('\u{1b}'));
        assert!(out.contains("\\u001b"));
    }

    #[test]
    fn extensions_match_formats() {
        assert_eq!(ExportFormat::Text.extension(), "txt");
        assert_eq!(ExportFormat::Markdown.extension(), "md");
        assert_eq!(ExportFormat::Json.extension(), "json");
    }
}
