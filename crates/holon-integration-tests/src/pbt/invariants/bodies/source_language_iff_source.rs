//! `inv-source-language-iff-source` — ADR-0004 domain invariant.
//!
//! Domain rule: a block carries a `source_language` **iff** its
//! `content_type` is [`ContentType::Source`]. Text and Image blocks carry
//! `None`; Source blocks carry `Some(lang)`. This is a property of the domain
//! model that every adapter projection must preserve — a Source row that lost
//! its language, or a Text row that grew one, is a projection corruption.
//!
//! Self-consistent within the SUT's convergent write-side truth
//! (`block_raw_snapshot`), read after the shared settle, so there is no
//! CDC-lag window to tolerate. No ref-side comparison needed.

use holon_api::ContentType;
use holon_pbt_core::capabilities::SutBackend;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvSourceLanguageIffSource;

impl InvSourceLanguageIffSource {
    pub const ID: InvariantId = InvariantId("inv-source-language-iff-source");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvSourceLanguageIffSource
where
    S: SutBackend,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &R, sut: &S) -> InvariantResult {
        for block in sut.block_raw_snapshot().await {
            let is_source = block.content_type == ContentType::Source;
            let has_lang = block.source_language.is_some();
            if is_source != has_lang {
                return InvariantResult::Fail(format!(
                    "[inv-source-language-iff-source] block {} violates the domain rule: \
                     content_type={:?} but source_language={:?} (source_language must be Some iff \
                     content_type == Source)",
                    block.id, block.content_type, block.source_language,
                ));
            }
        }
        InvariantResult::Ok
    }
}
