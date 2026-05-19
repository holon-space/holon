//! Reference state machine: `VariantRef` wrapper and `ReferenceStateMachine` impl.
//!
//! This contains the transition generation, preconditions, and reference model
//! application logic for the property-based test.

use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;

use fluxdi::{Injector, Provider, Shared};
use proptest::prelude::*;
use proptest_state_machine::ReferenceStateMachine;

use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;

use super::reference_state::{ReferenceState, ShadowInterpreter};
use super::types::*;

use loro::{ExportMode, LoroDoc};

/// Whether the PBT generator may produce mutations that overwrite the root
/// layout (render-source content, layout headline content). The original
/// disable was to keep the `set_data`-doesn't-propagate-to-children bug
/// reproducible (layout mutations could swap out `state_toggle`, hiding it).
/// That bug is now fixed (ReactiveRowSet single-writer + ReadOnlyMutable
/// downstream + leaf signal subscriptions; see
/// `reactive_view_model::tests::shared_data_cell_updates_propagate_to_state_toggle_child`).
///
/// Custom `index.org` layouts that drop a sidebar (and layout-mutated panel
/// render sources) are handled in the generator: `ref_state.region_predictable(region)`
/// short-circuits `focusable_rendered_block_ids` so ClickBlock candidates only
/// arise for regions where ref_state can predict production's rendering.
pub(crate) const LAYOUT_MUTATIONS_ENABLED: bool = true;

/// Gate for `DragDropBlock` transition generation.
///
/// Wiring:
/// - `assets/default/types/block_profile.yaml` `editing` and `default`
///   variants wrap each block in `column(row(draggable(icon), …), drop_zone())`.
/// - `ViewKind::DropZone { op_name }` carries the dispatched op declaratively.
/// - `UserDriver::drop_entity` overrides drive headless (shadow tree walk
///   via `HeadlessInputRouter::block_contents`) and GPUI (real
///   `MouseDown` → `MouseMove(pressed=Left)` → `MouseUp` events).
///
/// Wiring complete (Apr 2026): block_profile draggable/drop_zone widgets
/// now bind their `data` to the current row so `row_id()` returns the
/// block's id. Headless `drop_entity` polls `block_contents` for both
/// widgets to appear and bootstraps the router on first call. inv-editable-text-has-draggable is
/// a hard panic if any focus-tree text block lacks a Draggable wrapper.
pub(crate) const DRAG_DROP_ENABLED: bool = true;

/// Simulate Loro CRDT merge for concurrent text updates.
///
/// Creates two Loro peers from a common ancestor, applies one update on each,
/// then merges them. Returns the CRDT-merged content.
pub(crate) fn loro_merge_text(original: &str, update_a: &str, update_b: &str) -> String {
    let ancestor = LoroDoc::new();
    ancestor.set_peer_id(0).unwrap();
    let text = ancestor.get_text("content");
    text.update(original, Default::default()).unwrap();
    ancestor.commit();
    let snapshot = ancestor.export(ExportMode::Snapshot).unwrap();

    let peer_a = LoroDoc::new();
    peer_a.set_peer_id(1).unwrap();
    peer_a.import(&snapshot).unwrap();
    peer_a
        .get_text("content")
        .update(update_a, Default::default())
        .unwrap();
    peer_a.commit();

    let peer_b = LoroDoc::new();
    peer_b.set_peer_id(2).unwrap();
    peer_b.import(&snapshot).unwrap();
    peer_b
        .get_text("content")
        .update(update_b, Default::default())
        .unwrap();
    peer_b.commit();

    let b_updates = peer_b.export(ExportMode::all_updates()).unwrap();
    peer_a.import(&b_updates).unwrap();

    peer_a.get_text("content").to_string()
}

#[derive(Debug, Clone)]
pub struct VariantRef<V: VariantMarker>(pub ReferenceState, PhantomData<V>);

impl<V: VariantMarker> std::ops::Deref for VariantRef<V> {
    type Target = ReferenceState;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<V: VariantMarker> std::ops::DerefMut for VariantRef<V> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// Reset a peer's baseline tracking to its current `blocks` snapshot.
///
/// The baseline drives `merge_peer_blocks_into_primary`'s concurrent-edit
/// detection: a primary block whose content differs from the baseline (and
/// whose stable ID is in the peer's `modified_stable_ids`) signals that
/// both sides edited the same text, so we run a Loro CRDT merge instead of
/// naive last-writer-wins.
pub(crate) fn refresh_peer_baseline(peer: &mut super::reference_state::PeerRefState) {
    peer.baseline_contents = peer
        .blocks
        .values()
        .map(|pb| (pb.stable_id.clone(), pb.content.clone()))
        .collect();
}

/// Merge peer blocks into the primary's block state.
///
/// - Blocks the peer created since the last sync (tracked in
///   `created_stable_ids`) are added to the primary.
/// - Existing blocks that the peer explicitly modified (tracked in
///   `modified_stable_ids`) get their content updated. If the primary
///   *also* edited that content since the baseline (`AddPeer` / last sync),
///   we run Loro's text CRDT merge instead of naive LWW — RGA keeps both
///   concurrent insertions.
/// - Inherited-at-AddPeer blocks the primary may have since deleted are
///   NOT re-added — Loro's CRDT keeps primary-side deletes.
pub(crate) fn merge_peer_blocks_into_primary(
    block_state: &mut super::reference_state::BlockState,
    peer_blocks: &[super::peer_ops::PeerBlock],
    modified_stable_ids: &std::collections::HashSet<String>,
    created_stable_ids: &std::collections::HashSet<String>,
    baseline_contents: &HashMap<String, String>,
) {
    for pb in peer_blocks {
        let block_uri = EntityUri::block(&pb.stable_id);
        if let Some(existing) = block_state.blocks.get_mut(&block_uri) {
            if modified_stable_ids.contains(&pb.stable_id) {
                let baseline = baseline_contents
                    .get(&pb.stable_id)
                    .cloned()
                    .unwrap_or_default();
                existing.content = if existing.content != baseline {
                    // Primary diverged too — Loro's text CRDT keeps both
                    // concurrent insertions. Compute the merged result.
                    loro_merge_text(&baseline, &existing.content, &pb.content)
                } else {
                    pb.content.clone()
                };
            }
            continue;
        }
        // Only re-add blocks the peer explicitly created since the last
        // sync; inherited blocks the primary deleted stay deleted.
        if !created_stable_ids.contains(&pb.stable_id) {
            continue;
        }
        let parent_uri = pb
            .parent_stable_id
            .as_deref()
            .map(EntityUri::block)
            .unwrap_or_else(EntityUri::no_parent);
        let mut block = Block::from_block_content(
            block_uri.clone(),
            parent_uri.clone(),
            holon_api::BlockContent::text(pb.content.clone()),
        );
        block.created_at = 0;
        block.updated_at = 0;
        block_state.blocks.insert(block_uri.clone(), block);
        block_state.block_documents.insert(block_uri, parent_uri);
    }
}

impl<V: VariantMarker> ReferenceStateMachine for VariantRef<V> {
    type State = Self;
    type Transition = crate::pbt::transitions::E2ETransition;

    fn init_state() -> BoxedStrategy<Self::State> {
        // Init is **constant**. All previously-random init inputs (notably
        // `keyword_set`) have been lifted into transitions so a fixture's
        // `Vec<E2ETransition>` fully reproduces a run — no hidden randomness
        // hiding in `state.*` that would silently drift between proptest's
        // generation and a saved fixture's replay.
        //
        // See `devlog/2026-05-19-phase-c-validation-diagnosis.md` for the
        // bug class this prevents (BulkExternalAdd reading
        // `state.keyword_set` at apply time, with the random init being the
        // hidden bug-triggering input).
        let injector = Injector::root();
        let interp = Shared::new(holon_frontend::shadow_builders::build_shadow_interpreter());
        injector.provide::<ShadowInterpreter>(Provider::root({
            let s = interp;
            move |_| s.clone()
        }));
        let interpreter: Arc<ShadowInterpreter> = injector.resolve::<ShadowInterpreter>();

        Just(VariantRef(
            ReferenceState::new(V::variant(), interpreter),
            PhantomData,
        ))
        .boxed()
    }

    fn transitions(state: &Self::State) -> BoxedStrategy<Self::Transition> {
        crate::pbt::transitions::aggregate_transitions(state)
    }

    fn preconditions(state: &Self::State, transition: &Self::Transition) -> bool {
        // Dispatched through the per-transition `TransitionRef` trait
        // (ref-side, S-independent); each variant's precondition lives in
        // `transitions/<name>.rs`. VariantRef derefs to ReferenceState.
        use holon_pbt_core::TransitionRef;
        use validated::Validated;
        match transition.preconditions(state) {
            Validated::Good(()) => true,
            Validated::Fail(reasons) => {
                crate::pbt::validation::record_rejection(transition.variant_name(), &reasons);
                false
            }
        }
    }
    fn apply(mut state: Self::State, transition: &Self::Transition) -> Self::State {
        use holon_pbt_core::TransitionRef;
        transition.apply_to_ref(&mut state);
        state.last_transition_kind = Some(transition.variant_name());
        state
    }
}
