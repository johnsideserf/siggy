# U14 Native Attachments Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Attachments send and receive natively with the same on-disk layout and UX as the signal-cli path: downloads land in `config.download_dir` under the same naming/sanitization semantics, the image/voice render pipeline is untouched, and outgoing files upload with captions.

**Architecture:** Receive: the supervisor hydrates mapped events before journaling — a lazily-loaded second `Manager` (the same pattern the send loop already uses; `get_attachment` takes `&self`, so it cannot fight the receive stream's `&mut` borrow) downloads each `AttachmentPointer`, writes the blob into `download_dir` under the shared sanitized name, and fills `Attachment.local_path`; from there the entire existing display/DB/image pipeline works unchanged, and journaled events carry the path so replays are consistent. A pre-existing destination file short-circuits the download (reconnect-replay dedup) — which also makes the hydration path unit-testable without a network. Send: `SendCommand::{Message,GroupMessage}` carry the attachment path; the engine reads the file, uploads with a 30s timeout, and links the returned pointer into the DataMessage (body doubles as caption). Sender-supplied filenames are attacker input crossing a new trust boundary: both directions route through the sanitization extracted from the signal-cli path's `parse_attachment`.

**Tech Stack:** Rust, presage (pinned rev 63482ef), tokio fs. No new dependencies.

**Spec:** `docs/superpowers/plans/2026-07-07-native-backend-presage-plan.md` section "U14. Native attachments" (lines 356-364). Tracked by issue #643 (Phase 3).

## Global Constraints

- Never commit to master: branch `feature/643-u14-native-attachments`, PR targeting master, squash merge (CLAUDE.md).
- Commits and PR reference `part of #643`.
- Native lane commands use `--no-default-features --features native-backend`; the default lane must also stay green.
- Before push: `cargo fmt --check`, `cargo clippy --tests -- -D warnings` (both lanes), `cargo test` (both lanes). CI's Lint job runs rustfmt.
- Security (spec-mandated): sender-supplied filenames are attacker input. Every name written to disk passes the shared sanitization (strip `/`, `\`, `..`; id-derived fallback) AND the canonicalize-containment check; a name that escapes `download_dir` is rejected, never "fixed up" ad hoc.
- KTD-4 semantics unchanged: upload failures and timeouts fail the send honestly (`SendFailed`), never wedge; download failures leave `local_path` None so the message still renders with the existing placeholder, no crash.
- Existing signal-cli-path behavior must not change: `parse_attachment`'s tests keep passing after the helper extraction.
- The `App` struct field count is CI-ratcheted — no new `App` fields.
- Line numbers below are approximate — locate by function/test name.

---

## File Structure

| Action | File | Responsibility |
|--------|------|---------------|
| Modify | `src/signal/parse/helpers.rs` | Extract shared pure helpers: `sanitize_attachment_name`, `contained_dest`, `ext_to_mime` (new), `mime_to_ext` made `pub(crate)` |
| Create | `src/backend/native/attachments.rs` | Receive-side hydration: pointer naming, download, disk write, `local_path` fill |
| Modify | `src/backend/native/receive.rs` | Expose `attachment_id(ap)` (extracted from `map_attachment`) |
| Modify | `src/backend/native/supervisor.rs` | Thread `download_dir` through spawn; hydrate events before journal/emit |
| Modify | `src/backend/native/send.rs` | `attachment` on both SendCommand variants; upload + link pointer; `load_manager` made `pub(super)` |
| Modify | `src/backend/native/mod.rs` | Route attachments instead of gating; pass download_dir at spawn; paste-cleanup parity |

---

### Task 1: Extract shared attachment-name helpers

The signal-cli path's `parse_attachment` (`src/signal/parse/helpers.rs:50`) contains the naming semantics the spec's U2 characterization documents: use the sender filename else generate `{last-8-of-id}.{ext}`, strip doubled extensions, replace `/` `\` and `..`, fall back to `{last-8-of-id}.bin` when empty, and verify the canonicalized destination stays inside `download_dir`. Extract these as pure `pub(crate)` functions so the native path (Task 2/3) reuses them verbatim.

**Files:**
- Modify: `src/signal/parse/helpers.rs`

**Interfaces:**
- Produces (all in `crate::signal::parse::helpers`, all `pub(crate)`):
  - `fn sanitize_attachment_name(filename: Option<&str>, id: &str, content_type: &str) -> String`
  - `fn contained_dest(download_dir: &std::path::Path, name: &str) -> Option<std::path::PathBuf>`
  - `fn mime_to_ext(mime: &str) -> &'static str` (existing, visibility raised)
  - `fn ext_to_mime(path: &std::path::Path) -> String` (new, for outgoing uploads)

Check `src/signal/parse/mod.rs` for the module declaration: if `helpers` is not already visible outside `signal::parse`, raise the module (or re-export the four functions) so `crate::backend::native` can reach them.

- [ ] **Step 1: Write the failing tests in `helpers.rs`'s test module**

```rust
    #[test]
    fn traversal_names_are_sanitized_and_contained() {
        let dir = tempfile::tempdir().unwrap();
        for hostile in ["../../.bashrc", "/etc/passwd", "..\\..\\evil.exe"] {
            let name = sanitize_attachment_name(Some(hostile), "attid1234", "text/plain");
            assert!(
                !name.contains('/') && !name.contains('\\') && !name.contains(".."),
                "sanitized name must not carry separators or traversal: {name}"
            );
            let dest = contained_dest(dir.path(), &name).expect("sanitized name must be containable");
            assert!(dest.starts_with(dir.path()));
        }
    }

    #[test]
    fn missing_filename_generates_id_derived_name() {
        // Last 8 chars of the id + mime-derived extension - the exact
        // generation parse_attachment performs today ("someattachmentid"
        // is 16 chars; its last 8 are "chmentid"). If mime_to_ext maps
        // image/jpeg to something other than "jpg", fix THIS literal to
        // match the existing helper, never the helper to match the test.
        let name = sanitize_attachment_name(None, "someattachmentid", "image/jpeg");
        assert_eq!(name, "chmentid.jpg");
    }

    #[test]
    fn ext_to_mime_maps_common_types() {
        use std::path::Path;
        assert_eq!(ext_to_mime(Path::new("a.jpg")), "image/jpeg");
        assert_eq!(ext_to_mime(Path::new("a.PNG")), "image/png");
        assert_eq!(ext_to_mime(Path::new("a.gif")), "image/gif");
        assert_eq!(ext_to_mime(Path::new("a.webp")), "image/webp");
        assert_eq!(ext_to_mime(Path::new("a.mp4")), "video/mp4");
        assert_eq!(ext_to_mime(Path::new("a.aac")), "audio/aac");
        assert_eq!(ext_to_mime(Path::new("a.pdf")), "application/pdf");
        assert_eq!(ext_to_mime(Path::new("a.unknownext")), "application/octet-stream");
        assert_eq!(ext_to_mime(Path::new("noext")), "application/octet-stream");
    }
```

Replace the `missing_filename_generates_id_derived_name` placeholder body with the real assertion once you read the existing generation logic: the expected value is the last 8 chars of the id plus `.` plus `mime_to_ext("image/jpeg")` — i.e. `"chmentid.jpg"` for id `"someattachmentid"` if `mime_to_ext` maps jpeg to `jpg`. Read `mime_to_ext` first and hardcode the correct literal; the test must assert an exact string, not re-implement the logic.

- [ ] **Step 2: Run them to make sure they fail**

Run: `cargo test --quiet traversal_names ext_to_mime missing_filename 2>&1 | tail -5`
Expected: compile errors — the functions don't exist at those visibilities yet.

- [ ] **Step 3: Extract the helpers**

Carve the naming block out of `parse_attachment` (lines ~64-96: generate-if-missing, doubled-extension strip, separator/traversal replace, empty fallback) into `sanitize_attachment_name`, and the canonicalize-containment block (lines ~98-110) into `contained_dest`. `parse_attachment` calls both — behavior identical, its existing tests must keep passing unchanged. Raise `mime_to_ext` to `pub(crate)`. Add `ext_to_mime` as a match on the lowercased extension covering at least: jpg/jpeg, png, gif, webp, heic, mp4, mov, aac, m4a, mp3, ogg, wav, pdf, txt, defaulting to `application/octet-stream`.

- [ ] **Step 4: Run the full default-lane suite**

Run: `cargo test --quiet 2>&1 | grep -E "^test result|FAILED"`
Expected: all pass, including every pre-existing `parse_attachment` test.

- [ ] **Step 5: Commit**

```bash
git add src/signal/parse/helpers.rs src/signal/parse/mod.rs
git commit -m "refactor: extract shared attachment naming/sanitization helpers (part of #643)"
```

---

### Task 2: Receive-side attachment downloads

**Files:**
- Create: `src/backend/native/attachments.rs`
- Modify: `src/backend/native/receive.rs` (extract `attachment_id`), `src/backend/native/supervisor.rs` (plumb + hydrate), `src/backend/native/mod.rs` (pass download_dir, declare module), `src/backend/native/send.rs` (make `load_manager` `pub(super)`)

**Interfaces:**
- Consumes: `sanitize_attachment_name` / `contained_dest` / `mime_to_ext` from Task 1; `send::load_manager(store_file)` (existing, visibility raised); `Manager::get_attachment(&self, &AttachmentPointer) -> Result<Vec<u8>, _>`.
- Produces:
  - `receive.rs`: `pub(super) fn attachment_id(ap: &proto::AttachmentPointer) -> String` — the id-extraction currently inlined in `map_attachment` (CdnKey / CdnId match); `map_attachment` calls it.
  - `attachments.rs`: `pub(super) fn pointer_dest_name(ap: &proto::AttachmentPointer) -> String`; `pub(super) async fn hydrate(events: &mut [SignalEvent], item: &Received, manager: &mut Option<Manager<SqliteStore, Registered>>, store_file: &Path, download_dir: &Path)`.
  - `supervisor.rs`: `spawn(store_file, journal_db, download_dir: PathBuf)` (signature change; `mod.rs` startup passes the app's configured download dir — the same value the signal-cli path hands `parse_attachment`; locate it where `App` carries media/download configuration, `app.rs` ~584).

- [ ] **Step 1: Extract `attachment_id` in `receive.rs` (pure refactor, existing tests must keep passing)**

```rust
/// Attachment id as signal-cli renders it: the CDN key when present, else
/// the numeric CDN id (U2 format lock; also the stem for generated
/// filenames in the shared naming helper).
pub(super) fn attachment_id(ap: &proto::AttachmentPointer) -> String {
    use proto::attachment_pointer::AttachmentIdentifier;
    match &ap.attachment_identifier {
        Some(AttachmentIdentifier::CdnKey(key)) => key.clone(),
        Some(AttachmentIdentifier::CdnId(id)) => id.to_string(),
        None => String::new(),
    }
}
```

Mirror whatever `map_attachment` (receive.rs:382) actually does today — if its match differs from the above, keep ITS behavior and just extract it. `map_attachment` then calls `attachment_id(ap)`.

- [ ] **Step 2: Write the failing tests for `attachments.rs`**

Create `src/backend/native/attachments.rs` with a tests module first (add `mod attachments;` beside the sibling `mod` declarations in `src/backend/native/mod.rs`):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::types::SignalEvent;

    fn pointer(filename: Option<&str>, content_type: &str, key: &str) -> proto::AttachmentPointer {
        proto::AttachmentPointer {
            content_type: Some(content_type.to_string()),
            file_name: filename.map(|s| s.to_string()),
            attachment_identifier: Some(
                proto::attachment_pointer::AttachmentIdentifier::CdnKey(key.to_string()),
            ),
            ..Default::default()
        }
    }

    #[test]
    fn dest_name_prefers_sanitized_sender_filename() {
        let name = pointer_dest_name(&pointer(Some("../../.bashrc"), "text/plain", "cdnkey12"));
        assert!(!name.contains('/') && !name.contains(".."));
    }

    /// Voice notes arrive with no filename and an audio content type; the
    /// generated name must keep the audio extension so the existing
    /// playback path (content-type + file on disk) works unchanged.
    #[test]
    fn voice_note_pointer_gets_audio_extension() {
        let mut ap = pointer(None, "audio/aac", "voicekey99");
        ap.flags = Some(proto::attachment_pointer::Flags::VoiceMessage as u32);
        let name = pointer_dest_name(&ap);
        assert!(name.ends_with(".aac"), "generated voice-note name keeps audio ext: {name}");
    }

    /// Replay dedup doubles as the no-network test: when the destination
    /// file already exists, hydrate fills local_path without ever loading
    /// a Manager (the slot stays None).
    #[tokio::test]
    async fn hydrate_reuses_existing_files_without_a_manager() {
        let dir = tempfile::tempdir().unwrap();
        let ap = pointer(Some("photo.jpg"), "image/jpeg", "cdnkey12");
        let dest = dir.path().join(pointer_dest_name(&ap));
        std::fs::write(&dest, b"bytes").unwrap();

        let mut events = vec![test_message_event_with_attachment(&ap)];
        let mut manager = None;
        hydrate(
            &mut events,
            &test_received_with_attachment(&ap),
            &mut manager,
            std::path::Path::new("/nonexistent-store"),
            dir.path(),
        )
        .await;

        assert!(manager.is_none(), "existing file must not trigger a manager load");
        let SignalEvent::MessageReceived(m) = &events[0] else { panic!() };
        assert_eq!(
            m.attachments[0].local_path.as_deref(),
            Some(dest.to_str().unwrap())
        );
    }
}
```

Build the two `test_*` fixture helpers on the patterns in `receive.rs`'s tests (`text_data_message`, `content`, `Received::Content`): the event fixture is a `SignalEvent::MessageReceived` whose `attachments` vec came from `map_attachment(&ap)` (so `local_path` starts None), and the `Received` fixture wraps a DataMessage whose `attachments` vec contains `ap`. Reuse `receive.rs`'s test helpers if they are reachable; otherwise write minimal local ones.

- [ ] **Step 3: Run them to make sure they fail**

Run: `cargo test --no-default-features --features native-backend attachments:: 2>&1 | tail -5`
Expected: compile errors — module and functions don't exist.

- [ ] **Step 4: Implement `attachments.rs`**

```rust
//! Receive-side attachment hydration (#643 U14).
//!
//! Downloads run on the engine thread against a lazily-loaded second
//! Manager (`get_attachment` takes &self; the receive stream owns the
//! session Manager mutably). Hydration happens after mapping and BEFORE
//! journaling, so journaled events carry local_path and replays agree
//! with the original delivery. A destination file that already exists
//! short-circuits the download - reconnect replays re-derive the same
//! name and reuse the blob.

use std::path::{Path, PathBuf};

use presage::libsignal_service::proto;
use presage::manager::{Manager, Registered};
use presage_store_sqlite::SqliteStore;

use crate::debug_log;
use crate::signal::parse::helpers::{contained_dest, sanitize_attachment_name};
use crate::signal::types::SignalEvent;

use super::receive;

/// Download-dir filename for a pointer, via the shared (signal-cli
/// parity) naming semantics: sender filename when present - sanitized,
/// it is attacker input - else an id-derived generated name.
pub(super) fn pointer_dest_name(ap: &proto::AttachmentPointer) -> String {
    let id = receive::attachment_id(ap);
    let content_type = ap.content_type.as_deref().unwrap_or("application/octet-stream");
    sanitize_attachment_name(ap.file_name.as_deref(), &id, content_type)
}

/// Fill `local_path` on every pointer-backed attachment in `events`.
/// Pairing is by index: the mapper walked the DataMessage's attachments
/// in order, so event attachments align with the item's pointers.
/// Failures leave local_path None - the message renders with the
/// existing placeholder.
pub(super) async fn hydrate(
    events: &mut [SignalEvent],
    item: &presage::model::messages::Received,
    manager: &mut Option<Manager<SqliteStore, Registered>>,
    store_file: &Path,
    download_dir: &Path,
) {
    let pointers = item_attachment_pointers(item);
    if pointers.is_empty() {
        return;
    }
    for event in events.iter_mut() {
        let SignalEvent::MessageReceived(message) = event else {
            continue;
        };
        for (attachment, ap) in message.attachments.iter_mut().zip(pointers.iter()) {
            if attachment.local_path.is_some() {
                continue;
            }
            attachment.local_path =
                fetch_one(ap, manager, store_file, download_dir).await;
        }
    }
}

/// The DataMessage attachment pointers inside a received item, in mapper
/// order - both the direct path and the sync-sent echo carry them.
fn item_attachment_pointers(
    item: &presage::model::messages::Received,
) -> Vec<proto::AttachmentPointer> {
    use presage::libsignal_service::content::ContentBody;
    use presage::model::messages::Received;
    let Received::Content(content) = item else {
        return Vec::new();
    };
    match &content.body {
        ContentBody::DataMessage(dm) => dm.attachments.clone(),
        ContentBody::SynchronizeMessage(sm) => sm
            .sent
            .as_ref()
            .and_then(|sent| sent.message.as_ref())
            .map(|dm| dm.attachments.clone())
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

// (item_attachment_pointers mirrors the same two paths receive.rs's
// mapper reads attachments from - if the import paths or field shapes
// differ from the mapper's actual code, copy the mapper's, since it
// compiles today.)

async fn fetch_one(
    ap: &proto::AttachmentPointer,
    manager: &mut Option<Manager<SqliteStore, Registered>>,
    store_file: &Path,
    download_dir: &Path,
) -> Option<String> {
    let name = pointer_dest_name(ap);
    let dest = contained_dest(download_dir, &name)?;
    if dest.exists() {
        return Some(dest.to_string_lossy().into_owned());
    }
    if manager.is_none() {
        match super::send::load_manager(store_file).await {
            Ok(m) => *manager = Some(m),
            Err(e) => {
                debug_log::logf(format_args!("attachment manager load failed: {e}"));
                return None;
            }
        }
    }
    let bytes = match manager.as_ref().expect("loaded above").get_attachment(ap).await {
        Ok(bytes) => bytes,
        Err(e) => {
            debug_log::logf(format_args!("attachment download failed: {e}"));
            return None;
        }
    };
    if let Err(e) = std::fs::create_dir_all(download_dir) {
        debug_log::logf(format_args!("attachment dir create failed: {e}"));
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(download_dir, std::fs::Permissions::from_mode(0o700));
    }
    match std::fs::write(&dest, &bytes) {
        Ok(()) => Some(dest.to_string_lossy().into_owned()),
        Err(e) => {
            debug_log::logf(format_args!("attachment write failed: {e}"));
            None
        }
    }
}
```

Fill in `item_attachment_pointers` by reading how `receive.rs` reaches DataMessages (the `Received::Content` → `ContentBody::DataMessage` and `ContentBody::SynchronizeMessage` → `sent.message` paths) and mirroring them. Make `load_manager` in `send.rs` `pub(super)`. Fix any import-path drift the compiler reports (e.g. `Received`'s actual module path — copy it from `supervisor.rs`'s imports).

- [ ] **Step 5: Wire the supervisor**

`spawn(store_file, journal_db, download_dir: PathBuf)`; thread it into `run_supervisor` and the session; before the stream loop add `let mut attachments_manager: Option<Manager<SqliteStore, Registered>> = None;`; replace the per-item mapping loop's entry so events are hydrated before journal/emit:

```rust
        let mut events = receive::map_received(&item, &own_aci, &resolver);
        attachments::hydrate(
            &mut events,
            &item,
            &mut attachments_manager,
            &store_file,
            &download_dir,
        )
        .await;
        for event in events {
            // existing journal-append + emit body, unchanged
```

`mod.rs`'s startup passes the configured download dir into `spawn` (same source the signal-cli path uses for `parse_attachment` — find where `App` stores it, around `app.rs:584`'s media state or the config; pass a clone).

- [ ] **Step 6: Run the new tests and the native suite**

Run: `cargo test --no-default-features --features native-backend 2>&1 | grep -E "^test result|FAILED"`
Expected: all green including the three new attachments tests.

- [ ] **Step 7: Commit**

```bash
git add src/backend/native/ src/signal/parse/helpers.rs
git commit -m "feat(native): download incoming attachments into the download dir (part of #643)"
```

---

### Task 3: Send-side attachment upload

**Files:**
- Modify: `src/backend/native/send.rs`, `src/backend/native/mod.rs`

**Interfaces:**
- Consumes: `ext_to_mime`, `sanitize_attachment_name` (Task 1); `Manager::upload_attachments(&self, Vec<(AttachmentSpec, Vec<u8>)>)`; `AttachmentSpec` (import from presage's re-export — `presage::libsignal_service::sender::AttachmentSpec`; verify with the compiler and adjust to the actual path).
- Produces: `SendCommand::Message` and `SendCommand::GroupMessage` each gain `attachment: Option<PathBuf>`; `pub(super) fn attachment_spec(path: &Path, len: usize) -> AttachmentSpec`.

- [ ] **Step 1: Write the failing tests in `send.rs`'s tests module**

```rust
    #[test]
    fn attachment_spec_derives_type_and_sanitized_name() {
        let spec = attachment_spec(std::path::Path::new("/tmp/pics/../holiday photo.JPG"), 12345);
        assert_eq!(spec.content_type, "image/jpeg");
        assert_eq!(spec.length, 12345);
        let name = spec.file_name.expect("file name set");
        assert!(!name.contains('/') && !name.contains(".."));
        assert!(name.contains("holiday"));
    }
```

And in `mod.rs`, replace `dispatch_gates_attachments_honestly` with routing coverage (attachments now route; the U14 gate is gone):

```rust
    #[tokio::test]
    async fn dispatch_routes_attachments_to_the_engine() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = file_backed_app(dir.path());
        let (_event_tx, _status_tx, mut command_rx, mut backend) = engine_backend();

        let mut with_attachment = message_req("+15550001111", "caption", 1);
        if let SendRequest::Message { attachment, .. } = &mut with_attachment {
            *attachment = Some(std::path::PathBuf::from("/tmp/x.png"));
        }
        backend.dispatch(&mut app, with_attachment).await;
        let send::SendCommand::Message { attachment, body, .. } =
            command_rx.try_recv().expect("attachment send reaches the engine")
        else {
            panic!("expected Message command");
        };
        assert_eq!(attachment.as_deref(), Some(std::path::Path::new("/tmp/x.png")));
        assert_eq!(body, "caption");

        let mut group = message_req("Z3JvdXBpZA==", "hi", 2);
        if let SendRequest::Message { is_group, attachment, .. } = &mut group {
            *is_group = true;
            *attachment = Some(std::path::PathBuf::from("/tmp/y.jpg"));
        }
        backend.dispatch(&mut app, group).await;
        let send::SendCommand::GroupMessage { attachment, .. } =
            command_rx.try_recv().expect("group attachment reaches the engine")
        else {
            panic!("expected GroupMessage command");
        };
        assert_eq!(attachment.as_deref(), Some(std::path::Path::new("/tmp/y.jpg")));
        assert_eq!(app.pending.sends.len(), 2);
    }
```

- [ ] **Step 2: Run them to make sure they fail**

Run: `cargo test --no-default-features --features native-backend attachment_spec dispatch_routes_attachments 2>&1 | tail -5`
Expected: compile errors (missing field/function) plus the removed-test reference.

- [ ] **Step 3: Implement**

`send.rs`:

```rust
/// Upload spec for an outgoing file. The basename passes the shared
/// sanitizer (paste temp files and user paths are trusted less than they
/// look; parity with the receive side costs nothing).
pub(super) fn attachment_spec(path: &Path, len: usize) -> AttachmentSpec {
    let content_type = crate::signal::parse::helpers::ext_to_mime(path);
    let base = path.file_name().and_then(|n| n.to_str());
    let file_name = Some(crate::signal::parse::helpers::sanitize_attachment_name(
        base,
        "outgoing",
        &content_type,
    ));
    AttachmentSpec {
        content_type,
        length: len,
        file_name,
        preview: None,
        voice_note: None,
        borderless: None,
        width: None,
        height: None,
        caption: None,
        blur_hash: None,
    }
}

/// Read + upload one attachment with the KTD-4 timeout; errors surface
/// as strings so the caller fails the send honestly.
async fn prepare_attachment(
    manager: &Manager<SqliteStore, Registered>,
    path: &Path,
) -> Result<proto::AttachmentPointer, String> {
    let bytes = tokio::fs::read(path).await.map_err(|e| format!("read {}: {e}", path.display()))?;
    let spec = attachment_spec(path, bytes.len());
    let uploads = tokio::time::timeout(SEND_TIMEOUT, manager.upload_attachments(vec![(spec, bytes)]))
        .await
        .map_err(|_| "attachment upload timed out".to_string())?
        .map_err(|e| e.to_string())?;
    uploads
        .into_iter()
        .next()
        .ok_or_else(|| "upload returned no pointer".to_string())?
        .map_err(|e| e.to_string())
}
```

Add `attachment: Option<PathBuf>` to both `SendCommand::Message` and `SendCommand::GroupMessage`. In `send_one` and `send_group_one`, before constructing the DataMessage:

```rust
    let mut attachments = Vec::new();
    if let Some(path) = attachment {
        match prepare_attachment(&manager, &path).await {
            Ok(pointer) => attachments.push(pointer),
            Err(e) => {
                debug_log::logf(format_args!("native send: attachment failed: {e}"));
                emit(&event_tx, SignalEvent::SendFailed { token });
                return;
            }
        }
    }
```

and set `attachments` on the DataMessage (add the field to both construction sites; for groups that means extending `group_data_message` with an `attachments: Vec<proto::AttachmentPointer>` parameter — update its unit test to pass an empty vec and add an assertion that a passed pointer lands in `dm.attachments`). `proto::AttachmentPointer` import: `presage::libsignal_service::proto` as in `receive.rs`.

`mod.rs` dispatch: delete the attachment gate block; pass `attachment` into both command constructions. Add paste-cleanup parity right after the successful `engine.commands.send(command)` path — mirror `src/backend/signal_cli.rs`'s block (lines ~136-144): for an attachment path under `app.paste_temp_path`, insert into `app.pending_paste_cleanups` keyed by the token with the `PASTE_CLEANUP_SENTINEL_SECS` sentinel. Update the dispatch doc comment (attachments now route; rich bodies remain U15).

- [ ] **Step 4: Run the native suite**

Run: `cargo test --no-default-features --features native-backend 2>&1 | grep -E "^test result|FAILED"`
Expected: all green, including the existing group KTD-4 and dispatch tests.

- [ ] **Step 5: Commit**

```bash
git add src/backend/native/send.rs src/backend/native/mod.rs
git commit -m "feat(native): upload and send attachments with captions (part of #643)"
```

---

### Task 4: Full verification, PR, Tier-3 manual

- [ ] **Step 1: Both lanes green + rustfmt**

```bash
cargo fmt --check
cargo clippy --tests --no-default-features --features native-backend -- -D warnings
cargo test --no-default-features --features native-backend
cargo clippy --tests -- -D warnings
cargo test
```

- [ ] **Step 2: Push and open the PR**

```bash
git push -u origin feature/643-u14-native-attachments
gh pr create --title "feat(native): U14 - attachments send/receive with download-dir parity (part of #643)" --body "<summary: hydration architecture (second manager, pre-journal download, replay dedup), shared sanitization boundary, upload with KTD-4 timeout, paste-cleanup parity. End with the Claude Code attribution footer.>"
```

- [ ] **Step 3: Tier-3 manual (human partner + phone)**

1. Send a photo from the phone (1:1) → lands in `download_dir` under the expected name, renders inline in siggy.
2. Send a non-image file from the phone → appears as `[attachment: name]`, file present in `download_dir`.
3. Send a voice note from the phone → renders as an attachment and plays with `o`.
4. From siggy, attach an image with a caption (existing file-picker path) → arrives on the phone with caption; checkmarks progress.
5. From siggy, send an attachment into the group → members receive it in the group.
6. Restart siggy, have the phone re-deliver nothing: previously downloaded attachments still render (paths persisted); no duplicate files in `download_dir`.

- [ ] **Step 4: After Tier-3 passes: record results in the PR body, wait for CI, squash merge, check U14 off in #643 with a results comment**

```bash
gh pr merge --squash --delete-branch
```
