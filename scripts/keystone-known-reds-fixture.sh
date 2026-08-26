#!/usr/bin/env bash
# Fixture check for scripts/keystone-known-reds.sh.
#
# This check replays the archived 2026-07-31 full-depth corpus through the
# classifier and asserts each still-registered key's hit count is exactly what
# the corpus contains.
#
# What it CAN catch: a `Match pattern` edited so it no longer matches the very
# payload that motivated its row, and an over-broad pattern that starts
# swallowing a neighbouring signature.
#
# What it CANNOT catch — do not rely on it for this: an assertion message
# REWORDED in production or test code. The corpus is frozen text, so a reword
# leaves it classifying exactly as before while the pattern silently stops
# matching what the code now emits. Only a fresh full-depth run surfaces that.
#
# The corpus is committed zstd-compressed next to the hand-authored regressions
# because it is evidence, not scratch: /tmp is cleared on reboot, and these four
# logs are the only decoded record of several families' actual failure payloads.
#
# Usage:
#   scripts/keystone-known-reds-fixture.sh            # check against expected
#   scripts/keystone-known-reds-fixture.sh --bless    # regenerate expected
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
corpus="$repo_root/crates/holon-integration-tests/hand-authored-regressions/fixture-logs-2026-07-31"
expected="$corpus/expected-classification.txt"

# The four runs that exited non-zero. The corpus also holds the four green runs
# of the same nights (as the base-rate record: 4 red / 8 total = 50%), but only
# failed runs are classifier input.
red_runs=(
    keystone-nightly-20260731-083505-run2
    keystone-nightly-20260731-191108-run1
    keystone-nightly-20260731-191108-run2
    keystone-nightly-20260731-193535-run1
)

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
: >"$work/empty.log"

logs=()
for run in "${red_runs[@]}"; do
    src="$corpus/$run.log.zst"
    [ -f "$src" ] || { echo "[fixture] missing corpus log: $src" >&2; exit 2; }
    zstd -dq -o "$work/$run.log" "$src"
    logs+=("$work/$run.log")
done

# The classifier exits 1 while any novel signature remains; that is a verdict
# about the corpus, not about this check, so capture it rather than inherit it.
raw="$work/classification.txt"
set +e
"$repo_root/scripts/keystone-known-reds.sh" "${logs[@]}" >"$raw" 2>&1
set -e

# Only per-key hit counts are pinned, and only for keys the LIVE registry still
# carries as `known-red`. The novel count is deliberately NOT pinned: fixing a
# family removes its row, after which that family's archived panics correctly
# classify as novel. Pinning the tally would make every successful fix look like
# a fixture regression — the guard would punish exactly the work it exists to
# protect.
summary="$work/summary.txt"
grep -E '^WARN known-red' "$raw" \
    | sed -E 's/^(WARN known-red \[[a-z-]+\] x[0-9]+).*/\1/' >"$summary"

# Drop expectations for rows that are no longer `known-red` in the registry —
# a fixed family is absence, not drift. Everything still registered must match
# its historical count exactly.
live="$work/live-keys.txt"
awk -F'|' '/^\| *`/ {
    gsub(/^ *`|` *$/, "", $2); gsub(/^ *| *$/, "", $3)
    if ($3 == "known-red") print $2
}' "$repo_root/docs/Testing/KeystoneKnownReds.md" >"$live"

if [ "${1-}" != "--bless" ] && [ -f "$expected" ]; then
    filtered="$work/expected-filtered.txt"
    : >"$filtered"
    while read -r line; do
        key=$(printf '%s' "$line" | sed -E 's/^WARN known-red \[([a-z-]+)\].*/\1/')
        if grep -qx -- "$key" "$live"; then
            printf '%s\n' "$line" >>"$filtered"
        else
            echo "[fixture] skipping [$key] — no longer a known-red row (fixed)."
        fi
    done <"$expected"
    expected="$filtered"
fi

if [ "${1-}" = "--bless" ]; then
    cp "$summary" "$expected"
    echo "[fixture] blessed:"
    cat "$expected"
    exit 0
fi

[ -f "$expected" ] || { echo "[fixture] no expected file — run with --bless" >&2; exit 2; }

if ! diff -u "$expected" "$summary"; then
    echo ""
    echo "[fixture] FAIL: a still-registered known-red row no longer classifies the"
    echo "          ARCHIVED payload it was written for. The corpus is immutable, so"
    echo "          this is a Match pattern in docs/Testing/KeystoneKnownReds.md that"
    echo "          drifted from its own evidence. Fix the pattern, then --bless."
    echo "          (Removing a row because its family is FIXED does not land here —"
    echo "          those keys are skipped, see the lines above.)"
    exit 1
fi
echo "[fixture] PASS — classifier verdict on the 2026-07-31 corpus is unchanged."

# ---------------------------------------------------------------------------
# Outcome classification. Distinct from the pattern-drift check above: this
# pins what the classifier says about a log BEFORE any signature matching —
# whether it decides the run failed, passed, or said nothing. Nothing here is
# blessable; each case asserts one fixed verdict and exit code.
#
# The green cases are the same nights' passing runs, not synthetic text: a green
# keystone log is thousands of lines of invariant chatter, and it was exactly
# that chatter the classifier had to stop reading as a failure.
outcome_fail=0
expect_outcome() {
    local label="$1" want_code="$2" want_line="$3"
    shift 3
    local out code=0
    out=$("$repo_root/scripts/keystone-known-reds.sh" "$@" 2>&1) || code=$?
    if [ "$code" -ne "$want_code" ] || ! printf '%s\n' "$out" | grep -q -- "$want_line"; then
        echo "[fixture] FAIL outcome/$label: want exit $want_code and a line matching"
        echo "          '$want_line'; got exit $code:"
        printf '%s\n' "$out" | tail -20 | sed 's/^/          | /'
        outcome_fail=1
        return
    fi
    echo "[fixture] ok outcome/$label (exit $code)"
}

green_runs=(
    keystone-nightly-20260731-080542-run1
    keystone-nightly-20260731-081152-run1
    keystone-nightly-20260731-081152-run2
    keystone-nightly-20260731-083505-run1
)
green_logs=()
for run in "${green_runs[@]}"; do
    src="$corpus/$run.log.zst"
    [ -f "$src" ] || { echo "[fixture] missing corpus log: $src" >&2; exit 2; }
    zstd -dq -o "$work/$run.log" "$src"
    green_logs+=("$work/$run.log")
done

# Synthetic shapes no real corpus run happens to have. Committed `.log.zst` like
# the corpus: `.gitignore`'s `*.log` would otherwise drop them from every clean
# checkout, taking the cases that depend on them with it.
for run in failed-no-signature no-outcome green-with-stray-error; do
    src="$corpus/synthetic/$run.log.zst"
    [ -f "$src" ] || { echo "[fixture] missing synthetic log: $src" >&2; exit 2; }
    zstd -dq -o "$work/$run.log" "$src"
done

expect_outcome green-single 0 '^\[known-reds\] PASS: 1 green run' "${green_logs[0]}"
expect_outcome green-all 0 '^\[known-reds\] PASS: 4 green run' "${green_logs[@]}"
# A green log alongside a red one must not dilute the red one's verdict.
expect_outcome green-plus-red 1 '^\[known-reds\] FAIL: ' "${green_logs[0]}" "${logs[0]}"
# The constraint this fix must not break: a run that genuinely failed with no
# extractable panic stays NOVEL.
expect_outcome failed-no-signature 1 'run failed but no panic signature' \
    "$work/failed-no-signature.log"
# A test printing an `error: ` line on its own stdout has not failed. Only the
# harness's own failure vocabulary outranks an explicit `test result: ok.`.
expect_outcome green-with-stray-error 0 '^\[known-reds\] PASS: 1 green run' \
    "$work/green-with-stray-error.log"
# Truncated and missing both exit 3, so each pins the MESSAGE only its own path
# produces — asserting the shared exit code would let either case stand in for
# the other, and truncation detection could be deleted unnoticed.
expect_outcome truncated 3 'the log states no pass/fail' "$work/no-outcome.log"
expect_outcome empty 3 'the log states no pass/fail' "$work/empty.log"
expect_outcome missing 3 '^\[known-reds\] UNREADABLE: ' "$work/does-not-exist.log"

if [ "$outcome_fail" -ne 0 ]; then
    echo ""
    echo "[fixture] FAIL: the classifier's outcome verdict changed. A green log read"
    echo "          as a failure is the 2026-08-25 false-NOVEL defect returning; a"
    echo "          failed or unreadable log read as a pass is worse."
    exit 1
fi
echo "[fixture] PASS — outcome classification unchanged."
exit 0
