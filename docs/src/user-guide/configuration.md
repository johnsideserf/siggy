# Configuration

## Config file location

signal-tui loads its config from a TOML file at the platform-specific path:

| Platform | Path |
|---|---|
| Linux / macOS | `~/.config/signal-tui/config.toml` |
| Windows | `%APPDATA%\signal-tui\config.toml` |

You can override the path with the `-c` flag:

```sh
signal-tui -c /path/to/config.toml
```

## Config fields

All fields are optional. Here is a complete example with defaults:

```toml
account = "+15551234567"
signal_cli_path = "signal-cli"
download_dir = "/home/user/signal-downloads"
notify_direct = true
notify_group = true
inline_images = true
send_read_receipts = true
```

### Field reference

| Field | Type | Default | Description |
|---|---|---|---|
| `account` | string | `""` | Phone number in E.164 format |
| `signal_cli_path` | string | `"signal-cli"` | Path to the signal-cli binary |
| `download_dir` | string | `~/signal-downloads/` | Directory for downloaded attachments |
| `notify_direct` | bool | `true` | Terminal bell on new direct messages |
| `notify_group` | bool | `true` | Terminal bell on new group messages |
| `inline_images` | bool | `true` | Render image attachments as halfblock art |
| `send_read_receipts` | bool | `true` | Send read receipts when viewing conversations |

## CLI flags

CLI flags override config file values for the current session:

| Flag | Overrides |
|---|---|
| `-a +15551234567` | `account` |
| `-c /path/to/config.toml` | Config file path |
| `--incognito` | Uses in-memory database (no persistence) |

## Settings overlay

Press `/settings` inside the app to open the settings overlay. This provides
toggles for runtime settings:

- Notification toggles (direct / group)
- Sidebar visibility
- Inline image previews
- Show read receipts / receipt colors / nerd font icons
- Verbose reactions
- Send read receipts

Changes made in the settings overlay are saved to the config file when you
close the overlay, and persist across sessions.

## Incognito mode

```sh
signal-tui --incognito
```

Incognito mode replaces the on-disk SQLite database with an in-memory database.
No messages, conversations, or read markers are saved. When you exit, all data is
gone. The status bar shows a bold magenta **incognito** indicator.
