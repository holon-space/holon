#!/usr/bin/env bash
set -euo pipefail
S=/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/1d3fdfe9-af2d-42a8-aecb-fbc009830160/scratchpad
cd /Users/martin/Workspaces/pkm/holon/.claude/worktrees/_sw_integ
while true; do
  l=$(sysctl -n vm.loadavg | awk '{print int($2)}')
  r=$(pgrep -x rustc | wc -l | tr -d ' ')
  echo "$(date +%H:%M) load1=$l rustc=$r"
  if [ "$l" -lt 10 ] && [ "$r" -lt 3 ]; then break; fi
  sleep 120
done
echo "quiet at $(date +%H:%M) — running land gate"
bash $S/land-gate.sh
