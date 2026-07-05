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
- [x] Link previews (URL preview cards with title, description, thumbnail)
- [x] Polls (create with `/poll`, vote overlay, inline bar charts)
- [x] Pinned messages (pin/unpin with `p`, duration picker, banner display)
- [x] Identity key verification (`/verify` overlay with trust management)
- [x] Profile editor (`/profile` overlay for Signal profile fields)
- [x] About overlay (`/about` command showing app info)
- [x] Sidebar position setting (left or right placement)
- [x] Publish to crates.io (`cargo install siggy`)
- [x] Rename to siggy (auto-migration from signal-tui paths)

- [x] Forward messages (`f` key, filterable picker overlay)
- [x] Scroll position memory per conversation
- [x] Multi-line message input (Alt+Enter / Shift+Enter for newlines)
- [x] Message history pagination (scroll-up to load older messages)
- [x] Configurable keybindings (profiles, in-app rebinding, TOML overrides)
- [x] Export chat history (`/export` to plain text, Markdown, or JSON)
- [x] Sidebar filter (type-to-filter from Normal mode)
- [x] Jump to quoted message (`Q` to jump, `Ctrl+O` to jump back)
- [x] Delete conversations (`/delete` removes locally + declines message requests)
- [x] Session lock + boss key (`Ctrl-L`, `/lock`, `/lock-reset`, `--reset-lock`)
- [x] Native inline images inside tmux (DCS passthrough wrapping, `SIGGY_IMAGE_PROTOCOL` override) -- thanks @cultlead3r
- [x] Sixel image protocol (Windows Terminal and other Sixel terminals)
- [x] Open attachments and URLs from the action menu
- [x] Multiple concurrent typing indicators per group
- [x] Enhanced emoji shortcode coverage -- thanks @shwoop
- [x] Custom themes (drop-in TOML files + shipped template)
- [x] Auto-lock idle timer (`lock_timeout` config)
- [x] Bindable command actions (`open_contacts`, `open_settings`, `open_help`, `toggle_sidebar`, `attach`)
- [x] Non-interactive CLI for scripting (`--version`, `--check`, `--send`, `--list`, `--receive`)
- [x] Claude Code skill for the siggy CLI
- [x] Scheduled messages via the OS scheduler (documented pattern)
- [x] Side-by-side accounts via `db_path` config override
- [x] Supervised signal-cli reconnect with backoff
- [x] README translations (11 languages)
- [x] Voice message playback (inline via a detected CLI player, `audio_player` override)
- [x] Compose text formatting (`*bold*`, `_italic_`, `~strike~`, `` `mono` ``, `||spoiler||`)
- [x] Spoiler reveal on focus (`J`/`K` unmasks while focused)
- [x] Archive conversations + mark-as-unread (`/archive`, `/unread`)

## Future

Tracked in GitHub issues:

- [Outgoing link previews (#267)](https://github.com/johnsideserf/siggy/issues/267) -- generate preview cards for links you send (incoming previews already render)
- [Render sticker images inline (#610)](https://github.com/johnsideserf/siggy/issues/610)
- [Signal username support (#612)](https://github.com/johnsideserf/siggy/issues/612)
- [Fuzzy command palette (#614)](https://github.com/johnsideserf/siggy/issues/614)
- [Scriptable message triggers / auto-responder (#615)](https://github.com/johnsideserf/siggy/issues/615)
- [Voice message duration + progress indicator (#618)](https://github.com/johnsideserf/siggy/issues/618)

Have an idea? [Open an issue](https://github.com/johnsideserf/siggy/issues).
