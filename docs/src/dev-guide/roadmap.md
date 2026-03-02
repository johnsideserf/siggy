# Roadmap

## Completed

- [x] Send and receive plain text messages (1:1 and group)
- [x] Receive file attachments (displayed as `[attachment: filename]`)
- [x] Typing indicators
- [x] SQLite-backed message persistence with WAL mode
- [x] Unread message counts with persistent read markers
- [x] Vim-style modal editing (Normal / Insert modes)
- [x] Responsive layout with auto-hiding sidebar
- [x] First-run setup wizard with QR device linking
- [x] TUI error screens instead of stderr crashes
- [x] Commands: `/join`, `/part`, `/quit`, `/sidebar`, `/help`
- [x] Load contacts and groups on startup (name resolution, groups in sidebar)
- [x] Echo outgoing messages from other devices via sync messages
- [x] Contact name resolution from address book
- [x] Sync request at startup to refresh data from primary device
- [x] Inline image preview for attachments (halfblock rendering)
- [x] New message notifications (terminal bell, per-type toggles, per-chat mute)
- [x] Command autocomplete with Tab completion
- [x] Settings overlay
- [x] Input history (Up/Down to recall previous messages)
- [x] Incognito mode (`--incognito`)
- [x] Demo mode (`--demo`)
- [x] Delivery/read receipt display (status symbols on outgoing messages)
- [x] Contact list overlay (`/contacts`)
- [x] Copy to clipboard (`y`/`Y` in Normal mode)
- [x] Full timestamp on scroll (status bar shows date+time of focused message)
- [x] Message reactions (emoji picker, badge display, full lifecycle with DB persistence)
- [x] @mention autocomplete (type `@` in group or 1:1 chats)
- [x] Visible message selection (focus highlight, `J`/`K` message-level navigation)
- [x] Startup error handling (signal-cli stderr captured in TUI error screen)

## Up next

- [ ] **Send attachments** -- only receiving works today. Add a `/send-file <path>`
  command.
- [ ] **Message search** -- full-text search across conversations.

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
