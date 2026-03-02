# Commands

All commands start with `/`. Type `/` in Insert mode to open the autocomplete popup.

## Command reference

| Command | Alias | Arguments | Description |
|---|---|---|---|
| `/join` | `/j` | `<name>` | Switch to a conversation by contact name, number, or group |
| `/part` | `/p` | | Leave current conversation |
| `/sidebar` | `/sb` | | Toggle sidebar visibility |
| `/bell` | `/notify` | `[type]` | Toggle notifications (`direct`, `group`, or both) |
| `/mute` | | | Mute/unmute current conversation |
| `/contacts` | `/c` | | Browse synced contacts |
| `/settings` | | | Open settings overlay |
| `/help` | `/h` | | Show help overlay |
| `/quit` | `/q` | | Exit signal-tui |

## Autocomplete

When you type `/`, a popup appears showing matching commands. As you continue
typing, the list filters down. Use:

- **Up/Down arrows** to navigate the list
- **Tab** to complete the selected command
- **Esc** to dismiss the popup

## Examples

**Join a conversation by name:**
```
/join Alice
```

**Join by phone number:**
```
/j +15551234567
```

**Toggle direct message notifications off:**
```
/bell direct
```

**Toggle all notifications:**
```
/bell
```

**Mute the current conversation:**
```
/mute
```

## Messaging a new contact

To start a conversation with someone not in your sidebar, use `/join` with their
phone number in E.164 format:

```
/join +15551234567
```

The conversation will appear in your sidebar once the first message is exchanged.
