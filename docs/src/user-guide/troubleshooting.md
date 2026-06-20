# Troubleshooting

## signal-cli not found

**Symptom:** setup wizard says it cannot find signal-cli.

**Fix:** ensure signal-cli is installed and on your `PATH`. You can also set the
full path in your config:

```toml
signal_cli_path = "/usr/local/bin/signal-cli"
```

On Windows, use the full path to `signal-cli.bat`.

## QR code doesn't display properly

**Symptom:** the QR code appears garbled or too large during device linking.

**Fix:** make sure your terminal is at least 60 columns wide and supports Unicode
block characters. Try a modern terminal emulator like Windows Terminal, iTerm2,
Kitty, or Alacritty.

## "Java not found" or class version errors

**Symptom:** signal-cli fails to start with Java-related errors, or you see
`UnsupportedClassVersionError` mentioning "class file version 69.0".

**Fix:** signal-cli 0.14+ requires Java 25+. Install a compatible JDK:

```sh
# Windows
winget install EclipseAdoptium.Temurin.25.JDK

# macOS
brew install --cask temurin@25

# Or download from https://adoptium.net/
```

Verify with `java -version` -- you should see version 25 or higher. On Linux,
the install script uses the native signal-cli build which does not require Java.

## Messages not appearing

**Symptom:** the app starts but no messages show up.

**Fix:**
1. Check that your device is properly linked in Signal's settings on your phone
   (**Settings > Linked Devices**)
2. Try re-running the setup wizard: `siggy --setup`
3. Check signal-cli can communicate by running it directly:
   ```sh
   signal-cli -a +15551234567 receive
   ```

## Messages from other devices aren't syncing

**Symptom:** messages you send or receive on your phone (or another linked
device) while siggy is closed do not show up when you reopen siggy, and the
status bar briefly shows a signal-cli error such as `getServerGuid(...) must
not be null` or `Method not implemented`.

**Why:** siggy receives messages only while it is running, because it runs
signal-cli as a child process. Nothing syncs while siggy is closed. On reopen,
signal-cli drains the messages the server queued for it. If signal-cli is
outdated or its session has drifted, it can fail to process some of those
queued messages, and they never reach siggy.

**Fix:**

1. Upgrade signal-cli to the latest version. siggy uses newer signal-cli
   features, and an older build is also more likely to fail decrypting queued
   messages. Check your version with `signal-cli --version`.
2. If it persists, re-link the device by re-running `siggy --setup`. Re-linking
   resets the session. Note that it does not back-fill history, and messages
   that were already dropped will not reappear.
3. Linked devices only receive while connected, so for uninterrupted history
   keep siggy (or another always-on signal-cli client) running.

## Images not rendering

**Symptom:** images show as `[attachment: image.jpg]` instead of inline previews.

**Fix:** make sure `image_mode` in your config is not set to `"none"`. The
default `"halfblock"` works in any terminal with truecolor / 256-color and
proper Unicode support. `"native"` uses the Kitty / iTerm2 graphics protocol
where supported, with automatic fallback otherwise.

## Native images render as halfblock inside tmux

**Symptom:** outside tmux, image attachments render as actual pixels. Inside
tmux they fall back to halfblock even though the outer terminal supports a
native protocol.

**Why:** tmux strips Kitty (`ESC _G...`) and iTerm2 (`ESC ]1337;...`) escapes
unless they are wrapped in tmux's DCS passthrough envelope, and
`TERM_PROGRAM` becomes `tmux` so auto-detection cannot see the outer terminal.

**Fix:** two steps. First, in your `~/.tmux.conf`:

```
set -g allow-passthrough on
```

(Older tmux uses `set -g allow-passthrough all`. Requires tmux 3.3+.) Then
launch siggy with an explicit protocol override that names the outer
terminal:

```sh
SIGGY_IMAGE_PROTOCOL=kitty siggy        # or iterm2 / sixel / halfblock
```

Sixel passes through tmux 3.4+ natively and does not need the env var.

## Sidebar disappeared

**Symptom:** the sidebar is not visible.

**Fix:** if your terminal is narrower than 60 columns, the sidebar auto-hides.
Widen your terminal, or press `/sidebar` to force it on. You can also use
`Ctrl+Right` to widen the sidebar.

## Database errors

**Symptom:** errors about SQLite or the database file.

**Fix:** the database is stored alongside the config file. If it becomes corrupted,
you can delete it and siggy will create a fresh one on next launch. You'll
lose message history but all conversations will re-populate from signal-cli.

As a workaround, you can also run in incognito mode:

```sh
siggy --incognito
```
