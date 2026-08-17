---
id: 2026-08-14-eight-windowed-gpui-test-binaries-did
date: 2026-08-14
gap: PERCEPTION
secondary: ENVIRONMENT
status: FIXED
summary: >-
  Eight windowed GPUI test binaries did not compile, so the entire windowed
  slice silently ran in no gate
source_line: 710
---

## Bug

(task-#24 windowed-slice-fix lane; found by the A1 lane stumbling over it,
no gate reported it) **Eight windowed GPUI test binaries did not compile, so
the entire windowed slice silently ran in no gate**: an earlier increment
migrated the shared harness (`pbt_harness/sim_windowed_replay.rs:80`,
`pbt_harness/windowed_wide.rs:50`) from gpui's `TestApp` to
`HeadlessAppContext` and left the callers on `TestApp` — E0308 `expected
*const HeadlessAppContext, found &TestApp` in `gpui_window_slice.rs`,
`window_chord_reentrant_dispatch.rs`,
`structural_chord_stale_flush_windowed.rs`,
`undo_survives_blur_windowed.rs`, `cmd_enter_chord_dispatch.rs`,
`task_keyword_blur_windowed.rs`, `live_promotion_windowed.rs`,
`nested_page_real_engine.rs`. `gpui_journals_viewport.rs:12-14` records the
drift in a header comment, so it was known and unfixed.

## Root cause

task-#24 windowed-slice-fix lane, found by the A1 lane STUMBLING OVER IT —
no gate reported it: **eight windowed GPUI test binaries did not compile, so
the ENTIRE windowed slice silently ran in NO gate.** An earlier increment
migrated the shared harness
(`frontends/gpui/tests/pbt_harness/sim_windowed_replay.rs:80`,
`windowed_wide.rs:50`) from gpui's `TestApp` to `HeadlessAppContext` and
left the callers on `TestApp` (`gpui_window_slice.rs`,
`window_chord_reentrant_dispatch.rs`,
`structural_chord_stale_flush_windowed.rs`,
`undo_survives_blur_windowed.rs`, `cmd_enter_chord_dispatch.rs`,
`task_keyword_blur_windowed.rs`, `live_promotion_windowed.rs`,
`nested_page_real_engine.rs`) — E0308 `expected *const HeadlessAppContext,
found &TestApp`. `gpui_journals_viewport.rs:12-14` even RECORDS the drift in
a header comment, so it was known and unfixed. No assertion can express "a
test target that no longer builds"; a green gate and an uncompiled gate look
identical from the outside. ENVIRONMENT secondary: no gate compiles the
`holon-gpui` test targets, so the break could persist indefinitely. FIXED:
the eight callers migrated to
`HeadlessAppContext::with_platform(text_system, assets, ||
gpui_platform::current_headless_renderer())`, matching the already-migrated
`gpui_journals_viewport.rs`/`windowed_wide.rs` form; `cargo check -p
holon-gpui --tests --features holon-integration-tests/pbt,holon-gpui/pbt`
now exits 0 across ALL test binaries. Structural remedy — adding that `cargo
check` to a gate so a non-compiling windowed target is loud — is NOT done
here and is the open follow-up.)

## Missing piece

No assertion can express "a test target that no longer builds" — a green
gate and an uncompiled gate look identical from outside; and no gate
compiles the `holon-gpui` test targets, so the break could persist
indefinitely.

## Remedy

FIXED (compilation) — the eight callers migrated to
`HeadlessAppContext::with_platform(text_system, assets, \ | \ |
gpui_platform::current_headless_renderer())`, matching the already-migrated
`gpui_journals_viewport.rs`/`windowed_wide.rs` form; `cargo check -p
holon-gpui --tests --features holon-integration-tests/pbt,holon-gpui/pbt`
exits 0 across all test binaries. OPEN: adding that check to a gate so a
non-compiling windowed target is loud.
