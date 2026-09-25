//! `inv-loro-ui-rows-match-ref` — the row a no-Turso (`LoroUiWatcher`) session
//! renders for a block carries what profile conditions read.
//!
//! @pbt oracle correspondence
//! @pbt covers loro-ui-row-edge-fields — the Loro render path's row vs the
//!   reference: resolved `is_page_row` for every ref-known block, and the edge
//!   fields (tags/requires/advice_suppressed/contributes_to) for non-seed
//!   blocks
//! @pbt slips-if-removed the Loro UI row drops an edge field, so a
//!   tag-driven profile variant (the Page variant, `decision`) never matches
//!   in a no-Turso session while the store itself is correct
//!
//! The row is read back through the canonical block-row parser, so a missing
//! or wrongly shaped column fails with the parser's own message.

use std::collections::BTreeMap;

use holon_api::Block;
use holon_api::Value;
use holon_pbt_core::capabilities::RefBackend;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::SutLoroUiRows;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvLoroUiRowsMatchRef;

impl InvLoroUiRowsMatchRef {
    pub const ID: InvariantId = InvariantId("inv-loro-ui-rows-match-ref");
}

fn edge_field_diffs(row: &Block, ref_block: &Block) -> Vec<String> {
    let mut diffs = Vec::new();
    if row.tags != ref_block.tags {
        diffs.push(format!("tags {:?} != ref {:?}", row.tags, ref_block.tags));
    }
    if row.requires != ref_block.requires {
        diffs.push(format!(
            "requires {:?} != ref {:?}",
            row.requires, ref_block.requires
        ));
    }
    if row.advice_suppressed != ref_block.advice_suppressed {
        diffs.push(format!(
            "advice_suppressed {:?} != ref {:?}",
            row.advice_suppressed, ref_block.advice_suppressed
        ));
    }
    if row.contributes_to != ref_block.contributes_to {
        diffs.push(format!(
            "contributes_to {:?} != ref {:?}",
            row.contributes_to, ref_block.contributes_to
        ));
    }
    diffs
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvLoroUiRowsMatchRef
where
    R: RefBlockTree + RefBackend,
    S: SutLoroUiRows,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let non_seed: BTreeMap<_, _> = ref_
            .non_seed_blocks()
            .into_iter()
            .map(|b| (b.id.clone(), b))
            .collect();
        let mut compared = 0usize;
        let mut violations: Vec<String> = Vec::new();

        for row in sut.loro_ui_rows().await {
            // Scaffold blocks the model does not track have no expectation.
            if ref_.block_content(&row.id).is_none() {
                continue;
            }
            compared += 1;
            let expected_page = ref_.is_page_block(&row.id);
            if row.is_page_row != Value::Boolean(expected_page) {
                violations.push(format!(
                    "{}: is_page_row resolved {:?}, ref page={expected_page}",
                    row.id, row.is_page_row
                ));
            }
            match (&row.parsed, non_seed.get(&row.id)) {
                (Err(e), _) => violations.push(format!("{}: row does not parse: {e}", row.id)),
                (Ok(parsed), Some(ref_block)) => {
                    for diff in edge_field_diffs(parsed, ref_block) {
                        violations.push(format!("{}: {diff}", row.id));
                    }
                }
                (Ok(_), None) => {}
            }
        }

        if compared == 0 {
            return InvariantResult::Skipped("no Loro UI row is a ref-known block".to_string());
        }
        if violations.is_empty() {
            return InvariantResult::Ok;
        }
        InvariantResult::Fail(format!(
            "[inv-loro-ui-rows-match-ref] {} violations over {compared} rows: {:?}",
            violations.len(),
            violations.iter().take(10).collect::<Vec<_>>(),
        ))
    }
}
