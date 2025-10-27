//! `inv-viewmodel-decompiled-rows-match-query`.
//!
//! Gated by `is_properly_setup()` only (NOT `!nav_only`).
//!
//! Asserts (Strict) that the per-row rendered `content` strings EQUAL the
//! query `data_rows`' `content` (in order), filtered to the root render
//! expr's `visible_columns`. Both sides derive from the same watch snapshot
//! through `interpret_pure` (no viewport culling headless), so a rendered
//! list that is merely an ordered subset means the interpreter DROPPED rows
//! — the previous subset check let that pass.
//!
//! The SUT-internal extraction (interpret_pure → display tree,
//! `extract_rendered_rows`, data_rows `content` filtered to visible columns)
//! lives behind `SutRenderer::root_content_comparison`, which returns the two
//! `content` vectors already filtered. The body only owns the ordered-subset
//! comparison (`is_ordered_subset`) and the failure diagnostic. The
//! visible-column set is read ref-side via
//! `RefViewSelection::root_visible_columns()`.
//!
//! The inline's diagnostic additionally printed `render_expr.to_rhai()` and
//! `display_tree.pretty_print(0)` — both SUT-internal frontend artifacts not
//! reachable from the frontend-agnostic body. The ordered-subset diagnostic
//! (missing / out-of-order) plus the rendered/expected content lists are
//! preserved.
//!
//! Status: functional.

use holon_pbt_core::capabilities::RefViewSelection;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvViewmodelDecompiledRowsMatchQuery;

impl InvViewmodelDecompiledRowsMatchQuery {
    pub const ID: InvariantId = InvariantId("inv-viewmodel-decompiled-rows-match-query");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvViewmodelDecompiledRowsMatchQuery
where
    R: RefViewSelection,
    S: SutRenderer,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        // Inline gate: `if let Some(expected_expr) = root_render_expr()`.
        // Without a root render expr there are no visible columns and the
        // comparison cannot run.
        let visible_columns = ref_.root_visible_columns();

        // The SUT extracts the decompiled rendered content + query data
        // content, filtered to `visible_columns`. `None` means the inline's
        // gates didn't hold (root not ready, or any of rendered_rows /
        // visible_columns / data_rows empty) → nothing to assert.
        let Some((rendered_content, data_content)) =
            sut.root_content_comparison(&visible_columns).await
        else {
            return InvariantResult::Skipped(
                "root not ready, or rendered/visible-columns/data rows empty".into(),
            );
        };

        if rendered_content == data_content {
            return InvariantResult::Ok;
        }
        // Diagnose both directions: rendered rows with no data backing
        // (ghost/duplicated rows) and data rows that never rendered
        // (dropped rows — invisible under the old subset-only check).
        let subset_result =
            crate::display_assertions::is_ordered_subset(&rendered_content, &data_content);
        let dropped: Vec<&String> = {
            let mut rendered_iter = rendered_content.iter().peekable();
            data_content
                .iter()
                .filter(|d| {
                    if rendered_iter.peek() == Some(d) {
                        rendered_iter.next();
                        false
                    } else {
                        true
                    }
                })
                .collect()
        };
        InvariantResult::Fail(format!(
            "[inv-viewmodel-decompiled-rows-match-query] Decompiled content != query data \
             (ordered equality).\nRendered: {:?}\nExpected: {:?}\nDropped (in data, never \
             rendered): {:?}\nGhost/out-of-order rendered: missing={:?} \
             out_of_order={:?}\nVisible columns: {:?}",
            rendered_content,
            data_content,
            dropped,
            subset_result.missing_from_expected,
            subset_result.out_of_order,
            visible_columns,
        ))
    }
}
