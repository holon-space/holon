//! `inv-viewmodel-tree-virtual-slots` (SKIPPED — blocked upstream).
//!
//! Currently a no-op. When this invariant becomes unblocked the logic should:
//! - Gate on `ref_.active_render_expr_name(CapRegion::Main) == Some("tree")`
//! - Walk the snapshot looking for collection nodes whose last child has an
//!   entity_id containing `":__virtual:"`.
//! - Warn (not fail) when tree collections have no virtual child slot.
//!
//! Promote when `SutRenderer::widget_tree_snapshot` is wired for the headless
//! path and virtual-slot entity IDs are propagated through `WidgetSnapshot`.

use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvViewmodelTreeVirtualSlots;

impl InvViewmodelTreeVirtualSlots {
    pub const ID: InvariantId = InvariantId("inv-viewmodel-tree-virtual-slots");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvViewmodelTreeVirtualSlots
where
    S: SutRenderer,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &R, _: &S) -> InvariantResult {
        InvariantResult::Skipped("display_tree not wired for the headless path".into())
    }
}
