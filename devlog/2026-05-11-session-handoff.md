# Handoff — Phase 3.7 follow-ups landed; PBT residual remains

Date: 2026-05-11

## Handoff prompt (paste this into the new session)

> Pick up the Phase 3.7 "gate flip" roadmap. The previous session
> landed items 5 + 7 (typed `Event::routing_doc_uri` + archlint
> `routing_payload_key` smell) and item 1a (phantom-Loro startup-seed
> race — fixed via LEFT JOIN events filter in
> `seed_loro_from_persistent_store`). Item 1b (PBT mid-test phantom
> path) is the only remaining blocker before the gate flip.
>
> **Concrete next step**: run the full PBT with the instrumentation
> from the previous session still in place, and grep for
> `[PHANTOM-LORO-TRACE] apply_create BYPASS` lines that fire for
> `bulk-*` blocks. Two paths to choose from:
>
>   a. **Trust the focused reproducer is exhaustive**:
>      `crates/holon-integration-tests/tests/phantom_loro_exists_repro.rs`'s
>      mid-test variant `two_consecutive_bulk_batches_under_one_parent`
>      shows zero BYPASS post-fix. If the focused harness covers the
>      PBT's bulk-add path 1:1, item 1b is already closed — proceed
>      directly to the gate flip (item 2) by uncommenting
>      `handle.disable_inbound_runtime();` in
>      `crates/holon/src/sync/loro_module.rs:245` and removing
>      `let _hold_gate_open = &handle;`. Then run the full PBT.
>
>   b. **Verify against the actual PBT first** (safer):
>      ```
>      RUST_LOG=error PROPTEST_CASES=1 cargo test \
>        -p holon-integration-tests --test general_e2e_pbt \
>        general_e2e_pbt -- --nocapture 2>&1 | tee /tmp/pbt.log
>      grep "apply_create BYPASS" /tmp/pbt.log | grep -v "block:journals\|block:default-\|block:root-layout"
>      ```
>      Any BYPASS line for a non-layout block identifies a residual
>      writer. Static-analysis candidates from
>      `devlog/2026-05-11-phantom-loro-exists-investigation.md`
>      (already ruled out for the startup case) that could still be
>      live mid-test: `peer_create_block` + `sync_docs_direct`
>      transitions in the PBT generator, atomic-editor primitives
>      writing to Loro indirectly, or any LoroBlockOperations::create
>      caller (which has an `error!` regression-guard trace at the top).
>
> Recommend path (b) — one full PBT run is cheap insurance, and the
> traces are still in place to surface anything the focused
> reproducer missed. After it's clean, flip the gate and rerun.
>
> All other context: read this file, then the four detailed devlogs
> dated 2026-05-11 (typed-routing-doc-uri, phantom-loro-*).

## State the new session is starting from

### Working copy at branch tip

- Branch: detached
- Current commit: `lukmyvtq` (no description) — contains archlint
  rule, phantom-Loro fix + traces, focused test, devlogs.
- Parent: `qxlklrku` (no description) — contains typed
  routing_doc_uri (item 5) + earlier Phase 3.7 work.
- Both commits are uncommitted-style WIP — squash before any PR.

### Roadmap

| # | Item | Status |
|---|---|---|
| 1a | Phantom-Loro startup-seed race | **LANDED** (this session) |
| 1b | Phantom-Loro PBT mid-test variant | **OPEN** — needs PBT verification; may already be closed by 1a |
| 2 | Flip the inbound runtime gate | Ready to attempt after (1b) verifies clean |
| 3 | Gate integration test (deferred from Phase 3.3 step 2) | After (2) |
| 4 | Retire `sort_key` write path entirely (Stage 2-plus) | After (2) |
| 5 | Typed `_routing_doc_uri` | **LANDED** (this session) |
| 6 | Chord-op direct positioning via cell registry | After (4) |
| 7 | archlint rule for new `_routing_*` payload keys | **LANDED** (this session) |

### Verification baseline (all green at handoff)

```
cargo check --workspace --tests                                                    GREEN
cargo test -p holon-core --lib block_operations_tests                              19/19
cargo test -p holon --lib sync::loro_sync_controller                               16/16
cargo test -p holon --lib sync::block_cell_registry                                 5/5
cargo test -p holon --lib sync::turso_event_bus                                     3/3
cargo test -p holon-integration-tests --features otel-testing
    --test phantom_loro_exists_repro                                                2/2
```

Pre-existing failures unrelated to this work (verified on parent commit too):
`api::backend_engine::tests::test_execute_operation`,
`api::loro_backend_pbt::stateful_tests::test_loro_backend_state_machine`,
`api::sync_pbt::tests::share_subtree_pbt::subtree_share_round_trip_pbt`,
`holon-orgmode --lib file_watcher::tests::test_file_watcher_respects_gitignore`,
`holon-orgmode --features di sync_controller_mutation_pbt::test_sync_block_change_to_file`
+ `test_sync_file_change_to_blocks`.

### Files changed in this session's working commit (`lukmyvtq`)

Production code:

- `crates/holon/src/sync/loro_module.rs` — seed query gains LEFT JOIN
  filter + stable secondary sort; `apply_seed_row` trace at `debug!`.
- `crates/holon/src/api/loro_backend.rs` —
  `find_tree_id_by_stable_id` matched-node trace at `debug!`.
- `crates/holon/src/sync/loro_document_store.rs` — snapshot-exists
  trace at `debug!`; bare-`_` cleanups + `ALLOW(compatibility)` for
  legacy aliases (pre-existing archlint violations touched by the
  edit-hook chain).
- `crates/holon/src/sync/loro_block_operations.rs` — `error!` trace
  at `LoroBlockOperations::create` (regression guard, should never
  fire); bare-`_` cleanups on internal helpers.
- `crates/holon/src/sync/loro_sync_controller.rs` — `error!` trace
  at `apply_create` BYPASS branch (regression guard).

archlint rule:

- `archlint/smells/words.toml` — new `routing_payload_key` smell.

Test:

- `crates/holon-integration-tests/tests/phantom_loro_exists_repro.rs`
  — focused integration test (2 cases).

Devlogs:

- `devlog/2026-05-11-typed-routing-doc-uri.md` — items 5 + 7 landed
  (created this session; modified to update the roadmap table).
- `devlog/2026-05-11-phantom-loro-exists-investigation.md` — static
  analysis handoff with directed 5-step plan.
- `devlog/2026-05-11-phantom-loro-root-cause-found.md` — concrete
  trace data + three viable fix shapes.
- `devlog/2026-05-11-phantom-loro-startup-seed-fix.md` — landed-fix
  summary + verification + roadmap status.
- `devlog/2026-05-11-session-handoff.md` — this file.

Memory (cross-session):

- `~/.claude/projects/.../memory/phase3_7_followups_landed.md` — new.
- `~/.claude/projects/.../memory/phantom_loro_startup_seed_race.md` — new.
- `~/.claude/projects/.../memory/MEMORY.md` — index updated.

## Why path (b) is recommended over (a)

The previous session's mid-test focused reproducer (5-block bulk
under existing parent, then 5-block extension batch) cleanly passes
post-fix with zero BYPASS lines. That's encouraging but not
conclusive — the PBT generator emits sequences that combine
`BulkExternalAdd` with `PeerEdit` / `SyncWithPeer` / atomic-editor
primitives, and the static-analysis handoff already identified at
least three candidate writers that the focused reproducer doesn't
exercise:

1. `peer_create_block` (`crates/holon-integration-tests/src/pbt/peer_ops.rs:49`)
   — called from `apply_peer_edit` in `sut.rs:2461`. Writes directly
   to a peer's `LoroDoc` bypassing the EventBus.
2. `sync_docs_direct` — called from `apply_sync_with_peer` in
   `sut.rs:2493`. Imports peer's updates into the primary doc.
3. Atomic-editor primitives (`TypeChars`, `PressKey`,
   `FocusEditableText`, `DeleteBackward`) routed through
   `BlockCellRegistry::write_field` — these write to LoroText, not
   tree nodes, so probably not it, but worth scanning if the BYPASS
   trace surfaces a content-mutation event.

A single PBT run with traces tells us conclusively. ~25 minutes
is cheap compared to a wrong "ship it" call on the gate flip.

## Quick-start commands for the new session

```bash
# Sanity check that nothing rotted since handoff
cargo check --workspace --tests

# Path (b) — verify item 1b is actually closed:
RUST_LOG=error PROPTEST_CASES=1 cargo test \
  -p holon-integration-tests --test general_e2e_pbt \
  general_e2e_pbt -- --nocapture 2>&1 | tee /tmp/pbt.log
grep "apply_create BYPASS" /tmp/pbt.log \
  | grep -v "block:journals\|block:default-\|block:root-layout"
# If empty: item 1b closed. Proceed to gate flip.
# If non-empty: each line's `block <id>` identifies the residual writer.
#               Add a write-side error! trace at the suspected caller
#               and re-run.

# Path (a) — go straight to the gate flip:
# Edit crates/holon/src/sync/loro_module.rs:243-245 — uncomment
# `handle.disable_inbound_runtime();` and remove
# `let _hold_gate_open = &handle;`. Then full PBT.

# After gate flip succeeds, add the gate integration test
# (item 3) and start the sort_key retirement (item 4).
```

## Investigation traces left in place

These produce stderr output under `RUST_LOG=error`. Production
deployments don't see them at default log levels:

- `error!` (regression guards, should never fire):
  - `crates/holon/src/sync/loro_block_operations.rs::create`
  - `crates/holon/src/sync/loro_sync_controller.rs::apply_create` (BYPASS branch)
- `debug!` (informational, fires on hot paths):
  - `crates/holon/src/sync/loro_document_store.rs::get_global_doc` (snapshot exists check)
  - `crates/holon/src/api/loro_backend.rs::find_tree_id_by_stable_id` (matched node)
  - `crates/holon/src/sync/loro_module.rs::apply_seed_row` (create_block call site)

Remove all five in a cleanup PR after item 1b is conclusively closed.
The cost of leaving them in tree until then is negligible; the cost
of needing them and not having them is another 25-minute reproduction
cycle plus the static-analysis pass to re-locate the right insertion
points.
