---
id: 2026-09-10-holon-gpui-suite-red-at-main-and-ungated
date: 2026-09-10
gap: ENVIRONMENT
secondary: COVERAGE
status: OPEN
summary: >-
  The whole holon-gpui windowed test suite runs in no gate and is red at main —
  16 deterministic failures and 42 load-sensitive ones out of 390 tests.
---

## Bug

`cargo nextest run --no-fail-fast -p holon-gpui --features holon-gpui/pbt` was
run four times in the `gpui-reds` lane at base `91b1501d4016` (the wave-11
integration tip): three times at default parallelism under real machine load,
then once with `--test-threads=1` over the union of everything that had failed.

- loaded runs: 16, 58 and 19 failures of 390 tests
- serial rerun of those 58: **16 fail with no competing load**

Found by inspection during wave-11 integration, not by any automated gate —
which is the escape itself. `just landing-gate` typechecks `holon-gpui` through
`gate-compile` but executes none of its tests, and the per-land nextest leg is
`-p holon -p holon-app` (DEVELOPMENT.md, "Quality gates"). The crate's windowed
suite has therefore reported nothing for an unmeasured stretch of history.

The reds are not new to the wave-11 chain: 40 of the 58 names also appear in a
same-session `-p holon-gpui` run at `main` = `830d794f878f`
(`Summary [ 275.085s] 388 tests run: … 38 failed, 3 timed out`).

## Root cause

Two separable causes, hence the dual classification.

**ENVIRONMENT (primary) — no gate executes these tests.** The windowed targets
need `--features holon-gpui/pbt` and a real GPUI window; no gate tier pays that
cost, so the suite rotted with nothing reporting it. This is the identical shape
as `2026-09-02-two-instance-binary-is-red-on-main-and-in-no-gate` and as the 25
un-gated `-p holon` reds registered in
`docs/Testing/HolonCrateReds-2026-09-01.md`.

A second environment fact makes it worse: 42 of the 58 failures pass serially
and fail only under load. Whole-window boots
(`accordion_bounded_pbt`, `gpui_composed_windowed_loop`) carry no nextest
concurrency pin, so `accordion_region_is_bounded` times out at 120s in all three
loaded runs while completing in 58s serially. Any gate wired without those pins
would report noise on its first run.

**COVERAGE (secondary) — the windowed driver cannot reach the interactions.**
The largest deterministic family (6 of 16) is a driver capability gap, not a
product assertion: `crates/holon-integration-tests/src/pbt/driver_input.rs:418`
`[ClickBlock] click_entity failed for block:c1: entity block:c1 not in bounds`
and `crates/holon-integration-tests/src/pbt/op_write_cap.rs:289`/`:381`
`[SplitBlock/keystroke] … editable surface not projected by this driver`. Three
further rows (`structural_chord_…_loro`/`_sqlonly`,
`promoted_row_keeps_its_keyword_out_of_the_title_across_a_blur_sqlonly`) fail on
their own *vacuity guards* — the gesture dispatched no operation at all — which
is the same precondition failing quietly.

Full per-signature classification, failure text, first-seen rev, and a
discriminating check per family:
`docs/Testing/GpuiCrateReds-2026-09-10.md`. Every number there derives from
`lane-logs/gpui-reds-rates.txt` in the lane workspace.

## Missing piece

No gate tier runs `holon-gpui`. A test suite that no gate executes is
indistinguishable from a green one — the same mechanism as BugFunnel row 78
(a feature-gated suite compiling to zero tests) and as the two-instance binary,
now on its third repetition in this repo.

## Remedy

OPEN. Three separate lanes, none of them this one:

1. **Wire the gate step.** Add `-p holon-gpui --features holon-gpui/pbt` to the
   per-land nextest leg, classified against the two known-signature regex tiers
   proposed in `docs/Testing/GpuiCrateReds-2026-09-10.md`, after pinning
   `accordion_bounded_pbt` and `gpui_composed_windowed_loop` into single-thread
   nextest test-groups. 332 of 390 tests gate strictly from day one.
2. **Fix the deterministic 16**, family by family. Cheapest first:
   `every_windowed_target_declares_test_init` names the ten windowed targets
   missing `mod test_init;`, and closing it is what makes every other row's log
   output attributable.
3. **Shrink Tier 2.** 42 excused names is too many; it is one load measurement,
   and it shrinks as whole-window boots get concurrency pins.

The entry flips to FIXED only when the gate step lands AND the deterministic
tier is empty — a gate that permanently excuses 16 signatures has not closed
this escape.
