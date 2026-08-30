//! Navigation operations provider
//!
//! Implements NavigationOperations for backend-driven navigation state.
//! Operations modify navigation tables directly (not part of undo stack).

use std::collections::HashMap;
use std::str::FromStr;

use async_trait::async_trait;
use holon_api::EntityName;
use holon_api::NavigationOp;
use holon_api::OperationDescriptor;
use holon_api::OperationParam;
use holon_api::Region;
use holon_api::TypeHint;
use holon_api::Value;
use holon_core::OperationProvider;
use holon_core::OperationResult;
use holon_core::Result;
use holon_core::storage::types::StorageEntity;

use crate::storage::DbHandle;

/// Navigation operations entity name
pub const ENTITY_NAME: &str = "navigation";
pub const SHORT_NAME: &str = "nav";

/// The `region` operation parameter shared by every navigation op.
fn region_param() -> OperationParam {
    OperationParam {
        name: "region".to_string(),
        type_hint: TypeHint::OneOf {
            values: Region::ALL.iter().map(|r| Value::from(*r)).collect(),
        },
        description: "UI region to navigate".to_string(),
    }
}

/// The navigation entity's full operation descriptor set: the manual focus /
/// pin / history ops plus the macro-generated editor-cursor ops. Shared by
/// every `NavigationProvider` implementation (Turso-backed and in-memory) so
/// the render layer builds identical click intents regardless of backend.
pub fn navigation_operation_descriptors() -> Vec<OperationDescriptor> {
    let manual_ops = vec![
        OperationDescriptor {
            entity_name: ENTITY_NAME.into(),
            entity_short_name: SHORT_NAME.to_string(),
            id_column: "region".to_string(),
            name: NavigationOp::Focus.as_str().to_string(),
            display_name: "Focus".to_string(),
            description: "Navigate to focus on a specific block".to_string(),
            required_params: vec![
                region_param(),
                OperationParam {
                    name: "block_id".to_string(),
                    type_hint: TypeHint::String,
                    description: "Block ID to focus on".to_string(),
                },
            ],
            affected_fields: vec!["block_id".to_string()],
            param_mappings: vec![],
            target_scope: holon_api::TargetScope::Global,
            boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
            menu_exposure: holon_api::MenuExposure::NotListed {
                surface: holon_api::NonMenuSurface::Navigation,
            },
            trigger: None,
            bound_params: Default::default(),
            marking_delta: holon_api::marking::MarkingDelta::Undeclared,
            guard: holon_api::pattern::OpGuard::None,
            arcs: holon_api::arcs::TransitionArcs::Undeclared,
        },
        OperationDescriptor {
            entity_name: ENTITY_NAME.into(),
            entity_short_name: SHORT_NAME.to_string(),
            id_column: "region".to_string(),
            name: NavigationOp::FocusPin.as_str().to_string(),
            display_name: "Pin Block".to_string(),
            description: "Pin a block to a region (move-to-top dedup; right sidebar uses this)"
                .to_string(),
            required_params: vec![
                region_param(),
                OperationParam {
                    name: "block_id".to_string(),
                    type_hint: TypeHint::String,
                    description: "Block ID to pin".to_string(),
                },
            ],
            affected_fields: vec!["block_id".to_string()],
            param_mappings: vec![],
            target_scope: holon_api::TargetScope::Global,
            boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
            menu_exposure: holon_api::MenuExposure::NotListed {
                surface: holon_api::NonMenuSurface::Navigation,
            },
            trigger: None,
            bound_params: Default::default(),
            marking_delta: holon_api::marking::MarkingDelta::Undeclared,
            guard: holon_api::pattern::OpGuard::None,
            arcs: holon_api::arcs::TransitionArcs::Undeclared,
        },
        OperationDescriptor {
            entity_name: ENTITY_NAME.into(),
            entity_short_name: SHORT_NAME.to_string(),
            id_column: "region".to_string(),
            name: NavigationOp::Close.as_str().to_string(),
            display_name: "Close Pin".to_string(),
            description: "Soft-close a navigation_history row by id (sidebar X button)".to_string(),
            required_params: vec![OperationParam {
                name: "history_id".to_string(),
                type_hint: TypeHint::Number,
                description: "navigation_history.id to close".to_string(),
            }],
            affected_fields: vec!["closed_at".to_string()],
            param_mappings: vec![],
            target_scope: holon_api::TargetScope::Global,
            boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
            menu_exposure: holon_api::MenuExposure::NotListed {
                surface: holon_api::NonMenuSurface::Navigation,
            },
            trigger: None,
            bound_params: Default::default(),
            marking_delta: holon_api::marking::MarkingDelta::Undeclared,
            guard: holon_api::pattern::OpGuard::None,
            arcs: holon_api::arcs::TransitionArcs::Undeclared,
        },
        OperationDescriptor {
            entity_name: ENTITY_NAME.into(),
            entity_short_name: SHORT_NAME.to_string(),
            id_column: "region".to_string(),
            name: NavigationOp::GoBack.as_str().to_string(),
            display_name: "Go Back".to_string(),
            description: "Navigate to previous view in history".to_string(),
            required_params: vec![region_param()],
            affected_fields: vec!["block_id".to_string()],
            param_mappings: vec![],
            target_scope: holon_api::TargetScope::Global,
            boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
            // Action-bar only: a phone has no keyboard chord for history
            // navigation, while a desktop user already has one, so this is
            // deliberately NOT in the slash menu.
            menu_exposure: holon_api::MenuExposure::Listed {
                surfaces: holon_api::SurfaceSet {
                    slash_menu: false,
                    action_bar: true,
                },
            },
            trigger: None,
            // The bar acts on the main region; binding it here is what lets a
            // tap dispatch without asking the user which region they meant.
            bound_params: ::std::collections::HashMap::from([(
                "region".to_string(),
                holon_api::Value::from(holon_api::Region::Main),
            )]),
            marking_delta: holon_api::marking::MarkingDelta::Undeclared,
            guard: holon_api::pattern::OpGuard::None,
            arcs: holon_api::arcs::TransitionArcs::Undeclared,
        },
        OperationDescriptor {
            entity_name: ENTITY_NAME.into(),
            entity_short_name: SHORT_NAME.to_string(),
            id_column: "region".to_string(),
            name: NavigationOp::GoForward.as_str().to_string(),
            display_name: "Go Forward".to_string(),
            description: "Navigate to next view in history".to_string(),
            required_params: vec![region_param()],
            affected_fields: vec!["block_id".to_string()],
            param_mappings: vec![],
            target_scope: holon_api::TargetScope::Global,
            boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
            // Action-bar only: a phone has no keyboard chord for history
            // navigation, while a desktop user already has one, so this is
            // deliberately NOT in the slash menu.
            menu_exposure: holon_api::MenuExposure::Listed {
                surfaces: holon_api::SurfaceSet {
                    slash_menu: false,
                    action_bar: true,
                },
            },
            trigger: None,
            // The bar acts on the main region; binding it here is what lets a
            // tap dispatch without asking the user which region they meant.
            bound_params: ::std::collections::HashMap::from([(
                "region".to_string(),
                holon_api::Value::from(holon_api::Region::Main),
            )]),
            marking_delta: holon_api::marking::MarkingDelta::Undeclared,
            guard: holon_api::pattern::OpGuard::None,
            arcs: holon_api::arcs::TransitionArcs::Undeclared,
        },
        OperationDescriptor {
            entity_name: ENTITY_NAME.into(),
            entity_short_name: SHORT_NAME.to_string(),
            id_column: "region".to_string(),
            name: NavigationOp::GoHome.as_str().to_string(),
            display_name: "Go Home".to_string(),
            description: "Navigate to home view (no block focused)".to_string(),
            required_params: vec![region_param()],
            affected_fields: vec!["block_id".to_string()],
            param_mappings: vec![],
            target_scope: holon_api::TargetScope::Global,
            boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
            // Action-bar only: a phone has no keyboard chord for history
            // navigation, while a desktop user already has one, so this is
            // deliberately NOT in the slash menu.
            menu_exposure: holon_api::MenuExposure::Listed {
                surfaces: holon_api::SurfaceSet {
                    slash_menu: false,
                    action_bar: true,
                },
            },
            trigger: None,
            // The bar acts on the main region; binding it here is what lets a
            // tap dispatch without asking the user which region they meant.
            bound_params: ::std::collections::HashMap::from([(
                "region".to_string(),
                holon_api::Value::from(holon_api::Region::Main),
            )]),
            marking_delta: holon_api::marking::MarkingDelta::Undeclared,
            guard: holon_api::pattern::OpGuard::None,
            arcs: holon_api::arcs::TransitionArcs::Undeclared,
        },
        OperationDescriptor {
            entity_name: ENTITY_NAME.into(),
            entity_short_name: SHORT_NAME.to_string(),
            id_column: "region".to_string(),
            name: NavigationOp::Activate.as_str().to_string(),
            display_name: "Activate Tab".to_string(),
            description: "Move a region's cursor to an already-open history row (tab switch; no \
                          reorder, no scroll reset)"
                .to_string(),
            required_params: vec![
                region_param(),
                OperationParam {
                    name: "history_id".to_string(),
                    type_hint: TypeHint::Number,
                    description: "navigation_history.id of the open tab to activate".to_string(),
                },
            ],
            affected_fields: vec!["history_id".to_string()],
            param_mappings: vec![],
            target_scope: holon_api::TargetScope::Global,
            // Navigation moves only the reader's own view cursor; it never
            // touches a shared container or widens an audience (ADR 0028 A2).
            boundary_behavior: holon_api::BoundaryBehavior::PrivateOnly,
            menu_exposure: holon_api::MenuExposure::NotListed {
                surface: holon_api::NonMenuSurface::Navigation,
            },
            trigger: None,
            bound_params: Default::default(),
            marking_delta: holon_api::marking::MarkingDelta::Undeclared,
            guard: holon_api::pattern::OpGuard::None,
            arcs: holon_api::arcs::TransitionArcs::Undeclared,
        },
        OperationDescriptor {
            entity_name: ENTITY_NAME.into(),
            entity_short_name: SHORT_NAME.to_string(),
            id_column: "region".to_string(),
            name: NavigationOp::OpenTab.as_str().to_string(),
            display_name: "Open in New Tab".to_string(),
            description: "Open a block as an additional tab without closing the region's other \
                          open tabs (modifier-click)"
                .to_string(),
            required_params: vec![
                region_param(),
                OperationParam {
                    name: "block_id".to_string(),
                    type_hint: TypeHint::String,
                    description: "Block ID to open in a new tab".to_string(),
                },
            ],
            affected_fields: vec!["block_id".to_string()],
            param_mappings: vec![],
            target_scope: holon_api::TargetScope::Global,
            // Opens a page in the reader's own view; no sharing boundary
            // crossing (ADR 0028 A2).
            boundary_behavior: holon_api::BoundaryBehavior::PrivateOnly,
            menu_exposure: holon_api::MenuExposure::NotListed {
                surface: holon_api::NonMenuSurface::Navigation,
            },
            trigger: None,
            bound_params: Default::default(),
            marking_delta: holon_api::marking::MarkingDelta::Undeclared,
            guard: holon_api::pattern::OpGuard::None,
            arcs: holon_api::arcs::TransitionArcs::Undeclared,
        },
    ];
    manual_ops
}

/// Is `raw` shaped like something a region can focus?
///
/// A focus root reaches the screen only through `focus_roots JOIN block`, so
/// the one admissible shape is a `block:` URI. Anything else — a web address, a
/// link scheme with no view of its own — silently produced an empty panel,
/// which is how a URL came to travel as a `block_id` at all. `Err` carries the
/// reason as prose the caller splices into its own refusal.
fn focus_target_is_a_block(raw: &str) -> std::result::Result<(), String> {
    let uri = holon_api::EntityUri::parse(raw)
        .map_err(|e| format!("it does not parse as an entity URI ({e})"))?;
    if uri.scheme() != "block" {
        return Err(format!(
            "its scheme is '{}', not 'block' (an external URL or a bare link scheme names no \
             focusable entity)",
            uri.scheme()
        ));
    }
    Ok(())
}

/// Navigation provider for managing region focus state
pub struct NavigationProvider {
    db_handle: DbHandle,
}

impl NavigationProvider {
    pub fn new(db_handle: DbHandle) -> Self {
        Self { db_handle }
    }

    /// Focus on a specific block in a region (Replace semantics).
    ///
    /// Closes the prior open `navigation_history` row in the region (sets
    /// `closed_at`), inserts a new open row, and updates the cursor. Any
    /// forward history is cleared (like browser navigation). The
    /// `focus_roots` matview consumes only `closed_at IS NULL` rows, so the
    /// region converges to "exactly one open row" for main-panel use.
    ///
    /// For pin semantics (sidebar), use `focus_pin` instead — it dedupes
    /// by `(region, block_id)` and refreshes the timestamp.
    async fn focus(&self, region: Region, block_id: Option<&str>) -> Result<OperationResult> {
        tracing::debug!(
            "[NavigationProvider] focus: region={}, block_id={:?}",
            region,
            block_id
        );

        if let Some(id) = block_id {
            focus_target_is_a_block(id).map_err(|why| {
                format!(
                    "navigation.focus: refusing '{id}' as a {region} focus target — {why}. A \
                     focus root is joined to the block table to render, so a target that is not \
                     a block URI renders an EMPTY panel instead of failing."
                )
            })?;
        }

        let mut params = HashMap::new();
        params.insert("region".to_string(), Value::from(region));

        // Step 1: Get current cursor position (Turso doesn't support subqueries in
        // DELETE)
        tracing::debug!("[NavigationProvider] focus: getting current cursor position");
        let current_history_id = self.get_current_history_id(&mut params).await?;
        tracing::debug!(
            "[NavigationProvider] focus: current history_id = {}",
            current_history_id
        );

        // Step 1b: Idempotent re-focus. If the cursor's current row already
        // targets `block_id`, focusing it again must be a no-op — inserting a
        // fresh row would pile up duplicate back-stack entries on repeated
        // clicks AND desync the AUTOINCREMENT history_id that `close`/unpin
        // address by id (a later same-id close then hits the wrong row, leaking
        // a stale open row into `focus_roots`). Mirrors the reference model's
        // `current_focus(region) == block_id` skip in `navigate_focus.rs`.
        let current_block = self
            .db_handle
            .query(
                include_str!("../../sql/navigation/get_history_block.sql"),
                params.clone(),
            )
            .await
            .map_err(|e| format!("Failed to read current focus block: {}", e))?
            .first()
            .and_then(|row| row.get("block_id"))
            .and_then(|v| v.as_string_owned());
        if current_block.as_deref() == block_id {
            tracing::debug!(
                "[NavigationProvider] focus: idempotent re-focus on {:?} — no-op",
                block_id
            );
            return Ok(OperationResult::irreversible(vec![]));
        }

        // Step 2: Delete any forward history (entries after current cursor)
        tracing::debug!("[NavigationProvider] focus: executing DELETE from navigation_history");
        self.db_handle
            .query(
                include_str!("../../sql/navigation/clear_forward_history.sql"),
                params.clone(),
            )
            .await
            .map_err(|e| {
                tracing::debug!("[NavigationProvider] focus: DELETE failed: {}", e);
                format!("Failed to clear forward history: {}", e)
            })?;
        tracing::debug!("[NavigationProvider] focus: DELETE succeeded");

        // Step 2b: Soft-close every open row in this region so the
        // focus_roots matview ends up with only the row we're about to
        // insert. Open rows from prior navigations stay physically present
        // (for back/forward via the cursor) but exit the matview.
        self.db_handle
            .query(
                include_str!("../../sql/navigation/close_open_in_region.sql"),
                params.clone(),
            )
            .await
            .map_err(|e| format!("Failed to close prior open history rows: {}", e))?;

        // Step 3: Insert new history entry (closed_at defaults to NULL → open).
        let block_id_value = match block_id {
            Some(id) => Value::String(id.to_string()),
            None => Value::Null,
        };
        params.insert("block_id".to_string(), block_id_value);

        self.db_handle
            .query(
                include_str!("../../sql/navigation/insert_history.sql"),
                params.clone(),
            )
            .await
            .map_err(|e| format!("Failed to insert navigation history: {}", e))?;

        // Step 4: Get the new max history_id
        let max_result = self
            .db_handle
            .query(
                include_str!("../../sql/navigation/get_max_history_id.sql"),
                params.clone(),
            )
            .await
            .map_err(|e| format!("Failed to get max history id: {}", e))?;

        let new_history_id: i64 = max_result
            .first()
            .and_then(|row| row.get("max_id"))
            .and_then(|v| v.as_i64())
            .ok_or_else(|| "Failed to get new history_id".to_string())?;

        // Step 5: Update cursor to point to new entry
        params.insert("new_id".to_string(), Value::Integer(new_history_id));
        self.db_handle
            .query(
                include_str!("../../sql/navigation/upsert_cursor.sql"),
                params,
            )
            .await
            .map_err(|e| format!("Failed to update navigation cursor: {}", e))?;

        // Step 6: Retention cap — keep only the 100 most recent closed rows
        // per region so unbounded navigation doesn't grow the table (and the
        // CDC/matview state derived from it) forever. Back/forward beyond the
        // window is intentionally forgotten.
        let mut prune_params = HashMap::new();
        prune_params.insert("region".to_string(), Value::from(region));
        let threshold = self
            .db_handle
            .query(
                include_str!("../../sql/navigation/get_prune_threshold.sql"),
                prune_params.clone(),
            )
            .await
            .map_err(|e| format!("Failed to read history prune threshold: {}", e))?
            .first()
            .and_then(|row| row.get("id"))
            .and_then(|v| v.as_i64());
        if let Some(threshold_id) = threshold {
            prune_params.insert("threshold_id".to_string(), Value::Integer(threshold_id));
            self.db_handle
                .query(
                    include_str!("../../sql/navigation/prune_closed_history.sql"),
                    prune_params,
                )
                .await
                .map_err(|e| format!("Failed to prune closed history rows: {}", e))?;
        }

        tracing::debug!("[NavigationProvider] focus: completed successfully");
        Ok(OperationResult::irreversible(vec![]))
    }

    /// Pin a block to a region (Pin semantics, e.g. shift+click → right
    /// sidebar).
    ///
    /// Move-to-top dedup: if an open row for `(region, block_id)` already
    /// exists, refresh its timestamp so it sorts to the top of focus_roots.
    /// Otherwise insert a fresh open row. Cursor is left untouched — pins
    /// are not part of the back/forward stack.
    ///
    /// Implementation note: `db_handle.query()` returns an empty `Vec` for
    /// UPDATE statements regardless of rows-affected, so we can't infer
    /// "matched / not matched" from its result. Instead we SELECT first to
    /// check existence, then branch into UPDATE-or-INSERT. Two round-trips
    /// per click is acceptable for a UI-driven pin operation.
    async fn focus_pin(&self, region: Region, block_id: &str) -> Result<OperationResult> {
        tracing::debug!(
            "[NavigationProvider] focus_pin: region={}, block_id={}",
            region,
            block_id
        );

        let mut params = HashMap::new();
        params.insert("region".to_string(), Value::from(region));
        params.insert("block_id".to_string(), Value::String(block_id.to_string()));

        // Step 1: check whether an open pin already exists for (region, block_id).
        let existing = self
            .db_handle
            .query(
                "SELECT id FROM navigation_history WHERE region = $region AND block_id = \
                 $block_id AND closed_at IS NULL LIMIT 1",
                params.clone(),
            )
            .await
            .map_err(|e| format!("Failed to look up existing pin: {}", e))?;

        if existing.is_empty() {
            // Step 2a: no existing pin → insert.
            self.db_handle
                .query(
                    include_str!("../../sql/navigation/insert_history.sql"),
                    params.clone(),
                )
                .await
                .map_err(|e| format!("Failed to insert pin: {}", e))?;
            tracing::debug!("[NavigationProvider] focus_pin: inserted new pin");
        } else {
            // Step 2b: existing pin → bump its timestamp (move-to-top).
            self.db_handle
                .query(
                    include_str!("../../sql/navigation/update_pin_timestamp.sql"),
                    params.clone(),
                )
                .await
                .map_err(|e| format!("Failed to refresh pin timestamp: {}", e))?;
            tracing::debug!("[NavigationProvider] focus_pin: refreshed existing pin");
        }

        Ok(OperationResult::irreversible(vec![]))
    }

    /// Soft-close a specific navigation_history row by id, then follow the
    /// cursor if that row was a region's active tab.
    ///
    /// Used by the sidebar / tab-strip X button. `close` carries only the row
    /// handle (no region), so it looks the region up and, when the closed row
    /// IS that region's `navigation_cursor` target, moves the cursor to a
    /// still-open neighbor — LEFT preferred (the tab before it in stable
    /// insertion order), then RIGHT. With no open tab left, the cursor row is
    /// dropped so the cursor-joined main panel falls through to its default
    /// render instead of pinning a closed row (a blank panel). Closing a
    /// NON-active tab leaves the cursor untouched.
    async fn close(&self, history_id: i64) -> Result<OperationResult> {
        tracing::debug!("[NavigationProvider] close: history_id={}", history_id);

        let mut id_params = HashMap::new();
        id_params.insert("history_id".to_string(), Value::Integer(history_id));

        // One read: the region owning this row + that region's current cursor
        // target. A missing row means it is already gone — nothing to close or
        // follow.
        let row = self
            .db_handle
            .query(
                include_str!("../../sql/navigation/get_row_region_and_cursor.sql"),
                id_params.clone(),
            )
            .await
            .map_err(|e| {
                format!("Failed to look up region/cursor for history row {history_id}: {e}")
            })?;
        let region: Option<String> = row
            .first()
            .and_then(|r| r.get("region"))
            .and_then(|v| v.as_string_owned());
        let active: Option<i64> = row
            .first()
            .and_then(|r| r.get("cursor_id"))
            .and_then(|v| v.as_i64());

        // Soft-close the row (drops it from focus_roots via CDC).
        self.db_handle
            .query(
                include_str!("../../sql/navigation/close_history_id.sql"),
                id_params,
            )
            .await
            .map_err(|e| format!("Failed to close history row: {}", e))?;

        let Some(region) = region else {
            return Ok(OperationResult::irreversible(vec![]));
        };
        let region_val = Value::String(region.clone());

        // Cursor-follow only when the closed row WAS this region's active tab.
        if active != Some(history_id) {
            return Ok(OperationResult::irreversible(vec![]));
        }

        // Cursor-follow: LEFT neighbor first, then RIGHT.
        let mut neighbor_params = HashMap::new();
        neighbor_params.insert("region".to_string(), region_val.clone());
        neighbor_params.insert("history_id".to_string(), Value::Integer(history_id));
        let mut neighbor = self
            .db_handle
            .query(
                include_str!("../../sql/navigation/left_neighbor_open_tab.sql"),
                neighbor_params.clone(),
            )
            .await
            .map_err(|e| format!("Failed to find left neighbor tab: {e}"))?
            .first()
            .and_then(|row| row.get("id"))
            .and_then(|v| v.as_i64());
        if neighbor.is_none() {
            neighbor = self
                .db_handle
                .query(
                    include_str!("../../sql/navigation/right_neighbor_open_tab.sql"),
                    neighbor_params,
                )
                .await
                .map_err(|e| format!("Failed to find right neighbor tab: {e}"))?
                .first()
                .and_then(|row| row.get("id"))
                .and_then(|v| v.as_i64());
        }

        match neighbor {
            Some(neighbor_id) => {
                let mut set_params = HashMap::new();
                set_params.insert("region".to_string(), region_val);
                set_params.insert("history_id".to_string(), Value::Integer(neighbor_id));
                self.db_handle
                    .query(
                        include_str!("../../sql/navigation/set_cursor_to_history.sql"),
                        set_params,
                    )
                    .await
                    .map_err(|e| format!("Failed to move cursor to neighbor tab: {e}"))?;
            }
            None => {
                let mut delete_params = HashMap::new();
                delete_params.insert("region".to_string(), region_val);
                self.db_handle
                    .query(
                        include_str!("../../sql/navigation/delete_cursor.sql"),
                        delete_params,
                    )
                    .await
                    .map_err(|e| format!("Failed to clear cursor after last tab: {e}"))?;
            }
        }

        Ok(OperationResult::irreversible(vec![]))
    }

    /// Set the region's cursor to an already-open history row (tab switch).
    ///
    /// Moves ONLY the cursor — no insert, no close, no reorder — so the open
    /// set keeps its stable insertion order and `main_nav_generation` is not
    /// bumped (per-tab scroll survives). The main panel query filters
    /// `focus_roots` by this cursor, so activating flips which open tab
    /// renders. See ADR-0026 tab model (Q3 stable order, risk register #1/#3).
    async fn activate(&self, region: Region, history_id: i64) -> Result<OperationResult> {
        tracing::debug!(
            "[NavigationProvider] activate: region={}, history_id={}",
            region,
            history_id
        );

        let mut params = HashMap::new();
        params.insert("region".to_string(), Value::from(region));
        params.insert("history_id".to_string(), Value::Integer(history_id));

        self.db_handle
            .query(
                include_str!("../../sql/navigation/set_cursor_to_history.sql"),
                params,
            )
            .await
            .map_err(|e| format!("Failed to activate history row {history_id}: {e}"))?;

        // The cursor-on-open-row invariant otherwise rests on activate intents
        // being minted only from open focus_roots rows; a stale/racy intent for
        // a just-closed tab would blank the panel through the same join-break
        // go_back had — guard it like go_back/go_forward.
        self.assert_cursor_on_open_row(region).await?;

        Ok(OperationResult::irreversible(vec![]))
    }

    /// Open a block as an ADDITIONAL open tab in a region (multi-open).
    ///
    /// Idempotent by open row: if `(region, block_id)` is already open, point
    /// the cursor at that existing tab (no duplicate row). Otherwise insert a
    /// new open `navigation_history` row and point the cursor at it — WITHOUT
    /// closing the region's other open rows (that is `focus`'s replace
    /// semantics). The sole multi-open producer (ADR-0026 tab model, Q2).
    async fn open_tab(&self, region: Region, block_id: &str) -> Result<OperationResult> {
        tracing::debug!(
            "[NavigationProvider] open_tab: region={}, block_id={}",
            region,
            block_id
        );

        let mut params = HashMap::new();
        params.insert("region".to_string(), Value::from(region));
        params.insert("block_id".to_string(), Value::String(block_id.to_string()));

        // Already open? → activate that tab rather than inserting a duplicate.
        let existing = self
            .db_handle
            .query(
                include_str!("../../sql/navigation/get_open_history_id.sql"),
                params.clone(),
            )
            .await
            .map_err(|e| format!("Failed to look up open tab: {e}"))?;
        if let Some(id) = existing
            .first()
            .and_then(|row| row.get("id"))
            .and_then(|v| v.as_i64())
        {
            tracing::debug!(
                "[NavigationProvider] open_tab: {block_id} already open (id={id}) — activating"
            );
            return self.activate(region, id).await;
        }

        // Insert a new open row (closed_at defaults NULL → open) WITHOUT
        // closing the region's other open rows.
        self.db_handle
            .query(
                include_str!("../../sql/navigation/insert_history.sql"),
                params.clone(),
            )
            .await
            .map_err(|e| format!("Failed to insert open tab: {e}"))?;

        let max_result = self
            .db_handle
            .query(
                include_str!("../../sql/navigation/get_max_history_id.sql"),
                params.clone(),
            )
            .await
            .map_err(|e| format!("Failed to get max history id: {e}"))?;
        let new_history_id: i64 = max_result
            .first()
            .and_then(|row| row.get("max_id"))
            .and_then(|v| v.as_i64())
            .ok_or_else(|| "Failed to get new history_id after open_tab".to_string())?;

        params.insert("new_id".to_string(), Value::Integer(new_history_id));
        self.db_handle
            .query(
                include_str!("../../sql/navigation/upsert_cursor.sql"),
                params,
            )
            .await
            .map_err(|e| format!("Failed to update cursor after open_tab: {e}"))?;

        Ok(OperationResult::irreversible(vec![]))
    }

    /// Go back in navigation history
    async fn go_back(&self, region: Region) -> Result<OperationResult> {
        tracing::debug!("[NavigationProvider] go_back: region={}", region);

        let mut params = HashMap::new();
        params.insert("region".to_string(), Value::from(region));

        let current_id = self.get_current_history_id(&mut params).await?;

        // Find the previous history entry (traversal ordered by id — unchanged).
        let prev_result = self
            .db_handle
            .query(
                include_str!("../../sql/navigation/get_previous_entry.sql"),
                params.clone(),
            )
            .await
            .map_err(|e| format!("Failed to find previous entry: {}", e))?;

        if let Some(prev_id) = prev_result
            .first()
            .and_then(|row| row.get("id"))
            .and_then(|v| v.as_i64())
        {
            // Write-side invariant (ruled option (a)): the main-panel focus query
            // joins `focus_roots` (which is `navigation_history WHERE closed_at IS
            // NULL` — open rows only) to `navigation_cursor` on `history_id`, so
            // the cursor MUST land on an OPEN row. The back target was soft-closed
            // by the forward `focus_replace` that superseded it; re-open it and
            // close the departed row so the panel stays populated across back-nav
            // (before this, the cursor pointed at a closed row → 0-row focus query
            // → blank panel → creation-slot `0 live rows` panic).
            self.reopen_target_close_departed(region, current_id, prev_id)
                .await?;
            tracing::debug!(
                "[NavigationProvider] go_back: moved to history_id={prev_id} (target re-opened)"
            );
        } else {
            // No previous entry — go to home (NULL cursor is a LEGAL invariant
            // state: the panel intentionally falls through to default render).
            self.db_handle
                .query(
                    include_str!("../../sql/navigation/nullify_cursor.sql"),
                    params,
                )
                .await
                .map_err(|e| format!("Failed to go back to home: {}", e))?;
            tracing::debug!("[NavigationProvider] go_back: went to home (no previous entry)");
        }

        self.assert_cursor_on_open_row(region).await?;
        Ok(OperationResult::irreversible(vec![]))
    }

    /// Go forward in navigation history
    async fn go_forward(&self, region: Region) -> Result<OperationResult> {
        tracing::debug!("[NavigationProvider] go_forward: region={}", region);

        let mut params = HashMap::new();
        params.insert("region".to_string(), Value::from(region));

        let current_id = self.get_current_history_id(&mut params).await?;

        // Find the next history entry
        let next_result = self
            .db_handle
            .query(
                include_str!("../../sql/navigation/get_next_entry.sql"),
                params.clone(),
            )
            .await
            .map_err(|e| format!("Failed to find next entry: {}", e))?;

        // Step 3: Re-open + re-point the cursor if a next entry exists.
        if let Some(next_id) = next_result
            .first()
            .and_then(|row| row.get("id"))
            .and_then(|v| v.as_i64())
        {
            // Same write-side invariant as go_back: the forward target was
            // soft-closed when we navigated back past it, so re-open it and close
            // the departed row so the cursor lands on an OPEN focus_roots row.
            self.reopen_target_close_departed(region, current_id, next_id)
                .await?;
            tracing::debug!(
                "[NavigationProvider] go_forward: moved to history_id={next_id} (target re-opened)"
            );
        } else {
            tracing::debug!(
                "[NavigationProvider] go_forward: no next entry, staying at current position"
            );
        }

        self.assert_cursor_on_open_row(region).await?;
        Ok(OperationResult::irreversible(vec![]))
    }

    /// Navigate to home (root view, no specific block focused)
    async fn go_home(&self, region: Region) -> Result<OperationResult> {
        self.focus(region, None).await
    }

    /// Get the current cursor history_id for a region, returning 0 if none
    /// exists.
    async fn get_current_history_id(&self, params: &mut HashMap<String, Value>) -> Result<i64> {
        let cursor_result = self
            .db_handle
            .query(
                include_str!("../../sql/navigation/get_cursor.sql"),
                params.clone(),
            )
            .await
            .map_err(|e| format!("Failed to get cursor position: {}", e))?;

        let current_history_id: i64 = cursor_result
            .first()
            .and_then(|row| row.get("history_id"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);

        params.insert("current_id".to_string(), Value::Integer(current_history_id));
        Ok(current_history_id)
    }

    /// Move the region's cursor to a back/forward `target_id`, re-opening it
    /// and soft-closing the `departed_id` in ONE transaction (write-side
    /// invariant, ruled option (a)).
    ///
    /// The back/forward targets are soft-CLOSED rows (`focus_replace` closes
    /// the prior open focus on every forward navigation), but the
    /// main-panel focus query joins `focus_roots` — `navigation_history
    /// WHERE closed_at IS NULL` — to `navigation_cursor` on `history_id`,
    /// so pointing the cursor at a closed row yields a 0-row focus query
    /// and a blank panel. Re-opening the target keeps the cursor on an OPEN
    /// row; closing the departed row keeps the main region at "exactly one
    /// open focus row". `closed_at` is a DISPLAY flag (in-vs-out of
    /// `focus_roots`), not a departure timestamp — reopening only
    /// flips it and never renumbers ids, so back/forward traversal (ordered by
    /// `id`) is undisturbed.
    ///
    /// Atomicity is mandatory: the fork's deferred-FK/autocommit wart means a
    /// multi-statement write MUST go through `transaction()` (a mid-sequence
    /// autocommit would expose the empty-join intermediate the panel must never
    /// observe).
    async fn reopen_target_close_departed(
        &self,
        region: Region,
        departed_id: i64,
        target_id: i64,
    ) -> Result<()> {
        let region_str = region.as_str().to_string();
        self.db_handle
            .transaction(vec![
                // Close the departed focus row (guarded on still-open; a no-op at
                // home, where `departed_id` resolves to no row).
                (
                    "UPDATE navigation_history SET closed_at = datetime('now') \
                     WHERE id = ? AND closed_at IS NULL"
                        .to_string(),
                    vec![turso::Value::Integer(departed_id)],
                ),
                // Re-open the back/forward target so `focus_roots` tracks it.
                (
                    "UPDATE navigation_history SET closed_at = NULL WHERE id = ?".to_string(),
                    vec![turso::Value::Integer(target_id)],
                ),
                // Point the cursor at the (now open) target.
                (
                    "UPDATE navigation_cursor SET history_id = ? WHERE region = ?".to_string(),
                    vec![
                        turso::Value::Integer(target_id),
                        turso::Value::Text(region_str),
                    ],
                ),
            ])
            .await
            .map_err(|e| {
                format!(
                    "Failed to move nav cursor (region={region}, from={departed_id}, \
                     to={target_id}): {e}"
                )
                .into()
            })
    }

    /// Fail-loud per-region invariant (ruled option (a)): `navigation_cursor`
    /// must point at an OPEN `navigation_history` row (`closed_at IS NULL`) —
    /// or have NO row for the region, or a NULL (home) cursor. A cursor on
    /// an open `NavigateHome` row (`block_id NULL`) is LEGAL
    /// (open-but-not-focused; the panel falls through to default render),
    /// so the check is "cursor row is OPEN", never "cursor row is in
    /// focus_roots".
    ///
    /// A violation means the main panel would silently blank (the `focus_roots`
    /// matview excludes closed rows, so the panel query's
    /// `nc.history_id = fr.history_id` join yields nothing). Called after every
    /// cursor move in this provider's back/forward ops. `focus` / `activate` /
    /// `open_tab` / `close` maintain the invariant by construction (they only
    /// ever point the cursor at a freshly-inserted or already-open row, or
    /// delete the cursor row).
    async fn assert_cursor_on_open_row(&self, region: Region) -> Result<()> {
        let mut params = HashMap::new();
        params.insert("region".to_string(), Value::from(region));
        let rows = self
            .db_handle
            .query(
                "SELECT nc.history_id AS history_id, nh.closed_at AS closed_at \
                 FROM navigation_cursor nc \
                 LEFT JOIN navigation_history nh ON nh.id = nc.history_id \
                 WHERE nc.region = $region",
                params,
            )
            .await
            .map_err(|e| format!("cursor-invariant check failed (region={region}): {e}"))?;
        let Some(row) = rows.first() else {
            return Ok(()); // no cursor row for the region — legal (never navigated).
        };
        // NULL cursor = home — legal.
        if !matches!(row.get("history_id"), Some(Value::Integer(_))) {
            return Ok(());
        }
        // `closed_at` is a TEXT display flag: non-NULL (String/DateTime) = CLOSED.
        if matches!(
            row.get("closed_at"),
            Some(Value::String(_)) | Some(Value::DateTime(_))
        ) {
            let hid = row.get("history_id").and_then(|v| v.as_i64()).unwrap_or(-1);
            return Err(format!(
                "[NavigationProvider] cursor invariant violated: region={region} \
                 navigation_cursor points at CLOSED navigation_history row id={hid}. Closed rows \
                 are excluded from the focus_roots matview, so the main panel would blank. Every \
                 cursor move must land on an OPEN row (or a NULL/home cursor, or no row)."
            )
            .into());
        }
        Ok(())
    }
}

#[async_trait]
impl OperationProvider for NavigationProvider {
    fn operations(&self) -> Vec<OperationDescriptor> {
        navigation_operation_descriptors()
    }

    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: StorageEntity,
    ) -> Result<OperationResult> {
        if entity_name != ENTITY_NAME {
            return Err(format!(
                "NavigationProvider: expected entity '{}', got '{}'",
                ENTITY_NAME, entity_name
            )
            .into());
        }

        // `close` takes only `history_id` (no region) — handle before region
        // extraction.
        if op_name == NavigationOp::Close.as_str() {
            let history_id = params
                .get("history_id")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| "Missing required parameter 'history_id'".to_string())?;
            return self.close(history_id).await;
        }

        let region: Region = params
            .get("region")
            .cloned()
            .ok_or("Missing required parameter 'region'")?
            .try_into()
            .map_err(|e: Box<dyn std::error::Error + Send + Sync>| e.to_string())?;

        match NavigationOp::from_str(op_name) {
            Ok(NavigationOp::Focus) => {
                let block_id = params.get("block_id").and_then(|v| match v {
                    Value::String(s) => Some(s.as_str()),
                    _ => None,
                });
                self.focus(region, block_id).await
            }
            Ok(NavigationOp::FocusPin) => {
                let block_id = params
                    .get("block_id")
                    .and_then(|v| match v {
                        Value::String(s) => Some(s.as_str()),
                        _ => None,
                    })
                    .ok_or_else(|| "Missing required parameter 'block_id'".to_string())?;
                self.focus_pin(region, block_id).await
            }
            Ok(NavigationOp::GoBack) => self.go_back(region).await,
            Ok(NavigationOp::GoForward) => self.go_forward(region).await,
            Ok(NavigationOp::GoHome) => self.go_home(region).await,
            Ok(NavigationOp::Activate) => {
                let history_id = params
                    .get("history_id")
                    .and_then(|v| v.as_i64())
                    .ok_or_else(|| "Missing required parameter 'history_id'".to_string())?;
                self.activate(region, history_id).await
            }
            Ok(NavigationOp::OpenTab) => {
                let block_id = params
                    .get("block_id")
                    .and_then(|v| match v {
                        Value::String(s) => Some(s.as_str()),
                        _ => None,
                    })
                    .ok_or_else(|| "Missing required parameter 'block_id'".to_string())?;
                self.open_tab(region, block_id).await
            }
            // `close` is dispatched before region extraction above.
            Ok(NavigationOp::Close) => {
                unreachable!("close is handled before region extraction")
            }
            Err(_) => Err(format!(
                "NavigationProvider: unknown operation '{op_name}' for entity '{ENTITY_NAME}'"
            )
            .into()),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_operations_defined() {
        // Just verify operations are defined correctly
        // Full integration tests require DbHandle
        let ops = vec!["focus", "go_back", "go_forward", "go_home"];
        for op in ops {
            assert!(
                ["focus", "go_back", "go_forward", "go_home"].contains(&op),
                "Operation {} should be defined",
                op
            );
        }
    }
}
