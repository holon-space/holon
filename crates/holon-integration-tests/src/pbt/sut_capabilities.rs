//! Blanket `SutLoro` + `SutLoroLog` + `SutLifecycle` + Phase 6c-g impls on `E2ESut<V>`.
//!
//! Follows the same pattern as `reference_capabilities.rs`:
//! thin forwarding impls that expose capability-trait surface
//! over existing inherent / `SutHandle` methods.

use std::collections::{BTreeSet, HashSet};

use holon_pbt_core::capabilities::{
    CapBlockId, SutCdc, SutDriver, SutLayout, SutLifecycle, SutLoro, SutLoroLog, SutLoroTaskState,
    SutOrgFileWrite, SutOrgRender, SutQueryCompile, SutRenderer, SutSqlProjection, SutViewModel,
};

use super::sut::E2ESut;
use super::transition_dispatch::SutHandle;
use super::transitions::{PeerEditOp, TextOp};
use super::types::VariantMarker;
use holon_frontend::reactive::BuilderServices;

#[allow(async_fn_in_trait)]
impl<V: VariantMarker> SutLoro for E2ESut<V> {
    async fn apply_add_peer(&mut self) {
        <E2ESut<V> as SutHandle>::apply_add_peer(self).await;
    }

    async fn apply_peer_create(
        &mut self,
        peer_idx: usize,
        parent_stable_id: Option<&str>,
        content: &str,
        stable_id: &str,
    ) {
        <E2ESut<V> as SutHandle>::apply_peer_edit(
            self,
            peer_idx,
            &PeerEditOp::Create {
                parent_stable_id: parent_stable_id.map(str::to_owned),
                content: content.to_owned(),
                stable_id: stable_id.to_owned(),
            },
        )
        .await;
    }

    async fn apply_peer_update(&mut self, peer_idx: usize, stable_id: &str, content: &str) {
        <E2ESut<V> as SutHandle>::apply_peer_edit(
            self,
            peer_idx,
            &PeerEditOp::Update {
                stable_id: stable_id.to_owned(),
                content: content.to_owned(),
            },
        )
        .await;
    }

    async fn apply_peer_delete(&mut self, peer_idx: usize, stable_id: &str) {
        <E2ESut<V> as SutHandle>::apply_peer_edit(
            self,
            peer_idx,
            &PeerEditOp::Delete {
                stable_id: stable_id.to_owned(),
            },
        )
        .await;
    }

    async fn apply_peer_char_insert(
        &mut self,
        peer_idx: usize,
        stable_id: &str,
        pos_codepoint: usize,
        text: &str,
    ) {
        <E2ESut<V> as SutHandle>::apply_peer_char_edit(
            self,
            peer_idx,
            stable_id,
            &TextOp::Insert {
                pos_codepoint,
                text: text.to_owned(),
            },
        )
        .await;
    }

    async fn apply_peer_char_delete(
        &mut self,
        peer_idx: usize,
        stable_id: &str,
        pos_codepoint: usize,
        len_codepoint: usize,
    ) {
        <E2ESut<V> as SutHandle>::apply_peer_char_edit(
            self,
            peer_idx,
            stable_id,
            &TextOp::Delete {
                pos_codepoint,
                len_codepoint,
            },
        )
        .await;
    }

    async fn apply_sync_with_peer(&mut self, peer_idx: usize) {
        <E2ESut<V> as SutHandle>::apply_sync_with_peer(self, peer_idx).await;
    }

    async fn apply_merge_from_peer(&mut self, peer_idx: usize) {
        <E2ESut<V> as SutHandle>::apply_merge_from_peer(self, peer_idx).await;
    }

    /// Not wired: the capability trait models lag-based stale peers
    /// (`lag_steps: usize`) but `E2ESut::apply_create_stale_loro` takes
    /// `(org_filename, LoroCorruptionType)` — a pre-startup file-corruption
    /// concept. Phase 7 will decide whether to reconcile the two models or
    /// keep them separate. Until then this panics loudly if called.
    async fn apply_create_stale_loro(&mut self, _: usize) {
        unimplemented!(
            "SutLoro::apply_create_stale_loro on E2ESut: lag_steps-based peer snapshots \
             are not wired yet. The underlying SUT method takes (org_filename, \
             LoroCorruptionType) — a different model. Wire in Phase 7 once the \
             semantics are reconciled."
        )
    }
}

// ─── SutLifecycle ─────────────────────────────────────────────────────

#[allow(async_fn_in_trait)]
impl<V: VariantMarker> SutLifecycle for E2ESut<V> {
    async fn apply_start_app(&mut self) {
        self.ctx
            .start_app(true)
            .await
            .expect("SutLifecycle::apply_start_app failed");
    }

    /// Triggers a restart simulation with an empty expected-ids set. The
    /// wide-PBT transition passes the full expected set from `ref_state`;
    /// lifecycle callers that don't have ref_state use the empty set so
    /// `simulate_restart` still clears and re-syncs Loro without blocking
    /// on block convergence.
    async fn apply_simulate_restart(&mut self) {
        self.ctx
            .simulate_restart(&HashSet::new())
            .await
            .expect("SutLifecycle::apply_simulate_restart failed");
    }

    async fn is_app_started(&self) -> bool {
        self.ctx.is_running()
    }
}

// ─── SutLoroLog ───────────────────────────────────────────────────────

#[allow(async_fn_in_trait)]
impl<V: VariantMarker> SutLoroLog for E2ESut<V> {
    /// Delegates to `TestContext::loro_sync_error_count`, which reads the
    /// `LoroSyncController`'s atomic error counter. Same source as the
    /// `inv-loro-no-errors` check in `check_invariants_async`.
    async fn loro_had_errors(&self) -> bool {
        self.ctx.loro_sync_error_count() > 0
    }

    /// Not wired: requires access to the LoroSyncController's internal tree
    /// snapshot, which today is only read inside `check_invariants_async` via
    /// the controller's `tree_children` helper. Phase 7 will expose this
    /// through `TestContext` once `inv-live-children-match-ref` migrates onto
    /// `SutLoroLog`.
    async fn loro_children_of(&self, _: &str) -> Option<Vec<String>> {
        unimplemented!(
            "SutLoroLog::loro_children_of on E2ESut: Loro tree child snapshot is not yet \
             exposed through TestContext. Wire in Phase 7 alongside the \
             inv-live-children-match-ref invariant migration."
        )
    }
}

// ─── SutLoroTaskState ─────────────────────────────────────────────────

#[allow(async_fn_in_trait)]
impl<V: VariantMarker> SutLoroTaskState for E2ESut<V> {
    /// Not yet wired: projecting `task_state` out of Loro tags requires
    /// exposing the LoroSyncController's property map through `TestContext`,
    /// which is deferred to Phase 8 alongside `inv-live-children-match-ref`.
    async fn loro_task_state_of(&self, _: &str) -> Option<String> {
        unimplemented!(
            "SutLoroTaskState::loro_task_state_of on E2ESut: Loro tag \
             property projection is not yet exposed through TestContext. \
             Wire in Phase 8 when LoroSyncController tag-read surface lands."
        )
    }
}

// ─── SutSqlProjection ─────────────────────────────────────────────────

#[allow(async_fn_in_trait)]
impl<V: VariantMarker> SutSqlProjection for E2ESut<V> {
    /// Queries the `block` materialized view for a single row and returns its
    /// fields as strings. Same data path as `check_invariants_async`'s
    /// `inv-backend-blocks-match-ref` block matview read (sut.rs:4807).
    async fn block_row(&self, id: &CapBlockId) -> Option<Vec<String>> {
        let escaped = id.replace('\'', "''");
        let sql = format!("SELECT * FROM block WHERE id = '{escaped}'");
        let rows = self
            .ctx
            .query_sql(&sql)
            .await
            .expect("SutSqlProjection::block_row query failed");
        rows.into_iter().next().map(|row| {
            let mut fields: Vec<String> = row
                .into_values()
                .map(|v| v.as_string().unwrap_or_default().to_string())
                .collect();
            fields.sort();
            fields
        })
    }

    /// Returns all non-deleted block IDs from `block_raw`. Mirrors the
    /// `SELECT id FROM block_raw` query in `check_invariants_async`
    /// (sut.rs:4228).
    async fn all_block_ids(&self) -> BTreeSet<CapBlockId> {
        let rows = self
            .ctx
            .query_sql("SELECT id FROM block_raw")
            .await
            .expect("SutSqlProjection::all_block_ids query failed");
        rows.into_iter()
            .filter_map(|r| r.get("id").and_then(|v| v.as_string()).map(str::to_string))
            .collect()
    }

    /// Returns the current row count for a registered CDC watch via
    /// `TestContext::ui_model`. Returns `None` when the query_id is not
    /// registered.
    async fn watch_row_count(&self, query_id: &str) -> Option<usize> {
        self.ctx.ui_model.get(query_id).map(|acc| acc.len())
    }

    /// Queries `block_raw` (write-side base table, no matview hydration) for a
    /// single row. Used by the WARN/SKIP CDC-lag classifier in
    /// `check_invariants_async` (sut.rs:3189–3206 pattern).
    async fn block_raw_row(&self, id: &CapBlockId) -> Option<Vec<String>> {
        let escaped = id.replace('\'', "''");
        let sql = format!("SELECT * FROM block_raw WHERE id = '{escaped}'");
        let rows = self
            .ctx
            .query_sql(&sql)
            .await
            .expect("SutSqlProjection::block_raw_row query failed");
        rows.into_iter().next().map(|row| {
            let mut fields: Vec<String> = row
                .into_values()
                .map(|v| v.as_string().unwrap_or_default().to_string())
                .collect();
            fields.sort();
            fields
        })
    }

    /// Returns all distinct block_id values from `block_tags`. Mirrors
    /// `SELECT DISTINCT block_id FROM block_tags` — same table used by
    /// `inv-block-tags-references-exist`.
    async fn block_tag_block_ids(&self) -> BTreeSet<CapBlockId> {
        let rows = self
            .ctx
            .query_sql("SELECT DISTINCT block_id FROM block_tags")
            .await
            .expect("SutSqlProjection::block_tag_block_ids query failed");
        rows.into_iter()
            .filter_map(|r| {
                r.get("block_id")
                    .and_then(|v| v.as_string())
                    .map(str::to_string)
            })
            .collect()
    }

    /// Reads `json_extract(properties, '$.task_state')` from `block_raw`
    /// for the given block id. Returns `None` when the block doesn't exist
    /// or the property is absent/null.
    async fn block_task_state(&self, id: &CapBlockId) -> Option<String> {
        let escaped = id.replace('\'', "''");
        let sql = format!(
            "SELECT json_extract(properties, '$.task_state') AS task_state \
             FROM block_raw WHERE id = '{escaped}'"
        );
        let rows = self
            .ctx
            .query_sql(&sql)
            .await
            .expect("SutSqlProjection::block_task_state query failed");
        rows.into_iter().next().and_then(|r| {
            r.get("task_state")
                .and_then(|v| v.as_string())
                .map(str::to_string)
        })
    }
}

// ─── SutOrgFileWrite ──────────────────────────────────────────────────

#[allow(async_fn_in_trait)]
impl<V: VariantMarker> SutOrgFileWrite for E2ESut<V> {
    /// Delegates to `SutHandle::apply_write_org_file`, which writes the file
    /// via `TestContext::write_org_file` and — when the app is running — waits
    /// for `OrgSyncController` to ingest it and re-key `ctx.documents`.
    async fn write_org_file(&mut self, path: &str, contents: &str) {
        <E2ESut<V> as SutHandle>::apply_write_org_file(self, path, contents).await;
    }
}

// ─── SutCdc ───────────────────────────────────────────────────────────

#[allow(async_fn_in_trait)]
impl<V: VariantMarker> SutCdc for E2ESut<V> {
    /// True when the `live_blocks` CDC mirror has not yet consumed all events
    /// emitted since the last write. Delegates to `E2ESut::live_blocks_cdc_stale`
    /// (pub(super) accessor): compares `LiveData::consumed_seq()` against
    /// `db_handle().cdc_emitted_watermark()`. Returns `false` pre-startup or
    /// before the mirror is initialised.
    async fn cdc_in_flight(&self) -> bool {
        self.live_blocks_cdc_stale()
    }

    /// Drains pending CDC events from all active watches into the `ui_model`.
    /// Delegates to `TestContext::drain_cdc_events` — same logic used between
    /// transitions in `check_invariants_async` (sut.rs:3965, 4005).
    async fn drain_cdc(&mut self) {
        self.ctx.drain_cdc_events().await;
    }
}

// ─── SutViewModel ─────────────────────────────────────────────────────

#[allow(async_fn_in_trait)]
impl<V: VariantMarker> SutViewModel for E2ESut<V> {
    /// Drains all pending ViewModel emissions since the last call.
    /// Delegates to `E2ESut::vm_emissions_drain` (pub(super) accessor).
    /// Each ViewModel is rendered via `pretty_print(0)` — the same format
    /// used by `render_tree_of` for consistent slice-level string comparisons.
    async fn drain_vm_emissions(&mut self) -> Vec<String> {
        self.vm_emissions_drain()
            .into_iter()
            .map(|vm| vm.pretty_print(0))
            .collect()
    }

    /// True if the frontend root ViewModel is the Error variant.
    /// Mirrors `inv-frontend-root-not-error` (sut.rs:6038–6047):
    /// snapshots the root URI via the frontend engine and checks
    /// `widget_name() == "error"`. Uses `frontend_engine` (pub) and falls
    /// back to `false` when none is installed. `reactive_root_id` (private)
    /// is not reachable here, so uses `root_layout_block_uri` as the root.
    async fn frontend_root_is_error(&self) -> bool {
        let Some(engine) = self.frontend_engine.clone() else {
            return false;
        };
        let root_uri = holon_api::root_layout_block_uri();
        let vm = engine.snapshot(&root_uri);
        vm.widget_name() == Some("error")
    }
}

// ─── SutRenderer ──────────────────────────────────────────────────────

#[allow(async_fn_in_trait)]
impl<V: VariantMarker> SutRenderer for E2ESut<V> {
    /// Returns a debug-formatted render-tree string for `id` via
    /// `frontend_engine` (pub). `reactive_engine` (private, sut.rs:291)
    /// would provide a fallback path, but is not accessible here.
    /// Add a pub accessor in Phase 7 to wire the headless fallback.
    async fn render_tree_of(&self, id: &CapBlockId) -> Option<String> {
        let engine = self.frontend_engine.clone()?;
        let uri = holon_api::EntityUri::parse(id).ok()?;
        let rqr = engine.ensure_watching(&uri);
        if rqr.is_loading() {
            return None;
        }
        let (render_expr, data_rows) = rqr.snapshot();
        let services =
            holon_frontend::reactive::HeadlessBuilderServices::new(self.engine().clone());
        let vm = holon_frontend::interpret_pure(&render_expr, &data_rows, &services).snapshot();
        Some(vm.pretty_print(0))
    }

    /// Build a frontend-agnostic [`WidgetSnapshot`] from the ViewModel
    /// rooted at the layout root. Mirrors the path
    /// `render_tree_of` uses (interpret_pure against the layout root) but
    /// returns the structured snapshot instead of a pretty-printed string.
    async fn widget_tree_snapshot(&self) -> holon_pbt_core::capabilities::WidgetSnapshot {
        let empty = || holon_pbt_core::capabilities::WidgetSnapshot {
            kind: "empty".into(),
            entity_id: None,
            props: Default::default(),
            operations: Vec::new(),
            children: Vec::new(),
        };
        let Some(engine) = self.frontend_engine.clone() else {
            return empty();
        };
        let root_uri = holon_api::root_layout_block_uri();
        let rqr = engine.ensure_watching(&root_uri);
        if rqr.is_loading() {
            return empty();
        }
        let (render_expr, data_rows) = rqr.snapshot();
        let services =
            holon_frontend::reactive::HeadlessBuilderServices::new(self.engine().clone());
        let vm = holon_frontend::interpret_pure(&render_expr, &data_rows, &services).snapshot();
        view_model_to_snapshot(&vm)
    }

    /// Widget tree for an explicit block id. Builds the snapshot via
    /// interpret_pure against that block's resolved render_expr +
    /// data_rows, same path as `widget_tree_snapshot` but rooted at
    /// `block_id` instead of the layout root. Returns `None` if the
    /// block isn't watchable yet.
    async fn widget_tree_for(
        &self,
        block_id: &CapBlockId,
    ) -> Option<holon_pbt_core::capabilities::WidgetSnapshot> {
        let engine = self.frontend_engine.clone()?;
        let uri = holon_api::EntityUri::parse(block_id).ok()?;
        let rqr = engine.ensure_watching(&uri);
        if rqr.is_loading() {
            return None;
        }
        let (render_expr, data_rows) = rqr.snapshot();
        let services =
            holon_frontend::reactive::HeadlessBuilderServices::new(self.engine().clone());
        let vm = holon_frontend::interpret_pure(&render_expr, &data_rows, &services).snapshot();
        Some(view_model_to_snapshot(&vm))
    }

    /// Extracts the `id` column from the layout root's data_rows.
    /// Returns empty set if the layout root isn't watchable yet.
    async fn root_data_row_ids(&self) -> std::collections::BTreeSet<CapBlockId> {
        let Some(engine) = self.frontend_engine.clone() else {
            return Default::default();
        };
        let root_uri = holon_api::root_layout_block_uri();
        let rqr = engine.ensure_watching(&root_uri);
        if rqr.is_loading() {
            return Default::default();
        }
        let (_, data_rows) = rqr.snapshot();
        data_rows
            .iter()
            .filter_map(|r| r.get("id").and_then(|v| v.as_string()).map(String::from))
            .collect()
    }
}

/// Frontend-agnostic ViewModel→WidgetSnapshot translator.
///
/// Encoding contract (see `WidgetSnapshot` rustdoc in pbt-core):
/// - `kind` = `vm.widget_name()` or `"unknown"` for un-tagged kinds.
/// - `entity_id` = `vm.row_id()` if present.
/// - `props` = kind-specific scalar fields canonicalised to strings
///   (StateToggle.field/current/label/states, EditableText.field/content, etc.).
/// - `operations` = canonical form `<op_name>:<affected_fields_csv>:<param_names_csv>`,
///   one per `OperationWiring`. Invariants match by prefix
///   (`set_field:task_state:` etc.).
fn view_model_to_snapshot(
    vm: &holon_frontend::view_model::ViewModel,
) -> holon_pbt_core::capabilities::WidgetSnapshot {
    use holon_frontend::view_model::ViewKind;
    use std::collections::BTreeMap;

    let kind = vm.widget_name().unwrap_or("unknown").to_string();
    // For LiveBlock, the referenced block id lives in `kind.block_id`;
    // `vm.row_id()` returns None. Mirror `ViewModel::collect_ids_recursive`
    // semantics by surfacing block_id as entity_id so cross-slice
    // invariants can walk references without knowing about LiveBlock.
    let entity_id = match &vm.kind {
        ViewKind::LiveBlock { block_id, .. } => Some(block_id.clone()),
        _ => vm.row_id(),
    };

    let mut props: BTreeMap<String, String> = BTreeMap::new();
    match &vm.kind {
        ViewKind::StateToggle {
            field,
            current,
            label,
            states,
        } => {
            props.insert("field".into(), field.clone());
            props.insert("current".into(), current.clone());
            props.insert("label".into(), label.clone());
            props.insert("states".into(), states.clone());
        }
        ViewKind::EditableText { content, field } => {
            props.insert("field".into(), field.clone());
            props.insert("content".into(), content.clone());
        }
        ViewKind::RenderedText { content, field } => {
            props.insert("field".into(), field.clone());
            props.insert("content".into(), content.clone());
        }
        ViewKind::Text { content, .. } => {
            props.insert("content".into(), content.clone());
        }
        ViewKind::Badge { label } => {
            props.insert("label".into(), label.clone());
        }
        _ => {}
    }

    let operations: Vec<String> = vm
        .operations
        .iter()
        .map(|ow| {
            let fields_csv = ow.descriptor.affected_fields.join(",");
            let params_csv = ow
                .descriptor
                .required_params
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>()
                .join(",");
            format!("{}:{}:{}", ow.descriptor.name, fields_csv, params_csv)
        })
        .collect();

    let children = vm.children().iter().map(view_model_to_snapshot).collect();

    holon_pbt_core::capabilities::WidgetSnapshot {
        kind,
        entity_id,
        props,
        operations,
        children,
    }
}

// ─── SutLayout ────────────────────────────────────────────────────────

#[allow(async_fn_in_trait)]
impl<V: VariantMarker> SutLayout for E2ESut<V> {
    /// True if any element carrying `id` as its `entity_id` is currently
    /// in the BoundsRegistry. Mirrors the `lookup_entity` helper used in
    /// `inv-frontend-bounds-rendered` (sut.rs:6093–6105).
    async fn has_registered_bounds(&self, id: &CapBlockId) -> bool {
        let Some(ref geometry) = self.frontend_geometry else {
            return false;
        };
        geometry
            .element_info(&format!("render-entity-{id}"))
            .or_else(|| geometry.element_info(&format!("live-block-{id}")))
            .or_else(|| geometry.element_info(&format!("selectable-{id}")))
            .or_else(|| geometry.element_info(&format!("editable-text-{id}")))
            .or_else(|| geometry.find_by_entity_id(id))
            .is_some()
    }

    /// True if a `draggable` element carrying `id` is in the BoundsRegistry.
    /// Mirrors the `tree_draggable` collection in `inv-editable-text-has-draggable`
    /// (sut.rs:6634–6643): an element whose widget_type == "draggable" and
    /// entity_id == id.
    async fn has_draggable_handle(&self, id: &CapBlockId) -> bool {
        let Some(ref geometry) = self.frontend_geometry else {
            return false;
        };
        geometry.all_elements().into_iter().any(|(_, info)| {
            info.widget_type == "draggable" && info.entity_id.as_deref() == Some(id.as_str())
        })
    }

    /// True if any rendered element has widget_type == "error".
    /// Mirrors `inv-frontend-no-error-widgets` (sut.rs:6050–6063) via the
    /// BoundsRegistry; falls back to the ViewModel tree via `frontend_engine`
    /// (pub) when no geometry provider is installed.
    async fn any_error_widget(&self) -> bool {
        if let Some(ref geometry) = self.frontend_geometry {
            return geometry
                .all_elements()
                .into_iter()
                .any(|(_, info)| info.widget_type == "error");
        }
        let Some(engine) = self.frontend_engine.clone() else {
            return false;
        };
        let root_uri = holon_api::root_layout_block_uri();
        let vm = engine.snapshot(&root_uri);
        crate::display_assertions::count_error_nodes(&vm) > 0
    }
}

// ─── SutDriver ────────────────────────────────────────────────────────

#[allow(async_fn_in_trait)]
impl<V: VariantMarker> SutDriver for E2ESut<V> {
    /// Send a raw key chord to the currently focused entity.
    /// Not wired: SutDriver::driver_send_key_chord needs a KeyChord
    /// (not a raw string) and a known focused entity id — both are
    /// context-dependent. The existing `E2ESut::send_key_chord` requires
    /// an entity_id param and a parsed `holon_api::KeyChord`. A thin
    /// bridge would need parsing + focus resolution not yet exposed. Wire
    /// in Phase 7 alongside the SutFocus trait (which will expose
    /// `current_focus` from the reference model).
    async fn driver_send_key_chord(&mut self, _: &str) {
        unimplemented!(
            "SutDriver::driver_send_key_chord on E2ESut: requires a focused entity id \
             and a parsed KeyChord; E2ESut::send_key_chord already provides this with \
             explicit args. Bridge in Phase 7 once SutFocus exposes the current focus id."
        )
    }

    /// Click an entity by id via the installed UserDriver.
    /// Delegates to the driver's `click_entity` with region "main",
    /// the same default used for most SplitBlock / ClickBlock transitions
    /// (sut.rs:2228–2256).
    async fn driver_click(&mut self, id: &CapBlockId) {
        let driver = self
            .driver
            .as_ref()
            .expect("SutDriver::driver_click: driver not installed");
        driver
            .click_entity(id.as_str(), "main")
            .await
            .unwrap_or_else(|e| panic!("SutDriver::driver_click failed for {id}: {e:#}"));
    }

    /// Returns the current SQL-side focus block id from the `current_focus`
    /// matview. Not wired via the UserDriver (drivers don't expose a
    /// `current_focus()` verb); instead reads the authoritative SQL view,
    /// matching the prod path used in `check_invariants_async` (sut.rs:4665).
    /// Returns the Main-region focus id, or `None` when the matview is empty.
    async fn driver_current_focus(&self) -> Option<CapBlockId> {
        let rows = self
            .ctx
            .query_sql("SELECT region, block_id FROM current_focus")
            .await
            .expect("SutDriver::driver_current_focus: current_focus query failed");
        rows.into_iter()
            .find(|row| {
                row.get("region")
                    .and_then(|v| v.as_string())
                    .map(|r| r == "main")
                    .unwrap_or(false)
            })
            .and_then(|row| {
                row.get("block_id")
                    .and_then(|v| v.as_string())
                    .map(str::to_string)
            })
    }

    /// Returns the globally focused block id as tracked by the frontend
    /// engine's `focused_block()` field. Returns `None` in SqlOnly mode
    /// (no `frontend_engine` installed) or when the engine has no focus.
    /// Mirrors `inv-focus-matches-ref` (sut.rs:6750): `engine.focused_block()`.
    async fn engine_focused_block(&self) -> Option<CapBlockId> {
        self.frontend_engine
            .as_ref()
            .and_then(|engine| engine.focused_block())
            .map(|uri| uri.as_str().to_string())
    }

    /// Translate a reference-model synthetic block id (e.g. `block:ref-doc-0`)
    /// to the resolved UUID-based id the SUT engine tracks. Delegates to
    /// `E2ESut::resolve_uri`, which consults `doc_uri_map`.
    fn resolve_ref_block_id(&self, id: &CapBlockId) -> CapBlockId {
        let uri = holon_api::EntityUri::from_raw(id);
        self.resolve_uri(&uri).as_str().to_string()
    }
}

// ─── SutOrgRender ─────────────────────────────────────────────────────

#[allow(async_fn_in_trait)]
impl<V: VariantMarker> SutOrgRender for E2ESut<V> {
    /// Render all tracked documents to org-mode text, returning
    /// `(path_string, org_contents)` pairs.
    /// Delegates to `TestContext::snapshot_org_render_pairs` — the same
    /// path used by `inv-org-render-fixed-point` (sut.rs:4395–4412).
    async fn render_documents_to_org(&self) -> Vec<(String, String)> {
        let pairs = self
            .ctx
            .snapshot_org_render_pairs()
            .await
            .expect("SutOrgRender::render_documents_to_org: snapshot_org_render_pairs failed");
        pairs
            .into_iter()
            .map(|(path, (_disk, rendered))| (path.to_string_lossy().to_string(), rendered))
            .collect()
    }
}

// ─── SutQueryCompile ──────────────────────────────────────────────────

#[allow(async_fn_in_trait)]
impl<V: VariantMarker> SutQueryCompile for E2ESut<V> {
    /// Not wired: query compilation (PRQL/GQL) requires access to the
    /// holon query-compiler helpers. The compilation path lives in
    /// `crates/holon/src/` and is not currently exposed via `E2ESut` or
    /// `TestContext`. This trait is bound by GENERATORS (not invariants),
    /// so slices without it simply produce no query-content blocks — no
    /// regression. Wire in Phase 7 when a generator actually needs it.
    async fn compile_query(&self, _: &str, _: &str) -> Result<String, String> {
        unimplemented!(
            "SutQueryCompile::compile_query on E2ESut: query compiler helpers are in \
             crates/holon/src/ and not yet exposed via TestContext. Wire in Phase 7 \
             when a generator needs this path."
        )
    }
}
