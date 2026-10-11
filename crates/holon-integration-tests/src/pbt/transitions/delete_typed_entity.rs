//! Transition: delete a live free-standing typed entity.
//!
//! @pbt rung datatype
//! @pbt covers typed-matview-matches-ref — a deleted entity leaves the matview.
//! @pbt covers typed-loro-matches-ref — in every Loro mode the deleted entity
//!   is no longer live in the Loro store.

use holon_pbt_core::RequiredWiring;
use holon_pbt_core::StorageAdapter;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::SutTypedEntityWrite;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::reference_state::ReferenceState;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;
use crate::pbt::transitions::set_typed_entity_field::live_typed_entities;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("I delete the {type_name} entity {id}")]
pub struct DeleteTypedEntity {
    pub type_name: String,
    pub id: String,
}

impl TransitionFactory<ReferenceState> for DeleteTypedEntity {
    fn required_caps() -> Vec<holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn required_wiring() -> RequiredWiring {
        RequiredWiring::any_storage_of([StorageAdapter::Turso, StorageAdapter::Loro])
    }

    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let live: Vec<(String, String)> = live_typed_entities(state)
            .into_iter()
            .map(|(type_name, id, _)| (type_name, id))
            .collect();
        vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(!live.is_empty(), Reason::PreconditionFailed),
        ]
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(move |_| {
            let strat = proptest::sample::select(live)
                .prop_map(|(type_name, id)| DeleteTypedEntity { type_name, id })
                .boxed();
            (3, strat)
        })
    }
}

impl TransitionRef<ReferenceState> for DeleteTypedEntity {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(
                state.typed_entities.ids(&self.type_name).contains(&self.id),
                Reason::PreconditionFailed,
            ),
        ]
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        state.typed_entities.remove(&self.type_name, &self.id);
    }
}

crate::cap_transition! {
    DeleteTypedEntity: SutTypedEntityWrite,
    where R: [ RefLifecycle ],
    |me, _state, sut| {
        sut.delete_typed_entity(&me.type_name, &me.id).await;
    }
    sql_budget: |_me, _state| {
        ExpectedSql {
            reads: 0,
            writes: 1,
            ddl: 0,
            tolerance: 64,
        }
    }
}
