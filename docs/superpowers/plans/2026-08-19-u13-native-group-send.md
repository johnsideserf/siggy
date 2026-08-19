# U13 Native Group Send + Admin Gating Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Group conversations work natively for daily use: send messages into existing v2 groups over the engine thread, and group-menu admin operations show honest capability copy instead of forming doomed requests.

**Architecture:** U11 already landed the group *receive* side (`derive_group_id`, group conversation keying, GroupList directory with member uuids). U13 adds the send side: `dispatch` routes `is_group` messages to a new `SendCommand::GroupMessage`, the engine resolves the base64 group id back to the 32-byte master key by walking the store's groups (the derivation is one-way, so it's a search, not an inverse), and drives `Manager::send_message_to_group` under the same KTD-4 contract as 1:1 (`Ok` → `SendTimestamp`, `Err`/30s timeout → `SendFailed`, late `Ok` upgrades). presage at the pinned rev (63482ef) has **no group admin APIs** (verified: only `send_message_to_group` and `retrieve_group_avatar` exist), so `supports_group_admin()` stays `false` and the group menu gates all five mutating ops with capability copy.

**Tech Stack:** Rust, presage (pinned git rev 63482ef), no new dependencies.

**Spec:** `docs/superpowers/plans/2026-07-07-native-backend-presage-plan.md` section "U13. Native groups (send/receive; admin capability-gated)" (lines 346–354). Tracked by issue #643 (Phase 3).

## Global Constraints

- Never commit to master: branch `feature/643-u13-native-group-send`, PR targeting master, squash merge (CLAUDE.md).
- Commits and PR reference `part of #643`.
- Native lane build/test/clippy commands use `--no-default-features --features native-backend`; the default lane must also stay green (`signal-cli-backend` is the default feature and the two are mutually exclusive).
- Before push: `cargo fmt --check`, `cargo clippy --tests -- -D warnings` (both lanes), `cargo test` (both lanes). CI's Lint job runs rustfmt — clippy alone is not enough (learned in #672).
- Capability copy wording comes from the spec: mutating group-menu ops answer "not supported by the native engine yet".
- KTD-4 status semantics: wire timestamp is the SendToken; `Ok` → `SendTimestamp`; `Err` or 30s timeout → `SendFailed`; a timed-out future keeps driving and a late `Ok` upgrades Failed→Sent (the #486 lesson).
- KTD-6 format lock: group conversation ids are base64 of the 32-byte group identifier derived from the master key, exactly as signal-cli renders them (`derive_group_id` in `src/backend/native/receive.rs:52`).
- The `App` struct field count is CI-ratcheted — new state goes into existing `src/domain/` sub-structs, never onto `App` directly.

---

## File Structure

| Action | File | Responsibility |
|--------|------|---------------|
| Modify | `src/domain/overlays.rs` | `admin_gated` flag on `GroupMenuOverlayState` |
| Modify | `src/app.rs` | `GroupMenuHint::mutates_group()`, capability gate in `transition_group_menu` |
| Modify | `src/main.rs` | Wire `backend.supports_group_admin()` into the flag at startup |
| Modify | `src/backend/native/mod.rs` | Route group messages to the engine instead of gating them |
| Modify | `src/backend/native/send.rs` | `SendCommand::GroupMessage`, master-key resolution, shared KTD-4 driver |
| Modify | `src/backend/native/receive.rs` | Test only: group sync-echo keying fixture (coverage gap) |
| Modify | `src/app_tests.rs` | Group-menu gating tests |

---

### Task 1: Group-menu admin capability gate

presage's pinned rev has no group admin APIs, so `NativeBackend::supports_group_admin()` already returns `false` (`src/backend/native/mod.rs:140`) — but nothing consumes it: the group menu happily forms `SendRequest::RenameGroup` etc., which then die in dispatch's fallthrough. Gate at the menu so the user sees the capability copy at selection time. The dispatch fallthrough stays as the safety net.

**Files:**
- Modify: `src/domain/overlays.rs:177-188` (`GroupMenuOverlayState`)
- Modify: `src/app.rs:341` (`GroupMenuHint`), `src/app.rs:1533` (`transition_group_menu`)
- Modify: `src/main.rs` (immediately before `backend.startup(&mut app).await;`, around line 1501)
- Test: `src/app_tests.rs`

**Interfaces:**
- Produces: `app.group_menu.admin_gated: bool` (default `false` = ungated, preserving every existing test and the signal-cli/demo/mock backends, whose `supports_group_admin()` is `true`); `GroupMenuHint::mutates_group(&self) -> bool`.

- [ ] **Step 1: Write the failing test in `src/app_tests.rs`**

Place it near the other group-menu tests (search for `transition_group_menu` or `group_menu` in the file and match the local fixture convention — tests there take `mut app: App` via rstest fixture). Mirror the surrounding tests' setup for making the active conversation a group:

```rust
#[rstest]
fn gated_group_admin_answers_with_capability_copy(mut app: App) {
    // Match neighboring group-menu tests' setup for an active group
    // conversation, then:
    app.group_menu.admin_gated = true;

    app.transition_group_menu(GroupMenuHint::Rename);
    assert!(
        app.status_message
            .contains("not supported by the native engine yet"),
        "mutating op must answer with capability copy, got: {}",
        app.status_message
    );
    assert!(
        !matches!(app.group_menu.state, Some(GroupMenuState::Rename)),
        "gated op must not transition"
    );

    // The read-only member list is not a wire mutation and stays available.
    app.transition_group_menu(GroupMenuHint::Members);
    assert!(matches!(app.group_menu.state, Some(GroupMenuState::Members)));
}
```

- [ ] **Step 2: Run it to make sure it fails**

Run: `cargo test gated_group_admin 2>&1 | tail -5`
Expected: compile error — `admin_gated` and `mutates_group` don't exist yet.

- [ ] **Step 3: Add the flag to `GroupMenuOverlayState` in `src/domain/overlays.rs`**

Append to the struct (it derives `Default`; `false` = ungated keeps existing behavior everywhere):

```rust
    /// KTD-10 capability gate (#643 U13): true when the active backend
    /// cannot perform group admin operations (the native engine, until
    /// presage grows admin APIs). Mutating menu entries answer with
    /// capability copy instead of transitioning.
    pub admin_gated: bool,
```

- [ ] **Step 4: Add `mutates_group` to `GroupMenuHint` in `src/app.rs`**

Next to the existing `impl GroupMenuHint` (where `from_char` lives, near line 341):

```rust
    /// Menu entries that mutate the group on the wire - everything except
    /// the read-only member list (#643 U13 capability gate).
    pub fn mutates_group(&self) -> bool {
        !matches!(self, GroupMenuHint::Members)
    }
```

- [ ] **Step 5: Gate `transition_group_menu` in `src/app.rs:1533`**

First lines of the function, before the index/filter/input resets:

```rust
        if self.group_menu.admin_gated && hint.mutates_group() {
            self.status_message =
                "group admin: not supported by the native engine yet (#643)".to_string();
            return;
        }
```

- [ ] **Step 6: Wire the backend capability in `src/main.rs`**

Immediately before `backend.startup(&mut app).await;`:

```rust
    // KTD-10 (#643 U13): the group menu gates mutating admin ops when the
    // active backend cannot perform them. Runtime, not compile-time: --demo
    // picks DemoBackend inside the same binary.
    app.group_menu.admin_gated = !backend.supports_group_admin();
```

- [ ] **Step 7: Run the test and the full default-lane suite**

Run: `cargo test gated_group_admin 2>&1 | tail -3 && cargo test 2>&1 | grep "^test result"`
Expected: new test passes; no existing test regresses (default `false` keeps signal-cli behavior).

- [ ] **Step 8: Commit**

```bash
git add src/domain/overlays.rs src/app.rs src/main.rs src/app_tests.rs
git commit -m "feat(native): gate group admin menu ops behind backend capability (part of #643)"
```

---

### Task 2: Dispatch routes group messages to the engine

**Files:**
- Modify: `src/backend/native/mod.rs:149-201` (dispatch `Message` arm), tests at `src/backend/native/mod.rs:614-639`
- Modify: `src/backend/native/send.rs:40-52` (`SendCommand`)

**Interfaces:**
- Consumes: `SendRequest::Message { recipient, body, is_group, local_ts_ms, attachment, .. }` — for groups, `recipient` is the base64 group id (KTD-6).
- Produces: `SendCommand::GroupMessage { token: SendToken, group_id: String, body: String, timestamp_ms: u64 }` — Task 3 implements the engine side. The dispatch arm registers `pending.sends` and generates the wire timestamp identically to 1:1 (shared `next_wire_ts()` counter, so 1:1 and group sends never collide).

- [ ] **Step 1: Add the `GroupMessage` variant to `SendCommand` in `src/backend/native/send.rs`**

```rust
    GroupMessage {
        token: SendToken,
        /// Base64 group id (KTD-6 format lock) - resolved back to the
        /// 32-byte master key against the store on the engine thread.
        group_id: String,
        body: String,
        /// Wire timestamp, already uniqueness-adjusted by the adapter.
        timestamp_ms: u64,
    },
```

Add a temporary exhaustive-match arm in `run_send_loop` so the crate compiles before Task 3 (Task 3 replaces it):

```rust
            SendCommand::GroupMessage { token, .. } => {
                emit(&event_tx, SignalEvent::SendFailed { token });
            }
```

- [ ] **Step 2: Write the failing dispatch tests in `src/backend/native/mod.rs`**

Replace `dispatch_gates_groups_and_attachments_honestly` (line 615) — group sends now route; attachments stay gated, including on groups:

```rust
    #[tokio::test]
    async fn dispatch_routes_group_messages_to_the_engine() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = file_backed_app(dir.path());
        let (_event_tx, _status_tx, mut command_rx, mut backend) = engine_backend();

        let mut group = message_req("Z3JvdXBpZA==", "hi group", 1);
        if let SendRequest::Message { is_group, .. } = &mut group {
            *is_group = true;
        }
        backend.dispatch(&mut app, group).await;

        let command = command_rx.try_recv().expect("group send reaches the engine");
        let send::SendCommand::GroupMessage { group_id, body, .. } = command else {
            panic!("expected GroupMessage command");
        };
        assert_eq!(group_id, "Z3JvdXBpZA==");
        assert_eq!(body, "hi group");
        assert_eq!(
            app.pending.sends.len(),
            1,
            "group send registered for confirmation correlation"
        );
    }

    #[tokio::test]
    async fn dispatch_gates_attachments_honestly() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = file_backed_app(dir.path());
        let (_event_tx, _status_tx, mut command_rx, mut backend) = engine_backend();

        // 1:1 attachment: still U14.
        let mut with_attachment = message_req("+15550001111", "hi", 1);
        if let SendRequest::Message { attachment, .. } = &mut with_attachment {
            *attachment = Some(std::path::PathBuf::from("x.png"));
        }
        backend.dispatch(&mut app, with_attachment).await;
        assert!(app.status_message.contains("U14"));

        // Group attachment: the attachment gate wins over group routing.
        let mut group_attachment = message_req("Z3JvdXBpZA==", "hi", 2);
        if let SendRequest::Message { is_group, attachment, .. } = &mut group_attachment {
            *is_group = true;
            *attachment = Some(std::path::PathBuf::from("x.png"));
        }
        backend.dispatch(&mut app, group_attachment).await;
        assert!(app.status_message.contains("U14"));

        assert!(
            command_rx.try_recv().is_err(),
            "gated requests never reach the engine"
        );
        assert!(app.pending.sends.is_empty());
    }
```

- [ ] **Step 3: Run them to make sure they fail**

Run: `cargo test --no-default-features --features native-backend dispatch_routes_group 2>&1 | tail -5`
Expected: FAIL — dispatch still gates `is_group` with the U13 copy.

- [ ] **Step 4: Rewrite the dispatch `Message` arm in `src/backend/native/mod.rs`**

Delete the `is_group` gate block (lines 162-167). Keep the attachment gate first, then branch only at command construction:

```rust
                if attachment.is_some() {
                    app.status_message =
                        "native engine: attachments not implemented yet (#642 U14)".to_string();
                    handlers::signal::mark_send_failed(app, &recipient, local_ts_ms);
                    return;
                }
                if self.engine.is_none() {
                    app.status_message = "native engine: not connected".to_string();
                    handlers::signal::mark_send_failed(app, &recipient, local_ts_ms);
                    return;
                }
                let wire_ts = self.next_wire_ts();
                let engine = self.engine.as_ref().expect("checked above");
                let token = crate::signal::types::SendToken::new(wire_ts.to_string());
                debug_log::logf(format_args!(
                    "native send: to={} group={is_group} wire_ts={wire_ts} local_ts={local_ts_ms}",
                    debug_log::mask_phone(&recipient)
                ));
                app.pending
                    .sends
                    .insert(token.clone(), (recipient.clone(), local_ts_ms));
                let command = if is_group {
                    send::SendCommand::GroupMessage {
                        token: token.clone(),
                        group_id: recipient.clone(),
                        body,
                        timestamp_ms: wire_ts as u64,
                    }
                } else {
                    send::SendCommand::Message {
                        token: token.clone(),
                        recipient: recipient.clone(),
                        body,
                        timestamp_ms: wire_ts as u64,
                    }
                };
```

The existing error-cleanup tail (`if engine.commands.send(command).is_err() { ... }`) stays unchanged — it already covers both variants. Update the dispatch doc comment above the function: groups are routed now; attachments are U14, rich bodies U15.

- [ ] **Step 5: Run the tests**

Run: `cargo test --no-default-features --features native-backend backend::native 2>&1 | grep "^test result"`
Expected: all pass, including the strictly-increasing-timestamp tests (the counter is shared, untouched).

- [ ] **Step 6: Commit**

```bash
git add src/backend/native/mod.rs src/backend/native/send.rs
git commit -m "feat(native): route group messages to the engine command channel (part of #643)"
```

---

### Task 3: Engine-side group send with master-key resolution

The group id cannot be inverted to a master key (the derivation is one-way through `GroupSecretParams`), so resolution walks the store's groups and matches on the derived id — mirroring how `resolve_recipient` walks contacts for 1:1.

**Files:**
- Modify: `src/backend/native/send.rs` (replace Task 2's temporary arm; add `send_group_one`, `resolve_group_master_key`, `find_group_master_key`; extract `drive_send` from `send_one`)

**Interfaces:**
- Consumes: `SendCommand::GroupMessage` (Task 2); `derive_group_id(&[u8]) -> Option<String>` (existing, `src/backend/native/receive.rs:52`); `presage::libsignal_service::zkgroup::GroupMasterKeyBytes` (= `[u8; 32]`); `Manager::send_message_to_group(&mut self, master_key_bytes: &[u8], message, timestamp)`.
- Produces: same KTD-4 event contract as 1:1 — `SendTimestamp` / `SendFailed` keyed by token.

- [ ] **Step 1: Write the failing tests for master-key matching in `src/backend/native/send.rs`'s tests module**

```rust
    /// The base64 group id -> master key mapping is a search, not an
    /// inverse: derivation is one-way, so the engine walks the store's
    /// groups and matches on the derived id (#643 U13).
    #[test]
    fn group_master_key_found_by_derived_id() {
        let target: GroupMasterKeyBytes = [7u8; 32];
        let decoy: GroupMasterKeyBytes = [9u8; 32];
        let id = super::super::receive::derive_group_id(&target).unwrap();
        assert_eq!(
            find_group_master_key([decoy, target], &id),
            Some(target)
        );
    }

    #[test]
    fn unknown_group_id_resolves_no_master_key() {
        let known: GroupMasterKeyBytes = [7u8; 32];
        assert_eq!(find_group_master_key([known], "bm90LWEta25vd24taWQ="), None);
    }
```

- [ ] **Step 2: Run them to make sure they fail**

Run: `cargo test --no-default-features --features native-backend group_master_key_found 2>&1 | tail -3`
Expected: compile error — `find_group_master_key` doesn't exist.

- [ ] **Step 3: Implement resolution + group send in `src/backend/native/send.rs`**

Add the import:

```rust
use presage::libsignal_service::zkgroup::GroupMasterKeyBytes;
```

(If `derive_group_id` is not already visible as `pub` from a sibling module, adjust its visibility in `receive.rs` to `pub(crate)` or `pub` — it is currently `pub fn`.)

Add:

```rust
/// Match a canonical base64 group id back to its master key. Pure so it
/// unit-tests without a store; derivation is deterministic.
pub(super) fn find_group_master_key(
    keys: impl IntoIterator<Item = GroupMasterKeyBytes>,
    group_id: &str,
) -> Option<GroupMasterKeyBytes> {
    keys.into_iter()
        .find(|key| super::receive::derive_group_id(key).as_deref() == Some(group_id))
}

async fn resolve_group_master_key(
    manager: &Manager<SqliteStore, Registered>,
    group_id: &str,
) -> Option<GroupMasterKeyBytes> {
    let groups = match manager.store().groups().await {
        Ok(groups) => groups,
        Err(e) => {
            debug_log::logf(format_args!("native group send: groups read failed: {e}"));
            return None;
        }
    };
    find_group_master_key(groups.flatten().map(|(master_key, _)| master_key), group_id)
}

/// One group send, one task: resolve the master key, then the same KTD-4
/// contract as 1:1.
async fn send_group_one(
    mut manager: Manager<SqliteStore, Registered>,
    token: SendToken,
    group_id: String,
    body: String,
    timestamp_ms: u64,
    event_tx: mpsc::UnboundedSender<EngineEvent>,
) {
    let Some(master_key) = resolve_group_master_key(&manager, &group_id).await else {
        debug_log::logf(format_args!(
            "native group send: no master key for group {group_id}"
        ));
        emit(&event_tx, SignalEvent::SendFailed { token });
        return;
    };
    let message = DataMessage {
        body: Some(body),
        timestamp: Some(timestamp_ms),
        ..Default::default()
    };
    drive_send(
        manager.send_message_to_group(&master_key, message, timestamp_ms),
        token,
        timestamp_ms,
        event_tx,
    )
    .await;
}
```

- [ ] **Step 4: Extract `drive_send` from `send_one` (shared KTD-4 driver)**

Replace `send_one`'s `tokio::pin!`/`tokio::select!` tail (lines 178-207) with a call to the new shared driver, keeping `send_one`'s resolution and `DataMessage` construction as they are:

```rust
    drive_send(
        manager.send_message(recipient, message, timestamp_ms),
        token,
        timestamp_ms,
        event_tx,
    )
    .await;
```

and add (the body is the moved select block, verbatim except for the generic future):

```rust
/// KTD-4 driving shared by 1:1 and group sends: `Ok` -> `SendTimestamp`,
/// `Err` or 30s timeout -> `SendFailed` - and a timed-out future keeps
/// driving so a late `Ok` still upgrades Failed -> Sent exactly once via
/// the dedicated status route (#486).
async fn drive_send<F, E>(
    send: F,
    token: SendToken,
    timestamp_ms: u64,
    event_tx: mpsc::UnboundedSender<EngineEvent>,
) where
    F: std::future::Future<Output = Result<(), E>>,
    E: std::fmt::Display,
{
    tokio::pin!(send);
    tokio::select! {
        result = &mut send => match result {
            // Spike finding (#639 question 2): Ok means server-accepted;
            // the local timestamp IS the wire timestamp, so the #480
            // rewrite dance no-ops.
            Ok(()) => emit(&event_tx, SignalEvent::SendTimestamp {
                token,
                server_ts: timestamp_ms as i64,
            }),
            Err(e) => {
                debug_log::logf(format_args!("native send failed: {e}"));
                emit(&event_tx, SignalEvent::SendFailed { token });
            }
        },
        _ = tokio::time::sleep(SEND_TIMEOUT) => {
            // KTD-4: report the timeout, but keep driving the future - a
            // late Ok upgrades Failed → Sent exactly once via the
            // dedicated status route (#486).
            emit(&event_tx, SignalEvent::SendFailed { token: token.clone() });
            match send.await {
                Ok(()) => emit(&event_tx, SignalEvent::SendTimestamp {
                    token,
                    server_ts: timestamp_ms as i64,
                }),
                Err(e) => debug_log::logf(format_args!(
                    "native send failed after timeout: {e}"
                )),
            }
        }
    }
}
```

- [ ] **Step 5: Replace Task 2's temporary `run_send_loop` arm with the real spawn**

```rust
            SendCommand::GroupMessage {
                token,
                group_id,
                body,
                timestamp_ms,
            } => {
                // Per-send Manager clone: same isolation as 1:1 - one
                // stuck 30s timeout must not serialize the rest.
                let manager = manager.clone();
                let event_tx = event_tx.clone();
                tokio::task::spawn_local(send_group_one(
                    manager,
                    token,
                    group_id,
                    body,
                    timestamp_ms,
                    event_tx,
                ));
            }
```

Also extend `ensure_manager`'s `match command` so a manager-load failure fails a `GroupMessage` honestly (same as `Message`):

```rust
                    SendCommand::Message { token, .. }
                    | SendCommand::GroupMessage { token, .. } => {
```

(replacing the existing `SendCommand::Message { token, .. } =>` pattern; the emit body is unchanged).

- [ ] **Step 6: Run the tests**

Run: `cargo test --no-default-features --features native-backend backend::native::send 2>&1 | grep "^test result"`
Expected: all pass, including the pre-existing KTD-4 and classification tests (the driver extraction must not change behavior).

- [ ] **Step 7: Commit**

```bash
git add src/backend/native/send.rs
git commit -m "feat(native): group send via store-resolved master key, shared KTD-4 driver (part of #643)"
```

---

### Task 4: Close the group sync-echo coverage gap in receive

The receive side is U11 work, but its test inventory has a gap U13's spec calls out: the sync echo of your *own* group send (what a reconnect replays) must key by the derived group id, not the destination. `map_data_message` already implements this (`src/backend/native/receive.rs:101-117`); pin it with a fixture so a regression can't silently split group history.

**Files:**
- Modify: `src/backend/native/receive.rs` (tests module only)

- [ ] **Step 1: Write the test**

Model: `sync_sent_maps_to_outgoing_with_destination` (line 858) for the sync shape, `group_message_keys_by_derived_group_id` (line 525) for the group context:

```rust
    #[test]
    fn sync_sent_group_message_keys_by_group_id() {
        // The sync echo of an own group send (replayed on reconnect) must
        // land in the group conversation, not a 1:1 keyed by destination.
        let mk = [4u8; 32];
        let mut dm = text_data_message("me, from my phone, to the group", 1_700_000_006_000);
        dm.group_v2 = Some(proto::GroupContextV2 {
            master_key: Some(mk.to_vec()),
            ..Default::default()
        });
        let sm = proto::SyncMessage {
            sent: Some(proto::sync_message::Sent {
                timestamp: Some(1_700_000_006_000),
                message: Some(dm),
                ..Default::default()
            }),
            ..Default::default()
        };
        let c = content(OWN_ACI, sm);
        let events = map_received(
            &Received::Content(Box::new(c)),
            OWN_ACI,
            &FakeResolver::with_alice(),
        );
        let [SignalEvent::MessageReceived(m)] = &events[..] else {
            panic!("expected MessageReceived for group sync echo");
        };
        assert!(m.is_outgoing);
        assert_eq!(m.group_id.as_deref(), derive_group_id(&mk).as_deref());
    }
```

- [ ] **Step 2: Run it**

Run: `cargo test --no-default-features --features native-backend sync_sent_group 2>&1 | tail -3`
Expected: PASS (it pins existing behavior). If it FAILS, the mapping has a real bug — stop and debug with superpowers:systematic-debugging before proceeding; do not adjust the assertion to match broken output.

- [ ] **Step 3: Commit**

```bash
git add src/backend/native/receive.rs
git commit -m "test(native): pin group sync-echo conversation keying (part of #643)"
```

---

### Task 5: Full verification, PR, Tier-3 manual

- [ ] **Step 1: Both lanes green + rustfmt**

Run:
```bash
cargo fmt --check
cargo clippy --tests --no-default-features --features native-backend -- -D warnings
cargo test --no-default-features --features native-backend
cargo clippy --tests -- -D warnings
cargo test
```
Expected: fmt clean, zero clippy warnings, all tests pass on both lanes.

- [ ] **Step 2: Push and open the PR**

```bash
git push -u origin feature/643-u13-native-group-send
gh pr create --title "feat(native): U13 - group send + admin capability gating (part of #643)" --body "<summary per the sections above: routing, master-key resolution, KTD-4 driver reuse, admin gating with presage-API verification note, sync-echo fixture. End with the Claude Code attribution footer per CLAUDE.md.>"
```

- [ ] **Step 3: Tier-3 manual (needs the human partner + phone, in an existing v2 group)**

Build `cargo build --no-default-features --features native-backend`, then with the linked account:
1. Group message from the phone → arrives in the correct group conversation with the sender's name resolved.
2. Group send from siggy → appears for other members on their devices; status progresses past Sending.
3. A group mention received → renders with the display name (member uuid map from the store).
4. Group menu on the group: Members list works; Rename/Add/Remove/Leave/Create answer "not supported by the native engine yet" with no crash and no wire call.
5. Quit siggy, send group messages from the phone, relaunch → backlog lands in the group conversation exactly once.

- [ ] **Step 4: After Tier-3 passes: record results in the PR body, wait for CI, squash merge**

```bash
gh pr merge --squash --delete-branch
```

Then check the U13 row off in issue #643 with a results comment.
