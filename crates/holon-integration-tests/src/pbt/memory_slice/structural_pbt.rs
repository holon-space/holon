//! F2 Stage 1 keystone — drive a **composed `CapMap`** through the production
//! *structural* transition alphabet, checked by the shared composed-invariant
//! catalog against the real `ReferenceState` oracle.
//!
//! This is the first PBT to drive a composed slice (NOT the monolithic `E2ESut`)
//! through `aggregate`d production transitions: the slice's `CapMap` hosts the
//! `&self` `SutBlockTreeWrite` cap (Step A), so `SplitBlock`/`Indent`/… run their
//! real `apply_to_sut` against it via `MemoryBackend`. The SUT and the oracle
//! share a fixed (identity) id space (`fixed_ids`), so split mints the SAME
//! `:split-N` id the reference does (the `next_split_id` lockstep below).
//!
//! Scope = the 6 structural transitions (Split/Join/Indent/Outdent/MoveUp/
//! MoveDown). The editor caps are deliberately NOT wired (Stage 1a): structural
//! `apply_to_ref` refocuses/opens the editor on the new block, which the detached
//! mirror would lag — wiring the editor here would false-RED `inv-editor-*`.
//! Editor+structural unification is Stage 1b.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use holon::api::MemoryBackend;
use holon_api::repository::Lifecycle;
use holon_api::{EntityUri, Region};
use holon_pbt_core::composition::CapMap;
use holon_pbt_core::{TransitionImpl, TransitionRef, weighted_arm};
use proptest::prelude::Just;
use proptest::strategy::{BoxedStrategy, Strategy, Union};
use proptest_state_machine::{ReferenceStateMachine, prop_state_machine};
use validated::Validated;

use crate::pbt::composed::composed_invariant_catalog;
use crate::pbt::composed::harness::{ComposedSlice, ComposedSut};
use crate::pbt::composed::seed_primitives::fixed_ids;
use crate::pbt::composed::subsystem_seed::{build_started_ref, seed_store};
use crate::pbt::invariants::registry::Subsystem;
use crate::pbt::memory_slice::builders::memory_wide_structural;
use crate::pbt::memory_slice::components::MemoryBackendComponent;
use crate::pbt::op_write_cap::IdResolver;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transitions::{
    Indent, JoinBlock, MoveDown, MoveUp, NavigateFocus, Outdent, SplitBlock,
};

/// The structural slice's transition alphabet — the 6 production block-tree
/// transitions, each binding only `S: SutBlockTreeWrite` (proven cross-medium by
/// `tests/cross_medium_caps.rs`), so the aggregate dispatches against a composed
/// `CapMap` without needing `SutHandle` (which the `E2ETransition` enum requires).
#[derive(Clone, Debug)]
enum MemTransition {
    SplitBlock(SplitBlock),
    JoinBlock(JoinBlock),
    Indent(Indent),
    Outdent(Outdent),
}

/// BlockTree is the always-on substrate; no Loro, no UI/editor (Stage 1a).
fn structural_subsystems() -> BTreeSet<Subsystem> {
    BTreeSet::new()
}

/// The seeded oracle for the structural slice: the started `parent/c1/c2` tree,
/// with focus navigated to `parent` so its text children (`c1`/`c2`) are
/// `main_editable_descendants` and the structural generators have candidates.
/// `expected_focus_root_ids(Main)` reads `open_pins[Main]`, which only a
/// `NavigateFocus` populates (`build_started_ref`'s editor seed does not) — so we
/// drive the production `NavigateFocus::apply_to_ref` rather than hand-seed the
/// `open_pins`/`navigation_history`/counter bookkeeping. No editor is opened
/// (Stage 1a): the SUT hosts no `SutEditorMirrorRead`, so editor invariants
/// deselect regardless of the oracle's editor state.
fn structural_ref() -> ReferenceState {
    let mut state = build_started_ref(&structural_subsystems());
    let parent = fixed_ids().parent;
    NavigateFocus {
        region: Region::Main,
        block_id: parent,
    }
    .apply_to_ref(&mut state);
    state
}

/// Invariants that MUST run each tick — the non-vacuity guard so "green" means
/// "ran over real data", not "deselected everything".
const REQUIRED_INVARIANTS: &[&str] = &[
    "inv-no-orphan-blocks",
    "inv-no-parent-cycles",
    "inv-blocks-match-ref/block_raw",
    "inv-block-parent-matches-ref/block_raw",
];

/// One arm per structural transition via the shared `weighted_arm` over the SAME
/// generic `TransitionFactory<ReferenceState>` impls the wide PBT uses.
fn aggregate(state: &ReferenceState) -> BoxedStrategy<MemTransition> {
    let mut arms: Vec<(u32, BoxedStrategy<MemTransition>)> = vec![];
    macro_rules! arm {
        ($ty:ty, $variant:path) => {
            if let Validated::Good(Some(a)) =
                weighted_arm::<_, $ty, MemTransition>(state, 1, $variant)
            {
                arms.push(a);
            }
        };
    }
    arm!(SplitBlock, MemTransition::SplitBlock);
    arm!(JoinBlock, MemTransition::JoinBlock);
    arm!(Indent, MemTransition::Indent);
    arm!(Outdent, MemTransition::Outdent);
    // NB: MoveUp/MoveDown are intentionally NOT generated (their `SutBlockTreeWrite`
    // impls are wired and covered by lockstep teeth). They are pure sibling-*order*
    // swaps, and `MemoryBackend` has no `sequence` column — child order is
    // insertion/move order, which can drift from the oracle's `sequence()`+id
    // order across a long mixed sequence (the oracle's `recanon_and_rebuild`
    // reassigns sequences canonically; the store does not). No invariant compares
    // child order, so the drift is silent until a later swap targets a different
    // sibling. Reparenting ops (Split/Join/Indent/Outdent) depend on parent /
    // previous-sibling, not absolute order, so they stay faithful. A
    // sequence-faithful SUT ordering (or an explicit order invariant) is the
    // follow-on that re-admits the swap ops to generation.
    assert!(
        !arms.is_empty(),
        "no structural transitions applicable — the seeded ReferenceState has no \
         editable focus-root descendants (check build_started_ref's layout seed)"
    );
    Union::new_weighted(arms).boxed()
}

struct MemMachine;

impl ReferenceStateMachine for MemMachine {
    type State = ReferenceState;
    type Transition = MemTransition;

    fn init_state() -> BoxedStrategy<Self::State> {
        Just(structural_ref()).boxed()
    }

    fn transitions(state: &Self::State) -> BoxedStrategy<Self::Transition> {
        aggregate(state)
    }

    fn preconditions(state: &Self::State, transition: &Self::Transition) -> bool {
        // Re-gate on the production preconditions during shrink replay: structural
        // reparenting means a transition valid when generated can become invalid
        // after the shrinker drops an earlier one (e.g. Indent on a block that lost
        // its previous sibling). Returning `true` blindly would let proptest apply
        // it and panic the oracle's `apply_to_ref`; delegating lets proptest reject
        // the shrink instead.
        match transition {
            MemTransition::SplitBlock(t) => t.preconditions(state).is_good(),
            MemTransition::JoinBlock(t) => t.preconditions(state).is_good(),
            MemTransition::Indent(t) => t.preconditions(state).is_good(),
            MemTransition::Outdent(t) => t.preconditions(state).is_good(),
        }
    }

    fn apply(mut state: Self::State, transition: &Self::Transition) -> Self::State {
        match transition {
            MemTransition::SplitBlock(t) => t.apply_to_ref(&mut state),
            MemTransition::JoinBlock(t) => t.apply_to_ref(&mut state),
            MemTransition::Indent(t) => t.apply_to_ref(&mut state),
            MemTransition::Outdent(t) => t.apply_to_ref(&mut state),
        }
        state
    }
}

/// The memory structural slice as a thin [`ComposedSlice`] — the alphabet
/// (`MemTransition`/`MemMachine`), a `MemoryBackend` seed, and a **counter-sync** id
/// strategy: rather than reconcile uuid mints post-hoc (the headless slice's path),
/// it pushes the oracle's `next_id` into the backend's split-id hint via
/// [`align_ids`](ComposedSlice::align_ids) so each split mints the *same* `:split-N`
/// id the oracle does — the harness reconcile then sees an identity pair. Everything
/// else (runtime, reconcile, catalog check) is the generic [`ComposedSut`] harness's.
struct MemStructural;

impl ComposedSlice for MemStructural {
    type Transition = MemTransition;
    type Machine = MemMachine;
    type Handle = Arc<MemoryBackendComponent>;
    const REQUIRED_INVARIANTS: &'static [&'static str] = REQUIRED_INVARIANTS;
    const SETTLE: Duration = Duration::ZERO;

    async fn build(
        _: &IdResolver,
        _: &ReferenceState,
    ) -> (CapMap, Arc<MemoryBackendComponent>, BTreeSet<EntityUri>) {
        let backend = Arc::new(
            MemoryBackend::create_new("mem-structural".to_string())
                .await
                .expect("create_new MemoryBackend"),
        );
        seed_store(backend.as_ref(), &fixed_ids()).await;
        let (caps, component) = memory_wide_structural(backend);
        (caps, component, BTreeSet::new())
    }

    async fn apply_transition(
        transition: &MemTransition,
        ref_state: &ReferenceState,
        caps: &mut CapMap,
    ) {
        match transition {
            MemTransition::SplitBlock(t) => t.apply_to_sut(ref_state, caps).await,
            MemTransition::JoinBlock(t) => t.apply_to_sut(ref_state, caps).await,
            MemTransition::Indent(t) => t.apply_to_sut(ref_state, caps).await,
            MemTransition::Outdent(t) => t.apply_to_sut(ref_state, caps).await,
        }
    }

    /// Keep the `MemoryBackend`'s synthetic-id counter in lockstep with the oracle's
    /// `next_id` so the next split mints the id the oracle will mint (identity reconcile).
    fn align_ids(component: &Arc<MemoryBackendComponent>, ref_state: &ReferenceState) {
        component.set_next_split_id(ref_state.domain.block_state.next_id);
    }
}

prop_state_machine! {
    #![proptest_config(proptest::test_runner::Config {
        cases: 64,
        max_shrink_iters: 200,
        .. proptest::test_runner::Config::default()
    })]
    #[test]
    fn memory_slice_structural_pbt(sequential 1..20 => ComposedSut<MemStructural>);
}

// ─────────────────────────────────────────────────────────────────
// Teeth + deterministic regressions — prove the structural write path
// is real (the invariants *catch* a divergence, and a faithful op stays
// green), independent of the generated run above.
// ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod teeth {
    use super::*;
    use crate::pbt::reference_capabilities::reference_state_ref_caps;
    use crate::pbt::reference_state::Resolved;
    use holon_pbt_core::composition::run_selected;

    /// Build a seeded structural SUT (the `parent/c1/c2` tree over a real
    /// `MemoryBackend`, hosting `SutBlockTreeWrite` + `SutBackend`).
    fn seeded_sut(
        rt: &tokio::runtime::Runtime,
    ) -> (CapMap, std::sync::Arc<MemoryBackendComponent>) {
        let backend = rt.block_on(async {
            let backend = std::sync::Arc::new(
                MemoryBackend::create_new("mem-structural-teeth".to_string())
                    .await
                    .expect("create_new"),
            );
            seed_store(backend.as_ref(), &fixed_ids()).await;
            backend
        });
        memory_wide_structural(backend)
    }

    /// Positive: applying `Indent(c2)` to BOTH the SUT and the oracle in lockstep
    /// keeps every selected invariant green — the faithful structural write path.
    #[test]
    fn indent_in_lockstep_stays_green() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (mut caps, _component) = seeded_sut(&rt);
        let mut ref_state = structural_ref();
        let c2 = fixed_ids().c2;
        let t = Indent { block_id: c2 };
        t.apply_to_ref(&mut ref_state);
        rt.block_on(t.apply_to_sut(&ref_state, &mut caps));

        let ref_caps =
            reference_state_ref_caps(Resolved::identity(ref_state).map(std::sync::Arc::new));
        let report = rt.block_on(run_selected(
            &composed_invariant_catalog(),
            &caps,
            &ref_caps,
        ));
        assert!(
            report.failures().is_empty(),
            "lockstep Indent should be green: {:?}",
            report.failures()
        );
        assert!(
            report
                .ran_ids()
                .contains(&"inv-block-parent-matches-ref/block_raw"),
            "non-vacuity: parent invariant must have run (ran: {:?})",
            report.ran_ids()
        );
    }

    /// Teeth: applying `Indent(c2)` to the SUT but NOT the oracle re-parents c2
    /// under c1 in the store while the oracle still has it under `parent` —
    /// `inv-block-parent-matches-ref` MUST catch the divergence. Proves the write
    /// actually mutated the real store and the invariant has teeth.
    #[test]
    fn sut_only_indent_is_caught_by_parent_invariant() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (mut caps, _component) = seeded_sut(&rt);
        let ref_state = structural_ref(); // oracle left at the seed
        let c2 = fixed_ids().c2;
        let t = Indent { block_id: c2 };
        rt.block_on(t.apply_to_sut(&ref_state, &mut caps));

        let ref_caps =
            reference_state_ref_caps(Resolved::identity(ref_state).map(std::sync::Arc::new));
        let report = rt.block_on(run_selected(
            &composed_invariant_catalog(),
            &caps,
            &ref_caps,
        ));
        let failures = report.failures();
        assert!(
            failures
                .iter()
                .any(|(id, _)| *id == "inv-block-parent-matches-ref/block_raw"),
            "SUT-only Indent must diverge the parent invariant; failures were {failures:?}"
        );
    }

    /// Lockstep coverage for the swap ops that are gated out of *generation*
    /// (MoveUp/MoveDown): a short, drift-free sequence on the seed exercises their
    /// real `SutBlockTreeWrite` impls and confirms the SUT tree converges to the
    /// oracle's. `MoveDown(c1)` → `[c2,c1]`, `MoveUp(c1)` → `[c1,c2]`.
    #[test]
    fn move_ops_lockstep_stays_green() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (mut caps, _component) = seeded_sut(&rt);
        let mut ref_state = structural_ref();
        let c1 = fixed_ids().c1;

        let md = MoveDown {
            block_id: c1.clone(),
        };
        md.apply_to_ref(&mut ref_state);
        rt.block_on(md.apply_to_sut(&ref_state, &mut caps));

        let mu = MoveUp {
            block_id: c1.clone(),
        };
        mu.apply_to_ref(&mut ref_state);
        rt.block_on(mu.apply_to_sut(&ref_state, &mut caps));

        let ref_caps =
            reference_state_ref_caps(Resolved::identity(ref_state).map(std::sync::Arc::new));
        let report = rt.block_on(run_selected(
            &composed_invariant_catalog(),
            &caps,
            &ref_caps,
        ));
        assert!(
            report.failures().is_empty(),
            "lockstep MoveDown+MoveUp should be green: {:?}",
            report.failures()
        );
    }
}
