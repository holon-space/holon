#!/usr/bin/env bash
# Detect defensive programming patterns in Rust code that swallow errors.
#
# Usage: ./scripts/check-defensive-code.sh [path]
# Default path: crates/ frontends/
#
# A match is suppressed when the matched line — or the line directly above it —
# carries an `// ALLOW(...)` annotation, matching the repo's disclosure convention.
# Exit status is non-zero when any unannotated suspicious line survives, so the
# script can gate CI.

set -euo pipefail

PATH_ARGS="${1:-crates/ frontends/}"
FOUND=0

# Always-excluded test surfaces (this audit targets prod code).
# `/target/`: the recursive grep starts at `frontends/`, and the out-of-workspace
# `frontends/holon-worker` keeps its own `target/` there — so running
# `just check-worker-wasm` drops vendored build-script `.rs` into this scan and
# the ratchet judges third-party code. Same defect archlint had (BugFunnel
# 2026-08-15, ORACLE).
TEST_EXCLUDE='/target/|/tests/|_test\.rs|_pbt\.rs'
ALLOW_TOKEN='ALLOW('

header() {
    echo ""
    echo "=== $1 ==="
    echo ""
}

# Drop matches whose own line, or the line immediately above, carries `ALLOW(`.
# Reads `file:line:content` (grep -rn) on stdin, prints survivors.
filter_allowed() {
    local match file rest line content prev
    while IFS= read -r match; do
        [ -z "$match" ] && continue
        file=${match%%:*}
        rest=${match#*:}
        line=${rest%%:*}
        content=${rest#*:}
        if [[ "$content" == *"$ALLOW_TOKEN"* ]]; then
            continue
        fi
        if [ "$line" -gt 1 ]; then
            prev=$(sed -n "$((line - 1))p" "$file")
            if [[ "$prev" == *"$ALLOW_TOKEN"* ]]; then
                continue
            fi
        fi
        printf '%s\n' "$match"
    done
}

# run_grep LABEL PATTERN [EXTRA_EXCLUDE_ERE]
# All filtering happens here (not in a downstream pipe) so FOUND survives and
# counts reflect the post-filter result set.
run_grep() {
    local label="$1"
    local pattern="$2"
    local extra="${3:-}"
    header "$label"

    local results
    results=$(grep -rn --include='*.rs' -E "$pattern" $PATH_ARGS 2>/dev/null || true)
    [ -z "$results" ] && { echo "(none found)"; return; }

    results=$(printf '%s\n' "$results" | grep -vE "$TEST_EXCLUDE" || true)
    if [ -n "$extra" ] && [ -n "$results" ]; then
        results=$(printf '%s\n' "$results" | grep -vE "$extra" || true)
    fi
    if [ -n "$results" ]; then
        results=$(printf '%s\n' "$results" | filter_allowed)
    fi

    if [ -n "$results" ]; then
        echo "$results"
        FOUND=$((FOUND + $(printf '%s\n' "$results" | grep -c . || true)))
    else
        echo "(none after filtering)"
    fi
}

echo "Defensive Programming Audit"
echo "==========================="
echo "Scanning: $PATH_ARGS"
echo "(Excludes test files and lines annotated with // ALLOW(...))"

# Pattern 1: .ok() on Result — converts to Option, silently dropping errors.
# Exclude: writeln!/write! (writing to strings), .set() (OnceLock), .send() (channels).
run_grep "P1: .ok() on Result (suspicious — may swallow errors)" \
    '\.ok\(\)\s*[;,)]' \
    'writeln!|write!|\.set\(|\.send\('

# Pattern 2: filter_map with .ok() — silently drops errors from iterators.
run_grep "P2: filter_map(|..| ...ok()) — silently drops errors from iterators" \
    'filter_map.*\.ok\(\)'

# Pattern 3: Err(e) => { log; continue/return } — logged but not propagated.
run_grep "P3: Err(e) => warn/error + continue (error logged but swallowed)" \
    'Err\(e\)\s*=>\s*\{'

# Pattern 4: if let Ok() without else — ignoring error case.
run_grep "P4: if let Ok() — may ignore error case" \
    'if let Ok\('

# Pattern 5: let _ = expr that returns Result.
run_grep "P5: let _ = <Result-producing expr> — discards Result" \
    'let _\s*=.*\.(await|send|write|execute|insert|remove|close)'

# Pattern 6: catch_unwind — swallowing panics.
run_grep "P6: catch_unwind — swallowing panics" \
    'catch_unwind'

# Pattern 7: unwrap_or_default() on Result — may hide parse/deser failures.
run_grep "P7: unwrap_or_default() — may hide failures" \
    'unwrap_or_default\(\)' \
    'env::var'

echo ""
echo "==========================="
echo "Total suspicious lines: $FOUND"
echo ""
echo "Review each match manually. Not all are bugs:"
echo "  - .ok() on OnceLock::set() is fine (double-init is expected)"
echo "  - .ok() on channel send() is often fine (no receivers)"
echo "  - writeln!().ok() on String is fine (infallible)"
echo "  - catch_unwind in actor loops may be intentional resilience"
echo "  - unwrap_or_default() on Option (not Result) is usually fine"
echo ""
echo "Annotate intentional cases with '// ALLOW(<reason>)' to suppress them."

[ "$FOUND" -eq 0 ]
