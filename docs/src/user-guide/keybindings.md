# Keybindings

signal-tui uses vim-style modal editing with two modes: **Insert** (default) and
**Normal**.

## Global (both modes)

| Key | Action |
|---|---|
| `Ctrl+C` | Quit |
| `Tab` / `Shift+Tab` | Next / previous conversation |
| `PgUp` / `PgDn` | Scroll messages (5 lines) |
| `Ctrl+Left` / `Ctrl+Right` | Resize sidebar |

## Normal mode

Press `Esc` to enter Normal mode. The cursor stops blinking and the mode indicator
changes in the status bar.

### Scrolling

| Key | Action |
|---|---|
| `j` / `k` | Scroll down / up 1 line |
| `J` / `K` | Jump to previous / next message |
| `Ctrl+D` / `Ctrl+U` | Scroll down / up half page |
| `g` / `G` | Scroll to top / bottom |

### Actions

| Key | Action |
|---|---|
| `y` | Copy message body to clipboard |
| `Y` | Copy full line (`[HH:MM] <sender> body`) to clipboard |
| `r` | Open reaction picker on focused message |
| `@` | Mention autocomplete (in Insert mode) |

### Cursor movement

| Key | Action |
|---|---|
| `h` / `l` | Move cursor left / right |
| `w` / `b` | Word forward / back |
| `0` / `$` | Start / end of line |

### Editing

| Key | Action |
|---|---|
| `x` | Delete character at cursor |
| `D` | Delete from cursor to end of line |

### Entering Insert mode

| Key | Action |
|---|---|
| `i` | Insert at cursor |
| `a` | Insert after cursor |
| `I` | Insert at start of line |
| `A` | Insert at end of line |
| `o` | Insert (clear buffer first) |
| `/` | Insert with `/` pre-typed (for commands) |

## Insert mode (default)

Insert mode is the default on startup. You can type messages and commands directly.

| Key | Action |
|---|---|
| `Esc` | Switch to Normal mode |
| `Enter` | Send message or execute command |
| `Backspace` / `Delete` | Delete characters |
| `Up` / `Down` | Recall input history |
| `Left` / `Right` | Move cursor |
| `Home` / `End` | Jump to start / end of line |

## Input history

In Insert mode, press `Up` and `Down` to cycle through previously sent messages
and commands. History is per-session (not persisted to disk). Your current draft
is preserved while browsing history.
