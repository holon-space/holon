# GPUI PBT seed=42 follow-up — failure A fixed, B+C narrowed to one root cause

Continues `devlog/2026-05-14-081443.md` and `devlog/2026-05-14-splitblock-handoff.md`.

## Status

Reproducer: `PROPTEST_SEED=42 cargo test -p holon-gpui --test gpui_ui_pbt --features pbt`.

| Original failure | State |
|---|---|
| A: `sut.rs:800` NavigateFocus `click_entity` errored (sidebar bounds missing) | **Fixed deterministically.** |
| B: `sut.rs:2200` SplitBlock `wait_for_entity_bounds` timeout 5s for `block:c2f12z-s` | Still flakes. |
| C: `sut.rs:2219` SplitBlock click did not move focus to target | Same class as B — manifests now as either bounds-timeout (B) *or* post-split focus-not-on-new-block (a sharper variant, see below). |

3 consecutive seed=42 runs after this work:

```
Run 1: panic at sut.rs:2209 (was 2200)  — bounds unavailable for block:c2f12z-s after 5s
Run 2: panic at sut.rs:6184            — inv-focus-matches-ref: engine stuck on c2f12z-s, ref on new block (polled 1s)
Run 3: panic at sut.rs:2209            — bounds unavailable for block:c2f12z-s after 5s
```

So the remaining surface is two failure modes, both rooted in the GPUI render/focus pipeline after a write changes which blocks should be on screen.

## What changed this session

### 1. `apply_navigate_focus` now waits for sidebar bounds (sut.rs:~795)

The other input-bearing transitions (`ClickBlock`, `SplitBlock`) called `wait_for_entity_bounds` before `click_entity`. `NavigateFocus` didn't. Sidebar entries from a freshly-loaded layout sometimes hadn't promoted staged → committed in `BoundsRegistry`. Mirrored the `ClickBlock` pattern; failure A is gone in 3/3 runs.

### 2. SplitBlock ref model now sets `state.focused_block = Some(new_block_id)` (`transitions/split_block.rs:112`)

`apply_to_ref` already updated `focused_entity_id[Main]` to the new block. But the `inv-focus-matches-ref` invariant compares against `state.focused_block` (the engine-global mirror), which SplitBlock left stale. Production's expected end state is "focus on new block" (via editor_focus follow-up → editor_cursor → watch_editor_cursor → window.focus → InputEvent::Focus → `set_focus`). The ref model now mirrors that.

### 3. `apply_split_block` does a best-effort wait for engine focus to move (`sut.rs:~2275`)

After Enter + `wait_for_blocks_synced` + `map_unmapped_split_synthetic_ids`, derive the new block's real id from `db_rows - pre_known`, then `wait_for_focus_to_match(new_id, 2s)`. Soft (`let _ =`) — the propagation chain is genuinely flaky and the downstream invariant should catch real regressions.

### 4. `inv-focus-matches-ref` polls for 1s instead of asserting instantly (`sut.rs:~6162`)

Same rationale — absorb GPUI render/focus-loop latency.

## The bug that's still there (failure C → new variant)

Run 2 above: ref state `focused_block = block::split-0` (resolved to `block:26525fde-…`), engine `focused_block = block:c2f12z-s` (original split target), no movement after 1s of polling.

The chain that should fire:
1. `split_block` op completes — creates new block, follow-up `editor_focus_op(new_block)` writes `editor_cursor`.
2. SQL signal fires → each `EditorView`'s `watch_editor_cursor` subscriber (set up in `editor_view.rs:322`) receives `(block_id, cursor_offset)`.
3. The new block's `EditorView` filters on `row_id == block_id`, calls `window.focus(input.focus_handle(cx))`.
4. `InputEvent::Focus` fires on that input → `services.set_focus(new_block)` → engine mirror updates.

The chain misses if step 3's `EditorView` hasn't mounted yet at step 2 — the signal stream is fire-and-forget, no replay. The new block exists in SQL but its render entity / editor view's spawned task hasn't subscribed.

Empirically, after a split the new EditorView mounts within a render pass, but the editor_cursor write happens inside the same op-dispatch tick — so there's a real race between "new EditorView's `cx.spawn(...)` registers" and "watch_editor_cursor stream emits."

### Fix options

- **Cheapest:** keep some retained-value semantics on `watch_editor_cursor` so new subscribers get the latest emission. (Loro/Cell-style "current value" pattern.) Subscribers that don't care can filter.
- **Structural:** drive focus from the new EditorView's *mount* side — when an EditorView mounts and its row_id matches `editor_cursor[region=main].block_id`, grab focus.
- **Test-only band-aid:** in `apply_split_block`, after wait_for_blocks_synced, scroll/render-poll to ensure the new block's EditorView has mounted before checking focus.

I'd pick the structural option — it's the same idea as `render_entity_view.rs:118`'s synchronous `services.focused_block().as_ref() == Some(id)` mirror lookup at render-time, applied to editor_cursor.

## Failure B: `wait_for_entity_bounds` for `block:c2f12z-s` times out at 5s

This is the larger half of remaining flakes. The block exists in SQL (otherwise SplitBlock generator wouldn't pick it), but `BoundsRegistry::find_by_entity_id` / `element_info("render-entity-…")` / `element_info("selectable-…")` all miss for 5s with one scroll attempt.

Suspects (in order):
1. The block IS rendered but under an element id not in the wait_for_entity_bounds's tried list. Confirm by reading `BoundsRegistry::all()` after the timeout (e.g., via `PBT_PAUSE_SECONDS=15` then the embedded MCP at port 8528).
2. The block is rendered staged but never committed. The promotion fires on `BoundsRegistry::flush` — check whether the render path completed a full prepaint pass between the SplitBlock generator picking c2f12z-s and the wait.
3. The block is genuinely not yet rendered — virtualized list off-screen. The scroll attempt at 200ms inside `wait_for_entity_bounds` is best-effort and silently no-ops if the entity isn't in any virtualized list.

Next step: add a one-shot `BoundsRegistry` snapshot dump at the timeout point in `wait_for_entity_bounds`. If c2f12z-s is in there under a different id, fix the lookup. If absent, look at the render side.

## Files touched

- `crates/holon-integration-tests/src/pbt/sut.rs` — NavigateFocus wait, SplitBlock new-block focus wait, inv-focus-matches-ref polling, `wait_for_entity_bounds` now dumps BoundsRegistry on timeout.
- `crates/holon-integration-tests/src/pbt/transitions/split_block.rs` — `apply_to_ref` updates `focused_block`.
- `frontends/gpui/src/lib.rs` — top-level `editor_cursor → set_focus` bridge (production fix; see below).

## Update — failure C structural fix LANDED

Confirmed the chicken-and-egg via the BoundsRegistry dump: in failing runs the new block was rendered as `rendered_text`, NOT `editable_text`, and had no `EditorView` — therefore no `watch_editor_cursor` subscriber, therefore the `editor_focus` follow-up's signal hit no listener.

The fix in `frontends/gpui/src/lib.rs` adds a top-level `watch_editor_cursor` subscription right after the `root_signal` spawn. It mirrors `editor_cursor.block_id` into `services.set_focus(...)`, breaking the cycle:

```
editor_cursor write → top-level bridge → set_focus(new)
   → re-render → render_entity_view(new) renders editable
   → EditorView mounts → its own watch_editor_cursor replay (Mutable retains)
   → window.focus(new_input) → InputEvent::Focus → set_focus (no-op).
```

5 consecutive seed=42 runs post-fix:

| Run | Outcome |
|---|---|
| 1 | sut.rs:2228 — click_entity landed on `block:-q…-5` instead of `block:c2f12z-s` (variant C: wrong-element click, *not* the deadlock) |
| 2 | sut.rs:2209 — bounds timeout for `c2f12z-s` (failure B) |
| 3 | sut.rs:2209 — bounds timeout for `c2f12z-s` (failure B) |
| 4 | sut.rs:6347 — inv-displayed-text (further along; new) |
| 5 | assertions.rs:60 — org roundtrip parent_id mismatch (`file:index.org` vs `block:ref-doc-0`) on step 7 (further along; new) |

The deterministic post-SplitBlock focus deadlock is gone in 5/5 runs. Remaining failures are pre-existing, distinct.

## What's still flaky and what to do next

1. **Failure B (`c2f12z-s` bounds never appear, 2/5)** — still the biggest mystery. `c2f12z-s` is a pre-existing org block expected to be in Main after `NavigateFocus(ref-doc-0)`. The BoundsRegistry shows 79 elements at timeout, none mention `c2f12z-s`. Suspect: Main panel hasn't finished rendering its content blocks by the time SplitBlock generator picks one. Next step: dump panel content elements (not just ones mentioning the failing id) to see what Main actually rendered — likely the wrong document is loaded, or load is racing.

2. **Variant C: click landed on `block:-q…-5` not `block:c2f12z-s` (1/5)** — the click coordinates resolved a *different* block than `wait_for_entity_bounds` confirmed bounds for. Either the BoundsRegistry stored stale bounds for c2f12z-s pointing at -q…-5's coords, or there's overlap. Check ElementInfo for c2f12z-s vs -q…-5 in the failing run.

3. **inv-displayed-text and org-roundtrip (1/5 each)** — pre-existing further-along failures, now reachable because we cleared the earlier deadlock. Worth investigating separately; mention in their own devlogs.

The right next session takes failure B first — its mystery is the smallest remaining contained scope, and once Main rendering is reliable, variant C likely becomes diagnosable.

## Recommended next session

Pick **failure B** first (it's the smaller, more contained mystery — and once Bounds works reliably, failure C may become tractable to repro live with the embedded MCP). Use `PBT_PAUSE_SECONDS=20 PROPTEST_SEED=42 cargo test -p holon-gpui --test gpui_ui_pbt --features pbt`, then attach via `holon-live` MCP at port 8528 to inspect `BoundsRegistry` contents at the moment of timeout.

Don't reach for the editor_focus / watch_editor_cursor structural fix until the bounds story is sorted — same fragility may bite the new EditorView's mount path.
