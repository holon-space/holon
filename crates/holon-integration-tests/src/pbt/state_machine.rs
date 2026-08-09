//! Reference state machine: the `ReferenceMachine` (Full wiring) and the
//! `ReferenceStateMachine` impl.
//!
//! This contains the transition generation, preconditions, and reference model
//! application logic for the property-based test.

use std::collections::HashMap;
use std::sync::Arc;

use fluxdi::Injector;
use fluxdi::Provider;
use fluxdi::Shared;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_orgmode::OrgBlockExt;
use proptest::prelude::*;
use proptest_state_machine::ReferenceStateMachine;

use super::reference_state::ReferenceState;
use super::reference_state::ShadowInterpreter;

/// Whether the PBT generator may produce mutations that overwrite the root
/// layout (render-source content, layout headline content). The original
/// disable was to keep the `set_data`-doesn't-propagate-to-children bug
/// reproducible (layout mutations could swap out `state_toggle`, hiding it).
/// That bug is now fixed (ReactiveRowSet single-writer + ReadOnlyMutable
/// downstream + leaf signal subscriptions; see
/// `reactive_view_model::tests::shared_data_cell_updates_propagate_to_state_toggle_child`).
///
/// Custom `index.org` layouts that drop a sidebar (and layout-mutated panel
/// render sources) are handled in the generator:
/// `ref_state.region_predictable(region)` short-circuits
/// `focusable_rendered_block_ids` so ClickBlock candidates only
/// arise for regions where ref_state can predict production's rendering.
pub(crate) const LAYOUT_MUTATIONS_ENABLED: bool = true;

/// Gate for `DragDropBlock` transition generation.
///
/// Wiring:
/// - `assets/default/types/block_profile.yaml` `editing` and `default` variants
///   wrap each block in `column(row(draggable(icon), …), drop_zone())`.
/// - `ViewKind::DropZone { op_name }` carries the dispatched op declaratively.
/// - `UserDriver::drop_entity` overrides drive headless (shadow tree walk via
///   `HeadlessInputRouter::block_contents`) and GPUI (real `MouseDown` →
///   `MouseMove(pressed=Left)` → `MouseUp` events).
///
/// Wiring complete (Apr 2026): block_profile draggable/drop_zone widgets
/// now bind their `data` to the current row so `row_id()` returns the
/// block's id. Headless `drop_entity` polls `block_contents` for both
/// widgets to appear and bootstraps the router on first call.
/// inv-editable-text-has-draggable is a hard panic if any focus-tree text block
/// lacks a Draggable wrapper.
pub(crate) const DRAG_DROP_ENABLED: bool = true;

/// Build a fresh, constant `ReferenceState` for the given [`Wiring`]. Used by
/// every reference state machine's `init_state` (the canonical full-coverage
/// [`ReferenceMachine`] and each `declare_pbt_slice!`-generated machine), so
/// the wiring is the *only* thing that differs between manifests.
///
/// Init is **constant** apart from the wiring. All previously-random init
/// inputs (notably `keyword_set`) have been lifted into transitions so a
/// fixture's `Vec<E2ETransition>` fully reproduces a run — no hidden
/// randomness in `state.*` that would silently drift between proptest's
/// generation and a saved fixture's replay. See
/// `devlog/2026-05-19-phase-c-validation-diagnosis.md`.
pub fn fresh_reference_state(wiring: holon_pbt_core::Wiring) -> ReferenceState {
    let injector = Injector::root();
    let interp = Shared::new(holon_frontend::shadow_builders::build_shadow_interpreter());
    injector.provide::<ShadowInterpreter>(Provider::root({
        let s = interp;
        move |_| s.clone()
    }));
    let interpreter: Arc<ShadowInterpreter> = injector.resolve::<ShadowInterpreter>();
    ReferenceState::new(wiring, interpreter)
}

/// A [`fresh_reference_state`] advanced to the minimal "app started + properly
/// set up" state the editor transition preconditions require, **honestly**:
///
/// - `action.app_started = true`
/// - one seed-classified query block (its `block_documents` entry is
///   `no_parent`, so it is excluded from the non-seed block comparison) whose
///   id is registered in `layout_blocks.query_source_ids` — this is exactly
///   what `is_properly_setup()` checks (`!query_source_ids.is_empty()`).
///
/// It deliberately does **not** seed the working tree or open an editor — those
/// are caller concerns (the working tree is slice-specific, and the editor must
/// be opened only when the UI actor is wired). The full symmetric `StartApp` on
/// both sides is the deferred committed-parity path.
pub fn started_reference_state(wiring: holon_pbt_core::Wiring) -> ReferenceState {
    let mut state = fresh_reference_state(wiring);
    state.action.app_started = true;

    let query_id = EntityUri::block("started-ref-layout-query");
    state.domain.block_state.blocks.insert(
        query_id.clone(),
        Block::new_text(query_id.clone(), EntityUri::no_parent(), "query"),
    );
    state
        .domain
        .block_state
        .block_documents
        .insert(query_id.clone(), EntityUri::no_parent());
    state.domain.layout_blocks.query_source_ids.insert(query_id);

    state
}

/// Map a PBT [`Wiring`](holon_pbt_core::Wiring) manifest onto the SUT storage
/// substrate it implies (ADR 0004 Phase 9, part (a)). A manifest that includes
/// the query-capable `Turso` adapter builds the historical Turso SUT; a
/// Loro-only manifest (`Wiring::loro_backend()`) builds the no-Turso
/// `LoroMemory` SUT (no `BackendEngine`; reads via `BlockQuerySource`,
/// mutations via the Loro-native `OperationEngine`). This is what makes a
/// slice's `wiring:` select its backend instead of always getting Turso.
pub fn storage_selector_for_wiring(wiring: &holon_pbt_core::Wiring) -> holon::di::StorageSelector {
    if wiring
        .storage_adapters
        .contains(&holon_pbt_core::StorageAdapter::Turso)
    {
        holon::di::StorageSelector::Turso
    } else {
        holon::di::StorageSelector::LoroMemory
    }
}

/// The canonical full-coverage reference state machine (Full wiring). Drives
/// the GPUI real-window replay (`phased.rs`) and is `E2ESut`'s `Reference`.
/// `declare_pbt_slice!` generates its own per-manifest machine instead of
/// using this one, so each slice's `init_state` carries that slice's wiring.
#[derive(Debug, Clone)]
pub struct ReferenceMachine;

/// Merge peer blocks into the primary's block state, CONSUMING the shadow
/// mesh (which has already run the REAL CRDT sync on the shadow docs).
///
/// - Blocks the peer created since the last sync (tracked in
///   `created_stable_ids`) are added to the primary.
/// - Existing blocks that the peer explicitly modified (tracked in
///   `modified_stable_ids`) take the SHADOW primary's converged text — the
///   actual Loro merge outcome (concurrent-insert interleaving included),
///   predicted by the shadow instead of modeled or adopted from the SUT.
/// - Inherited-at-AddPeer blocks the primary may have since deleted are NOT
///   re-added — Loro's CRDT keeps primary-side deletes.
/// - Peer-created blocks are stamped with a `sequence` AFTER the parent's
///   existing children, modeling Loro's append-at-end create (their tree
///   fractional index sorts after every sibling the creating peer saw). Their
///   order WITHIN one merge is CRDT-arbitrary (Loro breaks fi ties by op id =
///   (lamport, peer id)), so it is stamped from the SHADOW primary's converged
///   child order — the clock-padded shadow reproduces the op-id tie-break
///   exactly (`clock_parity_spike`).
pub(crate) fn merge_peer_blocks_into_primary(
    block_state: &mut super::block_state::BlockState,
    peer_blocks: &[super::peer_ops::PeerBlock],
    modified_stable_ids: &std::collections::HashSet<String>,
    created_stable_ids: &std::collections::HashSet<String>,
    shadow: &super::shadow_mesh::ShadowMesh,
) {
    let shadow_text = |stable_id: &str| -> String {
        shadow.primary_content(stable_id).unwrap_or_else(|| {
            panic!("shadow primary lacks merged block {stable_id} — shadow mesh desynced")
        })
    };
    let mut created: Vec<&super::peer_ops::PeerBlock> = Vec::new();
    for pb in peer_blocks {
        let block_uri = EntityUri::block(&pb.stable_id);
        if let Some(existing) = block_state.blocks.get_mut(&block_uri) {
            if modified_stable_ids.contains(&pb.stable_id) {
                existing.content = shadow_text(&pb.stable_id);
            }
            continue;
        }
        // Only re-add blocks the peer explicitly created since the last
        // sync; inherited blocks the primary deleted stay deleted.
        if !created_stable_ids.contains(&pb.stable_id) {
            continue;
        }
        created.push(pb);
    }
    // Deterministic stamping order (peer_blocks arrives in HashMap order).
    created.sort_by(|a, b| a.stable_id.cmp(&b.stable_id));
    let mut next_seq_by_parent: HashMap<EntityUri, i64> = HashMap::new();
    for pb in &created {
        let block_uri = EntityUri::block(&pb.stable_id);
        let parent_uri = pb
            .parent_stable_id
            .as_deref()
            .map(EntityUri::block)
            .unwrap_or_else(EntityUri::no_parent);
        let mut block = Block::from_block_content(
            block_uri.clone(),
            parent_uri.clone(),
            holon_api::BlockContent::text(shadow_text(&pb.stable_id)),
        );
        block.created_at = 0;
        block.updated_at = 0;
        let next_seq = next_seq_by_parent
            .entry(parent_uri.clone())
            .or_insert_with(|| {
                block_state
                    .blocks
                    .values()
                    .filter(|b| b.parent_id == parent_uri)
                    .map(|b| b.sequence())
                    .max()
                    .map_or(0, |m| m + 1)
            });
        block.set_sequence(*next_seq);
        *next_seq += 1;
        block_state.blocks.insert(block_uri.clone(), block);
        block_state.block_documents.insert(block_uri, parent_uri);
    }
    // Permute every ≥2-member peer-created sibling group into the shadow's
    // converged relative order, within the sequence slots they already
    // occupy — the tie-break prediction replacing
    // `adopt_observed_peer_sibling_order`. Grouping spans ALL `block:peer-…`
    // siblings under a parent (not just this merge's creates): concurrent
    // creates from different peers tie in fractional index yet can arrive
    // via SEPARATE syncs, so arrival order ≠ op-id order (the keystone
    // caught exactly this: peer A's lamport-bumped create arrived first but
    // sorts second).
    let mut by_parent: HashMap<EntityUri, Vec<EntityUri>> = HashMap::new();
    for (id, b) in &block_state.blocks {
        if id.as_str().starts_with("block:peer-") {
            by_parent
                .entry(b.parent_id.clone())
                .or_default()
                .push(id.clone());
        }
    }
    for (parent_uri, mut members) in by_parent {
        if members.len() < 2 {
            continue;
        }
        let parent_sid = if parent_uri.is_no_parent() || parent_uri.is_sentinel() {
            None
        } else {
            Some(parent_uri.id())
        };
        let observed = shadow.primary_children_order(parent_sid);
        let rank: HashMap<&str, usize> = observed
            .iter()
            .enumerate()
            .map(|(i, s)| (s.as_str(), i))
            .collect();
        for m in &members {
            assert!(
                rank.contains_key(m.id()),
                "created block {m} missing from the shadow's child order under {parent_uri} \
                 (observed: {observed:?}) — shadow mesh desynced"
            );
        }
        let mut seqs: Vec<i64> = members
            .iter()
            .map(|m| block_state.blocks[m].sequence())
            .collect();
        seqs.sort_unstable();
        members.sort_by_key(|m| rank[m.id()]);
        for (m, seq) in members.iter().zip(seqs) {
            block_state
                .blocks
                .get_mut(m)
                .expect("group member present")
                .set_sequence(seq);
        }
    }
}

impl ReferenceStateMachine for ReferenceMachine {
    type State = ReferenceState;
    type Transition = crate::pbt::transitions::E2ETransition;

    fn init_state() -> BoxedStrategy<Self::State> {
        Just(fresh_reference_state(holon_pbt_core::Wiring::full())).boxed()
    }

    fn transitions(state: &Self::State) -> BoxedStrategy<Self::Transition> {
        crate::pbt::transitions::aggregate_transitions(state)
    }

    fn preconditions(state: &Self::State, transition: &Self::Transition) -> bool {
        // Alphabet gate FIRST, exactly as `WideE2EMachine::preconditions` and the
        // generation/replay/shrink stepper apply it: a transition whose
        // `required_wiring` + `required_caps` the state cannot satisfy is not in
        // the alphabet, so `preconditions` (the ONLY filter proptest re-applies
        // when it shrinks the initial state) must reject it too. A NO-OP for THIS
        // machine's fixed `Wiring::full()` / `cap_set == None` init (nothing is
        // ever gated out), but the gate lives here so a future variant that draws
        // its wiring can never keep a transition its shrunk CapMap has no provider
        // for and die in `CapMap::expect` instead of shrinking the divergence
        // (task #46's escape class).
        if !crate::pbt::stepper::transition_applicable(state, transition) {
            return false;
        }
        // Dispatched through the per-transition `TransitionRef` trait
        // (ref-side, S-independent); each variant's precondition lives in
        // `transitions/<name>.rs`.
        use holon_pbt_core::TransitionRef;
        use validated::Validated;
        match transition.preconditions(state) {
            Validated::Good(()) => true,
            Validated::Fail(reasons) => {
                holon_pbt_core::validation::record_rejection(transition.variant_name(), &reasons);
                false
            }
        }
    }
    fn apply(mut state: Self::State, transition: &Self::Transition) -> Self::State {
        use holon_pbt_core::TransitionRef;
        transition.apply_to_ref(&mut state);
        state.action.last_transition_kind = Some(transition.variant_name());
        state
    }
}

#[cfg(test)]
mod tests {
    use holon_pbt_core::StorageAdapter;
    use holon_pbt_core::TransitionRef;
    use holon_pbt_core::Wiring;
    use proptest_state_machine::ReferenceStateMachine;

    use super::ReferenceMachine;
    use crate::pbt::composed::wide_e2e::wide_e2e_ref_for;
    use crate::pbt::reference_state::ReferenceState;
    use crate::pbt::transitions::CreateDocument;
    use crate::pbt::transitions::E2ETransition;

    /// Twin of `wide_e2e::tests::shrunk_wiring_rejects_a_transition_its_capmap_cannot_host`
    /// at the `ReferenceMachine` level. The machine's own `init_state` fixes
    /// `Wiring::full()` / `cap_set == None`, so its gate never actually fires —
    /// but if a future variant drew a restrictive wiring, `preconditions` (the
    /// ONLY filter proptest re-applies while shrinking the initial state) MUST
    /// reject a gated-out transition. Here we hand
    /// `ReferenceMachine::preconditions` a restrictive state directly to
    /// prove the alphabet gate is wired in, not only present in
    /// `WideE2EMachine` (task #46's escape class).
    ///
    /// `CreateDocument` is the witness: its ref-side precondition is only
    /// `app_started`, which holds for every wiring, so the cap gate is the sole
    /// possible rejecter.
    #[test]
    fn reference_machine_preconditions_reject_a_capmap_gated_transition() {
        let narrow = Wiring::custom(vec![StorageAdapter::Loro], vec![], vec![]);
        let shrunk = wide_e2e_ref_for(&narrow);
        let transition = E2ETransition::CreateDocument(CreateDocument {
            file_name: "ref-shrink-probe.org".to_string(),
        });

        assert!(
            !shrunk.caps_available(&transition.required_caps()),
            "premise: a Loro-only draw composes no frontend component, so its CapMap has no \
             SutAppLifecycle"
        );
        assert!(
            <E2ETransition as TransitionRef<ReferenceState>>::preconditions(&transition, &shrunk)
                .is_good(),
            "premise: the ref-side precondition (app_started) holds for every wiring, so the cap \
             gate is the only thing that can reject this transition"
        );

        assert!(
            !<ReferenceMachine as ReferenceStateMachine>::preconditions(&shrunk, &transition),
            "ReferenceMachine::preconditions must reproduce the alphabet gate: a transition whose \
             caps the wiring cannot provide has to be rejected, otherwise a future wiring-drawing \
             variant boots a SUT that panics in CapMap::expect (task #46)"
        );
    }

    /// The no-op-today half: under the machine's real init (full wiring,
    /// unrestricted cap_set) the gate rejects NOTHING, so `preconditions` is
    /// governed purely by the ref-side precondition — the added gate cannot
    /// change any current keystone/hand-authored result.
    #[test]
    fn reference_machine_gate_is_a_noop_under_full_wiring() {
        let full = wide_e2e_ref_for(&Wiring::full());
        let transition = E2ETransition::CreateDocument(CreateDocument {
            file_name: "ref-full-probe.org".to_string(),
        });

        assert!(
            crate::pbt::stepper::transition_applicable(&full, &transition),
            "the alphabet gate must pass under full wiring / unrestricted cap_set — it gates \
             nothing here, so the fix is a pure no-op for the shipped ReferenceMachine"
        );
        assert!(
            <ReferenceMachine as ReferenceStateMachine>::preconditions(&full, &transition),
            "with the gate a no-op, the ref-side precondition (app_started) governs and admits \
             CreateDocument exactly as before"
        );
    }
}
