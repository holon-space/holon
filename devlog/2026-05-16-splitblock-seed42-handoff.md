# SplitBlock seed=42 — handoff after three-layer fix

Worktree: `.claude/worktrees/split-block-hit-test-tiebreak`
Repro:    `PROPTEST_SEED=42 cargo test -p holon-gpui --test gpui_ui_pbt --features pbt 2>&1 | tee /tmp/gpui_pbt.log`

## TL;DR

Three real bugs fixed in this branch. Previous failure at step 4 SplitBlock
(coordinate-stale click → wrong block focused → ordering.place error) is gone.
seed=42 now fails one step further, at step 4 SplitBlock for a different
reason: `block:c2f12z-s` (the `task_state: "WAITING"` block) is never
rendered into the Main panel after step 3 NavigateFocus. Its siblings
`block:nvhz...` and `block:-q--2b-9...` do appear. That is the only thing
keeping the seed from passing more steps; everything upstream of the render
is healthy.

## What ships in this branch

1. **GPUI hit-test tie-break on entity_id** — `frontends/gpui/src/user_driver.rs:140`.
   `editable_text#N` wrapper and `editable-text-block:{id}-content` child
   register identical bounds. The previous `sort_by(area)` was unstable, so on
   ties the wrapper (entity_id=None) non-deterministically won the hit-test
   log and obscured the real target. Sort now tie-breaks on
   `entity_id.is_some()` so the entity-bearing element wins consistently.

   Caveat: this only fixes the *logging*. GPUI's own dispatch tree still
   routes synthetic mouse-downs by its own hit-test, so a wrapper handler
   can still intercept the click in production. See open task #16.

2. **Pre-transition children-settled gate on the SUT** —
   `crates/holon-integration-tests/src/pbt/sut.rs`. New field
   `pre_ref_state: Option<ReferenceState>` on `E2ESut`, stashed at the END
   of `apply_transition_async` so during the next call it holds the
   previous post = the current pre. New helper
   `wait_for_children_settled(parent_id, timeout)` reads from it, then
   counts widgets with widget_type ∈ {rendered_text, editable_text}
   whose entity_id matches a known child of `parent_id` in the pre-state.

   Called from `apply_split_block` between `wait_for_entity_bounds` and
   `wait_for_widget_kind`. The pre-state choice is what makes this
   uniform across transitions: the `ref_state` passed to `apply_to_sut`
   is the POST-transition state, which already contains blocks the SUT
   hasn't been told to create yet. Waiting on the post-state would need
   a per-transition exclusion list (synthetic ids etc.); waiting on the
   pre-state expresses "show me what the user can see right now."

3. **OrgSync ordering propagate-wait + bare-key fix** —
   `crates/holon-orgmode/src/org_sync_controller.rs`. Two bugs in
   `on_file_changed`'s disk-order-replay block:

   - The existing code looked up `live_children` with `parent.id()` (bare,
     e.g. `"ref-doc-0"`), but `BlockOrdering::children` filters with
     `b.parent_id.as_str() == parent_id`, where `as_str()` on the block's
     parent returns the FULL URI (`"block:ref-doc-0"`). So `live_children`
     was always empty and the entire `place()` loop was silently a no-op.
   - After fixing that, `place()` could be called for newly-created blocks
     before the `LoroSyncController` inbound consumer had landed them in
     the Loro tree, surfacing as `update_block_position: Block not found`.

   Fix: between the create-batch and the disk-order replay, poll
   `ordering.children(parent_full_uri)` until every newly-created block
   appears (up to 2 s); bails loudly with
   `[on_file_changed] new blocks did not appear in ordering` if not.
   Disk-order-replay now keys `live_children` by full URI throughout.

   The original silent error `[OrgMode] Failed to process existing file ...
   ordering.place failed: Block not found` is gone in run-9.

## What's still failing

seed=42 step 4 SplitBlock targets `block:c2f12z-s` and times out in
`wait_for_entity_bounds`:

```
[SplitBlock] bounds unavailable for block:c2f12z-s:
wait_for_entity_bounds: timed out after 5s waiting for bounds of
entity "block:c2f12z-s" — tried element ids "render-entity-...",
"selectable-...", and entity_id scan; element was never rendered.
```

Post-NavigateFocus Main-panel geometry (from `/tmp/gpui_pbt9.log`
lines 438–450 in the failure dump) contains:

- `rendered-text-block:-q--2b-9--g39c5-e06u1565-5-content`
- `rendered-text-block:nvhz--r75-0sz-7-n37s9o5x7j-content`
- `editable-text-block:ref-doc-0-content` (the Page editor)

But no entry of any kind for `block:c2f12z-s`. ref-state expects this block
under `block:ref-doc-0` with properties `{task_state: "WAITING"}`.

The other two children of ref-doc-0 are plain text blocks and render fine.
Only the WAITING-task block is absent.

### Strongest hypothesis (carried over from the original handoff)

Memory note `tui_split_block_cdc_drop.md` and devlog
`2026-05-14-splitblock-handoff.md` flag this exact pattern: "Main render
filters out blocks with `task_state` set (tasks render via tasklist view,
not plain rows)." The c2f12z-s block has `task_state: WAITING` from the
WriteOrgFile step; that property is what differentiates it from its
rendered siblings.

### Cheap things to try first (~10 min total)

1. **grep the render-block / Main-panel render expression for any
   `task_state` filter.** Likely sites:
   - `crates/holon-frontend/src/view_model.rs` (LazyChildren / live_block
     resolution)
   - `frontends/gpui/src/render/builders/live_block.rs`
   - any PRQL/SQL the Main panel's view uses to list block children
     (often `block_with_query_source.sql` or a `focus_roots`-style
     matview chain)

   If a filter excludes task-tagged blocks, decide whether to remove it
   (so tasks render inline like LogSeq) or to have the tasklist view
   pick up the inline rendering too.

2. **MCP probe the live engine while paused.** `PROPTEST_SEED=42 cargo
   test ...` pauses naturally on the assertion; while the process is up,
   the holon MCP exposes:
   - `inspect_loro_blocks` — confirm c2f12z-s exists in Loro tree with
     `task_state: WAITING`.
   - `execute_query` against `block_raw` and the Main view to see whether
     c2f12z-s appears at the SQL layer vs disappears at the render layer.
   - `describe_ui` for the Main panel's resolved ViewModel.

3. **Sanity-check the org file content** that step 1 wrote. The org
   parser may be assigning `task_state` only to the c2f12z-s line; the
   render filter is the prime suspect, but it's worth confirming the
   block makes it into SQL with the expected shape.

## Open tasks

| ID  | Status | Subject |
|-----|--------|---------|
| #14 | done   | Verify hit-test tie-break |
| #15 | done   | Children-settled predicate |
| #17 | done   | OrgMode parser places block before create lands in Loro |
| #16 | open   | Wrapper editable_text swallows synthetic click; move focus handler to entity-bearing child |
| #18 | open   | WAITING-tagged block c2f12z-s doesn't render in Main after NavigateFocus |

Task #18 is the blocker for further seed=42 progress. Task #16 surfaces
again as soon as #18 is fixed and the run reaches step 11 (last known
position in run-4, where click coords were correct but the wrapper ate
the focus event).

## Files touched

```
frontends/gpui/src/user_driver.rs                  # hit-test tie-break
crates/holon-integration-tests/src/pbt/sut.rs      # pre_ref_state + wait_for_children_settled
crates/holon-orgmode/src/org_sync_controller.rs    # propagate-wait + bare-key fix
```

## Logs

- `/tmp/gpui_pbt.log` — original failure baseline (pre-fix).
- `/tmp/gpui_pbt2.log` — hit-test tie-break only; step 4 stale-coord race.
- `/tmp/gpui_pbt4.log` — + children-settled with synthetic-id filter;
  step 11 wrapper-eats-click.
- `/tmp/gpui_pbt5.log` — children-settled refactored to pre-state; bails
  cleanly at step 4 surfacing the OrgMode `ordering.place failed` error.
- `/tmp/gpui_pbt9.log` — final: OrgMode bug fixed; step 4 fails because
  c2f12z-s never renders. **This is the log to start with.**

## Suggested next steps

1. Read the c2f12z-s geometry section of `/tmp/gpui_pbt9.log` (lines
   436–450). Confirm it's missing while its siblings are present.
2. Spend ≤5 min greping for `task_state` filters in the Main render path
   (hypothesis above).
3. If nothing obvious surfaces, run the test with `RUST_LOG=info` and
   trace the live_block resolution for c2f12z-s — every other ref-doc-0
   child appears, so the divergence is local to that one block.
4. Don't touch the SUT-side fixes (#14/#15/#17). They're load-bearing
   for any subsequent seed=42 progress and are the new baseline.
