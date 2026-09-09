# holon-gpui red attribution — wave-11 chain tip vs `main` 830d794f878f

Verdict: **all 15 are PRE-EXISTING on `main`. Zero CAUSED by the wave-11 chain.**
No bisect at `0d28c423` was needed — no candidate passed deterministically on `main`.

## Evidence produced in this session

* Tree: `git -C /Users/martin/Workspaces/pkm/holon archive 830d794f878f` →
  `/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/bc7b1e67-1603-4c68-8742-84215e1a79e3/scratchpad/verify-gpui-main`
  (4447 files, `frontends/gpui/tests/` present). Target CoW-seeded from `_sw_integ`.
* Run (build-slot wrapped, script file
  `.../scratchpad/run-main-gpui.sh`):
  `cargo nextest run --no-fail-fast -p holon-gpui --features holon-gpui/pbt`
  Log: `.../scratchpad/main-gpui-full.log`
  `Summary [ 275.085s] 388 tests run: 347 passed (44 slow), 38 failed, 3 timed out, 6 skipped` (line 4046)
* Isolated 3× rerun of the one main-PASS candidate: `.../scratchpad/main-gherkin-iso.log`

`pwd` at every verdict step: `/Users/martin/Workspaces/pkm/holon` (script `cd`s to the extracted tree; it prints its own `pwd` at `main-gpui-full.log`).

## Table

| # | Test (binary + name) | main result | chain-tip result | Verdict | Evidence (main-gpui-full.log:line) |
|---|---|---|---|---|---|
| 1 | `windowed_log_capture every_windowed_target_declares_test_init` | FAIL | FAIL | PRE-EXISTING | 3796 |
| 2 | `layout_editor windowed_caret::clicking_an_entity_link_still_navigates` | FAIL | FAIL | PRE-EXISTING | 1934 |
| 3 | `layout_editor windowed_caret::clicking_an_external_link_opens_the_url_instead_of_navigating` | FAIL | FAIL | PRE-EXISTING | 1965 |
| 4 | `layout_smoke snapshot_captures_each_widget` | FAIL | FAIL | PRE-EXISTING | 2023 |
| 5 | `nested_page_chevron_gate an_opened_nested_page_paints_its_children` | FAIL | FAIL | PRE-EXISTING | 2115 |
| 9 | `gpui_gherkin_replay gpui_gherkin_replay` | PASS 72.7s in full run; isolated PASS/**FAIL**/PASS | FAIL | PRE-EXISTING (nondeterministic ON MAIN) | 2622 + `main-gherkin-iso.log:485` |
| 10 | `gpui_compose_sut_windowed windowed_split_then_clickblock_resolves_minted_id` | FAIL | FAIL | PRE-EXISTING | 2471 |
| 11 | `gpui_compose_sut_windowed windowed_composed_sut_replays_a_fixture_via_replay_steps_green` | FAIL | FAIL | PRE-EXISTING | 2174 |
| 12 | `gpui_sim_replay_capture gpui_sim_replay_capture` | FAIL | FAIL | PRE-EXISTING | 2208 |
| 13 | `gpui_composed_windowed_loop benchmark_windowed_per_case_boot_cost` | FAIL | FAIL | PRE-EXISTING | 2331 |
| 14 | `gpui_composed_windowed_loop general_e2e_composed_pbt_windowed` | TIMEOUT | TIMEOUT/FAIL | PRE-EXISTING | 3009 |
| 15 | `settings_integrations_setfield_popup_windowed clicking_multi_param_set_field_opens_param_popup_then_dispatches` | FAIL | FAIL | PRE-EXISTING | 3760 |
| 16 | `structural_chord_stale_flush_windowed …_external_split_loro` | FAIL | FAIL | PRE-EXISTING | 3864 |
| 17 | `structural_chord_stale_flush_windowed …_external_split_sqlonly` | FAIL | FAIL | PRE-EXISTING | 3896 |
| 18 | `task_keyword_blur_windowed promoted_row_keeps_its_keyword_out_of_the_title_across_a_blur_sqlonly` | FAIL | FAIL | PRE-EXISTING | 3933 |

### The three load-flakes (#6/#7/#8)

All PASS on `main` in the full run — consistent with the lane's load-flake call, not chain-caused.

| # | Test | main result | Evidence |
|---|---|---|---|
| 6 | `overlay_sidebar_dismisses_windowed a_desktop_sidebar_survives_the_same_two_gestures` | PASS 41.9s | 2907 |
| 7 | `overlay_sidebar_dismisses_windowed an_overlay_sidebar_dismisses_on_a_page_tap_and_on_a_tap_beside_it` | PASS 50.4s | 2956 |
| 8 | `pairing_conflict_badge_windowed a_pairing_conflict_copy_paints_its_badge` | PASS 40.5s | 2941 |

## `gpui_gherkin_replay` — why it is still PRE-EXISTING

Isolated on `main`, three consecutive runs: PASS 19.9s, **FAIL 15.1s**, PASS 10.4s.
The failure signature on `main` is identical to the chain's:

```
panicked at crates/holon-integration-tests/src/pbt/driver_input.rs:418:33:
[ClickBlock] click_entity failed for block:c1: entity block:c1 not in bounds
```

A test that fails on `main` with no chain code present cannot be chain-caused.

## Regex fragment for the known-red register

```
^holon-gpui::(windowed_log_capture every_windowed_target_declares_test_init|layout_editor windowed_caret::clicking_an_(entity_link_still_navigates|external_link_opens_the_url_instead_of_navigating)|layout_smoke snapshot_captures_each_widget|nested_page_chevron_gate an_opened_nested_page_paints_its_children|gpui_gherkin_replay gpui_gherkin_replay|gpui_compose_sut_windowed windowed_(split_then_clickblock_resolves_minted_id|composed_sut_replays_a_fixture_via_replay_steps_green)|gpui_sim_replay_capture gpui_sim_replay_capture|gpui_composed_windowed_loop (benchmark_windowed_per_case_boot_cost|general_e2e_composed_pbt_windowed)|settings_integrations_setfield_popup_windowed clicking_multi_param_set_field_opens_param_popup_then_dispatches|structural_chord_stale_flush_windowed structural_chord_does_not_flush_a_stale_buffer_over_an_external_split_(loro|sqlonly)|task_keyword_blur_windowed promoted_row_keeps_its_keyword_out_of_the_title_across_a_blur_sqlonly)$
```

## Do these reds predate `main`?

They are reds **at** `main` = `830d794f878f` — that is what this session measured, and it is the
only claim the evidence supports. The wave-10 land gate ran `-p holon -p holon-app` and never
built `holon-gpui`, so nothing in the landing record shows when the crate last went green; how
far back before `830d794f878f` these reds go is **not established** by this run and would need
the same extraction replayed at an earlier rev.

## Corroborating structural fact (not the basis of the verdict)

`git diff --stat 6452ac2d 91b1501d -- crates/holon-frontend frontends` is **empty** — the two
chain commits after the contrast lane touch no frontend code at all. `830d794f878f..0d28c423`
also touches no `frontends/` or `crates/holon-frontend` file.

## Side finding — `main` is far redder than the chain gate showed

The `-p holon-gpui`-only run at `main` produced **41** failures (38 FAIL + 3 TIMEOUT) of 388
tests, against 18 at the chain tip in the two-crate gate. Reds on `main` that never appeared in
the chain gate at all include the whole `action_bar_windowed` family (7), `chrome_one_row_windowed`
(5), `settings_integrations_*` (3 more), `block_focus_keeps_outline_windowed` (2),
`nested_page_real_engine`, `pairing_deferred_reimport_windowed`,
`pairing_toast_fits_the_window_windowed`, `breadcrumb_resolves_from_view_root_windowed`,
`integrations_sidebar_rows_windowed`, `test_platform_geometry_determinism`, and TIMEOUTs in
`accordion_bounded_pbt` / `arrow_nav_windowed_pbt`. These are load-sensitive: the single-crate run
packs the windowed targets more densely, and the box had several other cargo lanes running
concurrently. They do not change the attribution above (every one is a `main` red), but the
`holon-gpui` crate is not close to green on `main` and the known-red register will need a wider
sweep than these 15 names.
