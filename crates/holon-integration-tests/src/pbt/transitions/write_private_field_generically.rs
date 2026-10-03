//! Transition: write a block's `parent_id` or `sort_key` through a generic
//! write path (`set_field`, `update`, MCP `execute_operation`) instead of
//! through its placement owner.
//!
//! @pbt rung dispatch
//!   `attempt_private_field_write` dispatches at the production operation
//!   dispatcher or the MCP server, below every UI precondition.
//! @pbt covers private-block-fields-refused-on-generic-writes — `parent_id`
//!   and `sort_key` are written only by `move_block` and its family, so no
//!   generic write can store a parent cycle or a sibling-order break
//!
//! The oracle applies it as a NO-OP; the tree comparisons judge the store.

use holon_api::EntityUri;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::GenericWrite;
use holon_pbt_core::capabilities::PrivateFieldName;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefLayout;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::SutPrivateFieldWriteAttempt;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// Write `field = value` on block `id` through `via`.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("I write the private field {field} of block {id} as {value} via {via}")]
pub struct WritePrivateFieldGenerically {
    pub id: EntityUri,
    pub field: PrivateFieldName,
    pub via: GenericWrite,
    pub value: String,
}

fn movable<R: RefBlockTree>(state: &R, id: &EntityUri) -> bool {
    state.is_text_block(id) && !state.is_page_block(id) && !state.is_layout_block(id)
}

/// The block-CRUD authority registers `update` only in SqlOnly mode.
fn dispatchable<R: RefLifecycle>(state: &R, via: GenericWrite) -> bool {
    matches!(via, GenericWrite::SetField | GenericWrite::McpSetField) || !state.enable_loro()
}

const VIAS: [GenericWrite; 4] = [
    GenericWrite::SetField,
    GenericWrite::Update,
    GenericWrite::McpSetField,
    GenericWrite::McpUpdate,
];

impl<R: RefLifecycle + RefBlockTree + RefLayout> TransitionFactory<R>
    for WritePrivateFieldGenerically
{
    fn required_caps() -> Vec<holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let all = state.all_block_ids();
        let ids: Vec<EntityUri> = all
            .iter()
            .filter(|id| movable(state, id))
            .cloned()
            .collect();
        let vias: Vec<GenericWrite> = VIAS
            .into_iter()
            .filter(|via| dispatchable(state, *via))
            .collect();
        let parents_of: Vec<(EntityUri, Vec<EntityUri>)> = ids
            .iter()
            .map(|id| {
                (
                    id.clone(),
                    all.iter().filter(|p| *p != id).cloned().collect(),
                )
            })
            .filter(|(_, parents): &(EntityUri, Vec<EntityUri>)| !parents.is_empty())
            .collect();
        vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(!ids.is_empty(), Reason::PreconditionFailed),
        ]
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(move |_| {
            let via = proptest::sample::select(vias);
            let sort_key_writes = (
                proptest::sample::select(ids),
                via.clone(),
                proptest::string::string_regex("[a-z][0-9A-Za-z]{1,3}").expect("valid regex"),
            )
                .prop_map(|(id, via, value)| WritePrivateFieldGenerically {
                    id,
                    field: PrivateFieldName::SortKey,
                    via,
                    value,
                });
            let strat = if parents_of.is_empty() {
                sort_key_writes.boxed()
            } else {
                let parent_writes = (proptest::sample::select(parents_of), via).prop_flat_map(
                    |((id, parents), via)| {
                        proptest::sample::select(parents).prop_map(move |parent| {
                            WritePrivateFieldGenerically {
                                id: id.clone(),
                                field: PrivateFieldName::ParentId,
                                via,
                                value: parent.to_string(),
                            }
                        })
                    },
                );
                prop_oneof![sort_key_writes, parent_writes].boxed()
            };
            // Low weight: one refused write per run arms the rung, and every
            // draw spent here is a draw not spent on the editing alphabet.
            (2, strat)
        })
    }
}

impl<R: RefLifecycle + RefBlockTree + RefLayout> TransitionRef<R> for WritePrivateFieldGenerically {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(movable(state, &self.id), Reason::PreconditionFailed),
            check(dispatchable(state, self.via), Reason::PreconditionFailed),
            check(
                self.field != PrivateFieldName::ParentId
                    || EntityUri::parse(&self.value)
                        .is_ok_and(|p| p != self.id && state.all_block_ids().contains(&p)),
                Reason::PreconditionFailed,
            ),
        ]
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(|_| ())
    }

    fn apply_to_ref(&self, _: &mut R) {
        // A refused write changes nothing.
    }
}

crate::cap_transition! {
    WritePrivateFieldGenerically: SutPrivateFieldWriteAttempt,
    where R: [ RefLifecycle + RefBlockTree + RefLayout ],
    |me, state, sut| {
        sut.attempt_private_field_write(&me.id, me.field, me.via, &me.value)
            .await;
    }
    sql_budget: |_me, _state| {
        // The intent boundary refuses before any provider runs: no write.
        ExpectedSql {
            reads: 0,
            writes: 0,
            ddl: 0,
            tolerance: 16,
        }
    }
}
