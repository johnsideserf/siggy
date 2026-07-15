---
title: Native backend spike findings (presage link/send/receive go verdict)
date: 2026-07-15
category: architecture
module: backend / native
problem_type: architecture
component: native-backend
symptoms:
  - "Phase 2 (#642) gated on proving presage can link, receive, and send against production Signal"
  - "plan assumed Windows would not build the presage tree"
  - "unknown semantics: send Ok meaning, QueueEmpty timing, !Send store futures vs Tokio main loop"
root_cause: architecture
resolution_type: verified_go
severity: high
tags: [native-backend, presage, spike, linking, issue-639, issue-642, phase-0]
---

# Native backend spike findings (#639)

Live test run 2026-07-15 on Windows 11 (MSVC toolchain) against production
Signal, using the maintainer's primary account as a secondary linked device
("siggy-spike"). Spike branch: `spike/native-backend` (never merged), binary
`native-spike.exe` behind the `native-spike` Cargo feature.

**Verdict: GO for Phase 2 (#642).** All four spike questions answered
affirmatively; no blockers found.

## Pinned revision

presage main HEAD `63482efd` (2026-07-07). Not optional: upstream PR #421
(linking fix) merged that day; older revs (including gurk's April pin)
cannot link against Signal mobile 8.17+. U9 must pin this rev or later.

## The four spike questions

### 1. Does linking work at the pinned rev?

**Yes.** `Manager::link_secondary_device` produced a provisioning URI over
its oneshot channel; rendered as a terminal QR (qrcode crate, Dense1x2
unicode, inverted for dark terminals); scanned with Signal Android; manager
resolved with `LINKED OK`. The device appears as "siggy-spike" in Linked
Devices.

Two operational notes:

- The provisioning URI expires after roughly a minute. An expired session
  fails gracefully: `link failed: failed to provision device: no
  provisioning message received`, exit without panic.
- A failed/timed-out provisioning attempt does NOT corrupt the store. The
  second `link` run on the same sqlite store succeeded.

### 2. What does `Manager::send_message` resolving `Ok` mean?

**Server-accepted and delivered.** A `DataMessage` sent to the account's
own ACI returned `Ok` and the message appeared on the phone (Note to Self)
immediately. `Ok` is a real acceptance signal, strong enough to drive the
existing sent-status pipeline (the signal-cli adapter's equivalent is the
send RPC response timestamp).

### 3. `Received::QueueEmpty` behavior?

**Fires promptly and reliably on both initial and re-opened streams.**

- Fresh stream, first link, empty backlog: `QueueEmpty` arrived immediately
  after the stream opened, before any live traffic.
- Re-opened stream (process restarted): `QueueEmpty` again arrived
  immediately.

This is a sound "caught up" signal for the startup-sync gate (the
signal-cli adapter's sync-completion equivalent).

### 4. Does the LocalSet / `!Send` shape coexist with a Tokio main loop?

**Yes.** Parts of presage-store-sqlite's futures are `!Send`; the spike
drove everything from a `tokio::task::LocalSet` on a current-thread
runtime (the shape planned for the real adapter: a dedicated thread owning
the Manager, per KTD-8 / gurk's local_pool). No runtime friction observed,
including on Windows.

## Bonus findings (not spike questions, but adapter-relevant)

- **Windows MSVC builds the entire presage tree clean.** This overturns
  the plan's assumption that Windows would be deferred. Requirements:
  protoc on PATH (29.3 works), rusqlite downgraded 0.40 -> 0.38
  (libsqlite3-sys `links = "sqlite3"` conflict with presage-store-sqlite's
  0.36), and a `[patch.crates-io]` entry for Signal's curve25519-dalek
  fork.
- **Note to Self / own-device sends arrive as
  `SynchronizeMessage(SyncMessage { sent: Some(...) })`** - the same
  envelope shape signal-cli exposes as `syncMessage.sentMessage`. The
  existing sync-handling model in handlers/signal.rs maps onto presage
  without redesign.
- **Contact sync does not arrive automatically.** No `Contacts` item
  showed up during the receive windows after linking. E.164-based send
  therefore fails until a sync lands; the spike's send leg used the ACI
  uuid directly. The U10-U12 adapter needs ACI-first addressing and/or an
  explicit `request_contacts_sync` at startup (KTD-6).
- **Store location and hygiene**: the spike store is plaintext SQLite
  holding live identity keys. The real store (U9) must live under the
  platform data dir with the 0700 permission helper, and
  `--reset-account` tooling must learn the path.
- **Cargo feature gating works**: the spike bin sits behind a
  `native-spike` feature so the dep tree stays out of default builds.
  Reminder for CI scripting: `cargo build --bin native-spike` without the
  feature fails; a piped `| tail` masks that failure (check PIPESTATUS).

## Implications for Phase 2 (#642)

U9 (crate plumbing) starts with its riskiest unknowns already resolved:
the rev pin, the rusqlite downgrade, the curve25519 patch, protoc in CI,
and the LocalSet thread shape are all validated. The remaining Phase 2 gate
is the v1.14.0 boundary-soak window (until ~2026-07-21).
