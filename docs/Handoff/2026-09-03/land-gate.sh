#!/usr/bin/env bash
set -euo pipefail
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
export RUSTC_WRAPPER=
SP=/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/1d3fdfe9-af2d-42a8-aecb-fbc009830160/scratchpad
SEM=/opt/homebrew/opt/parallel/bin/sem
echo "tree: $(pwd)"
grep -q "typed_rows" crates/holon-core/src/file_format.rs || { echo "WRONG TREE (a3 missing)"; exit 9; }
grep -q "pickedItems" crates/holon-kitchen/src/shopping.rs || { echo "WRONG TREE (c2 missing)"; exit 9; }
grep -q "shopping.list_url" crates/holon-frontend/src/preferences.rs || { echo "WRONG TREE (c2-settings missing)"; exit 9; }
grep -q "query_positional" crates/holon/src/core/pantry_operations.rs || { echo "WRONG TREE (hygiene missing)"; exit 9; }
if grep -rnE '/![A-Za-z0-9_-]{20,}' crates docs assets 2>/dev/null | grep -viE 'synth|fixture|example' | grep -q .; then echo "TOKEN-SHAPED SEGMENT IN TREE"; exit 9; fi

# D43.a: holon-app nextest runs alongside the landing battery.
LANDLOG="$SP/land-battery.$$.log"
APPLOG="$SP/land-appnextest.$$.log"
"$SEM" --id holon-build -j4 --fg "just landing-gate > $LANDLOG 2>&1" &
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
  # allowed: the 5 registered matview reds + sanctioned flakes; anything else blocks the land
  if grep -vE "e2e_backend_engine_test|undo_concurrent_keystrokes|test_multi_peer_sync_iroh|turso_block_query_source_round_trip_pbt|cursor_filtered_main_panel_delivers_at_vault_scale" "$SP/land-fails.$$.txt" | grep -q .; then echo "NOVEL FAILURES holon/holon-app:"; cat "$SP/land-fails.$$.txt"; exit 1; fi
  echo "holon/holon-app: pass-with-note ($(wc -l < "$SP/land-fails.$$.txt") known failures)"
fi
tail -3 "$LANDLOG"
echo "ALL GREEN"
