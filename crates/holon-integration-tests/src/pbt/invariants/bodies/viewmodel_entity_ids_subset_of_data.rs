//! `inv-viewmodel-entity-ids-subset-of-data`.
//!
//! @pbt oracle correspondence
//! @pbt covers phantom-entity — a rendered entity id that is neither a
//!   root query-data row nor a ref-known block
//! @pbt slips-if-removed a render bug mints a widget with a fabricated or
//!   stale entity id (leftover peer row, mis-resolved uri); the UI shows a
//!   ghost row pointing at a non-existent block, nothing else observes it
//!
//! Catches *phantom* entities in the rendered ViewModel tree: every entity ID
//! the user sees must correspond either to a row of the root layout's query
//! data OR to a real block the reference model already knows exists. The layout
//! containers (`default-left-sidebar`, `default-main-panel`,
//! `default-right-sidebar`, …) are real seeded blocks the GPUI app renders for
//! the 3-column layout once layout-query blocks arrive; they come from the
//! layout, not the root's query data, so subtracting the ref-known block set
//! makes the check layout-agnostic instead of hard-coding those IDs.
//!
//! DECLARED display-only UI ids are excluded before the subset check: a
//! `:__virtual:<parent>` CreationPlaceholder ("type here to create") slot and
//! ADR-0015 display-placed occurrences carry ids with no backing block and are
//! intended UI, not phantom data (BugFunnel row 80 — the "backing rows +
//! declared virtual slots" class). Exclusion is via the TYPED `RowOrigin`
//! classifier + `collect_canonical_entity_ids`, never an id-infix sniff.
//!
//! A rendered entity that is *neither* query data nor a real ref block AND is
//! not a declared UI slot is a genuine phantom-entity violation and still
//! `Fail`s.
//!
//! The check reaches its assertion whenever both the rendered tree-id set and
//! the root layout's query-data-id set are non-empty (true at any settled
//! 3-column state); an empty tree/data set this tick is Skipped, not a vacuous
//! Ok. It does NOT gate on `has_root_render_expr()` — that gate is permanently
//! false under the wide run's 3-column layout and blinded this backstop (F1).
//!
//! Status: functional.

use holon_pbt_core::capabilities::RefLayout;
use holon_pbt_core::capabilities::RefViewSelection;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvViewmodelEntityIdsSubsetOfData;

impl InvViewmodelEntityIdsSubsetOfData {
    pub const ID: InvariantId = InvariantId("inv-viewmodel-entity-ids-subset-of-data");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvViewmodelEntityIdsSubsetOfData
where
    R: RefViewSelection + RefLayout,
    S: SutRenderer,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        // NB: NO `has_root_render_expr()` gate. In the wide composed run the
        // default 3-column layout keeps its render sources on the PANELS, not on
        // `root-layout`, so that gate is permanently false there and silently
        // blinds this phantom-id backstop (F1). The check needs only a rendered
        // tree + the root layout's query data — both present in 3-column mode
        // (the root layout watch carries the panel rows).
        let root = sut.widget_tree_snapshot().await;
        // Exclude DECLARED display-only UI ids that legitimately have no backing
        // block (they are intended UI, not phantom data — see BugFunnel row 80,
        // "rendered rows vs backing rows + DECLARED VIRTUAL SLOTS"):
        //  - `collect_canonical_entity_ids()` drops display-placed occurrences (ADR
        //    0015 `props["occurrence"]`).
        //  - a `:__virtual:<parent>` CreationPlaceholder ("type here to create") slot
        //    carries a synthetic id and no backing block; detect it with the TYPED
        //    `RowOrigin` classifier, never an id-infix sniff.
        // A genuinely fabricated NON-virtual id is still flagged — the phantom
        // teeth are intact; only the designed UI slots are excluded.
        let tree_ids: std::collections::BTreeSet<String> = root
            .collect_canonical_entity_ids()
            .into_iter()
            .filter(|id| !holon_frontend::RowOrigin::from_id(id).is_creation_placeholder())
            .collect();
        // `data_ids`/`ref_known` are `EntityUri`; widget-tree `entity_id`s are
        // raw strings — compare on the string surface.
        let data_ids: std::collections::BTreeSet<String> = sut
            .root_data_row_ids()
            .await
            .iter()
            .map(|u| u.as_str().to_string())
            .collect();

        // Not-applicable this snapshot tick: nothing rendered yet, or the root
        // layout's query data hasn't arrived. Disclosed as Skipped so the
        // non-vacuity engagement floor does NOT count it as exercised (F2) —
        // a vacuous `Ok` here would let the floor pass without a real check.
        if tree_ids.is_empty() || data_ids.is_empty() {
            return InvariantResult::Skipped(format!(
                "no rendered entity ids ({}) or no root query data ({}) this snapshot tick",
                tree_ids.len(),
                data_ids.len()
            ));
        }

        // Every block the reference model knows exists (incl. seed/source and
        // the default layout containers), already resolved into SUT ID space by
        // the runner's `with_resolved_doc_uris` view.
        let ref_known: std::collections::BTreeSet<String> = ref_
            .all_block_ids()
            .iter()
            .map(|u| u.as_str().to_string())
            .collect();

        // missing = tree_ids − data_row_ids − ref_known_block_ids
        let missing: Vec<&String> = tree_ids
            .iter()
            .filter(|id| !data_ids.contains(*id) && !ref_known.contains(*id))
            .collect();

        if missing.is_empty() {
            InvariantResult::Ok
        } else {
            InvariantResult::Fail(format!(
                "[inv-viewmodel-entity-ids-subset-of-data] ViewModel has phantom entity IDs that \
                 are neither query data nor known reference blocks. Missing: {missing:?}\nTree \
                 IDs ({}):\n  {tree_ids:?}\nData IDs ({}):\n  {data_ids:?}\nRef-known block IDs \
                 ({}):\n  {ref_known:?}",
                tree_ids.len(),
                data_ids.len(),
                ref_known.len(),
            ))
        }
    }
}
