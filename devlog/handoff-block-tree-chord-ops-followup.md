# Handoff: Block-tree chord ops follow-up (post-2026-04-26)

**Status (2026-04-27, end of pass):** All chord-op ordering bugs from `handoff-block-tree-chord-ops.md` are closed. The PBT (`PROPTEST_CASES=20` with chord-op weights) no longer fails on any *ordering* invariant. The next failure mode the test surfaces is a `SwitchView → ToggleState` navigation-timing precondition gap that is not chord-op-related.

## What this pass closed

The original handoff said the chord-op layer was settled at the headless dispatcher level but flagged several follow-ups. Devlog 2026-04-26-183329 then migrated the PBT chord-op path from `synthetic_dispatch` to the real `send_key_chord` input pipeline, which exposed new failure modes hidden by the soft warnings. This pass fixes those.

### 1. Bug B (Loro TreeID lookup) — already in tree

`LoroBackend::resolve_to_tree_id` (`crates/holon/src/api/loro_backend.rs:1372-1383`) accepts both `block:UUID` (prefixed) and bare `UUID` via `EntityUri::from_raw`, then looks up by the bare path against `STABLE_ID` metadata. `set_stable_id` (`loro_share_backend.rs:811-821`) strips to bare on write. The "URI→TreeID tolerance" the handoff requested is structurally in place; nothing to do here.

### 2. inv16 synthetic-id leak after SplitBlock — fixed

`crates/holon-integration-tests/src/pbt/sut.rs:4707` — inv16 was comparing reference-state ids (which still carry synthetic placeholders like `block::split-N` until the SUT's post-split resolve) directly against production draggable ids (which carry the real DB UUID). Fix: route `block.id` through `self.resolve_uri(...)` before the comparison, mirroring handoff fix #9 at the spot-check site.

### 3. Soft ordering warning → hard assert

`crates/holon-integration-tests/src/assertions.rs:117` was an `eprintln!("WARNING:")` with the comment "soft assertion — ordering bugs tracked separately". Promoted to `assert_eq!`. This unmasked two real production bugs (#4 and #6 below).

### 4. Org renderer ordered by stale `sequence` instead of `sort_key`

`crates/holon-org-format/src/org_renderer.rs:57-61, 95-110` sorted root and child blocks by `block.sequence()` (parser-assigned ordinal). `BlockOperations::{indent,outdent,move_block,split_block}` write `sort_key` (the fractional-index source of truth) but never touch `sequence`. After any structural mutation the new block's `sequence` defaulted to 0, tying with its anchor; the id-tiebreaker placed UUID-named blocks before literally-named ones. Switched both sort calls to `block.sort_key.cmp(&other.sort_key)`.

### 5. The big one — `default_sort_key` lower-case bug

`gen_key_between(Some("a0"), None) = "A180"`, but `"A180" < "a0"` lexicographically (`'A' = 0x41 < 'a' = 0x61`). So a block at the default `'a0'` (lower-case) and any block whose key was generated *after* it produce *opposite* lex order vs fractional order. Before this fix:

- `Create` mutation → block lands at `sort_key='a0'` → renderer puts it after every `gen_n_keys` sibling (correct, lucky).
- `Indent` after a `Create` → `gen_key_between(Some("a0"), None) = "A180"` → indented block ends up *before* the just-created sibling under the new parent (wrong).

Fixed by changing the default to upper-case `"A0"`, which lex-sorts consistently with `FractionalIndex::to_string()` outputs:

- `crates/holon-api/src/block.rs::default_sort_key` — `"A0"`
- `crates/holon/sql/schema/blocks.sql` — `DEFAULT 'A0'`
- `crates/holon-frontend/src/lib.rs` (2 doc-init INSERT sites)
- `crates/holon-todoist/src/models.rs` (placeholder)
- `crates/holon-org-format/src/{org_renderer,block_diff}.rs` (test fixtures)
- `crates/holon-core/src/block_operations_tests.rs` (fallback)
- `crates/holon-integration-tests/src/assertions.rs` (normalize_block sort_key)

### 6. Ref-state `Create` slot vs production `'A0'` semantics

Production `Create` without an explicit sort_key lands at `'A0'`, which lex-sorts after every `gen_n_keys`-assigned sibling — so the new block appends at end. Ref-state's `recanon_and_rebuild` sorts children by `(source-first, sequence, id)`; the new block has `sequence=0` (default), tied with the parser's first sibling, and id-tiebreak puts it in some arbitrary slot that depends on the new id's lex position.

Fix: `crates/holon-integration-tests/src/pbt/types.rs::Mutation::Create::apply_to` now assigns `block.sequence = max_sibling_sequence + 1` so the canonicalizer places the new block last (matching production's "append at end").

### 7. Ref-state Indent / Outdent positioning

Production `indent` makes the block the **last** child of its previous sibling (`sort_key = gen_key_between(siblings.last().sort_key, None)`). Production `outdent` calls `move_block(id, grandparent_id, Some(parent_id))` — block becomes the next sibling **after** its old parent.

Ref-state previously used `set_parent` for both, letting `recanon_and_rebuild` reassign sequence by tie-break — which doesn't match production positioning. Added two helpers in `reference_state.rs`:

- `move_as_last_child(block_id, new_parent)` — sets sequence to `max_sibling_seq + 1` under new parent. Used by `Indent`.
- `outdent_block(block_id)` — shifts later siblings under grandparent up by 1, sets `block.sequence = old_parent_seq + 1`. Used by `Outdent`.

`state_machine.rs:2743-2752` now calls these instead of `set_parent`.

## Verified

`PROPTEST_CASES=20` with chord-op weights — failure shape progression across this pass:

```
inv16 synthetic-id leak (sort_key='a0' fix #2 above)
  → Org file ordering wrong (renderer-uses-sequence #4 above)
    → Org file ordering wrong on Create (lex/case #5 above)
      → Org file ordering wrong on Indent-after-Create (lex/case #5 above)
        → Org file ordering wrong on Indent+Outdent (#7 above)
          → ToggleState entity not in ViewModel (NEW, unrelated)
```

Final command:
```sh
PROPTEST_CASES=20 PBT_WEIGHT_INDENT=10 PBT_WEIGHT_OUTDENT=10 PBT_WEIGHT_SPLIT_BLOCK=10 \
PBT_WEIGHT_CLICK_BLOCK=10 PBT_WEIGHT_DEFAULT=1 \
cargo nextest run -p holon-integration-tests --test general_e2e_pbt \
  -E 'binary(general_e2e_pbt) and test(=general_e2e_pbt)'
```

## What's left

### Bug X — `ToggleState` after `SwitchView { view_name: "sidebar" }` times out

Minimal shrink (with chord-op weights still on):
```
WriteOrgFile (single block with query/render/src children)
StartApp
SwitchView { view_name: "sidebar" }
ToggleState { block:..., new_state: "DONE" }
```

Failure: `crates/holon-integration-tests/src/pbt/sut.rs:1727` panics with
> [ToggleState] entity block:... did not appear in the resolved ViewModel within 5s — sidebar nav may not have populated the main panel yet.

`SwitchView` swaps the active view (e.g. "all" → "sidebar"). After the switch the main panel renders something other than the previously-focused block, but the `ToggleState` precondition still expects the block to be in the resolved ViewModel. Either:

- (a) The `SwitchView` apply branch in ref-state should clear `focused_entity_id[Main]` when the new view doesn't render that entity, so `ToggleState` preconditions reject the transition; or
- (b) The SUT should wait longer / re-navigate to surface the entity in main panel after a `SwitchView`.

(a) is more conservative — it brings the ref-state precondition closer to what production can actually act on.

### Higher PROPTEST_CASES

Worth pushing past 20 (100+ overnight) once Bug X is closed, to surface deeper interactions before declaring chord-ops fully settled.

### Pre-existing diagnostics

Compile diagnostics for `e2e_backend_engine_test.rs:111` (`EntityName` PartialEq), `todoist_datasource.rs:857` (missing `cycle_task_state` impl), `loro_marks_spike.rs`, `fork_at_test.rs`, `loro_backend_pbt.rs:949` are unrelated. They predate this work.

## Files touched (12)

Production code (3):
- `crates/holon-api/src/block.rs` (default_sort_key)
- `crates/holon-org-format/src/org_renderer.rs` (sort by sort_key)
- `crates/holon-frontend/src/lib.rs` (doc-init defaults)
- `crates/holon-todoist/src/models.rs` (placeholder)
- `crates/holon/sql/schema/blocks.sql` (column default)

Test infrastructure / fixtures (7):
- `crates/holon-core/src/block_operations_tests.rs`
- `crates/holon-org-format/src/block_diff.rs`
- `crates/holon-integration-tests/src/assertions.rs`
- `crates/holon-integration-tests/src/pbt/sut.rs`
- `crates/holon-integration-tests/src/pbt/types.rs`
- `crates/holon-integration-tests/src/pbt/reference_state.rs`
- `crates/holon-integration-tests/src/pbt/state_machine.rs`

## Key non-obvious finding to remember

Lower-case `'a0'` (the historical `default_sort_key`) lex-sorts AFTER the entire upper-case-hex range that `FractionalIndex::to_string()` produces. Whenever a default-keyed block coexisted with a `gen_n_keys` / `gen_key_between` block, the renderer's lex-comparison disagreed with the fractional-index intent. Easy to miss — the SQL schema's `DEFAULT 'a0'` had been there for a long time and worked by accident in pure-default scenarios. Always keep `default_sort_key` in the same hex case as `FractionalIndex` outputs.
