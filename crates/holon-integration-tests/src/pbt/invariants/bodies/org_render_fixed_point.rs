//! Phase 7 — `inv-org-render-fixed-point`.
//!
//! Body derived from `sut.rs:4380–4412`.
//! Re-renders every tracked org file from SQL and asserts the output equals
//! what's on disk — guards against the echo-suppression loop spin described
//! in the inline comment.
//!
//! 3-subsystem invariant: BlockTree + Renderer + Loro. Uses `SutOrgRender`
//! because `render_documents_to_org` returns both the rendered string and the
//! on-disk string via `snapshot_org_render_pairs`. The capability method
//! returns only the rendered text; the disk text is in the SUT's file system.
//!
//! The current `SutOrgRender::render_documents_to_org` implementation returns
//! only the rendered text (stripping the disk side). The body needs both sides.
//! This is blocked on the capability stub — it returns `Skipped` until
//! `SutOrgRender` is extended with a `snapshot_pairs` method that returns
//! `(disk, rendered)` per file.

use holon_pbt_core::capabilities::SutOrgRender;
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult, RunMode};

pub struct InvOrgRenderFixedPoint;

impl InvOrgRenderFixedPoint {
    pub const ID: InvariantId = InvariantId("inv-org-render-fixed-point");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvOrgRenderFixedPoint
where
    S: SutOrgRender,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    fn mode(&self) -> RunMode {
        RunMode::Strict
    }

    async fn check(&self, _: &R, _: &S) -> InvariantResult {
        // Blocked on Phase 7 plumbing: SutOrgRender::render_documents_to_org
        // currently returns only the rendered text, not the (disk, rendered)
        // pair needed for the fixed-point comparison. The inline assertion
        // in check_invariants_async uses snapshot_org_render_pairs which
        // returns both sides. Extend the capability with a snapshot_pairs
        // method and wire then.
        InvariantResult::Skipped(
            "blocked on Phase 7 plumbing: SutOrgRender needs snapshot_pairs() \
             returning (disk, rendered) per file"
                .to_string(),
        )
    }
}
