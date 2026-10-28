# Timing-critical tests, and the caps of the load-tolerant ones

The landing gate holds the machine quiet while it runs the steps that measure
time, and only those: the hand-authored replay, the windowed gate and the wide
nextest sweep assert no duration of their own, although the composed harness
arms a latency oracle on every transition. So it runs as two scripts on one
tip: `scripts/gate-integrate.sh` runs every other step at any load and builds
the timing binaries, then `scripts/gate-timing.sh` runs `just timing-gate`
under the hold and compiles nothing. This file is the source of truth for
the split: which tests really do judge a duration, and what cap the others get.

`scripts/timing-split.py` reads the two tables below and generates the
`.config/nextest.toml` blocks from them. Nothing in that file is hand-edited;
`scripts/timing-split.py check` fails when the committed blocks and the
generated ones differ.

## The four classes

A test's class follows from what its assertion compares, not from how long it
takes.

| Class | Rule | Where it runs |
| --- | --- | --- |
| **timing-critical** | The assertion compares a duration AND the point is that the duration is SMALL. Its failure reads "too slow". | the `timing` profile, under the gate's quiet hold |
| **load-tolerant** | Everything else, including a test that only TIMES OUT under load. | the `load-tolerant` profile, at any load, with `HOLON_PBT_TIMING=measure` |
| **liveness bound** | An upper-bound duration whose message reads "it never happened". It proves progress, not speed. Load-tolerant. | as above |
| **bug-under-load** | A CONTENT assertion whose value changes under load. This is a RACE. It stays load-tolerant and stays RED until it is fixed. | as above |

Three consequences worth stating:

- **A lower-bound duration assertion is load-insensitive.** Load can only make
  it pass. `assert!(start.elapsed() >= Duration::from_millis(250))` — a debounce
  — needs no quiet host.
- **A duration measured on a PAUSED tokio clock is load-insensitive too**, and
  more strongly: under `#[tokio::test(start_paused = true)]` the clock advances
  only when the runtime idles to the next timer, so `tokio::time::Instant`
  measures the timer schedule, not the host. The assertion ratchet classes such
  a site `virtual-clock`, and it stays out of the timing group. Four sites are
  `virtual-clock` today, all in `holon-mcp-client`'s timeout bars
  (`http_bounds.rs`, `subscribe_bounds.rs`, `mcp_call_surface.rs`); the one
  mcp-client bar that does keep a real `std::time::Instant`
  (`oauth_bootstrap_flow.rs:899`) is in the table below.
- **A bug-under-load may never be parked in the timing group.** Every row of
  the timing table must name the `file:line` of its duration assertion, and the
  check REJECTS a row whose named site holds no duration assertion. A content
  flake has no duration to point at, so the mechanism refuses it rather than
  relying on a reviewer to notice. Open examples: an edited quantity that reads
  200 instead of 250, a real 120 s hang in the default layout seed, and the
  `inv-two-instance-convergence` divergence that a latency red was concealing.

## Timing-critical tests

`Runs` says WHAT runs a row inside the gate's one quiet-hold step
(`just timing-gate`). All three values are timing-critical, so all three join
the `timing-critical` test group — a core to themselves — and all three leave
the load-tolerant complement.

| `Runs` | Run by | Why not the other |
| --- | --- | --- |
| `timing-step` | `cargo nextest run --profile timing` | — |
| `own-recipe` | its own recipe in `just timing-gate` (`just latency-slo-gate`, `just org-deep-text-timing`) | the recipe adds what nextest cannot: a wrong-tree assertion, `--no-capture`, `--ignored` |
| `release-bench` | nothing in a debug gate | an `#[ignore]`d release bench; it is listed so the class rules and the assertion ratchet still cover it |

So `[profile.timing]`'s filter is the `timing-step` rows and the test group's
is all of them. Selecting an `own-recipe` row in both would run the 600-write
drain twice in one hold.

| Selector | Runs | Assertion | Asserts | Headroom |
| --- | --- | --- | --- | --- |
| `binary_id(holon-integration-tests::integration) & test(/^latency_slo_gate::/)` | own-recipe | `crates/holon-integration-tests/tests/latency_slo_gate.rs:1076` | `TEETH_WRITE_DEADLINE` over the 600-write drain | thin by design |
| `binary_id(holon-integration-tests::integration) & test(/^latency_slo_gate::/)` | own-recipe | `crates/holon-integration-tests/tests/latency_slo_gate.rs:1211` | `FENCE_ROUND_PATIENCE` per fence round | thin by design |
| `binary_id(holon-org-format::deep_authored_text_never_aborts) & test(=a_marker_line_costs_time_linear_in_its_length)` | own-recipe | `crates/holon-org-format/tests/deep_authored_text_never_aborts.rs:205` | marker-line parse cost linear in length | thin; `#[ignore]`d, run by `just org-deep-text-timing` |
| `binary_id(holon-org-format::deep_authored_text_never_aborts) & test(=a_marked_line_renders_in_time_linear_in_its_length)` | own-recipe | `crates/holon-org-format/tests/deep_authored_text_never_aborts.rs:244` | marked-line render cost linear in length | thin; `#[ignore]`d, run by `just org-deep-text-timing` |
| `test(=turso_storage_repros::tabs_main_panel_delivery::cursor_filtered_main_panel_delivers_at_vault_scale)` | timing-step | `crates/holon/tests/turso_storage_repros/tabs_main_panel_delivery.rs:184` | cold matview create + first read < 5 s at ~70 pages | thin: 853 ms alone, 4.74 s at load 46, 7.52 s in an 885-test run |
| `package(holon-rows) & test(=a_number_of_many_digits_is_an_error_naming_the_limit_not_minutes_of_bigint_work)` | timing-step | `crates/holon-rows/tests/mapper_bounds.rs:338` | 500 000 digits refused in < 2 s — a superlinear-parse DoS bound | thin |
| `package(holon-loro) & test(=loro_sync_controller::boot_gate_tests::dropped_gate_is_observed_before_the_watchdog)` | timing-step | `crates/holon-loro/src/loro_sync_controller.rs:2566` | a dropped gate is observed before `WATCHDOG` = 200 ms, after a 30 ms sleep | **very thin: 170 ms** |
| `package(holon-loro) & test(=loro_share_backend::tests::settle_stays_bounded_while_the_sync_worker_dials_an_unreachable_peer)` | timing-step | `crates/holon-loro/src/loro_share_backend.rs:4921` | settle stays under 3 s while the worker dials an unreachable peer | moderate |
| `package(holon) & test(=api::backend_engine::tests::deriving_the_net_is_cheap_enough_for_the_tick_path)` | timing-step | `crates/holon/src/api/backend_engine.rs:3302` | one net derive costs < 5 ms on the tick path | thin |
| `package(holon) & test(=block_live_data_catch_up::wakes_when_blocks_arrive_via_cdc)` | timing-step | `crates/holon/tests/block_live_data_catch_up.rs:124` | the wait wakes on CDC arrival in < 2 s rather than poll-spinning | moderate |
| `package(holon) & test(=watch_context_membership_leak::a_failed_watch_setup_leaves_no_membership_row)` | timing-step | `crates/holon/tests/watch_context_membership_leak.rs:218` | the failure path returns in < 20 s rather than parking in a dependency wait | generous |
| `package(holon-app) & test(=integration_set_field_op::dispatching_begin_oauth_returns_before_the_flow_finishes)` | timing-step | `crates/holon-app/tests/integration_set_field_op.rs:401` | `begin_oauth` returns in < 1 s, once STARTED rather than once finished | moderate |
| `package(holon-app) & test(=quick_open_search_at_vault_scale::quick_open_search_at_vault_scale)` | timing-step | `crates/holon-app/tests/quick_open_search_at_vault_scale.rs:165` | each keystroke's search returns in < 6 s, the join-regression bound | thin: 0.8-5.3 s across quiet and loaded runs |
| `package(holon-loro) & test(=stable_id_index_create_cost::a_create_costs_the_same_in_a_large_tree_as_in_a_small_one)` | timing-step | `crates/holon-loro/tests/stable_id_index_create_cost.rs:64` | a create in a large tree costs at most 3x one in a small tree | thin |
| `package(holon-mcp) & test(=tools::dense_patch_readback_tests::a_patch_that_does_not_read_back_as_edited_is_rolled_back_by_name)` | timing-step | `frontends/mcp/src/tools.rs:10382` | an inexact apply answers within its 5 s limit | generous |
| `package(holon-mcp) & test(=describe_ui_deferred::fan_out_expansion_is_bounded_and_exhaustion_is_disclosed)` | timing-step | `frontends/mcp/tests/describe_ui_deferred.rs:320` | a bounded fan-out pass finishes in < 5 s | generous |
| `package(holon-mcp-client) & test(=oauth_bootstrap_flow::a_connected_but_silent_peer_cannot_pin_the_flow_past_its_timeout)` | timing-step | `crates/holon-mcp-client/tests/oauth_bootstrap_flow.rs:899` | a 400 ms timeout returns within 5 s | generous |
| `package(holon-loro) & test(=typed_entity_scale::shape_cost_at_ten_thousand_entities)` | release-bench | `crates/holon-loro/tests/typed_entity_scale.rs:348` | read p95 < 200 ms at 10 000 entities — the SLO itself | thin |
| `package(holon-loro) & test(=typed_entity_scale::shape_cost_at_ten_thousand_entities)` | release-bench | `crates/holon-loro/tests/typed_entity_scale.rs:354` | write p95 < 5 ms at 10 000 entities | thin |

The generous-headroom rows cost the hold seconds. They stay in the group
anyway: the rule must not acquire a "but the headroom is big" exception, or it
stops being mechanical.

The latency gate's two sites are `timing`, not `liveness`, although both
messages read "produced no e2e sample" / "still not visible". `TEETH_WRITE_DEADLINE`
and `FENCE_ROUND_PATIENCE` bound a *teeth* rung whose whole purpose is that the
wait is short, so the deadline IS the judgement.
The p95 the gate reports is judged one layer down, in
`crates/holon-api/src/latency_slo.rs:487`, through a `RungVerdict` — not a
comparison in assertion position, so the ratchet's line scanner does not see
it, and the row is held to its own named site instead.

**The drain gate is not in this table.**
`latency_drain::tests::the_drain_gate_passes_every_healthy_pipeline_and_fails_every_slow_one`
sweeps 1500 SYNTHETIC pipelines whose per-row microseconds are drawn by its own
RNG, and asserts `wrong.is_empty()` — a content assertion over a deterministic
simulation. `SERVICE_TIME_SLO_MS` bounds the simulated rows, never a measured
wall clock, so load cannot move the verdict. The sweep is merely SLOW
(~27 s idle, past 120 s on a loaded host), which is class load-tolerant-slow
and is why it already has a cap of its own. `just timing-gate` still runs
`just latency-slo-gate`, which is where a MEASURED drain is judged.

**The latency ORACLES are not in this table.** `inv-settle-budget` and
`inv-sql-budget`'s wall / query / RSS ceilings fire inside every composed run,
so they cannot be selected by a test filter. `HOLON_PBT_TIMING` governs them
instead (`crates/holon-integration-tests/src/pbt/timing_mode.rs`): `enforce` in
the hold, `measure` everywhere else. They are why the hold was 2 h 51 m.

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

## The enforced latency rung and its kind coverage

The p95 < 200 ms interaction→projection-visible SLO is a project rule, so
something under the hold must still ENFORCE it. With every other step in
`measure`, that is the rung's whole job.

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

| Recipe | `HOLON_PBT_TIMING` | Under the quiet hold |
| --- | --- | --- |
| `just timing-gate` | `enforce`, pinned | **yes — the one hold step** |
| `just latency-slo-gate` | `enforce`, pinned (its subject) | inside `timing-gate` |
| `just org-deep-text-timing` | `enforce`, pinned (its subject) | inside `timing-gate` |
| `just keystone-smoke` | the default, `enforce` | inside `timing-gate` |
| `just hand-authored` | `measure` by default | no |
| `just windowed-gate` | `measure` by default | no |
| `just load-tolerant-nextest` | `measure` by default | no |

`enforce` is the DEFAULT of the variable, so a recipe that pins it is guarding
against a caller that exported `measure` for the surrounding gate, not against
omission (`crates/holon-integration-tests/src/pbt/timing_mode.rs`).

Each measure-mode recipe ends in `scripts/assert-measure-mode.sh`, which
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

## Measure mode is never silent

Every budget breach in `measure` is printed as

```
[<quantity> MEASURE-ONLY] '<subject>' <value><unit> over the <budget><unit> budget — measured, not judged (HOLON_PBT_TIMING=measure)
```

and appended to `~/.cache/holon-build/timing-measure.jsonl` as
`{"gate","quantity","subject","value","budget","unit"}`, so a trend can show a
regression the enforced rung missed. `HOLON_GATE_ID` stamps the gate; a lane run
stamps `local`.

`MEASURE-ONLY` is the grep handle `scripts/assert-measure-mode.sh` counts in
each measure-mode step's log.
