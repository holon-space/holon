//! Subsystem-config shrinking — Design §8.7.
//!
//! The active *optional-subsystem set* is **test input proptest can shrink**, so
//! a failing case auto-minimizes to the minimal `(set of subsystems, transition
//! sequence)` that still reproduces. The axes are REAL components (§8.7: never
//! fixtures) — `build_sut` wires a real `ToggleStore`/`EditorComponent` per
//! present `Subsystem` over the always-on `BlockStore`.
//!
//! Bugs are planted as **wrong *reference* data** (the components stay correct),
//! so the differential invariant fires only when its subsystem is wired:
//!   - `ToggleBug` → `inv-toggle-match-ref` fails iff `Toggle` wired (seed-time).
//!   - `EditorBug` → `inv-editor-match-ref` fails iff `Editor` wired AND ≥1
//!     `TypeChar` ran (behavioral — drives joint (config, sequence) shrinking).
//!   - `BlockTreeBug` → `inv-blocks-match-ref` fails for EVERY config (the
//!     always-on substrate), no transition needed.
//!
//! `proptest::sample::subsequence` shrinks toward the shorter subsequence, so a
//! present subsystem shrinks toward absent — "fewer subsystems = the minimal
//! causal set" for free. A transition whose subsystem isn't wired is a
//! deterministic no-op (the cap gate = §8.7 precondition replay), so dropping a
//! subsystem correctly invalidates its now-unrunnable transitions.

use crate::components::build_sut;
use crate::core::{check_invariants, RefState, Subsystem, Transition};
use crate::transitions::{split::SplitBlock, toggle::ToggleState, typechar::TypeChar};
use proptest::prelude::*;

const SEED: [u64; 3] = [1, 2, 3];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // `None` is the no-bug baseline, exercised by the causal test
pub enum Plant {
    None,
    BlockTreeBug,
    ToggleBug,
    EditorBug,
}

/// A generated op. Realized into the real registered transition struct, so the
/// run goes through the exact `apply_to_ref`/`apply_to_sut`/`required_caps`
/// path — only the *generation* uses this small shrinkable enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Split,
    Toggle,
    Type(char),
}

impl Op {
    fn realize(&self, ref_state: &RefState) -> Box<dyn Transition> {
        match self {
            Op::Split => Box::new(SplitBlock { target: 1, new_id: ref_state.next_id }),
            Op::Toggle => Box::new(ToggleState { target: 1 }),
            Op::Type(c) => Box::new(TypeChar { ch: *c }),
        }
    }
}

/// The reference, with the plant's wrong data baked in (components stay correct).
fn planted_ref(plant: Plant) -> RefState {
    let mut r = RefState::seeded(&SEED);
    match plant {
        Plant::BlockTreeBug => r.blocks[0] = 999, // diverges from the real store always
        Plant::ToggleBug => {
            r.toggled.insert(1); // ref claims block 1 is toggled; the store says no
        }
        _ => {} // EditorBug is behavioral (below); None plants nothing
    }
    r
}

/// Run `(active, ops)` with cap-gated precondition replay + the behavioral plant.
/// `Err` ⇒ a wired invariant caught the planted reference divergence.
pub fn run_and_check(plant: Plant, active: &[Subsystem], ops: &[Op]) -> Result<(), Vec<String>> {
    let sut = build_sut(active, &SEED);
    let caps = sut.cap_set();
    let mut ref_state = planted_ref(plant);

    for op in ops {
        let t = op.realize(&ref_state);
        // §8.7 precondition replay: a transition whose subsystem isn't wired is a
        // deterministic no-op (not a panic, not a fake) — the cap gate.
        if !caps.satisfies(&t.required_caps()) {
            continue;
        }
        if t.preconditions(&ref_state).is_err() {
            continue;
        }
        // EditorBug: the *reference* editor drops the keystroke while the real
        // component types it → divergence emerges only after a TypeChar runs.
        let suppress_ref = plant == Plant::EditorBug && t.variant_name() == "TypeChar";
        if !suppress_ref {
            t.apply_to_ref(&mut ref_state);
        }
        ref_state.next_id = ref_state.blocks.iter().copied().max().unwrap_or(0) + 1;
        t.apply_to_sut(&sut);
    }

    check_invariants(&ref_state, &sut)
}

fn op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![
        Just(Op::Split),
        Just(Op::Toggle),
        proptest::char::range('a', 'e').prop_map(Op::Type),
    ]
}

/// The `(config, sequence)` strategy: config = a shrinkable subsequence of the
/// optional universe; sequence = up to 6 ops, re-gated at apply by the config.
fn case_strategy() -> impl Strategy<Value = (Vec<Subsystem>, Vec<Op>)> {
    (
        proptest::sample::subsequence(vec![Subsystem::Toggle, Subsystem::Editor], 0..=2),
        proptest::collection::vec(op_strategy(), 0..6),
    )
}

/// Drive proptest until the planted bug reproduces, then return the **minimized**
/// `(config, sequence)`. Deterministic RNG ⇒ a stable, assertable result.
pub fn minimize(plant: Plant) -> (Vec<Subsystem>, Vec<Op>) {
    use proptest::test_runner::{
        Config, RngAlgorithm, TestCaseError, TestError, TestRng, TestRunner,
    };
    // Deterministic RNG for a stable result; no failure-persistence file (this is
    // a bin, not a source-rooted test) to keep stderr clean.
    let config = Config { failure_persistence: None, ..Config::default() };
    let rng = TestRng::deterministic_rng(RngAlgorithm::ChaCha);
    let mut runner = TestRunner::new_with_rng(config, rng);
    let result = runner.run(&case_strategy(), |(active, ops)| {
        match run_and_check(plant, &active, &ops) {
            Ok(()) => Ok(()),
            Err(_) => Err(TestCaseError::fail("planted divergence caught")),
        }
    });
    match result {
        Err(TestError::Fail(_, value)) => value,
        other => panic!("expected a failing case for {plant:?}, got {other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn powerset() -> Vec<Vec<Subsystem>> {
        vec![
            vec![],
            vec![Subsystem::Toggle],
            vec![Subsystem::Editor],
            vec![Subsystem::Toggle, Subsystem::Editor],
        ]
    }

    /// Deterministic causal structure (no shrinking) — the robust §8.7 evidence.
    #[test]
    fn causal_structure_over_the_powerset() {
        let all_ops = [Op::Split, Op::Toggle, Op::Type('a')];
        for active in powerset() {
            assert!(
                run_and_check(Plant::None, &active, &all_ops).is_ok(),
                "no plant must stay green @ {active:?}"
            );
            assert_eq!(
                run_and_check(Plant::ToggleBug, &active, &[]).is_err(),
                active.contains(&Subsystem::Toggle),
                "toggle bug fails iff Toggle wired @ {active:?}"
            );
            assert_eq!(
                run_and_check(Plant::EditorBug, &active, &[Op::Type('x')]).is_err(),
                active.contains(&Subsystem::Editor),
                "editor bug fails iff Editor wired (with a keystroke) @ {active:?}"
            );
            assert!(
                run_and_check(Plant::EditorBug, &active, &[]).is_ok(),
                "editor bug needs the keystroke @ {active:?}"
            );
            assert!(
                run_and_check(Plant::BlockTreeBug, &active, &[]).is_err(),
                "blocktree bug fails for every config @ {active:?}"
            );
        }
    }

    /// proptest shrinking isolates the causal subsystem set + shortest sequence.
    #[test]
    fn shrinking_isolates_the_causal_subset() {
        let (cfg, ops) = minimize(Plant::ToggleBug);
        assert!(cfg.contains(&Subsystem::Toggle), "Toggle is causal; got {cfg:?}");
        assert!(ops.is_empty(), "toggle bug needs no transition; got {ops:?}");

        let (cfg, ops) = minimize(Plant::EditorBug);
        assert!(cfg.contains(&Subsystem::Editor), "Editor is causal; got {cfg:?}");
        assert!(
            ops.iter().any(|o| matches!(o, Op::Type(_))),
            "editor bug needs a TypeChar; got {ops:?}"
        );

        let (_cfg, ops) = minimize(Plant::BlockTreeBug);
        assert!(ops.is_empty(), "blocktree bug needs no transition; got {ops:?}");
    }
}
