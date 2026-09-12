# The `holon-gpui` crate's reds — register, 2026-09-10

Measured 2026-09-08 in the `gpui-reds` lane workspace.
Base: `91b1501d4016` (the wave-11 integration tip). Cross-attributed against
`main` = `830d794f878f`.

`holon-gpui` runs in **no gate**. `just landing-gate` typechecks it
(`gate-compile`) but executes none of its 390 tests, and the per-land nextest
leg is `-p holon -p holon-app`. Nothing has reported this crate's result since
before `830d794f878f`, and it is far from green.

Invocation (the only one that compiles the windowed targets):

```
cargo nextest run --no-fail-fast -p holon-gpui --features holon-gpui/pbt
```

No fix is attempted here. This document classifies; the deterministic families
are separate lanes.

**Updated 2026-09-09** by the `gpui-driver` lane, the first of those lanes to
report back: Family A resolved (rows 2, 5, 6, 16 CLOSED; rows 1, 3, 4 open on a
different cause), Family E's hypothesis refuted, deterministic tier 16 → 12.
Edits are confined to `Status` cells, the tier-1 allowlist, and the resolution
notes under each family; the 2026-09-08 measurement itself is not rewritten.

## Method — every number below comes from one file

`lane-logs/gpui-reds-rates.txt` in the lane workspace, produced by
`lane-logs/rates.sh` from the four run logs. Nothing in this register is typed
by hand from a scrollback.

| Run | Summary line |
|---|---|
| loaded 1 | `Summary [ 245.939s] 390 tests run: 374 passed (68 slow), 14 failed, 2 timed out, 6 skipped` |
| loaded 2 | `Summary [ 314.876s] 390 tests run: 332 passed (30 slow), 56 failed, 2 timed out, 6 skipped` |
| loaded 3 | `Summary [ 265.529s] 390 tests run: 371 passed (66 slow), 15 failed, 4 timed out, 6 skipped` |
| serial isolation | `Summary [1152.955s] 58 tests run: 42 passed (6 slow), 16 failed, 338 skipped` |

The three loaded runs are the full suite at default parallelism while several
other cargo lanes were building — that spread is the point. The union of their
failures is **58 names**; each was then rerun in ONE `--test-threads=1`
invocation of exactly those 58 (nextest gives every test its own process, so
serial execution is the isolation). The load spread is real and large: the same
tree produced 16, 58 and 19 failures.

**Classification rule used throughout:**

| Class | Definition |
|---|---|
| `deterministic` | fails serially, isolated from load |
| `load-sensitive` | fails in ≥1 loaded run, passes serially |

## Result

| | Count |
|---|---|
| Tests in the suite | 390 |
| Never red in any of the four runs | 332 |
| `deterministic` (as measured 2026-09-08) | **16** |
| `deterministic` (open, after the `gpui-driver` lane) | **12** |
| `load-sensitive` | **42** |

Rows 2, 5, 6 and 16 were closed by the `gpui-driver` lane on 2026-09-09 — one
cause, one fix, four rows (see Family A below). The deterministic tier is
therefore **12** open rows. `16` is kept as the measurement this document
reports; the second line is what a gate would find today.

The `main`-run log from the wave-11 attribution lane
(`main-gpui-full.log`, 41 failures at `830d794f878f`) is the first-seen
evidence. 40 of the 58 names appear in it; the other 18 are all
`load-sensitive` or flaky rows whose absence from a single `main` run proves
nothing about their age.

## The deterministic register (16 rows — 4 CLOSED, 12 open)

`first-seen` is `830d794f878f` where that rev's log carries the name, else
`91b1501d4016` — the rev this lane measured. Neither is a claim that the red
*started* there; no gate ever ran this crate, so the true origin is unmeasured
for every row.

`Status` is as of 2026-09-09. `Loaded`, `Serial` and `Failure text` stay as
MEASURED on 2026-09-08 — this register reports one measurement and does not
rewrite it; where a row's failure has since changed, the `Status` cell says so.

| # | Binary | Test | Status | Loaded | Serial | First-seen | Failure text (excerpt) |
|---|---|---|---|---|---|---|---|
| 1 | `gpui_compose_sut_windowed` | `windowed_split_then_clickblock_resolves_minted_id` | **OPEN — cause replaced** | 3/3 | FAIL | `830d794f878f` | `op_write_cap.rs:289` `[SplitBlock/keystroke] focus block:c1 failed: entity block:c1 not in bounds` |
| 2 | `gpui_composed_windowed_loop` | `benchmark_windowed_per_case_boot_cost` | **CLOSED** (`gpui-driver`) | 3/3 | FAIL | `830d794f878f` | `driver_input.rs:418` `[ClickBlock] click_entity failed for block:c1: entity block:c1 not in bounds` |
| 3 | `gpui_composed_windowed_loop` | `general_e2e_composed_pbt_windowed` | **OPEN — cause replaced; BORDERLINE on budget** | 3/3 (TIMEOUT under load) | FAIL 89s | `830d794f878f` | `[4b-loop] windowed PBT failed (shrunk): SUT apply panicked: [SplitBlock/keystroke] focus block:c1 failed: entity block:c1 not in bounds` |
| 4 | `gpui_sim_replay_capture` | `gpui_sim_replay_capture` | **OPEN — cause replaced** | 3/3 | FAIL | `830d794f878f` | replaying `presskey_loro_split_backspace`; `op_write_cap.rs:289` `focus block:c1 failed: entity block:c1 not in bounds` |
| 5 | `gpui_compose_sut_windowed` | `windowed_composed_sut_drives_a_click_gesture_sequence_green` | **CLOSED** (`gpui-driver`) | 2/3 | FAIL | `91b1501d4016` | `driver_input.rs:418` `[ClickBlock] click_entity failed for block:c1: entity block:c1 not in bounds` |
| 6 | `gpui_compose_sut_windowed` | `windowed_composed_sut_replays_a_fixture_via_replay_steps_green` | **CLOSED** (`gpui-driver`) | 1/3 | FAIL | `830d794f878f` | `driver_input.rs:418` `[ClickBlock] click_entity failed for block:c2: entity block:c2 not in bounds` |
| 7 | `layout_editor` | `windowed_caret::clicking_an_entity_link_still_navigates` | OPEN | 3/3 | FAIL 0.10s | `830d794f878f` | `layout_editor.rs:446` `a registered entity link should dispatch exactly one navigation intent; left: 0, right: 1` |
| 8 | `layout_editor` | `windowed_caret::clicking_an_external_link_opens_the_url_instead_of_navigating` | OPEN | 3/3 | FAIL 0.03s | `830d794f878f` | `layout_editor.rs:413` `clicking an external link should hand the URL to the platform opener; left: None, right: Some("https://example.com")` |
| 9 | `layout_smoke` | `snapshot_captures_each_widget` | OPEN | 3/3 | FAIL 0.03s | `830d794f878f` | `layout_smoke.rs:199` `expected 1 badge, got 2` |
| 10 | `nested_page_chevron_gate` | `an_opened_nested_page_paints_its_children` | OPEN | 3/3 | FAIL | `830d794f878f` | `nested_page_chevron_gate.rs:753` `"buy milk" is not among the painted text of the window … painted = ["▼", "A Nested Page"]` |
| 11 | `settings_integrations_setfield_popup_windowed` | `clicking_multi_param_set_field_opens_param_popup_then_dispatches` | OPEN — re-confirmed at `f134df9ece6c` 2026-09-12 (`uc-fixes`) | 3/3 | FAIL | `830d794f878f` | `…setfield_popup_windowed.rs:288` `the popup's value step must offer "op-param-item-value-true". op-param ids painted now: []` |
| 12 | `structural_chord_stale_flush_windowed` | `structural_chord_does_not_flush_a_stale_buffer_over_an_external_split_loro` | OPEN — unchanged by `gpui-driver` | 3/3 | FAIL | `830d794f878f` | `…rs:351` `vacuity guard: the Tab chord changed no parentage, so no structural op was dispatched` |
| 13 | `structural_chord_stale_flush_windowed` | `structural_chord_does_not_flush_a_stale_buffer_over_an_external_split_sqlonly` | OPEN — unchanged by `gpui-driver` | 3/3 | FAIL | `830d794f878f` | as row 12 |
| 14 | `task_keyword_blur_windowed` | `promoted_row_keeps_its_keyword_out_of_the_title_across_a_blur_sqlonly` | OPEN — unchanged by `gpui-driver` | 3/3 | FAIL | `830d794f878f` | `…rs:357` `vacuity guard: the blur dispatched NO operation (6 history rows before and after)` |
| 15 | `windowed_log_capture` | `every_windowed_target_declares_test_init` | OPEN | 3/3 | FAIL 0.03s | `830d794f878f` | `windowed_log_capture.rs:162` `these windowed test targets install no tracing subscriber — add \`mod test_init;\`` (10 files) |
| 16 | `block_focus_keeps_outline_windowed` | `a_short_window_still_paints_the_outline` | **CLOSED** (`gpui-driver`) — see caveat | 1/3 | FAIL | `830d794f878f` | `…rs:457` `the main panel has a 438.0px box and three one-line rows to draw, and painted 0 of them: {}; left: 0, right: 3` |

Rows 5, 6 and 16 fail serially but pass in some loaded runs. That is the
signature of a red with a *nondeterministic* trigger, not of a load flake —
they stay in the deterministic tier, because a test that fails when nothing
else competes with it is not excused by load.

### Rows 2, 5, 6, 16 — CLOSED 2026-09-09 by the `gpui-driver` lane

One cause, one fix, four rows. The harness change is
`SimUserDriver::click_point_when_painted` in
`frontends/gpui/tests/pbt_harness/sim_windowed_replay.rs`: every bounds-taking
verb now pumps until a frame actually paints the entity (bounded by
`BOUNDS_WAIT_PUMP_CYCLES`) and fails loud with a painted-window census, instead
of resolving the click point from one committed frame. Recorded as
`docs/Testing/bugfunnel/entries/2026-09-09-windowed-driver-reads-one-frame-for-entity-bounds.md`.

Green serially, `--test-threads=1` (`gpui-driver` lane-logs
`gate-nine-11537.log`, `gate-full-serial-53638.log`, `rev2-tests-76186.log`;
independently reproduced by the verifier):

| Row | Result |
|---|---|
| 2 | `PASS [41.909s]` |
| 5 | `PASS [16.970s]` |
| 6 | `PASS [16.766s]` |
| 16 | `PASS [11.950s]` |

**Row 16 caveat.** It was recorded 1/3 loaded + FAIL serial, i.e. a
nondeterministic trigger. Three green serial runs (lane ×2, verifier ×1) make it
*likely* fixed, not proven. Treat a future red there as a re-open, not a new row.

### Rows 1, 3, 4 — still OPEN, but the cause is no longer this family

They now fail PAST the bounds gate, at
`crates/holon-integration-tests/src/pbt/op_write_cap.rs:381`:

```
[SplitBlock/keystroke] cannot place the caret for content byte 0 on block:c1:
editable surface not projected by this driver
```

That string is the `UserDriver::surface_chars_before_content` trait DEFAULT
(`crates/holon-frontend/src/user_driver.rs:299`). Only the headless
`ReactiveEngineDriver` implements it, via
`HeadlessEditorMirror::content_offset_to_surface`; neither `SimUserDriver` nor
the production `GpuiUserDriver` does — so windowed `SplitBlock` caret placement
has never worked. Feature-sized, queued as the `windowed-split-caret` lane.

**Row 3 is BORDERLINE against its 120s nextest budget.** After the fix it gets
further into the generated sequence before failing, so each case does more work:
observed once as `FAIL 87.8s` and twice as `TIMEOUT 120s` (including in
isolation), always at the same panic site. That is base-like, not worse — the
row was already `3/3 (TIMEOUT under load)` before any of this — but it now sits
close enough to the budget that a TIMEOUT here should be read as *this* row, not
as a new load flake. The `windowed-split-caret` lane should expect to raise the
budget or shrink the case count.

## Root-cause hypotheses and their discriminating checks

One line per family, with the observation that would settle it. **No fix is
attempted in this lane** — each family is a separate lane.

### Family A — "entity not in bounds" (rows 1–6): the windowed driver cannot hit-test block entities

The single largest family: six tests, all panicking inside the shared PBT
driver at `crates/holon-integration-tests/src/pbt/driver_input.rs:418`
(`click_entity`) or `crates/holon-integration-tests/src/pbt/op_write_cap.rs:289`
/ `:381` (`focus` and caret placement, "editable surface not projected by this
driver"). Every message names a *seeded* block (`block:c1`, `block:c2`,
`block:parent`) — the driver's bounds lookup returns nothing for a block the
window is supposed to be painting.

- **Hypothesis:** the windowed driver reads a bounds/entity map that the GPUI
  compose SUT populates one frame later than the driver queries it (or not at
  all for rows below the first), so the lookup races the paint rather than
  missing a render.
- **Discriminating check:** at the failing `click_entity`/`focus` call, dump the
  painted-text census of the window alongside the driver's bounds map. Census
  contains the block's text but the bounds map does not ⇒ a
  registration/flush-ordering defect in the windowed driver. Census also lacks
  it ⇒ a real render gap, and the family belongs with rows 10/16.

**RESOLVED 2026-09-09 (`gpui-driver`). The check returned the SECOND branch: the
census also lacks it — a render gap, not a registration race.** The hypothesis
above is refuted; the family is one cause, and it is two facts:

1. The main panel's collection `ReactiveShell` is **re-created with an EMPTY item
   vec on every projection rebuild** and refills only on its next
   `signal_vec_cloned()` tick. Probes show the shell holding
   `items=5 visible=5` including `block:c1` with all five `gpui::list` row
   callbacks firing, while a *different*, freshly-constructed shell instance
   renders last and is the frame that gets committed — a full-height
   `reactive_shell` with no descendants at all. The window is not lost: engine
   focus and the panel's `view_mode_switcher` both name the right page.
2. The driver **sampled one frame**. `click_entity` resolved the click point with
   a single bounds read and bailed, while every other verb on the same driver
   already retried across frames.

Attribution proof: adding a bare `eprintln!` inside the list row callback — pure
timing, no logic — moved row 1 off the bounds bail onto its next precondition.

Fixing (2) closed rows 2, 5, 6, 16. Fact (1) is a PRODUCTION defect and stays
open in the bug funnel:
`2026-09-09-main-panel-collection-shell-is-rebuilt-empty-each-projection` plus
the oracle exemption that hid it,
`2026-09-09-content-fidelity-exempts-a-shell-with-no-descendants`. Note that the
driver's new frame-wait makes a *transient* blank panel unobservable to every
windowed oracle — the fix lane cannot use this suite as its red. The
*persistent* variant is row 10 (Family D), still open.

### Family B — link marks do not dispatch (rows 7, 8)

Both windowed link tests see zero effect from the click: no navigation intent,
no URL handed to the opener. Note `justfile:415` already carries a
"KNOWN RED (pre-existing): the link-mark `[[label]]` extraction divergence"
note for `pbt-layout-override`.

- **Hypothesis:** the link mark's clickable region is never registered as an
  interactive entity in the windowed path — the same registration seam as
  Family A, reached through the mark builder instead of the block row.
- **Discriminating check:** assert the window paints a link entity with a
  non-empty hit region before dispatching the click. Region absent ⇒ shared
  root cause with Family A; region present and click still inert ⇒ the mark's
  click handler is not wired in the windowed embedder.

### Family C — `snapshot_captures_each_widget`: "expected 1 badge, got 2" (row 9)

The paint dump in the failure shows `text#0`, `text#1`, `badge#2`.

- **Hypothesis:** a second badge widget entered the smoke fixture's widget set
  (the conflict-copy badge variant added under D93.a is the obvious candidate)
  without the snapshot's expected count following it — a stale oracle, not a
  product defect.
- **Discriminating check:** `git log -p` on
  `crates/holon-frontend/src/shadow_builders/badge.rs` against the assertion at
  `frontends/gpui/tests/layout_smoke.rs:199`. A badge added after the
  expectation was written ⇒ stale oracle, fix the test.

### Family D — content materialises but does not paint (rows 10, 16)

`an_opened_nested_page_paints_its_children` reports the gate open and the
content materialised, yet `painted = ["▼", "A Nested Page"]`.
`a_short_window_still_paints_the_outline` paints **0** of three rows in a
438px box.

- **Hypothesis:** the main panel's child list gets a zero or unsatisfiable
  height constraint in the windowed layout, so rows are laid out but clipped
  entirely rather than being absent from the tree.
- **Discriminating check:** print the main panel's computed bounds and each
  child row's bounds. Rows present with zero height or fully outside the parent
  box ⇒ a layout-constraint defect; rows absent from the tree ⇒ a projection
  defect, and Family A's second branch.

### Family E — vacuity guards fire: no operation is dispatched (rows 12, 13, 14)

All three are *guards*, not the properties themselves: the Tab chord changed no
parentage; the blur produced 6 history rows before and after. The test correctly
refuses to pass on an interaction that never happened.

- **Hypothesis:** the gesture never reaches the op funnel because the caret/focus
  was never placed — the identical precondition Family A shows failing loudly.
- **Discriminating check:** log the focused block id immediately before the
  chord/blur. No focused block ⇒ Family A is the single upstream cause for
  rows 1–6 and 12–14, and one fix closes nine rows.

**REFUTED 2026-09-09 (`gpui-driver`). Family E is NOT downstream of Family A.**
All three rows run on the same `SimUserDriver`, and all three fail **unchanged**
— same vacuity-guard text, same line — with Family A's frame-wait in place and
Family A's own rows green. So one fix closes **four** rows, not nine, and this
family needs its own root-cause pass: the caret/focus precondition is not what
is missing here.

### Family F — `every_windowed_target_declares_test_init` (row 15)

A meta-test with a complete, actionable list: ten windowed targets install no
tracing subscriber — `action_bar_windowed.rs`,
`android_soft_return_contract.rs`, `arrow_nav_windowed.rs`,
`arrow_nav_windowed_pbt.rs`, `gpui_journals_logseq_look.rs`,
`inert_integration_disclosure_windowed.rs`, `popup_row_click_windowed.rs`,
`settings_integrations_setfield_popup_windowed.rs`,
`slash_command_text_hidden_windowed.rs`, `state_toggle_switch_windowed.rs`.

- **Hypothesis:** none needed — targets were added without `mod test_init;`
  because nothing ran the guard.
- **Discriminating check:** add the module to the ten files; the test goes
  green. **This is the cheapest row in the register and it should land first**,
  because it is exactly why the other rows produce unattributed log output.

### Family G — the set-field param popup paints no values (row 11)

`op-param ids painted now: []` against a census that does show
`row: 4`, `selectable: 4`, `tree_item`.

- **Hypothesis:** the popup's value step renders its items under a different id
  prefix than the test asserts, or the multi-param op's value list resolves
  empty in the windowed wiring.
- **Discriminating check:** dump every painted id (not just `op-param*`) at the
  failing step. Items present under another prefix ⇒ a stale oracle; nothing
  list-shaped painted ⇒ a real popup wiring defect.

#### Re-confirmed 2026-09-12 by lane `uc-fixes`, and the hypothesis narrows

Still deterministic at `f134df9ece6c`, same assertion and same line. Attributed
by A/B rather than by date: with EVERY source file of that lane reverted to
`f134df9ece6c` it fails identically (`lane-logs/popup-ab-base-1789181169.log` in
the `uc-fixes` workspace; the restoration is sha256-proven in
`lane-logs/ab-restore-sha.txt`). It is not caused by `user-connections` and not
by the settings-table template change that lane made.

The discriminating check above is ANSWERED, and neither branch of it holds. The
census printed at the failing step carries **no integration widget of any kind**
— no `op_button`, no `state_toggle`, no `table-cell-col-*`, nothing of the
Settings modal:

```
census: {"column": 3, "divider": 2, "expand_toggle": 3, "icon": 4,
         "live_block": 4, "live_query": 2, "row": 4, "selectable": 4,
         "spacer": 3, "text": 5, "tree_item": 3, "view_mode_switcher": 3}
```

That is the seeded sidebar alone. So the popup's value items are not painted
under another prefix and the value list is not resolving empty — the MODAL IS
GONE by that point. The click that picks `field=enabled` in the first step
dismisses the popup AND closes the modal, which is the behaviour the sibling
`outside_click_on_inert_space_closes_the_popup_without_dispatching` asserts for
a click on inert space. The next root-cause pass should ask why the first step's
own item registers as outside-the-popup, not why the second step paints nothing.

Every name below failed in at least one loaded run and **passed serially**.
Full per-name rates are in `lane-logs/gpui-reds-rates.txt`; the families are:

| Family | Names | Worst loaded rate |
|---|---|---|
| `accordion_bounded_pbt accordion_region_is_bounded` | 1 | 3/3 — **TIMEOUT at 120s in all three loaded runs**, passes serially in 58s |
| `action_bar_windowed` | 9 | 1/3 |
| `chrome_one_row_windowed` | 8 | 1/3 |
| `settings_integrations_*` (4 binaries) | 6 | 2/3 |
| `pairing_*` (3 binaries) | 5 | 2/3 |
| `gpui_compose_sut_windowed` (3 green-serial cases) | 3 | 1/3 |
| others (`arrow_nav_windowed_pbt`, `block_focus_keeps_outline_windowed`, `breadcrumb_…`, `gpui_gherkin_replay`, `gpui_rebind_reset_smoke`, `integrations_*`, `nested_page_real_engine`, `overlay_sidebar_dismisses_windowed`, `tab_strip_resolves_at_boot_windowed`) | 10 | 2/3 |

`accordion_region_is_bounded` deserves its own note: it is the only row that is
red in **all three** loaded runs and green serially. It is a pure timeout — 120s
budget, 58s serial — so it will red any gate that runs this crate beside another
cargo lane unless it is pinned into a single-thread nextest test-group, the
treatment `cursor_filtered_main_panel` already has (D64.a).

## Proposed gate rule (proposal only — not implemented here)

**What can enter the landing gate today:** the whole crate, judged against a
known-signature list rather than against exit code — the same discipline
`docs/Testing/HolonCrateReds-2026-09-01.md` established for `-p holon`. 332 of
390 tests (85%) were green in all four runs and would be gated strictly from
day one. Gating only a green subset would instead need a name allowlist that
silently stops covering every new test.

Proposed step, beside the existing per-land nextest leg:

```
cargo nextest run --no-fail-fast -p holon-gpui --features holon-gpui/pbt
```

with two preconditions, because without them the gate reports noise:

1. Pin `accordion_bounded_pbt` and `gpui_composed_windowed_loop` into
   single-thread nextest test-groups in `.config/nextest.toml`. Both are
   whole-window boots; run beside the rest of the suite they time out on load,
   not on defect.
2. Classify with the fragments below. Tier 1 is **pass-with-note and must
   shrink** — each row is an open lane. Tier 2 is **pass-with-note under load
   only**: a Tier-2 name that fails in a *serial* rerun is a regression, not a
   flake. **Any name outside both tiers blocks the land.**

### Tier 1 — known deterministic (12), regex over test function names

Rows 2, 5, 6 and 16 are **removed** from this allowlist: they are fixed, so a
future red there is a REGRESSION the gate must catch, not a known red it should
excuse.

```
^(windowed_split_then_clickblock_resolves_minted_id|general_e2e_composed_pbt_windowed|gpui_sim_replay_capture|windowed_caret::clicking_an_entity_link_still_navigates|windowed_caret::clicking_an_external_link_opens_the_url_instead_of_navigating|snapshot_captures_each_widget|an_opened_nested_page_paints_its_children|clicking_multi_param_set_field_opens_param_popup_then_dispatches|structural_chord_does_not_flush_a_stale_buffer_over_an_external_split_loro|structural_chord_does_not_flush_a_stale_buffer_over_an_external_split_sqlonly|promoted_row_keeps_its_keyword_out_of_the_title_across_a_blur_sqlonly|every_windowed_target_declares_test_init)$
```

### Tier 2 — known load-sensitive (42), regex over test function names

```
^(accordion_region_is_bounded|a_desktop_window_paints_no_action_bar|entity_ops_come_before_global_ops_in_the_bar|resting_bottom_chrome_alone_does_not_raise_the_bar|tapping_a_dock_op_button_dispatches_the_operation|tapping_a_global_op_button_dispatches_it|the_dock_stays_one_row_below_the_main_panel|the_focused_blocks_ops_paint_in_the_dock_above_the_keyboard|the_global_tier_renders_with_nothing_focused|with_the_keyboard_down_the_bar_is_not_painted|arrow_walk_keeps_focus_on_the_reference_outline_neighbour|raising_the_keyboard_must_not_hide_the_block_rows|the_breadcrumb_follows_the_view_root_and_then_the_focus|a_no_op_tab_switch_costs_one_read|a_refused_tab_write_keeps_saying_so|a_write_that_never_lands_says_the_count_is_waiting|closing_the_active_tab_counts_down_and_moves_the_highlight|the_chrome_is_one_row_at_every_width|the_count_button_shows_the_failure_not_a_plausible_zero|the_tab_count_button_opens_a_list_that_switches_and_creates_tabs|the_tab_list_hangs_under_the_title_row_beneath_a_notch|overlay_windowed_caps_composes_layout_backend_and_driver_over_a_live_window|window_renders_compose_sut_base_and_base_hosts_backend|windowed_composed_sut_runs_full_catalog_green_on_the_initial_frame|gpui_gherkin_replay|rebind_reset_smoke|a_long_integration_name_does_not_push_the_status_out_of_its_row|integration_rows_show_name_icon_aligned_status_and_open_their_view|a_real_engine_nested_page_paints_its_children_when_opened|a_desktop_sidebar_survives_the_same_two_gestures|a_conflict_copy_that_is_also_a_page_still_wears_its_badge|a_long_conflict_copy_still_paints_a_visible_badge|a_pairing_conflict_copy_paints_its_badge|the_window_paints_the_deferred_reimport_banner_and_its_retry_completes_the_pair|every_pairing_toast_line_is_inside_the_window|the_settings_modal_paints_the_integration_rows_operations|every_row_op_button_stays_on_its_own_row|escape_closes_the_param_popup_without_dispatching|opening_a_second_rows_op_button_closes_the_first|outside_click_on_inert_space_closes_the_popup_without_dispatching|the_settings_modal_renders_integrations_as_an_aligned_table|the_tab_strip_draws_an_open_tab_without_being_prodded)$
```

Tier 2 is a wide list — 11% of the suite excused on load — and it should not
stay this wide. It is measured at ONE point of machine load and shrinks as
whole-window boots get concurrency pins.

## What this register does not establish

**When these reds started.** No gate has ever run `holon-gpui`, so every row's
`first-seen` is only the earliest rev whose log this session happens to hold.
Establishing a true origin needs the same extraction replayed at earlier revs.

## Bug-funnel entry

`docs/Testing/bugfunnel/entries/2026-09-10-holon-gpui-suite-red-at-main-and-ungated.md`
(gap `ENVIRONMENT`, secondary `COVERAGE`, status `OPEN`).

From the `gpui-driver` lane (2026-09-09), for Family A:

- `2026-09-09-windowed-driver-reads-one-frame-for-entity-bounds.md`
  (`ENVIRONMENT`/`ORACLE`, **FIXED**) — the harness cause of rows 2, 5, 6, 16.
- `2026-09-09-main-panel-collection-shell-is-rebuilt-empty-each-projection.md`
  (`PERCEPTION`/`ORACLE`, **OPEN**) — the production defect underneath, for the
  queued `panel-blank-frame` lane.
- `2026-09-09-content-fidelity-exempts-a-shell-with-no-descendants.md`
  (`ORACLE`, **OPEN**) — the exemption that let it live.
