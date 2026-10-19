#!/usr/bin/env bash
# Run the timing gate's steps independently and give ONE verdict over them
# (ruling 475/settle-then-judge.a, 2026-10-19).
#
# Usage: timing-gate-steps.sh <name> <command> [<name> <command> ...]
#
# The steps are arguments so this file can be pinned with planted steps
# (scripts/check-timing-gate-verdict.sh); `just timing-gate` passes the real
# four.
#
# Per step: exit 0 is GREEN, exit 3 or an `INVALID` line is NO VERDICT (the host
# was too busy to judge, which is not evidence about the tree), anything else is
# RED. A step never ends the gate: a wall-clock rung that could not judge must
# not hide whether the other rungs can.
#
# The gate: 1 if any step is RED, 3 (UNJUDGED, as `latency-scale-gate` uses it)
# if none is red and at least one gave no verdict, 0 only if all are green.
set -uo pipefail

[ "$#" -ge 2 ] && [ $(( $# % 2 )) -eq 0 ] || {
    echo "usage: $0 <name> <command> [<name> <command> ...]" >&2
    exit 2
}

mkdir -p target/gate-logs
total=$(( $# / 2 ))
i=0
names=()
verdicts=()
red=0
unjudged=0

while [ "$#" -gt 0 ]; do
    name="$1"
    cmd="$2"
    shift 2
    i=$(( i + 1 ))
    log="target/gate-logs/timing-gate-step-$i.log"
    echo "== timing-gate [$i/$total]: $name =="
    bash -o pipefail -euc "$cmd" 2>&1 | tee "$log"
    rc="${PIPESTATUS[0]}"
    if [ "$rc" -eq 0 ]; then
        verdict=GREEN
    elif [ "$rc" -eq 3 ] || grep -q INVALID "$log"; then
        verdict="NO VERDICT"
        unjudged=1
    else
        verdict=RED
        red=1
    fi
    echo "== timing-gate [$i/$total]: $name -> $verdict (exit $rc) =="
    names+=("$name")
    verdicts+=("$verdict (exit $rc)")
done

echo "== timing-gate steps =="
for n in "${!names[@]}"; do
    printf '   %-44s %s\n' "${names[$n]}" "${verdicts[$n]}"
done

if [ "$red" -eq 1 ]; then
    echo "== timing-gate RED =="
    exit 1
fi
if [ "$unjudged" -eq 1 ]; then
    echo "== timing-gate NO VERDICT (not red) — a step could not judge this host; re-run on a quiet machine =="
    exit 3
fi
echo "== timing-gate PASS =="
