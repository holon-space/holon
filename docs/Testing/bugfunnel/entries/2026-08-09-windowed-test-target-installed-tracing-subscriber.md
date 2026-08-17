---
id: 2026-08-09-windowed-test-target-installed-tracing-subscriber
date: 2026-08-09
gap: PERCEPTION
secondary: ENVIRONMENT
status: FIXED
summary: >-
  No windowed test target installed a tracing subscriber — all 49
  `frontends/gpui/tests/*.rs` binaries, plus the shared `tests/support/mod.rs`
  and `tests/pbt_harness/*`, ran under `tracing`'s no-op global dispatcher, so
  every `warn!`/`error!` a widget, driver or background worker emitted during
  a windowed test was discarded before any layer saw it.
source_line: 754
---

## Bug

(task #43 lane, found by code exploration while root-causing why the
`__NO_PATH__/` sentinel degradation survived six rounds of #27) **No
windowed test target installed a tracing subscriber — all 49
`frontends/gpui/tests/*.rs` binaries, plus the shared `tests/support/mod.rs`
and `tests/pbt_harness/*`, ran under `tracing`'s no-op global dispatcher, so
every `warn!`/`error!` a widget, driver or background worker emitted during
a windowed test was discarded before any layer saw it.** The headless side
is the opposite: `test_tracing::SpanCollector::global()` is its sole
installer and feeds `inv-no-observed-errors`. Effect: the "falls back
VISIBLY" tier — ranked ABOVE "fails with a clear error" — was unenforceable
in the windowed suite, and no windowed rung could notice an ERROR that did
not also break an assertion.

## Root cause

task #43 lane, found by code exploration while root-causing why the
`__NO_PATH__/` sentinel degradation survived six rounds of #27: **not one of
the 49 `frontends/gpui/tests/*.rs` targets installed a tracing subscriber,
so every `warn!` and `error!` emitted during a windowed test was discarded
by `tracing`'s no-op global dispatcher before any layer could see it** —
harness=true and harness=false alike, `tests/support/mod.rs` (26 targets)
and `tests/pbt_harness/*` included. The headless side had the opposite:
`test_tracing::SpanCollector::global()` is the sole installer, with
per-`TestScope` routing feeding `inv-no-observed-errors`. The consequence is
not one bug but a whole tier of the error philosophy being unenforceable in
the windowed suite: "falls back VISIBLY" is ranked ABOVE "fails with a clear
error", yet a windowed test could not tell a correct disclosed degradation
from a silent one, and every windowed rung was structurally incapable of
noticing an ERROR that did not also break an assertion. PERCEPTION: the
windowed rungs RAN the emitting code and the disclosure fired correctly — no
instrument existed to record it, so no assertion of any class could be
written. Secondary ENVIRONMENT: production installs a subscriber at every
entry point (`frontends/gpui/src/mobile.rs`, the desktop main) and the
windowed test environment did not, so the two differ in exactly the wiring
that makes logs exist. FIXED in-lane:
`frontends/gpui/tests/test_init/mod.rs` — a `#[ctor::ctor]` that calls
`SpanCollector::global()` before `main`/the libtest harness, wired into all
49 targets by a one-line `mod test_init;`, with
`windowed_log_capture.rs::every_windowed_target_declares_test_init` failing
the suite by name if a NEW target omits it. Idempotence rests on the
existing `OnceLock`, NOT on a `try_init` whose discarded `Err` would be the
same silent-degradation the lane exists to remove. The shared collector
gained a SECOND window: `ProblemCaptureLayer` now admits WARN and routes by
level, ERROR+panic to `captured_problems()` (unchanged —
`inv-no-observed-errors` reds on it) and WARN to the new
`captured_warnings()` (read only on demand). POLICY, mirroring the keystone
deliberately: ERROR reds, WARN is observable and never fatal — a blanket
WARN gate would red the profile-condition DEGRADED warning, the stale-home
retire refusal and ply's no-query-engine fallback, i.e. exactly the
disclosures the philosophy asks for, and would create pressure to downgrade
them to INFO. Red-for-the-right-reason: `a WARN emitted during render must
be capturable in a windowed test; captured warnings: []` and `an ERROR
emitted during render must land in the problem window; problems: []`, 2
failed / 1 passed, green 3/3 after
(`lane-logs/43-red-windowed-log-capture.log`,
`43-green-windowed-log-capture.log`). ONE REGRESSION CAUGHT BY THE GATE AND
FIXED, worth recording because it is the same class: the collector's
human-readable layer used `with_test_writer()`, i.e. stdout — harmless while
only `harness = true` binaries installed it, but a `harness = false`
binary's stdout carries libtest's list protocol, and the first full-suite
run died with `creating test list failed … did not end with the string ":
test"` on `reactive_vm_test`. The layer now writes to stderr, which nextest
captures per test; a subscriber must not be able to corrupt a machine
protocol.)

## Missing piece

PERCEPTION: the windowed rungs RAN the emitting code and the disclosure
fired correctly; no instrument existed to record it, so no assertion of any
class could be written. Missing piece = a capturing subscriber installed
before the SUT, plus a WARN window distinct from the problem window.
Secondary ENVIRONMENT: production installs a subscriber at every entry point
(`frontends/gpui/src/mobile.rs`, the desktop main) and the windowed test
environment did not — the two differ in exactly the wiring that makes logs
exist.

## Remedy

**FIXED in-lane 2026-08-09 (task #43).**
`frontends/gpui/tests/test_init/mod.rs`: a `#[ctor::ctor]` calling
`SpanCollector::global()` before `main`/the libtest harness, wired into all
49 targets by a one-line `mod test_init;` — structural, because a per-test
call site is both forgettable and too late (the SUT emits during
construction and from background threads).
`windowed_log_capture.rs::every_windowed_target_declares_test_init` names
any target that omits the declaration, so a new windowed target cannot
silently reintroduce the discard. Idempotence rests on the existing
`OnceLock`, NOT on `try_init` — a discarded `Err` there would be the same
silent degradation this row is about. The shared collector gained a second
window: `ProblemCaptureLayer` (was `ErrorCaptureLayer`) admits WARN and
routes by level — ERROR+panic to `captured_problems()` (unchanged), WARN to
`captured_warnings()`; no perf regression, since the registry's
`max_level_hint` is already INFO from the OTel layer. POLICY, mirroring the
keystone: **ERROR reds, WARN is observable and never fatal** — a blanket
WARN gate would red the profile-condition DEGRADED warning, the stale-home
retire refusal and ply's no-query-engine fallback, i.e. the disclosures the
philosophy asks for. Red-for-the-right-reason
`lane-logs/43-red-windowed-log-capture.log` (`captured warnings: []` /
`problems: []`, 2 failed / 1 passed), green 3/3 in
`43-green-windowed-log-capture.log`. Regression caught by the full-suite
gate and fixed, same class: the collector's human-readable layer wrote to
STDOUT (`with_test_writer()`), which is libtest's list protocol in a
`harness = false` binary — the first suite run died `creating test list
failed … did not end with the string ": test"` on `reactive_vm_test`; the
layer now writes to stderr, which nextest captures per test.
