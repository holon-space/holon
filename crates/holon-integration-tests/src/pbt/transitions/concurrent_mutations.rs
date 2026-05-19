//! Transition: concurrent UI + external mutations (post-startup).
//!
//! Mirrors the legacy logic split across `state_machine.rs:1322-1415` (generator),
//! `state_machine.rs:3296-3313` (precondition),
//! `state_machine.rs:2552-2646` (ref-state apply),
//! `sut.rs:3467-3482` (SUT apply dispatch), and
//! `transition_budgets.rs:261-275` (expected SQL).
//!
//! NOTE: The generator is permanently disabled (`if false &&`) in the legacy
//! code because reference model assumes External-always-wins (LWW), but actual
//! CRDT resolution is timing-dependent. The transition struct is preserved for
//! completeness and potential future re-enablement.

use crate::pbt::validation::{Reason, check};
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::SutHandle;
use holon_pbt_core::{TransitionFactory, TransitionImpl, TransitionRef};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, expected_mutation_sql};

use holon_api::{ContentType, EntityUri};

use crate::assign_reference_sequences_canonical;
use crate::pbt::state_machine::loro_merge_text;
use crate::pbt::types::{Mutation, MutationEvent};

use holon_api::block::Block;

/// Concurrent UI + external mutations on (possibly overlapping) blocks.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ConcurrentMutations {
    pub ui_mutation: MutationEvent,
    pub external_mutation: MutationEvent,
}

impl TransitionFactory<ReferenceState> for ConcurrentMutations {
    type Reason = Reason;
    fn weighted_generator(_: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // DISABLED: reference model assumes External always wins (LWW),
        // but actual CRDT resolution is timing-dependent. The legacy generator
        // is wrapped in `if false && ...` so no strategy is ever built. The
        // single `Fail(ConcurrentMutationsDisabled)` makes the disabled state
        // explicit in the rejection histogram.
        Validated::fail(Reason::ConcurrentMutationsDisabled)
    }
}

fn mutation_precondition(event: &MutationEvent, state: &ReferenceState) -> bool {
    if !state.app_started {
        return false;
    }
    match &event.mutation {
        Mutation::Delete { id, .. } => {
            state.block_state.blocks.contains_key(id) && !state.layout_blocks.contains(id)
        }
        Mutation::Update { id, .. } => {
            state.block_state.blocks.contains_key(id) && !state.layout_blocks.is_immutable(id)
        }
        Mutation::Move {
            id, new_parent_id, ..
        } => {
            state.block_state.blocks.contains_key(id)
                && state
                    .block_state
                    .blocks
                    .get(id)
                    .is_some_and(|b| b.content_type != ContentType::Source)
                && state
                    .block_state
                    .blocks
                    .get(new_parent_id)
                    .map_or(state.documents.contains_key(new_parent_id), |b| {
                        b.content_type != ContentType::Source
                    })
        }
        Mutation::Create { parent_id, .. } => {
            state.documents.contains_key(parent_id)
                || state
                    .block_state
                    .blocks
                    .get(parent_id)
                    .is_some_and(|b| b.content_type != ContentType::Source)
        }
        Mutation::RestartApp => true,
    }
}

impl TransitionRef<ReferenceState> for ConcurrentMutations {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let mut checks: Vec<Validated<(), Reason>> =
            vec![check(state.app_started, Reason::AppNotStarted)];

        let ui_ok = mutation_precondition(&self.ui_mutation, state);
        let ext_ok = mutation_precondition(&self.external_mutation, state);
        let same_create_id = matches!(
            (&self.ui_mutation.mutation, &self.external_mutation.mutation),
            (Mutation::Create { id: id1, .. }, Mutation::Create { id: id2, .. }) if id1 == id2
        );

        checks.push(check(ui_ok, Reason::PreconditionFailed));
        checks.push(check(ext_ok, Reason::PreconditionFailed));
        checks.push(check(!same_create_id, Reason::PreconditionFailed));

        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        let same_block_update = match (&self.ui_mutation.mutation, &self.external_mutation.mutation)
        {
            (
                Mutation::Update {
                    id: ui_id,
                    fields: ui_fields,
                    ..
                },
                Mutation::Update {
                    id: ext_id,
                    fields: ext_fields,
                    ..
                },
            ) if ui_id == ext_id => {
                let ui_content = ui_fields.get("content").and_then(|v| v.as_string());
                let ext_content = ext_fields.get("content").and_then(|v| v.as_string());
                match (ui_content, ext_content) {
                    (Some(ui_c), Some(ext_c)) => {
                        Some((ui_id.clone(), ui_c.to_string(), ext_c.to_string()))
                    }
                    _ => None,
                }
            }
            _ => None,
        };

        if let Some((block_id, ui_content, ext_content)) = same_block_update {
            if state.variant.enable_loro {
                let original = state
                    .block_state
                    .blocks
                    .get(&block_id)
                    .map(|b| b.content.as_str())
                    .unwrap_or("");
                let merged = loro_merge_text(original, &ui_content, &ext_content);
                if let Some(block) = state.block_state.blocks.get_mut(&block_id) {
                    block.content = merged;
                }
            } else {
                if let Some(block) = state.block_state.blocks.get_mut(&block_id) {
                    block.content = ext_content;
                }
            }
        } else {
            for event in [&self.ui_mutation, &self.external_mutation] {
                if let Mutation::Create { id, parent_id, .. } = &event.mutation {
                    let doc_uri = if parent_id.is_no_parent() || parent_id.is_sentinel() {
                        parent_id.clone()
                    } else {
                        fn find_doc(
                            block_id: &EntityUri,
                            state: &ReferenceState,
                        ) -> Option<EntityUri> {
                            let block = state.block_state.blocks.get(block_id)?;
                            if block.parent_id.is_no_parent() || block.parent_id.is_sentinel() {
                                Some(block.parent_id.clone())
                            } else {
                                find_doc(&block.parent_id, state)
                            }
                        }
                        find_doc(parent_id, state).unwrap_or_else(|| parent_id.clone())
                    };
                    state
                        .block_state
                        .block_documents
                        .insert(id.clone(), doc_uri);
                }

                let mut blocks: Vec<Block> = state.block_state.blocks.values().cloned().collect();
                event.mutation.apply_to(&mut blocks);
                state.block_state.blocks = blocks.into_iter().map(|b| (b.id.clone(), b)).collect();
            }
        }
        let mut all_blocks: Vec<Block> = state.block_state.blocks.values().cloned().collect();
        assign_reference_sequences_canonical(&mut all_blocks);
        state.block_state.blocks = all_blocks.into_iter().map(|b| (b.id.clone(), b)).collect();
        state.rebuild_profile_tracking();
        state.block_state.next_id += 2;
    }
}

#[allow(async_fn_in_trait)]
impl<S: SutHandle> TransitionImpl<ReferenceState, S> for ConcurrentMutations {
    async fn apply_to_sut(&self, ref_state: &ReferenceState, sut: &mut S) {
        sut.apply_concurrent_mutations(
            self.ui_mutation.clone(),
            self.external_mutation.clone(),
            ref_state,
        )
        .await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for ConcurrentMutations {
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        let watches = state.active_watches.len();
        let blocks = state.block_state.blocks.len();
        let docs = state.documents.len();
        let a = expected_mutation_sql(&self.ui_mutation.mutation, watches, blocks, docs);
        let b = expected_mutation_sql(&self.external_mutation.mutation, watches, blocks, docs);
        ExpectedSql {
            reads: a.reads + b.reads,
            writes: a.writes + b.writes,
            ddl: a.ddl + b.ddl,
            tolerance: a.tolerance + b.tolerance,
        }
    }
}
