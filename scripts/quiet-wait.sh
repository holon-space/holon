#!/usr/bin/env bash
# Takes the machine-wide quiet hold for pid $2 through the slot script $1, polling every
# GATE_STEP_POLL_S seconds while another job holds it (exit 75). Prints the slot script's refusal
# (its last stderr line, which names the holder) once per holder, and its other stderr lines as they
# come; after a wait, one line with its length. A take without a wait prints neither.
# Exit: the take's, 0 = taken.
set -euo pipefail
slot=$1 pid=$2 first="" seen=""
err=$(mktemp)
trap 'rm -f "$err"' EXIT
iso() { date -u -r "$1" +%Y-%m-%dT%H:%M:%SZ; }
while :; do
  rc=0; bash "$slot" --quiet-hold "$pid" 2>"$err" || rc=$?
  [ "$rc" = 75 ] || break
  now=$(date +%s)
  first=${first:-$now}
  sed '$d' "$err" >&2
  if [ "$(tail -n 1 "$err")" != "$seen" ]; then
    seen=$(tail -n 1 "$err")
    echo "quiet wait at $(iso "$now"): $seen"
  fi
  sleep "${GATE_STEP_POLL_S:-5}"
done
cat "$err" >&2
[ "$rc" = 0 ] || { echo "QUIET HOLD NOT TAKEN: with-build-slot.sh --quiet-hold exit $rc"; exit "$rc"; }
[ -z "$first" ] || { now=$(date +%s); echo "quiet wait: ended after $((now - first)) s; took the quiet hold at $(iso "$now")"; }
