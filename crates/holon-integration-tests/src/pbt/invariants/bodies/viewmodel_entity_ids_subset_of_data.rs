//! Phase 7 — `inv-viewmodel-entity-ids-subset-of-data` (FUNCTIONAL).
//!
//! Inline body originally at `sut.rs:5064–5114` (sub-check 10e).
//!
//! Asserts that every entity ID present in the widget tree snapshot is also
//! present in the root layout's data rows. The check is gated on:
//! - The ref model tracking a render expression (i.e. `has_root_render_expr()`)
//! - Both the tree-id set and the data-id set being non-empty
//!
//! When profile-variant YAML hard-codes `live_block("block:default-*")` IDs
//! outside the query data, `has_root_render_expr()` returns false and the
//! assertion is intentionally skipped.
//!
//! Status: functional.

use holon_pbt_core::capabilities::{RefRender, SutRenderer};
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult, RunMode};

pub struct InvViewmodelEntityIdsSubsetOfData;

impl InvViewmodelEntityIdsSubsetOfData {
    pub const ID: InvariantId = InvariantId("inv-viewmodel-entity-ids-subset-of-data");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvViewmodelEntityIdsSubsetOfData
where
    R: RefRender,
    S: SutRenderer,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    fn mode(&self) -> RunMode {
        RunMode::Strict
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        if !ref_.has_root_render_expr() {
            return InvariantResult::Ok;
        }

        let root = sut.widget_tree_snapshot().await;
        let tree_ids = root.collect_entity_ids();
        let data_ids = sut.root_data_row_ids().await;

        if tree_ids.is_empty() || data_ids.is_empty() {
            return InvariantResult::Ok;
        }

        let missing: Vec<&String> = tree_ids
            .iter()
            .filter(|id| !data_ids.contains(*id))
            .collect();

        if missing.is_empty() {
            InvariantResult::Ok
        } else {
            InvariantResult::Fail(format!(
                "[inv-viewmodel-entity-ids-subset-of-data] ViewModel has entity IDs not in \
                 query data. Missing: {missing:?}\n\
                 Tree IDs ({}):\n  {tree_ids:?}\n\
                 Data IDs ({}):\n  {data_ids:?}",
                tree_ids.len(),
                data_ids.len(),
            ))
        }
    }
}
