# signal-tui Roadmap

## Completed

- [x] Send and receive plain text messages (1:1 and group)
- [x] Receive file attachments (displayed as `[attachment: filename]`)
- [x] Typing indicators
- [x] SQLite-backed message persistence with WAL mode
- [x] Unread message counts with persistent read-marker
- [x] Vim-style modal editing (Normal/Insert modes)
- [x] Responsive layout with auto-hiding sidebar
- [x] First-run setup wizard with QR device linking
- [x] TUI error screens instead of stderr crashes
- [x] Commands: `/join`, `/part`, `/quit`, `/sidebar`, `/help`

- [x] Load contacts & groups on startup (name resolution + groups in sidebar)
- [x] Echo outgoing messages from other devices via sync messages
- [x] Contact name resolution from address book
- [x] Sync request at startup to refresh data from primary device
- [x] Inline image preview for attachments (halfblock rendering)

- [x] **New message notifications**
  - Terminal bell + unread count in terminal title
  - Separate toggles for direct and group messages (config + `/bell` command)
  - Per-conversation `/mute` with DB persistence
  - Setup wizard preferences step

- [x] **Delivery/read receipt display**
  - Status symbols on outgoing messages (Sending → Sent → Delivered → Read → Viewed)
  - Configurable via /settings (receipts, colors, Nerd Font icons)
  - Optional `--debug` flag for protocol diagnostics

- [x] **Contact list overlay**
  - `/contacts` (alias `/c`) command to browse synced contacts
  - Type-to-filter, j/k navigation, Enter to open conversation

- [x] **Copy to clipboard**
  - `y` copies message body, `Y` copies full formatted line
  - Cross-platform via arboard crate

- [x] **Full timestamp on scroll**
  - Status bar shows full date+time of focused message when scrolling

- [x] **Message reactions**
  - Emoji picker (`r` in Normal mode) with quick-react bar
  - Compact badge display (`👍 2 ❤️ 1`) with optional verbose mode
  - Full lifecycle: receive, sync, remove, persist (DB migration v4)

- [x] **@mention autocomplete**
  - Type `@` in group chats to mention members; also works in 1:1 chats
  - Incoming mentions highlighted in cyan+bold

- [x] **Visible message selection**
  - Dark background highlight on focused message when scrolling
  - `J`/`K` to jump between messages (skips separators and system messages)

- [x] **Startup error handling**
  - signal-cli stderr captured and displayed in TUI error screen

## Up Next

- [ ] **Send attachments**
  - Only receiving works today
  - Add `/send-file <path>` command

- [ ] **Message search**
  - Full-text search across conversations

## Future

- [ ] Multi-line message input (Shift+Enter for newlines)
- [ ] Message history pagination (scroll-up to load older messages)
- [ ] Correct group typing indicators (per-sender-to-group mapping)
- [ ] Message deletion and editing
- [ ] Group management (create, add/remove members, member list)
- [ ] Scroll position memory per conversation
- [ ] Configurable keybindings
- [ ] Reply to specific messages (quote reply)
- [ ] Forward messages
