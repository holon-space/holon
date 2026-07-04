//! `inv-mark-bounds-within-content` — every inline `MarkSpan` must lie within
//! its block's content.
//!
//! @pbt oracle internal-consistency — for every block, each mark's `end` is
//!   `<= content.chars().count()`, over the SUT write-side RAW snapshot (no
//! ref) @pbt covers mark-bounds — a link/format mark whose span outlives the
//! content   it decorates (producer wrote a full-span mark from stale content,
//! or a   content-only trim later shortened the text without adjusting marks)
//! @pbt slips-if-removed a decoupled marks-only write leaves a mark span longer
//!   than the block content; every render then aborts in
//!   `scalar_range_to_bytes` ("N..M exceeds text length K chars") and the page
//!   is permanently un-openable, with no differential oracle flagging the row
//!
//! Domain rule: a `MarkSpan`'s `[start, end)` addresses Unicode-scalar offsets
//! into `Block.content`, so `end <= content.chars().count()` must hold. A mark
//! that outlives its content is a projection corruption that aborts the GPUI
//! renderer on EVERY paint.
//!
//! Checked against the SUT's convergent write-side truth
//! (`block_raw_snapshot`) — the RAW persisted `(content, marks)` pair, NOT the
//! org-roundtrip-normalized reference model. The reference normalizer
//! (`normalize_content_for_org_roundtrip`) re-derives marks from
//! already-trimmed content, so it can never carry an out-of-bounds mark;
//! checking against it would mask exactly this bug. No ref-side comparison
//! needed.

use holon_pbt_core::capabilities::SutBackend;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvMarkBoundsWithinContent;

impl InvMarkBoundsWithinContent {
    pub const ID: InvariantId = InvariantId("inv-mark-bounds-within-content");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvMarkBoundsWithinContent
where
    S: SutBackend,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &R, sut: &S) -> InvariantResult {
        for block in sut.block_raw_snapshot().await {
            let Some(marks) = &block.marks else {
                continue;
            };
            let len = block.content.chars().count();
            for m in marks {
                if m.end > len {
                    return InvariantResult::Fail(format!(
                        "block {}: mark span {}..{} exceeds content length {len} \
                         chars (content={:?}) — a mark outliving its content \
                         aborts every render in scalar_range_to_bytes",
                        block.id, m.start, m.end, block.content
                    ));
                }
            }
        }
        InvariantResult::Ok
    }
}
