//! `RefPins` / `RefPinsMut` / `RefNavHistory` / `RefNavHistoryMut` /
//! `RefArrowNav`.

use holon_api::Region;
use holon_api::entity_uri::EntityUri;
use holon_pbt_core::capabilities::RefNavHistory;
use holon_pbt_core::capabilities::RefNavHistoryMut;
use holon_pbt_core::capabilities::RefPins;
use holon_pbt_core::capabilities::RefPinsMut;

use super::super::reference_state::ReferenceState;
use super::super::ui_types::OpenPinEntry;

impl RefPins for ReferenceState {
    fn open_pin_history_ids(&self) -> Vec<i64> {
        self.ui
            .user
            .open_pins
            .values()
            .flatten()
            .map(|p| p.history_id)
            .collect()
    }

    fn is_closeable_pin(&self, history_id: i64) -> bool {
        self.ui.user.open_pins.iter().any(|(region, pins)| {
            let focus = self.current_focus(*region);
            pins.iter()
                .any(|p| p.history_id == history_id && p.block_id.is_some() && p.block_id != focus)
        })
    }
}

impl RefPinsMut for ReferenceState {
    fn close_pin(&mut self, history_id: i64) {
        for pins in self.ui.user.open_pins.values_mut() {
            pins.retain(|p| p.history_id != history_id);
        }
    }

    fn upsert_open_pin(&mut self, region: Region, block_id: &EntityUri) {
        // Move-to-top dedup, mirroring `provider.rs::focus_pin`: SELECT existing
        // open `(region, block_id)`; UPDATE timestamp if found, else INSERT.
        // Bumping `next_pin_ts` (not `next_history_id`) on the UPDATE path
        // matches the no-INSERT path of `update_pin_timestamp.sql`.
        let added_ts_logical = self.ui.user.next_pin_ts;
        self.ui.user.next_pin_ts += 1;

        let pins = self.ui.user.open_pins.entry(region).or_default();
        if let Some(existing) = pins
            .iter_mut()
            .find(|p| p.block_id.as_ref() == Some(block_id))
        {
            existing.added_ts_logical = added_ts_logical;
        } else {
            let history_id = self.ui.tab.next_history_id;
            self.ui.tab.next_history_id += 1;
            self.ui
                .user
                .open_pins
                .entry(region)
                .or_default()
                .push(OpenPinEntry {
                    history_id,
                    block_id: Some(block_id.clone()),
                    added_ts_logical,
                });
        }
    }
}

// ─── RefNavHistory / RefNavHistoryMut ─────────────────────────────────

impl RefNavHistory for ReferenceState {
    fn can_go_back(&self, region: Region) -> bool {
        ReferenceState::can_go_back(self, region)
    }
    fn can_go_forward(&self, region: Region) -> bool {
        ReferenceState::can_go_forward(self, region)
    }
    fn predicts_navigation_focus(&self, block_id: &EntityUri, region: Region) -> bool {
        ReferenceState::predicts_navigation_focus(self, block_id, region)
    }
    fn predicted_sidebar_navigation_targets(&self) -> Vec<EntityUri> {
        ReferenceState::predicted_sidebar_navigation_targets(self)
    }
    fn drawer_is_open(&self, panel_id: &str) -> bool {
        holon_layout_testing::LayoutRefState::drawer_is_open(self, panel_id)
    }
}

impl RefNavHistoryMut for ReferenceState {
    fn nav_step_back(&mut self, region: Region) {
        if let Some(history) = self.ui.tab.navigation_history.get_mut(&region)
            && history.cursor > 0
        {
            history.cursor -= 1;
        }
        self.ui.tab.focused_entity_id.remove(&region);
        self.ui.tab.focused_cursor.remove(&region);
        // Blur on nav: clears `active_editor`, committing pending text only
        // under a real editor (mirrors prod's `on_blur`).
        self.blur_active_editor();
    }

    fn nav_step_forward(&mut self, region: Region) {
        if let Some(history) = self.ui.tab.navigation_history.get_mut(&region)
            && history.cursor < history.entries.len() - 1
        {
            history.cursor += 1;
        }
        self.ui.tab.focused_entity_id.remove(&region);
        self.ui.tab.focused_cursor.remove(&region);
        self.blur_active_editor();
    }

    fn nav_go_home(&mut self, region: Region) {
        // Idempotent like same-target focus: when already home (current focus is
        // `None`), prod's `focus(region, None)` writes NO new `navigation_history`
        // / `open_pins` row. Pushing a duplicate would let `NavigateBack` walk back
        // through phantom home entries the SUT never created.
        let already_home = self.current_focus(region).is_none();
        if !already_home {
            let history = self.ui.tab.navigation_history.entry(region).or_default();
            history.entries.truncate(history.cursor + 1);
            history.entries.push(None);
            history.cursor = history.entries.len() - 1;

            // `go_home` = `focus(region, None)`: close all open rows in region,
            // then insert a NULL-block_id home row. Kept in `open_pins` so
            // `next_history_id` aligns with SQLite's AUTOINCREMENT; filtered out of
            // `expected_focus_root_ids` (None block_id).
            let history_id = self.ui.tab.next_history_id;
            self.ui.tab.next_history_id += 1;
            let added_ts_logical = self.ui.user.next_pin_ts;
            self.ui.user.next_pin_ts += 1;
            let pins = self.ui.user.open_pins.entry(region).or_default();
            pins.clear();
            pins.push(OpenPinEntry {
                history_id,
                block_id: None,
                added_ts_logical,
            });
        }

        self.ui.tab.focused_entity_id.remove(&region);
        self.ui.tab.focused_cursor.remove(&region);
        // Mirror prod: `maybe_mirror_navigation_focus` clears the global
        // `focused_block` on go_home regardless of which region triggered it.
        self.ui.tab.focused_block = None;
        self.blur_active_editor();
    }

    fn nav_focus(&mut self, region: Region, block_id: &EntityUri) {
        // Re-focusing the region's current target is idempotent in prod:
        // `navigation.focus` on the active target writes no new history row.
        let already_focused = self.current_focus(region).as_ref() == Some(block_id);

        // Budget model: the first navigation to a root creates its watch matviews;
        // recorded pre-insert because the budget invariant only sees post-apply.
        self.ui.tab.last_navigate_first_visit =
            self.ui.tab.seen_focus_targets.insert(block_id.clone());

        if !already_focused {
            let history = self.ui.tab.navigation_history.entry(region).or_default();
            history.entries.truncate(history.cursor + 1);
            history.entries.push(Some(block_id.clone()));
            history.cursor = history.entries.len() - 1;

            // Mirror provider.rs `focus`: close all open rows in the region, then
            // insert a new open row. `next_history_id` follows AUTOINCREMENT (INSERT
            // only); a same-block re-focus inserts no row, so the counter holds.
            let history_id = self.ui.tab.next_history_id;
            self.ui.tab.next_history_id += 1;
            let added_ts_logical = self.ui.user.next_pin_ts;
            self.ui.user.next_pin_ts += 1;
            let pins = self.ui.user.open_pins.entry(region).or_default();
            pins.clear();
            pins.push(OpenPinEntry {
                history_id,
                block_id: Some(block_id.clone()),
                added_ts_logical,
            });
        }

        self.ui.tab.focused_entity_id.remove(&region);
        self.ui.tab.focused_cursor.remove(&region);
        // Mirror `UiState::set_focus`: the nav target becomes the global focus.
        self.ui.tab.focused_block = Some(block_id.clone());
        self.blur_active_editor();
    }
}

impl holon_pbt_core::capabilities::RefArrowNav for ReferenceState {
    type Direction = holon_frontend::navigation::NavDirection;

    fn region_has_focus(&self, region: Region) -> bool {
        self.ui.tab.focused_entity_id.contains_key(&region)
    }

    fn apply_arrow_navigate(
        &mut self,
        region: Region,
        direction: holon_frontend::navigation::NavDirection,
        steps: u8,
    ) {
        use holon_frontend::navigation::Boundary;
        use holon_frontend::navigation::CursorHint;
        use holon_frontend::navigation::NavDirection;

        use super::super::ui_types::CursorPosition;

        let mut current_id = self
            .ui
            .tab
            .focused_entity_id
            .get(&region)
            .expect("ArrowNavigate requires focused entity")
            .clone();
        let mut cursor = self
            .ui
            .tab
            .focused_cursor
            .get(&region)
            .copied()
            .unwrap_or(CursorPosition::start());

        let navigator = self.build_reference_navigator(region);

        for _ in 0..steps {
            let content = self
                .domain
                .block_state
                .blocks
                .get(&current_id)
                .map(|b| b.content.as_str())
                .unwrap_or("");
            let line_count = if content.is_empty() {
                1
            } else {
                content.split('\n').count()
            };
            let last_line = line_count.saturating_sub(1);

            let crosses_block = match direction {
                NavDirection::Up => cursor.line == 0,
                NavDirection::Down => cursor.line >= last_line,
                NavDirection::Left => cursor.line == 0 && cursor.column == 0,
                NavDirection::Right => {
                    let line_len = content
                        .split('\n')
                        .nth(cursor.line)
                        .map(|l| l.len())
                        .unwrap_or(0);
                    cursor.line >= last_line && cursor.column >= line_len
                }
            };

            if crosses_block {
                if let Some(ref nav) = navigator {
                    let boundary = match direction {
                        NavDirection::Up => Boundary::Top,
                        NavDirection::Down => Boundary::Bottom,
                        NavDirection::Left => Boundary::Left,
                        NavDirection::Right => Boundary::Right,
                    };
                    let hint = CursorHint {
                        column: cursor.column,
                        boundary,
                    };
                    if let Some(target) = nav.navigate(&current_id, direction, &hint) {
                        current_id = target.block_id.clone();
                        let target_content = self
                            .domain
                            .block_state
                            .blocks
                            .get(&current_id)
                            .map(|b| b.content.as_str())
                            .unwrap_or("");
                        let offset = holon_frontend::navigation::placement_to_offset(
                            target_content,
                            target.placement,
                        );
                        let (line, col) =
                            holon_frontend::navigation::offset_to_line_col(target_content, offset);
                        cursor = CursorPosition { line, column: col };
                    }
                }
            } else {
                match direction {
                    NavDirection::Up => {
                        cursor.line = cursor.line.saturating_sub(1);
                    }
                    NavDirection::Down => {
                        cursor.line = (cursor.line + 1).min(last_line);
                    }
                    NavDirection::Left => {
                        if cursor.column > 0 {
                            cursor.column -= 1;
                        } else if cursor.line > 0 {
                            cursor.line -= 1;
                            let prev_line_len = content
                                .split('\n')
                                .nth(cursor.line)
                                .map(|l| l.len())
                                .unwrap_or(0);
                            cursor.column = prev_line_len;
                        }
                    }
                    NavDirection::Right => {
                        let line_len = content
                            .split('\n')
                            .nth(cursor.line)
                            .map(|l| l.len())
                            .unwrap_or(0);
                        if cursor.column < line_len {
                            cursor.column += 1;
                        } else if cursor.line < last_line {
                            cursor.line += 1;
                            cursor.column = 0;
                        }
                    }
                }
            }
        }

        self.ui.tab.focused_block = Some(current_id.clone());
        self.ui.tab.focused_entity_id.insert(region, current_id);
        self.ui.tab.focused_cursor.insert(region, cursor);
    }
}
