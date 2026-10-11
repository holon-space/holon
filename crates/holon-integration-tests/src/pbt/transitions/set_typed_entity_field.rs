//! Transition: overwrite one persisted field of a live free-standing typed
//! entity — the datatype axis's update half.
//!
//! @pbt rung datatype
//! @pbt covers typed-matview-matches-ref — a `set_field` lands in the matview.
//! @pbt covers typed-loro-matches-ref — in every Loro mode the field lands in
//!   the entity's Loro child map.
//!
//! The type, the entity and the column are drawn from the oracle's live
//! entities, so nothing here names a type.

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

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("I set {field} of the {type_name} entity {id} to {value}")]
pub struct SetTypedEntityField {
    pub type_name: String,
    pub id: String,
    pub field: String,
    pub value: String,
}

/// Every `(type, live id, value columns)` the oracle holds.
pub(crate) fn live_typed_entities(state: &ReferenceState) -> Vec<(String, String, Vec<String>)> {
    state
        .typed_entities
        .declared()
        .flat_map(|(type_name, columns)| {
            state
                .typed_entities
                .ids(type_name)
                .into_iter()
                .map(move |id| (type_name.clone(), id, columns.clone()))
        })
        .collect()
}

impl TransitionFactory<ReferenceState> for SetTypedEntityField {
    fn required_caps() -> Vec<holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn required_wiring() -> RequiredWiring {
        RequiredWiring::any_storage_of([StorageAdapter::Turso, StorageAdapter::Loro])
    }

    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let live = live_typed_entities(state);
        vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(!live.is_empty(), Reason::PreconditionFailed),
        ]
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(move |_| {
            let strat = proptest::sample::select(live)
                .prop_flat_map(|(type_name, id, columns)| {
                    (
                        proptest::sample::select(columns),
                        proptest::string::string_regex("[a-z]{1,6}").expect("valid regex"),
                    )
                        .prop_map(move |(field, value)| SetTypedEntityField {
                            type_name: type_name.clone(),
                            id: id.clone(),
                            field,
                            value,
                        })
                })
                .boxed();
            (6, strat)
        })
    }
}

impl TransitionRef<ReferenceState> for SetTypedEntityField {
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
        state
            .typed_entities
            .set_field(&self.type_name, &self.id, &self.field, self.value.clone());
    }
}

crate::cap_transition! {
    SetTypedEntityField: SutTypedEntityWrite,
    where R: [ RefLifecycle ],
    |me, _state, sut| {
        sut.set_typed_entity_field(&me.type_name, &me.id, &me.field, &me.value).await;
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
