//! `RefToggle` / `RefToggleMut` / `RefTaskState` / `RefTaskStateToggle`.
//!
//! @pbt kind ref
//! @pbt covers expand-collapse — `is_expanded` (embedded-page lazy-collapse
//!   model) + task-state keyword for the ViewModel `value_fn_provider`
//!   invariants. Untracked toggles default open (see `widget_state`, disclosed).

use std::sync::Arc;

use holon_api::entity_uri::EntityUri;
use holon_pbt_core::capabilities::RefTaskState;
use holon_pbt_core::capabilities::RefToggle;
use holon_pbt_core::capabilities::RefToggleMut;

use super::super::reference_state::ReferenceState;
use super::parse_id;

impl RefToggle for ReferenceState {
    fn is_expanded(&self, id: &EntityUri) -> bool {
        self.ui.tab.expanded_toggles.contains(id)
    }
}

impl RefToggleMut for ReferenceState {
    fn set_expanded(&mut self, id: &EntityUri, expanded: bool) {
        if expanded {
            self.ui.tab.expanded_toggles.insert(id.clone());
        } else {
            self.ui.tab.expanded_toggles.remove(id);
        }
        // Collapse is DOCUMENT state (2026-07-11 ruling): the production
        // chevron/toggle handler dispatches `set_field(collapsed)` alongside
        // the view-local gate flip, so the SUT block row changes. Mirror it
        // on the ref block (single-sourced here — ExpandToggle and
        // ToggleCollapse both route through this cap) so
        // inv-blocks-match-ref stays honest. Toggles can target widgets
        // whose target_id is not a tracked block (synthetic fixtures) —
        // those have no ref block to update.
        if let Some(block) = self.domain.block_state.blocks.get_mut(id) {
            block.collapsed = !expanded;
        }
    }
    fn set_expanded_view_local(&mut self, id: &EntityUri, expanded: bool) {
        // View-local only — no `block.collapsed` field mutation. For
        // profile-driven toggles (embedded_page) where the lazy gate is
        // entirely visual and no `collapsed` column exists on the target.
        if expanded {
            self.ui.tab.expanded_toggles.insert(id.clone());
        } else {
            self.ui.tab.expanded_toggles.remove(id);
        }
    }
    fn toggle_drawer(&mut self, id: &str) {
        // Default-open, so an untracked drawer flips to closed.
        let current = holon_layout_testing::LayoutRefState::drawer_is_open(self, id);
        self.ui.tab.drawer_open.insert(id.to_string(), !current);
    }
}

impl RefTaskState for ReferenceState {
    fn task_state_of(&self, id: &EntityUri) -> Option<String> {
        let uri = parse_id(id)?;
        let block = self.domain.block_state.blocks.get(&uri)?;
        block
            .properties
            .get("task_state")
            .and_then(|v| v.as_string().map(str::to_owned))
    }
}

impl holon_pbt_core::capabilities::RefTaskStateToggle for ReferenceState {
    fn rendered_state_toggle_ids(&self) -> Vec<EntityUri> {
        let owned_render_expr = self
            .main_panel_render_expr()
            .or_else(|| self.root_render_expr())
            .cloned()
            .unwrap_or_else(super::super::reference_state::default_root_render_expr);

        let main_focus_roots = self.expected_focus_root_ids(holon_api::Region::Main);
        let visible_text_block_ids: Vec<EntityUri> = self
            .domain
            .block_state
            .blocks
            .values()
            .filter(|b| {
                b.content_type == holon_api::ContentType::Text
                    && !b.is_page()
                    && !self.domain.layout_blocks.contains(&b.id)
                    && self.is_descendant_of_any(&b.id, &main_focus_roots)
            })
            .map(|b| b.id.clone())
            .collect();

        let rows: Vec<holon_api::widget_spec::DataRow> = visible_text_block_ids
            .iter()
            .filter_map(|id| self.domain.block_state.blocks.get(id))
            .map(super::super::reference_state::block_to_data_row)
            .collect();
        let arc_rows: Vec<Arc<_>> = rows.into_iter().map(Arc::new).collect();
        // ALLOW(pbt-sut-handle-frontend-simulation): generator-side render lookup
        let vm = holon_frontend::interpret_pure(&owned_render_expr, &arc_rows, self);
        vm.snapshot()
            .state_toggle_block_ids()
            .into_iter()
            .filter_map(|id| holon_api::EntityUri::parse(&id).ok())
            .collect()
    }

    fn apply_toggle_state(
        &mut self,
        block_id: &EntityUri,
        new_state: holon_pbt_core::types::CycleTarget,
    ) {
        self.push_undo_snapshot();
        self.apply_mutation(&holon_pbt_core::types::MutationEvent {
            source: holon_pbt_core::types::MutationSource::UI,
            mutation: holon_pbt_core::types::Mutation::Update {
                id: block_id.clone(),
                fields: [(
                    "task_state".to_string(),
                    holon_api::Value::String(new_state.keyword().to_string()),
                )]
                .into(),
            },
        });
    }
}
