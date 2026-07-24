#!/usr/bin/env bash
# Publish siggy to crates.io with the native-backend git dependencies
# stripped (#668 follow-up).
#
# crates.io rejects manifests with git dependencies, and presage is not
# published there, so the crates.io artifact ships the signal-cli backend
# only. That matches reality: the native backend cannot be built from a
# crates.io install regardless (its deps only exist as a pinned git tree).
#
# The script works on a throwaway git worktree of HEAD, deletes every
# block in Cargo.toml between "BEGIN CRATES-IO-STRIP" and
# "END CRATES-IO-STRIP" markers, and publishes from there. The repo tree
# is never modified.
#
# Usage:
#   scripts/publish-crate.sh          # dry run (verify build, no upload)
#   scripts/publish-crate.sh --live   # actually publish
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

if [[ -n "$(git status --porcelain)" ]]; then
    echo "error: working tree is dirty; publish from a clean release commit" >&2
    exit 1
fi

live=0
if [[ "${1:-}" == "--live" ]]; then
    live=1
elif [[ $# -gt 0 ]]; then
    echo "usage: $0 [--live]" >&2
    exit 1
fi

worktree=$(mktemp -d "${TMPDIR:-/tmp}/siggy-publish.XXXXXX")
cleanup() {
    git worktree remove --force "$worktree" 2>/dev/null || true
    rm -rf "$worktree"
}
trap cleanup EXIT

git worktree add --detach "$worktree" HEAD >/dev/null

sed -i '/BEGIN CRATES-IO-STRIP/,/END CRATES-IO-STRIP/d' "$worktree/Cargo.toml"

if grep -qE '^\s*git = |^\[patch\.' "$worktree/Cargo.toml"; then
    echo "error: strip markers did not cover every git dependency; check Cargo.toml" >&2
    exit 1
fi

# The stripped manifest no longer matches the committed lockfile (the
# presage tree drops out). Let cargo prune it in the worktree copy.
(cd "$worktree" && cargo update --workspace --offline >/dev/null 2>&1 || cargo update --workspace >/dev/null)

echo "== stripped manifest diff =="
diff Cargo.toml "$worktree/Cargo.toml" || true
echo "============================"

if [[ $live -eq 1 ]]; then
    (cd "$worktree" && cargo publish --allow-dirty)
else
    (cd "$worktree" && cargo publish --dry-run --allow-dirty)
    echo
    echo "Dry run complete. Re-run with --live to upload to crates.io."
fi
