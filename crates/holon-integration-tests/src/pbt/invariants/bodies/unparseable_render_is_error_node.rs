//! `inv-unparseable-render-is-error-node` — a block whose render source does
//! not parse draws one `error` widget naming the block and the parse error.
//!
//! @pbt oracle correspondence
//! @pbt covers unparseable-render-source-is-visible — a render source that
//!   does not parse draws an error naming the block and the parse error, never
//!   a silent `table()`
//! @pbt slips-if-removed a typo in a render source silently becomes a view the
//!   author never wrote, or a bare block, and nothing says why
//!
//! Shares the oracle of `inv-render-only-block-draws-its-render`.

use holon_pbt_core::capabilities::RefRenderSources;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

use super::render_only_block_draws_its_render::check_kind;

pub struct InvUnparseableRenderIsErrorNode;

impl InvUnparseableRenderIsErrorNode {
    pub const ID: InvariantId = InvariantId("inv-unparseable-render-is-error-node");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvUnparseableRenderIsErrorNode
where
    R: RefRenderSources,
    S: SutRenderer,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        check_kind(ref_, sut, Self::ID.0, true).await
    }
}
