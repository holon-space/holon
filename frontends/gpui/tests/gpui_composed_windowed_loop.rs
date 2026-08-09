//! ★ Increment 4b — the WINDOWED generated-sequence proptest loop
//! (smoke-benchmark stage).
//!
//! This file first pins the per-case COST of a fresh windowed
//! `ComposedSut<WideE2E>` boot (window open + cross-runtime settle + a couple
//! of gesture ticks), because the plan's budget for the loop is derived from
//! that measurement (headless keystone reference: 16 cases / 46.9s). The
//! generated-sequence loop is added once the budget is chosen.
//!
//! ⚠ `--test-threads=1` mandatory: gpui `TestApp` is not parallel-safe.

use std::cell::RefCell;
use std::panic::AssertUnwindSafe;
use std::panic::catch_unwind;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Instant;

use holon_api::EntityUri;
use holon_api::Region;
use holon_integration_tests::pbt::ReferenceState;
use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2E;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2EMachine;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2EWindowedMachine;
use holon_integration_tests::pbt::composed::wide_e2e::disclose_excluded;
use holon_integration_tests::pbt::composed::wide_e2e::narrow_to_windowed_alphabet;
use holon_integration_tests::pbt::composed::wide_e2e::set_windowed_cap_set;
use holon_integration_tests::pbt::composed::wide_e2e::wide_e2e_windowed_ref;
use holon_integration_tests::pbt::transitions::ClickBlock;
use holon_integration_tests::pbt::transitions::E2ETransition;
use proptest::test_runner::Config;
use proptest::test_runner::TestCaseError;
use proptest::test_runner::TestRunner;
use proptest_state_machine::ReferenceStateMachine;
use proptest_state_machine::StateMachineTest;

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;
use pbt_harness::panic_message;
use pbt_harness::windowed_wide::with_windowed_wide_sut;

#[test]
fn benchmark_windowed_per_case_boot_cost() {
    let iters: usize = std::env::var("BENCH_ITERS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(4);
    let mut timings = Vec::new();
    for i in 0..iters {
        let t0 = Instant::now();
        with_windowed_wide_sut(|mut sut, _default_oracle| {
            let live = sut.cap_set();
            if i == 0 {
                disclose_excluded(&live);
            }
            let oracle = wide_e2e_windowed_ref(narrow_to_windowed_alphabet(live));
            ComposedSut::<WideE2E>::check_invariants(&sut, &oracle);

            // Two ClickBlock ticks (c1/c2 are the wide working-tree text leaves).
            let mut cur_oracle = oracle.clone();
            for block in ["c1", "c2"] {
                let t = E2ETransition::ClickBlock(ClickBlock {
                    region: Region::Main,
                    block_id: EntityUri::block(block),
                });
                if !<WideE2EMachine as ReferenceStateMachine>::preconditions(&cur_oracle, &t) {
                    continue;
                }
                cur_oracle = <WideE2EMachine as ReferenceStateMachine>::apply(cur_oracle, &t);
                sut = ComposedSut::<WideE2E>::apply(sut, &cur_oracle, t);
                ComposedSut::<WideE2E>::check_invariants(&sut, &cur_oracle);
            }
            Some(sut)
        });
        let dt = t0.elapsed();
        eprintln!("[4b-bench] case {i} wall={:.2}s", dt.as_secs_f64());
        timings.push(dt.as_secs_f64());
    }
    let total: f64 = timings.iter().sum();
    let avg = total / timings.len() as f64;
    eprintln!(
        "[4b-bench] SUMMARY over {} cases: total={total:.2}s avg={avg:.2}s min={:.2}s max={:.2}s",
        timings.len(),
        timings.iter().cloned().fold(f64::INFINITY, f64::min),
        timings.iter().cloned().fold(0.0, f64::max),
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// The windowed generated-sequence proptest loop.
// ─────────────────────────────────────────────────────────────────────────────

/// Drive ONE generated case over a freshly-booted windowed SUT, mirroring
/// `proptest_state_machine::StateMachineTest::test_sequential` (initial-frame
/// check, then per transition: `seen_counter.fetch_add(1)` → ref apply → SUT
/// apply → `check_invariants`) — EXCEPT the concrete SUT comes from the window
/// boot (not `init_test`), and every oracle-comparison `check_invariants` is
/// wrapped in `catch_unwind` so a divergence unwinds cleanly (the SUT is only
/// borrowed there, so it survives for normal teardown) and its signature can be
/// pinned. Returns `Some(sut)` for the harness to leak, or `None` if a
/// (rare) SUT-side `apply` panic consumed the SUT. On divergence, the message
/// is written to `out` and the drive stops.
fn drive_windowed_case(
    mut sut: ComposedSut<WideE2E>,
    initial_ref: ReferenceState,
    transitions: Vec<E2ETransition>,
    seen_counter: Option<Arc<AtomicUsize>>,
    out: &RefCell<Option<String>>,
) -> Option<ComposedSut<WideE2E>> {
    let mut ref_state = initial_ref;

    // Initial-frame invariants (block/storage families AND the windowed
    // geometry/focus floor).
    {
        let s = &sut;
        let r = &ref_state;
        if let Err(p) = catch_unwind(AssertUnwindSafe(|| {
            ComposedSut::<WideE2E>::check_invariants(s, r)
        })) {
            *out.borrow_mut() = Some(panic_message(&p));
            return Some(sut);
        }
    }

    for transition in transitions {
        // Mirror `test_sequential`: mark the transition seen BEFORE applying, so the
        // shrinker's "delete unseen trailing transitions" step is correct.
        if let Some(c) = seen_counter.as_ref() {
            c.fetch_add(1, Ordering::SeqCst);
        }
        ref_state = <WideE2EMachine as ReferenceStateMachine>::apply(ref_state, &transition);

        // SUT-side apply consumes the SUT; catch so a driver-level panic tears down
        // cleanly (the SUT is dropped inside the caught unwind on a
        // non-runtime-entered thread).
        match catch_unwind(AssertUnwindSafe(|| {
            ComposedSut::<WideE2E>::apply(sut, &ref_state, transition)
        })) {
            Ok(s) => sut = s,
            Err(p) => {
                *out.borrow_mut() = Some(format!("SUT apply panicked: {}", panic_message(&p)));
                return None;
            }
        }

        let s = &sut;
        let r = &ref_state;
        if let Err(p) = catch_unwind(AssertUnwindSafe(|| {
            ComposedSut::<WideE2E>::check_invariants(s, r)
        })) {
            *out.borrow_mut() = Some(panic_message(&p));
            return Some(sut);
        }
    }

    Some(sut)
}

/// ★ Increment 4b — the WINDOWED generated-sequence proptest loop.
///
/// Random `E2ETransition` sequences drawn from the WINDOWED alphabet (the
/// production `aggregate_transitions` auto-narrowed by the live windowed cap
/// set — see [`narrow_to_windowed_alphabet`] for the 6 disclosed EXCLUDED rows)
/// are applied to a per-case freshly-booted windowed `ComposedSut<WideE2E>` and
/// checked every tick against `wide_e2e_windowed_ref(narrowed_cap_set)` with
/// the windowed non-vacuity floor (`required_invariants` + the
/// `SutLayout`-keyed `inv-frontend-bounds-rendered` guard, both enforced inside
/// `ComposedSut::check_invariants`).
///
/// ## Disclosed case budget (measured, not silent)
/// `benchmark_windowed_per_case_boot_cost` measured ~9.8s/case steady-state
/// (12.6s first, window boot + cross-runtime settle dominate; the headless
/// keystone is ~2.9s/case → the windowed path is ~3.4× per case). At sequence
/// length 1..6 a green case is roughly boot + up-to-6 windowed ticks ≈ 15–20s.
/// So the default budget is a SMALL **4 cases** (plus one throwaway boot to
/// capture the cap set) ≈ 70–90s wall — comparable to the keystone's ~47s,
/// deliberately bounded because each windowed case is ~3.4× the cost.
/// Override with `PROPTEST_CASES`. Shrinking (only on failure) re-boots a
/// window per candidate at ~10s each, so `max_shrink_iters` is capped low (30).
///
/// ⚠ `--test-threads=1` mandatory (gpui not parallel-safe). The escalation
/// valve (plan): a red run with a pinned, classified divergence signature is an
/// acceptable outcome; a fudged-green one is not.
#[test]
fn general_e2e_composed_pbt_windowed() {
    // One throwaway window boot to read + narrow + DISCLOSE the live windowed cap
    // set. The strategy's `init_state` needs it before generation begins.
    with_windowed_wide_sut(|sut, _default_oracle| {
        let live = sut.cap_set();
        disclose_excluded(&live);
        set_windowed_cap_set(narrow_to_windowed_alphabet(live));
        Some(sut)
    });

    let cases: u32 = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(4);
    let seq_max: usize = std::env::var("PBT_NUM_STEPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(6);

    let config = Config {
        cases,
        max_shrink_iters: 30,
        ..Config::default()
    };
    let mut runner = TestRunner::new(config);

    let pinned: RefCell<Option<String>> = RefCell::new(None);
    let strategy = WideE2EWindowedMachine::sequential_strategy(1..=seq_max);

    let result = runner.run(&strategy, |(initial_ref, transitions, seen_counter)| {
        // Disclose the drawn windowed alphabet per case (honest non-vacuity of
        // generation: proves the narrowed cap set admits real gesture/seam
        // classes, not an empty draw).
        let kinds: Vec<&'static str> = transitions.iter().map(|t| t.variant_name()).collect();
        eprintln!(
            "[4b-loop] case: {} transition(s) drawn: {:?}",
            kinds.len(),
            kinds
        );
        let out: RefCell<Option<String>> = RefCell::new(None);
        with_windowed_wide_sut(|sut, _default_oracle| {
            drive_windowed_case(
                sut,
                initial_ref.clone(),
                transitions.clone(),
                seen_counter,
                &out,
            )
        });
        match out.into_inner() {
            None => Ok(()),
            Some(msg) => {
                if pinned.borrow().is_none() {
                    eprintln!("[4b-loop] PINNED first windowed divergence signature:\n{msg}");
                    *pinned.borrow_mut() = Some(msg.clone());
                }
                let head: String = msg.chars().take(300).collect();
                Err(TestCaseError::fail(head))
            }
        }
    });

    match result {
        Ok(()) => eprintln!(
            "[4b-loop] PASS — {cases} windowed generated-sequence case(s) (seq 1..={seq_max}) \
             GREEN against wide_e2e_windowed_ref + non-vacuity floor"
        ),
        Err(e) => panic!("[4b-loop] windowed PBT failed (shrunk): {e}"),
    }
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
