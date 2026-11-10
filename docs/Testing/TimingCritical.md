# Timing-critical tests, and the caps of the load-tolerant ones

The landing gate holds the machine quiet while it runs the steps that measure
time, and only those: the hand-authored replay, the windowed gate and the wide
nextest sweep assert no duration of their own, although the composed harness
arms a latency oracle on every transition. So it runs as two scripts on one
tip: `scripts/gate-integrate.sh` runs every other step and builds the timing
binaries, then `scripts/gate-timing.sh` runs `just timing-gate` under the hold
and compiles nothing. The integrate run holds the machine quiet as well, but
only for the wide nextest sweep and the windowed gate, and only when the
batch's selection names one of them: they judge no duration, yet their caps
and paint waits go red on a heavily loaded host. Both scripts run only the
steps a batch can reach; the timing steps run when a changed crate is in
their dependency closure (DEVELOPMENT.md, "Which landing steps a batch
runs"). This file explains the split: which tests really judge a duration,
and what cap the others get.

The code holds the class of a test, not this file. A test is timing-critical
when it calls `DurationBudget::small` (`crates/holon-api/src/duration_budget.rs`),
and `TIMING_OWNERS` in `scripts/timing-split.py` names each such test with the
recipe that runs it. `scripts/timing-split.py` generates the
`.config/nextest.toml` blocks from `TIMING_OWNERS` and from the cap and
load-sensitive tables below. Nothing in that file is hand-edited;
`scripts/timing-split.py check` fails when the committed blocks and the
generated ones differ.

## The four classes

A test's class follows from what its assertion compares, not from how long it
takes.

A **timing-critical** test compares a duration, and the point of the
comparison is that the duration is SMALL: its failure reads "too slow". It
makes that comparison through `DurationBudget::small`, and it runs under the
gate's quiet hold, in the `timing` profile or in a recipe of its own.

A **liveness bound** is an upper bound whose message reads "it never
happened". It proves progress, not speed, so it runs at any load. It is a
`DurationBudget::liveness` check, or a plain deadline in a wait loop.

A **bug-under-load** is a CONTENT assertion whose value changes under load: a
race. It has no duration to compare, so it cannot call `DurationBudget::small`
and cannot enter the timing group. It stays red until it is fixed.

Every other test is **load-tolerant**, also a test that only TIMES OUT under
load. It runs in the `load-tolerant` profile at any load.

Two more kinds of duration comparison need no quiet host. A **lower bound**
(`DurationBudget::lower_bound`, a debounce such as "at least 250 ms passed")
can only pass more easily under load. A duration on a **paused tokio clock**
(`#[tokio::test(start_paused = true)]`) measures the timer schedule, not the
host, because the clock advances only when the runtime idles to its next
timer.

`scripts/check-timing-assertions.sh` (`timing-split.py assertions`) keeps the
classes honest. It finds every duration comparison in assertion position and
every `DurationBudget::small` call in `crates/` and `frontends/`, and
`scripts/timing-assertions-baseline.txt` gives each one its class: `timing`,
`liveness`, `lower-bound`, `virtual-clock` or `harness`. A `timing` row must
be a `DurationBudget::small` call; a raw comparison, or a site that reaches
around the type, fails the check unless its row names one of the other
classes.

## Tests wait for the target of their write, not for a quiet period

A wait that ends after a quiet window passes on a fast host and fails on a
loaded one, so it is a bug-under-load by construction. A test waits for the
state its write must reach instead:

- After a write, `TestEnvironment::settle()` waits until every stage of
  `converge_signals` (Loro, CDC routing and apply, watch registration and
  release, the block feed, org ingest and org write-back) has reached the seq
  that the write produced.
- A test that proves an ABSENCE ("this write did NOT reach the disk") writes a
  sentinel after it with `TestEnvironment::sentinel_writeback`, which waits
  until the file holds the sentinel. Write-back runs in feed order, so every
  earlier write has passed write-back by then.
- Any other condition polls with `polling::wait_until` and a `Probe`. On its
  deadline it fails with `WaitHung`, which names the target, the stage and the
  last value it read.
- A remaining `sleep` in a test names its reason: a poll interval inside a
  target loop, or a product timer that the test drives on purpose.

Product code reads physical time only through the `Clock` port
(`crates/holon-api/src/clock.rs`): `now()`, `sleep`, `timeout` and `interval`
on monotonic `Mono` time, and `now_millis()` for wall time. Product durations
(the SQL actor's hang bound, the Loro boot-gate watchdog) come from `Timings`,
which an embedder sets through `SessionConfig::timings`, never from
environment variables. The archlint smell `physical_time` holds the line: a
new direct `Instant::now`, `tokio::time::sleep` or `thread::sleep` in product
code fails `just gate-arch`, and `archlint/baseline.txt` lists the sites not
yet moved onto the port.

## Timing-critical tests

There is no hand list of timing-critical tests. `scripts/timing-split.py
check` ties `TIMING_OWNERS` to the code in both directions:

- Every test whose own body calls `DurationBudget::small` must be selected by
  a `TIMING_OWNERS` key. A new timing check without its key fails.
- Every `TIMING_OWNERS` key must select at least one such test. A key whose
  test was renamed, deleted or moved to a liveness bound fails.

A key's recipe says what runs the test inside the gate's one quiet-hold step,
`just timing-gate`. A key with the recipe `just timing-gate` is in
`[profile.timing]`'s filter, so `cargo nextest run --profile timing` runs it.
A key with another recipe (`just latency-slo-gate`, `just org-deep-text-timing`)
has that recipe because it adds what nextest cannot, such as running without output capture or
`--ignored`; `just timing-gate` calls the recipe. Every key joins the
`timing-critical` test group, which is a core to itself, and leaves the
load-tolerant complement. So a test never runs twice in one hold.

A generous budget costs the hold some seconds. The test stays in the group
anyway, because the rule must stay mechanical.

The latency gate's teeth and fence deadlines (`TEETH_WRITE_DEADLINE`,
`FENCE_ROUND_PATIENCE`) are liveness bounds: their messages read "produced no
e2e sample" and "still not visible". The gate's timing checks are its service
p95 and its drain completion, both `DurationBudget::small`, and its host
contention cut: a boot DDL mean over the cut makes the host too busy for a
latency verdict. Under `enforce` that cut fails WITHOUT a verdict; under
`measure` it writes its ledger line and the rungs run. A late sample with no
measured value, a window held full and an undelivered drain have no duration
to judge; they stay red as liveness failures.

The drain gate's sweep test
(`latency_drain::tests::the_drain_gate_passes_every_healthy_pipeline_and_fails_every_slow_one`)
is not timing-critical. It sweeps synthetic pipelines whose per-row times its
own RNG draws, and asserts a content result over that simulation, so load
cannot move its verdict. It is merely slow, which is why it has a cap of its
own. A MEASURED drain is judged in `just latency-slo-gate`.

The latency ORACLES of the composed harness (`inv-settle-budget` and
`inv-sql-budget`'s wall, query and RSS ceilings) fire inside every composed
run, so no test filter selects them. They write the same `MEASURE-ONLY` line
and ledger line through `timing_mode::record`
(`crates/holon-api/src/timing_mode.rs`), and `HOLON_PBT_TIMING` sets their
mode with every other check: see "The recipes" below.

## Caps of the load-tolerant tests

A raised cap must still catch a hang, or the split trades a quiet host for a
blind gate. The default layout seed's test is the proof: a real 120 s hang,
reproduced on BOTH legs of an A/B, 3/3, at load under 14. A generous cap would
have hidden it — and hidden it as a
"load artifact", the most expensive kind of false green, because it arrives with
an explanation.

```
cap = clamp(ceil(K × p95_low), 60 s, 8 × p95_low)      K = 4
```

Three refusals:

1. **No low-load p95 means no raise.** A test that does not COMPLETE at low load
   has no p95, keeps the default 120 s cap, and stays red. A TIMEOUT is a
   CENSORED measurement, not a slow one, and a censored measurement buys no
   headroom. This is what catches that hang by construction.
2. **No cap above `8 × p95_low`.** The clamp is the hang detector. A test whose
   honest high-load inflation exceeds 8× is not slow — its work scales with
   load, which is a defect to report, not to pad.
3. **Every cap cites its evidence** in its override comment:
   `p95_low = <n>s (<m> runs, load < 16, <date>)` plus the resulting multiple.
   A cap with no citation fails the check.

### How `p95_low` is measured

The 95th percentile of per-test wall time over runs that COMPLETED (green or
red, never TIMEOUT) with the 1-minute load average **< 16** at start and
throughout, one test per nextest process, `NEXTEST_TEST_THREADS=1`, at least
5 completed runs. Load < 16 is the quiet condition of the A/B runs the ledger
was seeded from (observed 4.2-15.0 over 84 runs), so the two data sets are
comparable.

Collected from nextest's JUnit output: `[profile.load-tolerant.junit]` plus
`scripts/timing-split.py record <junit.xml> <load_start> <load_max>`, which appends
`date, commit, test, secs, load_start, load_max, verdict` to
`~/.holon-gate/test-duration-ledger.tsv` — the same home and append-only TSV
shape as `windowed-ledger.tsv` (`scripts/windowed-gate.py:156`), which records
verdicts but no durations.

**The load is the caller's, and it is required.** `just load-tolerant-nextest`
samples the 1-minute load every 30 s across the run and passes the start and
the maximum. One reading taken after the run ends cannot describe an
8391-test sweep that spanned an hour at load 30 to 215 — and stamped wrong it
is worse than missing: the first run of this recipe stamped its rows at the
post-run load of 8.39, which made a 479 s high-load duration enter the ledger
as a QUIET measurement and moved one test's `p95_low` from 121 s to 479 s. At
`cap = 4 x p95_low` that is a 1916 s cap, derived from a number no quiet host
ever produced. The `< 16` filter in `p95_low` IS the hang detector, so it can
only work on an honest stamp. Re-stamped at its real load, the same 8391 rows
feed no `p95_low` at all and serve their actual purpose: the first uncensored
high-load durations, which is what calibrates K.

`scripts/timing-split.py check` recomputes `p95_low` from the ledger's last
30 days and fails when a committed cap leaves the clamp **in either direction**,
so a cap cannot creep and a test that got faster gives its headroom back. The
hang detector tightens as the tree improves.

### Why K = 4, and how it gets calibrated

K = 4 is **tighter than every cap in the tree today**: the existing overrides
run 5× to 12× over their own stated quiet measurements
(`gpui_composed_windowed_loop` 48-65 s quiet against a 600 s cap;
`two_instance_composed_pbt` against 20 min). Adopting the rule ratchets the
existing caps DOWN. The current caps are round numbers, not measurements.

K cannot be derived from the data in hand, and this file does not pretend
otherwise: every test that inflated past its cap at gate load **timed out**, so
its high-load duration is censored. The uncensored pairs available are all at
load < 16 and show inflation of only 1.0-1.5×.

So K = 4 lands as a starting value and the FIRST gate after it is the
calibration run: that gate runs `--profile load-tolerant` at real lane load with
JUnit on, which yields the first uncensored high-load durations per test. Then

```
K := ceil(1.5 × max(p95_high / p95_low))     over the ledger
```

reported as a number with its evidence. **If that lands above 8 the clamp is
wrong rather than K, and that is a card, not a silent widening.**

### Current caps

`Test` is the FULL nextest test name — its module path included, the binary
name excluded — because the generated override filters it with `test(=…)`, an
exact match. `split_block_stale_display_regression` is a module inside the
`frontend_suite` module, so the bare name selects nothing and the cap would be
silently absent. `scripts/timing-split.py resolve` is what catches that: it
asks nextest which tests each generated filter selects and fails on an empty
one. It needs the test binaries built, so it runs inside
`just load-tolerant-nextest`, where they already are, not in `gate-arch`.

| Test | `p95_low` | Cap | Evidence |
| --- | --- | --- | --- |
| `frontend_suite::split_block_stale_display_regression::split_block_stale_viewmodel_display_regression` | 121 s | 484 s | 6 completed runs, load 4.8-8.6, 2026-10-19 |
| `projector_lag_lock::convert_block_to_page_survives_a_lagging_projection` | 108 s | 432 s | 6 completed runs, load 5.5-9.9, 2026-10-19 |

## Load-sensitive binaries

The landing gate's workspace nextest leg runs in parallel at whatever load the
lanes produce. Some binaries there are not timing-critical by the four classes
above, yet red or time out when many of them share the machine: the holon-gpui
binaries drive a real GPUI window, and the cost binaries time their own work. A
test group with `max-threads` caps how many of each run at once, so the leg
stays parallel and stops making its own reds.

`scripts/timing-split.py generate` writes one group and one override per
group from the two tables below. A test holds one group, and the first
matching override wins: the timing-critical override comes first, so a test
that is in both tables stays timing-critical. `scripts/timing-split.py
resolve` asks nextest for each row's tests and fails when a row selects none
or when one of its tests is in no group.

| Group | `max-threads` | Why this cap |
| --- | --- | --- |
| `windowed` | 4 | the concurrency `just windowed-gate` judges the windowed suite at: `--test-threads 1` per binary, `min(4, cpus / 4)` binaries at once (`scripts/windowed-gate.py:33`) |
| `cost-sensitive` | 1 | the rule of the `timing-critical` group: a test that times its own work never contends with another |

| Selector | Group | Evidence |
| --- | --- | --- |
| `package(holon-gpui)` | `windowed` | the whole package, as `just windowed-gate` runs it (`scripts/windowed-gate.py` `build`): binaries outside any name rule open a real window too, and `gpui_rebind_reset_smoke` went red in a loaded run; `general_e2e_composed_pbt_windowed` passed alone in 379 s and timed out at 600 s inside the parallel workspace leg; known reds `windowed-focus-not-in-bounds-under-load`, `windowed-entity-not-in-bounds-under-load` (KeystoneKnownReds.md) |
| `binary(/_cost/) + (kind(test) & test(/^[a-z0-9_]*_cost[a-z0-9_]*::/))` | `cost-sensitive` | a `_cost` binary, or a `_cost` module of a crate's one test binary; most time their work against a ceiling or a large-vs-small ratio; `find_foreign_blocks_cost`, `stable_id_index_cost` and `home_authority_ancestor_walk_cost` count operations and join by the name rule only; known red `shopping-mapping-cost-slo`: ~5 s alone, red at load average 34 in a land leg |
| `binary_id(holon-integration-tests::integration) & test(/^latency_scale_gate::/)` | `cost-sensitive` | judged against `docs/Testing/latency-scale-ceilings.txt`; skips unless `HOLON_SOAK_SEED_BLOCKS` is set |
| `binary_id(holon-integration-tests::integration) & (test(/^projector_lag_lock::/) + test(/^[a-z0-9_]+_under_lag::/))` | `cost-sensitive` | the lag locks the gate's quiet-hold step runs (`just projector-lag-lock`); `convert_block_to_page_survives_a_lagging_projection` measures p95_low 108 s (Current caps) |
| `binary_id(holon-integration-tests::integration) & test(/^loro_suite::loro_block_type_invalid_values::/)` | `cost-sensitive` | a 20 s wait for the edit to reach the file; red "the edit never reached the file" at load ~90 in a land leg |

## The latency rung and its kind coverage

The p95 < 200 ms interaction→projection-visible SLO is a project rule, so
one step under the hold judges it over generated transitions: the rung. While
`just timing-gate` runs in `measure`, the rung reports each breach as a
`MEASURE-ONLY` line and a ledger line; when the recipe goes back to `enforce`,
a breach is red.

**A hand-authored subset cannot be that rung.** The budgeted kind universe is
`transition_key()` (`crates/holon-integration-tests/src/pbt/transition_budgets.rs:255`)
over `E2ETransition::VARIANT_NAMES` (125 variants, `ApplyMutation` split by its
5 mutations) = **129 kinds**. The hand-authored corpus
(`crates/holon-integration-tests/hand-authored-regressions/keystone.jsonl`,
291 cases) exercises **81** of them; **48 are absent from the corpus
entirely**, `SharePage` among them — the kind of a real gate breach
(`'SharePage' took 7292ms`). A rung built from that corpus would not cover it.

So the enforced rung is the generated keystone, which does draw from the whole
alphabet. `scripts/timing-split.py coverage` checks it, and the threshold is ONE
constant:

```python
KIND_COVERAGE_MIN = 1.0        # fraction of the 129 budgeted kinds
KIND_COVERAGE_WINDOW = 20      # runs the coverage accumulates over before it JUDGES
```

`scripts/timing-split.py coverage <log...>` given FEWER logs than the window
prints a WARN that lists every kind that run missed and exits 0; given at least
the window it judges and reds. So `just timing-gate`'s own single-log call
warns — one random draw cannot touch 129 kinds, and a hard red per run would
only teach agents to ignore it — while the accumulated coverage keeps its
teeth. 20 is a starting value and is calibrated the way K is: the first gates'
logs say how many random runs the alphabet needs.

A deterministic tour of every transition removes the guess: it draws every
kind every time, so the window goes to 1, the gate's own call starts judging,
and the check becomes exact.

## The recipes, and which of them the hold covers

Every recipe runs in `measure`, so no lane or gate goes red because a
duration is over its budget: a breach is a `MEASURE-ONLY` line and a ledger
line. An unset `HOLON_PBT_TIMING` means `measure`
(`crates/holon-api/src/timing_mode.rs`). `just timing-gate` is the one step
under the quiet hold. It sets `export HOLON_PBT_TIMING=measure` for itself and
for the recipes it calls (`just latency-slo-gate`, `just org-deep-text-timing`,
`just keystone-smoke`). Making the gate judge durations again is that one line
set to `enforce`. The other recipes (`just hand-authored`, `just windowed-gate`,
`just load-tolerant-nextest`) keep a caller's value and default to `measure`.

`just hand-authored`, `just windowed-gate` and `just load-tolerant-nextest` end
in `scripts/assert-measure-mode.sh` when they ran in `measure`, which
asserts two things its log can actually prove: that the step really ran in
`measure` (the recipe echoes `HOLON_PBT_TIMING=` into the log first), and that
no ENFORCE-shaped timing verdict appears — in `measure` a breach can only
surface as a `MEASURE-ONLY` line, so `[inv-settle-budget] … took …ms`, or an
`[inv-sql-budget]` failure naming a wall, query, settle or RSS ceiling, ANYWHERE
in a line — the composed harness Debug-quotes a failed invariant inside its
panic — means the mode did not reach that oracle. It does NOT demand that a
`MEASURE-ONLY` line be present: a step whose every transition stayed inside its
budget emits none, and that is the honest green. The count is reported either
way.

The gate script that places these steps must export `HOLON_GATE_ID=$GATE`,
which is what stamps the measure ledger; without it every line reads `local`.

## One compile shape for every gate step and every rerun

A cargo call builds new units, and so compiles, when its build options or the
value of a variable that a build script reruns on differ from the call before.
The timing gate must compile nothing under the quiet hold, so every step of
`scripts/gate-integrate.sh` and `scripts/gate-timing.sh`, every lane check and
every rerun builds ONE shape:

- The build options are the justfile's `CANON`. A recipe narrows what RUNS
  with `--test <name>` or `-E`, never with `-p` or its own `--features`.
- The build-script env comes from ONE sourced helper,
  `scripts/cargo-shape-env.sh` (`gate` or `lane`). It sets
  `RUST_FONTCONFIG_DLOPEN` from the justfile, refuses a set
  `HOLON_BUILD_TIME`, and in `gate` mode pins `HOLON_BUILD_SHA` to the
  integrated tip (`@-`, with an empty `@`). Lanes leave `HOLON_BUILD_SHA`
  unset. The gate scripts, `scripts/flow-checks/run-step.sh`,
  `scripts/lane-nextest.py` and `gate-integrate.sh rerun` source it, and their logs
  print the values of these variables and of `RUSTFLAGS`, and only the NAMES
  of the set `CARGO_*` variables.
- The helper puts `scripts/cargo-shape/cargo` first on `PATH`; it records each
  cargo call with its shape env. Step 17 of `gate-timing.sh` runs
  `scripts/check-compile-shape.py` on that record before
  `just timing-gate-build`, and fails the gate on a second shape. The calls
  that are off the shape by necessity (app launches, `--target` triples,
  out-of-workspace manifests, the release shape) are its `EXCEPTIONS` list,
  the one place that names them.

Rerun gate reds in the integration checkout with `scripts/gate-integrate.sh
rerun <gate-id> <tip> '<filterset>'`: it sources the helper in `gate` mode,
runs the gate's extra nextest step on those tests alone, and appends its cargo
call to the gate's record, so step 17 checks it and the
`just timing-gate-build` after it compiles nothing.

## Measure mode is never silent

Every `DurationBudget::small` check and every error-level sample of a latency
oracle appends one line to the ledger, in both modes and whether or not it is over budget:
`~/.cache/holon-build/timing-checks.jsonl`, or the file that
`HOLON_TIMING_MEASURE_LOG` names. A line is one JSON object with exactly the
keys `gate`, `test`, `quantity`, `value`, `budget`, `unit` (`ms` or `B`),
`load` and `over`. `HOLON_GATE_ID` stamps the gate; a lane run stamps `local`.
`test` is `NEXTEST_TEST_NAME` when nextest runs the test.

In `measure`, an over-budget check also prints

```
[MEASURE-ONLY] <test> <quantity> <value><unit> over the <budget><unit> budget (load <load>) — measured, not judged (HOLON_PBT_TIMING=measure)
```

`MEASURE-ONLY` is the grep handle `scripts/assert-measure-mode.sh` counts in
each measure-mode step's log.

`scripts/timing-breaches.py` reads the ledger and prints one block per gate;
`scripts/gate-timing.sh` prints its gate's block at its end, and
`scripts/gate-integrate.sh` prints it when the gate is red. Fields are
separated by tabs:

```
== timing gate<N>: <k> over of <n> checks ==
over	<test>	<quantity>	<value>	<budget>	<unit>	<load>	<values of the last 5 gates>
TREND	<test>	<quantity>	<value>	<median>	<unit>	gate<A>,gate<B>,gate<N>
== end gate<N> ==
```

There is one `over` line per over-budget check of the gate. A `TREND` line
names a (test, quantity) that is over budget in each of its last 3 gates and
whose value in this gate is at least 1.5 times the median of up to 10 earlier
gates: a real slowdown, from which the steward cuts a card without reading a
log. The exit code reports only bad input: a malformed line, a missing or
unreadable ledger or a bad GATE argument exits 2, a malformed line with
`<path>:<line>: <reason>`, a ledger it cannot read with `<path>: <reason>`.
