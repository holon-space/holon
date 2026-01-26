# SplitBlock multi-bug fix — handoff (seed=42)

## TL;DR

`gpui_ui_pbt --features pbt` was failing on `PROPTEST_SEED=42` at step 6 (now step 3 after the StartApp weight bump) with `assertion left == right failed: Backend diverged from reference: Blocks differ`. Root cause was a tangle of four bugs in the production SplitBlock path; three are fixed, the fourth (sibling-ordering after the new block lands) is the current failure.

## What ships in this branch

1. **StartApp generator starvation** (`crates/holon-integration-tests/src/pbt/transitions/start_app.rs:42-48`). Weight was a flat `2` — unlucky seeds (e.g. `1778711358`) never selected StartApp in 50 pre-startup steps. Now: `weight = 2 + 8·pre_startup_file_count`, so as more org files land, StartApp's odds climb. Verified the original starvation seed now fires StartApp at step 3.

2. **`SplitBlock` typed at the boundary** (`crates/holon-core/src/traits.rs:736`). Signature was `fn split_block(&self, id: &str, position: i64)`. The `&str` arrived already prefixed (`"block:c2f12z-s"`) but `EntityUri::block(id)` at the call site (`traits.rs:828`) unconditionally re-prefixed it to `"block:block:..."` — a classic parse-don't-validate smell. Signature is now `fn split_block(&self, id: &EntityUri, position: i64)`, and the dispatch macro (`crates/holon-macros/src/operations_trait.rs`) gained:
   - extraction: `Value::String → EntityUri::from_raw(s)` (line ~570)
   - param-call borrow: `&EntityUri` (line ~603)
   - param→Value flatten (line ~342)

   Call sites updated: `block_operations_tests.rs:463/487/509/512` (synthetic store keyed on `"block:A"` now).

3. **Refuse to split Page blocks** (`traits.rs:744-749`). Pages have null `parent_id` at the SQL layer — the `__document_root__` you see in `Block { parent_id }` is added by hydration, not stored. Splitting a Page used to silently orphan the new block under `sentinel:no_parent`. Now: early `is_page()` check returns `Err`. **Caveat:** `is_page()` reads `tags` which the `SqlBlockOperations::get_by_id` path does NOT hydrate from the `block_tags` junction (see memory entry "Storage split is Turso's concern, not the renderer's"). So this guard fires in some paths but not others — it's defense in depth, not the primary fix.

4. **Enter capture-phase dispatch picked the wrong editor** (`frontends/gpui/src/views/editor_view.rs:505-528`). On Page-level pages with multiple stacked editors, GPUI capture-phase fires top-down — the Page's EditorView ran `Enter` on behalf of the focused child and dispatched `split_block` with its own `row_id` (the Page). Two-layer fix:
   - **Guard**: skip the capture body unless `input.read(cx).focus_handle(cx).is_focused(window)` is true (strict equality, not `contains_focused` — verified by reading gpui's `FocusId::is_focused`).
   - **Target**: dispatch against `services.focused_block()` (UiState's notion of focus) instead of `row_id`. So even when the capture fires on a shared ancestor editor, the operation still lands on the logically focused leaf.

   Runtime confirmation at seed=42 step 3: the diag showed `row_id="block:ref-doc-0" focused=true` before the fix — i.e. only the Page editor had GPUI focus, the leaf had none, and the split was targeting the Page. After the fix, the new block lands under `block:ref-doc-0` (correct parent) with content `"d83xI c8NQQ"` (correct cursor-tail of c2f12z-s) — `c2f12z-s` is split correctly down to `"Hym"`.

## What's still failing

After the four fixes above, seed=42 step 3 now hits `assertions.rs:133` — `Org file block ordering wrong: Block order mismatch under parent 'block:ref-doc-0'`:

```
Org file order:  [-q--2b-9.., c2f12z-s, nvhz.., e5f786c4..(new)]
Expected order:  [-q--2b-9.., c2f12z-s, e5f786c4..(new), nvhz..]
```

The new block lands at the end of the parent's children list instead of immediately after `c2f12z-s` (the block that was split). Hypotheses, ranked by likelihood:

1. **Most likely: Loro tree doesn't contain pre-existing siblings.** The pre-split siblings (`c2f12z-s`, `nvhz..`, `-q--2b-9..`) were created by the org-file parser path and live in the SQL `block` table; they may not have been seeded into the Loro tree. `LoroBackend::update_block_position(new_id, parent_id, Some(after.id()))` calls `require_tree_id(after_id)` → `find_tree_id_by_stable_id("c2f12z-s")` (loro_backend.rs:1518). If `c2f12z-s` isn't in the tree, this returns `BlockNotFound` and the call errors. But the new block IS in the SQL result, so either:
   - `create_block` ran (Loro insert) but `update_block_position` was skipped/errored silently, leaving the new block at whatever default position Loro chose (last child).
   - OR the new block is laid out by sort_key in the matview projection, and the sort_key was assigned before the position-update was attempted, putting it at the end.

   The `[create_entity-diag]` trace previously showed `update_block_position done` after `create_block done` — so the call succeeded. But "succeeded" may have meant "no-op because `pred` resolved to None" if there's a silent fallback.

   **First investigation step**: grep `update_block_position` in `loro_backend.rs` and prove whether `require_tree_id(after_id)` is non-fatal when the pred isn't in the tree. If it errors, find what swallows the error. The "fail loud" philosophy says nothing should swallow this — file as a separate bug.

2. **Less likely: `new_child_anchor` returns a sort_key positioned at the end.** `crates/holon-core/src/traits.rs:806` computes `new_sort_key = ordering.new_child_anchor(&parent_for_anchor, Some(id_str))`. In Loro mode this is supposed to return a placeholder that gets overwritten by `apply_create` reading `position_after_block_id`. Check `apply_create` in `sql_operation_provider.rs` — make sure `position_after_block_id` is actually `c2f12z-s` (not stale from the broken-target days) and that it propagates through.

3. **CDC projection lag.** The matview `block` might be reading stale sort_keys at the moment `expected_block_ids` is asserted. `wait_for_blocks_synced` only waits for IDs, not ordering. The reference's `recanon_and_rebuild` produces a deterministic ordering; the SQL side relies on Loro→SQL projection. If the projector hasn't observed the `mov_after` yet, ordering would be wrong. Look at `LoroSyncController.on_loro_changed` to see whether sort_key updates propagate inside the `wait_for_blocks_synced` window.

## Diagnostic recipes that worked

- `eprintln!` over the LLDB DAP for this test. `debugger_mcp` OOM'd twice (RSS > 4GB) trying to expand async state-machine locals — the function locals for `split_block` are buried in a huge `Future::poll` generator. The cheap workaround was adding `eprintln!` at:
  - `crates/holon-core/src/traits.rs` split_block entry (`id`, `position`, `block.parent_id()`, `parent_for_split`).
  - `crates/holon/src/sync/block_cell_registry.rs:382` (already there as `[create_entity-diag]` lines — pre-existing tooling).
  - `frontends/gpui/src/views/editor_view.rs:577` (`row_id`, `cursor_byte`).
- Reverted all temp prints before commit.

## Working-tree health

- `cargo check -p holon-core -p holon-gpui --features pbt` → clean (Finished `dev` profile, no errors).
- `cargo test -p holon-core --lib split_block` → 3/3 passing.
- `PROPTEST_SEED=42 cargo test -p holon-gpui --test gpui_ui_pbt --features pbt` → now fails at `assertions.rs:133` (ordering), not at `:60` (wrong-block divergence) — strictly later in the pipeline.
- `Cargo.lock` reverted to committed state (`turso_core 0faa82e1`). Earlier `cargo update turso` had bumped to `db64e76` which has unhandled AST variants in upstream and doesn't compile.

## Suggested next steps

1. Pick up at hypothesis #1 above — confirm Loro contains the pre-existing siblings or not at step 3 (e.g. `inspect_loro_blocks` MCP tool on a paused run, or `eprintln!` the tree node count inside `update_block_position`).
2. If the pre-existing siblings aren't in Loro, this is a known design issue: org-parser path bypasses Loro. The fix path is either to seed siblings into Loro at parse time, or to make `update_block_position` fall back to SQL-only when the predecessor isn't in Loro.
3. Strengthen the Page guard from #3 above by hydrating `tags` in `get_by_id`, or by adding a `is_document` flag to the SQL row that doesn't depend on the junction table.
4. Audit other capture_action handlers in `editor_view.rs` (Tab, Shift+Tab, Backspace, MoveUp, MoveDown, Escape) — they all share the same "Page-editor steals from focused leaf" structural bug. Apply the same two-layer guard (focus check + `services.focused_block()` target).

## Files touched

```
crates/holon-core/src/traits.rs                              # signature + Page guard + EntityUri threading
crates/holon-core/src/block_operations_tests.rs              # prefixed ids for synthetic store
crates/holon-macros/src/operations_trait.rs                  # EntityUri extraction/borrow/Value
crates/holon-integration-tests/src/pbt/transitions/start_app.rs  # weight scaling
frontends/gpui/src/views/editor_view.rs                      # capture-phase focus guard + services-focused target
```

## Reproducer

```
PROPTEST_SEED=42 cargo test -p holon-gpui --test gpui_ui_pbt --features pbt 2>&1 | tee /tmp/gpui_ui_pbt.log
```

Logs from the working session:
- `/tmp/gpui_ui_pbt.log` — original failure (seed=1778711358, never reaches StartApp).
- `/tmp/gpui_ui_pbt_42.log` — seed=42, original SplitBlock divergence at step 6.
- `/tmp/gpui_ui_pbt_diag.log` — seed=42 with diag prints proving `row_id="block:ref-doc-0"` was dispatched.
- `/tmp/gpui_ui_pbt_postfix.log` — after all 4 fixes, ordering panic at `assertions.rs:133`.
- `/tmp/gpui_ui_pbt_postfix2.log` — confirmed `row_id="block:ref-doc-0" focused=true` for the Page editor.
- `/tmp/gpui_ui_pbt_postfix3.log` — final state, ordering panic with detailed expected/actual ordering.
