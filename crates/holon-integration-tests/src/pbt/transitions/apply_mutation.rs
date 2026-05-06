//! Transition: apply a single mutation (post-startup).
//!
//! Mirrors the legacy logic split across `state_machine.rs:469-823` (generator),
//! `state_machine.rs:3118-3159` (precondition),
//! `state_machine.rs:2148-2202` (ref-state apply),
//! `sut.rs:2177-2180` (SUT apply dispatch), and
//! `transition_budgets.rs:230-231` (expected SQL).

use crate::pbt::validation::{Reason, check};
use proptest::prelude::*;
use proptest::strategy::{BoxedStrategy, Union};
use validated::Validated;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, expected_mutation_sql};

use holon_api::block::Block;
use holon_api::{ContentType, EntityUri};

use crate::assign_reference_sequences_canonical;
use crate::pbt::generators::{
    generate_layout_headline_mutation, generate_mutation, generate_profile_content_mutation,
    generate_render_source_mutation,
};
use crate::pbt::state_machine::LAYOUT_MUTATIONS_ENABLED;
use crate::pbt::types::{Mutation, MutationEvent, MutationSource};

/// Apply a single mutation (UI or external).
#[derive(Clone, Debug)]
pub struct ApplyMutation {
    pub event: MutationEvent,
}

impl E2ETransitionFactory for ApplyMutation {
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let checks: Vec<Validated<(), Reason>> =
            vec![check(state.app_started, Reason::AppNotStarted)];

        let merged: Validated<Vec<()>, Reason> = checks.into_iter().collect();
        match merged {
            Validated::Fail(reasons) => return Validated::Fail(reasons),
            Validated::Good(_) => {}
        }
        (|| {
            let peer_modified: std::collections::HashSet<String> = state
                .peers
                .iter()
                .flat_map(|p| p.modified_stable_ids.iter().cloned())
                .collect();
            let is_peer_modified = |id: &EntityUri| peer_modified.contains(id.id());
            let default_doc = EntityUri::no_parent();
            let block_ids: Vec<EntityUri> = state
                .block_state
                .blocks
                .iter()
                .filter(|(_, b)| {
                    !b.is_page()
                        && !is_peer_modified(&b.id)
                        && state
                            .block_state
                            .block_documents
                            .get(&b.id)
                            .is_none_or(|doc| *doc != default_doc)
                })
                .map(|(id, _)| id.clone())
                .collect();
            let text_block_ids: Vec<EntityUri> = state
                .block_state
                .blocks
                .iter()
                .filter(|(_, b)| {
                    b.content_type == ContentType::Text
                        && !b.is_page()
                        && !is_peer_modified(&b.id)
                        && state
                            .block_state
                            .block_documents
                            .get(&b.id)
                            .is_none_or(|doc| *doc != default_doc)
                })
                .map(|(id, _)| id.clone())
                .collect();
            let doc_uris: Vec<EntityUri> = state.documents.keys().cloned().collect();
            let next_id = state.block_state.next_id;

            let no_content_update: std::collections::HashSet<EntityUri> = state
                .layout_blocks
                .render_source_ids
                .iter()
                .chain(state.layout_blocks.query_source_ids.iter())
                .chain(state.profile_block_ids.iter())
                .cloned()
                .collect();

            let mut arms: Vec<(u32, BoxedStrategy<ApplyMutation>)> = Vec::new();

            if !doc_uris.is_empty() {
                // ui_mutation: weight 0 by default; opt-in with PBT_WEIGHT_UI_MUTATION=N
                let ui_weight: u32 = std::env::var("PBT_WEIGHT_UI_MUTATION")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                if ui_weight > 0 {
                    arms.push((
                        ui_weight,
                        generate_mutation(
                            next_id,
                            block_ids.clone(),
                            text_block_ids.clone(),
                            doc_uris.clone(),
                            no_content_update.clone(),
                        )
                        .prop_map(|mutation| ApplyMutation {
                            event: MutationEvent {
                                source: MutationSource::UI,
                                mutation,
                            },
                        })
                        .boxed(),
                    ));
                }

                arms.push((
                    1,
                    generate_mutation(
                        next_id,
                        block_ids.clone(),
                        text_block_ids.clone(),
                        doc_uris.clone(),
                        no_content_update.clone(),
                    )
                    .prop_map(|mutation| ApplyMutation {
                        event: MutationEvent {
                            source: MutationSource::External,
                            mutation,
                        },
                    })
                    .boxed(),
                ));
            }

            if LAYOUT_MUTATIONS_ENABLED {
                let seed_layout_block_ids: std::collections::HashSet<&str> = [
                    "block:default-main-panel",
                    "block:default-left-sidebar",
                    "block:default-right-sidebar",
                ]
                .into_iter()
                .collect();
                let headline_ids: Vec<EntityUri> = state
                    .layout_blocks
                    .headline_ids
                    .iter()
                    .filter(|id| !is_peer_modified(id))
                    .filter(|id| !seed_layout_block_ids.contains(id.as_str()))
                    .cloned()
                    .collect();
                if !headline_ids.is_empty() {
                    arms.push((
                        1,
                        generate_layout_headline_mutation(headline_ids, state.keyword_set.clone())
                            .prop_map(|mutation| ApplyMutation {
                                event: MutationEvent {
                                    source: MutationSource::UI,
                                    mutation,
                                },
                            })
                            .boxed(),
                    ));
                }

                let seed_render_source_ids: std::collections::HashSet<&str> = [
                    "block:holon-app-layout::render::0",
                    "block:holon-app-layout::src::0",
                    "block:root-layout::src::0",
                    "block:block:left_sidebar::render::0",
                    "block:block:left_sidebar::src::0",
                    "block:block:right_sidebar::render::0",
                    "block:block:right_sidebar::src::0",
                    "block:block:main_panel::render::0",
                    "block:block:main_panel::src::0",
                    "block:default-left-sidebar::render::0",
                    "block:default-left-sidebar::src::0",
                    "block:default-right-sidebar::render::0",
                    "block:default-right-sidebar::src::0",
                    "block:default-main-panel::render::0",
                    "block:default-main-panel::src::0",
                ]
                .into_iter()
                .collect();
                let render_ids: Vec<EntityUri> = state
                    .layout_blocks
                    .render_source_ids
                    .iter()
                    .filter(|id| !seed_render_source_ids.contains(id.as_str()))
                    .cloned()
                    .collect();
                if !render_ids.is_empty() {
                    arms.push((
                        1,
                        generate_render_source_mutation(render_ids)
                            .prop_map(|mutation| ApplyMutation {
                                event: MutationEvent {
                                    source: MutationSource::UI,
                                    mutation,
                                },
                            })
                            .boxed(),
                    ));
                }
            }

            let profile_ids: Vec<EntityUri> = state.profile_block_ids.iter().cloned().collect();
            if !profile_ids.is_empty() {
                arms.push((
                    1,
                    generate_profile_content_mutation(profile_ids)
                        .prop_map(|mutation| ApplyMutation {
                            event: MutationEvent {
                                source: MutationSource::UI,
                                mutation,
                            },
                        })
                        .boxed(),
                ));
            }

            if arms.is_empty() {
                // app_started is true but no documents / blocks → nothing to mutate.
                // Surface in the histogram rather than panicking.
                return Validated::fail(Reason::NoDocumentsAvailable);
            }

            let strat = Union::new_weighted(arms).boxed();
            Validated::Good((1, strat))
        })()
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for ApplyMutation {
    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let mut checks: Vec<Validated<(), Reason>> =
            vec![check(state.app_started, Reason::AppNotStarted)];

        // Mutation-type-specific gates
        match &self.event.mutation {
            Mutation::Delete { id, .. } => {
                checks.push(check(
                    state.block_state.blocks.contains_key(id),
                    Reason::PreconditionFailed,
                ));
                checks.push(check(
                    !state.layout_blocks.contains(id),
                    Reason::FocusedInLayoutBlocks,
                ));
            }
            Mutation::Update { id, .. } => {
                checks.push(check(
                    state.block_state.blocks.contains_key(id),
                    Reason::PreconditionFailed,
                ));
                checks.push(check(
                    !state.layout_blocks.is_immutable(id),
                    Reason::FocusedInLayoutBlocks,
                ));
            }
            Mutation::Move {
                id, new_parent_id, ..
            } => {
                checks.push(check(
                    state.block_state.blocks.contains_key(id),
                    Reason::PreconditionFailed,
                ));
                checks.push(check(
                    state
                        .block_state
                        .blocks
                        .get(id)
                        .is_some_and(|b| b.content_type != ContentType::Source),
                    Reason::PreconditionFailed,
                ));
                checks.push(check(
                    state
                        .block_state
                        .blocks
                        .get(new_parent_id)
                        .map_or(state.documents.contains_key(new_parent_id), |b| {
                            b.content_type != ContentType::Source
                        }),
                    Reason::PreconditionFailed,
                ));
            }
            Mutation::Create { parent_id, .. } => {
                checks.push(check(
                    state.documents.contains_key(parent_id)
                        || state
                            .block_state
                            .blocks
                            .get(parent_id)
                            .is_some_and(|b| b.content_type != ContentType::Source),
                    Reason::PreconditionFailed,
                ));
            }
            Mutation::RestartApp => {
                // No additional checks
            }
        }

        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        if self.event.source == MutationSource::UI {
            state.push_undo_snapshot();
        }
        if let Mutation::Create { id, parent_id, .. } = &self.event.mutation {
            let doc_uri = if parent_id.is_no_parent() || parent_id.is_sentinel() {
                parent_id.clone()
            } else {
                state
                    .block_state
                    .block_documents
                    .get(parent_id)
                    .cloned()
                    .unwrap_or_else(|| parent_id.clone())
            };
            state
                .block_state
                .block_documents
                .insert(id.clone(), doc_uri);
        }

        let mut blocks: Vec<Block> = state.block_state.blocks.values().cloned().collect();
        self.event.mutation.apply_to(&mut blocks);
        assign_reference_sequences_canonical(&mut blocks);
        state.block_state.blocks = blocks.into_iter().map(|b| (b.id.clone(), b)).collect();
        state.rebuild_profile_tracking();

        if let Mutation::Update { id, fields, .. } = &self.event.mutation
            && state.layout_blocks.render_source_ids.contains(id)
            && fields.contains_key("content")
            && let Some(block) = state.block_state.blocks.get(id)
            && let Some(expr) =
                super::super::reference_state::render_expr_from_rhai(block.content.as_str())
        {
            state.render_expressions.insert(id.clone(), expr);
        }

        state.block_state.next_id += 1;

        match &self.event.mutation {
            Mutation::Update { id, fields, .. } if fields.contains_key("content") => {
                state.reset_cursor_if_focused(id);
            }
            Mutation::Delete { id, .. } => {
                state.clear_focus_if_deleted(id);
            }
            _ => {}
        }
    }

    async fn apply_to_sut(&self, ref_state: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_apply_mutation(self.event.clone(), ref_state)
            .await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        let watches = state.active_watches.len();
        let blocks = state.block_state.blocks.len();
        let docs = state.documents.len();
        expected_mutation_sql(&self.event.mutation, watches, blocks, docs)
    }
}
