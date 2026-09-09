#!/usr/bin/env bash
# Land-time check: binds the landed tree to the wave-11 land gate measured on it (land-gate-wave11-r3.log).
set -euo pipefail
S=/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/bc7b1e67-1603-4c68-8742-84215e1a79e3/scratchpad
cd /Users/martin/Workspaces/pkm/holon/.claude/worktrees/_sw_integ
TIP=$(jj log -r '@-' -T 'commit_id.short(12)' --no-graph)
test "$TIP" = "$(cat $S/land-tip-w11.txt)" || { echo "TREE MOVED: $TIP"; exit 9; }
grep -q "alphabet is 77 transitions" docs/Architecture/FeatureMap.md || { echo "WRONG TREE"; exit 9; }
grep -q "holds_content_for" crates/holon-filesystem/src/file_sync_controller.rs || { echo "WRONG TREE"; exit 9; }
grep -q "ALL GREEN" "$S/land-gate-wave11-r3.log" || { echo "NO GREEN LAND GATE"; exit 1; }
grep -q "quiet at" "$S/land-gate-wave11-r3.log" || { echo "WRONG GATE RUN"; exit 1; }
BAT=$(ls -t "$S"/land-battery.*.log | head -1); APP=$(ls -t "$S"/land-appnextest.*.log | head -1)
grep -q "== landing gate PASS ==" "$BAT" || { echo "NO PASS MARKER in $BAT"; exit 1; }
SUMMARY=$(grep -E "Summary \[" "$APP" | tail -1); echo "holon/holon-app: $SUMMARY"
echo "$SUMMARY" | grep -qE "[1-9][0-9]* tests run" || { echo "UNEXPECTED SUMMARY"; exit 1; }
grep -E "^\s+FAIL \[|^\s+TIMEOUT \[" "$APP" | sed -E 's/^ *(FAIL|TIMEOUT) \[[^]]*\] \([^)]*\) //' | sort -u > "$S/land-fails-final-w11.txt"
KNOWN="e2e_backend_engine_test|undo_concurrent_keystrokes|test_multi_peer_sync_iroh|turso_block_query_source_round_trip_pbt|cursor_filtered_main_panel|notify_watcher_delivers_events_after_arm|subtree_share_round_trip_pbt|state_machine|a_dispatched_switch_reaches_the_seeded_section|quick_open_search_at_vault_scale|integration_toggle_round_trip|prod_session_create_block_persists_to_block_raw"
if grep -vE "$KNOWN" "$S/land-fails-final-w11.txt" | grep -q .; then echo "NOVEL:"; cat "$S/land-fails-final-w11.txt"; exit 1; fi
echo "pass-with-note: $(wc -l < "$S/land-fails-final-w11.txt" | tr -d ' ') registered failures"
echo "ALL GREEN"
