//! `inv-filter-spec-resolves` — FLT-1.b boundary invariant.
//!
//! @pbt oracle internal-consistency — every `holon_filter` source block in the
//!   SUT write-side snapshot resolves to a typed `FilterSpec` (no ref)
//! @pbt covers filter-source-projection — a filter predicate body that survives
//!   org → Loro → Turso projection still parses at the FLT-1.b boundary
//! @pbt slips-if-removed a projection mangles a `holon_filter` body (newline
//!   loss, header-arg drop) and no oracle notices the predicate no longer
//! parses
//!
//! Domain rule (FLT-1.b): a `holon_filter` source block's body is a predicate
//! in the render-DSL predicate grammar, and its containing headline plus the
//! block's `polarity`/`subtree` header args resolve to a [`FilterSpec`]. This
//! is the parse boundary — a Source block typed `holon_filter` whose body no
//! longer parses is a projection corruption, so we assert resolution over the
//! *real* projected block set, not a hand-built one.
//!
//! Self-consistent within the SUT's convergent write-side truth
//! (`block_raw_snapshot`), read after the shared settle: `inv-no-orphan-blocks`
//! guarantees each filter block's headline is present, so a resolution failure
//! is a real malformed-body / lost-header regression, not a settle race. No
//! ref-side comparison needed.

use holon_api::filter::FilterSpec;
use holon_api::types::SourceLanguage;
use holon_pbt_core::capabilities::SutBackend;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvFilterSpecResolves;

impl InvFilterSpecResolves {
    pub const ID: InvariantId = InvariantId("inv-filter-spec-resolves");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvFilterSpecResolves
where
    S: SutBackend,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &R, sut: &S) -> InvariantResult {
        let blocks = sut.block_raw_snapshot().await;
        for src in blocks
            .iter()
            .filter(|b| b.source_language == Some(SourceLanguage::HolonFilter))
        {
            if let Err(e) = FilterSpec::parse(&src.parent_id, &blocks) {
                return InvariantResult::Fail(format!(
                    "holon_filter source {} did not resolve to a FilterSpec: {e:#}",
                    src.id
                ));
            }
        }
        InvariantResult::Ok
    }
}
