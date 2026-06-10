---
title: Monotonic status guard silently dropped every Sending -> Failed write
date: 2026-06-10
category: database-issues
module: db / message status
problem_type: database_issue
component: database
symptoms:
  - "Messages whose send failed showed the Sending glyph forever in the current session"
  - "After restart, failed sends displayed as Sent (load_from_db promoted stale Sending rows)"
  - "db row status stayed 2 (Sending) after handle_send_failed ran, with no error logged"
root_cause: logic_error
resolution_type: code_fix
severity: high
tags: [sqlite, message-status, monotonic-guard, silent-failure, update-where-clause]
---

# Monotonic status guard silently dropped every Sending -> Failed write

## Problem

`Database::update_message_status` carries a WHERE clause guard `AND status < ?3`
so receipt upgrades can never downgrade a message (Read must not regress to
Delivered when receipts arrive out of order). But the `MessageStatus` i32
encoding places `Failed = 1` BELOW `Sending = 2`, so the guard also rejected
the one legitimate "downgrade" in the model: marking a Sending message as
Failed. Every failure write was a silent no-op.

## Symptoms

- The in-memory status flipped to Failed correctly, so the UI looked right
  until restart - then `load_from_db` promoted the still-Sending row to Sent,
  and a message that was never delivered displayed as sent.
- No error anywhere: `UPDATE ... WHERE` matching zero rows is success as far
  as rusqlite is concerned.

## What Didn't Work

- The original `handle_send_failed` path looked correct in review for months:
  it called `update_message_status(conv_id, ts, Failed.to_i32())` and checked
  the Result. The bug was invisible at the call site because the dropped
  write is encoded in the WHERE clause of a different file.

## Solution

A dedicated `mark_message_failed` method whose guard expresses the actual
state machine (failure is only valid FROM Sending), instead of reusing the
upward-only comparator:

```rust
pub fn mark_message_failed(&self, conv_id: &str, timestamp_ms: i64) -> Result<()> {
    self.conn.execute(
        "UPDATE messages SET status = ?3
         WHERE conversation_id = ?1 AND timestamp_ms = ?2 AND sender = 'you' AND status = ?4",
        params![conv_id, timestamp_ms,
                MessageStatus::Failed.to_i32(), MessageStatus::Sending.to_i32()],
    )?;
    Ok(())
}
```

Fixed in PR #509 (issue #486), together with demoting stale Sending rows to
Failed on load instead of promoting them to Sent.

## Why This Works

The status enum is mostly a ladder (Sending -> Sent -> Delivered -> Read ->
Viewed) with one branch (Sending -> Failed). A single numeric comparator can
encode the ladder or the branch, not both. Encoding the transition rule per
method ("failed only from Sending") matches the real state machine.

## Prevention

- **A guarded UPDATE that matches zero rows is a silent failure.** When a
  WHERE clause encodes a business rule (monotonicity, ownership, state), the
  intended transition set must be tested through the DB, not just in memory.
  The in-memory half of this bug was covered by tests for months; the DB half
  had none.
- When writing tests for status/state transitions, always assert the
  PERSISTED state by reloading from the DB, not only the in-memory struct.
  That is exactly how this was found (writing the regression test for #486).
- When a numeric enum encodes a state machine with any non-ladder edge,
  treat `<` / `>` comparisons on it as a smell. Either write per-transition
  methods or an explicit `fn may_transition(from, to) -> bool` table.
- Related hazard, same file: `update_message_status` also requires
  `sender = 'you'`, so it only ever touches outgoing rows. Any future status
  write for incoming messages needs its own method too.

## Related Issues

- Issue #486 / PR #509 (the fix)
- Issue #480 / PR #511 (sibling find-by-timestamp bug: in-place server
  timestamp rewrite broke the sorted invariant the binary search needs)
- Issue #484 (receipt buffer never drains; its fallback path will need a
  direct DB status write and must NOT reuse update_message_status for
  failure transitions)
