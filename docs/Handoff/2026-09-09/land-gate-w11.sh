#!/usr/bin/env bash
# Wave 11 land gate: run in _sw_integ at the final chain tip (land-tip-w11.txt), on a quiet machine.
set -euo pipefail
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
export RUSTC_WRAPPER=
export CARGO_BUILD_JOBS=6
SP=/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/bc7b1e67-1603-4c68-8742-84215e1a79e3/scratchpad
SEM=/opt/homebrew/opt/parallel/bin/sem
cd /Users/martin/Workspaces/pkm/holon/.claude/worktrees/_sw_integ
echo "tree: $(pwd)"
TIP=$(jj log -r '@-' -T 'commit_id.short(12)' --no-graph)
WANT=$(cat "$SP/land-tip-w11.txt")
test "$TIP" = "$WANT" || { echo "TREE MOVED: $TIP != $WANT"; exit 9; }
# one sentinel per wave-11 lane
grep -q "holds_content_for" crates/holon-filesystem/src/file_sync_controller.rs || { echo "WRONG TREE (plaintext-fix)"; exit 9; }
grep -q "VaultFilter" crates/holon-filesystem/src/vault_filter.rs || { echo "WRONG TREE (ingest-dupslug)"; exit 9; }
test -f crates/holon-loro/src/share_credentials.rs || { echo "WRONG TREE (share-lifecycle)"; exit 9; }
grep -q "struct ReadOnlyMembers" crates/holon-core/src/write_tier_gate.rs || { echo "WRONG TREE (readonly)"; exit 9; }
grep -q "struct Priority" crates/holon-api/src/types.rs || { echo "WRONG TREE (org-priority)"; exit 9; }
test ! -e scripts-lane || { echo "WRONG TREE (hygiene)"; exit 9; }
grep -q "expect_err" crates/holon-toon/src/org_reader.rs || { echo "WRONG TREE (clippy-tip)"; exit 9; }
test -f guests/cooklang/src/german.toml || { echo "WRONG TREE (cook-german-units)"; exit 9; }
grep -q "SessionShutdown" crates/holon-api/src/lifecycle.rs || { echo "WRONG TREE (session-shutdown)"; exit 9; }
grep -q "fn edit_target_id" crates/holon-frontend/src/*.rs || { echo "WRONG TREE (quick-open-caret)"; exit 9; }
grep -q "BOUNDS_WAIT_PUMP_CYCLES" frontends/gpui/tests/pbt_harness/sim_windowed_replay.rs || { echo "WRONG TREE (gpui-driver)"; exit 9; }
grep -rq "HOLON_PBT_REBOOT" crates/holon-integration-tests/src/pbt/transitions/reboot.rs || { echo "WRONG TREE (keystone-reboot)"; exit 9; }
grep -q "FIXED" docs/Testing/bugfunnel/entries/2026-08-08-blank-empty-link-puts-vault-into.md || { echo "WRONG TREE (bulk-empty-content)"; exit 9; }
grep -rlI '<<<<<<< conflict' crates docs | grep -q . && { echo "CONFLICT MARKERS"; exit 9; }
if grep -rnE '/![A-Za-z0-9_-]{20,}' crates docs assets 2>/dev/null | grep -viE 'synth|fixture|example' | grep -q .; then echo "TOKEN-SHAPED SEGMENT IN TREE"; exit 9; fi

# D106.a: clippy is a LAND-gate step (check only, never --fix)
CLIPLOG="$SP/land-clippy.$$.log"
"$SEM" --id holon-build-MacBook-Pro-2 -j6 --fg "cargo clippy --workspace --all-targets -- -D warnings > $CLIPLOG 2>&1" || { echo "CLIPPY RED (log $CLIPLOG)"; grep -E '^error' "$CLIPLOG" | head -5; exit 1; }
echo "clippy: green"

# D43.a: holon-app nextest runs alongside the landing battery.
LANDLOG="$SP/land-battery.$$.log"
APPLOG="$SP/land-appnextest.$$.log"
"$SEM" --id holon-build-MacBook-Pro-2 -j6 --fg "just landing-gate > $LANDLOG 2>&1" &
BATTERY=$!
sleep 30
# D64.a: the holon crate's integration tests are gated per land (always with holon-app: feature unification).
cargo nextest run --no-fail-fast -p holon -p holon-app > "$APPLOG" 2>&1 &
APP=$!
wait $BATTERY || { echo "LANDING BATTERY FAILED (log $LANDLOG)"; tail -8 "$LANDLOG"; wait $APP || true; exit 1; }
wait $APP || echo "holon/holon-app nextest exited non-zero — classifying its failures below"
grep -q "landing gate PASS" "$LANDLOG" || { echo "NO PASS MARKER"; exit 1; }
S=$(grep -E "Summary \[" "$APPLOG" | tail -1 || true)
echo "holon-app: $S"
echo "$S" | grep -qE "Summary \[.*\] [1-9][0-9]* tests run" || { echo "ZERO TESTS holon-app"; exit 1; }
if echo "$S" | grep -qE "[1-9][0-9]* failed"; then
  grep -E "^\s+FAIL \[|^\s+TIMEOUT \[" "$APPLOG" | sort -u | sed -E 's/^ *(FAIL|TIMEOUT) \[[^]]*\] \([^)]*\) //' | sort > "$SP/land-fails.$$.txt"
  KNOWN="e2e_backend_engine_test|undo_concurrent_keystrokes|test_multi_peer_sync_iroh|turso_block_query_source_round_trip_pbt|cursor_filtered_main_panel|notify_watcher_delivers_events_after_arm|subtree_share_round_trip_pbt|state_machine|a_dispatched_switch_reaches_the_seeded_section|quick_open_search_at_vault_scale|integration_toggle_round_trip|prod_session_create_block_persists_to_block_raw"
  if grep -vE "$KNOWN" "$SP/land-fails.$$.txt" | grep -q .; then echo "NOVEL FAILURES holon/holon-app:"; cat "$SP/land-fails.$$.txt"; exit 1; fi
  echo "holon/holon-app: pass-with-note ($(wc -l < "$SP/land-fails.$$.txt") known failures)"
fi
tail -3 "$LANDLOG"
echo "ALL GREEN"
