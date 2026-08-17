---
id: 2026-08-09-panics-whole-binary-runs-gpui-fork
date: 2026-08-09
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  `frontends/gpui/tests/nested_page_chevron_gate` panics ~2/20 whole-binary
  runs with `Detected activity on thread Some("stub-builder-services") … Your
  test is not deterministic` (gpui fork
  `crates/scheduler/src/test_scheduler.rs:111`), on
  `an_opened_nested_page_paints_its_children` and
  `real_profile_embedded_page_probe`; the same off-thread timing also
  intermittently reds the paint assertion at
  `nested_page_chevron_gate.rs:671`.
source_line: 750
---

## Bug

(task #59 lane, found by running the windowed binary repeatedly, not by one
test run) **`frontends/gpui/tests/nested_page_chevron_gate` panics ~2/20
whole-binary runs with `Detected activity on thread
Some("stub-builder-services") … Your test is not deterministic` (gpui fork
`crates/scheduler/src/test_scheduler.rs:111`), on
`an_opened_nested_page_paints_its_children` and
`real_profile_embedded_page_probe`; the same off-thread timing also
intermittently reds the paint assertion at
`nested_page_chevron_gate.rs:671`.** Those `#[gpui::test]`s build
`ExpandStoreServices::live()`, whose collection driver — spawned by
`watch_query_live` → `start_reactive_views` — runs on `StubBuilderServices`'
process-wide MULTI-THREAD tokio runtime; `run_until_parked()` parks only
gpui's executor and cannot wait on tokio, so whether a
`stub-builder-services` worker is active in the `TestScheduler`'s detection
window is wall-clock luck (failing runs 0.58–2.08s, passing ~0.07s).

## Root cause

task #59 lane, found by a verifier/agent running the windowed binary
repeatedly (not by a single test run): **the
`frontends/gpui/tests/nested_page_chevron_gate` binary panics ~2/20
whole-binary runs with `Detected activity on thread
Some("stub-builder-services") ThreadId(8), but test scheduler is running on
Some("an_opened_nested_page_paints_its_children") … Your test is not
deterministic` at the gpui fork's
`crates/scheduler/src/test_scheduler.rs:111`, hitting
`an_opened_nested_page_paints_its_children` and
`real_profile_embedded_page_probe`; the same off-thread timing also
intermittently reds the paint assertion itself
(`nested_page_chevron_gate.rs:671`, children not yet painted).** Reproduced
2/20 at base (all failing runs slow, 0.58–2.08s; all passing runs ~0.07s — a
wall-clock race, not a logic red). ROOT CAUSE: those `#[gpui::test]`s build
`ExpandStoreServices::live()`, whose collection driver — spawned by
`watch_query_live` → `start_reactive_views` — ran on `StubBuilderServices`'
process-wide MULTI-THREAD tokio runtime (`stub-builder-services`). The test
drives gpui with `run_until_parked()`, which parks only gpui's executor and
cannot wait on tokio, so whether a `stub-builder-services` worker is active
inside the `TestScheduler`'s off-thread detection window is luck.
ENVIRONMENT, not a product bug: the failing path is pure harness/timing
wiring — no product code runs differently — and it is precisely the
async-race-the-settle-masks litmus. The hazard was ALREADY documented for
the scenario proptest in `frontends/gpui/tests/support/mod.rs` (the
`quiescent_runtime` field + `test_quiescent_runtime_handle`: "the default
stub multi-thread runtime trips gpui's `TestScheduler` off-thread
detector"); the chevron binary simply never adopted it. FIXED in-lane by
giving the windowed tests their own quiescent-style runtime: a
`live_windowed()` constructor hands every spawn (collection driver AND
live-follow subscription) a per-fixture CURRENT-THREAD
`tokio::runtime::Runtime`, and a `pump()` drives that runtime on the test
thread between the click and the paint read, so the driver's initial
population runs on-thread (no `stub-builder-services` worker exists to
detect) and deterministically. Faithfulness preserved: it is still the REAL
driver-backed `new_collection` (the point of
`an_opened_nested_page_paints_its_children`), NOT downgraded to the
`new_static_with_layout` shortcut — only its execution is made
deterministic. The `#[tokio::test]` follow-edge tests
(`an_external_unfold_leaves_the_nested_page_closed`,
`an_external_fold_closes_the_nested_page_across_a_rebuild`) keep `live()`
(the ambient multi-thread runtime their `settle().await` drives). PROOF:
base 18/20 with the exact `test_scheduler.rs:111` signature
(`lane-logs/before/`); after fix 20/20 with zero scheduler panics and
`an_opened_nested_page_paints_its_children` — whose assertion requires the
two child rows "buy milk"/"see Journals now" to be painted — green every run
(`lane-logs/after/`); broader `cargo nextest run -p holon-gpui --features
holon-integration-tests/pbt,holon-gpui/pbt -j2 --no-fail-fast` 285/286, all
6 chevron-gate tests PASS, sole red the pre-existing unrelated
`gpui_window_slice::capmap_hosts_windowed_sutlayout_over_real_geometry` (a
matview-ghost/phantom-entity-id invariant, not a scheduler race)
(`lane-logs/windowed-suite.out`). NOVEL: no prior BugFunnel row references
the `TestScheduler` off-thread class; nothing to widen. GAP-NOT-CLOSED,
disclosed: this is a test-infra fix, not a covering PBT — there is no gate
that fails when a NEW windowed target spawns a driver on the multi-thread
stub runtime; the `support/mod.rs` `quiescent_runtime` doc-note remains the
only guidance. Evidence: `lane-logs/before/summary.txt`,
`lane-logs/after/summary.txt`, `lane-logs/windowed-suite.out`.)

## Missing piece

Pure harness/timing divergence — no product code runs differently; the
async-race-the-settle-masks litmus. The exact hazard was already documented
for the scenario proptest in `frontends/gpui/tests/support/mod.rs`
(`quiescent_runtime` / `test_quiescent_runtime_handle`), but the chevron
binary never adopted it. Missing piece = a gate that fails when a NEW
windowed target spawns a driver on the multi-thread stub runtime (NOT closed
— this is a test-infra fix, no covering PBT).

## Remedy

**FIXED in-lane 2026-08-09 (task #59).** A `live_windowed()` constructor
hands every spawn (collection driver and live-follow subscription) a
per-fixture CURRENT-THREAD `tokio::runtime::Runtime`, and `pump()` drives
that runtime on the test thread between the click and the paint read, so the
driver's initial population runs on-thread (no `stub-builder-services`
worker exists to detect) and deterministically. Faithfulness preserved:
still the REAL driver-backed `new_collection` (the point of
`an_opened_nested_page_paints_its_children`), NOT the
`new_static_with_layout` shortcut — only execution is made deterministic.
The `#[tokio::test]` follow-edge tests keep `live()` (the ambient
multi-thread runtime their `settle().await` drives). Proof: base 18/20 with
the exact `test_scheduler.rs:111` signature; after 20/20, zero scheduler
panics, children painted every run; broader `cargo nextest run -p holon-gpui
--features holon-integration-tests/pbt,holon-gpui/pbt -j2 --no-fail-fast`
285/286 (sole red the pre-existing unrelated capmap matview-ghost). Evidence
`lane-logs/before/summary.txt`, `lane-logs/after/summary.txt`,
`lane-logs/windowed-suite.out`.
