# Timing-critical tests, and the caps of the load-tolerant ones

The landing gate holds the machine quiet while it runs the steps that measure
time. Before #475 that hold was 2 h 51 m, and **90 % of it asserted no duration
at all** — the hand-authored replay, the windowed gate and the wide nextest
sweep were there only because the composed harness arms a latency oracle on
every transition. This file is the source of truth for the split: which tests
really do judge a duration, and what cap the others get.

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

Two consequences worth stating, because both were breached before #475:

- **A lower-bound duration assertion is load-insensitive.** Load can only make
  it pass. `assert!(start.elapsed() >= Duration::from_millis(250))` — a debounce
  — needs no quiet host.
- **A bug-under-load may never be parked in the timing group.** Every row of
  the timing table must name the `file:line` of its duration assertion, and the
  check REJECTS a row whose named site holds no duration assertion. A content
  flake has no duration to point at, so the mechanism refuses it rather than
  relying on a reviewer to notice. Open examples: #459 (an edited quantity reads
  200 instead of 250), #458 (a real 120 s hang), and the
  `inv-two-instance-convergence` divergence that a latency red was concealing.

## Timing-critical tests

`Runs` says WHERE a row is judged. `timing-step` rows make up the gate's
quiet-hold step and are what the generated nextest profiles select.
`release-bench` rows assert a duration too, so the class rules and the
assertion ratchet cover them, but they are `#[ignore]`d release benches that a
debug gate step must not run.

| Selector | Runs | Assertion | Asserts | Headroom |
| --- | --- | --- | --- | --- |
| `binary(latency_slo_gate)` | timing-step | `crates/holon-integration-tests/tests/latency_slo_gate.rs:1076` | `TEETH_WRITE_DEADLINE` over the 600-write drain | thin by design |
| `binary(latency_slo_gate)` | timing-step | `crates/holon-integration-tests/tests/latency_slo_gate.rs:1211` | `FENCE_ROUND_PATIENCE` per fence round | thin by design |
| `package(holon-api) & test(=latency_drain::tests::the_drain_gate_passes_every_healthy_pipeline_and_fails_every_slow_one)` | timing-step | `crates/holon-api/src/latency_drain.rs:78` | the queueing-theory drain bound | thin by design |
| `binary_id(holon-org-format::deep_authored_text_never_aborts) & test(=a_marker_line_costs_time_linear_in_its_length)` | timing-step | `crates/holon-org-format/tests/deep_authored_text_never_aborts.rs:205` | marker-line parse cost linear in length | thin; `#[ignore]`d, run by `just org-deep-text-timing` |
| `binary_id(holon-org-format::deep_authored_text_never_aborts) & test(=a_marked_line_renders_in_time_linear_in_its_length)` | timing-step | `crates/holon-org-format/tests/deep_authored_text_never_aborts.rs:244` | marked-line render cost linear in length | thin; `#[ignore]`d, run by `just org-deep-text-timing` |
| `test(=cursor_filtered_main_panel_delivers_at_vault_scale)` | timing-step | `crates/holon/tests/turso_storage_repros/tabs_main_panel_delivery.rs:184` | cold matview create + first read < 5 s at ~70 pages | thin: 853 ms alone, 4.74 s at load 46, 7.52 s in an 885-test run |
| `package(holon-rows) & test(=a_number_of_many_digits_is_an_error_naming_the_limit_not_minutes_of_bigint_work)` | timing-step | `crates/holon-rows/tests/mapper_bounds.rs:338` | 500 000 digits refused in < 2 s — a superlinear-parse DoS bound | thin |
| `package(holon-loro) & test(=loro_sync_controller::tests::dropped_gate_is_observed_before_the_watchdog)` | timing-step | `crates/holon-loro/src/loro_sync_controller.rs:2566` | a dropped gate is observed before `WATCHDOG` = 200 ms, after a 30 ms sleep | **very thin: 170 ms** |
| `package(holon-loro) & test(=loro_share_backend::tests::settle_stays_bounded_while_the_sync_worker_dials_an_unreachable_peer)` | timing-step | `crates/holon-loro/src/loro_share_backend.rs:4921` | settle stays under 3 s while the worker dials an unreachable peer | moderate |
| `package(holon) & test(=api::backend_engine::tests::deriving_the_net_is_cheap_enough_for_the_tick_path)` | timing-step | `crates/holon/src/api/backend_engine.rs:3267` | one net derive costs < 5 ms on the tick path | thin |
| `package(holon) & test(=wakes_when_blocks_arrive_via_cdc)` | timing-step | `crates/holon/tests/block_live_data_catch_up.rs:124` | the wait wakes on CDC arrival in < 2 s rather than poll-spinning | moderate |
| `package(holon) & test(=a_failed_watch_setup_leaves_no_membership_row)` | timing-step | `crates/holon/tests/watch_context_membership_leak.rs:218` | the failure path returns in < 20 s rather than parking in a dependency wait | generous |
| `package(holon-app) & test(=dispatching_begin_oauth_returns_before_the_flow_finishes)` | timing-step | `crates/holon-app/tests/integration_set_field_op.rs:401` | `begin_oauth` returns in < 1 s, once STARTED rather than once finished | moderate |
| `package(holon-loro) & test(=a_create_costs_the_same_in_a_large_tree_as_in_a_small_one)` | timing-step | `crates/holon-loro/tests/stable_id_index_create_cost.rs:64` | a create in a large tree costs at most 3x one in a small tree | thin |
| `package(holon-mcp) & test(=tools::tests::a_patch_that_does_not_read_back_as_edited_is_rolled_back_by_name)` | timing-step | `frontends/mcp/src/tools.rs:10382` | an inexact apply answers within its 5 s limit | generous |
| `package(holon-mcp) & test(=fan_out_expansion_is_bounded_and_exhaustion_is_disclosed)` | timing-step | `frontends/mcp/tests/describe_ui_deferred.rs:320` | a bounded fan-out pass finishes in < 5 s | generous |
| `package(holon-mcp-client) & test(=a_peer_that_never_answers_subscribe_is_cut_at_the_request_timeout)` | timing-step | `crates/holon-mcp-client/tests/subscribe_bounds.rs:116` | the wait ends at `REQUEST_TIMEOUT + 1 s` | generous |
| `package(holon-mcp-client) & test(=mcp_call_surface::tests::a_peer_that_trickles_is_cut_at_the_request_timeout_naming_the_call)` | timing-step | `crates/holon-mcp-client/src/mcp_call_surface.rs:225` | the same bar, unit-test form | generous |
| `package(holon-mcp-client) & test(=a_connected_but_silent_peer_cannot_pin_the_flow_past_its_timeout)` | timing-step | `crates/holon-mcp-client/tests/oauth_bootstrap_flow.rs:899` | a 400 ms timeout returns within 5 s | generous |
| `package(holon-loro) & test(=shape_cost_at_ten_thousand_entities)` | release-bench | `crates/holon-loro/tests/typed_entity_scale.rs:348` | read p95 < 200 ms at 10 000 entities — the SLO itself | thin |
| `package(holon-loro) & test(=shape_cost_at_ten_thousand_entities)` | release-bench | `crates/holon-loro/tests/typed_entity_scale.rs:354` | write p95 < 5 ms at 10 000 entities | thin |

The generous-headroom rows cost the hold seconds. They stay in the group
anyway: the rule must not acquire a "but the headroom is big" exception, or it
stops being mechanical.

Nine of these rows were found by `scripts/timing-split.py assertions`, not by
reading the gate: the manual inventory in the #475 plan missed them, including a
`p95 < 200 ms` that IS the SLO.

**The latency ORACLES are not in this table.** `inv-settle-budget` and
`inv-sql-budget`'s wall / query / RSS ceilings fire inside every composed run,
so they cannot be selected by a test filter. `HOLON_PBT_TIMING` governs them
instead (`crates/holon-integration-tests/src/pbt/timing_mode.rs`): `enforce` in
the hold, `measure` everywhere else. They are why the hold was 2 h 51 m.

## Caps of the load-tolerant tests

A raised cap must still catch a hang, or the split trades a quiet host for a
blind gate. #458 is the proof: a real 120 s hang, reproduced on BOTH A/B legs,
3/3, at load under 14. A generous cap would have hidden it — and hidden it as a
"load artifact", the most expensive kind of false green, because it arrives with
an explanation.

```
cap = clamp(ceil(K × p95_low), 60 s, 8 × p95_low)      K = 4
```

Three refusals:

1. **No low-load p95 means no raise.** A test that does not COMPLETE at low load
   has no p95, keeps the default 120 s cap, and stays red. A TIMEOUT is a
   CENSORED measurement, not a slow one, and a censored measurement buys no
   headroom. This is what catches #458 by construction.
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
5 completed runs. Load < 16 is the gate-44 A/B's own quiet condition (observed
4.2-15.0 over 84 runs), so the two data sets are comparable.

Collected from nextest's JUnit output: `[profile.load-tolerant.junit]` plus
`scripts/timing-split.py record <junit.xml>`, which appends
`date, commit, test, secs, load_start, load_max, verdict` to
`~/.holon-gate/test-duration-ledger.tsv` — the same home and append-only TSV
shape as `windowed-ledger.tsv` (`scripts/windowed-gate.py:156`), which records
verdicts but no durations.

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

| Test | `p95_low` | Cap | Evidence |
| --- | --- | --- | --- |
| `split_block_stale_display_regression` | 121 s | 484 s | 6 completed runs, load 4.8-8.6, 2026-10-19 (gate-44 A/B); was a 120 s cap with 2 s of headroom |
| `convert_block_to_page_survives_a_lagging_projection` | 108 s | 432 s | 6 completed runs, load 5.5-9.9, 2026-10-19 (gate-44 A/B); was a 120 s cap with 12 s of headroom |

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
entirely**, `SharePage` among them — the very kind of the gate-44 breach
(`'SharePage' took 7292ms`) that motivated #475. A rung built from that corpus
would not have covered the event that prompted the card.

So the enforced rung is the generated keystone, which does draw from the whole
alphabet. `scripts/timing-split.py coverage` checks it, and the threshold is ONE
constant:

```python
# How much of the budgeted kind alphabet the enforced rung must cover before
# the check reds. A random draw does not touch all 129 kinds in ONE run, so the
# coverage is accumulated over the ledger's last RUNS_WINDOW runs.
KIND_COVERAGE_MIN = 1.0        # fraction of the 129 budgeted kinds
KIND_COVERAGE_WINDOW = 1       # how many runs the coverage accumulates over
```

Set `KIND_COVERAGE_WINDOW = 1` for a per-run check and higher to accumulate.
One constant, one place, so moving between the two is an edit of a number — not
of the check. #414 ("Replace the land gate's random keystone smoke with a
deterministic tour of every transition") makes the per-run form sound: a
deterministic tour draws every kind every time, so when it lands the window
goes back to 1 and the check becomes exact.

## Measure mode is never silent

Every budget breach in `measure` is printed as

```
[<quantity> MEASURE-ONLY] '<subject>' <value><unit> over the <budget><unit> budget — measured, not judged (HOLON_PBT_TIMING=measure)
```

and appended to `~/.cache/holon-build/timing-measure.jsonl` as
`{"gate","quantity","subject","value","budget","unit"}`, so a trend can show a
regression the enforced rung missed. `HOLON_GATE_ID` stamps the gate; a lane run
stamps `local`.

`MEASURE-ONLY` is the grep handle each measure-mode recipe asserts the PRESENCE
of: a measure-mode step that emitted no measurement measured nothing, which is a
false green.
