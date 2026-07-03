//! Compose the frontend slice's SUT `CapMap` from its component — "a slice is a
//! component list" (§1). A fourth realization (after memory/Loro/SQL) backing the
//! *same* shared catalog, this one over the real headless render pipeline.

use std::sync::Arc;

use holon_pbt_core::capabilities::{SutFocus, SutQueryResults, SutSqlProjection};
use holon_pbt_core::composition::{CapMap, Config};

use super::block_query_component::BlockQueryFrontendComponent;
use super::components::HeadlessFrontendComponent;

/// Build the frontend composed SUT — a real headless `ReactiveEngine` exposed as
/// `SutRenderer` + `SutBackend`. The same `composed_invariant_catalog()` selects
/// against these caps; the renderer invariants light up over the real render
/// pipeline and the block-tree invariants run over `block_raw` too.
///
/// `SutQueryResults` is added HERE (the full-mode query-engine surface) rather than
/// in the component's `CapProvider::register` — exactly like `SutSqlProjection` —
/// so the negative-selection `sut_absent: [SutQueryResults]` discriminator stays
/// meaningful: the degraded [`block_query_degraded`] builder wires a renderer with
/// NO query engine, so its CapMap lacks this cap and selects the degraded twin.
pub fn frontend_wide(component: Arc<HeadlessFrontendComponent>) -> CapMap {
    let mut caps = Config::new().with_arc(component.clone()).build();
    caps.insert(component as Arc<dyn SutQueryResults>);
    caps
}

/// The degraded ("shows source") SUT `CapMap`: a real no-Turso block-query
/// frontend over a Loro `BlockQuerySource` whose root has a query-source child.
/// Provides `SutRenderer` (root render kind = `source_editor`) but NOT
/// `SutQueryResults` (no query engine in this wiring), so the catalog selects the
/// degraded `inv-viewmodel-shows-source-when-no-query` twin and DESELECTS the
/// full-mode `inv-viewmodel-decompiled-rows-match-query` twin — the first real
/// `sut_absent` negative-selection consumer.
pub fn block_query_degraded(component: Arc<BlockQueryFrontendComponent>) -> CapMap {
    Config::new().with_arc(component).build()
}

/// The navigation-slice SUT `CapMap`: [`frontend_wide`] plus the component's
/// `SutSqlProjection` (the `current_focus` / `focus_roots` matview reads the focus
/// invariants need). `SutSqlProjection` is added HERE rather than in the
/// component's `CapProvider::register` so the *other* frontend-slice tests don't
/// newly select `block_content_sql` — only the navigation slice (paired with a
/// `RefFocus`-only ref) runs the focus invariants. `SutFocusWrite` is already in
/// `register`, so the `NavigateFocus` transition drives this CapMap.
pub fn frontend_navigation_wide(component: Arc<HeadlessFrontendComponent>) -> CapMap {
    let mut caps = frontend_wide(component.clone());
    caps.insert(component.clone() as Arc<dyn SutSqlProjection>);
    // `SutFocus` (C-5 split, 2026-07-02): the focus/nav matview reads the
    // focus invariants need. Registered only on this navigation-driving CapMap.
    caps.insert(component as Arc<dyn SutFocus>);
    caps
}
