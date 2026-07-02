//! ★ Increment 4b — the WINDOWED generated-sequence proptest loop (smoke-benchmark stage).
//!
//! This file first pins the per-case COST of a fresh windowed `ComposedSut<WideE2E>` boot
//! (window open + cross-runtime settle + a couple of gesture ticks), because the plan's
//! budget for the loop is derived from that measurement (headless keystone reference:
//! 16 cases / 46.9s). The generated-sequence loop is added once the budget is chosen.
//!
//! ⚠ `--test-threads=1` mandatory: gpui `TestApp` is not parallel-safe.

use std::cell::RefCell;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use holon_api::{EntityUri, Region};
use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::wide_e2e::{
    wide_e2e_windowed_ref, WideE2E, WideE2EMachine,
};
use holon_integration_tests::pbt::transitions::{ClickBlock, E2ETransition};
use holon_integration_tests::pbt::ReferenceState;
use holon_pbt_core::capabilities::{
    SutHistoryWrite, SutNavHistoryDrive, SutNavHistoryWrite, SutViewControl,
};
use holon_pbt_core::composition::{CapId, CapSet};
use proptest::strategy::{BoxedStrategy, Just, Strategy};
use proptest::test_runner::{Config, TestCaseError, TestRunner};
use proptest_state_machine::{ReferenceStateMachine, StateMachineTest};

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;
use pbt_harness::panic_message;
use pbt_harness::windowed_wide::with_windowed_wide_sut;

/// The narrowed live windowed cap set, captured once (by a throwaway window boot at the top
/// of the loop test) before the strategy is built. `init_state` reads it so the generated
/// alphabet + the non-vacuity floor narrow to exactly what the WINDOW can drive.
static WINDOWED_CAP_SET: OnceLock<CapSet> = OnceLock::new();

/// Narrow a live windowed cap set to the windowed GENERATED alphabet.
///
/// The deferred windowed base is `full_headless` (a `HeadlessFrontendComponent`), which
/// still hosts the 6 EXCLUDED-row nav/history/view caps at the Direct-dispatch rung — but
/// no window-driver mechanism drives them yet (C-3 Rung Audit rows 19–24, tracked Phase 3
/// blockers). Driving them through the leftover dispatch impl while a window exists would be
/// an unfaithful cross-rung combination (Design §8.11), so they must NOT enter the windowed
/// generated alphabet. `CapSet::without` is the sanctioned, DISCLOSED narrowing: the caps
/// stay in the `CapMap` (their read invariants keep selecting), only the generation gate
/// drops their transitions. This is NOT the fix-the-cap-not-withhold anti-pattern (that
/// forbids faking a DIVERGENCE green) — it is the audit-prescribed exclusion of a
/// genuinely-undriveable transition class, disclosed here and in the audit table.
///
/// Cap → EXCLUDED transition rows:
/// - `SutNavHistoryWrite`  → NavigateHome (row 19)
/// - `SutNavHistoryDrive`  → NavigateBack/Forward, PinBlock, UnpinBlock (rows 20–22)
/// - `SutViewControl`      → SwitchView (row 23)
/// - `SutHistoryWrite`     → UndoLastMutation/Redo (row 24)
fn narrow_to_windowed_alphabet(cap_set: CapSet) -> CapSet {
    cap_set
        .without(&CapId::of::<dyn SutNavHistoryWrite>())
        .without(&CapId::of::<dyn SutNavHistoryDrive>())
        .without(&CapId::of::<dyn SutViewControl>())
        .without(&CapId::of::<dyn SutHistoryWrite>())
}

/// Report which of the 6 EXCLUDED-row caps the LIVE windowed base actually carries, so the
/// narrowing is disclosed against reality (not assumed).
fn disclose_excluded(cap_set: &CapSet) {
    for (name, present) in [
        (
            "SutNavHistoryWrite (NavigateHome)",
            cap_set.contains(&CapId::of::<dyn SutNavHistoryWrite>()),
        ),
        (
            "SutNavHistoryDrive (Back/Fwd/Pin/Unpin)",
            cap_set.contains(&CapId::of::<dyn SutNavHistoryDrive>()),
        ),
        (
            "SutViewControl (SwitchView)",
            cap_set.contains(&CapId::of::<dyn SutViewControl>()),
        ),
        (
            "SutHistoryWrite (Undo/Redo)",
            cap_set.contains(&CapId::of::<dyn SutHistoryWrite>()),
        ),
    ] {
        eprintln!("[4b-alphabet] EXCLUDED cap present-in-base={present}: {name} (narrowed out of generation)");
    }
}

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

/// The windowed sibling of [`WideE2EMachine`]: identical transition generation /
/// preconditions / apply (delegated), but `init_state` FIXES the oracle to the narrowed
/// live windowed cap set (`WINDOWED_CAP_SET`) instead of drawing `any_valid_wiring()`. That
/// cap set auto-narrows `aggregate_transitions` to the windowed alphabet (the 9 REBIND/OK
/// gesture rows) and drops the 6 EXCLUDED rows, and it is the same set the per-tick
/// `check_invariants` non-vacuity floor (`required_invariants`) is computed against.
struct WideE2EWindowedMachine;

impl ReferenceStateMachine for WideE2EWindowedMachine {
    type State = ReferenceState;
    type Transition = E2ETransition;

    fn init_state() -> BoxedStrategy<Self::State> {
        let cap_set = WINDOWED_CAP_SET
            .get()
            .expect("WINDOWED_CAP_SET must be captured (throwaway boot) before the strategy")
            .clone();
        Just(wide_e2e_windowed_ref(cap_set)).boxed()
    }

    fn transitions(state: &Self::State) -> BoxedStrategy<Self::Transition> {
        <WideE2EMachine as ReferenceStateMachine>::transitions(state)
    }

    fn preconditions(state: &Self::State, transition: &Self::Transition) -> bool {
        <WideE2EMachine as ReferenceStateMachine>::preconditions(state, transition)
    }

    fn apply(state: Self::State, transition: &Self::Transition) -> Self::State {
        <WideE2EMachine as ReferenceStateMachine>::apply(state, transition)
    }
}

/// Drive ONE generated case over a freshly-booted windowed SUT, mirroring
/// `proptest_state_machine::StateMachineTest::test_sequential` (initial-frame check, then
/// per transition: `seen_counter.fetch_add(1)` → ref apply → SUT apply → `check_invariants`)
/// — EXCEPT the concrete SUT comes from the window boot (not `init_test`), and every
/// oracle-comparison `check_invariants` is wrapped in `catch_unwind` so a divergence
/// unwinds cleanly (the SUT is only borrowed there, so it survives for normal teardown) and
/// its signature can be pinned. Returns `Some(sut)` for the harness to leak, or `None` if a
/// (rare) SUT-side `apply` panic consumed the SUT. On divergence, the message is written to
/// `out` and the drive stops.
fn drive_windowed_case(
    mut sut: ComposedSut<WideE2E>,
    initial_ref: ReferenceState,
    transitions: Vec<E2ETransition>,
    seen_counter: Option<Arc<AtomicUsize>>,
    out: &RefCell<Option<String>>,
) -> Option<ComposedSut<WideE2E>> {
    let mut ref_state = initial_ref;

    // Initial-frame invariants (block/storage families AND the windowed geometry/focus floor).
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

        // SUT-side apply consumes the SUT; catch so a driver-level panic tears down cleanly
        // (the SUT is dropped inside the caught unwind on a non-runtime-entered thread).
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
/// Random `E2ETransition` sequences drawn from the WINDOWED alphabet (the production
/// `aggregate_transitions` auto-narrowed by the live windowed cap set — see
/// [`narrow_to_windowed_alphabet`] for the 6 disclosed EXCLUDED rows) are applied to a
/// per-case freshly-booted windowed `ComposedSut<WideE2E>` and checked every tick against
/// `wide_e2e_windowed_ref(narrowed_cap_set)` with the windowed non-vacuity floor
/// (`required_invariants` + the `SutLayout`-keyed `inv-frontend-bounds-rendered` guard, both
/// enforced inside `ComposedSut::check_invariants`).
///
/// ## Disclosed case budget (measured, not silent)
/// `benchmark_windowed_per_case_boot_cost` measured ~9.8s/case steady-state (12.6s first,
/// window boot + cross-runtime settle dominate; the headless keystone is ~2.9s/case → the
/// windowed path is ~3.4× per case). At sequence length 1..6 a green case is roughly
/// boot + up-to-6 windowed ticks ≈ 15–20s. So the default budget is a SMALL **4 cases**
/// (plus one throwaway boot to capture the cap set) ≈ 70–90s wall — comparable to the
/// keystone's ~47s, deliberately bounded because each windowed case is ~3.4× the cost.
/// Override with `PROPTEST_CASES`. Shrinking (only on failure) re-boots a window per
/// candidate at ~10s each, so `max_shrink_iters` is capped low (30).
///
/// ⚠ `--test-threads=1` mandatory (gpui not parallel-safe). The escalation valve (plan):
/// a red run with a pinned, classified divergence signature is an acceptable outcome; a
/// fudged-green one is not.
#[test]
fn general_e2e_composed_pbt_windowed() {
    // One throwaway window boot to read + narrow + DISCLOSE the live windowed cap set. The
    // strategy's `init_state` needs it before generation begins.
    with_windowed_wide_sut(|sut, _default_oracle| {
        let live = sut.cap_set();
        disclose_excluded(&live);
        WINDOWED_CAP_SET
            .set(narrow_to_windowed_alphabet(live))
            .expect("WINDOWED_CAP_SET set once");
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
        // Disclose the drawn windowed alphabet per case (honest non-vacuity of generation:
        // proves the narrowed cap set admits real gesture/seam classes, not an empty draw).
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
