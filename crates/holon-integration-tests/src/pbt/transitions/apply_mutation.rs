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

use crate::pbt::reference_state::ReferenceState;
use holon_pbt_core::capabilities::SutLoro;
use holon_pbt_core::{TransitionFactory, TransitionImpl, TransitionRef};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, expected_mutation_sql};

use holon_api::block::Block;
use holon_api::{ContentType, EntityUri, QueryLanguage, Value};

use crate::assign_reference_sequences_canonical;
use crate::pbt::generators::{
    generate_layout_headline_mutation, generate_mutation, generate_profile_content_mutation,
    generate_render_source_mutation,
};
use crate::pbt::state_machine::LAYOUT_MUTATIONS_ENABLED;
use crate::pbt::sut::E2ESut;
use crate::pbt::sut_row_parsing::{mutation_expected_properties, row_properties_to_map};
use crate::pbt::types::{Mutation, MutationEvent, MutationSource};

use std::collections::HashMap;
use std::time::{Duration, Instant};

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
                dyn crate::pbt::local_caps::SutSeamMutate,
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
                                    entity: "block".to_string(),
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
                                        entity: "block".to_string(),
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
                                        entity: "block".to_string(),
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

/// SUT-side dispatch for [`ApplyMutation`], routed by the mutation's `source`.
///
/// This is the "one transition, source-as-shrinkable-axis" model (challenging the
/// one-cap-per-transition guideline): a single mutation can arrive through different
/// production INGRESS PATHS (org file-sync / Loro peer / UI gesture), and *which*
/// path is a shrinkable field on the transition — so the subsystem/source shrinker can
/// localize "diverges via org but not via Loro". Unlike `UserDriver` (a fidelity
/// LADDER with auto-descend), sources are an ORTHOGONAL categorical axis: there is no
/// "highest available" source; the generator samples among the *wired* ones.
///
/// The routing lives on the SUT, not in a single component, because the arms target
/// DIFFERENT caps (Loro peer vs file-seam vs driver) that, on the composed `CapMap`,
/// live on different components. `impl … for CapMap` is the one place that can reach
/// every sub-cap (`self.expect::<dyn …>()`); `E2ESut` keeps its unified
/// `block_tree_post_action` seam, so its impl is a no-op (the seam owns `ref_state`).
#[allow(async_fn_in_trait)]
pub trait SutApplyMutation {
    async fn apply_mutation_routed(&self, event: MutationEvent);
}

/// `E2ESut` routes `ApplyMutation` through its `block_tree_post_action` seam (which
/// owns `ref_state`), so the cap-level dispatch is a no-op — exactly as the old
/// `SutSeamMutate` no-op was. Keeps the monolith's behavior identical.
#[allow(async_fn_in_trait)]
impl SutApplyMutation for E2ESut {
    async fn apply_mutation_routed(&self, _: MutationEvent) {}
}

/// The composed `CapMap` has no seam, so it routes here. PROTOTYPE: the `LoroPeer`
/// arm is implemented (routed to the already-hosted `SutLoro` peer cap, mirroring the
/// seam's LoroPeer dispatch). The `External` (org) arm is the next increment — together
/// they give the org-vs-Loro differential the shrinker can localize. Other sources are
/// gated OUT of the composed alphabet by the generator (`state.cap_set.is_some()`), so
/// they cannot reach this `panic!`.
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
                self.expect::<dyn crate::pbt::local_caps::SutSeamMutate>()
                    .apply_mutation(event)
                    .await;
            }
            other => panic!(
                "[composed ApplyMutation] source {other:?} is not yet routed on the CapMap \
                 (implemented: LoroPeer, External). The generator gates the composed alphabet \
                 to implemented arms, so UI/Action are unreachable here."
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
impl crate::pbt::transition_budgets::SqlBudget for ApplyMutation {
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        let watches = state.mcp.active_watches.len();
        let blocks = state.domain.block_state.blocks.len();
        let docs = state.files.documents.len();
        expected_mutation_sql(&self.event.mutation, watches, blocks, docs)
    }
}

/// SUT-side body of `ApplyMutation`. Dispatches the mutation through the
/// appropriate driver path (UI keychord → driver fallback, External org-file
/// write, Action synthetic_dispatch), then waits for the block count to match
/// the reference, spot-checks the mutated row in `block_raw`, awaits org-file
/// convergence, and (for External) polls until the backend reflects the
/// content + properties change.
///
/// Extracted from `E2ESut::apply_mutation` so the SUT-struct file stops
/// carrying transition logic; the `SutHandle::apply_apply_mutation` impl now
/// just forwards here.
pub async fn apply_apply_mutation_to_sut(
    sut: &mut E2ESut,
    event: MutationEvent,
    ref_state: &ReferenceState,
) {
    match event.source {
        MutationSource::UI => {
            let (entity, op, mut params) = event.mutation.to_operation();

            // The reference model uses file-based document URIs (e.g. "file:doc_0.org")
            // but the real system assigns UUID-based IDs. Resolve before executing.
            if let Some(Value::String(pid)) = params.get("parent_id") {
                let pid = EntityUri::parse(pid).expect("Unable to parse parent_id");
                let resolved = sut.resolve_uri(&pid);
                params.insert("parent_id".to_string(), resolved.clone().into());
            }

            // Try keychord path first: if the operation has a keybinding, dispatch
            // via send_key_chord → shadow index → bubble_input. This exercises the
            // full keybinding pipeline, same as pressing Cmd+Enter in GPUI.
            //
            // The chord path clicks the entity before pressing the chord
            // (`send_key_chord` → click + focus), but `apply_to_ref` does NOT
            // model that click. Today no ApplyMutation op (create / set_field /
            // update / delete / move) has a binding in the engine registry, so
            // the path is unreachable; if a binding appears, the unmodeled
            // click-focus would diverge ref vs SUT and only surface ~1000 log
            // lines later in inv-focus-matches-ref. Fail HERE with the fix
            // instead of dispatching: mirror Indent/Outdent/MoveUp/MoveDown's
            // `transitions::model_chord_click_focus` in `apply_to_ref`, then
            // restore the dispatch (git history has it).
            let dispatched_via_keychord = if let Some(block_id) =
                params.get("id").and_then(|v| v.as_string())
            {
                if let Some(chord) = sut.find_keybinding_for_op(&op) {
                    panic!(
                        "[E2ESut::apply_mutation] op '{op}' now has a keybinding ({chord:?}) for \
                         block '{block_id}', but the chord path's click-focus is not modeled in \
                         ApplyMutation::apply_to_ref. Add the model_chord_click_focus mirror \
                         (see indent.rs) before enabling chord dispatch for this op."
                    );
                } else {
                    false
                }
            } else {
                false
            };

            if !dispatched_via_keychord {
                // Pure content edits (`block::set_field { field: "content" }`)
                // are routed through the real editor via `replace_text` —
                // click-to-focus + per-keystroke typing — so they exercise the
                // same MutableText / `on_text_changed` / structural-decision
                // pipeline a user's typing would, rather than `synthetic_dispatch`.
                //
                // Gated on `enable_loro` as a TEMPORARY wiring limitation, not
                // an architectural one: Loro-as-CRDT (and thus `MutableText`) is
                // independent of Loro-as-storage/P2P, so sql_only *should* be
                // able to drive the real editor too. But today the cell registry
                // wires no Loro backend in SqlOnly (`BlockCellRegistry::sql_only`
                // → `write_field` returns false; `editable_text` Errs), so the
                // mirror's char insert no-ops on content (it only tracks the
                // cursor). Until a storage-decoupled Loro CRDT is wired into
                // sql_only (see `headless_editor_mirror.rs` `sql_block_content`
                // TODO), those edits must stay on the synthetic `set_field` SQL
                // path to actually land. Once wired, drop this `enable_loro`
                // guard so content edits run through the real editor in both modes.
                let driver = sut
                    .driver
                    .borrow()
                    .clone()
                    .expect("driver not installed — was start_app called?");
                // Only drive the real editor for blocks that are currently
                // rendered: a real user can't click+type a block that isn't on
                // screen. Content edits on non-rendered blocks (e.g. inside a
                // collapsed/offscreen subtree the ref-model still mutates) route
                // to the synthetic path below instead.
                let content_target = if ref_state.enable_loro()
                    && op == "set_field"
                    && params.get("field").and_then(|v| v.as_string()) == Some("content")
                {
                    params
                        .get("id")
                        .and_then(|v| v.as_string())
                        .map(holon_api::entity_uri_from_id_str)
                        .filter(|id| driver.displayed_text(id).is_some())
                } else {
                    None
                };
                if let Some(id) = content_target {
                    let content = params
                        .get("value")
                        .and_then(|v| v.as_string())
                        .expect("set_field content edit always carries a value")
                        .to_string();
                    driver
                        .replace_text(&id, &content)
                        .await
                        .unwrap_or_else(|e| panic!("replace_text({id}) failed: {e:#}"));
                } else {
                    // SYNTHETIC: abstract mutations with no reachable user
                    // gesture (property updates, `block::update` packing custom
                    // props, parent moves, content edits on non-rendered blocks).
                    // These keep the synthetic path until they get real
                    // user-gesture coverage of their own.
                    eprintln!(
                        "[E2ESut::apply_mutation] Direct dispatch: entity={}, op={}",
                        entity, op
                    );
                    match driver.synthetic_dispatch(&entity, &op, params).await {
                        Ok(()) => {
                            eprintln!("[E2ESut::apply_mutation] synthetic_dispatch returned Ok")
                        }
                        Err(e) => panic!("Operation {}.{} failed: {:?}", entity, op, e),
                    }
                }
            }
        }

        MutationSource::External => {
            // Resolve file-based doc URIs to UUID-based (ctx.documents is re-keyed
            // to UUID after start_app). Block-to-block parent_ids pass through unchanged.
            eprintln!("[E2ESut::apply_mutation] External mutation - writing to Org file");
            let expected_blocks = sut.resolve_ref_blocks(ref_state, true);
            sut.ctx
                .apply_external_mutation(&expected_blocks)
                .await
                .unwrap_or_else(|e| {
                    panic!("[E2ESut::apply_mutation] External mutation failed: {e:?}")
                });
            // Deterministic handoff (ADR 0011): the in-memory write delivered
            // its change event synchronously; wait for the controller to have
            // processed exactly that seq before any settle/invariant logic.
            let seq = sut.ctx.org_fs.last_change_seq();
            sut.ctx
                .wait_for_org_change_processed(seq, Duration::from_millis(10000))
                .await;
            eprintln!(
                "[E2ESut::apply_mutation] External mutation processed by org sync (seq {seq})"
            );
        }

        MutationSource::Action => {
            // Action-sourced mutations are autonomous: the action watcher
            // observes a query result and calls `engine.execute_operation`
            // directly (see `action_watcher.rs::run_discovery_loop`).
            // There is no user keystroke or click to simulate here, so
            // routing through `send_key_chord` / `click_entity` would
            // *invent* a gesture the production code path never makes.
            // `synthetic_dispatch` is the faithful mirror of what the
            // action watcher actually does in production.
            let (entity, op, mut params) = event.mutation.to_operation();

            if let Some(Value::String(pid)) = params.get("parent_id") {
                let pid = EntityUri::parse(pid).expect("Unable to parse parent_id");
                let resolved = sut.resolve_uri(&pid);
                params.insert("parent_id".to_string(), resolved.clone().into());
            }

            eprintln!(
                "[E2ESut::apply_mutation] Action dispatch: entity={}, op={}",
                entity, op
            );
            let driver = sut
                .driver
                .borrow()
                .clone()
                .expect("driver not installed — was start_app called?");
            match driver.synthetic_dispatch(&entity, &op, params).await {
                Ok(()) => {
                    eprintln!("[E2ESut::apply_mutation] Action synthetic_dispatch returned Ok")
                }
                Err(e) => panic!("Action operation {}.{} failed: {:?}", entity, op, e),
            }
        }

        MutationSource::LoroPeer { peer_idx } => {
            // Apply the generic mutation through a Loro CRDT *peer* (not the
            // primary BackendEngine). Peer blocks are keyed by stable id, which
            // for a primary-derived block is `id.id()` (the bare, schemeless
            // id). No merge happens here — convergence is a separate
            // `SyncWithPeer`/`MergeFromPeer` transition — so we return right
            // after the peer edit, skipping the primary-convergence gates below.
            eprintln!(
                "[E2ESut::apply_mutation] LoroPeer mutation on peer {}: {:?}",
                peer_idx, event.mutation
            );
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
                    sut.apply_peer_create(peer_idx, parent_stable.as_deref(), &content, id.id())
                        .await;
                }
                Mutation::Update { id, fields, .. } => {
                    let content = fields
                        .get("content")
                        .and_then(|v| v.as_string())
                        .unwrap_or_else(|| {
                            panic!(
                                "LoroPeer Update on {} carries no `content` field — the peer \
                                 surface only models block-level content edits; the generator \
                                 must not emit non-content updates through LoroPeer",
                                id.id()
                            )
                        })
                        .to_string();
                    sut.apply_peer_update(peer_idx, id.id(), &content).await;
                }
                Mutation::Delete { id, .. } => {
                    sut.apply_peer_delete(peer_idx, id.id()).await;
                }
                Mutation::Move { id, .. } => panic!(
                    "LoroPeer mutation on {} has no peer mapping for `Move`: the peer surface \
                     exposes create/update/delete only; the generator must not emit Move \
                     through LoroPeer",
                    id.id()
                ),
                Mutation::RestartApp => panic!(
                    "LoroPeer mutation cannot be `RestartApp`: restart is a primary-instance \
                     lifecycle event with no peer mapping; the generator must not emit it \
                     through LoroPeer"
                ),
            }
            return;
        }
    }

    // Post-mutation count gate. This reads Turso's seed-calibrated CDC
    // accumulator (`block_raw` + the all-blocks mirror) — there is no unified
    // capability for it, and the no-Turso path converges instead via the polling
    // spot-check below plus the pre-invariant settle barrier — so it is gated on
    // the explicit storage backend the harness started, not a capability proxy.
    let expected_count = E2ESut::expected_content_block_count(ref_state);
    let expected_ids = sut.expected_block_ids(ref_state);
    if matches!(sut.ctx.storage(), holon::di::StorageSelector::Turso) {
        sut.await_block_count_or_panic(
            &expected_ids,
            expected_count,
            Duration::from_millis(10000),
            "E2ESut::apply_mutation",
        )
        .await;
    }

    // Spot-check: verify the mutated block's projected fields through the
    // `BlockQuerySource` capability — Turso CDC mirror or Loro tree, no backend
    // branch. Poll the snapshot until the block matches, which doubles as the
    // convergence wait for the no-Turso path. UI mutations only — External
    // mutations propagate through the file watcher (handled below).
    if event.source == MutationSource::UI
        && let Some(block_id) = event.mutation.target_block_id()
        && let Some(expected_block) = ref_state.domain.block_state.blocks.get(&block_id)
    {
        // Map synthetic split ids (`block::split-N`) to the real id via doc_uri_map,
        // so blocks created by SplitBlock are found under their real id.
        let resolved_block_id = sut.resolve_uri(&block_id);
        let source = sut.ctx.session().block_query().clone();
        let expected_content = expected_block.content.trim().to_string();
        let deadline = Instant::now() + Duration::from_millis(10000);
        loop {
            let snapshot = source.snapshot().await.unwrap_or_else(|e| {
                panic!(
                    "Post-mutation spot-check: `BlockQuerySource::snapshot()` failed for block '{}': {e}",
                    block_id
                )
            });
            let found = snapshot.iter_blocks().find(|b| b.id == resolved_block_id);
            if let Some(b) = found
                && b.content.trim() == expected_content
                && b.content_type == expected_block.content_type
            {
                break;
            }
            if Instant::now() >= deadline {
                let got = snapshot.iter_blocks().find(|b| b.id == resolved_block_id);
                panic!(
                    "Post-mutation spot-check timeout for block '{}': expected content {:?} / type {:?}, got {:?}",
                    block_id,
                    expected_content,
                    expected_block.content_type,
                    got.map(|b| (b.content.clone(), b.content_type)),
                );
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    } // UI mutations only

    // Org-file convergence + the External spot-check below are org-adapter
    // (file-watcher) test scaffolding: only the Turso wiring carries an Org
    // adapter and emits External mutations. No-Turso has no on-disk org files, so
    // skip — again keyed on the explicit storage backend, not a capability proxy.
    if !matches!(sut.ctx.storage(), holon::di::StorageSelector::Turso) {
        return;
    }

    // Wait for the org sync loop to stabilize (no more writes).
    sut.await_org_file_convergence().await;

    // External mutations write to disk; the file watcher asynchronously
    // delivers the change to the backend. `await_org_file_convergence` only
    // waits for the file itself to match, not for the backend to catch up.
    // For content or property updates (no count change), this can cause the
    // invariant check to run before the backend has the new state.
    //
    // Spot-check the mutated block's content AND properties in the backend,
    // polling until they match or the timeout fires. Properties are checked
    // against `event.mutation.fields` so custom-property updates like
    // `{effort: "7yzXz"}` also wait for SQL to catch up.
    if event.source == MutationSource::External
        && let Some(block_id) = event.mutation.target_block_id()
    {
        let resolved_id = sut.resolve_uri(&block_id);
        if let Some(expected_block) = ref_state.domain.block_state.blocks.get(&block_id) {
            let expected_content = expected_block.content.trim().to_string();
            let expected_properties: HashMap<String, Value> =
                mutation_expected_properties(&event.mutation);
            let deadline = Instant::now() + Duration::from_millis(5000);
            loop {
                let prql = format!(
                    "from block_raw | filter id == \"{}\" | select {{content, properties}}",
                    resolved_id
                );
                let rows = sut
                    .test_ctx()
                    .query(prql, QueryLanguage::HolonPrql, HashMap::new())
                    .await
                    .expect("[E2ESut::apply_mutation] External spot-check query failed");
                let row = rows.first();
                let actual_content = row
                    .and_then(|r| r.get("content"))
                    .and_then(|v| v.as_string())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let actual_properties = row
                    .and_then(|r| r.get("properties"))
                    .map(row_properties_to_map)
                    .unwrap_or_default();
                let content_match = actual_content == expected_content;
                let properties_match = expected_properties
                    .iter()
                    .all(|(k, v)| actual_properties.get(k) == Some(v));
                if content_match && properties_match {
                    break;
                }
                if Instant::now() >= deadline {
                    panic!(
                        "[E2ESut::apply_mutation] External sync timeout for \
                             block '{}': content actual={:?} expected={:?}; \
                             properties actual={:?} expected={:?}",
                        resolved_id,
                        actual_content,
                        expected_content,
                        actual_properties,
                        expected_properties
                    );
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
    }
}
