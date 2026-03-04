# Architecture

## Overview

siggy is a terminal Signal client that wraps
[signal-cli](https://github.com/AsamK/signal-cli) via JSON-RPC over stdin/stdout.
It is built on a Tokio async runtime with Ratatui for rendering.

```
+------------+   mpsc channels   +----------------+
|  TUI       | <---------------> |  Signal        |
|  (main     |   SignalEvent     |  Backend       |
|  thread)   |   UserCommand     |  (tokio task)  |
+------------+                   +--------+-------+
                                          |
                                   stdin/stdout
                                          |
                                 +--------v-------+
                                 |  signal-cli    |
                                 |  (child proc)  |
                                 +----------------+
```

## Async runtime

The application uses a **multi-threaded Tokio runtime** (via `#[tokio::main]`).
The main thread runs the TUI event loop. signal-cli communication happens in
spawned Tokio tasks that communicate back to the main thread via
`tokio::sync::mpsc` channels.

## Event loop

The main loop in `main.rs` runs on a 50ms tick:

1. **Poll keyboard** -- check for key events via Crossterm (non-blocking, 50ms timeout)
2. **Drain signal events** -- process all pending `SignalEvent` messages from the mpsc channel
3. **Render** -- call `ui::draw()` with the current `App` state

This keeps the UI responsive while processing backend events as they arrive.

## Startup sequence

1. Load config from TOML (or defaults)
2. Check if setup is needed (`account` field empty)
3. If needed: run the setup wizard (signal-cli detection, phone input, QR linking)
4. Open SQLite database (or in-memory for `--incognito`)
5. Spawn signal-cli child process
6. Load conversations and contacts from database + signal-cli
7. Enter the main event loop

## Key dependencies

| Crate | Purpose |
|---|---|
| `ratatui` 0.29 | Terminal UI framework |
| `crossterm` 0.28 | Cross-platform terminal I/O |
| `tokio` 1.x | Async runtime |
| `serde` / `serde_json` | JSON serialization for signal-cli RPC |
| `rusqlite` 0.32 | SQLite database (bundled) |
| `chrono` 0.4 | Timestamp handling |
| `qrcode` 0.14 | QR code generation for device linking |
| `image` 0.25 | Image decoding for inline previews |
| `anyhow` 1.x | Error handling |
| `toml` 0.8 | Config file parsing |
| `dirs` 6.x | Platform-specific directory paths |
| `uuid` 1.x | RPC request ID generation |
