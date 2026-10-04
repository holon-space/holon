# Windowed GPUI gate — measured known reds

The windowed gate (D48) runs this nextest subset. It opens real windows.

```
cargo nextest run --no-fail-fast -p holon-gpui -p holon-loro --lib \
  --test layout_editor --test layout_smoke --test windowed_log_capture \
  --test nested_page_chevron_gate --test task_keyword_blur_windowed \
  --test structural_chord_stale_flush_windowed --test gpui_compose_sut_windowed \
  --test gpui_composed_windowed_loop
```

Classify a red run with `scripts/keystone-known-reds.sh <log>`. The patterns
live in [KeystoneKnownReds.md](KeystoneKnownReds.md); this file does not hold a
second copy. A nextest `TIMEOUT` is a signature of its own
(`TIMEOUT: <binary> <test>`), so it is never read as a pass.

## Measured on main `314cf337` (2026-10-04)

Before the test fixes below: 639 run, 629 passed, 10 failed, 0 timed out.
After them (one run): 639 run, 635 passed, 4 failed, 0 timed out.

| Test | Signature | Class | Registry key |
| --- | --- | --- | --- |
| `gpui_compose_sut_windowed windowed_split_then_clickblock_resolves_minted_id` | `[SplitBlock/keystroke] cannot place the caret … editable surface not projected by this driver` | see registry row | `windowed-splitblock-no-editable-surface` (known-red) |
| `holon-loro loro_share_backend::tests::owning_page_types_a_missing_mount_and_an_unloaded_share` | `block:host is or holds a share (…), so this op cannot remove it` | see registry row | `loro-owning-page-types-share-removal` (known-red) |
| `gpui_composed_windowed_loop general_e2e_composed_pbt_windowed` | same SplitBlock signature as the first row (12 shrink re-panics) | see registry row | `windowed-splitblock-no-editable-surface` (known-red); red in 1 of 2 runs |
| `task_keyword_blur_windowed promoted_row_keeps_its_keyword_out_of_the_title_across_a_blur_sqlonly` | `vacuity guard: the blur dispatched NO operation (6 history rows before and after)` | undiagnosed | none — classifies NOVEL until triaged |

The product assertions of the blur test pass; only its non-vacuity guard
fails. The editor logs `no editor-cell registry … every keystroke will write
through the on-blur funnel` for the row. The cause of the missing blur write
is not measured.

`general_e2e_composed_pbt_windowed` passed in 48.5 s in the first run and
failed in 65 s in the second. On an older main it hit the 120 s nextest cap
once: that binary has no slow-timeout override in `.config/nextest.toml`.

## Reds fixed in the tests (all test/harness bugs)

| Test | Cause |
| --- | --- |
| `layout_editor windowed_caret::clicking_an_{external,entity}_link_…` | The fixture row had no `content` column, so `marks_of` dropped the link marks and the click seeded a caret. |
| `layout_smoke snapshot_captures_each_widget` | A badge records two trackers by design (registry wrapper + labelled builder tracker); the test expected one. |
| `nested_page_chevron_gate an_opened_nested_page_paints_its_children` | The test services had no query engine, so the `from descendants` live_query painted its degraded banner. |
| `windowed_log_capture every_windowed_target_declares_test_init` | `stop_signals_quit_through_the_session_shutdown.rs` did not declare `mod test_init;`. |
| `structural_chord_stale_flush_windowed …_{sqlonly,loro}` | `block.create` appends, so the edit row was the first child and Tab failed with `Cannot indent: no previous sibling`. The test now moves the sibling ahead of it. |
