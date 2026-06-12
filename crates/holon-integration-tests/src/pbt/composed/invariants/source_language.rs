//! `inv-source-language-iff-source` wired into the memory slice — `SutBackend`
//! only, ignores the reference.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::SutBackend;
use holon_pbt_core::composition::{BridgedInvariant, CapId, CapInvariant, Needs};

use crate::pbt::invariants::bodies::source_language_iff_source::InvSourceLanguageIffSource;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvSourceLanguageIffSource,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutBackend>()],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
    ))
}

#[cfg(test)]
mod tests {
    use crate::pbt::composed::fixtures::*;

    /// Catch (doc §6 gate): a Source block that lost its `source_language` — a
    /// projection corruption the API forbids at write time, injected via the
    /// fixture.
    #[tokio::test]
    async fn memory_slice_catches_source_without_language() {
        let bad = Block {
            content_type: ContentType::Source,
            source_language: None,
            ..Block::new_text(uri("local://s"), EntityUri::no_parent(), "code")
        };
        let report = run_selected(
            &composed_invariant_catalog(),
            &fixture_slice(vec![bad]),
            &CapMap::new(),
        )
        .await;

        let failures = report.failures();
        assert!(
            failures
                .iter()
                .any(|(id, _)| *id == "inv-source-language-iff-source"),
            "the corruption must be caught by inv-source-language-iff-source; failures={failures:?}",
        );
    }
}
