# Module Reference

signal-tui is organized into a flat module structure under `src/`.

## Source files

### `main.rs`

Entry point. Parses CLI arguments, runs the setup wizard if needed, opens the
database, spawns signal-cli, and runs the main event loop. Orchestrates the
startup sequence: setup wizard -> device linking -> app startup.

The event loop polls keyboard input (50ms timeout), drains signal events from
the mpsc channel, and renders each frame with `ui::draw()`.

### `app.rs`

All application state lives in the `App` struct. Owns conversations (stored in
a `HashMap` with an ordered `Vec` for sidebar ordering), the input buffer, and
the current mode (Normal / Insert).

Key entry point: `handle_signal_event()` processes all backend events -- incoming
messages, typing indicators, contact lists, group lists, and errors. This is the
single place where signal-cli events modify application state.

`get_or_create_conversation()` is the single point for ensuring a conversation
exists. It upserts to both the in-memory `HashMap` and SQLite. New conversations
append to `conversation_order`; existing ones are no-ops.

### `signal/client.rs`

Spawns the signal-cli child process and manages communication. Two Tokio tasks:

- **stdout reader** -- reads lines from signal-cli stdout, parses JSON-RPC into
  `SignalEvent` variants, and sends them through the mpsc channel
- **stdin writer** -- receives `JsonRpcRequest` structs and writes them as JSON
  lines to signal-cli stdin

The `pending_requests` map tracks RPC call IDs to correlate responses with their
original method (e.g., mapping a response ID back to `listContacts`).

### `signal/types.rs`

Shared types for signal-cli communication:

- `SignalEvent` -- enum of all events the backend can produce
- `SignalMessage` -- a message with source, timestamp, body, attachments, group info
- `Attachment` -- file metadata (content type, filename, local path)
- `JsonRpcRequest` / `JsonRpcResponse` -- JSON-RPC protocol structs
- `Contact` / `Group` -- address book and group info

### `ui.rs`

Stateless rendering. The `draw()` function takes an immutable `&App` reference and
renders the full UI: sidebar, chat area, input bar, and status bar.

Sender colors are hash-based (8 colors). Groups are prefixed with `#` in the sidebar.
OSC 8 hyperlinks are injected in a post-render pass (written directly to the terminal
after Ratatui's draw to avoid width calculation issues).

### `db.rs`

SQLite database layer with WAL mode. Four tables: `conversations`, `messages`,
`read_markers`, `reactions`. Schema migration is version-based (currently at v6,
see [Database Schema](database.md)).

Provides `open()` for disk-backed storage and `open_in_memory()` for incognito mode.

### `config.rs`

TOML configuration. The `Config` struct is serialized/deserialized with serde.
Fields: `account`, `signal_cli_path`, `download_dir`, `notify_direct`,
`notify_group`, `inline_images`, `send_read_receipts`. All fields have defaults.

`Config::load()` reads from the platform-specific path (or a custom path).
`Config::save()` writes the current config back to disk.

### `input.rs`

Input parsing. Converts text input into an `InputAction` enum. Handles all
slash commands (`/join`, `/part`, `/quit`, `/sidebar`, `/bell`, `/mute`,
`/contacts`, `/settings`, `/search`, `/attach`, `/help`) and their aliases.

Also defines `CommandInfo` and the `COMMANDS` constant used for autocomplete.

### `setup.rs`

Multi-step first-run wizard. Handles signal-cli detection (searching PATH),
phone number input with validation, and triggers the device linking flow.

### `link.rs`

Device linking flow. Runs signal-cli's `link` command, captures the QR code URI,
renders it in the terminal, and waits for the user to scan it with their phone.
Checks for successful account registration afterward.
