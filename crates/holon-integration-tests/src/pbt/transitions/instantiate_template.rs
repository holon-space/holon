//! Transition: instantiate a canned inline template through the production
//! `block.instantiate_template` operation. The SUT-side driver first seeds
//! the template blocks (idempotent `block.create`), then dispatches
//! `instantiate_template`. The ref side models the expected instance blocks
//! using the production deterministic-instance-id function, so existing
//! block-comparison invariants verify the result.
//!
//! @pbt rung dispatch
//!   `instantiate_template` mints template blocks via `block.create` dispatch;
//!   storage-only pin uses the op-floor SutTemplateInstantiate. No template
//!   gesture verified (see UNCERTAIN).
//! @pbt covers template-instantiate — canned inline template -> block.create
//! fan-out

use holon_api::EntityUri;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefBlockTreeMut;
use holon_pbt_core::capabilities::RefLayoutMutate;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// Canned template root id (matches `instantiate_template_tests` in
/// operation_engine.rs).
const TPL_ROOT: &str = "block:tpl";
const TPL_CHILD: &str = "block:tpl-c1";

/// Instantiate the canned template under an existing non-page block.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct InstantiateTemplate {
    pub parent_id: EntityUri,
    pub date: String,
    pub mood: String,
}

impl InstantiateTemplate {
    /// Per-instantiation context key. Production hands a FRESH key to every
    /// manual instantiation (`command_provider.rs`: `manual:{uuid}`) so each is
    /// a distinct instance; a fixed constant instead mints one colliding
    /// deterministic id for every draw, so a second `InstantiateTemplate` under
    /// a different parent degenerates into relocating the first instance
    /// (observed: "Cyclic move detected" when the second draw targets the first
    /// instance's own root). Derive a REPLAYABLE key from the draw's own fields
    /// so distinct draws mint distinct instances while an identical re-draw
    /// stays the idempotent re-fire the engine expects. Both the SUT dispatch
    /// and `apply_to_ref` read this, so their `deterministic_instance_id`s
    /// agree (the id space is oracle-side `parent_id`, identical before
    /// resolution).
    fn context_key(&self) -> String {
        format!("pbt:{}:{}:{}", self.parent_id, self.date, self.mood)
    }

    /// Valid instantiation targets: plain outline text blocks the user could
    /// focus and instantiate a template under. Excludes pages, SOURCE blocks
    /// (render/query/`holon_rule` sources — the engine's placement re-homes an
    /// instantiation off a source block onto its nearest container, e.g. off
    /// the boot `journals::action::0` rule onto its `journals::auto-create`
    /// heading; the oracle does not model that redirect, so keep source blocks
    /// out of the target set rather than import placement mechanics), and
    /// layout scaffolding. Shared by the generator and the precondition so
    /// replay gating agrees with generation.
    fn target_candidates<R: RefBlockTree>(state: &R) -> Vec<EntityUri> {
        state
            .all_non_seed_block_ids()
            .into_iter()
            .filter(|id| {
                state.is_text_block(id)
                    && !state.is_page_block(id)
                    && !state.is_no_content_update(id)
                    && !state.is_layout_block(id)
            })
            .collect()
    }
}

impl<R: RefLifecycle + RefBlockTree + RefLayoutMutate> TransitionFactory<R>
    for InstantiateTemplate
{
    fn required_caps() -> Vec<holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let candidates = Self::target_candidates(state);
        check(!candidates.is_empty(), Reason::PreconditionFailed).map(|_| {
            let strat = (prop::sample::select(candidates), "[a-z]{3,6}", "[a-z]{3,6}")
                .prop_map(|(parent_id, date, mood)| InstantiateTemplate {
                    parent_id,
                    date,
                    mood,
                })
                .boxed();
            (2, strat)
        })
    }
}

impl<R: RefLifecycle + RefBlockTree + RefBlockTreeMut + RefLayoutMutate> TransitionRef<R>
    for InstantiateTemplate
{
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(
                !Self::target_candidates(state).is_empty(),
                Reason::PreconditionFailed,
            ),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        // Model the template DEFINITION blocks the SUT driver seeds (idempotent
        // `block.create`) so the composed per-tick reconcile treats them as
        // oracle-held rather than surprise real-id mints, keeping the deep block
        // comparison balanced.
        //
        // These two fixed-id blocks are template-library scaffolding, not user
        // content — template-engine internals the reference deliberately does not
        // reproduce. `seed_template_definition` classifies them as SEED, which
        // excludes them from every block/children comparison on both sides, while
        // keeping `parent_id` truthful: the driver seeds the child UNDER the root,
        // and invariants that apply no seed filter (`inv-watch-rows-match-ref`, whose
        // AllBlocks watch returns every block) read that field directly. The driver
        // seeds root and child via two INDEPENDENT idempotent `block.create`s, so
        // guard each on ITS OWN existence (a shared root-only guard skips re-seeding
        // the child when only the child's create was undone). Each create snapshots
        // to stay 1:1 with the driver's separate User-origin create ops.
        let tpl_root = EntityUri::from_raw(TPL_ROOT);
        let tpl_child = EntityUri::from_raw(TPL_CHILD);
        if state.block_content(&tpl_root).is_none() {
            state.seed_template_definition(&EntityUri::no_parent(), "{{date}}", tpl_root.clone());
        }
        if state.block_content(&tpl_child).is_none() {
            state.seed_template_definition(&tpl_root, "see {{date}} now", tpl_child);
        }

        // The SUT's `plan_instantiation` mints deterministic instance ids from
        // `(template_id, context_key, node.id)` where the template nodes keep
        // their seeded ids `block:tpl` / `block:tpl-c1`; compute the SAME ids so
        // the instance blocks are born-equal (no synthetic→real reconcile).
        let ctx_key = self.context_key();
        let inst_root_id =
            holon_api::effect_id::deterministic_instance_id(TPL_ROOT, &ctx_key, TPL_ROOT);
        let inst_child_id =
            holon_api::effect_id::deterministic_instance_id(TPL_ROOT, &ctx_key, TPL_CHILD);

        // ONE undoable unit: `apply_instantiate_template` snapshots the
        // pre-instantiation tree exactly once and inserts BOTH instance blocks
        // without per-block snapshots, so a single `UndoLastMutation` removes the
        // WHOLE instantiation — matching the SUT's composite undo group. Per-block
        // snapshots would leave the instance root behind on undo.
        state.apply_instantiate_template(
            &self.parent_id,
            inst_root_id,
            inst_child_id,
            &self.date,
            &format!("see {} now", self.date),
            TPL_ROOT,
        );
    }
}

crate::cap_transition! {
    InstantiateTemplate: holon_pbt_core::capabilities::SutTemplateInstantiate,
    where R: [ RefLifecycle + RefBlockTree ],
    |me, _state, sut| {
        let bindings = vec![
            ("date".to_string(), me.date.clone()),
            ("mood".to_string(), me.mood.clone()),
        ];
        sut.instantiate_template(
            &EntityUri::from_raw(TPL_ROOT),
            &me.parent_id,
            &me.context_key(),
            &bindings,
        ).await;
    }
    sql_budget: |_me, state| {
        let blocks = state.block_count();
        ExpectedSql {
            reads: blocks + 10,
            writes: 6,
            ddl: 0,
            tolerance: 2,
        }
    }
}
