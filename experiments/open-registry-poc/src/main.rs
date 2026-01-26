//! Open-registry transition design — runnable PoC (unified-architecture revision
//! + §8.7 subsystem-config shrinking).
//!
//! Demonstrates:
//!   - open registry (zero central edits; `core.rs` names no transition);
//!   - SUT = γ's `CapMap`; `cap_transition!` injects cap extraction;
//!   - `apply_to_sut` takes no ref (self-contained transitions);
//!   - cap-gated alphabet AND cap-gated invariants;
//!   - shrink Clone (dyn-clone), replay (typetag);
//!   - §8.7: the active-subsystem set is shrinkable test input — a planted bug
//!     minimizes to its causal `(subsystem set, sequence)`.
//!
//! Run with `cargo run`.

#[macro_use]
mod macros;
mod components;
mod core;
mod invariants;
mod shrink;
mod transitions;

use crate::components::build_sut;
use crate::core::{
    build_alphabet, cap, check_invariants, discovered_transitions, CapMap, RefState, Subsystem,
    SutBlockRead, SutBlockTreeWrite, SutEditorRead, SutToggleRead, SutToggleWrite, Transition,
};
use crate::shrink::Plant;
use proptest::strategy::Strategy;
use proptest::test_runner::TestRunner;

fn rule(title: &str) {
    println!("\n\x1b[1m── {title} {}\x1b[0m", "─".repeat(56usize.saturating_sub(title.len())));
}

fn all_caps() -> Vec<core::CapRef> {
    vec![
        cap::<dyn SutBlockTreeWrite>(),
        cap::<dyn SutBlockRead>(),
        cap::<dyn SutToggleWrite>(),
        cap::<dyn SutToggleRead>(),
        cap::<dyn SutEditorRead>(),
    ]
}

fn main() {
    // ── 1. Registry discovery — no central enum ──
    rule("Registry discovery (no central enum)");
    for (name, caps) in discovered_transitions() {
        println!("  • {name:<14} caps={caps:?}");
    }

    // ── 2. Cap-gated alphabet narrowing, driven by the SUT's hosted caps ──
    rule("Cap-gated alphabet narrowing (driven by CapMap.cap_set)");
    let seed = RefState::seeded(&[1, 2, 3]);
    let full = build_sut(&[Subsystem::Toggle, Subsystem::Editor], &[1, 2, 3]);
    let no_toggle = build_sut(&[Subsystem::Editor], &[1, 2, 3]);
    println!("  Toggle+Editor  hosts {:?}", full.cap_set().names(&all_caps()));
    println!("    -> alphabet: {:?}", names_in_alphabet(&seed, &full));
    println!("  Editor only    hosts {:?}", no_toggle.cap_set().names(&all_caps()));
    println!("    -> alphabet: {:?}", names_in_alphabet(&seed, &no_toggle));
    println!("  (ToggleState auto-drops when the Toggle subsystem isn't wired)");

    // ── 3. Lockstep run over the CapMap SUT — apply_to_sut takes NO ref ──
    rule("Lockstep run (generate -> apply ref + CapMap -> check invariants)");
    let mut runner = TestRunner::deterministic();
    let mut ref_state = RefState::seeded(&[1, 2, 3]);
    let sut = build_sut(&[Subsystem::Toggle, Subsystem::Editor], &[1, 2, 3]);
    let mut sequence: Vec<Box<dyn Transition>> = Vec::new();

    for step in 0..8 {
        let strat = build_alphabet(&ref_state, &sut.cap_set());
        let transition: Box<dyn Transition> = strat.new_tree(&mut runner).unwrap().current();
        transition.preconditions(&ref_state).expect("generator emits only applicable transitions");
        println!("  step {step}: {:<14} {transition:?}", transition.variant_name());
        transition.apply_to_ref(&mut ref_state);
        ref_state.next_id = ref_state.blocks.iter().copied().max().unwrap_or(0) + 1;
        transition.apply_to_sut(&sut);
        check_invariants(&ref_state, &sut).expect("invariants held in lockstep");
        sequence.push(transition);
    }
    println!(
        "  8 steps lockstep-green; final blocks={:?} editor={:?}",
        sut.expect::<dyn SutBlockRead>().blocks(),
        sut.expect::<dyn SutEditorRead>().text()
    );

    // ── 4. Shrink-clone (dyn-clone) ──
    rule("Shrink-clone (dyn-clone)");
    let cloned = sequence.clone();
    assert_eq!(
        serde_json::to_string(&sequence).unwrap(),
        serde_json::to_string(&cloned).unwrap()
    );
    println!("  cloned {}-step sequence; clone is byte-identical ✓", sequence.len());

    // ── 5. Replay (typetag) ──
    rule("Replay (typetag serialize/deserialize)");
    let json = serde_json::to_string(&sequence).unwrap();
    let replayed: Vec<Box<dyn Transition>> = serde_json::from_str(&json).unwrap();
    let mut ref2 = RefState::seeded(&[1, 2, 3]);
    let sut2 = build_sut(&[Subsystem::Toggle, Subsystem::Editor], &[1, 2, 3]);
    for t in &replayed {
        t.apply_to_ref(&mut ref2);
        ref2.next_id = ref2.blocks.iter().copied().max().unwrap_or(0) + 1;
        t.apply_to_sut(&sut2);
    }
    assert_eq!(ref2, ref_state, "replayed ref diverged");
    println!("  deserialized {} steps via typetag; replay reproduced final state ✓", replayed.len());

    // ── 6. Subsystem-config shrinking (§8.7) ──
    rule("Subsystem-config shrinking (§8.7): config is shrinkable input");
    for plant in [Plant::BlockTreeBug, Plant::ToggleBug, Plant::EditorBug] {
        let (cfg, ops) = shrink::minimize(plant);
        let ops: Vec<String> = ops.iter().map(|o| format!("{o:?}")).collect();
        println!("  plant {plant:?}: minimal config = {cfg:?}, sequence = {ops:?}");
    }
    println!("  (each bug auto-minimizes to its causal subsystem set + shortest sequence)");

    rule("Result");
    println!("  SUT = CapMap; cap_transition! injects extraction; no ref in apply_to_sut;");
    println!("  the active-subsystem set is shrinkable test input. core.rs names nothing.\n");
}

fn names_in_alphabet(state: &RefState, sut: &CapMap) -> Vec<&'static str> {
    let mut runner = TestRunner::deterministic();
    let caps = sut.cap_set();
    let strat = build_alphabet(state, &caps);
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..50 {
        seen.insert(strat.new_tree(&mut runner).unwrap().current().variant_name());
    }
    seen.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typetag_roundtrip_preserves_behaviour() {
        let original: Vec<Box<dyn Transition>> = vec![
            Box::new(transitions::split::SplitBlock { target: 1, new_id: 4 }),
            Box::new(transitions::toggle::ToggleState { target: 2 }),
            Box::new(transitions::typechar::TypeChar { ch: 'z' }),
        ];
        let json = serde_json::to_string(&original).unwrap();
        let back: Vec<Box<dyn Transition>> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.len(), 3);
        assert_eq!(back[0].variant_name(), "SplitBlock");
        assert_eq!(back[2].variant_name(), "TypeChar");
    }

    #[test]
    fn cap_gate_drops_unwired_subsystem() {
        let state = RefState::seeded(&[1, 2]);
        let sut = build_sut(&[Subsystem::Editor], &[1, 2]); // no Toggle
        let names = names_in_alphabet(&state, &sut);
        assert!(names.contains(&"SplitBlock"));
        assert!(names.contains(&"TypeChar"));
        assert!(!names.contains(&"ToggleState"), "Toggle-gated variant leaked");
    }

    #[test]
    fn requirement_token_equals_used_trait() {
        assert_eq!(
            transitions::split::SplitBlock::caps(),
            vec![cap::<dyn SutBlockTreeWrite>()]
        );
    }

    #[test]
    fn invariants_gate_on_caps() {
        // An empty CapMap hosts no read cap → every invariant deselects (proven,
        // not faked) → vacuously Ok instead of panicking on expect.
        assert!(check_invariants(&RefState::seeded(&[1]), &CapMap::new()).is_ok());
    }
}
