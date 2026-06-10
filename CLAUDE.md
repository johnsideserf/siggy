# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Test Commands

```bash
cargo build                    # dev build
cargo build --release          # release build
cargo test                     # run all tests
cargo test app::tests          # run app module tests only
cargo test signal::parse::tests  # run signal parser tests only
cargo test <test_name>         # run a single test by name
```

## Architecture

Terminal Signal messenger client wrapping signal-cli via JSON-RPC. Built on Tokio async runtime with Ratatui TUI.

### Data Flow

```
Keyboard → InputAction → App state → SignalClient (mpsc) → signal-cli (JSON-RPC over stdin/stdout)
signal-cli → JsonRpcResponse → SignalEvent (mpsc) → App state → SQLite + Ratatui render
```

### Key Modules

- **main.rs** — Event loop: polls keyboard (50ms), drains signal events, renders each frame. Orchestrates setup wizard → device linking → app startup. `dispatch_send()` routes each `SendRequest` variant to its `SignalClient` RPC method (handlers stay sync; the main loop owns async I/O).
- **app.rs** — Application state. `App` (field count CI-ratcheted, see below) holds mode (Normal/Insert), per-overlay key handlers, and sub-structs extracted into `src/domain/`. Conversations live in `ConversationStore` (`conversation_store.rs`: HashMap + ordered Vec for the sidebar). ~40% of the file is the inline test module.
- **handlers/** — Backend/composer event handling extracted from app.rs: `signal.rs` (`handle_signal_event()`, the single entry point for all backend events), `input.rs` (composer text → `SendRequest`), `keys.rs` (shared key-action helpers). Overlay key handlers are still in app.rs (migration ongoing, #494).
- **domain/** — Extracted `App` sub-state (scroll, input, pending, overlays, image, lock, typing, search, …) per the #352 roadmap. Should be a leaf layer; three modules still import from app:: (#495).
- **conversation_store.rs** — `ConversationStore`: conversation map, ordering, read markers. `get_or_create_conversation()` is the single point for ensuring a conversation exists (upserts memory + SQLite).
- **signal/client.rs** — Spawns signal-cli child process. Tokio tasks: stdout reader (parses JSON-RPC into `SignalEvent`), stdin writer (sends `JsonRpcRequest`). `pending_requests` map correlates RPC call IDs with their method.
- **signal/parse/** — JSON-RPC → `SignalEvent` parsing, split by concern: `envelope.rs`, `message.rs`, `helpers.rs` (attachment handling), `rpc.rs`, `poll.rs`.
- **signal/types.rs** — Shared types: `SignalEvent` enum, `SignalMessage`, `Contact`, `Group`, JSON-RPC structs.
- **ui/** — Rendering: `mod.rs` (`draw()`), `chat_pane.rs`, `sidebar.rs`, `status_bar.rs`, `composer.rs`, `overlays/` (one file per overlay), `links.rs` (OSC 8 hyperlink post-render injection). Caveat: `draw()` takes `&mut App` and writes layout feedback (scroll clamping, focus derivation, mouse hit-rects) during render — see #496 before relying on "stateless rendering".
- **db.rs** — SQLite with WAL mode; version-based migrations (conversations, messages, read_markers, reactions, poll tables).
- **list_overlay.rs** — Shared list-overlay helpers (key classification, nav, scroll layout, index clamping). Use these for any new overlay; partial adoption is tracked in #499.
- **config.rs** — TOML config at platform-specific path. Fields: `account` (E.164 phone), `signal_cli_path`, `download_dir`, plus UI preferences.
- **input.rs** — Parses text input into `InputAction` enum (slash commands).
- **keybindings.rs / theme.rs** — Keybinding profiles with overrides; color themes incl. custom theme TOML loading.
- **setup.rs** — Multi-step first-run wizard (signal-cli detection, phone input, QR device linking).
- **link.rs** — Device linking flow with QR code display and account registration check.

### Conversations

Keyed by phone number (1:1) or group ID (groups). `get_or_create_conversation()` is the single point for ensuring a conversation exists — it upserts to both the in-memory HashMap and SQLite. New conversations append to `conversation_order`; existing ones are no-ops.

### Signal-CLI Communication

Notifications (incoming messages, typing, receipts) arrive as JSON-RPC requests with a `method` field. RPC responses (listContacts, listGroups) arrive with a `result` field and are matched by request ID via `pending_requests`. Both flows produce `SignalEvent` variants sent through the same mpsc channel.

### Modal Input

Insert mode (default) for typing messages. Normal mode (Esc) for vim-style navigation: j/k scroll, h/l cursor, w/b word movement, i/a/I/A/o to re-enter Insert.

## Git Workflow

Never commit directly to master. Always follow this process:

1. **Create a feature branch** before making any changes
2. **Run checks** before pushing: `cargo clippy --tests -- -D warnings && cargo test`
3. **Push** the branch to origin with `-u`
4. **Create a PR** via `gh pr create` targeting master
5. **Review** the PR diff, wait for CI to pass, then **squash merge** with `gh pr merge --squash --delete-branch`

Master is force-push protected.

### Issue Linking

Reference the GitHub issue number in commit messages and PR descriptions (e.g. `closes #29`). This auto-closes the issue when the PR is merged.

### Branch Naming

Use prefixed names: `feature/`, `fix/`, `refactor/`, `docs/` (e.g. `feature/dark-mode`, `fix/unread-count`, `docs/update-readme`).

### Exceptions

Trivial docs-only changes (CLAUDE.md tweaks, typo fixes) may be committed directly to master. All code changes must go through a PR.

## App field-count ratchet

`scripts/check-app-field-count.sh` runs in CI and fails PRs that grow the `App` struct beyond its committed baseline. To add a new field to `App`, first extract an existing one into `src/domain/` so the net count stays flat. See issue #352 for the current extraction roadmap.

If an addition is genuinely unavoidable (rare), bump `BASELINE` in the script and justify the increase in the PR body.

## Toolchain

The Rust toolchain is pinned in `rust-toolchain.toml` to keep CI deterministic - a new stable Rust release cannot break CI without a PR.

### Updating the pin

1. Read the [Rust release blog](https://blog.rust-lang.org/) for the new version.
2. Bump `channel` in `rust-toolchain.toml`.
3. Run `cargo clippy --tests -- -D warnings` locally; fix any new lints.
4. Open a PR. CI will run against the new pin.

## Releases

CI runs automatically on every push/PR (`.github/workflows/ci.yml`). Releases are triggered by pushing a version tag.

### Creating a Release

```bash
git tag v0.2.0
git push origin v0.2.0
```

This triggers `.github/workflows/release.yml` which:
1. Runs clippy + tests
2. Builds binaries for 4 targets: Linux x86_64, macOS x86_64, macOS arm64, Windows x86_64
3. Creates a GitHub Release with auto-generated changelog and attached archives

### Version Tags

Use semver: `v0.1.0`, `v0.2.0`, `v1.0.0`. Remember to update `version` in `Cargo.toml` before tagging.

### Install Scripts

- `install.sh` — Linux/macOS one-liner (`curl -fsSL .../install.sh | bash`)
- `install.ps1` — Windows one-liner (`irm .../install.ps1 | iex`)

Both download the latest release binary and handle signal-cli setup.
