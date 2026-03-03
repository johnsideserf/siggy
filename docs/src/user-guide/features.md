# Features

## Messaging

Send and receive 1:1 and group messages. Messages sent from your phone (or other
linked devices) sync into the TUI automatically.

## Attachments

- **Images** -- rendered inline as halfblock art when `inline_images = true`
- **Other files** -- shown as `[attachment: filename]` with the download path
- **Send files** -- use `/attach` to open a file browser and attach a file to
  your next message

Received attachments are saved to the `download_dir` configured in your config file
(default: `~/signal-downloads/`).

## Clickable links

URLs and file paths in messages are rendered as
[OSC 8 hyperlinks](https://gist.github.com/egmontkob/eb114294efbcd5adb1944c9f3cb5feda).
In supported terminals (Windows Terminal, iTerm2, Kitty, etc.), you can click them
to open in your browser.

## Typing indicators

When someone is typing, their name appears below the chat area. Contact name
resolution is used where available. signal-tui also sends typing indicators to
your conversation partners while you type, so they can see when you're composing
a message.

## Persistence

All conversations, messages, and read markers are stored in a SQLite database with
WAL (Write-Ahead Logging) mode for safe concurrent access. Data survives app restarts.

The database is stored alongside the config file:
- **Linux / macOS:** `~/.config/signal-tui/signal-tui.db`
- **Windows:** `%APPDATA%\signal-tui\signal-tui.db`

## Unread tracking

The sidebar shows unread counts next to each conversation. When you open a
conversation, a "new messages" separator line marks where you left off. Read
markers persist across restarts.

## Notifications

Terminal bell notifications fire when new messages arrive in background
conversations. Configure them per type:

- `notify_direct` -- 1:1 messages (default: on)
- `notify_group` -- group messages (default: on)
- `/mute` -- per-conversation mute (persists in the database)
- `/bell` -- toggle notification types at runtime

## Contact resolution

On startup, signal-tui requests your contact list and group list from signal-cli.
Names from your Signal address book are used throughout the sidebar, chat area,
and typing indicators.

## Responsive layout

The sidebar auto-hides on narrow terminals (less than 60 columns). Use
`Ctrl+Left` / `Ctrl+Right` to resize it, or `/sidebar` to toggle it.

## Incognito mode

```sh
signal-tui --incognito
```

Uses an in-memory database instead of on-disk SQLite. No messages, conversations,
or read markers are written to disk. The status bar shows a bold magenta
**incognito** indicator. When you exit, everything is gone.

## Message reactions

React to any message with `r` in Normal mode to open the emoji picker. Navigate
with `h`/`l` or press `1`-`8` to jump directly, then Enter to send.

Reactions display below messages as compact badges:

```
👍 2  ❤️ 1
```

Enable "Verbose reactions" in `/settings` to show sender names instead of counts.
Reactions sync across devices and persist in the database.

## @mentions

In group chats, type `@` to open a member autocomplete popup. Filter by name and
press Tab to insert the mention. Works in 1:1 chats too (with the conversation
partner). Incoming mentions are highlighted in cyan+bold.

## Visible message selection

When scrolling in Normal mode, the focused message gets a subtle dark background
highlight. This makes it clear which message `r` (react) and `y`/`Y` (copy) will
target. Use `J`/`K` (Shift+j/k) to jump between messages, skipping date
separators and system messages.

## Reply, edit, and delete

In Normal mode, act on the focused message:

- **`q` -- Quote reply** -- reply with a quoted block showing the original
  message. A reply indicator appears above your input while composing.
- **`e` -- Edit** -- edit your own outgoing messages. The original text is
  loaded into the input buffer. Edited messages display "(edited)".
- **`d` -- Delete** -- delete a message. Outgoing messages offer "delete for
  everyone" (remote delete) or "delete locally". Incoming messages can be
  deleted locally. Deleted messages show as "[deleted]".

All three features sync across devices and persist in the database.

## Message search

Use `/search <query>` (alias `/s`) to search across all conversations. Results
appear in a scrollable overlay with sender, snippet, and conversation name.
Press Enter to jump to the message in context.

After searching, use `n`/`N` in Normal mode to cycle through matches without
re-opening the overlay.

## Text styling

Signal formatting is rendered in the chat area:

- **Bold** -- displayed with terminal bold
- **Italic** -- displayed with terminal italic
- **Strikethrough** -- displayed with terminal strikethrough
- **Monospace** -- displayed in gray
- **Spoiler** -- hidden behind block characters (`████`)

Styles compose correctly with @mentions and link highlighting.

## Sticker messages

Incoming stickers display as `[Sticker: emoji]` in the chat area (e.g.
`[Sticker: 👍]`). If the sticker has no associated emoji, it shows as
`[Sticker]`.

## View-once messages

View-once messages display as `[View-once message]` with any attachments
suppressed, respecting the sender's ephemeral intent.

## System messages

Certain Signal events display as system messages (dimmed, centered) in the chat:

- **Missed calls** -- "Missed voice call" / "Missed video call"
- **Safety number changes** -- warning when a contact's safety number changes
- **Group updates** -- group metadata changes (member adds/removes)
- **Disappearing message timer** -- e.g. "Disappearing messages set to 1 day"

## Message action menu

Press `Enter` in Normal mode on a focused message to open a contextual action
menu. Available actions depend on the message type:

| Action | Key | Available on |
|---|---|---|
| Reply | `q` | Non-deleted messages |
| Edit | `e` | Your own outgoing messages |
| React | `r` | All messages |
| Copy | `y` | All messages |
| Delete | `d` | Non-deleted messages |

Navigate with `j`/`k`, press Enter to execute, or press the shortcut key
directly. Press `Esc` to close.

## Read receipts

signal-tui sends read receipts to message senders when you view a conversation,
letting them know you've read their messages. This can be toggled off via
`/settings` > "Send read receipts".

## Cross-device read sync

When you read messages on your phone or another linked device, signal-tui
receives the read sync and marks those conversations as read. Unread counts
update automatically.

## Demo mode

```sh
signal-tui --demo
```

Launches with dummy conversations and messages. No signal-cli process is spawned.
Useful for testing the UI, exploring keybindings, and taking screenshots.
