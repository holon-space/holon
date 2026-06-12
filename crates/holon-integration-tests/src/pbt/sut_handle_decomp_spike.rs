//! SutHandle-decomposition de-risking spikes (EXP-2/3) — see
//! `docs/Testing/PbtCompositionBacklog.md` "The decomposition mastermind plan".
//!
//! The make-or-break the single-cluster slices do NOT prove: can a composed
//! `CapMap` drive structural writes over a **real async backend** (Turso), where
//! `split_block` mints a fresh real id the oracle didn't predict — requiring a
//! synthetic-id map-back the synchronous `MemoryBackend` slice sidesteps with
//! `set_next_split_id`?
//!
//! Built in cuts, cheapest first (Step-A discipline):
//! - **C1** (`exp2_async_split_through_capmap_mints_real_id`): the bare async
//!   structural write path — drive `split_block` through the `CapMap`'s
//!   `SutBlockTreeWrite` cap (the reusable [`OpDispatchWriter`]) over a real Turso
//!   engine and observe a freshly-minted real id. Proves C1 (the path works) and
//!   exposes the id-mismatch C2 must reconcile.

// `new_sql_engine_with_structural_ops` was promoted from this spike into the SQL
// slice's builders (it is the async structural-write substrate composed slices
// reuse). Re-exported here so the spike's tests keep their `super::` references.
pub use crate::pbt::sql_slice::builders::{
    new_sql_engine_with_structural_ops, sql_structural_wide,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pbt::composed::composed_invariant_catalog;
    use crate::pbt::composed::seed_primitives::fixed_ids;
    use crate::pbt::composed::subsystem_seed::{build_started_ref, run_with_seeded_ref};
    use crate::pbt::invariants::registry::Subsystem;
    use crate::pbt::is_synthetic_ref_id;
    use crate::pbt::reference_state::ReferenceState;
    use crate::pbt::sql_slice::builders::sql_wide;
    use crate::pbt::sql_slice::components::SqlProjectionComponent;
    use crate::pbt::transitions::SplitBlock;
    use holon::api::BackendEngine;
    use holon_api::{Block, EntityUri};
    use holon_pbt_core::capabilities::{SutBackend, SutBlockTreeWrite};
    use holon_pbt_core::{TransitionImpl, TransitionRef};
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Arc;

    fn uri(s: &str) -> EntityUri {
        EntityUri::parse(s).expect("parse uri")
    }

    /// The oracle: a started ref with the fixed `parent/c1/c2` tree. **No focus
    /// navigation** — unlike the `MemoryBackend` structural slice, the SQL slice
    /// surfaces the `SutSqlProjection` focus caps (honest-empty), so a navigated
    /// oracle focus would false-RED `inv-navigation-focus`/`inv-focus-roots`
    /// against the empty SUT focus. `SplitBlock` needs only `RefBlockTree +
    /// RefLifecycle`, not focus, so an unfocused oracle is sufficient.
    fn structural_ref() -> ReferenceState {
        build_started_ref(&BTreeSet::<Subsystem>::new())
    }

    /// Drop a `ReferenceState` (which owns an `Arc<tokio::Runtime>`) off the async
    /// executor — dropping it inside a `#[tokio::test]` context panics.
    fn drop_ref_off_thread(state: ReferenceState) {
        std::thread::spawn(move || drop(state))
            .join()
            .expect("drop ReferenceState off the async executor");
    }

    /// Seed the Turso store with the fixed `parent/c1/c2` tree via the production
    /// create op, **exactly mirroring** `seed_primitives::seed_ref_tree` (parent
    /// under `no_parent`, same content constants) so the only ref↔SUT divergence
    /// is the split's minted id — the thing under test.
    pub(super) async fn seed_sql(engine: &Arc<BackendEngine>) {
        use crate::pbt::composed::seed_primitives::{C1, C2, PARENT};
        let c = SqlProjectionComponent::new(engine.clone());
        let ids = fixed_ids();
        c.create_block(&ids.parent, &EntityUri::no_parent(), PARENT)
            .await;
        c.create_block(&ids.c1, &ids.parent, C1).await;
        c.create_block(&ids.c2, &ids.parent, C2).await;
    }

    /// EXP-3 `ComposedRunner` reconciliation kernel (minimal): pair the oracle's
    /// freshly-minted **synthetic** split ids against the SUT's freshly-minted
    /// **real** ids and return a `synthetic → real` map. This is the async
    /// counterpart of `E2ESut::map_unmapped_split_synthetic_ids` — the part the
    /// `MemoryBackend` slice sidesteps with `set_next_split_id` (a real Turso
    /// `split_block` mints a `uuid`, not a hintable id). Pairs by id-emergence:
    /// the spike drives one split at a time, so exactly one of each appears.
    fn reconcile_split_ids(
        ref_state: &ReferenceState,
        sut_before: &BTreeSet<EntityUri>,
        sut_after: &BTreeSet<EntityUri>,
        already_mapped: &BTreeMap<EntityUri, EntityUri>,
    ) -> BTreeMap<EntityUri, EntityUri> {
        let synthetic: Vec<EntityUri> = ref_state
            .domain
            .block_state
            .blocks
            .keys()
            .filter(|id| is_synthetic_ref_id(id) && !already_mapped.contains_key(id))
            .cloned()
            .collect();
        let real_new: Vec<EntityUri> = sut_after.difference(sut_before).cloned().collect();
        assert_eq!(
            synthetic.len(),
            real_new.len(),
            "reconcile: one synthetic per minted real id (synthetic={synthetic:?}, real_new={real_new:?})"
        );
        let mut map = already_mapped.clone();
        for (syn, real) in synthetic.into_iter().zip(real_new) {
            map.insert(syn, real);
        }
        map
    }

    pub(super) async fn sut_ids(caps: &holon_pbt_core::composition::CapMap) -> BTreeSet<EntityUri> {
        caps.expect::<dyn SutBackend>()
            .block_raw_snapshot()
            .await
            .into_iter()
            .map(|b| b.id.clone())
            .collect()
    }

    /// **C1 — the async structural write path works through the composed `CapMap`.**
    /// Seed `parent` + `c1`("hello world") over a real Turso engine, then drive
    /// `apply_split_block(c1, 5)` *through the `CapMap`'s `SutBlockTreeWrite` cap*
    /// (the reusable `OpDispatchWriter` → production `split_block` op). Assert the
    /// store grew by one block and `c1` was truncated to "hello" — proving the
    /// production structural op ran via the composition path over an async backend.
    /// The new block carries a fresh **real** id (a `uuid`), distinct from any
    /// oracle synthetic `block::split-N` — the mismatch the EXP-3 `ComposedRunner`
    /// must reconcile.
    #[tokio::test]
    async fn exp2_async_split_through_capmap_mints_real_id() {
        let engine = new_sql_engine_with_structural_ops().await;

        // Seed a parent with one text child via the production create op.
        let seeder = SqlProjectionComponent::new(engine.clone());
        let parent = uri("block:parent");
        let c1 = uri("block:c1");
        seeder
            .create_block(&parent, &uri("block:root"), "parent")
            .await;
        seeder.create_block(&c1, &parent, "hello world").await;

        let caps = sql_wide(engine.clone());

        let before: std::collections::BTreeSet<EntityUri> = caps
            .expect::<dyn SutBackend>()
            .block_raw_snapshot()
            .await
            .into_iter()
            .map(|b| b.id.clone())
            .collect();
        assert!(before.contains(&c1), "seed present");

        // Drive the structural split THROUGH the composed CapMap cap (not the
        // component directly) — this is the composition-path proof.
        SutBlockTreeWrite::apply_split_block(&caps, &c1, 5).await;

        let after: Vec<Block> = caps.expect::<dyn SutBackend>().block_raw_snapshot().await;
        let after_ids: std::collections::BTreeSet<EntityUri> =
            after.iter().map(|b| b.id.clone()).collect();

        // Exactly one new block appeared, and it is NOT in the pre-split id set
        // (a freshly minted real id — the C2 reconciliation target).
        let minted: Vec<&EntityUri> = after_ids.difference(&before).collect();
        assert_eq!(
            minted.len(),
            1,
            "split should mint exactly one new block (before={before:?}, after={after_ids:?})"
        );
        let minted_id = minted[0];
        assert!(
            !minted_id.to_string().contains("split"),
            "minted id is a real backend id, not a synthetic oracle slot: {minted_id}"
        );

        // c1 truncated to the pre-cursor text; the new block holds the remainder.
        let c1_content = after.iter().find(|b| b.id == c1).map(|b| b.content.clone());
        assert_eq!(
            c1_content.as_deref(),
            Some("hello"),
            "c1 truncated at position 5"
        );
    }

    /// **C2 (positive) — the async make-or-break: split → reconcile → invariants
    /// pass against the real `ReferenceState` oracle.** Drive `SplitBlock(c1)` on
    /// BOTH the oracle (mints a synthetic `block::split-N`) and the composed
    /// `CapMap` over Turso (mints a real `uuid`), then run the EXP-3 reconciliation
    /// (`synthetic → real` map) + `with_resolved_doc_uris` and run the shared
    /// catalog. The block-tree invariants must agree — proving the synthetic-id
    /// map-back the `MemoryBackend` slice sidesteps works over an async backend
    /// that mints its own ids. This is the doc's open question, answered.
    #[tokio::test]
    async fn exp3_async_split_reconciled_against_oracle_passes_invariants() {
        let engine = new_sql_engine_with_structural_ops().await;
        seed_sql(&engine).await;
        let mut caps = sql_wide(engine.clone());
        let mut oracle = structural_ref();
        let c1 = fixed_ids().c1;

        let before = sut_ids(&caps).await;
        let split = SplitBlock {
            block_id: c1.clone(),
            position: 1,
        };
        split.apply_to_ref(&mut oracle); // oracle mints block::split-N
        TransitionImpl::apply_to_sut(&split, &oracle, &mut caps).await; // SUT mints uuid
        let after = sut_ids(&caps).await;

        let map = reconcile_split_ids(&oracle, &before, &after, &BTreeMap::new());
        assert_eq!(
            map.len(),
            1,
            "exactly one synthetic→real mapping for one split"
        );
        let resolved = oracle.with_resolved_doc_uris(&map);
        drop_ref_off_thread(oracle);

        let report = run_with_seeded_ref(&composed_invariant_catalog(), &caps, resolved).await;
        assert!(
            report.failures().is_empty(),
            "reconciled async split: block-tree invariants must agree with the oracle: {:?}",
            report.failures()
        );
        for id in [
            "inv-no-orphan-blocks",
            "inv-blocks-match-ref/block_raw",
            "inv-block-parent-matches-ref/block_raw",
        ] {
            assert!(
                report.ran_ids().contains(&id),
                "non-vacuity: {id} must run over real data (ran: {:?})",
                report.ran_ids()
            );
        }
    }

    /// **C2 (teeth) — the reconciliation is load-bearing.** Same async split, but
    /// SKIP the map-back: the oracle still carries the synthetic `block::split-N`,
    /// the SUT the real `uuid`, so `inv-blocks-match-ref` MUST catch the id
    /// divergence. Proves the C2 positive isn't passing vacuously and the
    /// invariants have teeth over the async store.
    #[tokio::test]
    async fn exp3_unreconciled_split_is_caught() {
        let engine = new_sql_engine_with_structural_ops().await;
        seed_sql(&engine).await;
        let mut caps = sql_wide(engine.clone());
        let mut oracle = structural_ref();
        let c1 = fixed_ids().c1;

        let split = SplitBlock {
            block_id: c1,
            position: 1,
        };
        split.apply_to_ref(&mut oracle);
        TransitionImpl::apply_to_sut(&split, &oracle, &mut caps).await;

        // NO reconciliation: empty map ⇒ the ref keeps the synthetic id.
        let resolved = oracle.with_resolved_doc_uris(&BTreeMap::new());
        drop_ref_off_thread(oracle);
        let report = run_with_seeded_ref(&composed_invariant_catalog(), &caps, resolved).await;
        assert!(
            !report.failures().is_empty(),
            "without the synthetic→real map-back the id divergence MUST be caught \
             (the make-or-break would be vacuous otherwise)"
        );
    }
}

/// **The full proptest `StateMachineTest`** — the multi-tick make-or-break.
///
/// Drives the mixed structural alphabet `{SplitBlock, Indent, Outdent}` over a
/// composed `CapMap` (real Turso engine + the id-resolving `OpDispatchWriter`)
/// against the real `ReferenceState` oracle, with **per-tick** synthetic→real
/// reconciliation accumulated into a shared `IdResolver`. This is the genuine
/// EXP-2/3 endgame at scale: every tick the SUT mints/moves real ids, the writer
/// resolves the oracle's (possibly synthetic) ids to the store's real ids, and the
/// shared catalog's block-tree invariants are checked against the reconciled
/// oracle. No CDC settle is needed — the block-tree invariants read the `block_raw`
/// base table, which is synchronously consistent over in-memory Turso.
///
/// The SUT registers only `SutBackend` + `SutBlockTreeWrite` (NOT
/// `SutSqlProjection`): the oracle navigates focus so the structural generators
/// have candidates, and omitting the focus-projection cap deselects
/// `inv-navigation-focus`/`inv-focus-roots` (the SQL slice has no navigation
/// engine — an empty focus projection would false-RED against the focused oracle).
#[cfg(test)]
mod state_machine {
    use super::tests::sut_ids;
    use super::{new_sql_engine_with_structural_ops, sql_structural_wide};
    use crate::pbt::composed::composed_invariant_catalog;
    use crate::pbt::composed::seed_primitives::{C1, C2, PARENT, fixed_ids};
    use crate::pbt::composed::subsystem_seed::{build_started_ref, run_with_seeded_ref};
    use crate::pbt::invariants::registry::Subsystem;
    use crate::pbt::is_synthetic_ref_id;
    use crate::pbt::op_write_cap::IdResolver;
    use crate::pbt::reference_state::ReferenceState;
    use crate::pbt::sql_slice::components::SqlProjectionComponent;
    use crate::pbt::transitions::{Indent, JoinBlock, NavigateFocus, Outdent, SplitBlock};
    use holon::api::BackendEngine;
    use holon_api::block::Block;
    use holon_api::{EntityUri, Region};
    use holon_pbt_core::composition::CapMap;
    use holon_pbt_core::{TransitionImpl, TransitionRef, weighted_arm};
    use proptest::prelude::Just;
    use proptest::strategy::{BoxedStrategy, Strategy, Union};
    use proptest_state_machine::{ReferenceStateMachine, StateMachineTest, prop_state_machine};
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::{Arc, Mutex};
    use validated::Validated;

    // Full structural alphabet over a **production-faithful Page-rooted seed**
    // (`page → parent → c1/c2`, focus on `parent`). Because every block now sits
    // under a real parent (the page), `split`/`indent` never reach `no_parent`, so
    // the NULL-parent constraint (which the `MemoryBackend` slice masks) is avoided
    // — re-admitting `Outdent`/`Join`/`Move`. The ONE residual escape is outdenting
    // a *direct child of the page* (its grandparent is `no_parent`), gated below.
    #[derive(Clone, Debug)]
    enum SqlTransition {
        SplitBlock(SplitBlock),
        Indent(Indent),
        Outdent(Outdent),
        JoinBlock(JoinBlock),
        /// Always-applicable no-op, so generation never dead-ends when a sequence of
        /// joins/outdents empties the focused subtree (the production `Nothing` binds
        /// `S: SutHandle`, which a `CapMap` does not satisfy, so we use a local unit
        /// variant that touches neither the oracle nor the SUT).
        Nothing,
    }

    /// `block:page` — the seed document root above the focus. A seed (excluded from
    /// the non-seed block comparison on both sides), so it is never operated on and
    /// keeps its non-NULL `no_parent` sentinel — production `QueryableCache` happy.
    fn page_id() -> EntityUri {
        EntityUri::block("page")
    }

    /// Outdent moves a block to its grandparent. Gate it when the grandparent is
    /// `no_parent` (i.e. the target's parent is the page itself) — that lone case
    /// would write a NULL-parent top-level text block the production store rejects.
    fn outdent_escapes_to_no_parent(state: &ReferenceState, id: &EntityUri) -> bool {
        let blocks = &state.domain.block_state.blocks;
        match blocks.get(id).and_then(|b| blocks.get(&b.parent_id)) {
            Some(parent) => parent.parent_id.is_no_parent(),
            None => true, // missing ancestor → treat as escape (gate out)
        }
    }

    /// True iff `id` has a previous sibling (a same-parent block with a lower
    /// `sequence`). `JoinBlock` is only oracle↔SUT-faithful for a non-first child
    /// (merge into the prev sibling); the oracle additionally models joining a
    /// *first* child as "promote children to the grandparent + delete", which the
    /// production `join_block` (no prev sibling) does not do — a real semantic-edge
    /// divergence, so we gate first-child joins out of generation.
    fn has_prev_sibling(state: &ReferenceState, id: &EntityUri) -> bool {
        use holon_orgmode::OrgBlockExt;
        let blocks = &state.domain.block_state.blocks;
        let Some(b) = blocks.get(id) else {
            return false;
        };
        let my_seq = b.sequence();
        blocks
            .values()
            .any(|o| o.parent_id == b.parent_id && o.id != b.id && o.sequence() < my_seq)
    }

    /// Production-faithful oracle: `page` (seed doc root) → `parent` → `c1`/`c2`,
    /// focus on `parent`. Re-roots `seed_ref_tree`'s `parent` under the page so the
    /// whole working tree is a descendant of a real document (nothing under
    /// `no_parent` except the page seed), matching how production stores blocks.
    fn page_rooted_ref() -> ReferenceState {
        let mut state = build_started_ref(&BTreeSet::<Subsystem>::new());
        let ids = fixed_ids();
        let page = page_id();
        // The page is a seed (excluded from the non-seed comparison), so its
        // sibling sequence is irrelevant — no `set_sequence` needed.
        let pageb = Block::new_text(page.clone(), EntityUri::no_parent(), "page");
        state.domain.block_state.blocks.insert(page.clone(), pageb);
        // Seed classification: a `block_documents` entry whose doc `is_no_parent`
        // marks the page as a seed (so `seed_block_ids` excludes it on both sides).
        state
            .domain
            .block_state
            .block_documents
            .insert(page.clone(), EntityUri::no_parent());
        if let Some(p) = state.domain.block_state.blocks.get_mut(&ids.parent) {
            p.parent_id = page.clone();
        }
        NavigateFocus {
            region: Region::Main,
            block_id: ids.parent,
        }
        .apply_to_ref(&mut state);
        state
    }

    /// Seed the Turso store to match `page_rooted_ref`: `page` (under `no_parent`),
    /// `parent` under `page`, `c1`/`c2` under `parent`.
    async fn seed_sql_page_rooted(engine: &Arc<BackendEngine>) {
        let c = SqlProjectionComponent::new(engine.clone());
        let ids = fixed_ids();
        let page = page_id();
        c.create_block(&page, &EntityUri::no_parent(), "page").await;
        c.create_block(&ids.parent, &page, PARENT).await;
        c.create_block(&ids.c1, &ids.parent, C1).await;
        c.create_block(&ids.c2, &ids.parent, C2).await;
    }

    fn sql_aggregate(state: &ReferenceState) -> BoxedStrategy<SqlTransition> {
        let mut arms: Vec<(u32, BoxedStrategy<SqlTransition>)> = vec![];
        macro_rules! arm {
            ($ty:ty, $variant:path) => {
                if let Validated::Good(Some(a)) =
                    weighted_arm::<_, $ty, SqlTransition>(state, 1, $variant)
                {
                    arms.push(a);
                }
            };
        }
        arm!(SplitBlock, SqlTransition::SplitBlock);
        arm!(Indent, SqlTransition::Indent);
        arm!(Outdent, SqlTransition::Outdent);
        arm!(JoinBlock, SqlTransition::JoinBlock);
        // MoveUp/MoveDown are NOT in this alphabet (same rationale as
        // `memory_slice::structural_pbt`): they are pure sibling-*order* swaps, and
        // the oracle orders by `sequence()` while the SUT orders by the SQL
        // `sort_key` fractional index. No invariant compares child order, so a swap
        // is silent until a later order-dependent op (indent/join prev-sibling)
        // targets a different sibling on each side. Re-admitting them needs an
        // explicit order-fidelity check (oracle `sequence` ↔ SUT `sort_key`).
        // Always-on no-op arm — guarantees a non-empty strategy even when the
        // structural generators have no candidates (emptied focus subtree).
        arms.push((1, Just(SqlTransition::Nothing).boxed()));
        Union::new_weighted(arms).boxed()
    }

    struct SqlMachine;
    impl ReferenceStateMachine for SqlMachine {
        type State = ReferenceState;
        type Transition = SqlTransition;

        fn init_state() -> BoxedStrategy<Self::State> {
            Just(page_rooted_ref()).boxed()
        }

        fn transitions(state: &Self::State) -> BoxedStrategy<Self::Transition> {
            sql_aggregate(state)
        }

        fn preconditions(state: &Self::State, transition: &Self::Transition) -> bool {
            match transition {
                SqlTransition::SplitBlock(t) => t.preconditions(state).is_good(),
                SqlTransition::Indent(t) => t.preconditions(state).is_good(),
                SqlTransition::Outdent(t) => {
                    t.preconditions(state).is_good()
                        && !outdent_escapes_to_no_parent(state, &t.block_id)
                }
                SqlTransition::JoinBlock(t) => {
                    t.preconditions(state).is_good() && has_prev_sibling(state, &t.block_id)
                }
                SqlTransition::Nothing => true,
            }
        }

        fn apply(mut state: Self::State, transition: &Self::Transition) -> Self::State {
            match transition {
                SqlTransition::SplitBlock(t) => t.apply_to_ref(&mut state),
                SqlTransition::Indent(t) => t.apply_to_ref(&mut state),
                SqlTransition::Outdent(t) => t.apply_to_ref(&mut state),
                SqlTransition::JoinBlock(t) => t.apply_to_ref(&mut state),
                SqlTransition::Nothing => {}
            }
            state
        }
    }

    /// The composed SUT: a `CapMap` over a real Turso engine (`SutBackend` +
    /// id-resolving `SutBlockTreeWrite`) plus the shared `IdResolver` the harness
    /// populates each tick. No `SutSqlProjection` (see module doc).
    struct SqlStructuralSut {
        caps: CapMap,
        resolver: IdResolver,
        rt: tokio::runtime::Runtime,
    }

    impl StateMachineTest for SqlStructuralSut {
        type SystemUnderTest = Self;
        type Reference = SqlMachine;

        fn init_test(_: &ReferenceState) -> Self {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("current-thread runtime");
            let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
            let caps: CapMap = rt.block_on(async {
                let engine = new_sql_engine_with_structural_ops().await;
                seed_sql_page_rooted(&engine).await;
                sql_structural_wide(engine, resolver.clone())
            });
            Self { caps, resolver, rt }
        }

        fn apply(mut sut: Self, ref_state: &ReferenceState, transition: SqlTransition) -> Self {
            let (before, after) = {
                let caps = &mut sut.caps;
                sut.rt.block_on(async move {
                    let before = sut_ids(caps).await;
                    match &transition {
                        SqlTransition::SplitBlock(t) => t.apply_to_sut(ref_state, caps).await,
                        SqlTransition::Indent(t) => t.apply_to_sut(ref_state, caps).await,
                        SqlTransition::Outdent(t) => t.apply_to_sut(ref_state, caps).await,
                        SqlTransition::JoinBlock(t) => t.apply_to_sut(ref_state, caps).await,
                        SqlTransition::Nothing => {}
                    }
                    let after = sut_ids(caps).await;
                    (before, after)
                })
            };
            // Per-tick reconciliation: a single transition mints at most one block,
            // so the unmapped synthetic id (oracle, post-apply) pairs 1:1 with the
            // new real id (SUT). Accumulate into the shared resolver.
            let mut map = sut.resolver.lock().expect("resolver lock");
            let synthetic: Vec<EntityUri> = ref_state
                .domain
                .block_state
                .blocks
                .keys()
                .filter(|id| is_synthetic_ref_id(id) && !map.contains_key(id))
                .cloned()
                .collect();
            let real_new: Vec<EntityUri> = after.difference(&before).cloned().collect();
            assert_eq!(
                synthetic.len(),
                real_new.len(),
                "per-tick reconcile: one synthetic per minted real id (syn={synthetic:?}, real={real_new:?})"
            );
            for (syn, real) in synthetic.into_iter().zip(real_new) {
                map.insert(syn, real);
            }
            drop(map);
            sut
        }

        fn check_invariants(sut: &Self, ref_state: &ReferenceState) {
            let map = sut.resolver.lock().expect("resolver lock").clone();
            let resolved = ref_state.with_resolved_doc_uris(&map);
            let report = sut.rt.block_on(run_with_seeded_ref(
                &composed_invariant_catalog(),
                &sut.caps,
                resolved,
            ));
            assert!(
                report.failures().is_empty(),
                "reconciled async structural sequence diverged from the oracle: {:?}",
                report.failures()
            );
            for id in [
                "inv-no-orphan-blocks",
                "inv-no-parent-cycles",
                "inv-blocks-match-ref/block_raw",
                "inv-block-parent-matches-ref/block_raw",
            ] {
                assert!(
                    report.ran_ids().contains(&id),
                    "non-vacuity: {id} must run over real data (ran: {:?})",
                    report.ran_ids()
                );
            }
        }
    }

    prop_state_machine! {
        #![proptest_config(proptest::test_runner::Config {
            cases: 48,
            max_shrink_iters: 200,
            .. proptest::test_runner::Config::default()
        })]
        #[test]
        fn exp2_full_async_structural_state_machine(sequential 1..12 => SqlStructuralSut);
    }
}

/// **EXP-4 — the `StartApp`/lifecycle model decision.** Proves model (b): a
/// component can boot its backend **lazily through `&self`** (interior mutability),
/// so `StartApp` can be a real composed transition rather than forced into
/// `init_test`. Pre-start reads are honest-absent (`None`), not faked. Combined
/// with the generic fact that `#[capmap_adapter]` already hosts any `&self` async
/// cap on `CapMap` (proven by `SutFocusWrite`/`SutQuiesce`/…), this de-risks
/// hosting a `&self` `SutLifecycle` cap — the one hard case in risk A.
///
/// (We do NOT flip the shared `SutLifecycle` trait here: `E2ESut` impls it
/// `&mut self` because its `start_app` builds fields in place. The decision this
/// probe supports: give the *composed* lifecycle cap a `&self` signature backed by
/// interior-mut lazy boot; `E2ESut` keeps its `&mut self` impl until it is retired.)
#[cfg(test)]
mod lifecycle_probe {
    use super::new_sql_engine_with_structural_ops;
    use crate::pbt::sql_slice::components::SqlProjectionComponent;
    use holon::api::BackendEngine;
    use holon_pbt_core::capabilities::SutBackend;
    use std::sync::{Arc, Mutex};

    /// A component whose backend is booted lazily via `&self` (the `StartApp`
    /// analog). `Mutex<Option<…>>` is the interior-mut seam; production would use a
    /// `tokio::sync::OnceCell` to boot once. Reads return honest-absent before start.
    struct LifecycleProbe {
        engine: Mutex<Option<Arc<BackendEngine>>>,
    }

    impl LifecycleProbe {
        fn new() -> Self {
            Self {
                engine: Mutex::new(None),
            }
        }

        /// The `&self` `apply_start_app`: boot the real backend and latch it. Await
        /// happens OUTSIDE the lock (no lock held across `.await`); idempotent.
        async fn start(&self) {
            if self.engine.lock().expect("lock").is_some() {
                return;
            }
            let engine = new_sql_engine_with_structural_ops().await;
            // Seed something observable so a post-start read is non-empty.
            let seeder = SqlProjectionComponent::new(engine.clone());
            let root = crate::pbt::composed::seed_primitives::fixed_ids().parent;
            seeder
                .create_block(&root, &holon_api::EntityUri::no_parent(), "seed")
                .await;
            *self.engine.lock().expect("lock") = Some(engine);
        }

        fn is_started(&self) -> bool {
            self.engine.lock().expect("lock").is_some()
        }

        /// Honest read: `None` before start (capability absent), `Some(count)` after.
        async fn block_count(&self) -> Option<usize> {
            let engine = self.engine.lock().expect("lock").clone()?;
            Some(
                SqlProjectionComponent::new(engine)
                    .block_raw_snapshot()
                    .await
                    .len(),
            )
        }
    }

    #[tokio::test]
    async fn exp4_lifecycle_lazy_self_boot_with_honest_pre_start_reads() {
        let probe = LifecycleProbe::new();
        // Pre-start: not started, reads are honest-absent (not a fabricated empty).
        assert!(!probe.is_started(), "not started before StartApp");
        assert_eq!(
            probe.block_count().await,
            None,
            "pre-start read must be honest-absent, not a faked empty"
        );

        // The `&self` StartApp boots the real backend into the interior-mut cell.
        probe.start().await;

        assert!(probe.is_started(), "started after StartApp");
        assert!(
            probe.block_count().await.unwrap_or(0) >= 1,
            "post-start read sees the booted+seeded backend"
        );

        // Idempotent: a second StartApp is a no-op (latched).
        probe.start().await;
        assert!(probe.is_started());
    }
}
