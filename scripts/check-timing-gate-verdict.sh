#!/usr/bin/env bash
# Pin the timing gate's step independence and its verdict: an INVALID step
# must NOT end the gate, and a run with no red and at least one INVALID must
# exit 3, not 0 and not 1.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
STEPS=scripts/timing-gate-steps.sh
fail=0
TIMING_GATE_LOG_DIR="$(mktemp -d)"
export TIMING_GATE_LOG_DIR
trap 'rm -rf "$TIMING_GATE_LOG_DIR"' EXIT

run() {  # run <expected exit> <case> <steps...>
    local want="$1" case="$2"
    shift 2
    local out rc=0
    out="$(bash "$STEPS" "$@" 2>&1)" || rc=$?
    if [ "$rc" -ne "$want" ]; then
        echo "FAIL: $case: exit $rc, expected $want"
        echo "$out" | sed 's/^/    /'
        fail=1
        return
    fi
    LAST_OUT="$out"
    echo "ok: $case -> exit $rc"
}

# 1. A planted INVALID in step 1 leaves steps 2-4 running, and the gate says
#    "no verdict" with exit 3.
run 3 "an INVALID step 1 does not end the gate" \
    "planted latency rung" 'echo "[latency-slo gate] INVALID (no verdict): planted"; exit 1' \
    "step 2" 'echo two' \
    "step 3" 'echo three' \
    "step 4" 'echo four'
for step in two three four; do
    grep -q "^$step$" <<<"$LAST_OUT" \
        || { echo "FAIL: step printing '$step' did not run after the INVALID"; fail=1; }
done
grep -q "NO VERDICT (not red)" <<<"$LAST_OUT" \
    || { echo "FAIL: the gate did not report NO VERDICT"; fail=1; }

# 2. The UNJUDGED exit code of a step counts the same as an INVALID line.
run 3 "exit 3 from a step is no verdict" \
    "scale-gate shaped step" 'echo "UNJUDGED (not red) — host too busy"; exit 3' \
    "step 2" 'echo two'

# 3. A red anywhere outranks a no-verdict, and the later steps still ran.
run 1 "a red outranks a no verdict" \
    "planted INVALID" 'echo INVALID; exit 1' \
    "planted red" 'echo "assertion failed"; exit 101' \
    "step 3" 'echo three'
grep -q "^three$" <<<"$LAST_OUT" \
    || { echo "FAIL: step 3 did not run after a red"; fail=1; }

# 4. All green is the only green.
run 0 "all green" "a" 'echo a' "b" 'echo b'
grep -q "timing-gate PASS" <<<"$LAST_OUT" \
    || { echo "FAIL: an all-green run did not say PASS"; fail=1; }

# 5. A step's own pipeline failure is a red, not a green (pipefail per step).
run 1 "a failing command inside a pipe is red" "piped" 'false | cat'

[ "$fail" -eq 0 ] || { echo "check-timing-gate-verdict: FAILED"; exit 1; }
echo "check-timing-gate-verdict: PASS"
