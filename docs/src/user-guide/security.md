# Security

## Trust model

siggy is a **thin TUI layer** over [signal-cli](https://github.com/AsamK/signal-cli).
It does not implement any cryptographic protocols, manage credentials, or contact
Signal servers directly. All security-critical operations are delegated to signal-cli,
which implements the full Signal Protocol.

This means siggy inherits signal-cli's security posture:

- **What signal-cli handles**: key generation, key exchange, message encryption/decryption,
  identity verification, contact and group management, attachment encryption, and all
  network communication with Signal servers.
- **What siggy handles**: rendering messages in a terminal, storing a local cache of
  conversations in SQLite, and forwarding user actions to signal-cli via JSON-RPC.

siggy never sees plaintext cryptographic keys or raw network traffic.

### Credential storage

Signal credentials (identity keys, session keys, pre-keys) are stored by signal-cli
in its own data directory (`~/.local/share/signal-cli/` on Linux). **siggy does not
read, write, or manage these files.** If credential storage security is a concern,
it should be addressed at the signal-cli level or via OS-level protections (encrypted
home directory, restrictive file permissions).

## Encryption

### In transit

All messages are end-to-end encrypted using the Signal Protocol, handled entirely
by signal-cli. siggy communicates with signal-cli over a local stdin/stdout pipe
using JSON-RPC -- no network sockets are involved.

### At rest

Messages are stored **unencrypted** in a local SQLite database (`siggy.db`). This is
the same approach used by Signal Desktop and most other messaging clients. The
rationale is that local storage protection is best handled at the OS level
(full-disk encryption, screen lock, file permissions) rather than by individual
applications.

The database uses `PRAGMA secure_delete = ON`, which zeroes out deleted content in
the database file rather than leaving it recoverable in free pages.

### Files on disk

| File | Contents | Location |
|------|----------|----------|
| `siggy.db` | Message history, contacts, groups | Platform config directory |
| `siggy.db-wal` | Recent uncommitted writes | Same directory |
| `config.toml` | Phone number, settings | Platform config directory |
| `debug.log` | Debug output (opt-in, PII redacted by default) | `~/.cache/siggy/` |
| Download directory | Received attachments | `~/signal-downloads/` or configured path |

Platform config directories:
- **Linux / macOS**: `~/.config/siggy/`
- **Windows**: `%APPDATA%\siggy\`

## Privacy features

### Incognito mode

```sh
siggy --incognito
```

Uses an in-memory database instead of on-disk SQLite. No messages, conversations,
or read markers are written to disk. When you exit, everything is gone.

### Notification previews

Desktop notification content is configurable via the `notification_preview` setting
in `/settings`:

| Level | Title | Body |
|-------|-------|------|
| `full` (default) | Sender name | Message content |
| `sender` | Sender name | "New message" |
| `minimal` | "New message" | *(empty)* |

### Debug logging

Debug logging is **opt-in only** and disabled by default.

- `--debug` -- enables logging with **PII redaction**: phone numbers are masked
  (e.g. `+4***...567`), message bodies are replaced with `[msg: 42 chars]`, and
  contact/group lists show only counts.
- `--debug-full` -- enables logging with full unredacted output. Only use this
  when you need actual message content for troubleshooting, and delete the log
  file afterwards.

Debug logs are written to `~/.cache/siggy/debug.log` with 10 MB rotation. On
Unix systems, the log file and directory are created with restrictive permissions
(0600 / 0700).

### Clipboard

Copied message content is automatically cleared from the system clipboard after
30 seconds (configurable via `clipboard_clear_seconds` in config).

## Recommendations

- **Enable full-disk encryption** on your device (BitLocker, LUKS, FileVault).
  This is the single most effective protection for data at rest.
- **Use `--incognito` mode** for sensitive sessions where you don't want any
  messages persisted to disk.
- **Set `notification_preview = "sender"` or `"minimal"`** if you're concerned
  about notification content being visible on lock screens or in screen recordings.
- **Use a screen lock** to prevent physical access to your terminal session.
- **On shared systems**, restrict file permissions on the config directory
  (`chmod 700 ~/.config/siggy`).

## Reporting vulnerabilities

If you discover a security issue, please report it responsibly via
[GitHub Issues](https://github.com/johnsideserf/siggy/issues) or contact the
maintainer directly. We take security seriously and will respond promptly.
