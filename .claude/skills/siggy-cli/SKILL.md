---
name: siggy-cli
description: Use when sending, receiving, or listing Signal messages through siggy, or checking siggy's setup, from scripts/automation without the interactive TUI - drives siggy's non-interactive CLI flags (--send / --receive / --list / --check / --version).
---

# Driving siggy from the command line

siggy is a terminal Signal client. Besides its interactive TUI, it exposes a
small non-interactive CLI for scripting and automation (issue #257). Every
command below reads the same config/account and cached database the TUI uses, so
the account must already be set up (run `siggy` once, or `siggy --setup`, to link
a device). Verify readiness first with `--check`.

All output is plain or tab-separated stdout with scriptable exit codes, so it
pipes cleanly into `grep` / `cut` / `awk` / `jq`-style tooling.

## Commands

### Check setup (do this first)
```sh
siggy --check
```
Prints a health report (version, config path, account, signal-cli path + detected
version, download dir). **Exit 0** means ready to send/receive; **exit 1** means
signal-cli is missing or no account is configured. Use it as a guard before the
other commands.

### Send one message
```sh
siggy --send <recipient> <message>
```
- `<recipient>`: an E.164 phone number for a 1:1 (e.g. `+15551234567`) or a group
  id for a group.
- Spawns signal-cli, sends, waits up to 30s for confirmation.
- **Exit 0** = the message was confirmed sent; **exit 1** = rejected, timed out,
  or signal-cli error; **exit 2** = missing arguments.
- Quote the message so the shell passes it as one argument.

```sh
siggy --check && siggy --send +15551234567 "deploy finished"
```

### List conversations
```sh
siggy --list
```
Reads the cached database (no signal-cli spawn) and prints one tab-separated row
per conversation: `unread<TAB>type<TAB>name<TAB>id`, where `type` is `dm` or
`group`. No header. Exit 1 if the database does not exist yet.

```sh
# conversations with unread messages
siggy --list | awk -F'\t' '$1 > 0'
# resolve a name to its id (phone number or group id)
siggy --list | awk -F'\t' '$3 == "Alice" { print $4 }'
```

### Stream incoming messages
```sh
siggy --receive
```
Streams incoming messages as they arrive, one tab-separated row each:
`timestamp_ms<TAB>sender<TAB>group-id<TAB>body` (group-id is empty for 1:1).
Outgoing/sync and attachment-only messages are skipped; bodies have control
characters stripped to stay one line each. Runs until signal-cli disconnects or
you interrupt with Ctrl-C, so wrap it with a timeout for bounded runs:

```sh
# capture 60 seconds of incoming messages
timeout 60 siggy --receive > inbox.tsv
# auto-reply to a keyword
siggy --receive | while IFS=$'\t' read -r ts from group body; do
  case "$body" in *ping*) siggy --send "$from" "pong";; esac
done
```

### Version
```sh
siggy --version   # or -V  -> "siggy <x.y.z>"
```

## Notes

- These commands do not launch the TUI and exit on their own (except `--receive`,
  which streams until interrupted).
- `--send`/`--receive` need signal-cli reachable and an account linked; gate
  scripts on `siggy --check` succeeding.
- Recipient ids: use `siggy --list` to find a contact's phone number or a group's
  id; the same value works as the `--send` recipient.
