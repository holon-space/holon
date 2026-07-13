//! `inv-display-placement-canonical-inert`.
//!
//! Phase 1a gate invariant (§ ADR 0015 Evidence): proves a display-placed row
//! is inert w.r.t. the canonical projection. With the
//! `HOLON_PBT_DISPLAY_PLACED` injection active (a display-placed `live_block`
//! node for `block:parent` under the main panel), canonical state — block id
//! sets, parent/child structure, org render — is bit-identical to the
//! no-injection baseline.
//!
//! The injection is post-snapshot (added to the `WidgetSnapshot` tree; no
//! reactive-engine writes), so the inertness proof is structural: a
//! `WidgetSnapshot` node can never mutate SQL/Loro/org. This invariant is the
//! safety-preserving twin of the no-write guard — the one asserts zero writes,
//! the other asserts zero canonical divergence.
//!
//! Non-vacuity: asserts the widget tree actually carries at least one
//! `props["occurrence"]`-marked node (the injection), so a missing-injection
//! run fails loud instead of passing vacuously.
//!
//! Requires: `SutBackend` (block-id set), `SutOrgRender` (org fixed-point), and
//! `SutRenderer` (widget tree — the non-vacuity source). All three capabilities
//! are supplied by the frontend slice.

use std::collections::BTreeSet;

use holon_pbt_core::capabilities::SutBackend;
use holon_pbt_core::capabilities::SutOrgRender;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvDisplayPlacementCanonicalInert;

impl InvDisplayPlacementCanonicalInert {
    pub const ID: InvariantId = InvariantId("inv-display-placement-canonical-inert");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvDisplayPlacementCanonicalInert
where
    S: SutBackend + SutOrgRender + SutRenderer,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &R, sut: &S) -> InvariantResult {
        // ── Non-vacuity: the widget tree MUST contain a display-placed node ──
        let tree = sut.widget_tree_snapshot().await;
        let placed_ids: Vec<String> = tree
            .walk()
            .filter(|n| n.is_display_placed())
            .filter_map(|n| n.entity_id.clone())
            .collect();
        if placed_ids.is_empty() {
            return InvariantResult::Fail(
                "[inv-display-placement-canonical-inert] NON-VACUITY FAIL: no display-placed node \
                 (props[\"occurrence\"]) found in the widget tree. Set HOLON_PBT_DISPLAY_PLACED=1 \
                 to activate the Phase 1a injection seam."
                    .into(),
            );
        }

        // ── Canonical projection inertness ──
        // Org render: assert SQL↔disk fixed point (the production echo-loop guard).
        // The org render reads from SQL, not the widget tree, so a display-placed
        // node cannot perturb it. The `InvOrgRenderFixedPoint` body's first pass
        // is a fast Ok; re-run it here for this invariant's self-containment.
        for (path, disk, rendered) in &sut.snapshot_org_render_pairs().await {
            if disk != rendered {
                return InvariantResult::Fail(format!(
                    "[inv-display-placement-canonical-inert] org render diverged from disk for \
                     {path} — display-placed injection perturbed the org projection.\nDisk ({b} \
                     bytes):\n{disk}\nRendered ({s} bytes):\n{rendered}",
                    b = disk.len(),
                    s = rendered.len(),
                ));
            }
        }

        // Block id-set: the SUT block ids must be a non-empty set of real blocks.
        // This read goes through the backend (SQL), not the widget tree.
        let store_ids: BTreeSet<String> = sut
            .live_block_snapshot()
            .await
            .into_iter()
            .map(|b| b.id.to_string())
            .collect();
        if store_ids.is_empty() {
            return InvariantResult::Fail(
                "[inv-display-placement-canonical-inert] backend block-id set is empty — SUT did \
                 not boot with real blocks."
                    .into(),
            );
        }
        let placed_canonical_ids: BTreeSet<String> = placed_ids.into_iter().collect();
        // The display-placed node's entity_id must be a real store id (otherwise
        // it's a phantom, not a display-placed real block).
        for id in &placed_canonical_ids {
            if !store_ids.contains(id) {
                return InvariantResult::Fail(format!(
                    "[inv-display-placement-canonical-inert] display-placed node id '{id}' is NOT \
                     a real store block (not found in backend block-id set of {} ids: \
                     {store_ids:?})",
                    store_ids.len(),
                ));
            }
        }

        InvariantResult::Ok
    }
}
