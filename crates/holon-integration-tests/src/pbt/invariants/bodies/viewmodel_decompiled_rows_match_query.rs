//! Phase 7 — `inv-viewmodel-decompiled-rows-match-query` (DEFERRED).
//!
//! Inline body lives at `sut.rs:5116–5173` (inside the ReactiveEngine snapshot
//! block, labelled inv-viewmodel-decompiled-rows-match-query).
//!
//! # Why deferred
//!
//! The body requires:
//! - `reactive.ensure_watching(&root_id)` snapshot `render_expr` + `data_rows`
//!   — from private `self.reactive_engine`
//! - `ref_state.root_render_expr()` — ref-side render expression, no Ref* cap
//! - `crate::display_assertions::extract_rendered_rows(&display_tree)` — internal helper
//! - `crate::display_assertions::is_ordered_subset(...)` — internal helper
//! - `holon_frontend::interpret_pure` — frontend type not in pbt-core
//!
//! Wire when `SutViewModel` exposes `rendered_row_contents()` and
//! `data_row_contents()` and `RefBlockTree` covers `root_render_expr`.

pub struct InvViewmodelDecompiledRowsMatchQuery;

impl InvViewmodelDecompiledRowsMatchQuery {
    pub const ID: holon_pbt_core::invariant::InvariantId =
        holon_pbt_core::invariant::InvariantId("inv-viewmodel-decompiled-rows-match-query");
}

// `Invariant` impl intentionally omitted — body migration deferred.
