//! `cdc_delivery_pbt` — Phase B follow-up slice (PbtSlicing.md candidate #2).
//!
//! # Why this slice exists
//!
//! Targets the matview→CDC→watch delivery chain. MEMORY.md lists four
//! historically-painful Turso IVM bug classes — `json_group_array` multiset
//! negative, MatchCounterOperator Uninitialized, `focus_roots` LEFT-JOIN drop,
//! IVM matview-cursor first-open empty — every one of them surfaces in the
//! same mechanism: a watch subscriber reads stale or extra rows from a
//! materialised view after a write that should have propagated through the
//! CDC pipeline.
//!
//! The wide PBT catches these but at minutes/case; `storage_consistency_pbt`
//! catches them at ~7.7 s/case but exercises peer-merge / org-file / chord-op
//! transitions that aren't the dominant suspects for matview drift. This
//! slice narrows to **storage mutations + watch lifecycle**, concentrating
//! the proptest shrinker on the transitions most likely to trip a CDC bug.
//!
//! # Composition
//!
//! - **SUT variant**: `E2ESut<SqlOnly>` — same slim backend (Turso + Loro,
//!   no ReactiveEngine / GPUI / driver) used by `storage_consistency_pbt`.
//!   A future slim-SUT pass (Phase 9 follow-up) could shave more startup
//!   cost, but reusing this variant keeps the diff small and validates the
//!   slicing framework's *composability*, not a one-off SUT.
//! - **Reference variant**: `VariantRef<SqlOnly>` — same ref model as the
//!   storage slice.
//! - **Transitions** (narrowed from storage's 16 → 8):
//!   - `StartApp` — required bootstrap.
//!   - `BulkExternalAdd`, `WriteOrgFile` (regular files only, see
//!     storage_consistency_pbt for the index.org race) — generate block_raw
//!     rows that hydrate through the `block` matview.
//!   - `SplitBlock`, `JoinBlock` — chord ops that hit `block_raw` +
//!     `block_tags` junction simultaneously (the LEFT-JOIN shape that
//!     produced multiple multiset-negative bugs).
//!   - `SetupWatch`, `RemoveWatch` — exercises the watch lifecycle the
//!     other PBTs only touch incidentally.
//!   - `TypeChars` — drives `block_raw.content` UPDATEs that previously
//!     triggered the no-op-UPDATE LEFT-JOIN drop bug (see MEMORY:
//!     `handoff_turso_ivm_focus_roots_2026-05-07.md`).
//!
//! Deliberately omitted: peer transitions (`AddPeer`/`PeerEdit`/
//! `SyncWithPeer`/`MergeFromPeer` — they bring Loro-CRDT churn that pollutes
//! the CDC focus); navigation (no UI in this slice); editor cursor moves
//! (no CDC writes).
//!
//! # Invariants
//!
//! - `InvLoroNoErrors` — required on any Loro-touching slice; matview-CDC
//!   bugs often present first as a `[LoroSyncController] Failed to apply`
//!   log line because the SQL→Loro mirror drops events the matview lost.
//! - `InvBlockTagsReferencesExist` — orphan `block_tags` rows are a direct
//!   matview-drift signature (the junction table outliving a `block_raw`
//!   delete is the exact shape that produces multiset-negative panics on
//!   the next aggregation).
//!
//! # What this slice does NOT yet catch
//!
//! `inv-watch-rows-match-ref` is the ideal invariant for this slice but
//! requires plumbing not yet built — `SutSqlProjection::watch_field_value`
//! + a `RefWatchQueries` cap covering `ref_state.query_results(watch_spec)`.
//! See `crates/holon-integration-tests/src/pbt/invariants/bodies/watch_rows_match_ref.rs`
//! for the blocker list. When that lands, this slice gains its
//! highest-leverage invariant for free.
//!
//! # Cost
//!
//! Targets ~30 s wall for 16 cases × 1..10 steps. Reuses `E2ESut<SqlOnly>`
//! init cost (~7-8 s/case), which is the dominant overhead. A slim
//! Turso-only SUT (no Loro) would unlock ~1 s/case but is a separate
//! refactor — see `docs/Testing/PbtSlicing.md` § "Adding a new slice".

use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::OnceLock;

use proptest::prelude::*;
use proptest::strategy::{BoxedStrategy, Union};
use proptest_state_machine::{ReferenceStateMachine, StateMachineTest, prop_state_machine};

use holon_integration_tests::pbt::transition_dispatch::E2ETransitionFactory;
use holon_integration_tests::pbt::transitions::E2ETransition;
use holon_integration_tests::pbt::transitions::E2ETransitionImpl;
use holon_integration_tests::pbt::{E2ESut, SqlOnly, VariantRef};

use holon_integration_tests::pbt::invariants::bodies::block_tags_references_exist::InvBlockTagsReferencesExist;
use holon_integration_tests::pbt::invariants::bodies::loro_no_errors::InvLoroNoErrors;
use holon_pbt_core::invariant::{Invariant, InvariantResult};

use holon_integration_tests::pbt::transitions::{
    BulkExternalAdd, JoinBlock, RemoveWatch, SetupWatch, SplitBlock, StartApp, TypeChars,
    WriteOrgFile,
};

// ── CDC-delivery transition generator ──────────────────────────────────
//
// Narrowed to the subset most likely to exercise the matview→CDC→watch
// chain. See module doc for the rationale per transition.
fn cdc_delivery_transitions(
    state: &holon_integration_tests::pbt::ReferenceState,
) -> BoxedStrategy<E2ETransition> {
    let mut arms: Vec<(u32, BoxedStrategy<E2ETransition>)> = Vec::new();

    macro_rules! try_add {
        ($ty:ty, $variant:expr) => {
            if let validated::Validated::Good((w, s)) =
                <$ty as E2ETransitionFactory>::weighted_generator(state)
            {
                arms.push((w, s.prop_map(|t| $variant(t)).boxed()));
            }
        };
    }

    try_add!(StartApp, E2ETransition::StartApp);

    // WriteOrgFile: filter to regular files only — same index.org race that
    // `storage_consistency_pbt` documents at line 130-150. The render+SQL
    // query blocks in index.org variants race with ReactiveEngine startup
    // and trip `assert_cdc_quiescent`.
    if let validated::Validated::Good((w, s)) =
        <WriteOrgFile as E2ETransitionFactory>::weighted_generator(state)
    {
        let filtered = s
            .prop_filter("skip index.org (CDC quiescence race)", |wof| {
                wof.filename != "index.org"
            })
            .prop_map(E2ETransition::WriteOrgFile)
            .boxed();
        arms.push((w, filtered));
    }

    try_add!(BulkExternalAdd, E2ETransition::BulkExternalAdd);
    try_add!(SplitBlock, E2ETransition::SplitBlock);
    try_add!(JoinBlock, E2ETransition::JoinBlock);
    try_add!(TypeChars, E2ETransition::TypeChars);
    try_add!(SetupWatch, E2ETransition::SetupWatch);
    try_add!(RemoveWatch, E2ETransition::RemoveWatch);

    assert!(
        !arms.is_empty(),
        "cdc_delivery_transitions: no transition applicable — \
         StartApp must be available pre-startup, storage mutations post-startup"
    );
    Union::new_weighted(arms).boxed()
}

// ── CdcDeliveryMachine: custom ReferenceStateMachine ─────────────────────

pub struct CdcDeliveryMachine;

impl ReferenceStateMachine for CdcDeliveryMachine {
    type State = VariantRef<SqlOnly>;
    type Transition = E2ETransition;

    fn init_state() -> BoxedStrategy<Self::State> {
        <VariantRef<SqlOnly> as ReferenceStateMachine>::init_state()
    }

    fn transitions(state: &Self::State) -> BoxedStrategy<Self::Transition> {
        cdc_delivery_transitions(state)
    }

    fn preconditions(state: &Self::State, transition: &Self::Transition) -> bool {
        use holon_integration_tests::pbt::validation::record_rejection;
        use validated::Validated;
        match transition.preconditions(state) {
            Validated::Good(()) => true,
            Validated::Fail(reasons) => {
                record_rejection(transition.variant_name(), &reasons);
                false
            }
        }
    }

    fn apply(mut state: Self::State, transition: &Self::Transition) -> Self::State {
        transition.apply_to_ref(&mut state);
        state.last_transition_kind = Some(transition.variant_name());
        state
    }
}

// ── CdcDeliverySut: StateMachineTest wrapper ─────────────────────────────

pub struct CdcDeliverySut {
    inner: E2ESut<SqlOnly>,
}

impl StateMachineTest for CdcDeliverySut {
    type SystemUnderTest = Self;
    type Reference = CdcDeliveryMachine;

    fn init_test(_: &<Self::Reference as ReferenceStateMachine>::State) -> Self::SystemUnderTest {
        static SHARED_RUNTIME: OnceLock<Arc<tokio::runtime::Runtime>> = OnceLock::new();
        let runtime = SHARED_RUNTIME
            .get_or_init(|| Arc::new(tokio::runtime::Runtime::new().unwrap()))
            .clone();
        CdcDeliverySut {
            inner: E2ESut::new(runtime).unwrap(),
        }
    }

    fn apply(
        mut sut: Self::SystemUnderTest,
        ref_state: &<Self::Reference as ReferenceStateMachine>::State,
        transition: <Self::Reference as ReferenceStateMachine>::Transition,
    ) -> Self::SystemUnderTest {
        let runtime = sut.inner.runtime.clone();
        runtime.block_on(sut.inner.apply_transition_async(ref_state, &transition));
        sut
    }

    fn check_invariants(
        sut: &Self::SystemUnderTest,
        ref_state: &<Self::Reference as ReferenceStateMachine>::State,
    ) {
        // Skip until StartApp has initialised the Turso engine — same gate
        // as storage_consistency_pbt; no SQL is queryable before then.
        use holon_pbt_core::capabilities::RefLifecycle;
        if !ref_state.app_started() {
            return;
        }
        let runtime = sut.inner.runtime.clone();
        let ref_inner: &holon_integration_tests::pbt::ReferenceState = &**ref_state;

        runtime.block_on(async {
            // inv-loro-no-errors — required for any Loro-touching slice.
            match InvLoroNoErrors.check(ref_inner, &sut.inner).await {
                InvariantResult::Ok => {}
                InvariantResult::Fail(msg) => panic!("{msg}"),
                InvariantResult::Skipped(_) => {}
            }
            // inv-block-tags-references-exist — direct matview-drift signature.
            match InvBlockTagsReferencesExist(PhantomData)
                .check(ref_inner, &sut.inner)
                .await
            {
                InvariantResult::Ok => {}
                InvariantResult::Fail(msg) => panic!("{msg}"),
                InvariantResult::Skipped(_) => {}
            }
        });
    }
}

// ── Test entry point ─────────────────────────────────────────────────────

prop_state_machine! {
    // Wall-clock target: ≤60 s wall. Reuses E2ESut<SqlOnly> init cost
    // (dominant overhead). A slim Turso-only SUT would unlock ~1 s/case
    // but is a separate refactor.
    //
    // 16 cases × 1..10 steps is enough density to hit watch lifecycle
    // (Setup → mutations → Remove) without runaway wall-clock. Shrinking
    // capped at 20 iters keeps failure reproduction fast.
    #![proptest_config(proptest::test_runner::Config {
        cases: 16,
        max_shrink_iters: 20,
        .. proptest::test_runner::Config::default()
    })]
    #[test]
    fn cdc_delivery_pbt(sequential 1..10 => CdcDeliverySut);
}
