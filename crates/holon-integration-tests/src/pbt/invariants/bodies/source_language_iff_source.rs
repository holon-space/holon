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
use holon_oracles::checks::{SourceLanguageRow, find_source_language_violations};
use holon_pbt_core::capabilities::SutBackend;
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult};

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
        // Check body lives in `holon_oracles::checks` — shared with the live
        // debug-build oracle, one implementation, no drift.
        let rows: Vec<SourceLanguageRow> = sut
            .block_raw_snapshot()
            .await
            .into_iter()
            .map(|b| SourceLanguageRow {
                id: b.id,
                is_source: b.content_type == ContentType::Source,
                source_language: b.source_language.map(|l| l.to_string()),
            })
            .collect();
        match find_source_language_violations(&rows).into_iter().next() {
            Some(message) => InvariantResult::Fail(message),
            None => InvariantResult::Ok,
        }
    }
}
