# Changelog

## v0.7.0

### Text styling

- **Rich text rendering** -- messages with Signal formatting now display with
  proper styling: **bold**, *italic*, ~~strikethrough~~, `monospace`, and spoiler
  text. Spoiler content is hidden behind block characters (closes #66)

### Sticker messages

- **Sticker display** -- incoming stickers are now shown as `[Sticker: emoji]`
  in the chat area instead of being silently dropped (closes #67)

### View-once messages

- **View-once handling** -- view-once messages display as `[View-once message]`
  with attachments suppressed, respecting the ephemeral intent (closes #68)

### Cross-device read sync

- **Read state sync** -- when you read messages on your phone or another linked
  device, signal-tui marks those conversations as read and updates unread counts
  automatically (closes #71)

### System messages

- **Missed calls** -- missed voice and video calls now show as system messages
- **Safety number changes** -- a warning appears when a contact's safety number
  changes
- **Group updates** -- group metadata changes (member adds/removes) display as
  system messages
- **Disappearing message timer** -- changes to the expiration timer show a
  human-readable message (e.g. "Disappearing messages set to 1 day")

### Message action menu

- **Enter key menu** -- press Enter in Normal mode on a focused message to open
  a contextual action menu. Available actions (shown with key hints): Reply (q),
  Edit (e), React (r), Copy (y), Delete (d). Navigate with j/k, press Enter to
  execute, or use the shortcut key directly (closes #85)

### Bug fixes

- **"New messages" bar** -- the unread separator no longer persists after viewing
  a conversation with new messages (#90)

---

## v0.6.1

### Bug fixes

- **j/k scroll fixed** -- viewport no longer gets stuck when scrolling with
  `j`/`k`. The root cause was the message window expanding in lockstep with
  scroll offset, keeping the viewport position constant (#84)
- **J/K navigation in short conversations** -- `J`/`K` message jumping now
  works even when all messages fit the viewport (no scroll offset needed) (#84)
- **Edit preserves quotes** -- editing a quoted message no longer strips the
  original quote on remote clients. The wire-format phone number is now
  preserved through display name resolution (#84)
- **Contact names no longer revert to phone numbers** -- conversations would
  permanently show phone numbers in the sidebar when messages arrived before
  the contact list synced. Fixed by preventing phone-number fallback names
  from overwriting real display names in the database (#84)
- **Contact name recovery on startup** -- 1:1 conversations still named as
  phone numbers (e.g. when signal-cli's contact list has no cached profile
  name) now recover the correct name from stored message sender fields (#86)
- **Reaction sender names after reload** -- reaction senders no longer revert
  to phone numbers after restarting the app (#80)
- **Non-contact name resolution** -- display names for non-contacts in
  reactions and quotes are now resolved correctly (#83)
- **Mention placeholders in quotes** -- U+FFFC placeholder characters from
  @mentions are now stripped from quoted text (#79)

### Improvements

- **Loading screen** -- a loading indicator now appears during startup while
  contacts and groups sync from signal-cli (#81, #82)
- **Install scripts updated** -- Windows and macOS install scripts now
  reference Java 25+ (required by signal-cli 0.14). The Windows script
  checks the actual Java version before installing signal-cli (#87)

---

## v0.6.0

### Reply, edit, and delete messages

- **Quote reply** -- press `q` in Normal mode on any message to reply with a
  quote. A reply indicator appears above the input box, and the sent message
  includes a quoted block showing the original author and text (closes #15)
- **Edit messages** -- press `e` on your own outgoing message to edit it.
  The original text is loaded into the input buffer for modification. Edited
  messages display an "(edited)" label. Edits sync across devices (closes #24)
- **Delete messages** -- press `d` on any message to open a delete confirmation.
  Outgoing messages offer "delete for everyone" (remote delete) or "delete
  locally". Incoming messages can be deleted locally. Deleted messages show as
  "[deleted]" (closes #23)

### Message search

- **`/search` command** -- search across all conversations with `/search <query>`
  (alias: `/s`). Results appear in a scrollable overlay showing sender, message
  snippet, and conversation name. Press Enter to jump directly to the message in
  context. Use `n`/`N` in Normal mode to cycle through matches (closes #14)
- **Highlight matches** -- search terms are highlighted in the result snippets

### File attachments

- **`/attach` command** -- send files with `/attach` to open a file browser
  overlay. Navigate with `j`/`k`, Enter to select, Backspace to go up a
  directory. The selected file attaches to your next message, shown as a
  pending indicator in the input area (closes #54)

### /join autocomplete

- **Contact and group autocomplete** -- `/join` now offers Tab-completable
  suggestions from your contacts and groups. Type `/join ` and see matching
  names, or keep typing to filter. Groups and contacts are distinguished by
  color (closes #21)

### Send typing indicators

- **Outbound typing** -- signal-tui now sends typing indicators to your
  conversation partner while you type. Typing state starts on the first
  keypress, auto-stops after 5 seconds of inactivity, and stops immediately
  when you send or switch conversations (closes #58)

### Send read receipts

- **Read receipt sending** -- when you view a conversation, read receipts are
  automatically sent to message senders, letting them know you've read their
  messages. Controlled by the "Send read receipts" toggle in `/settings`
  (closes #59)

### Welcome screen

- **Getting started hints** -- the welcome screen now shows useful commands
  and navigation tips including Tab/Shift+Tab for cycling conversations

### Bug fixes

- **Out-of-order messages** -- messages with delayed delivery timestamps are
  now inserted in correct chronological order (#56)
- **Link highlight** -- fixed background color bleeding on highlighted links
  and J/K message navigation edge cases (#55)

### Database

- **Migration v5** -- adds index on `messages(conversation_id, timestamp_ms)`
  for faster search queries
- **Migration v6** -- adds `is_edited`, `is_deleted`, `quote_author`,
  `quote_body`, `quote_ts_ms`, and `sender_id` columns to the messages table

---

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
- **Configurable** -- three new settings toggles: "Show read receipts" (on/off),
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
