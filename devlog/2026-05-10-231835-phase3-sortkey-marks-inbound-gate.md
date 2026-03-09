# Phase 3.2 + 3.3-step-1 — sort_key + marks under Loro authority, inbound runtime gate added

Date: 2026-05-10

## What landed

Two deferred Phase-3 items from
`devlog/2026-05-09-175751-phase2-authority-flip-landed.md`:

1. **sort_key and marks are now first-class Loro-authored fields**
   (Phase 3.2). They leave the `BlockCellRegistry::write_field` skip list
   and route through Loro the same way `content` / `parent_id` / `tags`
   already did. The outbound projector emits SQL UPDATEs for sort_key /
   marks changes; the inbound consumer reflects non-Loro-origin SQL
   writes back into the right Loro encoding (top-level meta key for
   sort_key, Peritext for marks).

2. **`LoroSyncController` gained an opt-in gate for the inbound
   runtime path** (Phase 3.3 step 1 of 2). Default `true` (no behaviour
   change). The eventual production flip lives in the app boot — see
   "Step 2" below.

The original Phase 3.1 (per-field cell backing structs) was **skipped**.
The pragmatic single dispatcher delivers the same authority guarantee
in less code; factoring it out is a clean refactor that should wait
until a second entity type (Todoist/JIRA) makes the abstraction
concrete.

## Phase 3.2 details

### sort_key

The chord-op code path used to write sort_key directly to SQL because
the Loro side had no top-level encoding for it; reads from Loro
returned `Block.sort_key = default("A0")` for every block, and the
outbound projector's diff never saw a change, so SQL stayed correct
purely because Loro was bypassed.

Now sort_key lives at `meta["sort_key"]` on each tree node:

- `LoroBackend::update_block_sort_key(id, sort_key) -> Result<()>`
  inserts the meta key and bumps `updated_at`, then emits a
  `Change::Updated`.
- `read_block_from_tree` extracts the meta key and assigns
  `block.sort_key` after `Block::from_block_content` builds the default.
- `blocks_differ` / `block_to_params` / `block_diff_params` (in
  `loro_sync_controller.rs`) include sort_key, so the outbound
  projector emits SQL UPDATEs for sort_key changes.
- `apply_fields_changed` (inbound) routes a `sort_key` `FieldsChanged`
  tuple to `update_block_sort_key` instead of stuffing it into the
  properties JSON.
- `apply_create` and `apply_update_with_backend` honour a `sort_key`
  key in their JSON payload, calling `update_block_sort_key` after the
  base create / update.
- `BlockCellRegistry::write_field` gained a `"sort_key"` arm; the
  field is removed from the legacy skip list.

### marks

The marks round-trip in the Loro→SQL direction already worked
(`read_text_marks` in `loro_backend.rs` reconstructs the
`Vec<MarkSpan>` from the `LoroText`'s Peritext deltas, and the
outbound diff includes marks). The remaining gap was the cell-route
write path: chord ops calling `set_field("marks", json_str)` fell
through to direct SQL, and `apply_fields_changed` reflected that
change back into Loro via `update_block_marked`.

Now `BlockCellRegistry::write_field` gained a `"marks"` arm:

- Parses the value (`Value::Null` clears marks; `Value::String(json)`
  goes through `holon_api::marks_from_json`).
- Reads the current text via `LoroBackend::get_block` (the Peritext
  re-application path requires it; the wholesale text re-write is a
  no-op when the value is unchanged).
- Calls `LoroBackend::update_block_marked(id, current_text, &marks)`.

Marks leave the skip list. `apply_fields_changed`'s marks branch
remains for non-Loro-origin events (org parser, SQL-direct test
writes).

### Other skip-list entries

`id`, `depth`, `content_type`, `source_language`, `source_name` stay
on the SQL path:

- `id` is never re-assigned via `set_field`.
- `depth` is derived from the tree on outbound projection.
- The three `content_*` / `source_*` fields are written by the
  content-creation paths (`update_block_text`, chord-time content
  create) rather than `set_field` callers.

These can stay in the skip list indefinitely; the Phase 3.3 gate
(below) doesn't depend on them being migrated.

## Phase 3.3 step 1 — inbound runtime gate

`LoroSyncController` now carries three new shared atomics:

- `inbound_runtime_enabled: Arc<AtomicBool>` (default `true`)
- `inbound_runtime_drop_count: Arc<AtomicUsize>` (post-disable drops)
- `inbound_runtime_applied_count: Arc<AtomicUsize>` (all applies)

`on_inbound_event_inner` checks the gate after the existing echo-
suppression (`origin == EventOrigin::Loro` short-circuit). When the
gate is `false`, a non-Loro-origin event fires a `warn!` trace, ticks
the drop counter, and returns without touching Loro.

The `LoroSyncControllerHandle` exposes:

- `disable_inbound_runtime()` / `enable_inbound_runtime()`
- `inbound_runtime_enabled() -> bool`
- `inbound_runtime_drop_count() -> usize`
- `inbound_runtime_applied_count() -> usize`

Default behaviour is unchanged: existing tests, integration PBTs, and
production code paths see the same SQL→Loro reflection as before.

### Step 2 (not in this commit)

To actually demote: the app's startup boot (in `loro_module.rs` or
its caller) needs to call `handle.disable_inbound_runtime()` after
the org parser's seed pass has completed. The right place is
post-`controller.start().await` once a "seeded" signal exists — the
seed signal isn't defined yet.

Approximations that could work:

- After the first `last_synced` advance from `Frontiers::default()`
  to a non-empty Frontiers (signals the controller has done at least
  one outbound pass over seeded state).
- After the EventBus' initial-state-replay queue drains (cleaner;
  needs explicit signal from `EventBus::subscribe`).
- A flag the org parser sets directly when `read_org_file` returns.

Picking one is the next session's call. The gate is in place; the
data plane works either way.

### Arch-test gate

`no_inbound_loro_sync_runtime` was named in the original plan; it
would assert that the inbound runtime path stays *off* after
startup. With the gate as a runtime flag, the natural form is a
unit/integration test that:

1. Boots an app fixture with the gate enabled.
2. Lets startup seeding complete.
3. Flips the gate off.
4. Performs a chord op (which should write only via the cell route).
5. Asserts `inbound_runtime_drop_count == 0` afterwards.

That test is **deferred** until the production flip in step 2 lands —
it needs a stable seed signal to attach to.

## Files touched

- `crates/holon/src/api/loro_backend.rs` — added
  `update_block_sort_key`, extended `read_block_from_tree`, extended
  `diff_blocks_changed`, added the `sort_key_round_trips_through_loro_meta`
  test. Touched a few pre-existing archlint nits (`fallback` comments,
  `.ok()` on a Result, underscore-prefixed trait param) flagged on edit.
- `crates/holon/src/sync/loro_sync_controller.rs` — sort_key in
  `block_to_params` / `block_diff_params` / `blocks_differ`; sort_key
  branches in `apply_fields_changed` / `apply_create` /
  `apply_update_with_backend`; inbound runtime gate (atomics, accessors,
  gate-check in `on_inbound_event_inner`).
- `crates/holon/src/sync/block_cell_registry.rs` — `"sort_key"` and
  `"marks"` arms in `write_field`; both fields removed from the skip
  list. Added `use crate::api::repository::CoreOperations` for the
  marks branch's `get_block` call.

## Verification

| Check | Status |
|-------|--------|
| `cargo check --workspace --tests` | GREEN |
| `cargo test -p holon --lib sync::loro_sync_controller` | 11/11 |
| `cargo test -p holon --lib sync::block_cell_registry` | 5/5 |
| `cargo test -p holon --lib sync::loro_text_cell_backing` | 3/3 |
| `cargo test -p holon --lib api::loro_backend::tests` | 26/26 (incl. new `sort_key_round_trips_through_loro_meta`) |
| `cargo test -p holon-core --lib block_operations_tests` | 19/19 |
| `general_e2e_pbt_sql_only` (PROPTEST_CASES=1) | ✅ 588s |
| `general_e2e_pbt` (Full, PROPTEST_CASES=1) | ✅ 565s |

Three pre-existing failures in `cargo test -p holon --lib` were verified
against the parent revision and are NOT caused by this work:

- `api::backend_engine::tests::test_execute_operation` —
  "No provider registered for entity: test_item" (test_item provider
  registration order in a multi_thread test runtime).
- `api::loro_backend_pbt::stateful_tests::test_loro_backend_state_machine`
  — PBT child process crash on startup.
- `api::sync_pbt::tests::test_multi_peer_sync_iroh` — iroh PBT flake.

## Out of scope / next session

- **Phase 3.3 step 2**: production flip of the inbound runtime gate
  (decide on the seed-complete signal, wire it in `loro_module.rs`).
- **Arch-test gate `no_inbound_loro_sync_runtime`**: defer until
  step 2 lands.
- **Per-field cell backing structs** (the skipped Phase 3.1) —
  revisit when a second entity type (Todoist/JIRA) lands and the
  abstraction has a concrete second user.
- **`apply_properties_from_json` `.ok()` swallow** — there's an
  ALLOW(ok) marker on a malformed-JSON tolerance that should become
  fail-loud at the boundary.
- **Pre-existing 3 test failures** noted above — not new regressions,
  but they should get triaged in their own session.
