# Changelog

## v0.5.0

### Message reactions

- **Emoji reactions** -- react to any message with `r` in Normal mode to
  open the reaction picker. Navigate with `h`/`l` or `1`-`8`, press
  Enter to send. Reactions display below messages as compact emoji
  badges (e.g. `👍 2 ❤️ 1`) with an optional verbose mode showing
  sender names (closes #16)
- **Reaction sync** -- incoming reactions, sync reactions from other
  devices, and reaction removals are all handled in real time
- **Persistence** -- reactions are stored in the database (migration v4)
  and restored on startup

### @mentions

- **Mention autocomplete** -- type `@` in group chats to open a member
  autocomplete popup. Filter by name, press Tab to insert the mention.
  Works in 1:1 chats too (with the conversation partner)
- **Mention display** -- incoming mentions are highlighted in cyan+bold
  in the chat area

### Visible message selection

- **Focus highlight** -- when scrolling in Normal mode, the focused
  message gets a subtle dark background highlight so you can see exactly
  which message reactions and copy will target
- **`J`/`K` navigation** -- Shift+j and Shift+k jump between actual
  messages, skipping date separators and system messages

### Startup error handling

- **stderr capture** -- signal-cli startup errors (missing Java, bad
  config, etc.) are now captured and displayed in a TUI error screen
  instead of silently failing

### Internal

- Major refactoring across four PRs (#45-#48): extracted shared key
  handlers, data-driven settings system, split `parse_receive_event`
  into sub-functions, modernized test helpers, added persistent debug
  log and pending_requests TTL

---

## v0.4.0

### Contact list

- **`/contacts` command** -- new overlay for browsing all synced contacts,
  with j/k navigation, type-to-filter by name or number, and Enter to
  open a conversation (alias: `/c`) (closes #22)

### Clipboard

- **Copy to clipboard** -- in Normal mode, `y` copies the selected
  message body and `Y` copies the full formatted line
  (`[HH:MM] <sender> body`) to the system clipboard (closes #28)

### Navigation

- **Full timestamp on scroll** -- when scrolling through messages in
  Normal mode, the status bar now shows the full date and time of the
  focused message (e.g. "Sun Mar 01, 2026 12:34:56 PM") (closes #27)

---

## v0.3.3

### Bug fixes

- **Settings persistence** -- changes made in `/settings` are now saved
  to the config file and persist between sessions (fixes #40)
- **Input box scrolling** -- long messages no longer disappear when
  typing past the edge of the input box; text now scrolls horizontally
  to keep the cursor visible (fixes #39)
- **Image preview refresh** -- toggling "Inline image previews" in
  `/settings` now immediately re-renders or clears previews on existing
  messages (fixes #41)

### Settings

- **Tab to toggle** -- Tab key now toggles settings items in the
  `/settings` overlay, alongside Space and Enter

---

## v0.3.2

### Read receipts and delivery status

- **Message status indicators** -- outgoing messages now show delivery
  lifecycle symbols: `◌` Sending → `○` Sent → `✓` Delivered → `●` Read
  → `◉` Viewed
- **Real-time updates** -- status symbols update live as recipients
  receive and read your messages
- **Group receipt support** -- delivery and read receipts work correctly
  in group conversations
- **Race condition handling** -- receipts that arrive before the server
  confirms the send are buffered and replayed automatically
- **Persistent status** -- message status is stored in the database and
  restored on reload (stale "Sending" messages are promoted to "Sent")
- **Nerd Font icons** -- optional Nerd Font glyphs available via
  `/settings` > "Nerd Font icons"
- **Configurable** -- three new settings toggles: "Read receipts" (on/off),
  "Receipt colors" (colored/monochrome), "Nerd Font icons" (unicode/nerd)

### Debug logging

- **`--debug` flag** -- opt-in protocol logging to `signal-tui-debug.log`
  for diagnosing signal-cli communication issues

### Database

- **Migration v3** -- adds `status` and `timestamp_ms` columns to the
  messages table (automatic on first run)

---

## v0.3.1

### Image attachments

- **Embedded file links** -- attachment URIs are now hidden behind clickable
  bracket text (e.g. `[image: photo.jpg]`) instead of showing the raw
  `file:///` path
- **Double extension fix** -- filenames like `photo.jpg.jpg` are stripped to
  `photo.jpg` when signal-cli duplicates the extension
- **Improved halfblock previews** -- increased height cap from 20 to 30
  cell-rows for better inline image quality
- **Native image protocols** -- experimental support for Kitty and iTerm2
  inline image rendering, off by default. Enable via `/settings` >
  "Native images (experimental)"
- **Pre-resized encoding** -- native protocol images are resized and cached
  as PNG before sending to the terminal, avoiding multi-megabyte raw file
  transfers every frame

### Attachment lookup

- **MSYS/WSL path fix** -- `find_signal_cli_attachment` now checks both
  platform-native data dirs (`AppData/Roaming`) and POSIX-style
  (`~/.local/share`) where signal-cli stores files under MSYS or WSL.
  Fixes outgoing images sent from Signal desktop not displaying in the TUI.

### Platform

- **Windows Ctrl+C fix** -- suppress the `STATUS_CONTROL_C_EXIT` error on
  exit by disabling the default Windows console handler (crossterm already
  captures Ctrl+C as a key event in raw mode)

### Documentation

- mdBook documentation site with custom mIRC/Win95 light theme and dark mode
  toggle

---

## v0.3.0

Initial public release.

- Terminal Signal client wrapping signal-cli via JSON-RPC
- Vim-style modal input (Normal/Insert modes)
- Sidebar with conversation list, unread counts, typing indicators
- Inline halfblock image previews
- OSC 8 clickable hyperlinks
- SQLite persistence with WAL mode
- Incognito mode (`--incognito`)
- Demo mode (`--demo`)
- First-run setup wizard with QR device linking
- Slash commands: `/join`, `/part`, `/quit`, `/sidebar`, `/help`, `/settings`,
  `/mute`, `/notify`, `/bell`
- Input history (Up/Down recall)
- Autocomplete popup for commands and @mentions
- Configurable notifications (direct/group) with terminal bell
- Cross-platform: Linux, macOS, Windows
