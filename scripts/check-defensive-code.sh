#!/usr/bin/env bash
# Detect defensive programming patterns in Rust code that swallow errors.
# Uses ast-grep for AST-based matching and grep for text-based fallbacks.
#
# Usage: ./scripts/check-defensive-code.sh [path]
# Default path: crates/ frontends/

set -euo pipefail

PATH_ARGS="${1:-crates/ frontends/}"
FOUND=0

header() {
    echo ""
    echo "=== $1 ==="
    echo ""
}

run_ast_grep() {
    local label="$1"
    local pattern="$2"
    shift 2
    header "$label"
    local count
    count=$(ast-grep --pattern "$pattern" --lang rust $@ 2>/dev/null | wc -l || true)
    if [ "$count" -gt 0 ]; then
        ast-grep --pattern "$pattern" --lang rust $@ 2>/dev/null || true
        FOUND=$((FOUND + count))
    else
        echo "(none found)"
    fi
}

run_grep() {
    local label="$1"
    local pattern="$2"
    shift 2
    header "$label"
    local results
    results=$(grep -rn --include='*.rs' -E "$pattern" $@ 2>/dev/null || true)
    if [ -n "$results" ]; then
        echo "$results"
        local count
        count=$(echo "$results" | wc -l)
        FOUND=$((FOUND + count))
    else
        echo "(none found)"
    fi
}

echo "Defensive Programming Audit"
echo "==========================="
echo "Scanning: $PATH_ARGS"
echo "(Excludes test files and writeln!/fmt::Write .ok() calls)"

# Pattern 1: .ok() on Result — converts to Option, silently dropping errors
# Exclude: writeln!().ok() (writing to strings), OnceLock::set().ok(), send().ok()
run_grep "P1: .ok() on Result (suspicious — may swallow errors)" \
    '\.ok\(\)\s*[;,)]' \
    $PATH_ARGS \
    | grep -v 'writeln!' \
    | grep -v 'write!' \
    | grep -v '\.set(' \
    | grep -v '\.send(' \
    | grep -v '/tests/' \
    | grep -v '_test.rs' \
    | grep -v '_pbt.rs' \
    || echo "(none after filtering)"

# Pattern 2: filter_map with .ok() — silently drops errors from iterators
run_grep "P2: filter_map(|..| ...ok()) — silently drops errors from iterators" \
    'filter_map.*\.ok\(\)' \
    $PATH_ARGS \
    | grep -v '/tests/' \
    | grep -v '_test.rs' \
    || echo "(none after filtering)"

# Pattern 3: Err(e) => { log; continue/return } — logged but not propagated
run_grep "P3: Err(e) => warn/error + continue (error logged but swallowed)" \
    'Err\(e\)\s*=>\s*\{' \
    $PATH_ARGS \
    | grep -v '/tests/' \
    | grep -v '_test.rs' \
    | grep -v '_pbt.rs' \
    || echo "(none after filtering)"

# Pattern 4: if let Ok() without else — ignoring error case
run_grep "P4: if let Ok() — may ignore error case" \
    'if let Ok\(' \
    $PATH_ARGS \
    | grep -v '/tests/' \
    | grep -v '_test.rs' \
    | grep -v '_pbt.rs' \
    || echo "(none after filtering)"

# Pattern 5: let _ = expr that returns Result
run_grep "P5: let _ = <Result-producing expr> — discards Result" \
    'let _\s*=.*\.(await|send|write|execute|insert|remove|close)' \
    $PATH_ARGS \
    | grep -v '/tests/' \
    | grep -v '_test.rs' \
    || echo "(none after filtering)"

# Pattern 6: catch_unwind — swallowing panics
run_grep "P6: catch_unwind — swallowing panics" \
    'catch_unwind' \
    $PATH_ARGS \
    | grep -v '/tests/' \
    | grep -v '_test.rs' \
    || echo "(none after filtering)"

# Pattern 7: unwrap_or_default() on Result — may hide parse/deser failures
run_grep "P7: unwrap_or_default() — may hide failures" \
    'unwrap_or_default\(\)' \
    $PATH_ARGS \
    | grep -v '/tests/' \
    | grep -v '_test.rs' \
    | grep -v '_pbt.rs' \
    | grep -v 'env::var' \
    || echo "(none after filtering)"

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
