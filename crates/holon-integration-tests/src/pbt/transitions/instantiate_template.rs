//! Transition: instantiate a canned inline template through the production
//! `block.instantiate_template` operation. The SUT-side driver first seeds
//! the template blocks (idempotent `block.create`), then dispatches
//! `instantiate_template`. The ref side models the expected instance blocks
//! using the production deterministic-instance-id function, so existing
//! block-comparison invariants verify the result.

use holon_api::EntityUri;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefBlockTree;
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
const CTX_KEY: &str = "pbt";

/// Instantiate the canned template under an existing non-page block.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct InstantiateTemplate {
    pub parent_id: EntityUri,
    pub date: String,
    pub mood: String,
}

impl<R: RefLifecycle + RefBlockTree + RefLayoutMutate> TransitionFactory<R>
    for InstantiateTemplate
{
    fn required_caps() -> Vec<holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let candidates: Vec<EntityUri> = state
            .all_non_seed_block_ids()
            .into_iter()
            .filter(|id| !state.is_page_block(id))
            .collect();
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

impl<R: RefLifecycle + RefBlockTree + RefLayoutMutate> TransitionRef<R> for InstantiateTemplate {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(
                state.all_non_seed_block_ids().len() > 0,
                Reason::PreconditionFailed,
            ),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        let inst_root_id =
            holon_api::effect_id::deterministic_instance_id(TPL_ROOT, CTX_KEY, TPL_ROOT);
        let inst_child_id =
            holon_api::effect_id::deterministic_instance_id(TPL_ROOT, CTX_KEY, TPL_CHILD);

        state.create_block_under_with_id(&self.parent_id, &self.date, inst_root_id.clone());
        state.create_block_under_with_id(
            &inst_root_id,
            &format!("see {} now", self.date),
            inst_child_id,
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
            CTX_KEY,
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
