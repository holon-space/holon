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

Each count is the run's own nextest `Summary` line. Logs live under
`lane-logs/main-reds-2/` and `lane-logs/verify-reds-2/`.

| Run | Summary line |
| --- | --- |
| before the test fixes below (`win-r1.log:2104`) | `639 tests run: 629 passed (4 slow), 10 failed, 5 skipped` |
| after them (`win-r2-fixed.log:7761`) | `639 tests run: 635 passed (3 slow), 4 failed, 5 skipped` |
| verifier rerun (`windowed-subset-run1.log:6373`) | `639 tests run: 635 passed (6 slow), 4 failed, 5 skipped` |

The 5 skipped tests are excluded by the default filter or `#[ignore]`
(`Starting 639 tests across 10 binaries (5 tests skipped)`, `win-r2-fixed.log:21`).

| Test | Signature | Class | Registry key |
| --- | --- | --- | --- |
| `gpui_compose_sut_windowed windowed_split_then_clickblock_resolves_minted_id` | `[SplitBlock/keystroke] cannot place the caret … editable surface not projected by this driver` | see registry row | `windowed-splitblock-no-editable-surface` (known-red) |
| `holon-loro loro_share_backend::tests::owning_page_types_a_missing_mount_and_an_unloaded_share` | `block:host is or holds a share (…), so this op cannot remove it` | see registry row | `loro-owning-page-types-share-removal` (known-red) |
| `gpui_composed_windowed_loop general_e2e_composed_pbt_windowed` | same SplitBlock signature as the first row: 11 panics in this test (`win-r2-fixed.log` lines 1342–2958: 10 at `op_write_cap.rs:382`, then the shrunk re-report at `gpui_composed_windowed_loop.rs:270`) | see registry row | `windowed-splitblock-no-editable-surface` (known-red); red in 2 of 3 runs (table above) |

`general_e2e_composed_pbt_windowed` passed in 48.509 s (`win-r1.log:2099`),
failed in 65.004 s (`win-r2-fixed.log:859`) and failed in 56.952 s
(`windowed-subset-run1.log:861`). Its binary has no slow-timeout override, so
the default 2 min cap applies (`.config/nextest.toml:6`).

## Reds fixed in the tests (all test/harness bugs)

| Test | Cause |
| --- | --- |
| `layout_editor windowed_caret::clicking_an_{external,entity}_link_…` | The fixture row had no `content` column, so `marks_of` dropped the link marks and the click seeded a caret. |
| `layout_smoke snapshot_captures_each_widget` | A badge records two trackers by design (registry wrapper + labelled builder tracker); the test expected one. |
| `nested_page_chevron_gate an_opened_nested_page_paints_its_children` | The test services had no query engine, so the `from descendants` live_query painted its degraded banner. |
| `windowed_log_capture every_windowed_target_declares_test_init` | `stop_signals_quit_through_the_session_shutdown.rs` did not declare `mod test_init;`. |
| `task_keyword_blur_windowed …_blur_sqlonly` | The vacuity guard required the blur to dispatch a write. Neither arm attaches an editor cell (D113.a), so each keystroke commits its own `set_field` and re-baselines the blur funnel: history went 1 → 6 over typing and 6 → 6 over the blur (`lane-logs/main-reds-3/blur-probe.log:70-76`). The guard now requires typing to add rows and the blur to add none. |
| `structural_chord_stale_flush_windowed …_{sqlonly,loro}` | `block.create` appends, so the edit row was the first child and Tab failed with `Cannot indent: no previous sibling`. The test now moves the sibling ahead of it. |
