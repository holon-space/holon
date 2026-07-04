#!/usr/bin/env bash
# Baseline-ratchet wrapper around check-defensive-code.sh.
#
# The raw audit reports a large stock of pre-existing defensive-code violations
# that we are not fixing in one go. This wrapper lets the quality gate fail only
# on *new* violations: it compares the current audit against a committed baseline
# (scripts/defensive-baseline.txt) and exits non-zero only if a signature appears
# now that was not in the baseline.
#
# Signature = "<path>\t<trimmed source line>" (the line NUMBER is deliberately
# dropped so that moving unrelated code does not register as a new violation).
# This is intentionally permissive: swapping one bad line for another identical
# one in the same file is not caught, but any genuinely new offending line, and
# any new file, is.
#
# Usage:
#   scripts/defensive-ratchet.sh            # gate: fail on NEW violations
#   scripts/defensive-ratchet.sh --update   # regenerate the baseline from HEAD
#
# After an intentional, reviewed reduction (or unavoidable addition) run
# `--update` and commit scripts/defensive-baseline.txt.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BASELINE="$SCRIPT_DIR/defensive-baseline.txt"
AUDIT="$SCRIPT_DIR/check-defensive-code.sh"

# Emit the normalized signature set (sorted, unique) on stdout.
current_signatures() {
    # Audit exits non-zero when violations exist; that is expected, so tolerate it.
    local raw
    raw="$("$AUDIT" 2>/dev/null || true)"
    printf '%s\n' "$raw" | grep -E '\.rs:[0-9]+:' | \
        sed -E 's/^([^:]+):[0-9]+:[[:space:]]*(.*)$/\1'$'\t''\2/' | \
        sed -E 's/[[:space:]]+$//' | \
        sort -u
}

cd "$REPO_ROOT"

if [ "${1:-}" = "--update" ]; then
    current_signatures > "$BASELINE"
    echo "Baseline updated: $BASELINE ($(wc -l < "$BASELINE" | tr -d ' ') violations)"
    exit 0
fi

if [ ! -f "$BASELINE" ]; then
    echo "ERROR: baseline $BASELINE missing. Run: scripts/defensive-ratchet.sh --update" >&2
    exit 2
fi

CURRENT="$(current_signatures)"
# Lines present now but absent from the baseline = new violations.
NEW="$(comm -13 "$BASELINE" <(printf '%s\n' "$CURRENT") || true)"

if [ -n "$NEW" ]; then
    echo "NEW defensive-code violations (not in baseline):"
    echo ""
    printf '%s\n' "$NEW" | sed 's/\t/  ->  /'
    echo ""
    echo "These swallow errors. Fix them, or annotate the line with // ALLOW(<reason>)."
    echo "If genuinely intentional and reviewed, run: scripts/defensive-ratchet.sh --update"
    exit 1
fi

BASE_COUNT=$(wc -l < "$BASELINE" | tr -d ' ')
CUR_COUNT=$(printf '%s\n' "$CURRENT" | grep -c . || true)
echo "defensive-code ratchet OK: no new violations (baseline $BASE_COUNT, current $CUR_COUNT)."
