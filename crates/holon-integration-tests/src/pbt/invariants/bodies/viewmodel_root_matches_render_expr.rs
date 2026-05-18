//! Phase 7 — `inv-viewmodel-root-matches-render-expr` (DEFERRED).
//!
//! Inline body lives at `sut.rs:5019–5061` (inside ReactiveEngine snapshot block,
//! sub-check 10d).
//!
//! # Why deferred
//!
//! The body requires:
//! - `display_tree` from private `reactive_engine` interpret_pure path
//! - `ref_state.root_render_expr()` — ref-side gate, no Ref* cap
//! - `holon_api::render_types::RenderExpr` + `expected_expr.to_rhai()` — api types
//!   not in pbt-core
//!
//! Wire when `SutRenderer::render_tree_of` or a new `SutViewModel::root_widget_name()`
//! covers the headless path and `RefBlockTree` gains `root_render_expr_name()`.

pub struct InvViewmodelRootMatchesRenderExpr;

impl InvViewmodelRootMatchesRenderExpr {
    pub const ID: holon_pbt_core::invariant::InvariantId =
        holon_pbt_core::invariant::InvariantId("inv-viewmodel-root-matches-render-expr");
}

// Status: ref-side unblocked (RefRender::has_root_render_expr added).
// Remaining blocker: display_tree comes from reactive_engine (private RefCell);
// RefRender::has_root_render_expr is a bool gate but the body also needs
// ref_state.root_render_expr() as a RenderExpr (not in any cap trait).
// Promote when SutViewModel::root_widget_name is wired and RefRender gains root_render_expr_name.
