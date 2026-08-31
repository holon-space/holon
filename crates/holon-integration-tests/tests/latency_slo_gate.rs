//! **The latency SLO as a GATE — Martin's ruling D50.a (2026-08-31).**
//!
//! The SLO ("interaction→projection-visible p95 < 200ms") had an oracle that
//! painted a banner and no automated check that could fail a build:
//! `run_self_checks` skips the whole budget family against a live app, so the
//! running app could breach its own SLO with every check still green
//! (docs/Testing/bugfunnel/entries/
//! 2026-08-31-set-field-e2e-latency-exceeds-slo-on-empty-vault.md).
//!
//! Two rungs, because one number could not carry it. Gating raw `stage="e2e"`
//! measures the DRIVER, not the tree: `ms` is service time plus the wait behind
//! everything queued ahead, so it condemns a healthy pipeline typed at quickly
//! and clears a slow one driven slowly.
//!
//! * [`latency_slo_rung_service_time_p95`] — one interaction in flight
//!   (dispatch, settle, next), p95 over n ≥ 30 `set_field`-class writes.
//! * [`latency_slo_rung_throughput_floor`] — the same writes dispatched
//!   back-to-back through the fire-and-forget door, scored on how fast the
//!   pipeline drains while saturated. REPORT-ONLY on the rate today; see its
//!   doc comment for the measured spread that forced that and what promoting it
//!   to a gate needs.
//!
//! Both score `holon_api::latency_slo::SloWindow`, the type the runtime
//! `latency-slo` oracle also scores, so the banner and this gate cannot report
//! different numbers for the same pipeline.
//!
//! The drive reuses the keystone alphabet and drivers verbatim — the
//! `CreateBlockUnderFocus → FocusEditableText → TypeChars×N` prefix the
//! `latency-ratchet.jsonl` corpus already uses, replayed through
//! `ComposedSut<WideE2E>`. Nothing here is a second implementation of the SUT.
//!
//! **Why a p95 here when `docs/Testing/latency-ceilings.txt` gates p50.** That
//! file is a RATCHET: its ceilings hug the measurement, so p95's measured
//! 1.34x-3.35x run-to-run spread would flap it. This gate is not a ratchet — it
//! judges against the fixed 200ms SLO. Service p95 measured 30 · 38 · 47 · 58 ·
//! 63 · 68 · 69 ms over seven runs of an unmodified tree, so the worst observed
//! run clears the budget by 2.9x while the whole spread is 2.3x. The margin
//! absorbs the volatility that breaks a ratchet, and the statistic stays the
//! one the SLO actually names.
//!
//! @pbt kind gate
//! @pbt covers latency-slo-service-time — paced interaction→visible p95
//! @pbt covers latency-slo-throughput — saturated pipeline drain rate

use std::collections::HashMap;

use holon_api::EntityName;
use holon_api::EntityUri;
use holon_api::Value;
use holon_api::latency_slo::MIN_DRAIN_INTERVALS;
use holon_api::latency_slo::MIN_SERVICE_SAMPLES;
use holon_api::latency_slo::RungVerdict;
use holon_api::latency_slo::SERVICE_TIME_SLO_MS;
use holon_api::latency_slo::SloWindow;
use holon_api::latency_slo::THROUGHPUT_FLOOR_WRITES_PER_SEC;
use holon_api::latency_slo::fault_injection::set_delivery_delay_ms;
use holon_frontend::operations::OperationIntent;
use holon_frontend::reactive::dispatch_intent_through_armed_door;
use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::slo_probe::MAX_CONTENTION_MS;
use holon_integration_tests::pbt::composed::slo_probe::SloProbe;
use holon_integration_tests::pbt::composed::slo_probe::contention_ms;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2E;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2EMachine;
use holon_integration_tests::pbt::composed::wide_e2e::wide_e2e_ref;
use holon_integration_tests::pbt::reference_state::ReferenceState;
use holon_integration_tests::pbt::transitions::CreateBlockUnderFocus;
use holon_integration_tests::pbt::transitions::E2ETransition;
use holon_integration_tests::pbt::transitions::FocusEditableText;
use holon_integration_tests::pbt::transitions::TypeChars;
use proptest_state_machine::ReferenceStateMachine;
use proptest_state_machine::StateMachineTest;

/// Paced writes. Past [`MIN_SERVICE_SAMPLES`] with margin, because the
/// correlator only samples an interaction that produced a CDC delta — a rung
/// sized exactly at the floor would go `Unjudged` the first time one write
/// coalesced.
const PACED_WRITES: usize = 40;

/// Burst writes. Far past [`MIN_DRAIN_INTERVALS`], because only the SATURATED
/// intervals count and how many of them a burst produces depends on how the
/// dispatch loop races the drain. Measured at 40 writes: 26 / 36 / 15 saturated
/// intervals across three runs, the last of them too few to judge at all. A
/// deeper burst keeps the queue non-empty for long enough that the statistic
/// rests on a stretch rather than on the race.
const BURST_WRITES: usize = 150;

/// The block every write lands on. Born-equal id, so oracle and SUT share it
/// and no synthetic→real reconcile is in the measured path.
const HOST_ID: &str = "block:slo-gate-host";

/// The probe is process-global and both rungs clear it when they arm, so they
/// take turns. (Under nextest each test is its own process and this is free.)
static RUNG_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn host_uri() -> EntityUri {
    EntityUri::parse(HOST_ID).expect("HOST_ID is a well-formed block uri")
}

/// The burst rung's per-write target. One block per write, because the
/// correlator closes only the NEWEST pending entry per target and supersedes
/// the rest: 40 rapid writes to ONE row coalesce into a single delivery and a
/// single sample (measured), which is no evidence about drain rate at all.
fn burst_target(i: usize) -> String {
    format!("block:slo-gate-burst-{i}")
}

/// Bring the SUT to "an editor is open on a block we own" — the exact prefix
/// the latency-ratchet corpus uses — plus the burst rung's target rows.
/// Nothing here is measured.
fn setup_sequence() -> Vec<E2ETransition> {
    // Burst targets FIRST, then the host, then the focus: a create moves the
    // editor, so focusing the host has to be the last thing the prefix does.
    let mut v: Vec<E2ETransition> = (0..BURST_WRITES)
        .map(|i| {
            E2ETransition::CreateBlockUnderFocus(CreateBlockUnderFocus {
                content: format!("burst target {i}"),
                id: Some(
                    EntityUri::parse(&burst_target(i)).expect("burst target is a well-formed uri"),
                ),
            })
        })
        .collect();
    v.push(E2ETransition::CreateBlockUnderFocus(
        CreateBlockUnderFocus {
            content: "slo gate host".to_string(),
            id: Some(host_uri()),
        },
    ));
    v.push(E2ETransition::FocusEditableText(FocusEditableText {
        block_id: host_uri(),
    }));
    v
}

/// `n` single-character editor commits. Each one is a `set_field`-class write:
/// the editor VM commits every keystroke, which is the op the dogfood run
/// measured and the op this SLO is about.
fn write_sequence(n: usize) -> Vec<E2ETransition> {
    (0..n)
        .map(|i| {
            E2ETransition::TypeChars(TypeChars {
                // Cycle the alphabet rather than repeating one char: an
                // identity re-commit produces no CDC delta and therefore no
                // sample at all.
                text: ((b'a' + (i % 26) as u8) as char).to_string(),
            })
        })
        .collect()
}

/// Boot the SUT and run the (unmeasured) setup prefix.
fn boot() -> (ComposedSut<WideE2E>, ReferenceState) {
    let mut ref_state = wide_e2e_ref();
    let mut sut = <ComposedSut<WideE2E> as StateMachineTest>::init_test(&ref_state);
    for t in setup_sequence() {
        assert!(
            WideE2EMachine::preconditions(&ref_state, &t),
            "latency-slo gate: setup transition {t:?} violates its precondition against the \
             booted oracle — the prefix is malformed (fix it, do not skip it)"
        );
        ref_state = WideE2EMachine::apply(ref_state, &t);
        sut = <ComposedSut<WideE2E> as StateMachineTest>::apply(sut, &ref_state, t);
    }
    (sut, ref_state)
}

/// Refuse to score a run the host was too busy to judge.
///
/// Wall-clock latency here moves more with host business than with any code
/// change: on a contended box this rung measured service p95 455ms and drain
/// 2.2/s, against 45ms and 44/s on a quiet one — the same tree, minutes apart.
/// Judging that would make the gate something people re-run until green.
///
/// A refused run PANICS rather than returning: a "too busy to judge" that
/// passes is a vacuous green, which is exactly the hole this whole change
/// closes. The message says INVALID so a reader can tell it from a real red —
/// the same three-outcome shape `just latency-gate` gets from its exit code 3.
fn require_a_judgeable_host() {
    let Some(ddl) = contention_ms() else {
        panic!(
            "[latency-slo gate] INVALID (not red): the boot emitted no `matview_ddl` events, so \
             the contention covariate is missing and this run cannot be certified quiet enough \
             to judge. The probe layer or the storage boot changed — investigate, do not relax."
        );
    };
    assert!(
        ddl <= MAX_CONTENTION_MS,
        "[latency-slo gate] INVALID (not red): mean boot matview_ddl {ddl:.1}ms exceeds the \
         {MAX_CONTENTION_MS:.0}ms contention cut, so the host was too busy for a wall-clock \
         latency verdict. NOTHING was scored — this is not evidence the tree regressed. Re-run \
         on a quiet machine. If it persists on an idle host, the tree itself slowed boot DDL, \
         which is a finding (see docs/Testing/latency-ceilings.txt)."
    );
    eprintln!(
        "[latency-slo gate] host admitted: mean boot matview_ddl {ddl:.1}ms <= {MAX_CONTENTION_MS:.0}ms"
    );
}

/// Fraction of dispatched burst writes that may fail to produce a delivery.
///
/// The fire-and-forget door returns `Ok` the moment it hands the intent off, so
/// a downstream failure is invisible to the caller — measured, 150 dispatches
/// yielded 62-63 deliveries (~59% loss) while the rung reported a rate as if
/// nothing had been lost. A rung that silently measures a partly-failing
/// pipeline understates retirement and calibrates its floor against the
/// understatement.
///
/// The loss is a HARNESS artifact (see the burst-loss note in the bugfunnel
/// entry), so this budget is set where it is to keep the rung honest about the
/// artifact rather than to certify it: the run's output line always states
/// landed/dispatched, and a loss worse than this fails loudly instead of being
/// absorbed into the rate.
const MAX_BURST_LOSS: f64 = 0.75;

/// Boot a fresh SUT and drive one burst through the production fire-and-forget
/// door. Returns the measured window.
fn measure_burst() -> SloWindow {
    let (sut, _ref_state) = boot();
    let engine = sut
        .handle()
        .reactive()
        .expect("the full-headless draw boots a reactive engine");

    let probe = SloProbe::arm();
    sut.runtime().block_on(async {
        engine.ui_state().set_detached_dispatch(true);
        for i in 0..BURST_WRITES {
            let mut params = HashMap::new();
            params.insert("id".to_string(), Value::String(burst_target(i)));
            params.insert("field".to_string(), Value::String("content".to_string()));
            // Distinct per write: an identity re-commit produces no CDC delta,
            // so it would yield no sample and silently shrink the burst.
            params.insert("value".to_string(), Value::String(format!("burst {i}")));
            dispatch_intent_through_armed_door(
                &engine,
                OperationIntent::new(EntityName::new("block"), "set_field".to_string(), params),
            )
            .await
            .expect("the detached door accepts a content write");
        }
        engine.ui_state().set_detached_dispatch(false);
    });
    sut.settle_projections();
    let window = probe.snapshot();
    drop(probe);

    // Disclose the shortfall on EVERY run, not only when it is fatal: the rate
    // below is deliveries per second, so a reader has to know how many of the
    // dispatched writes ever became one.
    let landed = window.len();
    let loss = 1.0 - (landed as f64 / BURST_WRITES as f64);
    eprintln!(
        "[latency-slo gate] burst: {landed}/{BURST_WRITES} dispatched writes produced a \
         delivery ({:.0}% lost)",
        loss * 100.0,
    );
    assert!(
        loss <= MAX_BURST_LOSS,
        "[latency-slo gate] the burst lost {:.0}% of its writes ({landed}/{BURST_WRITES} landed), \
         over the {:.0}% budget. The fire-and-forget door reports Ok regardless, so this is the \
         only place the shortfall is visible — the drain rate below it would be measuring a \
         mostly-failing pipeline.",
        loss * 100.0,
        MAX_BURST_LOSS * 100.0,
    );
    window
}

/// Fail with the window's full report. A latency red must say what it measured
/// — "the gate went red" with no number is not actionable.
fn assert_rung(name: &str, verdict: RungVerdict, window: &SloWindow) {
    match verdict {
        RungVerdict::Pass { measured, n } => {
            eprintln!("[latency-slo gate] {name}: PASS measured={measured:.1} n={n}");
        }
        RungVerdict::Fail { measured, n } => panic!(
            "[latency-slo gate] {name} FAILED: measured={measured:.1} over n={n}.\n  \
             {}\n  Total samples in window: {}. Ten of eleven unmodified-tree runs \
             cleared this budget by 2.9x or better, so this is very likely a real breach — \
             but one admitted run did measure 183ms, so confirm on an idle host before \
             attributing it to the tree.",
            window.report(),
            window.len(),
        ),
        RungVerdict::Unjudged { n, needed } => panic!(
            "[latency-slo gate] {name} produced NO VERDICT: {n} usable samples, {needed} \
             required. A gate that cannot judge is not a gate that passed.\n  {}\n  Total \
             samples in window: {}. Either the drive stopped emitting `stage=e2e` events \
             (correlation regression) or the pacing regime this rung depends on broke.",
            window.report(),
            window.len(),
        ),
    }
}

/// **RUNG 1 — SERVICE TIME.** One interaction in flight at a time: the harness
/// settles each transition's projections before the next dispatches, so every
/// sample is dispatched into an empty queue and its `ms` is the pipeline's own
/// cost. p95 must clear the 200ms SLO.
#[test]
fn latency_slo_rung_service_time_p95() {
    let _turn = RUNG_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (mut sut, mut ref_state) = boot();
    require_a_judgeable_host();

    let probe = SloProbe::arm();
    for t in write_sequence(PACED_WRITES) {
        ref_state = WideE2EMachine::apply(ref_state, &t);
        sut = <ComposedSut<WideE2E> as StateMachineTest>::apply(sut, &ref_state, t);
    }
    let window = probe.snapshot();
    drop(probe);

    eprintln!(
        "[latency-slo gate] service rung: {} ({} samples, {} of them service-time)",
        window.report(),
        window.len(),
        window.service_sample_count(),
    );
    // The paced drive must produce service samples by construction. Zero means
    // the harness stopped settling between transitions, which would silently
    // turn this rung into a second throughput measurement.
    assert!(
        window.service_sample_count() >= MIN_SERVICE_SAMPLES,
        "[latency-slo gate] the paced drive produced only {} service-time samples out of {} \
         deliveries — every transition here settles before the next dispatches, so this means \
         the pacing broke, not that the pipeline is slow. Window: {}",
        window.service_sample_count(),
        window.len(),
        window.report(),
    );
    assert_rung(
        &format!("service-time p95 (< {SERVICE_TIME_SLO_MS}ms)"),
        window.service_verdict(),
        &window,
    );
}

/// **RUNG 2 — THROUGHPUT FLOOR.** The same `set_field` writes dispatched
/// back-to-back through the production fire-and-forget door, so the queue
/// builds and the deliveries measure how fast the pipeline RETIRES work rather
/// than how fast the driver offers it.
///
/// Driven by intent rather than by the `TypeChars` transition: the editor cap
/// awaits each commit before returning, so a transition drive cannot put two
/// interactions in flight no matter which door it takes (measured — a detached
/// `TypeChars` burst produced 0 saturated intervals over 38 deliveries).
/// `dispatch_intent_through_armed_door` is the same door the GPUI keystroke
/// handler uses, and it dispatches the same `block`/`set_field` op.
///
/// The oracle is deliberately not advanced: this rung measures throughput, and
/// the SUT state it leaves behind is discarded. Convergence over these writes
/// is the keystone's job, not this rung's.
///
/// **REPORT-ONLY on the rate.** The floor is computed and printed, but a rate
/// below it does not fail this test; the floor's own falsification lives in
/// `holon_api::latency_slo`'s `throughput_rung_fails_a_slow_drain`. On hosts
/// the contention covariate admitted, an unmodified tree measured 27.0/s and
/// 9.5/s — a 2.8x spread with the covariate reading quiet both times, so no
/// floor between them is anything but a coin flip, and a floor below them both
/// is decoration.
///
/// This is the treatment `docs/Testing/latency-ceilings.txt` already gives its
/// two SplitBlock rungs: measured and printed every run, unable to fail a build
/// until the slow mode is attributed. What DOES fail here is structural — a
/// burst that did not saturate, or one that produced no samples — because those
/// mean the rung measured nothing, which must never read as a pass.
///
/// To promote it to a gate: attribute the spread (the CDC actor's batching
/// granularity is the prime suspect — the same 150 writes retire in 3 batches
/// on one run and many more on the next), then calibrate per the ceilings
/// file's methodology.
#[test]
fn latency_slo_rung_throughput_floor() {
    let _turn = RUNG_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let window = measure_burst();
    require_a_judgeable_host();

    eprintln!(
        "[latency-slo gate] throughput rung: {} ({} samples, {} saturated intervals)",
        window.report(),
        window.len(),
        window.drain_interval_count(),
    );
    // A burst that never saturated measured nothing about drain rate. That is
    // an unusable run, not a fast one.
    assert!(
        window.drain_interval_count() >= MIN_DRAIN_INTERVALS,
        "[latency-slo gate] the burst produced only {} saturated intervals out of {} \
         deliveries — the detached dispatch door did not put multiple interactions in flight, \
         so drain rate is unmeasurable. This rung cannot pass on this evidence. Window: {}",
        window.drain_interval_count(),
        window.len(),
        window.report(),
    );
    // REPORT-ONLY (see the doc comment): printed, never fatal on the rate.
    match window.throughput_verdict() {
        RungVerdict::Pass { measured, n } => eprintln!(
            "[latency-slo gate] throughput (report-only): {measured:.1} writes/s over n={n} \
             — at or above the {THROUGHPUT_FLOOR_WRITES_PER_SEC:.0}/s floor"
        ),
        RungVerdict::Fail { measured, n } => eprintln!(
            "[latency-slo gate] throughput (report-only): {measured:.1} writes/s over n={n} \
             — BELOW the {THROUGHPUT_FLOOR_WRITES_PER_SEC:.0}/s floor. Not fatal while this \
             rung is report-only; investigate if it persists on an idle host."
        ),
        RungVerdict::Unjudged { n, needed } => panic!(
            "[latency-slo gate] throughput produced NO VERDICT: {n} deliveries, {needed} \
             required. The burst measured nothing, which is not a pass.\n  {}",
            window.report(),
        ),
    }
}

// ── TEETH ────────────────────────────────────────────────────────────────────
// What a gate promises has to be falsifiable, and the two halves of that
// promise are proven at different levels — deliberately, after measurement.
//
// THE VERDICT FLIP is owned by `holon_api::latency_slo`'s unit tests, which
// feed the scorer synthetic windows and need no host:
//   * `service_rung_fails_on_a_slow_paced_pipeline` — slow input ⇒ Fail.
//   * `throughput_rung_fails_a_slow_drain` — slow drain ⇒ Fail.
//   * `an_idle_session_with_one_queued_delivery_is_not_a_slow_drain` and
//     `batched_deliveries_do_not_inflate_the_drain_rate` — the two false-red
//     estimators this rung shipped and lost, pinned so they cannot return.
//
// THE WIRING — that a real slowdown in the real CDC apply path reaches that
// scorer — is what only an integration test can show, and it is what the test
// below asserts.
//
// Why the integration test does NOT assert the flip. Measured across injected
// per-row delays of 60 / 250 / 300ms and paced drives of 30 / 40 / 80 writes:
// an injection strong enough to move a verdict also destroys the sample
// population that verdict needs. Paced service samples fell to n=0 / 6 / 19 /
// 28 against a floor of 30 (a slowed pipeline stops draining between
// transitions, so samples stop qualifying as service time), and the burst arm
// delivered 0 of 150 writes at EVERY delay tried — 60ms and 250ms alike, so
// 60ms is a rejected configuration, not a shipped one. The two knobs are not
// independently controllable through this lever, so a flip assertion here would
// report INCONCLUSIVE on an unmodified tree — a flaky gate, not a teeth check.
// What IS shipped is [`TEETH_DELAY_MS`] = 250ms on the paced arm only.
// An earlier version accepted `Unjudged` as "red enough" precisely to dodge
// this, and duly reported that the rung had teeth on a run that collected ZERO
// samples. That vacuity is what this structure removes.

/// The injected per-ROW delay for the wiring check.
const TEETH_DELAY_MS: u64 = 250;

/// Service p50 the slowed run must exceed. Clean runs measured 22-45ms across
/// every run of this lane; slowed runs measured 112-125ms. 90ms sits ~2x above
/// the clean ceiling and ~1.25x below the slowed floor, so neither host noise
/// nor a lost sample decides it.
const TEETH_MIN_SLOWED_P50_MS: u64 = 90;

/// Writes the wiring check drives. More than [`PACED_WRITES`], because the
/// injection costs samples: measured n=4 surviving from 40 writes, n=19 from
/// 80.
const TEETH_PACED_WRITES: usize = 80;

/// Enough surviving samples for a median to mean anything. Deliberately far
/// below `MIN_SERVICE_SAMPLES`: this test does not render a verdict, it checks
/// that the injection reached the scorer.
const TEETH_MIN_SAMPLES: usize = 5;

/// **The injection must reach the scorer through the real pipeline.** Arming a
/// per-row delay in `LiveData`'s CDC apply path must move the service-time
/// statistic the gate reads — proving probe, correlator and scorer are wired to
/// production, not to a mock.
#[test]
fn a_slowed_pipeline_moves_the_service_statistic() {
    let _turn = RUNG_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (mut sut, mut ref_state) = boot();

    let probe = SloProbe::arm();
    set_delivery_delay_ms(TEETH_DELAY_MS);
    for t in write_sequence(TEETH_PACED_WRITES) {
        ref_state = WideE2EMachine::apply(ref_state, &t);
        sut = <ComposedSut<WideE2E> as StateMachineTest>::apply(sut, &ref_state, t);
    }
    set_delivery_delay_ms(0);
    let window = probe.snapshot();
    drop(probe);

    eprintln!("[latency-slo gate] teeth (wiring): {}", window.report());
    let n = window.service_sample_count();
    assert!(
        n >= TEETH_MIN_SAMPLES,
        "the slowed run produced only {n} service samples (need {TEETH_MIN_SAMPLES} for a \
         median) — the injection is not reaching the scorer, or the drive collapsed entirely. \
         Window: {}",
        window.report(),
    );
    let (p50, max) = window
        .service_p50_max_ms()
        .expect("n >= TEETH_MIN_SAMPLES > 0");
    assert!(
        p50 >= TEETH_MIN_SLOWED_P50_MS,
        "the {TEETH_DELAY_MS}ms per-row injection did NOT move the service statistic: p50 \
         {p50}ms (max {max}ms) over n={n}, under the {TEETH_MIN_SLOWED_P50_MS}ms this check \
         requires — an unslowed tree measures 22-45ms here. Either the fault injector is not \
         reaching `LiveData::subscribe`, or the probe is no longer reading the events the \
         correlator emits. The gate would be scoring something that does not respond to the \
         pipeline.",
    );
    eprintln!(
        "[latency-slo gate] teeth (wiring): injection reached the scorer — p50 {p50}ms \
         (max {max}ms) over n={n}, against a 22-45ms unslowed baseline"
    );
}
