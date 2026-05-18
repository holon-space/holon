//! Phase 7 — `inv-viewmodel-root-matches-render-expr` (FUNCTIONAL).
//!
//! Inline body originally at `sut.rs:5019–5062` (sub-check 10d).
//!
//! Asserts that the root widget kind in the snapshot matches the reference
//! model's active render expression name for `CapRegion::Main`. The engine
//! may wrap the root in a `view_mode_switcher`; in that case we look one
//! level deeper (first child's kind).
//!
//! Status: functional.

use holon_pbt_core::capabilities::{CapRegion, RefRender, SutRenderer};
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult, RunMode};

pub struct InvViewmodelRootMatchesRenderExpr;

impl InvViewmodelRootMatchesRenderExpr {
    pub const ID: InvariantId = InvariantId("inv-viewmodel-root-matches-render-expr");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvViewmodelRootMatchesRenderExpr
where
    R: RefRender,
    S: SutRenderer,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    fn mode(&self) -> RunMode {
        RunMode::Strict
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let Some(expected_name) = ref_.active_render_expr_name(CapRegion::Main) else {
            return InvariantResult::Ok;
        };

        let root = sut.widget_tree_snapshot().await;
        let actual = root.kind.as_str();

        let matches = actual == expected_name
            || (actual == "view_mode_switcher"
                && root
                    .children
                    .first()
                    .map(|c| c.kind.as_str() == expected_name)
                    .unwrap_or(false));

        if matches {
            InvariantResult::Ok
        } else {
            InvariantResult::Fail(format!(
                "[inv-viewmodel-root-matches-render-expr] Root widget '{actual}' \
                 does not match expected render expr '{expected_name}'"
            ))
        }
    }
}
