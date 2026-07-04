//! Transition: apply a single mutation (post-startup).
//!
//! Mirrors the legacy logic split across `state_machine.rs:469-823`
//! (generator), `state_machine.rs:3118-3159` (precondition),
//! `state_machine.rs:2148-2202` (ref-state apply),
//! `sut.rs:2177-2180` (SUT apply dispatch), and
//! `transition_budgets.rs:230-231` (expected SQL).

use std::collections::HashMap;

use holon_api::ContentType;
use holon_api::EntityUri;
use holon_api::Value;
use holon_api::block::Block;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionImpl;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::SutLoro;
use holon_pbt_core::types::Mutation;
use holon_pbt_core::types::MutationEvent;
use holon_pbt_core::types::MutationSource;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use proptest::strategy::Union;
use validated::Validated;

use crate::assign_reference_sequences_canonical;
use crate::pbt::generators::generate_layout_headline_mutation;
use crate::pbt::generators::generate_mutation;
use crate::pbt::generators::generate_profile_content_mutation;
use crate::pbt::generators::generate_render_source_mutation;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::state_machine::LAYOUT_MUTATIONS_ENABLED;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::expected_mutation_sql;
use crate::pbt::types::MutationApply;

/// Apply a single mutation (UI or external).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ApplyMutation {
    pub event: MutationEvent,
}

impl TransitionFactory<ReferenceState> for ApplyMutation {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        // Routed by `SutApplyMutation` (source as a shrinkable axis). The gate names
        // `SutLoro` so the composed alphabet admits `ApplyMutation` exactly when the
        // peer mesh is wired (the implemented arm); the generator further restricts the
        // composed source set to the implemented arms via `state.cap_set.is_some()`.
        vec![::holon_pbt_core::composition::CapId::of::<
            dyn ::holon_pbt_core::capabilities::SutLoro,
        >()]
    }

    type Reason = Reason;
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let checks: Vec<Validated<(), Reason>> =
            vec![check(state.action.app_started, Reason::AppNotStarted)];

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
                .domain
                .block_state
                .blocks
                .iter()
                .filter(|(_, b)| {
                    !b.is_page()
                        && !is_peer_modified(&b.id)
                        && state
                            .domain
                            .block_state
                            .block_documents
                            .get(&b.id)
                            .is_none_or(|doc| *doc != default_doc)
                })
                .map(|(id, _)| id.clone())
                .collect();
            let text_block_ids: Vec<EntityUri> = state
                .domain
                .block_state
                .blocks
                .iter()
                .filter(|(_, b)| {
                    b.content_type == ContentType::Text
                        && !b.is_page()
                        && !is_peer_modified(&b.id)
                        && state
                            .domain
                            .block_state
                            .block_documents
                            .get(&b.id)
                            .is_none_or(|doc| *doc != default_doc)
                })
                .map(|(id, _)| id.clone())
                .collect();
            // Extended-gen axis 3 (same-block concurrency): text blocks the
            // primary may edit even though a peer has a pending edit on them.
            // The ref merge already covers the both-sides-diverged case
            // (`merge_peer_blocks_into_primary` consumes the shadow mesh's
            // real CRDT merge), so the conflicting fraction is admissible —
            // it just was never generated.
            // Content-only: Delete/Move on peer-modified blocks stay excluded
            // (no ref merge model for structural conflicts yet).
            let conflict_text_block_ids: Vec<EntityUri> = state
                .domain
                .block_state
                .blocks
                .iter()
                .filter(|(_, b)| {
                    b.content_type == ContentType::Text
                        && !b.is_page()
                        && is_peer_modified(&b.id)
                        && state
                            .domain
                            .block_state
                            .block_documents
                            .get(&b.id)
                            .is_none_or(|doc| *doc != default_doc)
                })
                .map(|(id, _)| id.clone())
                .collect();
            let doc_uris: Vec<EntityUri> = state.files.documents.keys().cloned().collect();
            let next_id = state.domain.block_state.next_id;

            let no_content_update: std::collections::HashSet<EntityUri> = state
                .domain
                .layout_blocks
                .render_source_ids
                .iter()
                .chain(state.domain.layout_blocks.query_source_ids.iter())
                .chain(state.domain.profile_block_ids.iter())
                .cloned()
                .collect();

            let mut arms: Vec<(u32, BoxedStrategy<ApplyMutation>)> = Vec::new();

            // Source-routing: on the composed path, draw a source's arm only when its
            // CapMap arm is implemented. `composed` (the ref carries a `cap_set`) gates the
            // not-yet-composed UI/layout/profile arms to native; the External arm is gated
            // instead on `SutSeamMutate` presence (native always; composed iff a frontend),
            // and the LoroPeer arm self-gates on `enable_loro`+peers below.
            let composed = state.cap_set.is_some();
            let seam_present = state.caps_available(&[::holon_pbt_core::composition::CapId::of::<
                dyn holon_pbt_core::capabilities::SutSeamMutate,
            >()]);

            if !doc_uris.is_empty() {
                // ui_mutation: weight 0 by default; opt-in with PBT_WEIGHT_UI_MUTATION=N
                let ui_weight: u32 = std::env::var("PBT_WEIGHT_UI_MUTATION")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                if !composed && ui_weight > 0 {
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

                if seam_present {
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
            }

            // Axis 3 arm (extended gen): primary content edit on a block a
            // peer has concurrently modified. On the next merge the oracle's
            // shadow mesh predicts the CRDT outcome (clock-padded real Loro
            // merge); the SUT runs the production merge — divergence here
            // means the shadow mis-mirrors production op ids.
            {
                let conflict_ids: Vec<EntityUri> = conflict_text_block_ids
                    .iter()
                    .filter(|id| !no_content_update.contains(id))
                    .cloned()
                    .collect();
                if !composed
                    && crate::pbt::generators::extended_gen_enabled()
                    && !conflict_ids.is_empty()
                {
                    let conflict = (proptest::sample::select(conflict_ids), "[a-z]{4,8}")
                        .prop_map(|(id, content)| {
                            eprintln!(
                                "[axis3] conflict arm: primary External content edit on \
                                 peer-modified block {id}"
                            );
                            (id, content)
                        })
                        .prop_map(|(id, content)| ApplyMutation {
                            event: MutationEvent {
                                source: MutationSource::External,
                                mutation: Mutation::Update {
                                    id,
                                    fields: [("content".to_string(), Value::String(content))]
                                        .into_iter()
                                        .collect(),
                                },
                            },
                        })
                        .boxed();
                    // Weight 4: the eligible window (peer edit pending, merge
                    // not yet drawn) is rare, so when it IS open the conflict
                    // arm should win the union often enough to get sampled.
                    arms.push((4, conflict));
                }
            }

            if !composed && LAYOUT_MUTATIONS_ENABLED {
                let seed_layout_block_ids: std::collections::HashSet<&str> = [
                    "block:default-main-panel",
                    "block:default-left-sidebar",
                    "block:default-right-sidebar",
                ]
                .into_iter()
                .collect();
                let headline_ids: Vec<EntityUri> = state
                    .domain
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
                        generate_layout_headline_mutation(headline_ids, None)
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
                    // The seed layout uses INCONSISTENT id schemes (see
                    // assets/default/index.org): the LEFT sidebar's render/src
                    // carry bare `:id left_sidebar::…` ids, while the right
                    // sidebar / main panel use `default-…::…`. The earlier list
                    // had `block:block:left_sidebar::…` (a double-`block:` typo)
                    // and `block:default-left-sidebar::…` (wrong scheme) — neither
                    // matched the real `block:left_sidebar::render::0`, so render
                    // mutations clobbered the sidebar's `navigation_focus` render
                    // with the focus_chain fixture and broke NavigateFocus.
                    "block:left_sidebar::render::0",
                    "block:left_sidebar::src::0",
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
                    .domain
                    .layout_blocks
                    .render_source_ids
                    .iter()
                    .filter(|id| !seed_render_source_ids.contains(id.as_str()))
                    // Defense-in-depth against id drift: never mutate a render
                    // that provides a `navigation_focus` affordance. The ref
                    // model's `predicts_navigation_focus` assumes sidebar pages
                    // stay clickable, so rewriting such a render (e.g. to the
                    // focus_chain fixture) would desync ref vs SUT navigability.
                    .filter(|id| {
                        !state
                            .domain
                            .block_state
                            .blocks
                            .get(*id)
                            .is_some_and(|b| b.content.contains("navigation_focus"))
                    })
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

            let profile_ids: Vec<EntityUri> =
                state.domain.profile_block_ids.iter().cloned().collect();
            if !composed && !profile_ids.is_empty() {
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

            // LoroPeer channel: apply a generic mutation through a Loro CRDT
            // peer. Only valid when Loro is wired (`enable_loro`) AND at least
            // one peer exists (from prior `AddPeer`). Peer blocks are keyed by
            // stable id; the generated mutation's bare ids ARE those stable ids.
            // Mirrors `PeerEdit`'s generator: Create (deterministic stable id so
            // ref + SUT agree) and Update (content-only); Delete is skipped
            // (cascading-delete ref-model gap, same as PeerEdit).
            if state.enable_loro() && !state.peers.is_empty() {
                let peer_count = state.peers.len();
                let seq = state.domain.block_state.next_id;
                let peer_blocks_per_idx: Vec<Vec<String>> = (0..peer_count)
                    .map(|idx| state.peers[idx].blocks.keys().cloned().collect::<Vec<_>>())
                    .collect();

                let create_blocks = peer_blocks_per_idx.clone();
                let create = (0..peer_count, "[a-z]{4,8}")
                    .prop_flat_map(move |(peer_idx, content)| {
                        let has_blocks = !create_blocks[peer_idx].is_empty();
                        let parent_strat = if has_blocks {
                            proptest::option::of(proptest::sample::select(
                                create_blocks[peer_idx].clone(),
                            ))
                            .boxed()
                        } else {
                            Just(None).boxed()
                        };
                        parent_strat.prop_map(move |parent_stable_id: Option<String>| {
                            let sid = crate::pbt::transitions::deterministic_peer_block_id(
                                peer_idx,
                                parent_stable_id.as_deref(),
                                &content,
                                seq,
                            );
                            let parent_uri = parent_stable_id
                                .as_deref()
                                .map(EntityUri::block)
                                .unwrap_or_else(EntityUri::no_parent);
                            let mut fields = HashMap::new();
                            fields.insert("content".to_string(), Value::String(content.clone()));
                            ApplyMutation {
                                event: MutationEvent {
                                    source: MutationSource::LoroPeer { peer_idx },
                                    mutation: Mutation::Create {
                                        id: EntityUri::block(&sid),
                                        parent_id: parent_uri,
                                        fields,
                                    },
                                },
                            }
                        })
                    })
                    .boxed();
                arms.push((1, create));

                let updatable: Vec<(usize, Vec<String>)> = (0..peer_count)
                    .map(|idx| (idx, peer_blocks_per_idx[idx].clone()))
                    .filter(|(_, ids)| !ids.is_empty())
                    .collect();
                if !updatable.is_empty() {
                    let update = proptest::sample::select(updatable)
                        .prop_flat_map(|(peer_idx, ids)| {
                            (Just(peer_idx), proptest::sample::select(ids), "[a-z]{4,8}")
                        })
                        .prop_map(|(peer_idx, stable_id, content)| {
                            let mut fields = HashMap::new();
                            fields.insert("content".to_string(), Value::String(content));
                            ApplyMutation {
                                event: MutationEvent {
                                    source: MutationSource::LoroPeer { peer_idx },
                                    mutation: Mutation::Update {
                                        id: EntityUri::block(&stable_id),
                                        fields,
                                    },
                                },
                            }
                        })
                        .boxed();
                    arms.push((1, update));
                }
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

impl TransitionRef<ReferenceState> for ApplyMutation {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let mut checks: Vec<Validated<(), Reason>> =
            vec![check(state.action.app_started, Reason::AppNotStarted)];

        // LoroPeer mutations are validated against the targeted peer's state,
        // not the primary block map.
        if let MutationSource::LoroPeer { peer_idx } = self.event.source {
            checks.push(check(state.enable_loro(), Reason::LoroRequiredForPeers));
            checks.push(check(
                peer_idx < state.peers.len(),
                Reason::PeerIndexOutOfBounds,
            ));
            if peer_idx < state.peers.len() {
                let peer = &state.peers[peer_idx];
                let valid = match &self.event.mutation {
                    Mutation::Create { parent_id, .. } => {
                        parent_id.is_no_parent()
                            || parent_id.is_sentinel()
                            || peer.blocks.contains_key(parent_id.id())
                    }
                    Mutation::Update { id, fields, .. } => {
                        peer.blocks.contains_key(id.id()) && fields.contains_key("content")
                    }
                    Mutation::Delete { id, .. } => peer.blocks.contains_key(id.id()),
                    Mutation::Move { .. } | Mutation::RestartApp => false,
                };
                checks.push(check(valid, Reason::PeerEditSourceBlockViolation));
            }
            return checks
                .into_iter()
                .collect::<Validated<Vec<()>, _>>()
                .map(|_| ());
        }

        // Mutation-type-specific gates
        match &self.event.mutation {
            Mutation::Delete { id, .. } => {
                checks.push(check(
                    state.domain.block_state.blocks.contains_key(id),
                    Reason::PreconditionFailed,
                ));
                checks.push(check(
                    !state.domain.layout_blocks.contains(id),
                    Reason::FocusedInLayoutBlocks,
                ));
            }
            Mutation::Update { id, .. } => {
                checks.push(check(
                    state.domain.block_state.blocks.contains_key(id),
                    Reason::PreconditionFailed,
                ));
                checks.push(check(
                    !state.domain.layout_blocks.is_immutable(id),
                    Reason::FocusedInLayoutBlocks,
                ));
            }
            Mutation::Move {
                id, new_parent_id, ..
            } => {
                checks.push(check(
                    state.domain.block_state.blocks.contains_key(id),
                    Reason::PreconditionFailed,
                ));
                checks.push(check(
                    state
                        .domain
                        .block_state
                        .blocks
                        .get(id)
                        .is_some_and(|b| b.content_type != ContentType::Source),
                    Reason::PreconditionFailed,
                ));
                checks.push(check(
                    state
                        .domain
                        .block_state
                        .blocks
                        .get(new_parent_id)
                        .map_or(state.files.documents.contains_key(new_parent_id), |b| {
                            b.content_type != ContentType::Source
                        }),
                    Reason::PreconditionFailed,
                ));
            }
            Mutation::Create { parent_id, .. } => {
                checks.push(check(
                    state.files.documents.contains_key(parent_id)
                        || state
                            .domain
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
        // LoroPeer mutations apply to the peer's reference state only — they do
        // NOT touch the primary block map until a `SyncWithPeer`/`MergeFromPeer`
        // converges them. Mirrors `PeerEdit::apply_to_ref` (peer_apply_*),
        // keyed off the mutation's bare ids.
        if let MutationSource::LoroPeer { peer_idx } = self.event.source {
            use holon_pbt_core::capabilities::RefPeersMut;
            match &self.event.mutation {
                Mutation::Create {
                    id,
                    parent_id,
                    fields,
                    ..
                } => {
                    let parent_stable = if parent_id.is_no_parent() || parent_id.is_sentinel() {
                        None
                    } else {
                        Some(parent_id.id().to_string())
                    };
                    let content = fields
                        .get("content")
                        .and_then(|v| v.as_string())
                        .unwrap_or_default();
                    state.peer_apply_create(peer_idx, parent_stable.as_deref(), content, id.id());
                }
                Mutation::Update { id, fields, .. } => {
                    let content = fields
                        .get("content")
                        .and_then(|v| v.as_string())
                        .expect("LoroPeer Update must carry a `content` field");
                    state.peer_apply_update(peer_idx, id.id(), content);
                }
                Mutation::Delete { id, .. } => {
                    state.peer_apply_delete(peer_idx, id.id());
                }
                Mutation::Move { .. } | Mutation::RestartApp => {
                    panic!("LoroPeer mutation has no ref mapping for Move/RestartApp")
                }
            }
            return;
        }

        if self.event.source == MutationSource::UI {
            state.push_undo_snapshot();
        }
        if let Mutation::Create { id, parent_id, .. } = &self.event.mutation {
            let doc_uri = if parent_id.is_no_parent() || parent_id.is_sentinel() {
                parent_id.clone()
            } else {
                // The new block belongs to its parent's document. But when the
                // parent is itself a top-level page (its own `block_documents`
                // entry is `no_parent`/`sentinel`), the page IS the document —
                // the child lives in the page's org file, not in the page's
                // (sentinel) document. Inheriting the sentinel would misclassify
                // the child as a seed block and drop it from the `/org` view.
                match state.domain.block_state.block_documents.get(parent_id) {
                    Some(doc) if !doc.is_no_parent() && !doc.is_sentinel() => doc.clone(),
                    _ => parent_id.clone(),
                }
            };
            state
                .domain
                .block_state
                .block_documents
                .insert(id.clone(), doc_uri);
        }

        let mut blocks: Vec<Block> = state.domain.block_state.blocks.values().cloned().collect();
        self.event.mutation.apply_to(&mut blocks);
        assign_reference_sequences_canonical(&mut blocks);
        state.domain.block_state.blocks = blocks.into_iter().map(|b| (b.id.clone(), b)).collect();
        state.rebuild_profile_tracking();

        if let Mutation::Update { id, fields, .. } = &self.event.mutation
            && state.domain.layout_blocks.render_source_ids.contains(id)
            && fields.contains_key("content")
            && let Some(block) = state.domain.block_state.blocks.get(id)
            && let Some(expr) =
                super::super::reference_state::render_expr_from_rhai(block.content.as_str())
        {
            state.domain.render_expressions.insert(id.clone(), expr);
        }

        state.domain.block_state.next_id += 1;

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
}

/// The composed `CapMap` has no seam, so it routes here. PROTOTYPE: the
/// `LoroPeer` arm is implemented (routed to the already-hosted `SutLoro` peer
/// cap, mirroring the seam's LoroPeer dispatch). The `External` (org) arm is
/// the next increment — together they give the org-vs-Loro differential the
/// shrinker can localize. Other sources are gated OUT of the composed alphabet
/// by the generator (`state.cap_set.is_some()`), so they cannot reach this
/// `panic!`.
#[allow(async_fn_in_trait)]
pub trait SutApplyMutation {
    async fn apply_mutation_routed(&self, event: MutationEvent);
}

#[allow(async_fn_in_trait)]
impl SutApplyMutation for holon_pbt_core::composition::CapMap {
    async fn apply_mutation_routed(&self, event: MutationEvent) {
        match event.source {
            MutationSource::LoroPeer { peer_idx } => {
                let loro = self.expect::<dyn SutLoro>();
                match &event.mutation {
                    Mutation::Create {
                        id,
                        parent_id,
                        fields,
                        ..
                    } => {
                        let parent_stable = if parent_id.is_no_parent() || parent_id.is_sentinel() {
                            None
                        } else {
                            Some(parent_id.id().to_string())
                        };
                        let content = fields
                            .get("content")
                            .and_then(|v| v.as_string())
                            .unwrap_or_default()
                            .to_string();
                        loro.apply_peer_create(
                            peer_idx,
                            parent_stable.as_deref(),
                            &content,
                            id.id(),
                        )
                        .await;
                    }
                    Mutation::Update { id, fields, .. } => {
                        let content = fields
                            .get("content")
                            .and_then(|v| v.as_string())
                            .unwrap_or_else(|| {
                                panic!("LoroPeer Update on {} carries no `content` field", id.id())
                            })
                            .to_string();
                        loro.apply_peer_update(peer_idx, id.id(), &content).await;
                    }
                    Mutation::Delete { id, .. } => {
                        loro.apply_peer_delete(peer_idx, id.id()).await;
                    }
                    Mutation::Move { id, .. } => panic!(
                        "LoroPeer mutation on {} has no peer mapping for `Move`",
                        id.id()
                    ),
                    Mutation::RestartApp => {
                        panic!("LoroPeer mutation cannot be `RestartApp`")
                    }
                }
            }
            MutationSource::External => {
                // Org-file ingress: rewrite the affected user doc(s) and let the live
                // FileSyncController re-ingest. Reuses the same-signature `SutSeamMutate`
                // cap (the frontend's real composed seam) — no bespoke trait.
                self.expect::<dyn holon_pbt_core::capabilities::SutSeamMutate>()
                    .apply_mutation(event)
                    .await;
            }
            other => panic!(
                "[composed ApplyMutation] source {other:?} is not yet routed on the CapMap \
                 (implemented: LoroPeer, External). The generator gates the composed alphabet to \
                 implemented arms, so UI/Action are unreachable here."
            ),
        }
    }
}

#[allow(async_fn_in_trait)]
impl<S: SutApplyMutation> TransitionImpl<ReferenceState, S> for ApplyMutation {
    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut S) {
        sut.apply_mutation_routed(self.event.clone()).await;
    }
}

#[cfg(feature = "otel-testing")]
use holon_pbt_core::capabilities::RefSqlCardinality;
#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for ApplyMutation {
    fn expected_sql<R: RefSqlCardinality>(&self, state: &R) -> ExpectedSql {
        let watches = state.active_watch_count();
        let blocks = state.block_count();
        let docs = state.document_count();
        expected_mutation_sql(&self.event.mutation, watches, blocks, docs)
    }
}
