//! `inv-viewmodel-snapshot`.
//!
//! Asserts the root widget of the headless ViewModel snapshot is not an
//! `error` widget.
//!
//! ## Bound
//!
//! `S: SutRenderer`. `widget_tree_snapshot()` runs the headless
//! `interpret_pure` pipeline; an empty/placeholder/loading snapshot surfaces
//! as kind `"empty"`, which is not `"error"` → `Ok`.

use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult};

pub struct InvViewmodelSnapshot;

impl InvViewmodelSnapshot {
    pub const ID: InvariantId = InvariantId("inv-viewmodel-snapshot");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvViewmodelSnapshot
where
    S: SutRenderer,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &R, sut: &S) -> InvariantResult {
        let root = sut.widget_tree_snapshot().await;

        // Log-only: entity-id count.
        tracing::debug!(
            target: "pbt_invariant",
            "[inv-viewmodel-snapshot] root='{}', {} entity IDs",
            root.kind,
            root.collect_entity_ids().len(),
        );

        if root.kind == "error" {
            InvariantResult::Fail(
                "[inv-viewmodel-snapshot] Root layout rendered as error widget".into(),
            )
        } else {
            InvariantResult::Ok
        }
    }
}
