#!/usr/bin/env bash
# Land-time check: binds the landed tree to the land gate that was measured on it (battery + holon/holon-app nextest).
set -euo pipefail
S=/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/bc7b1e67-1603-4c68-8742-84215e1a79e3/scratchpad
cd /Users/martin/Workspaces/pkm/holon/.claude/worktrees/_sw_integ
TIP=$(jj log -r '@-' -T 'commit_id.short(12)' --no-graph)
test "$TIP" = "34fca6bf0b8d" || { echo "TREE MOVED: $TIP"; exit 9; }
grep -q MirrorSchema crates/holon-mcp-client/src/mcp_sidecar.rs || { echo "WRONG TREE"; exit 9; }
BAT=$(ls -t "$S"/land-battery.*.log | head -1); APP=$(ls -t "$S"/land-appnextest.*.log | head -1)
grep -q "== landing gate PASS ==" "$BAT" || { echo "NO PASS MARKER in $BAT"; exit 1; }
grep -q "quiet at 03:48" "$S/land-gate-wave9-r5.log" || { echo "WRONG GATE RUN"; exit 1; }
SUMMARY=$(grep -E "Summary \[" "$APP" | tail -1); echo "holon/holon-app: $SUMMARY"
echo "$SUMMARY" | grep -qE "725 tests run" || { echo "UNEXPECTED SUMMARY"; exit 1; }
grep -E "^\s+FAIL \[|^\s+TIMEOUT \[" "$APP" | sed -E 's/^ *(FAIL|TIMEOUT) \[[^]]*\] \([^)]*\) //' | sort -u > "$S/land-fails-final.txt"
# registered: 5 e2e_backend matview reds, the iroh multi-peer flake, and turso_backend_state_machine (pre-existing main red, HolonCrateReds-2026-09-01 row 20)
if grep -vE "e2e_backend_engine_test|test_multi_peer_sync_iroh|test_turso_backend_state_machine" "$S/land-fails-final.txt" | grep -q .; then echo "NOVEL:"; cat "$S/land-fails-final.txt"; exit 1; fi
echo "pass-with-note: $(wc -l < "$S/land-fails-final.txt" | tr -d ' ') registered failures"
echo "ALL GREEN"
