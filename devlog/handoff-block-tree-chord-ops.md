# Handoff: Block-tree chord ops (outdent / split_block / move / indent)

**Status (2026-04-26, end of pass):** PBT passes `PROPTEST_CASES=20` cleanly with the chord-op weight knobs from this handoff. Nine fixes landed across production code and test infrastructure. Bug B (Loro TreeID lookup) is still active in the live app but doesn't surface in headless because the PBT bypasses chord routing — see end of file.

## Fixes landed this pass

1. **`crates/holon-core/src/traits.rs:779`** — `split_block` was inserting the new block's text under the `"title"` param key, leaving the new block with `content=""` and a stray `properties.title` set. Changed key to `"content"`. Reproduced by PBT (`SplitBlock { position: 0 }`).

2. **`crates/holon-integration-tests/src/pbt/sut.rs:2195`** — SUT mapped a DB id (`block:UUID`) by re-wrapping it through `EntityUri::block(...)` (which prefixes), producing `block:block:UUID`. Switched to `EntityUri::from_raw(...)`.

3. **`crates/holon-integration-tests/src/pbt/sut.rs` (org-file ref blocks)** — `ref_blocks_org_only` translated `parent_id` synth→file but not `b.id` synth→real, so post-SplitBlock org files (with the real UUID on disk) compared against `block::split-N` placeholders. Added `b.id = resolve(&b.id)`.

4. **`crates/holon-org-format/src/parser.rs`** — added `assign_per_parent_sort_keys` post-pass after `process_headlines` so sibling groups get unique fractional indices via `gen_n_keys(N)` instead of every block staying at `sort_key="a0"`. Without this, `BlockOperations::get_prev_sibling` (filter `sort_key < block.sort_key`, strict) finds nothing for any sibling and `indent` panics.

5. **`crates/holon-orgmode/src/block_params.rs`** — `build_block_params` (the choke point feeding OrgSyncController-driven CREATE/UPDATE batches into SQL) was not inserting `sort_key`, so even with the parser fix the column always defaulted to `'a0'`. Added the param.

6. **`crates/holon-integration-tests/src/assertions.rs`** — `normalize_block` was structurally comparing `sort_key` values; production assigns real fractional indices, the reference model only tracks `sequence`. Normalized to `"a0"` so structural equality ignores the impl detail; ordering is still validated separately by `assert_block_order`.

7. **`crates/holon-integration-tests/src/pbt/reference_state.rs`** — `split_block` set the new block's sequence to `original_seq + 1` and let `recanon_and_rebuild` tie-break-by-id decide the actual order. When pre-existing siblings already occupied that sequence the new block could end up *past* them, so a later `Indent` saw a different `previous_sibling` than production's sort_key-based view. Now we shift every later sibling up by one before inserting, mirroring production's "place strictly between original and next sibling" semantic.

8. **`crates/holon-orgmode/src/org_sync_controller.rs::blocks_differ`** — the function ignored `sort_key`, so re-parsing a file with a different sibling count (e.g. `BulkExternalAdd` adding 9 blocks to a 3-block file) produced 9 fresh fractional keys from `gen_n_keys(12)` for the new blocks while existing blocks kept stale keys from `gen_n_keys(3)` — the two keyspaces aren't lexicographically order-comparable, so `get_prev_sibling` failed on the first bulk-added block. Adding `a.sort_key != b.sort_key` to `blocks_differ` re-issues UPDATE events for existing blocks, so the whole file shares one consistent fractional ordering.

9. **`crates/holon-integration-tests/src/pbt/sut.rs` (post-mutation spot-check)** — the spot-check PRQL filtered by the raw `block_id` returned by `event.mutation.target_block_id()`, which for SplitBlock-created blocks is the synthetic `block::split-N` placeholder. Resolved via `self.resolve_uri(&block_id)` so the query hits the real DB id.

## Verified by PBT

After fixes 1–9 the PBT failure shifted layer by layer:
"Backend diverged on doubled-prefix split id" → "Org file diverged on synthetic id" → "Indent: no previous sibling" → "Org file diverged on sort_key field" → "Backend diverged on chord-op chain" → "Cannot indent: no previous sibling (bulk-add case)" → "Spot-check: no row for synthetic id" → **PASS**.

Final command:
```sh
PROPTEST_CASES=20 PBT_WEIGHT_INDENT=10 PBT_WEIGHT_OUTDENT=10 PBT_WEIGHT_SPLIT_BLOCK=10 \
PBT_WEIGHT_CLICK_BLOCK=10 PBT_WEIGHT_DEFAULT=1 \
cargo nextest run -p holon-integration-tests --test general_e2e_pbt \
  -E 'binary(general_e2e_pbt) and test(=general_e2e_pbt)'
```
→ `1 test run: 1 passed (1 slow), 2 skipped` (~6 min on the dev machine).

## What still needs doing

- **Bug B (Loro TreeID)** is unrelated to the headless PBT path (the test bypasses chord routing to drive `BlockOperations` directly) but still active in the live app — see `/tmp/holon.log` showing `update_parent_id failed: Cannot resolve parent URI to TreeID: block:UUID`. Fix is to make `loro_sync_controller`'s URI→TreeID lookup tolerant of the prefixed `block:UUID` form the same way `BlockEntity::parent_id()` was tightened in the previous pass. To validate, manual outdent/split testing in the live GPUI app while watching `/tmp/holon.log` is still the ground truth — the headless PBT cannot reproduce this layer.
- **Higher PROPTEST_CASES.** 20 cases passed cleanly. Worth pushing to 100+ overnight to catch deeper interactions (e.g. peer-merge x chord-op chains) before declaring the chord-op layer settled.

## What works

The PBT (`crates/holon-integration-tests/tests/general_e2e_pbt.rs`) now exercises the full operation-execution path for `Indent` / `Outdent` / `MoveUp` / `MoveDown` / `SplitBlock` and reproduces production failures.

Run reproducer:
```sh
PROPTEST_CASES=5 PBT_WEIGHT_INDENT=10 PBT_WEIGHT_OUTDENT=10 PBT_WEIGHT_SPLIT_BLOCK=10 \
PBT_WEIGHT_CLICK_BLOCK=10 PBT_WEIGHT_DEFAULT=1 \
cargo nextest run -p holon-integration-tests --test general_e2e_pbt \
  -E 'binary(general_e2e_pbt) and test(=general_e2e_pbt)'
```

Shrinks reliably to a 2-step sequence:
```
ClickBlock { region: Main, block_id: <some block> }
SplitBlock { block_id: <same>, position: N }   // or Outdent / Indent / MoveUp / MoveDown
```

## How the PBT exercises the production stack

The PBT's `Indent`/`Outdent`/`MoveUp`/`MoveDown`/`SplitBlock` apply branches in `crates/holon-integration-tests/src/pbt/sut.rs:2065-2110` go through a new helper `dispatch_block_op` (`sut.rs:417-450`) which calls `synthetic_dispatch("block", op, params)` directly. This bypasses `bubble_input` (the headless `HeadlessInputRouter` has its own emission-cascade reachability gap unrelated to production), and exercises the same `OperationDispatcher → SqlBlockOperations → BlockOperations::{indent,outdent,move_block,split_block}` chain that the live GPUI app reaches once chord routing succeeds.

This was the unlock for "make the tests detect production bugs". Previously every PBT shrink got stuck at "Keychord did not match" in the headless router and never reached `BlockOperations`.

## Fixes already landed

1. **`crates/holon-frontend/src/shadow_builders/render_entity.rs`** — union `ctx.operations` with the inner widget's vm.operations (was attach-when-empty, which silently dropped the block-level chord ops because `editable_text` already had its own `set_field`).

2. **`crates/holon-frontend/src/focus_path.rs`** — added `LiveBlockResolver` type, `InputRouter::set_block_resolver`, `build_focus_path_with_resolver`, `dfs_find_with_resolver`. Lets `nav.bubble_input` cross `live_block` boundaries by asking a resolver for the block's current `ReactiveViewModel`.

3. **`frontends/gpui/src/{navigation_state,lib}.rs`** — `NavigationState::set_block_resolver` plumbing; production installs a resolver that calls `engine.snapshot_reactive`. Verified by `holon.log` showing operations reach the dispatcher in the live app.

4. **`crates/holon-core/src/traits.rs:1110-1129`** — `BlockEntity::parent_id()` for `holon_api::Block` returns the **full URI** (`block:UUID`), not the bare path. SQL stores the prefixed form (`EntityUri::block(uuid)` serializes that way at the boundary, see `crates/holon-api/src/entity_uri.rs:296`). Returning `as_block_id()` made `outdent`'s `get_by_id(parent_id)` miss every parent → "Parent not found".

5. **`crates/holon-api/src/block.rs:300-309`** — added `sort_key: String` field with `default_sort_key()` returning `"a0"` (matches SQL column default). Refactored constructors (`new_text`, `new_source`, `new_image`, `from_block_content`) and all `Block { ... }` literals across `parser.rs`, `org_renderer.rs`, `block_diff.rs`, `holon-org-format/src/parser.rs`, and the PBT state machine to use `..Block::default()` so future field additions are a one-line change.

6. **`crates/holon-core/src/traits.rs:1116-1125`** — `BlockEntity::sort_key()` reads `self.sort_key` (was returning `self.id.as_str()` = `"block:UUID"`, with non-hex `:`/`-` → `gen_key_between` panicked on `from_hex_string`'s `u8::from_str_radix(.., 16).unwrap()`).

7. **`crates/holon-core/src/traits.rs:756-762`** — `split_block` mints `format!("block:{uuid}")` for the new block id. Was a bare `Uuid::new_v4()` → format mismatch with all other blocks.

8. **`crates/holon-core/src/traits.rs` (5 sites)** — replaced empty-string entity_name placeholders in `move_block_op` / `set_field_op` / `set_is_document_op` / `editor_focus_op` inverse and follow-up construction with valid URI-scheme strings (`"placeholder"`, `"navigation"`). The `OperationDispatcher` overwrites `entity_name` after `execute_operation` returns (`operation_dispatcher.rs:504`), but `EntityName::new` debug-asserts `is_valid_uri_scheme` immediately on construction — so the placeholder must already pass that check. Without this, every outdent panicked with `Invalid entity name after normalization:` exactly as seen in `holon.log`.

## Remaining bugs (PBT will surface them next; see `/tmp/holon.log`)

### Bug A — `Failed to generate fractional index between given keys`

```
ERROR Operation 'outdent' on entity 'block' failed:
  Failed to generate fractional index between given keys
```

`gen_key_between` returns `Err(())` from inside `loro_fractional_index::FractionalIndex::new` when `prev_key >= next_key` (or some closely-related ordering issue). After my fix to `BlockEntity::sort_key()`, sort keys are now **read** correctly, but they may collide / be ordered in a way that leaves no room for a new key.

Likely cause: blocks created via `parser.rs` (org parsing) or other paths leave sort_key = `"a0"` (the default). Multiple blocks with identical sort_key → `gen_key_between(Some("a0"), Some("a0"))` can't generate a key strictly between two equal keys.

**To fix:** when blocks are created from org files / Loro / split, generate a real fractional index instead of leaving the default `"a0"`. The org parser at `crates/holon-org-format/src/parser.rs:282-293` should use `gen_key_between(prev_key, None)` per heading. Look at how `OrgSyncController` populates sort_key today — there may be a separate normalization pass that's missing.

**Quick check:** `SELECT id, sort_key FROM block ORDER BY parent_id, sort_key` in `holon-direct` against your live DB. If many siblings share `sort_key = 'a0'` you've found it.

### Bug B — Loro sync `Cannot resolve parent URI to TreeID`

```
ERROR LoroSyncController: Failed to apply FieldsChanged event for block:5df48242-...:
  update_parent_id failed: Internal error: Failed to update parent_id:
  Cannot resolve parent URI to TreeID: block:c3ad7889-...
```

The SQL outdent succeeded (CDC event fired), but Loro's mirror can't apply it — the new parent isn't a `TreeID` in the Loro tree. This is Loro's analog of the bare-id-vs-prefixed-id issue, but in the URI→TreeID lookup table.

Look at `crates/holon/src/sync/loro_sync_controller.rs` `update_parent_id` and `apply_fields_changed` — there's a URI lookup that's failing for the prefixed form. Likely the lookup table is keyed by bare path while `parent_id` arrives as full URI (or vice versa).

### Bug C — Backend ↔ reference state divergence on chord ops

Once Bugs A and B are fixed, the PBT will surface the next layer:
```
assertion `left == right` failed:
  Backend diverged from reference: Blocks differ between actual and expected
```

The reference state's `apply` for `Indent`/`Outdent`/`Move*`/`SplitBlock` in `crates/holon-integration-tests/src/pbt/state_machine.rs` doesn't update its block-tree the same way production does — sort_key changes, depth changes, parent_id changes. Mechanical: mirror the production trait logic in the ref-state apply branches.

## Files to look at first in the next session

- `/tmp/holon.log` — last 200 lines have the latest error patterns from manual app testing. The user reported "outdent works sometimes, split_block does not work at all".
- `crates/holon-core/src/traits.rs` — `outdent` (~675), `move_block` (~564), `split_block` (~722). All call `gen_key_between` with sort_keys; investigate where invalid hex pairs come from.
- `crates/holon/src/sync/loro_sync_controller.rs` — `apply_fields_changed`, `update_parent_id`. The TreeID resolution.
- `crates/holon/src/sync/loro_blocks_datasource.rs` — likely owns the URI ↔ TreeID map.
- `crates/holon-org-format/src/parser.rs` and `crates/holon-orgmode/src/parser.rs` — block creation from org files; check whether sort_key gets a real fractional index or stays `"a0"`.

## How to validate progress

1. Run the PBT reproducer above. Failure should move from "Invalid entity name" → "fractional index" → "TreeID" → "Backend diverged". Each shift = a layer fixed.
2. After each fix, manually retry outdent/split in the live GPUI app while watching `/tmp/holon.log`. Production behavior is the ground truth.
3. The headless PBT can't reproduce the chord-routing layer (different `HeadlessInputRouter` reachability gap) — `gpui_ui_pbt.rs` would, but is slow/macOS-display-bound. Production manual + headless dispatcher PBT covers most of the surface.

## Pre-existing diagnostics (NOT blocking, NOT mine)

The compile diagnostics for `loro_sync_controller.rs:320`, `loro_marks_spike.rs`, `fork_at_test.rs`, `loro_backend_pbt.rs:949` are about a `loro` API breakage (`Side` private, `LoroDoc` Result wrapping, method-arity changes). They predate this work and don't affect the binary or PBT crates I edited. Ignore unless they start blocking your test runs.

## Memory files updated

None this session — recommend after Bug A fix you write a memory entry summarizing the sort_key column ↔ struct field gap and the bare-id-vs-prefixed-id pattern, since those came up repeatedly.
