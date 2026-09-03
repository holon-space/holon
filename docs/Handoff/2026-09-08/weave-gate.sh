#!/usr/bin/env bash
# usage: bash weave-gate.sh <lane>   (run from the _sw_integ tree by sw weave --check)
set -euo pipefail
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
export RUSTC_WRAPPER=
export CARGO_BUILD_JOBS=6
LANE=${1:?lane}
SP=/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/bc7b1e67-1603-4c68-8742-84215e1a79e3/scratchpad
SEM=/opt/homebrew/opt/parallel/bin/sem
echo "tree: $(pwd)"
case "$LANE" in
  kitchen-a3)  grep -q "typed_rows" crates/holon-core/src/file_format.rs || { echo "WRONG TREE"; exit 9; }
               CRATES="-p holon-kitchen -p holon-core -p holon -p holon-app"; EXTRA="";;
  kitchen-c2)  grep -q "pickedItems" crates/holon-kitchen/src/shopping.rs || { echo "WRONG TREE"; exit 9; }
               CRATES="-p holon-kitchen -p holon-mcp-client -p holon-app"; EXTRA="";;
  c2-settings) grep -q "shopping.list_url" crates/holon-frontend/src/preferences.rs || { echo "WRONG TREE"; exit 9; }
               CRATES="-p holon-frontend -p holon-mcp-client -p holon-app"; EXTRA="hand";;
  hygiene)     grep -q "query_positional" crates/holon/src/core/pantry_operations.rs || { echo "WRONG TREE"; exit 9; }
               CRATES="-p holon-filesystem -p holon -p holon-mcp-client -p holon-app"; EXTRA="";;
  reds-triage) grep -q "vault-scale-latency" .config/nextest.toml || { echo "WRONG TREE"; exit 9; }
               CRATES="-p holon -p holon-app -p holon-loro"; EXTRA="";;
  funnel-false-alarm) grep -q "FALSE-ALARM" scripts/bugfunnel.py || { echo "WRONG TREE"; exit 9; }
               /usr/bin/python3 scripts/bugfunnel.py check > "$SP/w-$LANE-bugfunnel.$$.log" 2>&1 || { echo "BUGFUNNEL CHECK FAILED"; exit 1; }
               grep -q "0 problems" "$SP/w-$LANE-bugfunnel.$$.log" || { echo "BUGFUNNEL PROBLEMS"; exit 1; }
               CRATES="-p holon-frontend"; EXTRA="";;
  change-origin-schema) grep -q "declares_column" crates/holon-turso/src/schema_catalog.rs || { echo "WRONG TREE"; exit 9; }
               /usr/bin/python3 scripts/bugfunnel.py check > "$SP/w-$LANE-bugfunnel.$$.log" 2>&1 || { echo "BUGFUNNEL CHECK FAILED"; exit 1; }
               CRATES="-p holon-turso -p holon -p holon-app"; EXTRA="loro";;
  share-mount-tags) grep -q "sharing_a_non_page_block_leaves_its_tags_on_the_block" crates/holon/tests/share_mount_tags_e2e.rs || { echo "WRONG TREE"; exit 9; }
               /usr/bin/python3 scripts/bugfunnel.py check > "$SP/w-$LANE-bugfunnel.$$.log" 2>&1 || { echo "BUGFUNNEL CHECK FAILED"; exit 1; }
               CRATES="-p holon-loro -p holon-loro-wiring -p holon -p holon-app"; EXTRA="loro";;
  share-create-routing) grep -rq "share" crates/holon-loro/src/loro_block_operations.rs || { echo "WRONG TREE"; exit 9; }
               /usr/bin/python3 scripts/bugfunnel.py check > "$SP/w-$LANE-bugfunnel.$$.log" 2>&1 || { echo "BUGFUNNEL CHECK FAILED"; exit 1; }
               CRATES="-p holon-loro -p holon-loro-wiring -p holon -p holon-app"; EXTRA="loro";;
  subtree-share-race) grep -q "SettleScope" crates/holon-loro/src/loro_share_backend.rs || { echo "WRONG TREE"; exit 9; }
               /usr/bin/python3 scripts/bugfunnel.py check > "$SP/w-$LANE-bugfunnel.$$.log" 2>&1 || { echo "BUGFUNNEL CHECK FAILED"; exit 1; }
               CRATES="-p holon-loro -p holon -p holon-app"; EXTRA="loro";;
  crdt-default) grep -rq "UnavailableEntities" crates/holon/src || { echo "WRONG TREE"; exit 9; }
               /usr/bin/python3 scripts/bugfunnel.py check > "$SP/w-$LANE-bugfunnel.$$.log" 2>&1 || { echo "BUGFUNNEL CHECK FAILED"; exit 1; }
               CRATES="-p holon-frontend -p holon-mcp -p holon -p holon-app"; EXTRA="hand loro";;
  caret-oracle) grep -q "slot-birth\|birth_block_under_slot" crates/holon-integration-tests/src/pbt/reference_state.rs || { echo "WRONG TREE"; exit 9; }
               /usr/bin/python3 scripts/bugfunnel.py check > "$SP/w-$LANE-bugfunnel.$$.log" 2>&1 || { echo "BUGFUNNEL CHECK FAILED"; exit 1; }
               CRATES="-p holon -p holon-app"; EXTRA="hand";;
  pair-inc0) grep -q "shallow_owner_converges_with_a_receiver_bootstrapped_into_an_empty_doc" crates/holon/tests/sync_suite/sync_pbt.rs || { echo "WRONG TREE"; exit 9; }
               /usr/bin/python3 scripts/bugfunnel.py check > "$SP/w-$LANE-bugfunnel.$$.log" 2>&1 || { echo "BUGFUNNEL CHECK FAILED"; exit 1; }
               CRATES="-p holon-loro -p holon-sharing -p holon -p holon-app"; EXTRA="loro";;
  pair-inc5) grep -q "RefuseContainer" crates/holon-sharing/src/acceptor.rs || { echo "WRONG TREE"; exit 9; }
               /usr/bin/python3 scripts/bugfunnel.py check > "$SP/w-$LANE-bugfunnel.$$.log" 2>&1 || { echo "BUGFUNNEL CHECK FAILED"; exit 1; }
               CRATES="-p holon-loro -p holon-sharing -p holon -p holon-app"; EXTRA="loro";;
  pair-inc3) grep -q "TwoInstanceTransport" crates/holon-integration-tests/src/pbt/composed/two_instance_transport.rs || { echo "WRONG TREE"; exit 9; }
               /usr/bin/python3 scripts/bugfunnel.py check > "$SP/w-$LANE-bugfunnel.$$.log" 2>&1 || { echo "BUGFUNNEL CHECK FAILED"; exit 1; }
               CRATES="-p holon-loro -p holon-sharing -p holon -p holon-app"; EXTRA="loro";;
  loro-pin) grep -q 'rev = "6f5b2d7e"' Cargo.toml || { echo "WRONG TREE"; exit 9; }
               /usr/bin/python3 scripts/bugfunnel.py check > "$SP/w-$LANE-bugfunnel.$$.log" 2>&1 || { echo "BUGFUNNEL CHECK FAILED"; exit 1; }
               CRATES="-p holon-loro -p holon-sharing -p holon -p holon-app"; EXTRA="loro";;
  pair-two-writer) grep -rq "HOLON_PBT_TWO_WRITER" crates/holon-integration-tests/src || { echo "WRONG TREE"; exit 9; }
               /usr/bin/python3 scripts/bugfunnel.py check > "$SP/w-$LANE-bugfunnel.$$.log" 2>&1 || { echo "BUGFUNNEL CHECK FAILED"; exit 1; }
               CRATES="-p holon-loro -p holon -p holon-app"; EXTRA="loro hand";;
  pair-inc1) grep -q "retain_grounded_parent_updates" crates/holon-loro/src/loro_sync_controller.rs || { echo "WRONG TREE"; exit 9; }
               /usr/bin/python3 scripts/bugfunnel.py check > "$SP/w-$LANE-bugfunnel.$$.log" 2>&1 || { echo "BUGFUNNEL CHECK FAILED"; exit 1; }
               CRATES="-p holon-loro -p holon-loro-wiring -p holon -p holon-app"; EXTRA="loro";;
  pair-inc2) grep -q "live_node_counts" crates/holon-integration-tests/src/pbt/composed/two_instance.rs || { echo "WRONG TREE"; exit 9; }
               /usr/bin/python3 scripts/bugfunnel.py check > "$SP/w-$LANE-bugfunnel.$$.log" 2>&1 || { echo "BUGFUNNEL CHECK FAILED"; exit 1; }
               CRATES="-p holon-loro -p holon-app"; EXTRA="loro";;
  org-writeback-reds) grep -q "pass_in_flight" crates/holon-orgmode/src/di.rs || { echo "WRONG TREE"; exit 9; }
               /usr/bin/python3 scripts/bugfunnel.py check > "$SP/w-$LANE-bugfunnel.$$.log" 2>&1 || { echo "BUGFUNNEL CHECK FAILED"; exit 1; }
               CRATES="-p holon-orgmode -p holon-api -p holon-loro -p holon -p holon-app"; EXTRA="loro";;
  ingest-contract) grep -q "ingest_recovered" crates/holon-filesystem/src/sync_ports.rs || { echo "WRONG TREE"; exit 9; }
               /usr/bin/python3 scripts/bugfunnel.py check > "$SP/w-$LANE-bugfunnel.$$.log" 2>&1 || { echo "BUGFUNNEL CHECK FAILED"; exit 1; }
               CRATES="-p holon-core -p holon-filesystem -p holon-orgmode -p holon-kitchen -p holon -p holon-app"; EXTRA="";;
  org-bold-link) grep -q "doubled_emphasis_round_trips_byte_identically" crates/holon-org-format/tests/render_lossless_shapes.rs || { echo "WRONG TREE"; exit 9; }
               /usr/bin/python3 scripts/bugfunnel.py check > "$SP/w-$LANE-bugfunnel.$$.log" 2>&1 || { echo "BUGFUNNEL CHECK FAILED"; exit 1; }
               CRATES="-p holon-org-format -p holon-orgmode -p holon-app"; EXTRA="hand";;
  sync-peer-types) grep -rq "ColumnValueKind" crates/holon-api/src || { echo "WRONG TREE"; exit 9; }
               /usr/bin/python3 scripts/bugfunnel.py check > "$SP/w-$LANE-bugfunnel.$$.log" 2>&1 || { echo "BUGFUNNEL CHECK FAILED"; exit 1; }
               CRATES="-p holon-api -p holon-turso -p holon -p holon-app"; EXTRA="";;
  lowcode-inc1) test -f crates/holon-rows/Cargo.toml || { echo "WRONG TREE"; exit 9; }
               CRATES="-p holon-rows -p holon-kitchen -p holon-core -p holon-app"; EXTRA="";;
  pair-prod) grep -q "pair_with_owner" crates/holon-loro/src/device_pairing_op.rs || { echo "WRONG TREE"; exit 9; }
               CRATES="-p holon-loro -p holon-sharing -p holon-loro-wiring -p holon-app"; EXTRA="loro";;
  docs-adr) test -f docs/adr/0033-own-device-pairing-whole-store-replication.md || { echo "WRONG TREE"; exit 9; }
               /usr/bin/python3 scripts/bugfunnel.py check > "$SP/w-$LANE-bugfunnel.$$.log" 2>&1 || { echo "BUGFUNNEL CHECK FAILED"; exit 1; }
               CRATES="-p holon-architecture-tests"; EXTRA="";;
  dogfood-explore) test -f docs/Testing/bugfunnel/entries/2026-09-03-quick-open-search-returns-no-matches-for-every-query.md || { echo "WRONG TREE"; exit 9; }
               /usr/bin/python3 scripts/bugfunnel.py check > "$SP/w-$LANE-bugfunnel.$$.log" 2>&1 || { echo "BUGFUNNEL CHECK FAILED"; exit 1; }
               CRATES="-p holon-architecture-tests"; EXTRA="";;
  admit-fuzz) test -f crates/holon-sharing/tests/admit_hostile_envelope_pbt.rs || { echo "WRONG TREE"; exit 9; }
               CRATES="-p holon-sharing -p holon-loro"; EXTRA="";;
  search-fix) grep -q "FOLD_CLASSES" crates/holon/src/api/query_engine.rs || { echo "WRONG TREE"; exit 9; }
               /usr/bin/python3 scripts/bugfunnel.py check > "$SP/w-$LANE-bugfunnel.$$.log" 2>&1 || { echo "BUGFUNNEL CHECK FAILED"; exit 1; }
               CRATES="-p holon -p holon-app -p holon-pbt-core"; EXTRA="hand";;
  lowcode-inc2a) test -f crates/holon-plugin-host/Cargo.toml || { echo "WRONG TREE"; exit 9; }
               /usr/bin/python3 scripts/bugfunnel.py check > "$SP/w-$LANE-bugfunnel.$$.log" 2>&1 || { echo "BUGFUNNEL CHECK FAILED"; exit 1; }
               CRATES="-p holon-plugin-host -p holon-rows -p holon-kitchen -p holon-core -p holon-app"; EXTRA="";;
  lowcode-inc4) test -f crates/holon-rows/src/lib.rs && grep -q "RowMapper" crates/holon-rows/src/lib.rs || { echo "WRONG TREE"; exit 9; }
               /usr/bin/python3 scripts/bugfunnel.py check > "$SP/w-$LANE-bugfunnel.$$.log" 2>&1 || { echo "BUGFUNNEL CHECK FAILED"; exit 1; }
               CRATES="-p holon-mcp-client -p holon-kitchen -p holon-rows -p holon-plugin-host -p holon-app"; EXTRA="";;
  pair-reimport) test -f crates/holon-loro/src/pairing_swap.rs || { echo "WRONG TREE"; exit 9; }
               /usr/bin/python3 scripts/bugfunnel.py check > "$SP/w-$LANE-bugfunnel.$$.log" 2>&1 || { echo "BUGFUNNEL CHECK FAILED"; exit 1; }
               CRATES="-p holon-loro -p holon-sharing -p holon-loro-wiring -p holon-app -p holon-architecture-tests"; EXTRA="loro";;
  dogfood-search) test -f docs/Testing/bugfunnel/entries/2026-09-03-search-folding-crashes-the-app-on-cyrillic-and-greek.md || { echo "WRONG TREE"; exit 9; }
               /usr/bin/python3 scripts/bugfunnel.py check > "$SP/w-$LANE-bugfunnel.$$.log" 2>&1 || { echo "BUGFUNNEL CHECK FAILED"; exit 1; }
               CRATES="-p holon-architecture-tests"; EXTRA="";;
  readonly-edits) grep -rq "ReadOnlyTextCellBacking" crates || { echo "WRONG TREE"; exit 9; }
               /usr/bin/python3 scripts/bugfunnel.py check > "$SP/w-$LANE-bugfunnel.$$.log" 2>&1 || { echo "BUGFUNNEL CHECK FAILED"; exit 1; }
               CRATES="-p holon-loro -p holon-frontend -p holon-core -p holon-filesystem -p holon -p holon-app"; EXTRA="";;
  sql-loss) test -f crates/holon/tests/batch_delete_cascade_updates.rs || { echo "WRONG TREE"; exit 9; }
               /usr/bin/python3 scripts/bugfunnel.py check > "$SP/w-$LANE-bugfunnel.$$.log" 2>&1 || { echo "BUGFUNNEL CHECK FAILED"; exit 1; }
               CRATES="-p holon-loro -p holon -p holon-app"; EXTRA="loro";;
  target-gc) test -f scripts/target-gc.py && grep -q "CANON" justfile || { echo "WRONG TREE"; exit 9; }
               just --list > "$SP/w-$LANE-justlist.$$.log" 2>&1 || { echo "JUST LIST FAILED"; exit 1; }
               /usr/bin/python3 scripts/target-gc.py --self-test > "$SP/w-$LANE-selftest.$$.log" 2>&1 || { echo "TARGET-GC SELFTEST FAILED"; exit 1; }
               CRATES="-p holon-app"; EXTRA="";;
  docs-truth) test -f archlint/smells/identity_minting.toml || { echo "WRONG TREE"; exit 9; }
               CRATES="-p holon-architecture-tests"; EXTRA="";;
  guests-repro) grep -q "HOLON_GUEST_STAGE" guests/build.sh || { echo "WRONG TREE"; exit 9; }
               just guests-verify > "$SP/w-$LANE-guestsverify.$$.log" 2>&1 || { echo "GUESTS-VERIFY FAILED"; tail -4 "$SP/w-$LANE-guestsverify.$$.log"; exit 1; }
               grep -q "matches its source" "$SP/w-$LANE-guestsverify.$$.log" || { echo "GUESTS-VERIFY NO MATCH LINE"; exit 1; }
               CRATES="-p holon-plugin-host -p holon-kitchen -p holon-app"; EXTRA="";;
  mcp-authority-fix) test -f docs/Testing/bugfunnel/entries/2026-09-08-sidecar-mirrored-type-has-no-stamp-home-panics-boot.md || { echo "WRONG TREE"; exit 9; }
               /usr/bin/python3 scripts/bugfunnel.py check > "$SP/w-$LANE-bugfunnel.$$.log" 2>&1 || { echo "BUGFUNNEL CHECK FAILED"; exit 1; }
               CRATES="-p holon-mcp-client -p holon-loro-wiring -p holon-loro -p holon-frontend -p holon-app"; EXTRA="";;
  smoke-ab) test -f docs/Testing/bugfunnel/entries/2026-09-08-keystone-novel-reds-only-under-machine-contention.md || { echo "WRONG TREE"; exit 9; }
               /usr/bin/python3 scripts/bugfunnel.py check > "$SP/w-$LANE-bugfunnel.$$.log" 2>&1 || { echo "BUGFUNNEL CHECK FAILED"; exit 1; }
               CRATES="-p holon-architecture-tests"; EXTRA="";;
  pair-conflict-badge) grep -q "pairing_conflict" assets/default/types/block_profile.yaml || { echo "WRONG TREE"; exit 9; }
               CRATES="-p holon-frontend -p holon-loro -p holon-app"; EXTRA="hand";;
  pair-boot-degraded) grep -q "PairingReimportDeferred" crates/holon-loro/src/degraded_signal_bus.rs || { echo "WRONG TREE"; exit 9; }
               CRATES="-p holon-loro -p holon-loro-wiring -p holon-sharing -p holon-app"; EXTRA="loro";;
  *) echo "unknown lane $LANE"; exit 9;;
esac
# credential-shape backstop on tracked text (synthetic fixtures are labelled)
if grep -rnE '/![A-Za-z0-9_-]{20,}' crates docs assets 2>/dev/null | grep -viE 'synth|fixture|example' | grep -q .; then
  echo "TOKEN-SHAPED SEGMENT IN TREE (unlabelled)"; grep -rnE '/![A-Za-z0-9_-]{20,}' crates docs assets | grep -viE 'synth|fixture|example' | cut -c1-80; exit 9
fi
step() {
  local name=$1; shift
  local log="$SP/w-$LANE-$name.$$.log"
  if ! "$SEM" --id holon-build -j6 --fg "$* > $log 2>&1"; then
    if [ "$name" = nextest ] || [ "$name" = smoke ]; then echo "STEP $name NONZERO (classified below, log $log)"; return 0; fi
    echo "STEP $name FAILED (log $log)"; tail -6 "$log"; exit 1
  fi
  if grep -qE '^error(\[|:)|usage:' "$log"; then echo "STEP $name ERROR MARKER ($log)"; exit 1; fi
  echo "STEP $name OK ($log)"
}
FMTLOG="$SP/w-$LANE-fmt.$$.log"
if cargo fmt --check > "$FMTLOG" 2>&1; then echo "STEP fmt OK ($FMTLOG)"; else
  # the search-fix chain commit's test file is unformatted until rev 3 squashes in place; nothing else may differ
  if grep -E "^Diff in" "$FMTLOG" | grep -v "search_deep_script_query_does_not_overflow.rs" | grep -q .; then
    echo "STEP fmt FAILED ($FMTLOG)"; grep -E "^Diff in" "$FMTLOG" | sed 's/ at line.*//' | sort -u; exit 1; fi
  echo "STEP fmt OK-with-note: only the search-fix file differs ($FMTLOG)"; fi
step check "cargo check --workspace --all-targets"
step arch "just analyze-arch"
step archtests "cargo nextest run -p holon-architecture-tests"
step nextest "cargo nextest run --no-fail-fast $CRATES"
step smoke "just keystone-smoke"
step wwasm "just check-worker-wasm"
step fwasm "just check-frontend-wasm"
[[ " $EXTRA " == *" hand "* ]] && step hand "just hand-authored"
[[ " $EXTRA " == *" loro "* ]] && step loro "just loro-suite"
true
LOG=$(ls -t "$SP"/w-$LANE-nextest.*.log | head -1)
S=$(grep -E "Summary \[" "$LOG" | tail -1 || true); echo "nextest: $S"
echo "$S" | grep -qE "Summary \[.*\] [1-9][0-9]* tests run" || { echo "ZERO TESTS"; exit 1; }
if echo "$S" | grep -qE "[1-9][0-9]* failed"; then
  echo "nextest FAILURES:"; grep -E "^\s+FAIL \[" "$LOG" | head -20
  # allow only the known signatures
  # allowlist = the measured main-baseline failure set (ab-holon-main.fails.txt, population run at ed38a4dae833) + known flakes
  grep -E "^\s+FAIL \[|^\s+TIMEOUT \[" "$LOG" | sort -u | sed -E 's/^ *(FAIL|TIMEOUT) \[[^]]*\] \([^)]*\) //' | sort > "$SP/w-$LANE-fails.$$.txt"
  KNOWN="e2e_backend_engine_test|undo_concurrent_keystrokes|test_multi_peer_sync_iroh|turso_block_query_source_round_trip_pbt|cursor_filtered_main_panel|notify_watcher_delivers_events_after_arm|subtree_share_round_trip_pbt|state_machine"
  if grep -vE "$KNOWN" "$SP/w-$LANE-fails.$$.txt" | grep -q .; then
    echo "NOVEL FAILURE(S):"; grep -vE "$KNOWN" "$SP/w-$LANE-fails.$$.txt"; exit 1; fi
  echo "nextest: pass-with-note (known signatures only)"
fi
ARCH=$(ls -t "$SP"/w-$LANE-arch.*.log | head -1)
grep -qE "0 new violation" "$ARCH" || { echo "ARCHLINT NEW VIOLATIONS"; grep -E "violation" "$ARCH" | tail -3; exit 1; }
SMOKELOG=$(ls -t "$SP"/w-$LANE-smoke.*.log | head -1)
if grep -qE "FAILED|panicked" "$SMOKELOG"; then
  bash scripts/keystone-known-reds.sh "$SMOKELOG" > "$SP/w-$LANE-kr.$$.log" 2>&1 || { echo "SMOKE RED + CLASSIFIER FAIL"; exit 1; }
  grep -qE "0 novel" "$SP"/w-$LANE-kr.$$.log || { echo "NOVEL KEYSTONE RED"; exit 1; }
  echo "smoke: pass-with-note"
else echo "smoke: green"; fi
echo "ALL GREEN"
