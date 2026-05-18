//! Phase 7 — `inv-viewmodel-tree-virtual-slots` (SKIPPED — blocked upstream).
//!
//! Inline body originally at `sut.rs:5447–5508` (sub-check 10j).
//!
//! The inline body is explicitly a no-op: it defines a walk function, initialises
//! `found = 0; without = 0`, immediately discards them with `let _ = (found, without)`,
//! and emits "SKIPPED — display_tree not wired in this scope". The walk is never
//! invoked because `display_tree` is not accessible in the inline scope.
//!
//! When this invariant becomes unblocked the logic should:
//! - Gate on `ref_.active_render_expr_name(CapRegion::Main) == Some("tree")`
//! - Walk the snapshot looking for collection nodes whose last child has an
//!   entity_id containing `":__virtual:"`.
//! - Warn (not fail) when tree collections have no virtual child slot.
//!
//! Status: honestly SKIPPED — same `display_tree` blocker as the inline body.
//! Promote when `SutRenderer::widget_tree_snapshot` is wired for the headless
//! path and virtual-slot entity IDs are propagated through `WidgetSnapshot`.

use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult, RunMode};

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

    fn mode(&self) -> RunMode {
        RunMode::Warn
    }

    async fn check(&self, _: &R, _: &S) -> InvariantResult {
        InvariantResult::Skipped(
            "display_tree not wired — same blocker as inline sut.rs:5486".into(),
        )
    }
}
