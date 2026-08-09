//! `RefWiring` / `RefViewSelection` / `RefViewSelectionMut` /
//! `RefSqlCardinality` — the remaining harness/misc-state capability impls.
//!
//! @pbt kind ref
//! @pbt covers view-selection — active view + render-expr name per region, the
//!   cap-set gate (`RefWiring`, harness config not domain state), and the SQL
//!   row-cardinality budget model (`RefSqlCardinality`).

use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::RefSqlCardinality;
use holon_pbt_core::capabilities::RefViewSelection;
use holon_pbt_core::capabilities::RefViewSelectionMut;
use holon_pbt_core::capabilities::RefWiring;

use super::super::reference_state::ReferenceState;
use super::from_cap_region;

impl RefWiring for ReferenceState {
    fn has_cap_set(&self) -> bool {
        self.harness.cap_set.is_some()
    }

    fn caps_available(&self, caps: &[holon_pbt_core::composition::CapId]) -> bool {
        ReferenceState::caps_available(self, caps)
    }
}

impl RefViewSelectionMut for ReferenceState {
    fn set_current_view(&mut self, view: &str) {
        self.ui.user.current_view = view.to_string();
    }
}

impl RefViewSelection for ReferenceState {
    fn current_view(&self) -> String {
        ReferenceState::current_view(self)
    }

    fn active_render_expr_name(&self, region: CapRegion) -> Option<String> {
        let api_region = from_cap_region(region);
        self.active_render_expr_name(api_region)
    }

    fn root_render_expr_name(&self) -> Option<String> {
        // Faithful to inline 10d: read the ROOT render expr (NOT
        // main-panel-preferring) and extract the FunctionCall name.
        // Returns None when there's no root render expr OR when it isn't
        // a FunctionCall; callers disambiguate via has_root_render_expr().
        match self.root_render_expr()? {
            holon_api::render_types::RenderExpr::FunctionCall { name, .. } => Some(name.clone()),
            _ => None,
        }
    }

    fn has_root_render_expr(&self) -> bool {
        self.root_render_expr().is_some()
    }

    fn root_visible_columns(&self) -> Vec<String> {
        // Faithful to inline 10f: `expected_expr.visible_columns()` on the
        // ROOT render expr. Empty when there's no root render expr.
        self.root_render_expr()
            .map(|e| e.visible_columns())
            .unwrap_or_default()
    }

    fn main_panel_block_id(&self) -> Option<holon_api::entity_uri::EntityUri> {
        self.main_panel_block_id().as_ref().map(super::cap_id)
    }

    fn main_panel_render_expr_name(&self) -> Option<String> {
        // The content the main panel should render: its own render expr,
        // falling back to the root render expr (mirrors
        // active_render_expr_name(Main)). Only FunctionCall names are returned.
        match self.main_panel_render_expr().or(self.root_render_expr())? {
            holon_api::render_types::RenderExpr::FunctionCall { name, .. } => Some(name.clone()),
            _ => None,
        }
    }
}

impl RefSqlCardinality for ReferenceState {
    fn block_count(&self) -> usize {
        self.domain.block_state.blocks.len()
    }
    fn document_count(&self) -> usize {
        self.files.documents.len()
    }
    fn active_watch_count(&self) -> usize {
        self.mcp.active_watches.len()
    }
    fn last_navigate_first_visit(&self) -> bool {
        self.ui.tab.last_navigate_first_visit
    }
    fn last_open_tab_activated(&self) -> bool {
        self.ui.tab.last_open_tab_activated
    }
    fn content_writes_reach_sql(&self) -> bool {
        !self.enable_loro()
    }
}
