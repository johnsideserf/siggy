#!/usr/bin/env bash
#
# App field-count ratchet.
#
# App was a single 85-field god object at the time this guardrail was added.
# Extracting fields into src/domain/ sub-structs is ongoing (see issue #352).
# This script enforces the ratchet: App's field count must not grow beyond the
# committed baseline. To add a field, extract an existing one first so the net
# count stays flat or drops.
#
# If a field addition is genuinely necessary (rare), lower BASELINE at the same
# time as the extraction, or raise BASELINE with a justification in the PR body.
#
set -euo pipefail

BASELINE=71

count=$(awk '
  /^pub struct App \{/ { inside=1; next }
  inside && /^}/        { inside=0; exit }
  inside && /^ *pub [a-z_]/ { n++ }
  END { print n+0 }
' src/app.rs)

if [ "$count" -gt "$BASELINE" ]; then
  cat >&2 <<MSG
App field-count ratchet failed: $count fields, baseline is $BASELINE.

To add a field to App, first extract an existing one into src/domain/ so the
net count stays flat. See issue #352 for the current extraction roadmap and
docs/ideation/2026-04-22-maintainability-ideation.md for the rationale.

If this addition is genuinely unavoidable, bump BASELINE in
scripts/check-app-field-count.sh and justify the increase in the PR body.
MSG
  exit 1
fi

echo "App field count: $count (baseline: $BASELINE)"
