# Roadmap

## Completed

- [x] Send and receive plain text messages (1:1 and group)
- [x] Receive file attachments (displayed as `[attachment: filename]`)
- [x] Typing indicators (receive and send)
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
- [x] Reply to specific messages (quote reply with `q` key)
- [x] Edit own messages (`e` key, "(edited)" label, cross-device sync)
- [x] Delete messages (`d` key, remote delete + local delete)
- [x] Message search (`/search`, `n`/`N` navigation)
- [x] Send file attachments (`/attach` command with file browser)
- [x] `/join` autocomplete (contacts and groups with Tab completion)
- [x] Send typing indicators (auto-start/stop on keypress)
- [x] Send read receipts (automatic on conversation view, configurable)
- [x] System messages (missed calls, safety number changes, group updates, expiration timer)
- [x] Message action menu (Enter in Normal mode, contextual actions on focused message)
- [x] Text styling (bold, italic, strikethrough, monospace, spoiler rendering)
- [x] Display stickers (shown as `[Sticker: emoji]` in chat)
- [x] View-once messages (shown as `[View-once message]` placeholder)
- [x] Cross-device read sync (sync read state across linked devices)
- [x] Disappearing messages (honor timers, countdown display, `/disappearing` command)
- [x] Group management (`/group` command: view/add/remove members, rename, create, leave)
- [x] Message requests (detect unknown senders, accept/delete with banner UI)
- [x] Block/unblock contacts (`/block`, `/unblock` commands)
- [x] Mouse support (click sidebar, scroll messages, click input bar, overlay scroll)
- [x] Color themes (selectable themes via `/theme` or `/settings`)
- [x] Desktop notifications (OS-native via `notify-rust`, configurable toggle)

## Future

### Medium priority

- [ ] Link previews -- display URL previews for shared links (#63)
- [ ] Polls -- create and vote in Signal polls (#64)
- [ ] Pinned messages -- view and manage pinned messages (#65)

### Low priority

- [ ] Publish to crates.io (#11)
- [ ] Update profile from TUI (#69)
- [ ] Identity key verification (#70)
- [ ] Multi-line message input (Shift+Enter for newlines)
- [ ] Message history pagination (scroll-up to load older messages)
- [ ] Scroll position memory per conversation
- [ ] Configurable keybindings
- [ ] Forward messages
