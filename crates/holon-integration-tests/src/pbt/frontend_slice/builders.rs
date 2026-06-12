//! Compose the frontend slice's SUT `CapMap` from its component — "a slice is a
//! component list" (§1). A fourth realization (after memory/Loro/SQL) backing the
//! *same* shared catalog, this one over the real headless render pipeline.

use std::sync::Arc;

use holon_pbt_core::capabilities::SutSqlProjection;
use holon_pbt_core::composition::{CapMap, Config};

use super::components::HeadlessFrontendComponent;

/// Build the frontend composed SUT — a real headless `ReactiveEngine` exposed as
/// `SutRenderer` + `SutBackend`. The same `composed_invariant_catalog()` selects
/// against these caps; the renderer invariants light up over the real render
/// pipeline and the block-tree invariants run over `block_raw` too.
pub fn frontend_wide(component: Arc<HeadlessFrontendComponent>) -> CapMap {
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
    caps.insert(component as Arc<dyn SutSqlProjection>);
    caps
}
