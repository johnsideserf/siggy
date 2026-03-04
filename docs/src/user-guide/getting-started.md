# Getting Started

## First launch

Run siggy with no arguments:

```sh
siggy
```

If no config file exists, the **setup wizard** starts automatically.

## Setup wizard

The wizard walks through three steps:

1. **Locate signal-cli** -- siggy searches your `PATH` for `signal-cli`. If it
   can't find it, you'll be prompted to enter the full path.

2. **Enter your phone number** -- provide your Signal phone number in E.164 format
   (e.g. `+15551234567`). This is the account siggy will connect to.

3. **Link your device** -- a QR code is displayed in the terminal. Scan it with the
   Signal app on your phone:
   - Open Signal on your phone
   - Go to **Settings > Linked Devices > Link New Device**
   - Scan the QR code shown in the terminal

Once linked, siggy saves your config and starts the main interface.

## Re-running setup

To re-run the setup wizard at any time:

```sh
siggy --setup
```

This is useful if you need to link a different account or reconfigure signal-cli.

## Demo mode

Try the full UI without a Signal account or signal-cli:

```sh
siggy --demo
```

Demo mode populates the interface with dummy conversations and messages. It's useful
for exploring keybindings, commands, and the layout before committing to setup.

## CLI options

| Flag | Description |
|---|---|
| `-a`, `--account <NUMBER>` | Phone number in E.164 format (overrides config) |
| `-c`, `--config <PATH>` | Path to a custom config file |
| `--setup` | Re-run the first-time setup wizard |
| `--demo` | Launch with dummy data (no signal-cli needed) |
| `--incognito` | In-memory storage only; nothing persists after exit |

## Basic navigation

Once launched, the interface has three areas:

- **Sidebar** (left) -- lists your conversations; groups are prefixed with `#`
- **Chat area** (center) -- shows messages for the selected conversation
- **Input bar** (bottom) -- type messages and commands here

Use `Tab` / `Shift+Tab` to switch between conversations, or type `/join <name>` to
jump to a specific contact or group.

Press `Esc` to enter Normal mode for vim-style scrolling and navigation. The default
mode is Insert, where you can type messages immediately.
