//! `RefAdvice`.
//!
//! @pbt kind ref
//! @pbt covers advice-weave — delegates to the pure `advice_expectation`
//!   module, an INDEPENDENT Rust re-implementation of the advice matview SQL
//!   (self-join + suppression anti-join) — differential vs the SUT's Turso IVM
//!   path, not a mirror. FIDELITY: v1 only models `AnchorSelector::HasTag`
//!   (other selectors `unreachable!` — fail-loud, not silent).

use holon_api::entity_uri::EntityUri;
use holon_pbt_core::capabilities::AdviceExpectation;
use holon_pbt_core::capabilities::RefAdvice;

use super::super::advice_expectation::active_rule;
use super::super::advice_expectation::expectation_for;
use super::super::advice_expectation::matview_rows_for;
use super::super::reference_state::ReferenceState;

/// Advice-weave read surface (ADR 0021/0022) — delegates to the pure
/// `advice_expectation` module over the resolved block map. Plain reads
/// suffice: the `ReferenceState` behind the caps is already `Resolved`
/// (`with_resolved_doc_uris` → `remapped_doc_uris`), so `block.id` and the
/// `advice_suppressed` edge targets are already in SUT id space; no per-method
/// remapping is needed here. Ids are rendered via `EntityUri::as_str()` — the
/// scheme-form `block_raw.id` carries — so anchor/candidate strings compare
/// directly against the SUT advice matview.
impl RefAdvice for ReferenceState {
    fn advice_expectation(&self, anchor: &str) -> AdviceExpectation {
        let blocks = &self.domain.block_state.blocks;
        let Some(rule) = active_rule(blocks) else {
            return AdviceExpectation::default();
        };
        let anchor_id =
            EntityUri::parse(anchor).expect("advice anchor id must be a valid EntityUri");
        expectation_for(blocks, &rule, &anchor_id)
    }

    fn advice_matview_rows(&self) -> Vec<(String, String, u32)> {
        let blocks = &self.domain.block_state.blocks;
        let Some(rule) = active_rule(blocks) else {
            return Vec::new();
        };
        matview_rows_for(blocks, &rule)
            .into_iter()
            .map(|(a, c, n)| (a.as_str().to_string(), c.as_str().to_string(), n))
            .collect()
    }

    fn advice_matview_name(&self) -> Option<String> {
        active_rule(&self.domain.block_state.blocks).map(|rule| rule.name.matview_name())
    }
}
