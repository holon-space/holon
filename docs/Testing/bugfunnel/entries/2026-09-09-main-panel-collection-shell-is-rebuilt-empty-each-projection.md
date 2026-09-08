---
id: 2026-09-09-main-panel-collection-shell-is-rebuilt-empty-each-projection
date: 2026-09-09
gap: PERCEPTION
secondary: ORACLE
status: OPEN
summary: >-
  The main panel's collection `ReactiveShell` is re-created with an EMPTY item
  vec on every projection rebuild and refills only on its next
  `signal_vec_cloned()` tick, so the outline paints zero rows for at least one
  frame after every change — a blank-panel flicker on the production render
  path, measured in the windowed harness but never bounded in a real window.
---

## Bug

Found 2026-09-09 by the `gpui-driver` lane while root-causing the windowed
driver's "entity not in bounds" family — outside any test, by probing the
render path. This entry is the PRODUCTION half; the harness half is
`2026-09-09-windowed-driver-reads-one-frame-for-entity-bounds` (FIXED).

With `HOLON_GPUI_RENDER_PROBE` on `frontends/gpui/src/views/reactive_shell.rs`,
the main panel's collection shell reports

```
list-mode shell=0x777234bc00 items=5 visible=5
  ids=["block:structural-page","block:parent","block:c1","block:c2",
       "block:__virtual:structural-page"]
```

and its `gpui::list` row callback runs for all five rows — the rows ARE built.
Then a *different* shell instance, freshly constructed with an empty `items`,
renders last, and its frame is the one that gets committed
(`lane-logs/run-listprobe2.out:214,330` — distinct `shell=` pointers, the second
at `items=0 visible=0`). The committed frame's bounds table shows
`reactive_shell#42` at 912×1018 with **no descendants at all**
(`lane-logs/run-full-census.out:322-395`).

## Root cause

Not yet fully established — this entry is OPEN, and the naming of the seam is
the useful part.

`get_or_create_reactive_shell` (`frontends/gpui/src/render/builders/mod.rs:364`)
keys the GPUI entity on `CacheKey::ReactiveShell(view.stable_cache_key())`.
Probing it showed ONE key, `ReactiveShell(10148517494279780842)`, resolving
against a succession of different `Arc<ReactiveView>` allocations
(`lane-logs/run-keyprobe.out:212-234`, `view_ptr` `…bdc10` → `…e0390` →
`…ee4a90`), and new shell instances appearing anyway. So either the cache entry
is being evicted between rebuilds, or the key is colliding across distinct
views and the loser gets a fresh entity. Either way, a newly-constructed
`ReactiveShell::new_for_collection` starts with `items: vec![]` and fills only
when its `signal_vec_cloned()` subscription next delivers — which is at least
one frame later.

`stable_cache_key` (`crates/holon-frontend/src/reactive_view.rs:1031`) exists
precisely to survive this ("a brand-new `Arc<ReactiveView>` is created but it
wraps the same underlying `Arc<ReactiveRenderedRows>` … so a downstream consumer
can reuse the same GPUI entity across rebuilds"), so whatever breaks it is
defeating a mechanism already designed for the case.

This is the classic stateful-regrouping hazard from the
`derived-data-contracts` skill: a component holding derived data is rebuilt and
re-snapshots asynchronously, so it publishes an empty intermediate state.

## Missing piece

**PERCEPTION (primary).** A one-frame blank of the outline after every edit is
a visual defect, and no headless assertion can see it — the engine's view model
is correct throughout (`inv-main-panel-rows-match-focus` reads `SutRenderer`,
not painted bounds, and was green while the window painted nothing).

**ORACLE (secondary).** The one invariant shaped for this — `assert_content_fidelity`
— exempts exactly this frame; that exemption has its own entry,
`2026-09-09-content-fidelity-exempts-a-shell-with-no-descendants`.

## Keystone repro

The headless keystone has no frames and no virtualized list, so it structurally
cannot reach this. A windowed repro needs an oracle that samples EVERY committed
frame between two gestures, not the settled frame — no such oracle exists in the
suite today, and the `gpui-driver` fix makes the transient case *less*
observable, not more (see Remedy).

The PERSISTENT variant of the same render gap IS covered and IS red: register
row 10, `an_opened_nested_page_paints_its_children`
(`frontends/gpui/tests/nested_page_chevron_gate.rs:753`), where the gate opens
onto nothing and stays that way. Family D in
`docs/Testing/GpuiCrateReds-2026-09-10.md`.

## Remedy

NOT FIXED. Queued as the `panel-blank-frame` lane.

Two things a fix lane must know:

1. **The measurement is windowed-harness-only.** How long the blank lasts in a
   real window at real frame rates is unmeasured. It may be imperceptible; it
   may be the flicker behind other reports. Measure before designing.
2. **The windowed driver no longer trips over it.** `gpui-driver` made every
   bounds-taking verb wait for a frame that paints the entity
   (`BOUNDS_WAIT_PUMP_CYCLES`, `frontends/gpui/tests/pbt_harness/sim_windowed_replay.rs`).
   That was the right call for the driver — a driver that samples an arbitrary
   frame is not the gesture seam it stands in for — but it means the transient
   blank is now **unobservable to every windowed oracle**: the gestures wait it
   out silently and the one invariant shaped to catch an empty shell exempts the
   empty case. A fix lane cannot use the windowed PBT suite as its red; it needs
   a per-frame probe, or the oracle from the sibling entry.
