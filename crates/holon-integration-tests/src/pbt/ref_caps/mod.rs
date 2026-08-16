//! Phase 2 — blanket impls of `holon_pbt_core::capabilities::*` on
//! [`ReferenceState`].
//!
//! @pbt kind ref
//! @pbt covers ref-capmap-wiring — `CapProvider::register` exposes the single
//!   `ReferenceState` as the ref `CapMap` for `run_selected`. Every
//! `caps.insert`   is registered UNCONDITIONALLY; selection is `SUT ∧ ref`
//! (`Needs`), so a ref   cap with no matching SUT cap is inert (deselects), NOT
//! a vacuity feeder.   FIDELITY: over-registering a ref cap could newly SELECT
//! a catalog invariant   (the "catalog scope creep" risk) — each `caps.insert`
//! comment states which   SUT slice gates it.
//!
//! Translates between the capability traits' stringly-typed `EntityUri`
//! surface and `ReferenceState`'s `EntityUri`-based internals. Zero
//! behaviour change: every method delegates to an existing
//! `ReferenceState` field or method.
//!
//! When the wide PBT's transitions migrate to capability bounds (Phase 3),
//! they pick these impls up automatically — no code in the transition
//! changes beyond the trait-bound `where` clauses.
//!
//! ## Boundary translation: `EntityUri` ↔ `EntityUri`
//!
//! - Read methods returning ids: produce `String` via
//!   `EntityUri::as_str().to_string()`.
//! - Read methods taking ids: parse via `EntityUri::from_str` and propagate
//!   `None` on failure (a malformed id can't match anything in the block tree).
//! - Write methods taking ids: parse + `.expect()` — wide PBT only ever sees
//!   valid `EntityUri`s, so a parse failure is a programmer error.
//!
//! Split (RefStateSplit Increment 2, plan
//! `docs/Plans/RefStateSplit-2026-07-12.md` §1/§3) into one file per trait
//! cluster; this module keeps the id-translation helpers, the `CapProvider`
//! impl, and `reference_state_ref_caps()`.

use std::collections::BTreeSet;
use std::sync::Arc;

use holon_api::Region;
use holon_api::entity_uri::EntityUri;
use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::RefAdvice;
use holon_pbt_core::capabilities::RefBackend;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefClock;
use holon_pbt_core::capabilities::RefEditorMirror;
use holon_pbt_core::capabilities::RefFocus;
use holon_pbt_core::capabilities::RefGlobalFocus;
use holon_pbt_core::capabilities::RefHistoryExpectation;
use holon_pbt_core::capabilities::RefLayout;
use holon_pbt_core::capabilities::RefTaskState;
use holon_pbt_core::capabilities::RefToggle;
use holon_pbt_core::capabilities::RefViewSelection;
use holon_pbt_core::capabilities::RefWatch;

use super::reference_state::ReferenceState;
use super::reference_state::Resolved;

mod advice;
mod audience;
mod blocks;
mod boot;
mod clock;
mod docs;
mod editor;
mod entity_schemes;
mod focus;
mod layout;
mod misc;
mod nav;
mod peers;
mod shared_view;
mod toggle;
mod watches;

// ─── Helpers ──────────────────────────────────────────────────────────
//
// `CapBlockId` is now `holon_api::EntityUri` (the capability surface uses
// the real domain type), so the former `EntityUri ↔ String` boundary
// translation collapses to identity. These thin wrappers are kept so the
// per-method call sites below don't all have to change shape.

pub(super) fn cap_id(uri: &EntityUri) -> EntityUri {
    uri.clone()
}

pub(super) fn cap_id_set(uris: BTreeSet<EntityUri>) -> BTreeSet<EntityUri> {
    uris
}

/// Identity: a `CapBlockId` already *is* an `EntityUri`.
pub(super) fn parse_id(id: &EntityUri) -> Option<EntityUri> {
    Some(id.clone())
}

/// Identity: a `CapBlockId` already *is* an `EntityUri`.
pub(super) fn parse_id_must(id: &EntityUri) -> EntityUri {
    id.clone()
}

pub(super) fn from_cap_region(r: CapRegion) -> Region {
    // CapRegion::Sidebar maps to LeftSidebar (the primary sidebar in wide PBT).
    // Pure slice impls use Single (no region distinction).
    match r {
        CapRegion::Main | CapRegion::Single => Region::Main,
        CapRegion::Sidebar => Region::LeftSidebar,
    }
}

// ─── CapProvider — the keystone (ADR 0007 / PbtCompositionDesign §6) ───
//
// Lets a live `ReferenceState` BE the ref `CapMap` that `run_selected`
// consumes, so any slice (and the generic subsystem-shrink PBT) can read the
// single real oracle instead of a bespoke parallel ref model.
//
// We register the read caps the composed catalog consumes today
// (`RefBackend` + `RefBlockTree` for the block invariants, `RefEditorMirror`
// for the editor invariants, `RefLayout` for the windowed
// `inv-frontend-bounds-rendered`). Further widening of the read surface
// (focus/render/…) stays deferred (it could newly *select* catalog invariants —
// the "catalog scope creep" risk in the plan) until each is wired.
//
// `RefEditorMirror` is registered unconditionally: selection is an AND over
// the SUT and ref cap sets (`Needs::selected_against`), so the editor
// invariants still deselect for a config whose SUT has no editor — the ref
// carrying the cap is harmless.
impl holon_pbt_core::composition::CapProvider for ReferenceState {
    fn register(self: Arc<Self>, caps: &mut holon_pbt_core::composition::CapMap) {
        caps.insert(self.clone() as Arc<dyn RefBackend>);
        caps.insert(self.clone() as Arc<dyn RefBlockTree>);
        caps.insert(self.clone() as Arc<dyn RefEditorMirror>);
        // `RefLayout` carries the layout-block / document metadata the windowed
        // `inv-frontend-bounds-rendered` reads (`has_user_documents`,
        // `region_entity_focused`). Registering it unconditionally is harmless to
        // existing slices: selection ANDs the SUT and ref cap sets, and only the
        // windowed slice supplies the matching `SutLayout + SutViewSelection`.
        caps.insert(self.clone() as Arc<dyn RefLayout>);
        // `RefWatch` carries the active-watch query set + expected rows the B5
        // watch invariants read (E1 SutWatch relocation). Harmless to existing
        // slices: only the frontend slice supplies the matching `SutWatch`.
        caps.insert(self.clone() as Arc<dyn RefWatch>);
        // `RefFocus` carries the per-region navigation focus + expected focus roots
        // the `inv-navigation-focus` / `inv-focus-roots` invariants read (SutHandle
        // decomposition: NavigateFocus onto SutFocusWrite). Harmless to existing
        // slices: selection ANDs the SUT and ref cap sets, and only a slice that
        // also supplies `SutSqlProjection` (+`SutBackend`) selects the focus
        // invariants — and only the navigation slice drives real focus data.
        caps.insert(self.clone() as Arc<dyn RefFocus>);
        // `RefViewSelection` carries the active-view / render-expr metadata the
        // ViewModel invariants read (`inv-view-selection`, the C3 renderer
        // cluster). The logic already lives on `ReferenceState`; this just
        // exposes it on the ref `CapMap`. Harmless to existing slices:
        // selection ANDs the SUT and ref cap sets, and only a slice supplying
        // `SutViewSelection`/`SutRenderer` selects it.
        caps.insert(self.clone() as Arc<dyn RefViewSelection>);
        // `RefTaskState` + `RefGlobalFocus` carry the task-state / global-focus
        // metadata the `value_fn_provider_*` ViewModel invariants read (C3 batch 2).
        // Logic already on `ReferenceState`; harmless to existing slices (selection
        // ANDs SUT∧ref cap sets — only a `SutViewSelection` slice selects them).
        caps.insert(self.clone() as Arc<dyn RefTaskState>);
        // `RefToggle` carries the expand/collapse (`is_expanded`) model the
        // `inv-embedded-page-collapsed-lazy` invariant reads (Phase 4 ExpandToggle
        // keystone). Harmless to existing slices: selection ANDs SUT∧ref cap sets,
        // and only the frontend slice supplying `SutRenderer` selects it.
        caps.insert(self.clone() as Arc<dyn RefToggle>);
        // `RefNavHistory` carries the drawer open/closed model
        // `inv-drawer-open-matches-ref` reads. Harmless to existing slices:
        // selection ANDs SUT∧ref cap sets, and only a slice supplying
        // `SutRenderer` selects that invariant.
        caps.insert(self.clone() as Arc<dyn holon_pbt_core::capabilities::RefNavHistory>);
        // `RefJournalFeed` scopes `inv-journal-feed-viewport-lazy` to the feed's
        // own day pages. Harmless to existing slices: only a slice supplying
        // windowed `SutLayout` geometry selects that invariant.
        caps.insert(self.clone() as Arc<dyn holon_pbt_core::capabilities::RefJournalFeed>);
        // `RefAdvice` carries the advice-weave expectation (ADR 0021/0022) the
        // `advice rows woven` keystone invariant reads. Harmless to existing
        // slices: selection ANDs SUT∧ref cap sets, and only a slice supplying the
        // matching SUT advice-matview cap selects it.
        caps.insert(self.clone() as Arc<dyn RefAdvice>);
        // `RefClock` carries the calendar-day model + predicted journal count the
        // `AdvanceDay` journal-count invariant reads (ADR 0024 §6). Harmless to
        // existing slices: selection ANDs SUT∧ref cap sets, and only a slice
        // supplying the matching `SutJournalCount` selects it.
        caps.insert(self.clone() as Arc<dyn RefClock>);
        // `RefHistoryExpectation` carries the C2 provenance-oracle expectation
        // (ever-created id anchor + UI-driven op-group floor) the `history_*`
        // correspondences read. Harmless to existing slices: selection ANDs
        // SUT∧ref cap sets, and only a Turso arm supplying the matching
        // `SutHistory` selects the history invariants.
        caps.insert(self.clone() as Arc<dyn RefHistoryExpectation>);
        // `RefUndoRedoBurned` carries the ids a `Redo` re-minted away from
        // (harness-stamped). Ref-only gate for `inv-undo-redo-reference-heal`;
        // empty until a round trip completes, so the invariant is vacuously
        // green on every other draw.
        caps.insert(self.clone() as Arc<dyn holon_pbt_core::capabilities::RefUndoRedoBurned>);
        caps.insert(self.clone() as Arc<dyn RefGlobalFocus>);
        // `RefAudience` carries the ADR 0028 C2/H3 sharing overlay (per-block
        // policy audience + per-container effective audience + epoch) the
        // `inv-audience-never-over-approximates` keystone oracle reads. Ref-only
        // (no SUT twin yet — no share machinery in prod), so it selects on every
        // composed draw like `inv-no-page-under-non-page`; `shared_block_ids()`
        // is empty on a ref modeling no crossings, so the oracle is vacuously
        // green and adds no false RED to the keystone.
        caps.insert(self.clone() as Arc<dyn holon_pbt_core::capabilities::RefAudience>);
        // `RefSharedView` carries the true-sharing whole-vault share + the
        // owner→receiver round count the two-instance convergence/boundary
        // oracles read. Ref-only; both invariants ALSO need a receiver SUT cap,
        // so they deselect on every single-instance slice.
        caps.insert(self as Arc<dyn holon_pbt_core::capabilities::RefSharedView>);
    }
}

/// Build the ref `CapMap` from a [`Resolved`] [`ReferenceState`] — the keystone
/// helper the slices and the generic PBT use in place of
/// `ref_map`/`full_ref_map`.
///
/// Requires the [`Resolved`] witness: the comparison caps built here compare
/// ids directly against the SUT, so the ref's ids must already live in the
/// SUT's id space (see [`ReferenceState::with_resolved_doc_uris`] /
/// [`Resolved::identity`]). An unresolved ref is a compile error here.
pub fn reference_state_ref_caps(
    state: Resolved<Arc<ReferenceState>>,
) -> holon_pbt_core::composition::CapMap {
    let mut caps = holon_pbt_core::composition::CapMap::new();
    holon_pbt_core::composition::CapProvider::register(state.into_inner(), &mut caps);
    caps
}
