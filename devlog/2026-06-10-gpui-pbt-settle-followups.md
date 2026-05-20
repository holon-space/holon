# gpui PBT reactive-settle follow-ups

Date: 2026-06-10. Implements the prioritized follow-up list from the
reactive-settle handoff (`devlog/2026-06-10-gpui-pbt-reactive-settle.md`).
Worktree `gpui-pbt-speed`; the reactive-settle work itself (plus the
no-Loro-slices work) was absorbed into main by concurrent sessions before
this session started.

## 1. Harness trust — exit-0 trap FIXED

The off-signature-first-failure swallowing was already fixed by the
pbt-no-loro-slices session (`pbt_harness/random_pbt.rs`: the FIRST failure
always fails the run). The remaining hole was the **exit-0 trap**: on macOS
`cx.quit()` terminates the process (NSApp terminate → exit 0) before
`WindowHost::run_window`'s post-`app.run` `bg_handle.join()` re-raise ever
runs. Fix (`pbt_harness/windowed_replay.rs`): the bg thread stashes its
panic message in a shared `bg_failure: Arc<Mutex<Option<String>>>` BEFORE
setting `done`; the rebind loop's quit branch reads it and
`std::process::exit(101)` on failure instead of a plain quit. The
"grep for `all N case(s) passed`" workaround is obsolete — non-zero exit is
now trustworthy. Benefits `gpui_windowed_minimize` too (same host).

## 2. inv-window-focus-matches-engine-focus (NEW invariant)

`RenderedElement` gained `focused: Option<bool>` (populated from
`ElementInfo.focused`, recorded per committed frame by the `editable_text`
builder). New body
`invariants/bodies/window_focus_matches_engine_focus.rs`: SUT-internal
coherence between engine `focused_block` (synchronous authority) and the
committed frame's window-focused editor (follows via spawned binding).
Polled 1s (lag tolerated, settled divergence fails). Directions:
- window-focused editor ≠ engine focus (or engine unfocused) → Fail
  (zombie editor / steal-back);
- engine focused + that block's `editable_text` mounted but NOT
  window-focused → Fail (lost handoff); editor not mounted at all → Skip
  (sidebar/nav focus is legitimate).
Registered `[EditorState, FrontendBounds]`, Strict, `ProperlySetup` gate,
dispatched in the `native_self_invariants` table (needs `SutDriver`).
Headless slices skip it structurally (no FrontendBounds).

## 3. Zombie-editor prod fix — blur on focus-authority departure

`views/editor_view.rs::spawn_focus_binding` previously did nothing when
focus LEFT a row (`if !focused { continue; }`): after a join/split moved
the authority, the old editor kept WINDOW focus until the new editor
mounted+grabbed (≥1 frame), and a fast keystroke was consumed by the stale
editor — mutating the WRONG block. Now the binding's `false` arm blurs the
window iff this editor still holds window focus (guard makes the
A-blur-after-B-grab ordering race harmless: A's handle is no longer
focused, so no blur). A keystroke in the gap is now *dropped*, not
*misdelivered*; the driver's per-keystroke window-focus waits
(sut_capabilities) already retry. The new invariant (#2) pins this
structurally.

## 4. HOLON_PBT_SUPERHUMAN_INPUT=1

Disables the one-committed-frame keystroke pacing in
`GpuiUserDriver::send_raw_keystroke` — stress mode to hunt the
dropped-character editor-echo race as a prod bug. Not part of the green
gate.

## 5. TUI GeometryProvider::changed()

`TuiGeometry` now carries an `install_notify: Arc<Notify>`; `install()`
(called by the renderer per render pass) wakes `changed()` waiters.
`TuiState.last_registry` changed type `Arc<Mutex<RenderRegistry>>` →
`TuiGeometry` so the per-frame write goes through `install()` (readers use
the new `TuiGeometry::lock()`; `shared()` still hands the raw Arc to the
driver/input-pump). The windowed minimizer's `wait_for_paint_quiescence`
was deliberately NOT converted: its cost is a 240ms *stability window*
(cursor-blink frames keep committing, so commit-wakes fire constantly);
the 30ms poll adds ≤30ms per rebind.

## 6. HOLON_PBT_STEP_BUDGET_MS — per-transition CI budget

`stepper::StepTimingAgg` aggregates (apply+check) ms per applied
transition (StartApp excluded) in both replay loops (`run_sequence`,
`fixtures::replay_steps`) and on finish prints the mean and panics if it
exceeds `HOLON_PBT_STEP_BUDGET_MS`. Deviation from the handoff: a
dedicated env var, not the boolean `HOLON_PERF_BUDGET` — the threshold is
per-runner (headless ~230ms/txn → gpui jobs set ≈350 for the 1.5× ratio).

## 7. justfile

`frontends/gpui/justfile`: `just pbt [target] [steps] [cases]` and
`just pbt-no-loro` — ONE `cargo test --test … --features pbt` invocation
from `frontends/gpui`, so cargo runs exactly the binary it built
(stale-binary trap structurally dead). Combined with #1, plain exit-code
checking works.

## 9. Smaller

- `phased.rs`: both 200ms post-sync sleeps deleted
  (`wait_for_blocks_synced` is the barrier; invariants poll internally).
- `[SplitBlock-presplit]` per-split SQL probe now opt-in via
  `HOLON_PBT_SPLIT_PROBE=1` (failure-path probes stay unconditional).
- `apply_navigate_focus` re-click loop wakes on `geometry.changed()`
  (50ms cap) instead of a fixed 50ms sleep.

## 8. Pre-existing bugs (status)

- **8a** Indent → `inv-blocks-match-ref/org` divergence under no-Loro:
  still open, warn-gated; regression entry 1 reproduces.
- **8b** whitespace-only (`"\t"`) blocks never paint a text widget →
  `wait_for_children_settled` burns 5s: still open (needs a prod-or-model
  decision: should a tab-only block render an editable row?).
- **8c** ApplyMutation chord click-focus: investigated — NO ApplyMutation
  op (create/set_field/update/delete/move) has an engine keybinding, so
  the chord path is unreachable today. Replaced the silent dispatch with a
  loud panic instructing to add the `model_chord_click_focus` ref mirror
  before enabling chord dispatch for such an op (the old dispatch code is
  in git history).

## Mid-session main churn + iroh-sync unbreak

Concurrent sessions absorbed parts of this work (and the reactive-settle
work) into main `smqlpyom` while the session ran, twice rewriting the
worktree's base. New main carried a HALF-LANDED Loro history-compaction
change that broke `crates/holon` under its default `iroh-sync` feature:
`export_delta_or_full_snapshot` was called but never defined, and
`LoroDocumentStore` derived `Clone` over a new non-Clone `AtomicU64` field.
Unbroken here following the change's own comments: the helper exports
`ExportMode::updates(peer_vv)` and falls back (warn!-disclosed) to a full
snapshot when the peer is behind compacted history; `save_counter` became
`Arc<AtomicU64>` (clones share one compaction schedule). Whoever owns the
compaction thread should double-check the accept-side export
(`sync_doc_handle_connection` still uses a bare `updates` export) and add
the stale-peer test.

## New finding (fail-loud now working)

A fresh random no-Loro case failed REAL (exit 101 — the new exit path
works end-to-end): `[SplitBlock] bounds unavailable` for an empty
split-created block (`block::split-14`) that HAD rendered earlier
(has_content=false warnings) and then vanished from the committed
BoundsRegistry; 5s timeout. Same family as 8b (empty/whitespace block
paint). Deterministic repro persisted by proptest into
`tests/gpui_ui_pbt_no_loro.proptest-regressions` (left in place — the gate
is red-by-design until the rendered-set bug is fixed; capture also at
`tests/.captures/gpui_ui_pbt_no_loro.captured.json`). Pre-dates this
session's changes (the binary that failed contained only the exit-fix,
invariant, and blur changes — none of which write to render state).

## Investigation: the gate-red missing-row bug (2026-06-11 session continuation)

Layer-probe instrumentation (now permanent in the failure paths) pinned the
headline manifestation precisely: at SplitBlock-bounds-timeout the target
block is present in `block_raw`, in the `block` matview, in the MAIN-PANEL
WATCH's data rows (25/25), and in the `HeadlessLiveTree` items (25/25) —
every data/watch/VM layer is correct; only the PAINTED window lacks the row
(296 registry elements, none for the target, post scroll-to-reveal). The
drop is paint-side in the virtualized main panel.

The failure point is nondeterministic across replays of the same persisted
seed — observed faces: (a) bounds timeout for an off-viewport row, (b)
`[SplitBlock] Block count mismatch` (a split's CREATE missing from
block_raw at the post-split check), (c) click-to-focus editable variant
never taking window focus (`observed focused states: []`), (d)
children-settled timeout. One underlying churn-sensitive render stall,
several symptoms.

Causality tests (all on the correct fresh binaries — see the stale-binary
postscript):
- **Blur fix exonerated**: 3/3 runs fail identically with
  `HOLON_GPUI_BLUR_ON_FOCUS_LEAVE=0`. Restored to default-ON.
- **New main exonerated**: the HANDOFF-ERA binary (old main, pre
  block-sync-refactor, none of this session's changes) fails 3/3 with the
  same failure family — and exits 0, which is precisely why the handoff
  believed the gpui gates were green ×4. The bug is PRE-EXISTING; the
  exit-code fix made it visible.

Also found and instrumented: `GpuiUserDriver::scroll_to_entity` discards
`handled=false` (`let _ =`), and `scroll_entity_into_view` silently
`continue`s out of three legs (now eprintln'd). In the instrumented run the
target's scroll never logged a bail, so scroll dispatch is not (alone) the
culprit.

Next step (needs an interactive session): replay the persisted seed with
`PBT_PAUSE_SECONDS` + the embedded MCP (`describe_ui`, port 8528) or
debugger breakpoints in the tree driver / `ReactiveShell` VecDiff
application, paused at the stall, to see why the panel stops materializing
rows under split churn.

## Validation

Registry guard tests 31/31. Forced-failure run (`HOLON_PBT_FORCE_FAIL_AT_STEP=2`)
exits 101 with `[windowed-replay] bg thread failed:` — exit-0 trap dead. A
real failure (above) also exited 101. The no-Loro run passed its persisted
regression entries + 33 random steps with the new window-focus invariant
Strict before hitting the new finding. NOTE the tee/pipe exit-code trap bit
twice this session — always capture `$?` of the build/test itself.
