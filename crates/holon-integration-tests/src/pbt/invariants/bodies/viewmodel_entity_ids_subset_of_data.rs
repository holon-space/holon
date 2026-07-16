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
//! A rendered entity that is *neither* query data nor a real ref block is a
//! genuine phantom-entity violation and still `Fail`s.
//!
//! The check is gated on:
//! - The ref model tracking a render expression (i.e. `has_root_render_expr()`)
//! - Both the tree-id set and the data-id set being non-empty
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
        if !ref_.has_root_render_expr() {
            return InvariantResult::Ok;
        }

        let root = sut.widget_tree_snapshot().await;
        let tree_ids = root.collect_entity_ids();
        // `data_ids`/`ref_known` are `EntityUri`; widget-tree `entity_id`s are
        // raw strings — compare on the string surface.
        let data_ids: std::collections::BTreeSet<String> = sut
            .root_data_row_ids()
            .await
            .iter()
            .map(|u| u.as_str().to_string())
            .collect();

        if tree_ids.is_empty() || data_ids.is_empty() {
            return InvariantResult::Ok;
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
