//! `RefFocus` / `RefFocusMut` / `RefFocusRoots` / `RefGlobalFocus`.
//!
//! @pbt kind ref
//! @pbt covers navigation-focus — per-region focused entity + cursor and the
//!   `focus_roots` set (mirrors `schema/matview_focus_roots.sql`: open pins /
//!   `navigation_history WHERE closed_at IS NULL`). The matview shape is
//!   HAND-modeled in `expected_focus_root_ids`; keep it in sync with that SQL.

use std::collections::BTreeSet;

use holon_api::Region;
use holon_api::entity_uri::EntityUri;
use holon_pbt_core::capabilities::CapCursor;
use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::RefFocus;
use holon_pbt_core::capabilities::RefFocusMut;
use holon_pbt_core::capabilities::RefFocusRoots;
use holon_pbt_core::capabilities::RefGlobalFocus;

use super::super::reference_state::ReferenceState;
use super::super::ui_types::CursorPosition;
use super::cap_id;
use super::cap_id_set;
use super::from_cap_region;
use super::parse_id_must;

impl RefFocus for ReferenceState {
    fn expected_focus_root_rows(&self) -> Vec<(String, Vec<String>)> {
        Region::ALL
            .iter()
            .map(|region| {
                let roots = self
                    .expected_focus_root_ids(*region)
                    .into_iter()
                    .map(|u| u.as_str().to_string())
                    .collect();
                (region.as_str().to_string(), roots)
            })
            .collect()
    }

    fn navigation_focus_rows(&self) -> Vec<(String, Option<String>)> {
        self.ui
            .tab
            .navigation_history
            .iter()
            .map(|(region, hist)| {
                (
                    region.as_str().to_string(),
                    hist.current_focus().map(|u| u.as_str().to_string()),
                )
            })
            .collect()
    }

    fn current_focus(&self, region: CapRegion) -> Option<EntityUri> {
        ReferenceState::current_focus(self, from_cap_region(region))
            .as_ref()
            .map(cap_id)
    }

    fn focused_cursor(&self, region: CapRegion) -> Option<CapCursor> {
        let r = from_cap_region(region);
        self.ui.tab.focused_cursor.get(&r).map(|cp| CapCursor {
            line: cp.line,
            column: cp.column,
        })
    }
}

impl RefFocusMut for ReferenceState {
    fn set_focus(&mut self, region: CapRegion, id: EntityUri, cursor: CapCursor) {
        let uri = parse_id_must(&id);
        let r = from_cap_region(region);
        self.ui.tab.focused_entity_id.insert(r, uri.clone());
        self.ui.tab.focused_cursor.insert(
            r,
            CursorPosition {
                line: cursor.line,
                column: cursor.column,
            },
        );
        if r == Region::Main {
            self.ui.tab.focused_block = Some(uri);
        }
    }

    fn clear_focus_if_deleted(&mut self, id: &EntityUri) {
        let uri = parse_id_must(id);
        ReferenceState::clear_focus_if_deleted(self, &uri);
    }

    fn open_active_editor(&mut self, id: EntityUri, content: String, cursor_byte: usize) {
        self.ui.tab.active_editor = Some(super::super::ui_types::ActiveEditor {
            block_id: id,
            in_memory_content: content,
            cursor_byte,
            dirty: false,
        });
    }

    fn close_active_editor(&mut self) {
        self.ui.tab.active_editor = None;
    }
}

// ─── Phase 7 Stage B — extended ref-side cap impls ───────────────────

impl RefFocusRoots for ReferenceState {
    fn rendered_focus_root_ids(&self, region: CapRegion) -> BTreeSet<EntityUri> {
        let api_region = from_cap_region(region);
        cap_id_set(self.rendered_focus_root(api_region))
    }
}

impl RefGlobalFocus for ReferenceState {
    fn global_focused_block(&self) -> Option<EntityUri> {
        self.ui.tab.focused_block.as_ref().map(cap_id)
    }
}
