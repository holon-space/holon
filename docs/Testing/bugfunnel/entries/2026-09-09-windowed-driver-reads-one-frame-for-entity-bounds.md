---
id: 2026-09-09-windowed-driver-reads-one-frame-for-entity-bounds
date: 2026-09-09
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
summary: >-
  The windowed PBT driver resolved an entity's click point from ONE committed
  frame, while the main panel's virtualized `ReactiveShell` is re-created with
  an empty item vec on every projection rebuild and fills only on the next
  signal tick — so a gesture aimed at a row that is on screen a frame later
  failed with `entity block:c1 not in bounds`, and six of the sixteen
  deterministic `holon-gpui` reds were that one read.
---

## Bug

Found 2026-09-09 by the `gpui-driver` lane running the `GpuiCrateReds-2026-09-10`
register's Family-A discriminating check — outside any test, by instrumenting
the driver's own failure path. Not a new red: the six rows have failed since
before `830d794f878f`, unattributed because `holon-gpui` runs in no gate.

Every Family-A row panicked with the same text from the shared PBT driver:

```
[SplitBlock/keystroke] focus block:c1 failed: entity block:c1 not in bounds
[ClickBlock] click_entity failed for block:c1: entity block:c1 not in bounds
```

`block:c1` is a seeded working-tree block the window is supposed to be painting.

## Root cause

The register's stated hypothesis — "the driver reads a bounds map the compose
SUT populates one frame later" — is half right and its discriminating check
settled it the other way. The census this lane added to the bail shows the
window painting 73 elements, engine focus correctly on `block:structural-page`,
the main panel correctly mounted on that page (its `view_mode_switcher` carries
`block:structural-page::{tree,table,board}_view`) — and NOTHING beneath the
panel's `reactive_shell`. So the census lacks the block: a render gap, not a
registration race.

`HOLON_GPUI_RENDER_PROBE` on the shell (`frontends/gpui/src/views/reactive_shell.rs`)
localised it. The main panel's collection shell reports
`items=5 visible=5 ids=["block:structural-page", "block:parent", "block:c1",
"block:c2", "block:__virtual:structural-page"]` and its `gpui::list` row
callback runs for all five rows — the rows ARE built. But a *different* shell
instance, freshly constructed with an empty `items`, renders last and is the one
whose frame gets committed. Probing `get_or_create_reactive_shell`
(`frontends/gpui/src/render/builders/mod.rs`) showed the same
`CacheKey::ReactiveShell(10148517494279780842)` resolving against successive
`Arc<ReactiveView>` allocations, so the panel's shell is re-created across
projection rebuilds and each new one starts empty, filling only when its
`signal_vec_cloned()` subscription next delivers.

The driver's side is the half that made this fatal.
`SimUserDriver::click_entity` (and `send_key_chord`'s click-to-focus,
`set_block_expanded`, `drop_entity`) resolved the click point with ONE
`text_center` / `bounds_center_f32` read and bailed if it came up empty. Every
other verb on that driver already retries across frames —
`dispatch_keystroke_until_handled` pumps a full cycle between attempts and fails
loud only at a deadline, precisely because "the editor takes window focus on a
later render pass". The bounds read was the odd one out, so it sampled whichever
frame the empty rebuild happened to leave committed.

Proof of attribution: adding an `eprintln!` inside the list row callback — pure
timing, no logic — moved `windowed_split_then_clickblock_resolves_minted_id`
off the bounds bail and onto its NEXT precondition
(`lane-logs/run-keyprobe.out:472`).

## Missing piece

**ENVIRONMENT (primary).** The driver's timing model is not the gesture seam it
stands in for. A user clicks a frame they can see; this driver clicked whatever
frame was committed when it happened to look, and a blank-for-one-frame panel is
a state a real click cannot land in. The divergence is invisible headless (no
frames) and exists only on the windowed rung, which no gate runs.

**ORACLE (secondary).** `assert_content_fidelity`
(`crates/holon-layout-testing/src/invariants.rs:203`) is the invariant that
should have named this shape, and it is gated on `total_descendants > 0`: a
`reactive_shell` laid out at 912×1018 with NO descendants at all — exactly the
frame here — is exempt. The composed `inv-main-panel-rows-match-focus` reads
the engine's `SutRenderer` view model, not the window's painted bounds, so it
was green throughout while the window painted nothing. Nothing in the suite
asserts "a panel whose focus root has rows painted at least one of them".

## Keystone repro

The headless keystone cannot reach this: it has no frames, so there is no
committed-frame sampling and no virtualized list. This is a windowed-rung
defect, and the covering tests below ARE the windowed keystone
(`general_e2e_composed_pbt_windowed` and the composed windowed SUT tests).

## Remedy

FIXED in the `gpui-driver` lane, at the driver boundary rather than per test.
`SimUserDriver::click_point_when_painted`
(`frontends/gpui/tests/pbt_harness/sim_windowed_replay.rs`) pumps a full frame
cycle until a frame paints the entity, and fails loud after
`BOUNDS_WAIT_PUMP_CYCLES` (8) with a `miss_census` naming what the window DID
paint — element count, engine focus, the painted entity set, and every element
bound to the missing id with its clipped/visible state.

The budget is counted in PUMP CYCLES, not wall time, and that is load-bearing:
`pump_cycle` advances the app's FAKE CLOCK 500ms per iteration, so a wall-clock
deadline is a clock-injection rate — raising it changes what the tests under it
simulate, not just how long they wait. A first revision used 5s and pushed
`arrow_walk_keeps_focus_on_the_reference_outline_neighbour` from a 52s serial
baseline into a 120s TIMEOUT; the cycle budget removes the coupling.

All FIVE bounds-taking verbs share that one contract — `click_entity`,
`send_key_chord` click-to-focus, `set_block_expanded`, `drop_entity`
(source+target) and `scroll_entity`. `scroll_entity` was the worst of them: it
read one frame and returned `Ok(())` on a miss, so a scroll aimed at a blank
frame became a silent no-op (`CLAUDE.md`: never swallow errors). It now fails
loud like the rest.

**This fix makes a TRANSIENT blank panel unobservable to every windowed
oracle.** The gestures now wait it out silently, and the one invariant shaped to
catch an empty shell exempts the empty case. That is the right trade for the
driver — a driver that samples an arbitrary frame is not the gesture seam it
stands in for — but it means the production defect underneath is no longer
visible from this suite. It is tracked separately and OPEN:
`2026-09-09-main-panel-collection-shell-is-rebuilt-empty-each-projection` and
`2026-09-09-content-fidelity-exempts-a-shell-with-no-descendants`. The
PERSISTENT variant stays red and attributable as register row 10,
`an_opened_nested_page_paints_its_children`.

Covering tests, red before / green after (red log
`lane-logs/discriminate-a-92095.log`, green `lane-logs/gate-nine-11537.log`):

- `holon-gpui::gpui_compose_sut_windowed windowed_composed_sut_drives_a_click_gesture_sequence_green`
- `holon-gpui::gpui_compose_sut_windowed windowed_composed_sut_replays_a_fixture_via_replay_steps_green`
- `holon-gpui::gpui_composed_windowed_loop benchmark_windowed_per_case_boot_cost`

Still OPEN, and deliberately not fixed here because each is an independent
cause the same rows also hit:

1. **The windowed rung has no editable-surface projection.** Register rows 1, 3
   and 4 now fail past the bounds gate at
   `crates/holon-integration-tests/src/pbt/op_write_cap.rs:381` with `editable
   surface not projected by this driver` — the `UserDriver::surface_chars_before_content`
   trait DEFAULT (`crates/holon-frontend/src/user_driver.rs:299`). Only the
   headless `ReactiveEngineDriver` implements it, via
   `HeadlessEditorMirror::content_offset_to_surface`; neither `SimUserDriver`
   nor the production `GpuiUserDriver` does. So windowed `SplitBlock` caret
   placement has never worked, which is a feature-sized gap of its own.
2. **The main panel blanks for at least one frame on every projection rebuild**,
   and **`assert_content_fidelity` exempts exactly that frame.** Both are
   production/oracle defects, not harness ones, so each has its own OPEN entry —
   `2026-09-09-main-panel-collection-shell-is-rebuilt-empty-each-projection` and
   `2026-09-09-content-fidelity-exempts-a-shell-with-no-descendants` — where the
   queued `panel-blank-frame` lane can find them.
3. **Register rows 12–14 are NOT this cause.** The three vacuity guards
   (`structural_chord_does_not_flush_a_stale_buffer_over_an_external_split_{loro,sqlonly}`,
   `promoted_row_keeps_its_keyword_out_of_the_title_across_a_blur_sqlonly`) run
   on this same `SimUserDriver` and fail UNCHANGED with the frame-wait in place,
   so the register's "the caret was never placed, Family A is the single
   upstream cause for rows 1–6 and 12–14" is refuted: one fix closes three rows,
   not nine.
