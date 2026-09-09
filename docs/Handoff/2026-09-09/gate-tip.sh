#!/usr/bin/env bash
# usage: bash gate-tip.sh <lane> <tip-commit> <downstream-lane...>
# Gate a (conflict-resolved) chain tip in _sw_integ under the lane's gate case, then release the held downstream weaves.
set -uo pipefail
trap "" TERM HUP
SP=/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/bc7b1e67-1603-4c68-8742-84215e1a79e3/scratchpad
INTEG=/Users/martin/Workspaces/pkm/holon/.claude/worktrees/_sw_integ
LANE=${1:?lane}; TIP=${2:?tip}; shift 2
LOG=$SP/weave-${GATE_LOG_LANE:-$LANE}.log
cd "$INTEG" || exit 9
jj workspace update-stale > /dev/null 2>&1
jj new "$TIP" > /dev/null 2>&1 || { echo "jj new $TIP failed" | tee -a "$LOG"; exit 9; }
parent=$(jj log -r '@-' --no-graph -T 'commit_id.short(12)')
[ "$parent" = "$TIP" ] || { echo "integ parent $parent != $TIP" | tee -a "$LOG"; exit 9; }
grep -rlI '<<<<<<< conflict' crates docs | grep -q . && { echo "WRONG TREE (conflict markers)" | tee -a "$LOG"; exit 9; }
cp "$SP/weave-gate.sh" "$SP/weave-gate.run-$LANE-resolved.sh"
echo "resolved-tip gate at $TIP started $(date)" >> "$LOG"
bash "$SP/weave-gate.run-$LANE-resolved.sh" "$LANE" > "$SP/gate-$LANE-resolved.out" 2>&1
rc=$?
tail -5 "$SP/gate-$LANE-resolved.out" >> "$LOG"
if [ $rc -eq 0 ]; then
  echo "[weave-gate] GREEN" >> "$LOG"
  for extra in ${GATE_ALSO_GREEN:-}; do printf "[weave-gate] GREEN\nweave-exit=0\n" >> "$SP/weave-$extra.log"; done
  echo "weave-exit=0" >> "$LOG"
  cd /Users/martin/Workspaces/pkm/holon
  for l in "$@"; do
    : > "$SP/weave-$l.log"
    nohup bash "$SP/weave-$l.sh" > "$SP/weave-$l.log" 2>&1 &
  done
  echo "downstream weaves relaunched: $*" >> "$LOG"
else
  echo "[weave-gate] RED rc=$rc (resolved-tip gate; see gate-$LANE-resolved.out)" >> "$LOG"
  echo "weave-exit=$rc" >> "$LOG"
fi
exit $rc
