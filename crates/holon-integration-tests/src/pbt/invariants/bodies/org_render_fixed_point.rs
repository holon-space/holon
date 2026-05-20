//! `inv-org-render-fixed-point`.
//!
//! Re-renders every tracked org file from the current SQL state and asserts
//! the output equals the bytes already on disk — guards against the
//! echo-suppression loop spin where `render(SQL) != disk` would force
//! `re_render_all_tracked` to write a different file on the next tick,
//! firing FSEvent, reprocessing via `on_file_changed`, and looping.
//!
//! Catches the May-2026 shared-tree mount loop where a property-drawer key
//! round-trip differed between ingestion and render. The `inv-blocks-match-ref`
//! family does NOT cover this — the parser is forgiving of property
//! ordering / sibling reordering driven by `sort_key` drift, and never
//! sees disagreement at all when the bug only manifests in a file shape
//! the reference model never generates (e.g. `:share-role: mount`).
//!
//! Capability: `SutOrgRender::snapshot_org_render_pairs` returns
//! `(path, disk, rendered)` triples.

use holon_pbt_core::capabilities::SutOrgRender;
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult};

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

    async fn check(&self, _: &R, sut: &S) -> InvariantResult {
        for (path, disk, rendered) in sut.snapshot_org_render_pairs().await {
            if disk != rendered {
                return InvariantResult::Fail(format!(
                    "[inv-org-render-fixed-point] {path} would be rewritten by the \
                     next re_render_all_tracked → echo-suppression loop risk.\n\
                     --- disk ({} bytes) ---\n{disk}\n--- rendered from SQL ({} bytes) ---\n{rendered}",
                    disk.len(),
                    rendered.len(),
                ));
            }
        }
        InvariantResult::Ok
    }
}
