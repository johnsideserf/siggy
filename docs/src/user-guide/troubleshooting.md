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
2. Try re-running the setup wizard: `signal-tui --setup`
3. Check signal-cli can communicate by running it directly:
   ```sh
   signal-cli -a +15551234567 receive
   ```

## Images not rendering

**Symptom:** images show as `[attachment: image.jpg]` instead of inline previews.

**Fix:** make sure `inline_images = true` in your config (this is the default).
Also check that your terminal supports 256 colors or truecolor. Halfblock
rendering requires a terminal with proper Unicode support.

## Sidebar disappeared

**Symptom:** the sidebar is not visible.

**Fix:** if your terminal is narrower than 60 columns, the sidebar auto-hides.
Widen your terminal, or press `/sidebar` to force it on. You can also use
`Ctrl+Right` to widen the sidebar.

## Database errors

**Symptom:** errors about SQLite or the database file.

**Fix:** the database is stored alongside the config file. If it becomes corrupted,
you can delete it and signal-tui will create a fresh one on next launch. You'll
lose message history but all conversations will re-populate from signal-cli.

As a workaround, you can also run in incognito mode:

```sh
signal-tui --incognito
```
