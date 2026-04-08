# Sync-Aware Viewport Stabilization Design

**Issue:** #310

**Goal:** When opening siggy after an extended offline period, provide a smooth experience where the user can interact immediately while signal-cli streams historical messages in the background, without viewport jumping, notification spam, or input lag.

## Problem

signal-cli delivers pending messages chronologically (oldest first) via JSON-RPC notifications. After weeks offline, this can mean thousands of messages streaming over 5+ minutes. Currently:

1. The viewport follows each message as it arrives, creating a forced "replay" from oldest to newest
2. Desktop notifications and terminal bells fire for every message
3. App responsiveness degrades because every message triggers a full UI redraw

## Architecture

Add a `SyncState` struct to `App` that tracks whether an initial sync burst is in progress. During sync, throttle rendering, stabilize the viewport, and suppress notifications. When sync ends, snap the viewport to the newest messages and fire a single summary notification.

No changes to message processing, storage, or signal-cli communication. This is purely a presentation-layer optimization.

## Sync Burst Detection

`SyncState` starts with `active: true` on app startup. The sync burst is considered over when no messages arrive for 3 seconds AND at least 10 seconds have elapsed since the app started. The minimum elapsed time prevents false exits from brief pauses in signal-cli's message stream (e.g. pagination boundaries).

```rust
pub struct SyncState {
    /// Whether initial sync is in progress
    pub active: bool,
    /// Total messages received during this sync
    pub message_count: usize,
    /// When the last signal-cli message arrived
    pub last_message_time: Option<Instant>,
    /// When the app started (for minimum sync duration)
    pub started_at: Instant,
    /// Suppressed notification counts per conversation
    pub suppressed_notifications: HashMap<String, usize>,
    /// Whether the user manually scrolled during sync
    pub user_scrolled: bool,
}
```

Detection is checked in the main event loop each iteration. If `active` is true and `last_message_time` is more than 3 seconds ago and `started_at` is more than 10 seconds ago, trigger the sync exit sequence.

Edge case: if the user is online and opens the app with no pending messages, sync will exit after 10 seconds + 3 seconds of quiet. This is fine since the SyncState has no visible effect when no messages are arriving.

## Render Throttling

During sync, gate UI redraws behind a timer:
- Signal events do NOT trigger immediate redraws
- Redraws happen at most once every 500ms (via a `last_redraw: Instant` check)
- Keyboard and mouse events still trigger immediate redraws so user input feels responsive
- This fixes the sluggish conversation switching by freeing CPU for event processing between renders

The throttle is applied in the main event loop by conditionally setting `needs_redraw` based on whether 500ms has elapsed since the last redraw, unless the redraw was triggered by user input.

## Viewport Stabilization

During sync, prevent the viewport from following incoming messages:

- When `handle_message()` inserts a message into the active conversation, increment `scroll_offset` by 1 to compensate. Since `scroll_offset` is measured from the bottom (0 = newest), inserting any message extends the list and the increment keeps the viewport anchored to the same messages.
- The view stays where it was; messages accumulate silently
- If the user manually scrolls (j/k, page up/down, mouse scroll), set `user_scrolled: true` and stop adjusting `scroll_offset`
- When sync ends: if `user_scrolled` is false, reset `scroll_offset` to 0 (snap to newest). If true, leave it where the user put it.

The `scroll_offset` adjustment happens in `handle_message()` after the message insertion, gated behind `self.sync.active && !self.sync.user_scrolled`.

## Notification Suppression

During sync:
- Do not set `pending_bell`
- Do not call `show_desktop_notification()`
- Instead, increment `suppressed_notifications[conv_id]` for each message that would have triggered a notification
- Unread counts on sidebar conversations still update normally (useful for user orientation)

When sync ends:
- Fire a single desktop notification summarizing what arrived: "N new messages in M conversations"
- Set `pending_bell` once (single terminal ding)
- Clear `suppressed_notifications`

## Status Bar

During sync, override the status bar message with: "Syncing... (N messages received)"

This updates every time the render throttle allows a redraw (every 500ms). When sync ends, revert to the normal status bar content.

## Read Position

During sync, do not advance `last_read_index` for incoming messages in the active conversation. The user hasn't actually read them. When sync ends and the viewport snaps to bottom, call `mark_read()` as normal to advance the read position.

## Files Changed

| File | Change |
|------|--------|
| `src/app.rs` | Add `SyncState` struct, add `pub sync: SyncState` field to `App`, adjust `handle_message()` for viewport stabilization and notification suppression, adjust `last_read_index` logic |
| `src/main.rs` | Add sync exit detection in main loop, add render throttling logic |
| `src/ui.rs` | Show sync status in status bar when `app.sync.active` |

No new files needed. SyncState is small enough to live in app.rs alongside the other state structs (or in a new `src/sync.rs` if we prefer to follow the ConversationStore/AutocompleteState extraction pattern).
