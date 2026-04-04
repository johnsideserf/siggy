# Sync-Aware Viewport Stabilization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stabilize the UI during initial message sync so users can interact immediately without viewport jumping, notification spam, or input lag.

**Architecture:** Add a `SyncState` to `App` that starts active on launch and deactivates when message rate drops. During sync: throttle redraws to 500ms (unless user input), increment `scroll_offset` to pin the viewport, suppress notifications and accumulate counts, show sync progress in the status bar. On sync exit: snap viewport to newest, fire summary notification, resume normal rendering.

**Tech Stack:** Rust, no new dependencies. Uses `std::time::Instant` for timing.

---

## File Structure

| Action | File | Responsibility |
|--------|------|---------------|
| Modify | `src/app.rs` | Add `SyncState` struct and `pub sync: SyncState` field, adjust `handle_message()` for viewport/notification/read-marker changes, add `end_sync()` method |
| Modify | `src/main.rs` | Add sync exit detection and render throttling in main event loop |
| Modify | `src/ui.rs` | Show sync progress in status bar during sync |

---

### Task 1: Add SyncState struct and field to App

**Files:**
- Modify: `src/app.rs`

- [ ] **Step 1: Write tests for SyncState**

Add these tests at the end of the existing `mod tests` block in `src/app.rs`:

```rust
    #[rstest]
    fn sync_starts_active(app: App) {
        assert!(app.sync.active);
        assert_eq!(app.sync.message_count, 0);
        assert!(!app.sync.user_scrolled);
    }

    #[rstest]
    fn sync_should_end_requires_quiet_and_min_elapsed(mut app: App) {
        // Just started, no messages -- should NOT end (min elapsed not met)
        assert!(!app.sync.should_end());

        // Fake started_at to 15 seconds ago, no messages ever -- should end
        app.sync.started_at = Instant::now() - std::time::Duration::from_secs(15);
        assert!(app.sync.should_end());

        // Recent message -- should NOT end even with elapsed time
        app.sync.last_message_time = Some(Instant::now());
        assert!(!app.sync.should_end());

        // Message was 5 seconds ago, started 15 seconds ago -- should end
        app.sync.last_message_time = Some(Instant::now() - std::time::Duration::from_secs(5));
        assert!(app.sync.should_end());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test sync_starts_active 2>&1 | tail -5`
Expected: FAIL (field `sync` does not exist)

- [ ] **Step 3: Add the SyncState struct definition**

Add this above the `pub struct App` definition (around line 171), after the existing state struct definitions:

```rust
/// Tracks initial sync burst state for viewport stabilization and notification suppression.
pub struct SyncState {
    /// Whether initial sync is in progress
    pub active: bool,
    /// Total messages received during this sync
    pub message_count: usize,
    /// When the last signal-cli message arrived
    pub last_message_time: Option<Instant>,
    /// When the app started (for minimum sync duration)
    pub started_at: Instant,
    /// Suppressed notification counts per conversation: conv_id -> count
    pub suppressed_notifications: HashMap<String, usize>,
    /// Whether the user manually scrolled during sync
    pub user_scrolled: bool,
}

impl SyncState {
    pub fn new() -> Self {
        Self {
            active: true,
            message_count: 0,
            last_message_time: None,
            started_at: Instant::now(),
            suppressed_notifications: HashMap::new(),
            user_scrolled: false,
        }
    }

    /// Whether the sync burst should end: no messages for 3 seconds AND at least
    /// 10 seconds since startup (prevents false exits from brief signal-cli pauses).
    pub fn should_end(&self) -> bool {
        let elapsed = self.started_at.elapsed().as_secs() >= 10;
        let quiet = match self.last_message_time {
            Some(t) => t.elapsed().as_secs() >= 3,
            None => true, // no messages ever received
        };
        elapsed && quiet
    }
}
```

- [ ] **Step 4: Add the `sync` field to the App struct**

Add this field to `pub struct App` (around line 171, after the `pub autocomplete: AutocompleteState` field):

```rust
    /// Initial sync burst state (viewport stabilization, notification suppression)
    pub sync: SyncState,
```

- [ ] **Step 5: Initialize `sync` in `App::new()`**

In the `App::new()` method, add this line alongside the other field initializers:

```rust
            sync: SyncState::new(),
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test sync_starts_active sync_should_end 2>&1 | tail -10`
Expected: 2 tests pass

- [ ] **Step 7: Commit**

```bash
git add src/app.rs
git commit -m "Add SyncState struct for initial sync burst tracking (#310)"
```

---

### Task 2: Notification suppression during sync

**Files:**
- Modify: `src/app.rs:3809-3836` (notification logic in `handle_message()`)

- [ ] **Step 1: Write tests for notification suppression**

Add these tests to `mod tests` in `src/app.rs`:

```rust
    #[rstest]
    fn sync_suppresses_notifications(mut app: App) {
        // Sync is active by default
        assert!(app.sync.active);

        // Set up: known contact so conversation is accepted
        app.store.contact_names.insert("+1".to_string(), "Alice".to_string());
        app.store.get_or_create_conversation("+other", "Other", false, &app.db);
        app.active_conversation = Some("+other".to_string());
        app.notifications.notify_direct = true;

        let msg = make_msg("+1", Some("hello"), None, false);
        app.handle_signal_event(SignalEvent::MessageReceived(msg));

        // Bell should NOT fire during sync
        assert!(!app.notifications.pending_bell);
        // But suppressed count should be tracked
        assert_eq!(app.sync.suppressed_notifications.get("+1").copied().unwrap_or(0), 1);
        // Sync message count should increment
        assert!(app.sync.message_count > 0);
    }

    #[rstest]
    fn notifications_fire_after_sync_ends(mut app: App) {
        // End sync
        app.sync.active = false;

        app.store.contact_names.insert("+1".to_string(), "Alice".to_string());
        app.store.get_or_create_conversation("+other", "Other", false, &app.db);
        app.active_conversation = Some("+other".to_string());
        app.notifications.notify_direct = true;

        let msg = make_msg("+1", Some("hello"), None, false);
        app.handle_signal_event(SignalEvent::MessageReceived(msg));

        // Bell SHOULD fire after sync ends
        assert!(app.notifications.pending_bell);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test sync_suppresses_notifications notifications_fire_after_sync 2>&1 | tail -10`
Expected: `sync_suppresses_notifications` FAILS (bell fires during sync)

- [ ] **Step 3: Add sync message tracking in `handle_message()`**

In `handle_message()` (around line 3571), near the top of the method after the `conv_id` is determined (around line 3580), add:

```rust
        // Track sync progress
        if self.sync.active {
            self.sync.message_count += 1;
            self.sync.last_message_time = Some(Instant::now());
        }
```

- [ ] **Step 4: Gate notifications behind sync check**

In `handle_message()`, find the notification block (around line 3809). Wrap the bell and desktop notification logic with a sync check. Replace the block:

```rust
        if !is_active && !msg.is_outgoing {
            if let Some(c) = self.store.conversations.get_mut(&conv_id) {
                c.unread += 1;
            }
            let conv_accepted = self.store.conversations.get(&conv_id).map(|c| c.accepted).unwrap_or(true);
            let not_muted_or_blocked = conv_accepted
                && !self.muted_conversations.contains(&conv_id)
                && !self.blocked_conversations.contains(&conv_id);
            let type_enabled = if is_group { self.notifications.notify_group } else { self.notifications.notify_direct };
            if type_enabled && not_muted_or_blocked {
                self.notifications.pending_bell = true;
            }
            if self.notifications.desktop_notifications && not_muted_or_blocked {
                let notif_body = msg.body.as_deref().unwrap_or("");
                let notif_group = if is_group {
                    self.store.conversations.get(&conv_id).map(|c| c.name.clone())
                } else {
                    None
                };
                show_desktop_notification(
                    &sender_display,
                    notif_body,
                    is_group,
                    notif_group.as_deref(),
                    &self.notifications.notification_preview,
                );
            }
        }
```

With:

```rust
        if !is_active && !msg.is_outgoing {
            if let Some(c) = self.store.conversations.get_mut(&conv_id) {
                c.unread += 1;
            }
            let conv_accepted = self.store.conversations.get(&conv_id).map(|c| c.accepted).unwrap_or(true);
            let not_muted_or_blocked = conv_accepted
                && !self.muted_conversations.contains(&conv_id)
                && !self.blocked_conversations.contains(&conv_id);
            if self.sync.active {
                // During sync: suppress notifications, track counts
                let type_enabled = if is_group { self.notifications.notify_group } else { self.notifications.notify_direct };
                if type_enabled && not_muted_or_blocked {
                    *self.sync.suppressed_notifications.entry(conv_id.clone()).or_insert(0) += 1;
                }
            } else {
                // Normal operation: fire notifications
                let type_enabled = if is_group { self.notifications.notify_group } else { self.notifications.notify_direct };
                if type_enabled && not_muted_or_blocked {
                    self.notifications.pending_bell = true;
                }
                if self.notifications.desktop_notifications && not_muted_or_blocked {
                    let notif_body = msg.body.as_deref().unwrap_or("");
                    let notif_group = if is_group {
                        self.store.conversations.get(&conv_id).map(|c| c.name.clone())
                    } else {
                        None
                    };
                    show_desktop_notification(
                        &sender_display,
                        notif_body,
                        is_group,
                        notif_group.as_deref(),
                        &self.notifications.notification_preview,
                    );
                }
            }
        }
```

- [ ] **Step 5: Run tests**

Run: `cargo test sync_suppresses_notifications notifications_fire_after_sync 2>&1 | tail -10`
Expected: Both tests pass

- [ ] **Step 6: Run full test suite**

Run: `cargo test 2>&1 | grep "^test result"`
Expected: All tests pass (some existing notification tests may need the sync flag set to false; check and fix if needed)

- [ ] **Step 7: Commit**

```bash
git add src/app.rs
git commit -m "Suppress notifications during initial sync, track counts (#310)"
```

---

### Task 3: Viewport stabilization during sync

**Files:**
- Modify: `src/app.rs` (handle_message read-marker logic around line 3838, scroll handlers)

- [ ] **Step 1: Write tests for viewport stabilization**

Add these tests to `mod tests`:

```rust
    #[rstest]
    fn sync_stabilizes_scroll_offset(mut app: App) {
        assert!(app.sync.active);
        app.store.get_or_create_conversation("+1", "Alice", false, &app.db);
        app.active_conversation = Some("+1".to_string());
        app.scroll_offset = 0;

        // Receive a message during sync
        let msg = make_msg("+1", Some("hello from sync"), None, false);
        app.handle_signal_event(SignalEvent::MessageReceived(msg));

        // scroll_offset should have been incremented to compensate
        assert!(app.scroll_offset > 0, "scroll_offset should increase during sync");
    }

    #[rstest]
    fn sync_does_not_stabilize_after_user_scroll(mut app: App) {
        assert!(app.sync.active);
        app.store.get_or_create_conversation("+1", "Alice", false, &app.db);
        app.active_conversation = Some("+1".to_string());
        app.scroll_offset = 0;
        app.sync.user_scrolled = true;

        let msg = make_msg("+1", Some("hello"), None, false);
        app.handle_signal_event(SignalEvent::MessageReceived(msg));

        // scroll_offset should NOT be adjusted when user has scrolled
        assert_eq!(app.scroll_offset, 0);
    }

    #[rstest]
    fn sync_does_not_advance_read_index_for_active_conv(mut app: App) {
        assert!(app.sync.active);
        app.store.get_or_create_conversation("+1", "Alice", false, &app.db);
        app.active_conversation = Some("+1".to_string());

        let initial_read = app.store.last_read_index.get("+1").copied().unwrap_or(0);

        let msg = make_msg("+1", Some("hello"), None, false);
        app.handle_signal_event(SignalEvent::MessageReceived(msg));

        let after_read = app.store.last_read_index.get("+1").copied().unwrap_or(0);
        assert_eq!(initial_read, after_read, "read index should not advance during sync");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test sync_stabilizes_scroll sync_does_not_stabilize sync_does_not_advance_read 2>&1 | tail -10`
Expected: Tests fail

- [ ] **Step 3: Add viewport stabilization in `handle_message()`**

In `handle_message()`, after the message insertion block (the `push_msg` closure call, around line 3770), add this code:

```rust
        // During sync: stabilize viewport by compensating scroll_offset
        if self.sync.active && !self.sync.user_scrolled {
            if self.active_conversation.as_ref() == Some(&conv_id) {
                self.scroll_offset = self.scroll_offset.saturating_add(1);
            }
        }
```

- [ ] **Step 4: Gate read-marker advancement behind sync check**

In `handle_message()`, find the "Active conversation: send read receipt and advance read marker" block (around line 3838). Change:

```rust
        if is_active {
            if !msg.is_outgoing && conv_accepted && !self.blocked_conversations.contains(&conv_id) {
                self.queue_single_read_receipt(&sender_id, msg_ts_ms);
            }
            if let Some(conv) = self.store.conversations.get(&conv_id) {
                self.store.last_read_index.insert(conv_id.clone(), conv.messages.len());
            }
```

To:

```rust
        if is_active {
            if !self.sync.active {
                if !msg.is_outgoing && conv_accepted && !self.blocked_conversations.contains(&conv_id) {
                    self.queue_single_read_receipt(&sender_id, msg_ts_ms);
                }
                if let Some(conv) = self.store.conversations.get(&conv_id) {
                    self.store.last_read_index.insert(conv_id.clone(), conv.messages.len());
                }
            }
```

Make sure the closing brace for the new `if !self.sync.active` block is placed correctly, before the `if let Ok(Some(rowid))` line (the DB read-marker persist should still run during sync to keep the DB consistent).

- [ ] **Step 5: Mark user scroll in Normal mode scroll handlers**

In `handle_normal_key()` (around line 3101), add `self.sync.user_scrolled = true;` to each scroll action. Change:

```rust
            Some(KeyAction::ScrollDown) => { self.scroll_offset = self.scroll_offset.saturating_sub(1); self.focused_msg_index = None; None }
            Some(KeyAction::ScrollUp) => { self.scroll_offset = self.scroll_offset.saturating_add(1); self.focused_msg_index = None; None }
```

To:

```rust
            Some(KeyAction::ScrollDown) => { self.scroll_offset = self.scroll_offset.saturating_sub(1); self.focused_msg_index = None; self.sync.user_scrolled = true; None }
            Some(KeyAction::ScrollUp) => { self.scroll_offset = self.scroll_offset.saturating_add(1); self.focused_msg_index = None; self.sync.user_scrolled = true; None }
```

Do the same for `HalfPageDown`, `HalfPageUp`, `FocusNextMessage`, `FocusPrevMessage`, and `ScrollToBottom` on the following lines. Also do the same for the Insert mode scroll handlers (around line 3333) and the `PageScrollUp`/`PageScrollDown` in `handle_global_key()` (around line 2922). Also set it in `handle_mouse_event()` for `ScrollUp`/`ScrollDown` events on the messages area (around line 6094).

- [ ] **Step 6: Run tests**

Run: `cargo test sync_stabilizes_scroll sync_does_not_stabilize sync_does_not_advance_read 2>&1 | tail -10`
Expected: All 3 tests pass

- [ ] **Step 7: Run full test suite**

Run: `cargo clippy --tests -- -D warnings && cargo test 2>&1 | grep "^test result"`
Expected: All pass

- [ ] **Step 8: Commit**

```bash
git add src/app.rs
git commit -m "Stabilize viewport and suppress read-marker during sync (#310)"
```

---

### Task 4: Sync exit sequence

**Files:**
- Modify: `src/app.rs` (add `end_sync()` method)

- [ ] **Step 1: Write test for end_sync()**

Add to `mod tests`:

```rust
    #[rstest]
    fn end_sync_snaps_to_bottom_and_fires_bell(mut app: App) {
        app.sync.active = true;
        app.sync.message_count = 50;
        app.scroll_offset = 30;
        app.sync.suppressed_notifications.insert("+1".to_string(), 10);
        app.sync.suppressed_notifications.insert("+2".to_string(), 5);

        app.end_sync();

        assert!(!app.sync.active);
        assert_eq!(app.scroll_offset, 0, "should snap to bottom");
        assert!(app.notifications.pending_bell, "should fire summary bell");
        assert!(app.sync.suppressed_notifications.is_empty(), "should clear suppressed counts");
    }

    #[rstest]
    fn end_sync_respects_user_scroll(mut app: App) {
        app.sync.active = true;
        app.scroll_offset = 15;
        app.sync.user_scrolled = true;

        app.end_sync();

        assert!(!app.sync.active);
        assert_eq!(app.scroll_offset, 15, "should preserve user scroll position");
    }

    #[rstest]
    fn end_sync_no_bell_when_no_suppressed(mut app: App) {
        app.sync.active = true;
        app.sync.message_count = 5;
        // No suppressed notifications

        app.end_sync();

        assert!(!app.sync.active);
        assert!(!app.notifications.pending_bell);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test end_sync 2>&1 | tail -10`
Expected: FAIL (method `end_sync` not found)

- [ ] **Step 3: Implement `end_sync()`**

Add this method to the `impl App` block:

```rust
    /// End the initial sync burst: snap viewport, fire summary notification, resume normal behavior.
    pub fn end_sync(&mut self) {
        self.sync.active = false;

        // Snap viewport to newest messages (unless user manually scrolled)
        if !self.sync.user_scrolled {
            self.scroll_offset = 0;
        }

        // Fire summary notification if any were suppressed
        let total: usize = self.sync.suppressed_notifications.values().sum();
        let conv_count = self.sync.suppressed_notifications.len();
        if total > 0 {
            self.notifications.pending_bell = true;
            if self.notifications.desktop_notifications {
                let body = format!("{total} new messages in {conv_count} conversations");
                show_desktop_notification("siggy", &body, false, None, "full");
            }
        }
        self.sync.suppressed_notifications.clear();

        // Mark current conversation as read now that viewport is at bottom
        self.mark_read();

        // Update status to clear sync message
        self.status_message = if self.connected {
            "connected".to_string()
        } else {
            "disconnected".to_string()
        };
    }
```

- [ ] **Step 4: Run tests**

Run: `cargo test end_sync 2>&1 | tail -10`
Expected: All 3 tests pass

- [ ] **Step 5: Commit**

```bash
git add src/app.rs
git commit -m "Add end_sync() method for sync exit sequence (#310)"
```

---

### Task 5: Render throttling and sync exit detection in main loop

**Files:**
- Modify: `src/main.rs:1122-1125` (drain_events redraw trigger)
- Modify: `src/main.rs` (add sync exit check)

- [ ] **Step 1: Add render throttle for sync**

In `src/main.rs`, find the drain_events call (around line 1122):

```rust
        // Drain signal events (non-blocking), detect disconnect
        if backend.drain_events(&mut app) {
            needs_redraw = true;
        }
```

Replace with:

```rust
        // Drain signal events (non-blocking), detect disconnect
        if backend.drain_events(&mut app) {
            if app.sync.active {
                // During sync: throttle redraws to 500ms to keep UI responsive
                if last_sync_redraw.elapsed() >= std::time::Duration::from_millis(500) {
                    needs_redraw = true;
                    last_sync_redraw = Instant::now();
                }
            } else {
                needs_redraw = true;
            }
        }
```

- [ ] **Step 2: Declare the `last_sync_redraw` variable**

Near the top of the `run_app()` function (around line 995, before the main loop), add:

```rust
    let mut last_sync_redraw = Instant::now();
```

Make sure `use std::time::Instant;` is in scope (check if it's already imported in main.rs; if not, add it).

- [ ] **Step 3: Add sync exit detection**

After the drain_events block, add the sync exit check:

```rust
        // Check if initial sync burst has ended
        if app.sync.active && app.sync.should_end() {
            app.end_sync();
            needs_redraw = true;
        }
```

- [ ] **Step 4: Verify it compiles and tests pass**

Run: `cargo clippy --tests -- -D warnings && cargo test 2>&1 | grep "^test result"`
Expected: All pass

- [ ] **Step 5: Commit**

```bash
git add src/main.rs
git commit -m "Add render throttling and sync exit detection in main loop (#310)"
```

---

### Task 6: Status bar sync indicator

**Files:**
- Modify: `src/ui.rs:2193` (draw_status_bar function)

- [ ] **Step 1: Add sync indicator to status bar**

In `src/ui.rs`, in the `draw_status_bar` function (around line 2193), after the quit_confirm early return (around line 2207) and before the mode indicator, add:

```rust
    // Sync progress indicator (overrides normal status bar)
    if app.sync.active && app.sync.message_count > 0 {
        let bar = Line::from(vec![
            Span::styled(" Syncing... ", Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)),
            Span::styled(
                format!("({} messages received)", app.sync.message_count),
                Style::default().fg(theme.statusbar_fg),
            ),
        ]);
        frame.render_widget(
            Paragraph::new(bar).style(Style::default().bg(theme.statusbar_bg)),
            area,
        );
        return;
    }
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo clippy --tests -- -D warnings 2>&1 | tail -5`
Expected: Clean

- [ ] **Step 3: Commit**

```bash
git add src/ui.rs
git commit -m "Show sync progress indicator in status bar (#310)"
```

---

### Task 7: Update status message during sync

**Files:**
- Modify: `src/app.rs` (handle_message status_message override)

- [ ] **Step 1: Override status_message during sync**

In `handle_message()`, after the sync tracking lines added in Task 2 (the `self.sync.message_count += 1` block), add:

```rust
            self.status_message = format!("Syncing... ({} messages received)", self.sync.message_count);
```

This ensures the status message stays current even if the status bar doesn't use the early-return path (e.g. if the status bar rendering changes in the future).

- [ ] **Step 2: Verify everything works together**

Run: `cargo clippy --tests -- -D warnings && cargo test 2>&1 | grep "^test result"`
Expected: All tests pass, clippy clean

- [ ] **Step 3: Commit**

```bash
git add src/app.rs
git commit -m "Update status message during sync progress (#310)"
```

---

### Task 8: Fix existing tests that assume no sync state

**Files:**
- Modify: `src/app.rs` (test module)

Some existing tests may fail because sync is now active by default and suppresses notifications/read-markers. Tests that check notification behavior or read-marker advancement need `app.sync.active = false` set in their setup.

- [ ] **Step 1: Run full test suite and identify failures**

Run: `cargo test 2>&1 | grep "FAILED\|failures"`
Identify which tests fail due to sync being active.

- [ ] **Step 2: Fix failing tests**

For each failing test, add `app.sync.active = false;` after the `app` fixture is created. This is the correct fix because these tests are testing normal (non-sync) behavior.

Common tests that will likely need this:
- `bell_rings_for_background_dm`
- `bell_not_set_for_active_conversation` (should still pass since it checks no bell)
- `active_conv_queues_read_receipt`
- Any test that checks `last_read_index` advancement for the active conversation
- Any test that checks `pending_bell` is set to true

Do NOT add `sync.active = false` to the new sync-specific tests from Tasks 1-4.

- [ ] **Step 3: Run full test suite**

Run: `cargo clippy --tests -- -D warnings && cargo test 2>&1 | grep "^test result"`
Expected: All tests pass

- [ ] **Step 4: Commit**

```bash
git add src/app.rs
git commit -m "Fix existing tests for sync-active-by-default behavior (#310)"
```
