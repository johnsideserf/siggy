//! Scriptable message triggers (#615).
//!
//! Config-driven rules over incoming messages, loaded from `triggers.toml`
//! next to the main config: "on a message matching X from Y, auto-reply Z
//! and/or run a command". One engine, two hosts: the TUI evaluates rules in
//! `handle_signal_event` (replies queue through the normal send path) and
//! `siggy --watch` runs the same rules headless.
//!
//! Message bodies are remote-controlled, so the engine is built around
//! safety rails rather than raw power:
//! - `run` commands are argv arrays spawned directly (never through a
//!   shell); the message reaches the command as JSON on stdin, so hostile
//!   bodies cannot inject arguments.
//! - `run` rules stay dormant unless the file sets a top-level
//!   `allow_run = true`.
//! - Rules never fire on outgoing/synced-own messages, nor on messages
//!   older than engine start (the initial sync replays history).
//! - Each (rule, conversation) pair is rate limited (default 30s) so two
//!   auto-responders cannot ping-pong.
//!
//! Matching is dependency-free: case-insensitive `substring` (default),
//! `exact`, or `prefix` -- `run` commands receive the full message and can
//! do finer filtering themselves.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::Deserialize;

/// Default per-(rule, conversation) cooldown in seconds.
const DEFAULT_RATE_LIMIT_SECS: u64 = 30;

/// How a rule's `match` text is compared against the message body
/// (always case-insensitively).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchMode {
    Substring,
    Exact,
    Prefix,
}

/// One armed rule.
#[derive(Debug, Clone)]
pub struct Rule {
    pub match_text: String,
    pub match_mode: MatchMode,
    /// Sender filter: the sender's phone number / id must equal this.
    pub from: Option<String>,
    /// Conversation filter: matches the conversation id or exact name.
    pub conversation: Option<String>,
    pub reply: Option<String>,
    pub run: Option<Vec<String>>,
    pub rate_limit_secs: u64,
}

/// The incoming message being evaluated.
pub struct MessageContext<'a> {
    pub body: &'a str,
    pub sender_id: &'a str,
    pub sender_name: Option<&'a str>,
    pub conv_id: &'a str,
    pub conv_name: &'a str,
    pub is_group: bool,
    pub is_outgoing: bool,
    pub timestamp_ms: i64,
}

/// An action the host must execute for a fired rule.
#[derive(Debug, PartialEq)]
pub enum TriggerAction {
    /// Send `text` into the conversation the message arrived in.
    Reply {
        conv_id: String,
        is_group: bool,
        text: String,
    },
    /// Spawn `argv` with `stdin_json` written to its stdin.
    Run {
        argv: Vec<String>,
        stdin_json: String,
    },
}

/// Rule engine: rules plus the per-conversation cooldown bookkeeping.
#[derive(Default)]
pub struct TriggerEngine {
    rules: Vec<Rule>,
    allow_run: bool,
    /// Wall-clock ms at load; messages stamped earlier never fire (guards
    /// against the initial-sync history replay).
    armed_since_ms: i64,
    /// Last firing per (rule index, conversation id).
    last_fired: HashMap<(usize, String), Instant>,
    /// Load-time diagnostics, surfaced by both hosts.
    pub warnings: Vec<String>,
}

// --- TOML shapes ---------------------------------------------------------

#[derive(Deserialize)]
struct TriggersFile {
    #[serde(default)]
    allow_run: bool,
    #[serde(default, rename = "trigger")]
    triggers: Vec<RuleToml>,
}

#[derive(Deserialize)]
struct RuleToml {
    #[serde(rename = "match")]
    match_text: String,
    match_mode: Option<String>,
    from: Option<String>,
    conversation: Option<String>,
    reply: Option<String>,
    run: Option<Vec<String>>,
    rate_limit_secs: Option<u64>,
}

impl TriggerEngine {
    /// The path rules are loaded from: `<config dir>/siggy/triggers.toml`.
    pub fn rules_path() -> Option<std::path::PathBuf> {
        Some(dirs::config_dir()?.join("siggy").join("triggers.toml"))
    }

    /// Load rules from the default path. A missing file is a normal empty
    /// engine; a malformed file is an empty engine with a warning.
    pub fn load_default() -> Self {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let Some(path) = Self::rules_path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::from_toml_str(&text, now_ms),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => Self {
                warnings: vec![format!("triggers.toml read error: {e}")],
                ..Self::default()
            },
        }
    }

    /// Parse rules from TOML text. `now_ms` becomes the armed-since floor.
    pub fn from_toml_str(text: &str, now_ms: i64) -> Self {
        let mut engine = Self {
            armed_since_ms: now_ms,
            ..Self::default()
        };
        let file: TriggersFile = match toml::from_str(text) {
            Ok(f) => f,
            Err(e) => {
                engine
                    .warnings
                    .push(format!("triggers.toml parse error: {e}"));
                return engine;
            }
        };
        engine.allow_run = file.allow_run;
        for (i, rule) in file.triggers.into_iter().enumerate() {
            let n = i + 1;
            let match_mode = match rule.match_mode.as_deref() {
                None | Some("substring") => MatchMode::Substring,
                Some("exact") => MatchMode::Exact,
                Some("prefix") => MatchMode::Prefix,
                Some(other) => {
                    engine.warnings.push(format!(
                        "trigger {n}: unknown match_mode \"{other}\" (use substring, exact, or prefix); rule skipped"
                    ));
                    continue;
                }
            };
            if rule.match_text.trim().is_empty() {
                engine
                    .warnings
                    .push(format!("trigger {n}: empty match; rule skipped"));
                continue;
            }
            if rule.reply.is_none() && rule.run.is_none() {
                engine
                    .warnings
                    .push(format!("trigger {n}: no reply or run action; rule skipped"));
                continue;
            }
            if rule.run.is_some() && !engine.allow_run {
                engine.warnings.push(format!(
                    "trigger {n}: run action ignored (set top-level allow_run = true to arm it)"
                ));
            }
            if let Some(argv) = &rule.run
                && argv.is_empty()
            {
                engine
                    .warnings
                    .push(format!("trigger {n}: empty run command; rule skipped"));
                continue;
            }
            if rule.from.is_none() && rule.conversation.is_none() {
                engine.warnings.push(format!(
                    "trigger {n}: no from/conversation filter - it fires for any sender"
                ));
            }
            engine.rules.push(Rule {
                match_text: rule.match_text.to_lowercase(),
                match_mode,
                from: rule.from,
                conversation: rule.conversation,
                reply: rule.reply,
                run: rule.run,
                rate_limit_secs: rule.rate_limit_secs.unwrap_or(DEFAULT_RATE_LIMIT_SECS),
            });
        }
        engine
    }

    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// Evaluate an incoming message against every rule, applying the safety
    /// rails, and return the actions the host must execute. `now` is passed
    /// in (rather than read from the clock) so rate limiting is testable.
    pub fn evaluate(&mut self, ctx: &MessageContext<'_>, now: Instant) -> Vec<TriggerAction> {
        let mut actions = Vec::new();
        if self.rules.is_empty()
            || ctx.is_outgoing
            || ctx.body.is_empty()
            || ctx.timestamp_ms < self.armed_since_ms
        {
            return actions;
        }
        let body_lower = ctx.body.to_lowercase();

        for (i, rule) in self.rules.iter().enumerate() {
            if let Some(from) = &rule.from
                && from != ctx.sender_id
            {
                continue;
            }
            if let Some(conv) = &rule.conversation
                && conv != ctx.conv_id
                && conv != ctx.conv_name
            {
                continue;
            }
            let matched = match rule.match_mode {
                MatchMode::Substring => body_lower.contains(&rule.match_text),
                MatchMode::Exact => body_lower.trim() == rule.match_text,
                MatchMode::Prefix => body_lower.starts_with(&rule.match_text),
            };
            if !matched {
                continue;
            }
            // Cooldown per (rule, conversation): the loop-guard against two
            // auto-responders answering each other forever.
            let key = (i, ctx.conv_id.to_string());
            if let Some(last) = self.last_fired.get(&key)
                && rule.rate_limit_secs > 0
                && now.duration_since(*last) < Duration::from_secs(rule.rate_limit_secs)
            {
                continue;
            }
            self.last_fired.insert(key, now);

            if let Some(text) = &rule.reply {
                actions.push(TriggerAction::Reply {
                    conv_id: ctx.conv_id.to_string(),
                    is_group: ctx.is_group,
                    text: text.clone(),
                });
            }
            if let Some(argv) = &rule.run
                && self.allow_run
            {
                actions.push(TriggerAction::Run {
                    argv: argv.clone(),
                    stdin_json: message_json(ctx, i),
                });
            }
        }
        actions
    }
}

/// The JSON document a `run` command receives on stdin. The body travels
/// here (serde-escaped) rather than in the command line, so message content
/// can never become arguments.
fn message_json(ctx: &MessageContext<'_>, rule_index: usize) -> String {
    serde_json::json!({
        "timestamp_ms": ctx.timestamp_ms,
        "sender": ctx.sender_id,
        "sender_name": ctx.sender_name,
        "conversation_id": ctx.conv_id,
        "conversation_name": ctx.conv_name,
        "is_group": ctx.is_group,
        "body": ctx.body,
        "rule": rule_index + 1,
    })
    .to_string()
}

/// Spawn a `run` action's command, detached, writing the message JSON to its
/// stdin from a helper thread so the caller never blocks. Spawn failures are
/// returned for the host to surface; the child itself is fire-and-forget.
pub fn execute_run(argv: &[String], stdin_json: String) -> std::io::Result<()> {
    let (exe, args) = argv.split_first().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "empty run command")
    })?;
    let mut child = std::process::Command::new(exe)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        std::thread::spawn(move || {
            use std::io::Write;
            let _ = stdin.write_all(stdin_json.as_bytes());
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(body: &'a str, sender: &'a str, ts: i64) -> MessageContext<'a> {
        MessageContext {
            body,
            sender_id: sender,
            sender_name: None,
            conv_id: sender,
            conv_name: "Alice",
            is_group: false,
            is_outgoing: false,
            timestamp_ms: ts,
        }
    }

    const PING: &str = r#"
        [[trigger]]
        match = "ping"
        match_mode = "exact"
        reply = "pong"
    "#;

    #[test]
    fn exact_match_replies_case_insensitively() {
        let mut e = TriggerEngine::from_toml_str(PING, 1000);
        let now = Instant::now();
        assert_eq!(
            e.evaluate(&ctx("PING", "+1", 2000), now),
            vec![TriggerAction::Reply {
                conv_id: "+1".into(),
                is_group: false,
                text: "pong".into()
            }]
        );
        assert!(e.evaluate(&ctx("ping pong", "+2", 2000), now).is_empty());
    }

    #[test]
    fn rails_skip_outgoing_and_presync_messages() {
        let mut e = TriggerEngine::from_toml_str(PING, 1000);
        let now = Instant::now();
        // Older than engine start: history replayed by the initial sync.
        assert!(e.evaluate(&ctx("ping", "+1", 999), now).is_empty());
        // Own messages never fire rules.
        let mut own = ctx("ping", "+1", 2000);
        own.is_outgoing = true;
        assert!(e.evaluate(&own, now).is_empty());
    }

    #[test]
    fn rate_limit_is_per_rule_per_conversation() {
        let mut e = TriggerEngine::from_toml_str(PING, 1000);
        let t0 = Instant::now();
        assert_eq!(e.evaluate(&ctx("ping", "+1", 2000), t0).len(), 1);
        // Same conversation, inside the window: suppressed.
        assert!(e.evaluate(&ctx("ping", "+1", 2001), t0).is_empty());
        // Different conversation: its own cooldown.
        assert_eq!(e.evaluate(&ctx("ping", "+2", 2002), t0).len(), 1);
        // Past the window: fires again.
        let later = t0 + Duration::from_secs(31);
        assert_eq!(e.evaluate(&ctx("ping", "+1", 2003), later).len(), 1);
    }

    #[test]
    fn sender_and_conversation_filters_gate() {
        let toml = r#"
            [[trigger]]
            match = "hi"
            from = "+1"
            conversation = "Alice"
            reply = "hello"
        "#;
        let mut e = TriggerEngine::from_toml_str(toml, 0);
        let now = Instant::now();
        assert_eq!(e.evaluate(&ctx("hi", "+1", 1), now).len(), 1);
        // Wrong sender.
        assert!(e.evaluate(&ctx("hi", "+9", 1), now).is_empty());
        // Conversation filter also accepts the conv id.
        let toml_id = r#"
            [[trigger]]
            match = "hi"
            conversation = "+1"
            reply = "hello"
        "#;
        let mut e2 = TriggerEngine::from_toml_str(toml_id, 0);
        assert_eq!(e2.evaluate(&ctx("hi", "+1", 1), now).len(), 1);
    }

    #[test]
    fn run_requires_allow_run_and_body_travels_via_stdin_json() {
        let gated = r#"
            [[trigger]]
            match = "deploy"
            from = "+1"
            run = ["deploy.sh", "--now"]
        "#;
        let mut e = TriggerEngine::from_toml_str(gated, 0);
        assert!(
            e.warnings.iter().any(|w| w.contains("allow_run")),
            "expected an allow_run warning, got {:?}",
            e.warnings
        );
        assert!(
            e.evaluate(&ctx("deploy", "+1", 1), Instant::now())
                .is_empty()
        );

        let armed = format!("allow_run = true\n{gated}");
        let mut e = TriggerEngine::from_toml_str(&armed, 0);
        let actions = e.evaluate(&ctx("deploy; rm -rf /", "+1", 1), Instant::now());
        let TriggerAction::Run { argv, stdin_json } = &actions[0] else {
            panic!("expected Run action");
        };
        // argv is fixed from config; the hostile body only exists inside
        // the JSON payload.
        assert_eq!(argv, &["deploy.sh".to_string(), "--now".to_string()]);
        let v: serde_json::Value = serde_json::from_str(stdin_json).unwrap();
        assert_eq!(v["body"], "deploy; rm -rf /");
        assert_eq!(v["sender"], "+1");
    }

    #[test]
    fn load_warnings_and_skips() {
        let toml = r#"
            [[trigger]]
            match = "no action here"

            [[trigger]]
            match = ""
            reply = "x"

            [[trigger]]
            match = "bad mode"
            match_mode = "regex"
            reply = "x"

            [[trigger]]
            match = "broad"
            reply = "ok"
        "#;
        let e = TriggerEngine::from_toml_str(toml, 0);
        assert_eq!(e.rule_count(), 1, "only the last rule is valid");
        assert_eq!(e.warnings.len(), 4); // 3 skips + 1 "fires for any sender"
        // Parse errors degrade to an empty engine with a warning.
        let bad = TriggerEngine::from_toml_str("not [ toml", 0);
        assert_eq!(bad.rule_count(), 0);
        assert!(!bad.warnings.is_empty());
    }
}
