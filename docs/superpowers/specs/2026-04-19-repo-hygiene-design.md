# Repo Hygiene Design

**Goal:** Harden the project against the kinds of latent issues that caused the April 11-19 CI outage: a floating Rust toolchain picked up Rust 1.95's new clippy lints, producing 33 errors on code that hadn't changed. Master was red for about a week before anyone noticed.

## Problem

Three hygiene gaps surfaced during the outage audit:

1. **CI toolchain floats.** `dtolnay/rust-toolchain@stable` in `ci.yml` and `release.yml` picks up whatever stable Rust happens to ship on the day CI runs. When Rust 1.95 stabilized `collapsible_match`, `unnecessary_sort_by`, and `manual_checked_ops`, master went red without any code change on our end.

2. **Formatting drift.** No `rustfmt.toml`, no CI check. `cargo fmt --check` currently fails on dozens of files. External contributors who run `cargo fmt` get noise-filled PRs (shwoop's #319 ballooned from a focused mute feature to +6348/-2187 across 22 files, mostly rustfmt churn).

3. **Branch protection is thin.** The `no-delete-forcepush` ruleset blocks deletion and force-push, but does not require CI to pass before merge. Nothing prevents merging a red PR.

## Non-Goals

- Edition bump 2021 → 2024. Mechanical but not urgent. Tracked as a follow-up issue.
- Release-profile test run. Dev-profile tests already catch real bugs; adding a release test would double CI time for marginal value.
- Adopting a formal MSRV policy. Out of scope; can be decided later.

## Architecture

Three small, independent changes. Each ships as its own PR so any single one can be reverted without affecting the others.

### 1. Pin the Rust toolchain

Add `rust-toolchain.toml` at the repo root:

```toml
[toolchain]
channel = "1.95.0"
components = ["clippy", "rustfmt"]
```

CI picks this up automatically via `dtolnay/rust-toolchain@stable` (the action respects `rust-toolchain.toml` when present). No workflow edits needed.

Update `.github/dependabot.yml` to watch the toolchain file? **No** - dependabot doesn't support `rust-toolchain.toml` natively. Instead, we rely on the existing manual update cadence: when a new stable lands with features worth adopting, bump the pin in a PR like any other dependency. The Rust release blog will surface what's new.

Document the pin-bump process in `CLAUDE.md` under a new "Toolchain updates" section.

### 2. Commit one-off `cargo fmt` pass and enforce in CI

Two-commit PR:

1. **Commit 1**: `cargo fmt` run across the entire codebase. Large diff (~50 files), purely mechanical. Review focus: confirm no logic touched, just whitespace and line wrapping.
2. **Commit 2**: Add `cargo fmt --check` step to `ci.yml` right before the clippy step.

```yaml
- name: Format check
  run: cargo fmt --check
- name: Clippy
  run: cargo clippy --tests -- -D warnings
```

Intentionally skipping `rustfmt.toml`. The existing codebase style is *almost* default rustfmt - letting default rustfmt be the source of truth removes the need to author and maintain a config file.

Tradeoff accepted: some hand-tuned compact lines (long struct initializers, compact match arms in `theme.rs`) will expand across multiple lines. The one-time readability loss is worth the permanent reviewability gain.

### 3. Require CI to pass before merge

Update the existing `no-delete-forcepush` ruleset (ID 13341653) via the GitHub API or UI to add a `required_status_checks` rule:

```json
{
  "type": "required_status_checks",
  "parameters": {
    "required_status_checks": [
      { "context": "Lint & Test" }
    ],
    "strict_required_status_checks_policy": false
  }
}
```

`strict_required_status_checks_policy: false` means branches don't need to be up-to-date with master, just passing CI. This matches the current squash-merge flow where PRs aren't auto-rebased.

This cannot be fully automated from a spec - it's a repo settings change. The spec's job is to document the target configuration; the actual change is a ~30-second click in the GitHub UI.

## Execution order

1. Pin toolchain (unblocks #2 - need to know what rustfmt version the CI check will use).
2. One-off `cargo fmt` + CI check (introduces the format check that #3's required-status rule will enforce).
3. Required status check on ruleset (enforces #1 and #2).

Doing these in order means each step's CI confirms the prior step is working before the next one tightens the screws.

## Rollback

All three changes are independently revertible:

- Toolchain pin: delete `rust-toolchain.toml`. CI falls back to floating stable.
- Fmt check: remove the CI step. The committed fmt pass stays (no harm).
- Required status check: remove via GitHub UI. Ruleset reverts to deletion + force-push only.

## Testing

- **Toolchain pin**: CI green on the PR that adds `rust-toolchain.toml`. Verify `cargo clippy` in CI uses 1.95.0 via the step log.
- **Fmt pass**: `cargo test` passes on the fmt-pass PR (no logic changes). CI green.
- **Required status check**: open a throwaway PR with a deliberately failing commit; confirm the merge button is blocked. Close without merging.

## Follow-ups

- File issue: "Upgrade Cargo edition 2021 → 2024". Link to Rust 2024 migration guide.
- Monitor the next 2-3 stable Rust releases to confirm the pin-bump workflow is sustainable. If bumping feels like friction, consider a scheduled CI job that tests against `beta` as an early-warning signal.
