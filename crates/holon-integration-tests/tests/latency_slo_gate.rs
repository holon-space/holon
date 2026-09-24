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
//! * [`latency_slo_rung_drain_test`] — the controlled drain test (Martin's
//!   ruling D207.a): 600 `set_field` writes offered at 20/s through the
//!   fire-and-forget door, each to its own block, must all be visible within
//!   `N/f + s`. See `holon_api::latency_drain` for the rule and its proof.
//! * [`a_slowed_pipeline_fails_the_drain_test`] — the same drive with a per-row
//!   delivery delay that halves the capacity must FAIL.
//! * [`latency_slo_rung_facade_origin_is_measured_and_not_pooled`] — an
//!   agent/MCP-driven operation through `HolonService::execute_operation` is
//!   measured at all, and its samples stay out of the UI percentile (D119.a).
//! * [`latency_slo_rung_a_facade_clock_in_flight_does_not_alter_a_ui_sample`] —
//!   the registry partition, against the real pipeline: a facade clock in
//!   flight leaves a concurrent UI interaction service-time eligible, and a
//!   delivery for a block both origins are waiting on closes BOTH.
//!
//! The service rung scores `holon_api::latency_slo::SloWindow`, the type the
//! runtime `latency-slo` oracle also scores, so the banner and this gate cannot
//! report different service numbers for the same pipeline. The window's
//! passive drain estimate is printed beside the drain test as a disclosure and
//! never judged.
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
//! @pbt covers latency-slo-throughput — controlled drain test at the floor
//! @pbt covers latency-slo-facade-origin — facade interactions measured, scored
//! apart
//! @pbt covers latency-slo-origin-partition — a facade clock in flight cannot
//! alter a UI sample

use std::collections::HashMap;

use holon::api::holon_service::HolonService;
use holon_api::EntityName;
use holon_api::EntityUri;
use holon_api::Value;
use holon_api::latency_drain::DRAIN_WRITES;
use holon_api::latency_drain::DrainTest;
use holon_api::latency_drain::DrainVerdict;
use holon_api::latency_drain::Drive;
use holon_api::latency_drain::Step;
use holon_api::latency_e2e::pending_targets;
use holon_api::latency_slo::ClockOrigin;
use holon_api::latency_slo::E2eSample;
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

/// How often the drive looks for new deliveries while it waits.
const DRAIN_POLL: std::time::Duration = std::time::Duration::from_millis(2);

/// The per-row delivery delay the drain teeth arm: capacity at most
/// 1000 / 200 = 5 writes/s, half the floor, however the rows batch.
const THROUGHPUT_TEETH_DELAY_MS: u64 = 200;

/// The block every write lands on. Born-equal id, so oracle and SUT share it
/// and no synthetic→real reconcile is in the measured path.
const HOST_ID: &str = "block:slo-gate-host";

/// The probe is process-global and both rungs clear it when they arm, so they
/// take turns. (Under nextest each test is its own process and this is free.)
static RUNG_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn host_uri() -> EntityUri {
    EntityUri::parse(HOST_ID).expect("HOST_ID is a well-formed block uri")
}

/// The drain test's per-write target. One block per write: a delivered block
/// row carries no `write_seq` in this wiring, so the correlator closes the
/// NEWEST pending clock on a target and supersedes the rest. Distinct targets
/// make every closure the write's own.
const DRIVE_TARGET_PREFIX: &str = "block:slo-gate-burst-";

fn burst_target(i: usize) -> String {
    format!("{DRIVE_TARGET_PREFIX}{i}")
}

/// The drain test's warm-up write lands here.
fn warm_up_target() -> String {
    format!("{DRIVE_TARGET_PREFIX}warm-up")
}

/// What the drain test's setup creates for it.
#[derive(Clone, Copy, PartialEq)]
enum DriveTargets {
    Skip,
    Create,
}

/// Bring the SUT to "an editor is open on a block we own" — the exact prefix
/// the latency-ratchet corpus uses — plus the drain test's target rows when
/// asked. Nothing here is measured.
fn setup_sequence(targets: DriveTargets) -> Vec<E2ETransition> {
    let names: Vec<String> = match targets {
        DriveTargets::Skip => Vec::new(),
        DriveTargets::Create => (0..DRAIN_WRITES)
            .map(burst_target)
            .chain(std::iter::once(warm_up_target()))
            .collect(),
    };
    // Burst targets FIRST, then the host, then the focus: a create moves the
    // editor, so focusing the host has to be the last thing the prefix does.
    let mut v: Vec<E2ETransition> = names
        .iter()
        .map(|name| {
            E2ETransition::CreateBlockUnderFocus(CreateBlockUnderFocus {
                content: format!("burst target {name}"),
                id: Some(EntityUri::parse(name).expect("burst target is a well-formed uri")),
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
fn boot(targets: DriveTargets) -> (ComposedSut<WideE2E>, ReferenceState) {
    let mut ref_state = wide_e2e_ref();
    let mut sut = <ComposedSut<WideE2E> as StateMachineTest>::init_test(&ref_state);
    for t in setup_sequence(targets) {
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
/// A refused run PANICS, so the test fails: a "too busy to judge" that passes
/// is a vacuous green. The message says INVALID so a reader can tell a run
/// that judged nothing from one that judged the tree slow.
fn require_a_judgeable_host() {
    let Some(ddl) = contention_ms() else {
        panic!(
            "[latency-slo gate] INVALID (this test fails WITHOUT a verdict on the tree): the \
             boot emitted no `matview_ddl` events, so \
             the contention covariate is missing and this run cannot be certified quiet enough \
             to judge. The probe layer or the storage boot changed — investigate, do not relax."
        );
    };
    assert!(
        ddl <= MAX_CONTENTION_MS,
        "[latency-slo gate] INVALID (this test fails WITHOUT a verdict on the tree): mean boot \
         matview_ddl {ddl:.1}ms exceeds the \
         {MAX_CONTENTION_MS:.0}ms contention cut, so the host was too busy for a wall-clock \
         latency verdict. NOTHING was scored — this is not evidence the tree regressed. Re-run \
         on a quiet machine. If it persists on an idle host, the tree itself slowed boot DDL, \
         which is a finding (see docs/Testing/latency-ceilings.txt)."
    );
    eprintln!(
        "[latency-slo gate] host admitted: mean boot matview_ddl {ddl:.1}ms <= {MAX_CONTENTION_MS:.0}ms"
    );
}

/// Arms the per-row delivery delay, and disarms it when dropped, a panic
/// included: the delay is process-global, and a later test must not run
/// against a slowed pipeline.
struct ArmedDeliveryDelay;

impl ArmedDeliveryDelay {
    fn arm(ms: u64) -> Self {
        set_delivery_delay_ms(ms);
        Self
    }
}

impl Drop for ArmedDeliveryDelay {
    fn drop(&mut self) {
        set_delivery_delay_ms(0);
    }
}

/// The drive's samples: those on its targets. Any other UI interaction in the
/// window is not the drive's to count.
fn collect_drive_samples(probe: &SloProbe, seen: &mut usize, into: &mut Vec<E2eSample>) {
    let fresh = probe.samples_after(ClockOrigin::Ui, *seen);
    *seen += fresh.len();
    into.extend(
        fresh
            .into_iter()
            .filter(|s| s.target.starts_with(DRIVE_TARGET_PREFIX)),
    );
}

/// Drive the controlled drain test through the production fire-and-forget
/// door, as [`Drive`] schedules it, and judge it.
///
/// `delay_ms` arms the per-row delivery delay for the drive. It sleeps in
/// `LiveData::subscribe` before the subscriber applies a batch.
fn run_drain_test(sut: &ComposedSut<WideE2E>, delay_ms: u64) -> DrainVerdict {
    let engine = sut
        .handle()
        .reactive()
        .expect("the full-headless draw boots a reactive engine");
    let test = DrainTest::gate();
    let mut drive = Drive::new(
        test,
        warm_up_target(),
        (0..DRAIN_WRITES).map(burst_target).collect(),
    );

    let probe = SloProbe::arm();
    let delay = ArmedDeliveryDelay::arm(delay_ms);
    let samples = sut.runtime().block_on(async {
        engine.ui_state().set_detached_dispatch(true);
        let mut seen = 0;
        let mut samples = Vec::new();
        loop {
            let now = std::time::Instant::now();
            collect_drive_samples(&probe, &mut seen, &mut samples);
            match drive.step(now, &samples, pending_targets) {
                Step::Dispatch { target } => {
                    let mut params = HashMap::new();
                    params.insert("id".to_string(), Value::String(target.clone()));
                    params.insert("field".to_string(), Value::String("content".to_string()));
                    params.insert(
                        "value".to_string(),
                        Value::String(format!("drained {target}")),
                    );
                    drive.dispatched(std::time::Instant::now());
                    dispatch_intent_through_armed_door(
                        &engine,
                        OperationIntent::new(
                            EntityName::new("block"),
                            "set_field".to_string(),
                            params,
                        ),
                    )
                    .await
                    .expect("the detached door accepts a content write");
                }
                Step::Wait { until } => {
                    tokio::time::sleep(until.saturating_duration_since(now).min(DRAIN_POLL)).await;
                }
                Step::Done => break,
            }
        }
        engine.ui_state().set_detached_dispatch(false);
        samples
    });
    drop(delay);
    let lost: Vec<_> = probe
        .lost_clocks()
        .into_iter()
        .filter(|l| l.target.starts_with(DRIVE_TARGET_PREFIX))
        .collect();
    drop(probe);
    assert!(
        lost.is_empty(),
        "[latency-slo gate] the correlator dropped drive clocks unmeasured: {lost:?}. The \
         window keeps drive clocks below its capacity and the stall bound ends the drive long \
         before its expiry, so this is a broken premise, not a slow pipeline"
    );
    let verdict = drive.verdict();
    let ratio = match &verdict {
        DrainVerdict::Pass { completion, limit }
        | DrainVerdict::Late {
            completion: Some(completion),
            limit,
            ..
        } => format!("{:.2}", completion.as_secs_f64() / limit.as_secs_f64()),
        _ => "-".to_string(),
    };
    eprintln!(
        "[latency-slo gate] drain calibration: delay={delay_ms}ms/row C/L={ratio} \
         verdict={verdict:?} limit={:?} stall_bound={:?}",
        test.limit(),
        test.stall_bound(),
    );
    let mut window = SloWindow::new(
        ClockOrigin::Ui,
        samples.len().max(1),
        SERVICE_TIME_SLO_MS,
        THROUGHPUT_FLOOR_WRITES_PER_SEC,
    );
    for s in samples {
        window.record(s);
    }
    eprintln!("[latency-slo gate] drain window: {}", window.report());
    if window.drain_estimate().is_below() {
        eprintln!(
            "[latency-slo gate] WARNING (disclosure, not a verdict): the passive drain estimate \
             is below the floor over this run: {:?}",
            window.drain_estimate(),
        );
    }
    verdict
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
    let (mut sut, mut ref_state) = boot(DriveTargets::Skip);
    require_a_judgeable_host();

    let probe = SloProbe::arm();
    for t in write_sequence(PACED_WRITES) {
        ref_state = WideE2EMachine::apply(ref_state, &t);
        sut = <ComposedSut<WideE2E> as StateMachineTest>::apply(sut, &ref_state, t);
    }
    let window = probe.snapshot(ClockOrigin::Ui);
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

/// **RUNG 2 — THE DRAIN TEST (Martin's ruling D207.a).** [`DRAIN_WRITES`]
/// writes offered at twice the floor rate must all be visible within
/// `N/f + s` of the first dispatch. A healthy pipeline cannot miss it and one
/// below 9.934 writes/s cannot make it: see `holon_api::latency_drain`.
///
/// Driven by intent rather than by the `TypeChars` transition: the editor cap
/// awaits each commit before returning, so a transition drive cannot put two
/// interactions in flight. `dispatch_intent_through_armed_door` is the same
/// door the GPUI keystroke handler uses, and it dispatches the same
/// `block`/`set_field` op. The oracle is deliberately not advanced: the SUT
/// state this leaves behind is discarded.
#[test]
fn latency_slo_rung_drain_test() {
    let _turn = RUNG_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (sut, _ref_state) = boot(DriveTargets::Create);
    require_a_judgeable_host();
    match run_drain_test(&sut, 0) {
        DrainVerdict::Pass { completion, limit } => eprintln!(
            "[latency-slo gate] drain test: PASS — {DRAIN_WRITES} writes visible after \
             {completion:?}, limit {limit:?}"
        ),
        DrainVerdict::Invalid { write, late_by } => panic!(
            "[latency-slo gate] INVALID (this test fails WITHOUT a verdict on the tree): the \
             driver offered write {write} {late_by:?} behind the floor-rate schedule while the \
             pipeline had room, so the run did not offer the load the proof needs. Re-run on a \
             quiet machine."
        ),
        fail => panic!(
            "[latency-slo gate] drain test FAILED: {fail:?} (floor \
             {THROUGHPUT_FLOOR_WRITES_PER_SEC:.0}/s). A healthy pipeline cannot fail this test; \
             load arriving after boot admission can, so confirm on an idle host before \
             attributing it to the tree."
        ),
    }
}

/// **The drain test must respond to the pipeline.** The same drive with
/// [`THROUGHPUT_TEETH_DELAY_MS`] per row armed in `LiveData`'s apply path —
/// capacity at most 5 writes/s — must FAIL. No host admission: a busy host only
/// makes a slow pipeline slower.
#[test]
fn a_slowed_pipeline_fails_the_drain_test() {
    let _turn = RUNG_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (sut, _ref_state) = boot(DriveTargets::Create);
    match run_drain_test(&sut, THROUGHPUT_TEETH_DELAY_MS) {
        verdict if verdict.is_fail() => eprintln!(
            "[latency-slo gate] drain teeth: {verdict:?} with {THROUGHPUT_TEETH_DELAY_MS}ms per \
             row armed"
        ),
        DrainVerdict::Pass { completion, limit } => panic!(
            "[latency-slo gate] {THROUGHPUT_TEETH_DELAY_MS}ms per row caps the pipeline at \
             {:.1} writes/s, yet all {DRAIN_WRITES} writes were visible after {completion:?}, \
             inside the {limit:?} limit. Either the injector is not reaching the subscriber, or \
             the drain test no longer measures completion.",
            1000.0 / THROUGHPUT_TEETH_DELAY_MS as f64,
        ),
        invalid => panic!(
            "[latency-slo gate] INVALID (this test fails WITHOUT a verdict on the tree): \
             {invalid:?}. Re-run on a quiet machine."
        ),
    }
}

/// The facade rung's target: the block the paced prefix already focuses, so the
/// write it receives is delivered through the same mirror the UI writes are.
const FACADE_WRITES: usize = 5;

/// **RUNG 3 — THE FACADE CLOCK (Martin's ruling D119.a, 2026-09-12).**
///
/// An operation driven through `HolonService::execute_operation` — the session
/// facade the embedded MCP server and every agent-driven op enter through —
/// must open an interaction clock, or agent-driven work is simply absent from
/// the SLO. It used to be: the facade dispatched straight into the engine and
/// the correlator never saw the interaction, so a facade-only session measured
/// nothing at all while reporting no gap.
///
/// And the samples it produces must NOT be pooled with the UI ones. A facade
/// clock opens above the frontend dispatch seam, so it excludes a cost every UI
/// sample carries; a shared percentile would move with the agent/human traffic
/// mix rather than with the pipeline.
///
/// This rung asserts both halves against the real pipeline:
///   1. driving through the facade produces `ClockOrigin::Facade` samples;
///   2. the UI window's sample count is exactly the UI drive's, so not one
///      facade sample landed in the percentile the SLO gate scores.
///
/// It renders no p95 verdict — five writes is not a percentile, and the service
/// budget is rung 1's job. What it gates is presence and separation.
#[test]
fn latency_slo_rung_facade_origin_is_measured_and_not_pooled() {
    let _turn = RUNG_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (mut sut, mut ref_state) = boot(DriveTargets::Skip);
    let engine = sut
        .handle()
        .engine()
        .expect("the full-headless draw boots a Turso BackendEngine")
        .clone();
    // Exactly what the embedded MCP server constructs for an agent session.
    let facade = HolonService::new_with_origin(
        engine,
        holon_api::OpOrigin::Agent {
            session_id: "latency-slo-gate".to_string(),
            tool_call_id: "facade-rung".to_string(),
        },
    );

    let probe = SloProbe::arm();

    // (a) A short UI drive, settled between transitions. These are the samples
    // the SLO percentile is taken over.
    let ui_writes = 6;
    for t in write_sequence(ui_writes) {
        ref_state = WideE2EMachine::apply(ref_state, &t);
        sut = <ComposedSut<WideE2E> as StateMachineTest>::apply(sut, &ref_state, t);
    }
    sut.settle_projections();
    let ui_only = probe.snapshot(ClockOrigin::Ui).len();

    // (b) The same class of write, driven through the facade instead. One at a
    // time with a settle between, so each closes on its own delivery rather
    // than superseding the previous entry on the shared target.
    for i in 0..FACADE_WRITES {
        let mut params = holon_api::StorageEntity::new();
        params.insert("id".into(), Value::String(HOST_ID.to_string()));
        params.insert("field".into(), Value::String("content".to_string()));
        params.insert("value".into(), Value::String(format!("facade write {i}")));
        sut.runtime()
            .block_on(facade.execute_operation(&EntityName::new("block"), "set_field", params))
            .expect("the facade accepts a content write on the focused host block");
        sut.settle_projections();
    }

    let ui = probe.snapshot(ClockOrigin::Ui);
    let facade_window = probe.snapshot(ClockOrigin::Facade);
    drop(probe);

    eprintln!(
        "[latency-slo gate] facade rung: ui {} | facade {}",
        ui.report(),
        facade_window.report(),
    );

    // 1. The facade opened a clock and it closed at projection-visible.
    assert!(
        !facade_window.is_empty(),
        "[latency-slo gate] {FACADE_WRITES} operations through HolonService::execute_operation          produced ZERO facade-origin samples. The facade opens no interaction clock, so every          agent/MCP-driven operation is invisible to the latency SLO (D119.a). UI window for          comparison: {}",
        ui.report(),
    );

    // 2. Not one of them reached the UI percentile.
    assert_eq!(
        ui.len(),
        ui_only,
        "[latency-slo gate] the UI window grew from {ui_only} to {} samples while only the          FACADE was driven — facade samples are being pooled into the percentile the SLO gate          scores. They measure a shorter span (no frontend dispatch) and must be scored apart.",
        ui.len(),
    );
    assert!(
        facade_window
            .samples()
            .iter()
            .all(|s| s.origin == ClockOrigin::Facade),
        "a facade window may only ever hold facade samples"
    );
    eprintln!(
        "[latency-slo gate] facade rung: {} facade samples measured, UI window unchanged at {}          — the two origins are scored apart",
        facade_window.len(),
        ui.len(),
    );
}

/// **RUNG 4 — THE ORIGIN PARTITION, against the real pipeline.**
///
/// Separate windows are not enough on their own. Before the pending registry
/// was partitioned, a facade clock in flight reached the UI percentile through
/// two other doors, and both are production paths that RUNG 3 cannot see
/// because it drives the two origins one after the other with a settle between:
///
/// 1. **Queue depth.** `in_flight`/`backlog` were counted over the whole
///    registry, so one origin's queue moved the other's numbers — and the
///    resulting exclusion from the service-time rung was SILENT, leaving a
///    reader unable to tell a quiet stretch from one crowded out by agent
///    traffic.
///
///    The fix is not to pretend the pipeline is not shared. It IS shared, so a
///    facade op in flight really does make a concurrent UI interaction wait,
///    and admitting that sample as service time would report queue wait as
///    service time — a fake in the other direction. Martin's round-2 ruling:
///    populations stay partitioned (`in_flight`/`backlog` are per origin), but
///    ELIGIBILITY is cross-origin, and every exclusion is counted and named in
///    the report. This rung asserts the excluded-and-disclosed behaviour.
/// 2. **Supersession.** `close_delivered` deleted older entries of the same
///    `(target, kind)` as no-ops without consulting origin, so a user edit and
///    an agent op on ONE block silently annihilated whichever was older — no
///    sample, no expiry, no disclosure.
///
/// This rung holds two facade clocks open across a real UI interaction and
/// checks both. The facade clock is enrolled through `interaction_dispatched` —
/// the very function `HolonService::execute_operation` calls — rather than by
/// awaiting a real facade op, because a real one that stays in flight for
/// exactly the duration of a UI keystroke is not schedulable deterministically,
/// and a racy latency rung is worse than none. Everything else is production:
/// the UI drive is the keystone `TypeChars` transition, and the closes come
/// from real CDC deliveries.
#[test]
fn latency_slo_rung_a_facade_clock_in_flight_does_not_alter_a_ui_sample() {
    let _turn = RUNG_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (mut sut, mut ref_state) = boot(DriveTargets::Skip);

    let probe = SloProbe::arm();

    // (a) A facade clock on an unrelated block, held open for the whole rung:
    // nothing will ever deliver `block:facade-inflight-probe`.
    holon_api::latency_e2e::interaction_dispatched(
        "set_field",
        "block:facade-inflight-probe",
        holon_api::latency_e2e::Observable::BlockRow(None),
        ClockOrigin::Facade,
    );
    // (b) A facade clock on the block the UI is about to write, so the SAME
    // delivery is what both interactions are waiting for.
    holon_api::latency_e2e::interaction_dispatched(
        "set_field",
        HOST_ID,
        holon_api::latency_e2e::Observable::BlockRow(None),
        ClockOrigin::Facade,
    );

    // (c) One real UI interaction, settled by the harness like rung 1's.
    for t in write_sequence(1) {
        ref_state = WideE2EMachine::apply(ref_state, &t);
        sut = <ComposedSut<WideE2E> as StateMachineTest>::apply(sut, &ref_state, t);
    }
    sut.settle_projections();

    let ui = probe.snapshot(ClockOrigin::Ui);
    let facade = probe.snapshot(ClockOrigin::Facade);
    drop(probe);

    eprintln!(
        "[latency-slo gate] partition rung: ui {} | facade {}",
        ui.report(),
        facade.report(),
    );

    // 1. The UI interaction was measured at all.
    assert!(
        !ui.is_empty(),
        "[latency-slo gate] the UI write produced no sample at all — either the drive or the \
         correlator broke, and this rung cannot judge the partition. Facade window: {}",
        facade.report(),
    );

    // 2. SUPERSESSION (D2). The facade clock on the SAME block closed with its own
    //    sample rather than being deleted as a supersession of the UI one. Exactly
    //    one: the probe clock on the unrelated block never delivers.
    //
    //    Checked before the eligibility assertion below on purpose. Existence
    //    is the more basic fact — a sample that was silently deleted cannot be
    //    judged eligible or ineligible — and ordering it first keeps each of
    //    the two defects reachable by its own assertion.
    assert_eq!(
        facade.len(),
        1,
        "[latency-slo gate] expected the facade clock on {HOST_ID} to close with its own sample \
         off the same delivery, got {} facade samples. A UI clock and a facade clock on one \
         block are two interactions, not one superseding the other — a cross-origin \
         supersession deletes the loser silently, with no sample and no disclosure. Facade: {}",
        facade.len(),
        facade.report(),
    );

    // 3. QUEUE DEPTH (D1). The UI sample shared the pipeline with two facade
    //    clocks, so it is NOT service time — and the exclusion is visible.
    //
    //    Three things must hold together, and each fails differently:
    //      (a) the sample's own queue was empty, so `in_flight`/`backlog`
    //          stayed partitioned and did not absorb the facade traffic;
    //      (b) it is nonetheless excluded, because the shared pipeline made it
    //          wait and scoring it as uncontended would be a fake;
    //      (c) the exclusion is COUNTED and NAMED, so a thin population always
    //          says why it is thin.
    let sample = &ui.samples()[0];
    assert_eq!(
        (sample.in_flight, sample.backlog),
        (1, 0),
        "[latency-slo gate] this origin's OWN queue depth must stay partitioned — got \
         in_flight={} backlog={}, which means facade traffic is being counted into the UI \
         queue and the UI numbers now move with the agent/human traffic mix. UI: {}",
        sample.in_flight,
        sample.backlog,
        ui.report(),
    );
    assert!(
        !sample.is_service_time(),
        "[latency-slo gate] a UI interaction that shared the pipeline with 2 facade clocks was \
         scored as SERVICE TIME (contended={}). Service time means uncontended; the pipeline \
         is shared, so foreign traffic is real contention and this sample carries queue wait. \
         Admitting it reports queue wait as service time. UI: {}",
        sample.contended,
        ui.report(),
    );
    assert_eq!(
        ui.cross_origin_excluded(),
        1,
        "[latency-slo gate] the cross-origin exclusion was not COUNTED (got {}). An exclusion \
         that shrinks the population silently is indistinguishable from a quiet stretch — that \
         indistinguishability is what made the shared-registry version a defect rather than a \
         policy. UI: {}",
        ui.cross_origin_excluded(),
        ui.report(),
    );
    let report = ui.report();
    assert!(
        report.contains("1 excluded: facade traffic in the shared pipeline"),
        "[latency-slo gate] the report must NAME what it excluded and whose traffic caused it, \
         so a reader of a thin `n` knows why. Got: {report}",
    );
    eprintln!(
        "[latency-slo gate] partition rung: the same-target facade clock closed with its own \
         sample; the UI sample kept its own queue depth (in_flight=1 backlog=0) and was \
         excluded from the service rung as cross-origin contended, counted and named"
    );
}

// ── TEETH ────────────────────────────────────────────────────────────────────
// What a gate promises has to be falsifiable.
//
// * SERVICE: the test below arms [`TEETH_DELAY_MS`] per row on the paced arm,
//   where each write waits for its own sample, and the service statistic must
//   move. It proves the wiring; the service verdict's flip is owned by
//   `holon_api::latency_slo`'s `service_rung_fails_on_a_slow_paced_pipeline`.
// * THROUGHPUT: [`a_slowed_pipeline_fails_the_drain_test`] arms a per-row delay
//   that halves the capacity and asserts the drain test fails. The rule's
//   healthy/slow property is in `holon_api::latency_drain`.
//
// An earlier version accepted `Unjudged` as "red enough", and duly reported
// that a rung had teeth on a run that collected ZERO samples. Both teeth
// therefore require a judged, moving statistic.

/// The injected per-ROW delay for the wiring check.
const TEETH_DELAY_MS: u64 = 250;

/// Service p50 the slowed run must exceed. Clean runs measured 22-45ms across
/// every run of this lane. Every slowed sample carries the whole armed delay,
/// so 90ms sits ~2x above the clean ceiling and far below the delay.
const TEETH_MIN_SLOWED_P50_MS: u64 = 90;

/// Writes the wiring check drives.
const TEETH_PACED_WRITES: usize = 80;

/// How long one slowed write may take to become visible before the drive
/// declares the pipeline stuck.
const TEETH_WRITE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

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
    let (mut sut, mut ref_state) = boot(DriveTargets::Skip);

    let probe = SloProbe::arm();
    set_delivery_delay_ms(TEETH_DELAY_MS);
    // A transition returns before the slowed delivery lands. Each write waits
    // for its own sample, so every write flies alone and is service time.
    for (i, t) in write_sequence(TEETH_PACED_WRITES).into_iter().enumerate() {
        ref_state = WideE2EMachine::apply(ref_state, &t);
        sut = <ComposedSut<WideE2E> as StateMachineTest>::apply(sut, &ref_state, t);
        let started = std::time::Instant::now();
        while probe.snapshot(ClockOrigin::Ui).len() <= i {
            assert!(
                started.elapsed() < TEETH_WRITE_DEADLINE,
                "[latency-slo gate] teeth write {i} produced no e2e sample within \
                 {TEETH_WRITE_DEADLINE:?} with {TEETH_DELAY_MS}ms per row armed"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    set_delivery_delay_ms(0);
    let window = probe.snapshot(ClockOrigin::Ui);
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
