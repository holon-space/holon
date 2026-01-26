# Handoff — layout_pbt drawer-toggle dump divergence (shared-Arc state leak class)

**Status: RESOLVED 2026-05-18.** Diagnosis below was wrong — the bug had
nothing to do with a shared `Arc<ReactiveView>` or with services leaking
between mounts. Real cause: **layout PBT generators emitted scenarios with
multiple overlay drawers anchored to the same edge of a `columns`, or with
an overlay-drawer nested directly inside another overlay-drawer.** In both
shapes the toggle hitboxes render at identical screen coordinates, so one
synthesized mouse-down at the toggle's center hits *every* stacked toggle's
hitbox and GPUI correctly dispatches `on_mouse_down` to all of them — N
drawers flip per intended click, end-state dump diverges from the reference.

Verified by three eprintln probes (mount/click/handler) showing one click
firing three drawer handlers, all with identical `registered_bounds=
(0.0,0.0,12.0,600.0)` and identical services pointer. Fix:
`crates/holon-layout-testing/src/generators.rs`: new
`arb_wrapped_collection_no_drawer()`; `arb_columns_with_sidebars` and
`arb_columns_with_overlay_sidebars` use it for the L/M/R slots so a
drawer-inside-drawer (and same-edge sibling overlays) cannot be generated.

After the fix, proptest surfaces a *different* failure class
(`SwitchViewMode` end-state divergence on a `view_mode_switcher >
live_block` with no drawers anywhere) — track separately, not the same
bug as this handoff. The "Shared `ReactiveView`" / `walk_and_reset_widget_state`
direction from the H1 section below would have been wasted effort.

---

Original handoff (preserved for context):

**Status:** open. Worktree `toggle-collapse-transition`. Layout PBT: 5/6 cases pass; one shrinks to a `ToggleDrawer` action and fails end-state equivalence.

**Not caused by this worktree.** Same failure class existed pre-refactor; the SceneState refactor (this worktree) just makes it harder to hide. Surfaces in any oracle that compares "reference mount with state pre-applied" vs "test mount with state replayed via real clicks."

## Symptom

```
=== minimal failing input ===
Scenario: columns(drawer(_,overlay,260px,list[1]),
                  drawer(_,overlay,260px,outline[5]),
                  drawer(_,overlay,260px,drawer(_,overlay,260px,list[54])))
Handles: 0
Drawers: 4 (incl. one nested)
Actions: 1
  0: ToggleDrawer(ToggleDrawer { block_id: "pbt-drawer-20" })
```

`pbt-drawer-20` is the **inner** drawer of the nested pair (`drawer-22 > drawer-20 > list[54]`). All drawers default open; reference replays `SceneState::replay` → `state.drawer_open["pbt-drawer-20"] = false` → reference pre-applies via `state.closed_drawers()`. Test starts default-open, then clicks the toggle, ending closed.

Both should converge to the same final dump. They don't.

## What the dumps show

Reference renders the **closed** state of the inner drawer (only its `drawer_toggle 12x600` survives — the production "shrunk" path renders just the click target). Test renders the **open** state, with content (`live_block` / `selectable` chain visible).

The diff is "did the toggle happen at all" — not a layout shift, not a measure issue. The toggle action's effect on the test render is invisible to the dump comparison.

## Root cause hypotheses, ranked

### H1 (most likely) — Shared `ReactiveView` Arc, same as the collapse-state leak

The blueprint thunk captures `Arc<ReactiveView>` once:
```rust
let shape = Shape(Arc::new(move || ReactiveViewModel {
    collection: Some(view.clone()),  // ← same view across all materialize() calls
    ..
}));
```
Reference Mount: `services.drawer_states["pbt-drawer-20"] = false` (fresh `TestServices`). Test Mount: fresh `TestServices` again (so drawer state itself doesn't leak). **But** the rendered drawer reads `services.widget_state(block_id).open` via `BuilderServices` — and the `ReactiveView` is shared, so its captured services reference might be the reference's. Worth dumping `Arc::strong_count(&view)` and the drawer-builder's `services.clone()` site to confirm whether the test's `services` is the one being read on click.

If confirmed, fix is the same shape as the collapse fix: walk the rendered tree on every Mount and reset drawer-related widget state to defaults, then re-apply per-Mount overrides. Or rebuild the `ReactiveView` per materialize.

### H2 — Click coordinate misses the inner drawer

`drawer_toggle_id_for("pbt-drawer-20")` resolves to the bounds registry entry, and `click_at_element` centres on those bounds. But the inner drawer is overlay-mode and nested inside another overlay drawer; if the **outer** drawer renders on top, the click hits the outer drawer's toggle area instead. Test path runs through `mouse-down/mouse-up` simulation — hit-testing applies. Quick check: log which element ID the synthesized event actually lands on; if it's `pbt-drawer-22` not `-20`, that's it.

### H3 — Render-pass order doesn't propagate the click before snapshot

`apply_action` calls `click_at_element` then `self.settle(cx)`. If the production drawer-state update goes through an async path (channel send to a watcher, deferred `cx.notify`), `settle` might return before the next render incorporates the new state. Inspect `services.set_widget_open` for any task spawning.

## Where to look

- `frontends/gpui/tests/support/mod.rs`:
  - `GpuiInteractionSession::click_at_element` (line ~813) — `info.center()` may be picking up the wrong element when bounds overlap.
  - `GpuiScenarioSession::open` (line ~610) — services fresh per Mount; drawer pre-apply at line ~634.
  - `walk_and_reset_tree_items` / `pre_set_collapsed_rows` (added in this worktree) — drawer state has no analogue here; if H1 is right, add one.
- `frontends/gpui/src/render/builders/drawer.rs`:31 — `on_mouse_down` body calls `services.set_widget_open`. Check whether `services` here is the closure's captured reference or shared via `ReactiveView`.
- `crates/holon-layout-testing/src/scene_state.rs`:80-110 — `apply()` for `ToggleDrawer` toggles `drawer_open` correctly; that's not the bug.

## Minimal repro

```bash
cargo test --test layout_pbt -p holon-gpui layout_invariants_hold_for_random_scenarios
```

Proptest re-shrinks deterministically; the shrunk case in `.proptest-regressions` is the inner-drawer-toggle scenario above. Add a `dbg!` in `drawer.rs:47` `on_mouse_down` to confirm whether the click handler fires on the inner drawer at all when the test mount runs.

## Why this matters

The SceneState refactor (this worktree) made the oracle stricter: it now demands reference and test mounts converge on **every** UI dimension, not just modes. Drawer-toggle dump equivalence was failing under the old oracle too (intermittent shrinks landed on `ToggleDrawer` cases pre-refactor — see proptest-regressions history). The refactor doesn't add the bug, it stops masking it.

Fixing this unlocks the cleanest possible bug-detection signal for any future UI variant: "reference dump == test dump" with no per-variant escape hatches.

## What's already known good

- `ToggleCollapse` end-to-end (via this worktree's SceneState integration): converges. Reference and test dumps match. Confirms the SceneState + `pre_set_collapsed_rows` + `reset_collapsibles_to_default` cycle works for the collapse dimension.
- `SwitchViewMode` + `DeliverBlockContent`: handled via `block_registrations_with_overrides(&state.active_modes)` at Mount time; fresh `BlockTreeRegistry` per Mount, no Arc-sharing path. Passing.
- All 5 non-drawer-toggle cases in this run.

## Suggested next steps (priority order)

1. **Verify H1**: add `eprintln!` in `drawer.rs:on_mouse_down` printing the `Arc::as_ptr(&services)` of the services it's about to call `set_widget_open` on. Compare against the services pointer the test's Mount installed. If they differ → H1 confirmed.
2. **If H1**: apply the collapse-state fix pattern — add `walk_and_reset_widget_state` to reset every drawer's `services.widget_state(id).open = true` on Mount before re-applying overrides. Centralise in `open()` next to `reset_collapsibles_to_default`.
3. **If H1 rejected**: instrument `click_at_element` to log the element id that actually receives the hit (post-hit-test). Verify it matches `pbt-drawer-20`.
4. **If both rejected**: instrument `apply_action` to dump `services.widget_state("pbt-drawer-20").open` before `settle()`, after `settle()`, and at snapshot time.

## Don't regress

- `ToggleCollapse` already works. Don't unify drawer-reset into the same walk without verifying tree_items aren't accidentally touched.
- `SceneState::apply` is the single source of truth for "what each action mutates"; don't add parallel folds.
- `EmptyRef` was deleted intentionally — `SceneState::default()` is the replacement.
