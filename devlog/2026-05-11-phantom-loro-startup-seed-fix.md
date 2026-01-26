# Phantom-Loro-exists startup-seed race — FIXED

Date: 2026-05-11

## TL;DR

Closing-pass on the Phase 3.7 follow-up
(`devlog/2026-05-11-phantom-loro-root-cause-found.md`). The startup-time
race where `seed_loro_from_persistent_store` and the OrgFileWatcher's
initial-scan-driven CDC stream both create the same blocks in Loro is
now gone. Fix: make the seed query skip rows that still have unprocessed
`block.created` events on the `loro` consumer. Those blocks will arrive
via the regular CDC inbound path and `apply_create` creates them
through the typed-positional pipeline — no bypass, no scrambled
children order.

The remaining `[PHANTOM-LORO-TRACE]` traces are demoted from `error!`
to `debug!` where they fire on hot paths, but kept in place at
`error!` on the two write sites that should NEVER fire post-fix
(`LoroBlockOperations::create`, `apply_create BYPASS`). They serve as
regression guards.

## The fix

`crates/holon/src/sync/loro_module.rs::seed_loro_from_persistent_store`
query change:

```diff
-let rows = db_handle.query(
-    "SELECT id, parent_id, content, content_type, source_language, properties
-     FROM {table} ORDER BY created_at ASC", …)
+let rows = db_handle.query(
+    "SELECT b.id, b.parent_id, b.content, b.content_type, b.source_language, b.properties
+     FROM {table} b
+     LEFT JOIN events e ON e.aggregate_id = b.id
+                       AND e.aggregate_type = 'block'
+                       AND e.event_type = 'block.created'
+                       AND e.processed_by_loro = 0
+     WHERE e.id IS NULL
+     ORDER BY b.created_at ASC, b.id ASC", …)
```

The LEFT JOIN + `WHERE e.id IS NULL` keeps a row in the seed result
set **iff** no unprocessed `block.created` event exists for it.
Concretely:

- **Blocks written via `OperationProvider`** (OrgSync's initial scan,
  any chord op, any future inbound source) all produce a
  corresponding `events` row. As long as the loro consumer hasn't
  processed it yet (`processed_by_loro = 0`), the seed skips the
  block. CDC delivers the event → `apply_create` creates the block
  in Loro with correct positioning.

- **Blocks written via raw INSERTs** (notably `seed_default_layout`'s
  layout panel + sidebars, which write SQL directly to bypass the
  event bus) produce no events. The LEFT JOIN keeps them. The seed
  creates them in Loro. This is the legitimate seed path.

- **Blocks whose `block.created` event has already been processed**
  by the loro consumer (`processed_by_loro = 1`) — same handling as
  "no event" because `processed_by_loro = 0` is the JOIN predicate.
  The seed re-creates them. This is harmless if the block is still
  present in Loro (`apply_seed_row` returns `Ok(false)` due to the
  existing-tree-id check), and a useful recovery if Loro's snapshot
  was reset.

Secondary tiebreak `, b.id ASC` added so the seed result set is
deterministic across runs, easing reproduction of any residual
issues.

## Verification

`crates/holon-integration-tests/tests/phantom_loro_exists_repro.rs`:

| Test | Before fix | After fix |
|---|---|---|
| `bulk_add_five_siblings_under_one_parent_at_startup` | 5 seed creates for `bulk-0-*`; 5 apply_create BYPASS; tree counters scrambled (13, 41, 55, 69, 27) | 0 seed creates for `bulk-0-*`; 0 apply_create BYPASS; tree counters sequential |
| `two_consecutive_bulk_batches_under_one_parent` | pass (mid-test never had the bug) | pass |

`RUST_LOG=error cargo test … phantom_loro_exists_repro --` shows zero
`[PHANTOM-LORO-TRACE] apply_create BYPASS` lines after the fix, vs.
12+ before for the same test.

Targeted suites verified post-fix:

| Suite | Result |
|---|---|
| `cargo check --workspace --tests` | GREEN |
| `cargo test -p holon-core --lib block_operations_tests` | 19/19 |
| `cargo test -p holon --lib sync::loro_sync_controller` | 16/16 |
| `cargo test -p holon --lib sync::block_cell_registry` | 5/5 |
| `cargo test -p holon --lib sync::turso_event_bus` | 3/3 |

## What the mid-test path does NOT do

The `two_consecutive_bulk_batches_under_one_parent` test writes the
org file mid-test (after `start_app` completes, so the seed has long
run with empty SQL). The trace output for this test shows:

- `apply_seed_row` fires only for `seed_default_layout` blocks at
  startup. No `bulk-*` blocks.
- No `apply_create BYPASS` for any `bulk-*` block. All go through the
  create path with correct positioning.
- `find_tree_id_by_stable_id` fires for `bulk-A`/`B`/`C` during
  readback queries — these are normal cache lookups, not bypasses.

**The Phase 3.7 open follow-up describes a mid-test failure mode**
("first block flows through `apply_create` normally; remaining four
take the early-bypass") that this focused test does NOT reproduce.
The PBT-side failure must require additional PBT-specific
conditions:

- `PeerEditOp::Create` + `SyncWithPeer` transitions that write to
  Loro directly via `peer_create_block` + `sync_docs_direct` (these
  bypass apply_create entirely).
- Atomic-editor primitive interactions (`FocusEditableText`,
  `TypeChars`, `PressKey`) firing between bulk transitions.
- Accumulated state from many transitions before
  `BulkExternalAdd`.

The startup race was the dominant noise source. With it resolved,
the PBT-side mid-test issue (if it still exists) should be much
easier to isolate — the next session can re-enable the full PBT
replay (`PROPTEST_CASES=1 cargo test … general_e2e_pbt`) and trust
that any `apply_create BYPASS` trace lines that appear identify the
remaining writer paths.

## Roadmap status

| # | Item | Status |
|---|---|---|
| 1a | Resolve phantom-Loro-exists flake — startup | **LANDED** |
| 1b | Resolve phantom-Loro-exists flake — PBT mid-test | OPEN; reproducible only via full PBT |
| 2 | Flip the gate | blocked on (1b) verification — likely unblocks once PBT runs clean |
| 3 | Gate integration test | blocked on (2) |
| 4 | Retire sort_key write path entirely | blocked on (2) |
| 5 | Typed `_routing_doc_uri` | LANDED |
| 6 | Chord-op direct positioning via cell registry | blocked on (4) |
| 7 | archlint rule for new `_routing_*` payload keys | LANDED |

If a Full PBT run post-fix shows zero `[PHANTOM-LORO-TRACE] apply_create BYPASS`
lines for `bulk-*` blocks, item (1b) collapses to (1a). The user
should run the PBT next.

## Files modified

- `crates/holon/src/sync/loro_module.rs` — seed query gains LEFT JOIN
  filter and stable secondary sort; `apply_seed_row` trace demoted to
  `debug!`.
- `crates/holon/src/api/loro_backend.rs` — `find_tree_id_by_stable_id`
  matched-node trace demoted to `debug!`.
- `crates/holon/src/sync/loro_document_store.rs` — snapshot-exists
  trace demoted to `debug!`.

Traces kept at `error!` (regression guards):

- `crates/holon/src/sync/loro_block_operations.rs::create` —
  documented-dormant path; should never fire.
- `crates/holon/src/sync/loro_sync_controller.rs::apply_create` —
  the BYPASS branch; firing in production indicates the seed-CDC
  barrier failed.

The focused test
`crates/holon-integration-tests/tests/phantom_loro_exists_repro.rs`
stays in tree as a regression gate — it's a ~1-second check that
runs alongside the existing `bidirectional_sync` tests.
