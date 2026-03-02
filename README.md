# signal-tui

[![CI](https://github.com/johnsideserf/signal-tui/actions/workflows/ci.yml/badge.svg)](https://github.com/johnsideserf/signal-tui/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/johnsideserf/signal-tui)](https://github.com/johnsideserf/signal-tui/releases/latest)
[![License: GPL-3.0](https://img.shields.io/github/license/johnsideserf/signal-tui)](LICENSE)
[![Docs](https://img.shields.io/badge/docs-signal--tui-blue)](https://johnsideserf.github.io/signal-tui/)

A terminal-based Signal messenger client with an IRC aesthetic. Wraps [signal-cli](https://github.com/AsamK/signal-cli) via JSON-RPC for the messaging backend.

![signal-tui screenshot](screenshot.png)

## Install

### Pre-built binaries

Download the latest release for your platform from [Releases](https://github.com/johnsideserf/signal-tui/releases).

**Linux / macOS** (one-liner):

```sh
curl -fsSL https://raw.githubusercontent.com/johnsideserf/signal-tui/master/install.sh | bash
```

**Windows** (PowerShell):

```powershell
irm https://raw.githubusercontent.com/johnsideserf/signal-tui/master/install.ps1 | iex
```

Both scripts download the latest release binary and check for signal-cli.

### Build from source

Requires Rust 1.70+.

```sh
cargo install --git https://github.com/johnsideserf/signal-tui.git
```

Or clone and build locally:

```sh
git clone https://github.com/johnsideserf/signal-tui.git
cd signal-tui
cargo build --release
# Binary is at target/release/signal-tui
```

## Prerequisites

- [signal-cli](https://github.com/AsamK/signal-cli) installed and accessible on PATH (or configured via `signal_cli_path`)
- A Signal account linked as a secondary device (the setup wizard handles this)

## Usage

```sh
signal-tui                        # Launch (uses config file)
signal-tui -a +15551234567        # Specify account
signal-tui -c /path/to/config.toml  # Custom config path
signal-tui --setup                # Re-run first-time setup wizard
signal-tui --demo                 # Launch with dummy data (no signal-cli needed)
signal-tui --incognito            # No local message storage (in-memory only)
```

On first launch, the setup wizard guides you through locating signal-cli, entering your phone number, and linking your device via QR code.

## Configuration

Config is loaded from:
- **Linux/macOS:** `~/.config/signal-tui/config.toml`
- **Windows:** `%APPDATA%\signal-tui\config.toml`

```toml
account = "+15551234567"
signal_cli_path = "signal-cli"
download_dir = "/home/user/signal-downloads"
notify_direct = true
notify_group = true
inline_images = true
```

All fields are optional. `signal_cli_path` defaults to `"signal-cli"` (found via PATH), and `download_dir` defaults to `~/signal-downloads/`. On Windows, use the full path to `signal-cli.bat` if it isn't in your PATH.

## Features

- **Messaging** -- Send and receive 1:1 and group messages
- **Attachments** -- Image previews rendered inline as halfblock art; non-image attachments shown as `[attachment: filename]`
- **Clickable links** -- URLs and file paths are OSC 8 hyperlinks (clickable in terminals like Windows Terminal, iTerm2, etc.)
- **Typing indicators** -- Shows who is typing with contact name resolution
- **Message sync** -- Messages sent from your phone appear in the TUI
- **Persistence** -- SQLite message storage with WAL mode; conversations and read markers survive restarts
- **Unread tracking** -- Unread counts in sidebar with "new messages" separator in chat
- **Notifications** -- Terminal bell on new messages (configurable per direct/group, per-chat mute)
- **Contact resolution** -- Names from your Signal address book; groups auto-populated on startup
- **Message reactions** -- React with `r` in Normal mode; emoji picker with badge display (`👍 2 ❤️ 1`)
- **@mentions** -- Type `@` in group chats to mention members with autocomplete
- **Message selection** -- Focused message highlight when scrolling; `J`/`K` to jump between messages
- **Read receipts** -- Status symbols on outgoing messages (Sending → Sent → Delivered → Read → Viewed)
- **Setup wizard** -- First-run onboarding with QR code device linking
- **Vim keybindings** -- Modal editing (Normal/Insert) with full cursor movement
- **Command autocomplete** -- Tab-completion popup for slash commands
- **Settings overlay** -- Toggle notifications, sidebar, inline images from within the app
- **Responsive layout** -- Resizable sidebar that auto-hides on narrow terminals (<60 columns)
- **Incognito mode** -- `--incognito` uses in-memory storage; nothing persists after exit
- **Demo mode** -- Try the UI without signal-cli (`--demo`)

## Commands

| Command | Alias | Description |
|---|---|---|
| `/join <name>` | `/j` | Switch to a conversation by contact name, number, or group |
| `/part` | `/p` | Leave current conversation |
| `/sidebar` | `/sb` | Toggle sidebar visibility |
| `/bell [type]` | `/notify` | Toggle notifications (`direct`, `group`, or both) |
| `/mute` | | Mute/unmute current conversation |
| `/contacts` | `/c` | Browse synced contacts |
| `/settings` | | Open settings overlay |
| `/help` | `/h` | Show help overlay |
| `/quit` | `/q` | Exit signal-tui |

Type `/` to open the autocomplete popup. Use `Tab` to complete, arrow keys to navigate.

To message a new contact: `/join +15551234567` (E.164 format).

## Keyboard Shortcuts

The app uses vim-style modal editing with two modes: **Insert** (default) and **Normal**.

### Global (both modes)

| Key | Action |
|---|---|
| `Ctrl+C` | Quit |
| `Tab` / `Shift+Tab` | Next / previous conversation |
| `PgUp` / `PgDn` | Scroll messages (5 lines) |
| `Ctrl+Left` / `Ctrl+Right` | Resize sidebar |

### Normal mode

Press `Esc` to enter Normal mode.

| Key | Action |
|---|---|
| `j` / `k` | Scroll down / up 1 line |
| `J` / `K` | Jump to previous / next message |
| `Ctrl+D` / `Ctrl+U` | Scroll down / up half page |
| `g` / `G` | Scroll to top / bottom |
| `h` / `l` | Move cursor left / right |
| `w` / `b` | Word forward / back |
| `0` / `$` | Start / end of line |
| `x` | Delete character at cursor |
| `D` | Delete from cursor to end |
| `y` / `Y` | Copy message body / full line |
| `r` | React to focused message |
| `i` | Enter Insert mode |
| `a` | Enter Insert mode (cursor right 1) |
| `I` / `A` | Enter Insert mode at start / end of line |
| `o` | Enter Insert mode (clear buffer) |
| `/` | Enter Insert mode with `/` pre-typed |

### Insert mode (default)

| Key | Action |
|---|---|
| `Esc` | Switch to Normal mode |
| `Enter` | Send message / execute command |
| `Backspace` / `Delete` | Delete characters |
| `Up` / `Down` | Recall input history |
| `Left` / `Right` | Move cursor |
| `Home` / `End` | Jump to start / end of line |

## Architecture

```
Keyboard --> InputAction --> App state --> SignalClient (mpsc) --> signal-cli (JSON-RPC stdin/stdout)
signal-cli --> JsonRpcResponse --> SignalEvent (mpsc) --> App state --> SQLite + Ratatui render
```

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

Built with [Ratatui](https://ratatui.rs/) + [Crossterm](https://github.com/crossterm-rs/crossterm) on a [Tokio](https://tokio.rs/) async runtime.

## License

[GPL-3.0](LICENSE)
