//! `inv-mark-bounds-within-content` wired into any `SutBackend`-bearing slice —
//! reads the RAW block snapshot, ignores the reference.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::SutBackend;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::mark_bounds_within_content::InvMarkBoundsWithinContent;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvMarkBoundsWithinContent,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutBackend>()],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
        Attribution::at(Layer::StoreCrdt, file!()),
    ))
}

#[cfg(test)]
mod tests {
    use holon_api::InlineMark;
    use holon_api::MarkSpan;

    use crate::pbt::composed::fixtures::*;

    /// Catch (doc §6 gate): a link/format mark whose span outlives the block
    /// content — the corruption `convert_block_to_page` injects when it writes
    /// a full-span mark computed from stale content, decoupled from a later
    /// content-only trim.
    #[tokio::test]
    async fn memory_slice_catches_out_of_bounds_mark() {
        let mut bad = Block::new_text(uri("local://b"), EntityUri::no_parent(), "abc");
        // content "abc" is 3 scalars; a mark spanning 0..5 outlives it.
        bad.marks = Some(vec![MarkSpan::new(0, 5, InlineMark::Bold)]);
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
                .any(|(id, _)| *id == "inv-mark-bounds-within-content"),
            "the out-of-bounds mark must be caught by inv-mark-bounds-within-content; \
             failures={failures:?}",
        );
    }

    /// Positive: a mark exactly filling the content is in-bounds and passes.
    #[tokio::test]
    async fn memory_slice_passes_in_bounds_mark() {
        let mut ok = Block::new_text(uri("local://b"), EntityUri::no_parent(), "abc");
        ok.marks = Some(vec![MarkSpan::new(0, 3, InlineMark::Bold)]);
        let report = run_selected(
            &composed_invariant_catalog(),
            &fixture_slice(vec![ok]),
            &CapMap::new(),
        )
        .await;

        assert!(
            !report
                .failures()
                .iter()
                .any(|(id, _)| *id == "inv-mark-bounds-within-content"),
            "an in-bounds mark must not trip inv-mark-bounds-within-content",
        );
    }
}
