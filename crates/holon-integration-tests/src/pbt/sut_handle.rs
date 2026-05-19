//! `SutHandle` trait impl for `E2ESut<V>` — the per-transition SUT-side
//! dispatch surface. Each method here is the concrete reaction the wide
//! PBT runs when proptest hands it a transition variant.
//!
//! The trait itself lives in [`crate::pbt::transition_dispatch::SutHandle`].
//! Many methods are thin chord/driver dispatches; a few (`apply_start_app`,
//! `apply_bulk_external_add`, `apply_toggle_state`, `apply_split_block`,
//! `apply_trigger_doc_link`, `apply_click_block`, `apply_edit_via_*`,
//! `apply_trigger_slash_command`) still carry inline business logic that
//! Phase C migration will move into per-transition modules under
//! `pbt/transitions/`.
//!
//! Extracted from `sut.rs` (Phase D3).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_api::{QueryLanguage, Value};

use crate::wait_for_file_condition;

use super::reference_state::ReferenceState;
use super::sut::E2ESut;
use super::types::*;

#[async_trait::async_trait(?Send)]
impl<V: VariantMarker> crate::pbt::transition_dispatch::SutHandle for E2ESut<V> {
    /// Override the default-panicking SutHandle stub: route the layout-PBT
    /// `Clickable::click_at_element` capability through the same
    /// `UserDriver::click_entity` path the rich-PBT chord transitions use.
    /// Region is unknown at this layer (the shared variant only has an
    /// `element_id`) so we pass an empty region string — the driver
    /// resolves geometry via the bounds registry alone.
    async fn apply_click_at_element(&mut self, element_id: &str) {
        let driver = self
            .driver
            .as_ref()
            .expect("driver not installed — was start_app called?");
        // The driver's editor_focus fallback requires a valid region. Infer
        // from the element_id prefix; default to "main" for generic clicks.
        let region = if element_id.contains("left-sidebar") || element_id.contains("left_sidebar") {
            "left_sidebar"
        } else if element_id.contains("right-sidebar") || element_id.contains("right_sidebar") {
            "right_sidebar"
        } else {
            "main"
        };
        driver
            .click_entity(element_id, region)
            .await
            .unwrap_or_else(|e| {
                panic!("[LayoutPBT::click_at_element] click_entity({element_id}) failed: {e:#}")
            });
        self.ctx.drain_region_cdc_events().await;
    }

    /// Override the default-panicking SutHandle stub. Backend tests don't
    /// generate `DeliverBlockContent` (the variant's `weighted_generator`
    /// returns `Fail(DeliverNotMeaningfulInBackendTests)`), so this should
    /// never be reached. Keep the override so accidental wiring fails
    /// loud with a typed message instead of the default `unimplemented!`.
    async fn apply_deliver_block_content_loaded(&mut self, block_id: &str) {
        panic!(
            "[LayoutPBT::deliver_block_content_loaded] reached for {block_id} — backend PBT \
             should reject DeliverBlockContent in its generator (see weighted_generator)."
        );
    }

    async fn navigate_back(&mut self, region: holon_api::Region) {
        debug_assert_eq!(
            region,
            holon_api::Region::Main,
            "NavigateBack generator must only emit Main; got {region:?}"
        );
        self.send_leader_chord("go_back", "NavigateBack").await;
        self.ctx.drain_region_cdc_events().await;
        self.dump_nav_tables("after NavigateBack").await;
    }

    async fn apply_write_org_file(&mut self, filename: &str, content: &str) {
        tracing::trace!(
            "[apply] WriteOrgFile: {} ({} bytes)",
            filename,
            content.len()
        );
        self.write_org_file(filename, content)
            .await
            .expect("Failed to write org file");

        // If app is running, wait for OrgSyncController to ingest the file
        // and re-key ctx.documents from `file:<filename>` to the resolved
        // doc URI. Mirrors the start_app loop (see apply_start_app body):
        // without this, subsequent transitions like apply_bulk_external_add
        // that resolve the doc via `resolve_uri` (which checks doc_uri_map)
        // and then `ctx.documents.get(&resolved)` will miss because docs
        // added post-startup never got re-keyed.
        if !self.ctx.is_running() {
            return;
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match self.ctx.resolve_doc_uri_by_name(filename).await {
                Ok(resolved) => {
                    let file_key = holon_api::EntityUri::file(filename);
                    if let Some(path) = self.ctx.documents.remove(&file_key) {
                        self.ctx.documents.insert(resolved.clone(), path);
                    }
                    // The synthetic URI minted by the ref-model equals
                    // the resolved URI (WriteOrgFile.apply_to_sut
                    // injects `#+ID: <synthetic>` into the file).
                    if !self.doc_uri_map.contains_key(&resolved) {
                        self.doc_uri_map.insert(resolved.clone(), resolved.clone());
                    }
                    break;
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
    }

    async fn apply_create_directory(&mut self, path: &str) {
        tracing::trace!("[apply] CreateDirectory: {}", path);
        let full_path = self.temp_dir.path().join(path);
        tokio::fs::create_dir_all(&full_path)
            .await
            .expect("Failed to create directory");
    }

    async fn apply_git_init(&mut self) {
        tracing::trace!("[apply] GitInit");
        let output = tokio::process::Command::new("git")
            .args(["init"])
            .current_dir(self.temp_dir.path())
            .output()
            .await
            .expect("Failed to run git init");
        assert!(output.status.success(), "git init failed: {:?}", output);
    }

    async fn apply_jj_git_init(&mut self) {
        tracing::trace!("[apply] JjGitInit");
        let output = tokio::process::Command::new("jj")
            .args(["git", "init"])
            .current_dir(self.temp_dir.path())
            .output()
            .await
            .expect("Failed to run jj git init");
        assert!(output.status.success(), "jj git init failed: {:?}", output);
    }

    async fn apply_create_stale_loro(
        &mut self,
        org_filename: &str,
        corruption_type: crate::LoroCorruptionType,
    ) {
        tracing::trace!(
            "[apply] CreateStaleLoro: {} ({:?})",
            org_filename,
            corruption_type
        );
        self.write_stale_loro_file(org_filename, corruption_type)
            .await
            .expect("Failed to create stale loro file");
    }

    #[tracing::instrument(
        skip(self, ref_state),
        fields(wait_for_ready, enable_todoist, enable_loro),
        name = "pbt.apply_start_app"
    )]
    async fn apply_start_app(
        &mut self,
        ref_state: &ReferenceState,
        wait_for_ready: bool,
        enable_todoist: bool,
        enable_loro: bool,
    ) {
        tracing::trace!(
            "[apply] StartApp (wait_for_ready={}, enable_todoist={}, enable_loro={})",
            wait_for_ready,
            enable_todoist,
            enable_loro
        );
        self.set_enable_todoist(enable_todoist);
        self.set_enable_loro(enable_loro);
        self.start_app(wait_for_ready)
            .await
            .expect("Failed to start app");

        // Install the default mutation driver now that the engine exists.
        if self.driver.is_none() {
            self.install_driver();
        }

        // Initialize real MCP integration for IVM re-evaluation testing.
        let db_handle = self.ctx.engine().db_handle().clone();
        match crate::pbt_mcp_fake::PbtMcpIntegration::new(db_handle).await {
            Ok(integration) => self.pbt_mcp = Some(integration),
            Err(e) => {
                tracing::trace!("[apply] PbtMcpIntegration init failed (non-fatal): {e}")
            }
        }

        // Mirror Flutter startup: call initial_widget() after engine ready.
        let expects_valid_index = ref_state.is_properly_setup();
        let root_id = ref_state
            .root_layout_block_id()
            .unwrap_or_else(holon_api::root_layout_block_uri);
        tracing::trace!(
            "[apply] Calling render_entity('{}') (expects valid index.org: {})",
            root_id,
            expects_valid_index
        );

        let render_result = self.engine().blocks().render_entity(&root_id, &None).await;

        match (expects_valid_index, render_result) {
            (true, Ok((_render_expr, _stream))) => {
                tracing::trace!("[apply] render_entity('{}') succeeded", root_id);
            }
            (true, Err(e)) => {
                let err_str = e.to_string();
                if err_str.contains("ScalarSubquery") || err_str.contains("materialized view") {
                    tracing::trace!(
                        "[apply] render_entity('{}') failed due to known Turso IVM limitation (GQL): {}",
                        root_id,
                        e
                    );
                } else {
                    panic!(
                        "render_entity('{}') failed but reference state has valid index.org: {}",
                        root_id, e
                    );
                }
            }
            (false, Ok(_)) => {
                panic!(
                    "render_entity('{}') succeeded but reference state has no valid index.org",
                    root_id
                );
            }
            (false, Err(e)) => {
                tracing::trace!(
                    "[apply] render_entity('{}') correctly failed (no valid index.org): {}",
                    root_id,
                    e
                );
            }
        }

        // Set up region watches for all regions
        for region in holon_api::Region::ALL {
            if let Err(e) = self.setup_region_watch(*region).await {
                tracing::trace!(
                    "[apply] Region watch setup for {} failed (non-fatal): {}",
                    region.as_str(),
                    e
                );
            }
        }

        // Set up all-blocks CDC watch (invariant #1 uses this instead of direct SQL)
        self.setup_all_blocks_watch()
            .await
            .expect("Failed to set up all-blocks CDC watch");

        // Capture the production seed-block count
        let expected_ids = self.expected_block_ids(ref_state);
        self.prime_seed_count(&expected_ids, std::time::Duration::from_secs(10))
            .await;

        // Push the reference state's TODO keyword set into production.
        if let Some(ref ks) = ref_state.keyword_set {
            use holon_orgmode::models::OrgDocumentExt;
            let default_doc_uri = holon_api::EntityUri::no_parent();
            let rows = self
                .ctx
                .query_sql(&format!(
                    "SELECT b.id, b.parent_id, \
                     (SELECT json_group_array(bt.tag) FROM block_tags bt WHERE bt.block_id = b.id) as tags, \
                     b.content, b.content_type, b.properties \
                     FROM block_raw b WHERE b.id = '{}'",
                    default_doc_uri
                ))
                .await
                .expect("query default doc block");
            if let Some(row) = rows.first() {
                let mut doc_block = Block::new_text(
                    default_doc_uri.clone(),
                    holon_api::EntityUri::no_parent(),
                    row.get("content").and_then(|v| v.as_string()).unwrap_or(""),
                );
                doc_block.tags = row
                    .get("tags")
                    .map(|v| match v {
                        Value::Array(arr) => arr
                            .iter()
                            .filter_map(|x| x.as_string().map(|s| s.to_string()))
                            .collect(),
                        Value::Json(s) | Value::String(s) => {
                            serde_json::from_str::<Vec<String>>(s).unwrap_or_default()
                        }
                        _ => Vec::new(),
                    })
                    .unwrap_or_default();
                if let Some(props_val) = row.get("properties")
                    && let Some(s) = props_val.as_string()
                    && let Ok(map) =
                        serde_json::from_str::<std::collections::HashMap<String, Value>>(s)
                {
                    doc_block.properties = map;
                }
                doc_block.set_todo_keywords(Some(ks.0.clone()));
                let params = holon_orgmode::build_block_params(
                    &doc_block,
                    &doc_block.parent_id,
                    &default_doc_uri,
                );
                if let Err(e) = self
                    .ctx
                    .test_ctx()
                    .execute_op("block", "update", params)
                    .await
                {
                    tracing::trace!("[apply] Failed to push keyword_set into production: {e}");
                } else {
                    tracing::trace!(
                        "[apply] Pushed keyword_set ({} keywords) into production default doc",
                        ks.0.len()
                    );
                }
            } else {
                tracing::trace!(
                    "[apply] WARNING: keyword_set set but default doc {} not found in DB",
                    default_doc_uri
                );
            }
        }

        // Populate doc_uri_map for pre-startup documents whose document
        // entities were created by OrgSyncController during startup.
        for (synthetic_uri, filename) in &ref_state.documents {
            if !self.doc_uri_map.contains_key(synthetic_uri) {
                match self.ctx.resolve_doc_uri_by_name(filename).await {
                    Ok(resolved) => {
                        tracing::trace!(
                            "[apply] Mapped pre-startup doc: {} → {}",
                            synthetic_uri,
                            resolved
                        );
                        self.doc_uri_map
                            .insert(synthetic_uri.clone(), resolved.clone());
                        // Re-key ctx.documents from file-based to UUID-based URI
                        let file_key = holon_api::EntityUri::file(filename);
                        if let Some(path) = self.ctx.documents.remove(&file_key) {
                            self.ctx.documents.insert(resolved, path);
                        }
                    }
                    Err(e) => {
                        tracing::trace!(
                            "[apply] Could not resolve pre-startup doc {}: {}",
                            synthetic_uri,
                            e
                        );
                    }
                }
            }
        }

        // Initialize LoroSut if Loro is enabled
        if let Some(doc_store) = self.ctx.doc_store() {
            tracing::trace!("[apply] Loro enabled — initializing LoroSut for invariant checking");
            self.loro_sut = Some(crate::pbt::loro_sut::LoroSut::new(doc_store.clone()));
        }

        // Initialize the ReactiveEngine now so all subsequent
        // transitions can read the reactive tree — just like the real GPUI frontend.
        self.ensure_reactive_engine(&root_id).await;
        tracing::trace!("[apply] ReactiveEngine initialized for root '{}'", root_id);
    }

    async fn apply_navigate_focus(
        &mut self,
        region: holon_api::Region,
        block_id: &holon_api::EntityUri,
    ) {
        // The generator restricts NavigateFocus to `Region::Main` and
        // to LeftSidebar-listed pages — the only navigation path a
        // real user can trigger. Sanity-check both invariants here.
        debug_assert_eq!(
            region,
            holon_api::Region::Main,
            "NavigateFocus generator must only emit Main; got {region:?}"
        );
        let resolved_id = self.resolve_uri(block_id);
        tracing::trace!(
            "[apply] NavigateFocus: region={region:?} block={block_id} (resolved={resolved_id})"
        );
        // Drive the real UI: clicking the LeftSidebar entry dispatches
        // its bound `navigation.focus(region: "main", block_id)` action.
        // `ReactiveEngineDriver::click_entity` shares GPUI's intent
        // resolution: snapshot_resolved → find_click_intent_in_view_model
        // → apply_intent. No driver-specific synthesis needed.
        //
        // Mouse-driven dispatch under GPUI requires committed bounds in
        // BoundsRegistry — sidebar entries from a freshly-loaded layout
        // may not have promoted staged → committed by the time the test
        // polls. Mirror ClickBlock / SplitBlock and wait first; headless
        // drivers no-op the wait.
        self.wait_for_entity_bounds(resolved_id.as_str(), Duration::from_secs(5))
            .await
            .unwrap_or_else(|e| panic!("[NavigateFocus] {e}"));
        let driver = self
            .driver
            .as_ref()
            .expect("driver not installed — was start_app called?");
        driver
            .click_entity(resolved_id.as_str(), "left_sidebar")
            .await
            .unwrap_or_else(|e| {
                panic!("[NavigateFocus] click_entity failed for sidebar entry {resolved_id}: {e:#}")
            });
        self.ctx.drain_region_cdc_events().await;
        self.dump_nav_tables("after NavigateFocus").await;
    }

    async fn apply_navigate_forward(&mut self, region: holon_api::Region) {
        debug_assert_eq!(
            region,
            holon_api::Region::Main,
            "NavigateForward generator must only emit Main; got {region:?}"
        );
        self.send_leader_chord("go_forward", "NavigateForward")
            .await;
        self.ctx.drain_region_cdc_events().await;
        self.dump_nav_tables("after NavigateForward").await;
    }

    async fn apply_navigate_home(&mut self, region: holon_api::Region) {
        debug_assert_eq!(
            region,
            holon_api::Region::Main,
            "NavigateHome generator must only emit Main; got {region:?}"
        );
        self.send_leader_chord("go_home", "NavigateHome").await;
        self.ctx.drain_region_cdc_events().await;
        self.dump_nav_tables("after NavigateHome").await;
    }

    async fn apply_simulate_restart(&mut self, ref_state: &ReferenceState) {
        let expected_ids = self.expected_block_ids(ref_state);
        self.simulate_restart(&expected_ids)
            .await
            .expect("SimulateRestart failed");
    }

    async fn apply_create_document(&mut self, file_name: &str, ref_state: &ReferenceState) {
        tracing::trace!("[apply] Creating document: {}", file_name);
        match self.create_document(file_name).await {
            Ok(uuid_uri) => {
                // Find the synthetic URI from ref_state (keyed by filename)
                let synthetic_uri = ref_state
                    .documents
                    .iter()
                    .find(|(_, name)| *name == file_name)
                    .map(|(uri, _)| uri.clone())
                    .expect("CreateDocument: synthetic URI not found in reference state");
                tracing::trace!("[apply] Created document: {} → {}", synthetic_uri, uuid_uri);
                self.doc_uri_map.insert(synthetic_uri, uuid_uri);
            }
            Err(e) => panic!("Failed to create document: {}", e),
        }
    }

    async fn apply_remove_watch(&mut self, query_id: &str) {
        self.remove_watch(query_id);
    }

    async fn apply_switch_view(&mut self, view_name: &str) {
        self.switch_view(view_name);
    }

    async fn apply_concurrent_schema_init(&mut self) {
        tracing::trace!(
            "[apply] ConcurrentSchemaInit: testing sequential operations don't cause database lock"
        );

        // This test verifies that normal sequential operations don't cause
        // "database is locked" errors. The original bug was:
        // 1. ensure_navigation_schema() called during DI init
        // 2. initial_widget() called it AGAIN while IVM was still processing
        // 3. This caused persistent "database is locked" errors
        //
        // After the fix, sequential operations should work without locking issues.
        let engine = self.engine();

        // Run several query_and_watch operations SEQUENTIALLY (not concurrently)
        // Each creates a materialized view, which should work fine when done one at a time
        for i in 0..3 {
            let prql = format!(
                "from block_raw | select {{id, content}} | filter id != \"dummy-{}\" ",
                i
            );
            let sql = engine
                .compile_to_sql(&prql, QueryLanguage::HolonPrql)
                .expect("PRQL compilation should succeed");
            let start = std::time::Instant::now();
            match engine.query_and_watch(sql, HashMap::new(), None).await {
                Ok(_) => {
                    eprintln!(
                        "[ConcurrentSchemaInit] query_and_watch {} succeeded in {:?}",
                        i,
                        start.elapsed()
                    );
                }
                Err(e) => {
                    let error_str = format!("{:?}", e);
                    let elapsed = start.elapsed();
                    eprintln!(
                        "[ConcurrentSchemaInit] query_and_watch {} FAILED in {:?}: {}",
                        i, elapsed, error_str
                    );
                    // Check for the specific "database is locked" error that indicates
                    // the double-schema-init bug
                    if error_str.contains("database is locked") {
                        panic!(
                            "DATABASE LOCK BUG: Sequential query_and_watch {} failed with 'database is locked' after {:?}!\n\
                                 This indicates the ensure_navigation_schema() is still being called multiple times.\n\
                                 Error: {}",
                            i, elapsed, error_str
                        );
                    }
                    // Other errors (like "Database schema changed") might occur due to
                    // other concurrent activity and are not necessarily the double-init bug
                }
            }
        }

        // Also run some simple queries to verify basic operations work
        for i in 0..2 {
            let sql = "SELECT id FROM block_raw LIMIT 1".to_string();
            let start = std::time::Instant::now();
            match engine.execute_query(sql, HashMap::new(), None).await {
                Ok(_) => {
                    eprintln!(
                        "[ConcurrentSchemaInit] simple query {} succeeded in {:?}",
                        i,
                        start.elapsed()
                    );
                }
                Err(e) => {
                    let error_str = format!("{:?}", e);
                    let elapsed = start.elapsed();
                    eprintln!(
                        "[ConcurrentSchemaInit] simple query {} FAILED in {:?}: {}",
                        i, elapsed, error_str
                    );
                    if error_str.contains("database is locked") {
                        panic!(
                            "DATABASE LOCK BUG: Sequential simple query {} failed with 'database is locked' after {:?}!\n\
                                 Error: {}",
                            i, elapsed, error_str
                        );
                    }
                }
            }
        }

        eprintln!("[ConcurrentSchemaInit] All sequential operations completed successfully");
        eprintln!("[ConcurrentSchemaInit] Test completed successfully");
    }

    async fn apply_setup_watch(
        &mut self,
        query_id: &str,
        query: &crate::pbt::query::TestQuery,
        language: QueryLanguage,
    ) {
        let (source, lang_str) = query.compile_for(language);
        tracing::trace!(
            "[apply] SetupWatch: {} ({}) → {}",
            query_id,
            lang_str,
            &source[..source.len().min(80)]
        );
        self.setup_watch(query_id, &source, lang_str)
            .await
            .expect("Watch setup failed");
    }

    async fn apply_toggle_state(&mut self, block_id: &holon_api::EntityUri, new_state: &str) {
        let resolved_block_id = self.resolve_uri(block_id);
        tracing::trace!(
            "[apply] ToggleState: block={block_id} (resolved={resolved_block_id}) → {new_state:?}"
        );

        // Use a fully cross-block-resolved ViewModel: each nested
        // `live_block` is recursively interpreted via
        // `engine.snapshot_resolved`, which calls `ensure_watching`
        // per block so per-region UiWatchers fire. We poll until the
        // target entity is visible — sidebar/main-panel slots populate
        // asynchronously, and the older `current_resolved_view_model`
        // (which only interprets the root) returns an empty
        // `live_block` for the main-panel slot.
        let display_tree = self
            .wait_for_entity_in_resolved_view_model(
                resolved_block_id.as_str(),
                Duration::from_secs(5),
            )
            .await
            .unwrap_or_else(|| {
                panic!(
                    "[ToggleState] entity {resolved_block_id} did not appear in the \
                     resolved ViewModel within 5s — sidebar nav may not have populated \
                     the main panel yet."
                )
            });

        let all_toggles = crate::display_assertions::collect_state_toggle_nodes(&display_tree);
        let toggle = all_toggles.iter().find(|t| {
            t.row_id()
                .is_some_and(|id| id == resolved_block_id.as_str())
        });
        if toggle.is_none() {
            eprintln!(
                "[ToggleState] No StateToggle with id={block_id} in resolved tree \
                 (found {} toggles, root={:?}).\nTree:\n{}",
                all_toggles.len(),
                display_tree.widget_name(),
                display_tree.pretty_print(0),
            );
            panic!("[ToggleState] No StateToggle with id={block_id} in resolved tree");
        }
        let toggle = toggle.unwrap();

        let (field, current, states) = match &toggle.kind {
            holon_frontend::view_model::ViewKind::StateToggle {
                field,
                current,
                states,
                ..
            } => (field.clone(), current.clone(), states.clone()),
            _ => panic!("[ToggleState] Expected StateToggle, got {:?}", toggle.kind),
        };

        assert!(
            !toggle.operations.is_empty(),
            "[ToggleState] StateToggle for {block_id} has no operations"
        );
        let op = holon_frontend::operations::find_set_field_op(&field, &toggle.operations);
        assert!(
            op.is_some(),
            "[ToggleState] No set_field op for '{field}' on {block_id}"
        );
        let op = op.unwrap();

        let row_id = toggle.row_id();
        assert!(
            row_id.is_some(),
            "[ToggleState] StateToggle for {block_id} has no entity id"
        );
        let row_id = row_id.unwrap();
        let entity_name = toggle
            .entity_name()
            .expect("[ToggleState] StateToggle has no entity name");

        let states_vec: Vec<String> = states.split(',').map(|s| s.to_string()).collect();
        assert!(
            states_vec.iter().any(|s| s == new_state),
            "[ToggleState] '{new_state}' not in states {states_vec:?}"
        );

        // Validate that the keybinding registry's chord for
        // `cycle_task_state` was joined onto the rendered state_toggle
        // node's operations. Reading directly off the resolved
        // ViewModel node we already located — bypasses the older
        // `assert_keychord_resolves` path that walks
        // `current_reactive_tree`, whose `live_block` slots are not
        // synchronously populated in the headless test (the same
        // limitation we worked around with
        // `wait_for_entity_in_resolved_view_model`).
        if let Some(expected_chord) = self.find_keybinding_for_op("cycle_task_state") {
            let cycle_op_chord = toggle.operations.iter().find_map(|ow| {
                if ow.descriptor.name == "cycle_task_state" {
                    ow.descriptor.key_chord().cloned()
                } else {
                    None
                }
            });
            assert_eq!(
                cycle_op_chord.as_ref(),
                Some(&expected_chord),
                "[ToggleState] state_toggle on {block_id} is missing the \
                 keybinding-joined `cycle_task_state` op (expected chord {expected_chord:?}). \
                 Operations on the node: {:?}",
                toggle
                    .operations
                    .iter()
                    .map(|ow| (
                        ow.descriptor.name.clone(),
                        ow.descriptor.key_chord().cloned()
                    ))
                    .collect::<Vec<_>>()
            );
            eprintln!(
                "[ToggleState] keychord validation OK: {expected_chord:?} bound on \
                 cycle_task_state for {row_id}"
            );
        }

        // Dispatch the actual mutation via set_field (PBT controls exact new_state)
        let intent = holon_frontend::OperationIntent::set_field(
            &entity_name,
            &op.name,
            &row_id,
            &field,
            Value::String(new_state.to_string()),
        );
        eprintln!("[ToggleState] Dispatching set_field: {current:?} → {new_state:?}");
        let driver = self
            .driver
            .as_ref()
            .expect("driver not installed — was start_app called?");
        driver
            .apply_intent(intent)
            .await
            .expect("ToggleState dispatch failed");

        // Let the CDC event propagate through the enrichment pipeline.
        // The data matview CDC fires synchronously from the DB write, but
        // the channel-based forwarding needs a yield to process.
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;

        // ── Fresh-tree check ─────────────────────────────────
        // Snapshot the reactive tree NOW — before the structural
        // re-render can mask CDC enrichment bugs. The structural CDC
        // also fires for this row change and triggers a re-render
        // with fresh query_view data (which uses a different JSON
        // parsing path). By checking here, we observe the
        // CDC-enriched data before it gets replaced.
        if let Some((_root, post_tree)) = self.current_reactive_tree() {
            let post_toggle = crate::display_assertions::find_state_toggle_for_block_reactive(
                &post_tree,
                &resolved_block_id,
            );
            if let Some(post) = post_toggle
                && post.widget_name().as_deref() == Some("state_toggle")
            {
                let post_current = post.prop_str("current").unwrap_or_else(|| "".to_string());
                // The value must be either the new state (CDC propagated
                // correctly) or the old state (CDC hasn't arrived yet).
                // It must NOT be empty when we set it to a non-empty value
                // — that would mean the CDC enrichment dropped the property.
                if !new_state.is_empty() && post_current.is_empty() {
                    panic!(
                        "[ToggleState] Post-mutation ViewModel has empty StateToggle \
                             for block {block_id}! Set '{current}' → '{new_state}' but \
                             got ''. This means the CDC enrichment pipeline lost the \
                             task_state property (flatten_properties bug)."
                    );
                }
            }
        }

        // Live-tree vs fresh-tree check is done in check_invariants
        // via the HeadlessLiveTree (inv10_live).
    }

    async fn apply_edit_via_display_tree(
        &mut self,
        block_id: &holon_api::EntityUri,
        new_content: &str,
    ) {
        let resolved_block_id = self.resolve_uri(block_id);
        tracing::trace!(
            "[apply] EditViaDisplayTree: block={block_id} (resolved={resolved_block_id}) → {new_content:?}"
        );

        // In production, leaf blocks are rendered by the render_entity() DSL
        // function using entity profiles + row data from the parent query.
        // We replicate this by querying the block's data and interpreting
        // render_entity() with that data as context.
        let engine = self.engine();
        let sql = format!(
            "SELECT id, content, content_type, source_language, parent_id \
             FROM block_raw WHERE id = '{}'",
            resolved_block_id
        );
        let data_rows = engine
            .execute_query(sql, HashMap::new(), None)
            .await
            .expect("block query failed in EditViaDisplayTree");
        assert!(
            !data_rows.is_empty(),
            "[EditViaDisplayTree] Block {block_id} not found in database"
        );

        let render_expr = holon_api::render_types::RenderExpr::FunctionCall {
            name: "render_entity".to_string(),
            args: Vec::new(),
        };

        let engine_clone = Arc::clone(engine);
        let display_tree = tokio::task::spawn_blocking(move || {
            let services = holon_frontend::reactive::HeadlessBuilderServices::new(engine_clone);
            holon_frontend::interpret_pure(
                &render_expr,
                &data_rows
                    .iter()
                    .cloned()
                    .map(std::sync::Arc::new)
                    .collect::<Vec<_>>(),
                &services,
            )
            .snapshot()
        })
        .await
        .expect("spawn_blocking panicked");

        // Walk tree to find EditableText node for this block_id
        fn find_editable_for_block<'a>(
            node: &'a holon_frontend::ViewModel,
            block_id: &EntityUri,
        ) -> Option<&'a holon_frontend::ViewModel> {
            if matches!(
                &node.kind,
                holon_frontend::view_model::ViewKind::EditableText { .. }
            ) && node
                .entity
                .get("id")
                .and_then(|v| v.as_string())
                .is_some_and(|id| id == block_id.as_str())
            {
                return Some(node);
            }
            node.children()
                .iter()
                .find_map(|c| find_editable_for_block(c, block_id))
        }

        let editable = find_editable_for_block(&display_tree, &resolved_block_id)
            .or_else(|| find_editable_for_block(&display_tree, &resolved_block_id))
            .unwrap_or_else(|| {
                panic!(
                    "[EditViaDisplayTree] No EditableText with id={resolved_block_id} in display tree.\n\
                     This means render_entity created the node without entity context.\n{}",
                    display_tree.pretty_print(0)
                )
            });

        assert!(
            !editable.operations.is_empty(),
            "[EditViaDisplayTree] EditableText for {block_id} has empty operations.\n\
             set_field cannot fire on blur.\n{}",
            display_tree.pretty_print(0)
        );

        // Extract operation metadata and execute
        let op = holon_frontend::operations::find_set_field_op("content", &editable.operations)
            .expect("No set_field operation found on EditableText");

        let row_id = editable.row_id().expect("EditableText entity has no 'id'");
        let entity_name = editable
            .entity_name()
            .expect("EditableText entity has no entity name");

        let intent = holon_frontend::OperationIntent::set_field(
            &entity_name,
            &op.name,
            &row_id,
            "content",
            Value::String(new_content.to_string()),
        );

        let driver = self
            .driver
            .as_ref()
            .expect("driver not installed — was start_app called?");
        driver
            .apply_intent(intent)
            .await
            .expect("set_field via display tree failed");
    }

    async fn apply_edit_via_view_model(
        &mut self,
        block_id: &holon_api::EntityUri,
        new_content: &str,
    ) {
        let resolved_block_id = self.resolve_uri(block_id);
        tracing::trace!(
            "[apply] EditViaViewModel: block={block_id} (resolved={resolved_block_id}) → {new_content:?}"
        );

        // 1. Query block data and render via render_entity() DSL (same as EditViaDisplayTree)
        let engine = self.engine();
        let sql = format!(
            "SELECT id, content, content_type, source_language, parent_id \
             FROM block_raw WHERE id = '{}'",
            resolved_block_id
        );
        let data_rows = engine
            .execute_query(sql, HashMap::new(), None)
            .await
            .expect("block query failed in EditViaViewModel");
        assert!(
            !data_rows.is_empty(),
            "[EditViaViewModel] Block {resolved_block_id} not found in database"
        );

        let render_expr = holon_api::render_types::RenderExpr::FunctionCall {
            name: "render_entity".to_string(),
            args: Vec::new(),
        };

        let engine_clone = Arc::clone(engine);
        let display_tree = tokio::task::spawn_blocking(move || {
            let services = holon_frontend::reactive::HeadlessBuilderServices::new(engine_clone);
            holon_frontend::interpret_pure(
                &render_expr,
                &data_rows
                    .iter()
                    .cloned()
                    .map(std::sync::Arc::new)
                    .collect::<Vec<_>>(),
                &services,
            )
            .snapshot()
        })
        .await
        .expect("spawn_blocking panicked");

        // 2. Find EditableText node for this block
        let editable = display_tree
            .find_editable_text(resolved_block_id.as_str())
            .unwrap_or_else(|| {
                panic!(
                    "[EditViaViewModel] No EditableText with id={resolved_block_id} in display tree.\n{}",
                    display_tree.pretty_print(0)
                )
            });

        // 3. Verify triggers are present
        assert!(
            !editable.triggers.is_empty(),
            "[EditViaViewModel] EditableText for {block_id} has no triggers.\n{}",
            display_tree.pretty_print(0)
        );

        // 4. Build EditorViewModel and verify normal text doesn't fire triggers
        let mut ctrl = holon_frontend::EditorViewModel::from_view_model(editable);
        assert!(
            matches!(
                ctrl.on_text_changed("hello", 1),
                holon_frontend::EditorAction::None
            ),
            "[EditViaViewModel] Normal text 'hello' should NOT fire any trigger"
        );

        // 5. Simulate blur with new content
        let original_value = match &editable.kind {
            holon_frontend::view_model::ViewKind::EditableText { content, .. } => content.clone(),
            _ => unreachable!(),
        };
        let action = ctrl.on_blur(new_content);

        // 6. Dispatch the resulting operation
        match action {
            holon_frontend::EditorAction::Execute(intent) => {
                let driver = self
                    .driver
                    .as_ref()
                    .expect("driver not installed — was start_app called?");
                driver
                    .apply_intent(intent)
                    .await
                    .expect("set_field via ViewModel TextSync failed");
            }
            holon_frontend::EditorAction::None => {
                assert_eq!(
                    new_content,
                    original_value,
                    "[EditViaViewModel] on_blur returned None but content changed \
                     ({original_value:?} → {new_content:?}). \
                     Operations not wired? ops={:?}",
                    editable
                        .operations
                        .iter()
                        .map(|o| &o.descriptor.name)
                        .collect::<Vec<_>>()
                );
            }
            other => panic!(
                "[EditViaViewModel] Expected Execute from on_blur, got {:?}",
                other
            ),
        }
    }

    async fn apply_bulk_external_add(
        &mut self,
        doc_uri: &holon_api::EntityUri,
        blocks: &[holon_api::block::Block],
        ref_state: &ReferenceState,
    ) {
        tracing::trace!(
            "[apply] BulkExternalAdd: adding {} blocks to {}",
            blocks.len(),
            doc_uri
        );

        // Resolve file-based URI to UUID-based URI (documents map uses UUID keys after StartApp)
        let resolved_uri = self.resolve_uri(doc_uri);
        let file_path = self.ctx.documents.get(&resolved_uri).unwrap_or_else(|| {
            panic!(
                "Document not found for BulkExternalAdd: {} (resolved: {})",
                doc_uri, resolved_uri
            )
        });

        // Get all blocks for this document from reference state.
        // Note: ref_state already includes the new blocks (from apply_reference).
        // Resolve parent_ids so blocks_by_document matches UUID-based doc URIs.
        let resolved_blocks = self.resolve_ref_blocks(ref_state, true);
        let grouped = holon_api::blocks_by_document(&resolved_blocks);
        let all_blocks: Vec<holon_api::block::Block> = grouped
            .into_iter()
            .find(|(uri, _)| *uri == resolved_uri)
            .map(|(_, blocks)| blocks)
            .unwrap_or_default();
        let existing_count = all_blocks.len().saturating_sub(blocks.len());

        // Find the document block for this document (needed for #+TODO: header)
        let doc_block = resolved_blocks
            .iter()
            .find(|b| b.id == resolved_uri && b.is_page());

        // Serialize to org file (with document header so custom keywords round-trip)
        let live_blocks: Vec<&holon_api::block::Block> = all_blocks.iter().collect();
        let org_content =
            crate::serialize_blocks_to_org_with_doc(&live_blocks, &resolved_uri, doc_block);

        tracing::trace!(
            "[BulkExternalAdd] Writing {} total blocks ({} new) to {:?}",
            all_blocks.len(),
            blocks.len(),
            file_path
        );
        // DEBUG: print blocks being serialized
        for b in &all_blocks {
            tracing::trace!(
                "[BulkExternalAdd] block: {} parent_id={} type={}",
                b.id,
                b.parent_id,
                b.content_type
            );
        }
        tracing::trace!("[BulkExternalAdd] ORG CONTENT:\n{}", org_content);
        tokio::fs::write(file_path, &org_content)
            .await
            .expect("Failed to write bulk external add");

        // =========================================================================
        // FLUTTER STARTUP BUG REPRODUCTION:
        // Immediately after writing bulk data, spawn concurrent query_and_watch calls
        // while IVM is still processing the block_with_path materialized view.
        // This simulates what Flutter does: UI requests reactive queries while
        // the backend is still processing the initial data sync.
        // =========================================================================
        let engine = self.test_ctx().engine();
        let num_concurrent_watches = 3; // Simulate multiple UI components requesting data
        let mut watch_tasks = Vec::new();

        // Timeout for query_and_watch calls.
        // If the OperationScheduler's mark_available bug is present, these calls
        // will hang forever because:
        // 1. query_and_watch creates a materialized view via execute_ddl_with_deps
        // 2. The DDL requires Schema("block") dependency
        // 3. OperationScheduler checks if "block" is in available set - it's NOT
        // 4. Operation is queued in pending, response_rx.await hangs forever
        // 5. mark_available() was never called for core tables during DI init
        let query_timeout = Duration::from_secs(10);

        for i in 0..num_concurrent_watches {
            let engine_clone = engine.clone();
            let prql = format!(
                "from block_raw | select {{id, content}} | filter id != \"bulk-race-{}\" ",
                i
            );
            let sql = engine
                .compile_to_sql(&prql, QueryLanguage::HolonPrql)
                .expect("PRQL compilation should succeed");
            let task = tokio::spawn(async move {
                let start = Instant::now();
                // Use timeout to detect scheduler hangs
                let result = tokio::time::timeout(
                    query_timeout,
                    engine_clone.query_and_watch(sql.clone(), HashMap::new(), None),
                )
                .await;
                (i, start.elapsed(), sql, result)
            });
            watch_tasks.push(task);
        }

        // Note: Schema initialization happens during app startup via SchemaRegistry.
        // We don't need to test concurrent schema init here - the query_and_watch
        // calls above already test the critical concurrency path.

        // Check results - database lock/schema change errors indicate the Flutter bug
        // These manifest as various error messages:
        // - "database is locked" - SQLite busy timeout expired
        // - "Database schema changed" - IVM detected concurrent schema modifications
        // - "Failed to lock connection pool" - Connection pool contention
        fn is_concurrency_error(error_str: &str) -> bool {
            error_str.contains("database is locked")
                || error_str.contains("Database schema changed")
                || error_str.contains("Failed to lock connection pool")
        }

        for task in watch_tasks {
            match task.await {
                Ok((i, elapsed, _prql, Ok(Ok(_)))) => {
                    tracing::trace!(
                        "[BulkExternalAdd] Concurrent query_and_watch {} succeeded in {:?}",
                        i,
                        elapsed
                    );
                }
                Ok((i, elapsed, prql, Ok(Err(e)))) => {
                    let error_str = format!("{:?}", e);
                    if is_concurrency_error(&error_str) {
                        panic!(
                            "FLUTTER STARTUP BUG REPRODUCED: query_and_watch {} failed with concurrency error \
                                 after {:?} while bulk data ({} blocks) was being synced!\n\
                                 This is the exact bug that causes Flutter app to get stuck during startup.\n\
                                 Query: {}\n\
                                 Error: {}",
                            i,
                            elapsed,
                            blocks.len(),
                            prql,
                            error_str
                        );
                    } else {
                        panic!(
                            "Concurrent query_and_watch {} failed after {:?}: {}\nQuery: {}",
                            i, elapsed, error_str, prql
                        );
                    }
                }
                Ok((i, elapsed, prql, Err(_timeout))) => {
                    // Timeout occurred - this indicates the scheduler bug
                    panic!(
                        "SCHEDULER BUG: query_and_watch {} timed out after {:?}!\n\n\
                             Root cause: OperationScheduler's mark_available() was never called for 'blocks' table.\n\n\
                             The materialized view creation is stuck in the scheduler's pending queue:\n\
                             - execute_ddl_with_deps submitted with requires=[Schema(\"blocks\")]\n\
                             - can_execute() returned false (blocks not in available set)\n\
                             - Operation queued in pending, response_rx.await blocks forever\n\n\
                             Query: {}\n\n\
                             Fix required:\n\
                             1. Call scheduler_handle.mark_available() for core tables after schema creation in DI\n\
                             2. Ensure MarkAvailable command calls process_pending_queue() to wake pending ops",
                        i, elapsed, prql
                    );
                }
                Err(e) => {
                    panic!("Query task panicked: {:?}", e);
                }
            }
        }

        // Poll until file contains expected block count (with timeout)
        let expected_block_count = all_blocks.len();
        let file_path_clone = file_path.clone();
        let start = Instant::now();
        let timeout = Duration::from_millis(5000);

        let condition_met = wait_for_file_condition(
            &file_path_clone,
            |content| {
                let text_count = content.matches(":ID:").count();
                let src_count = content.to_lowercase().matches("#+begin_src").count();
                text_count + src_count == expected_block_count
            },
            timeout,
        )
        .await;

        let elapsed = start.elapsed();
        let final_content = tokio::fs::read_to_string(file_path)
            .await
            .expect("Failed to read file after bulk add");
        let text_block_count = final_content.matches(":ID:").count();
        let source_block_count = final_content.to_lowercase().matches("#+begin_src").count();
        let actual_block_count = text_block_count + source_block_count;

        if !condition_met || actual_block_count < expected_block_count {
            panic!(
                "SYNC LOOP BUG: BulkExternalAdd wrote {} blocks but only {} remain after {:?}!\n\
                     Expected {} blocks total ({} existing + {} new).\n\
                     File content:\n{}",
                expected_block_count,
                actual_block_count,
                elapsed,
                expected_block_count,
                existing_count,
                blocks.len(),
                final_content
            );
        }
        tracing::trace!(
            "[BulkExternalAdd] File verified with {} blocks after {:?}",
            actual_block_count,
            elapsed
        );

        // Now wait for the blocks to sync to the DATABASE.
        let expected_db_count = Self::expected_content_block_count(ref_state);
        let expected_ids = self.expected_block_ids(ref_state);
        let db_timeout = Duration::from_millis(10000);
        let db_start = Instant::now();

        let actual_rows = self.wait_for_blocks_synced(&expected_ids, db_timeout).await;
        let db_elapsed = db_start.elapsed();

        if actual_rows.len() == expected_db_count {
            tracing::trace!(
                "[BulkExternalAdd] Database synced ({} blocks) in {:?}",
                expected_db_count,
                db_elapsed
            );
        } else {
            // Diagnostic: print which ref_state blocks are missing from SQL.
            let sql_ids: std::collections::HashSet<String> = actual_rows
                .iter()
                .filter_map(|r| r.get("id").and_then(|v| v.as_string()).map(String::from))
                .collect();
            let ref_non_doc: Vec<&holon_api::block::Block> = ref_state
                .block_state
                .blocks
                .values()
                .filter(|b| !b.is_page())
                .collect();
            let mut missing: Vec<String> = Vec::new();
            let mut extra: Vec<String> = Vec::new();
            for b in &ref_non_doc {
                let resolved = self.resolve_uri(&b.id);
                if !sql_ids.contains(resolved.as_str()) {
                    missing.push(format!(
                        "{} (resolved={}) parent={} doc={:?}",
                        b.id,
                        resolved,
                        b.parent_id,
                        ref_state.block_state.block_documents.get(&b.id)
                    ));
                }
            }
            let ref_ids: std::collections::HashSet<String> = ref_non_doc
                .iter()
                .map(|b| self.resolve_uri(&b.id).to_string())
                .collect();
            for sid in &sql_ids {
                if !ref_ids.contains(sid) {
                    extra.push(sid.clone());
                }
            }
            panic!(
                "[BulkExternalAdd] WARNING: Database has {} blocks, expected {} after {:?}\n\
                 MISSING from SQL ({}):\n  {}\n\
                 EXTRA in SQL ({}):\n  {}",
                actual_rows.len(),
                expected_db_count,
                db_elapsed,
                missing.len(),
                missing.join("\n  "),
                extra.len(),
                extra.join("\n  "),
            );
        }

        // Poll until org files stabilize (sync controller finishes re-rendering)
        self.wait_for_org_files_stable(25, Duration::from_millis(5000))
            .await;
    }

    async fn apply_concurrent_mutations(
        &mut self,
        ui_mutation: crate::pbt::types::MutationEvent,
        external_mutation: crate::pbt::types::MutationEvent,
        ref_state: &ReferenceState,
    ) {
        tracing::trace!(
            "[apply] ConcurrentMutations: UI={:?}, External={:?}",
            ui_mutation.mutation,
            external_mutation.mutation
        );
        // Delegate to the inherent impl method (Rust resolves inherent before trait).
        let ui = ui_mutation;
        let ext = external_mutation;
        let start = std::time::Instant::now();
        tracing::trace!("[apply_concurrent_mutations] ext_event: {:?}", ext);
        let expected_blocks = self.resolve_ref_blocks(ref_state, false);
        if let Err(e) = self.ctx.apply_external_mutation(&expected_blocks).await {
            eprintln!("[ConcurrentMutations] External mutation failed: {:?}", e);
        }
        let (entity, op, mut params) = ui.mutation.to_operation();
        if let Some(holon_api::Value::String(pid)) = params.get("parent_id") {
            let pid = holon_api::EntityUri::parse(pid).expect("Unable to parse parent_id");
            let resolved = self.resolve_uri(&pid);
            params.insert("parent_id".to_string(), resolved.clone().into());
        }
        let driver = self
            .driver
            .as_ref()
            .expect("driver not installed — was start_app called?");
        match driver.synthetic_dispatch(&entity, &op, params).await {
            Ok(()) => {}
            Err(e) => panic!("Concurrent UI mutation {}.{} failed: {:?}", entity, op, e),
        }
        let expected_count = ref_state.block_state.blocks.len();
        let expected_ids = self.expected_block_ids(ref_state);
        self.await_block_count_or_panic(
            &expected_ids,
            expected_count,
            std::time::Duration::from_millis(15000),
            "ConcurrentMutations",
        )
        .await;
        let expected_blocks = self.resolve_ref_blocks(ref_state, true);
        self.await_org_file_convergence(&expected_blocks).await;
        let _ = start;
    }

    async fn apply_apply_mutation(
        &mut self,
        event: crate::pbt::types::MutationEvent,
        ref_state: &ReferenceState,
    ) {
        tracing::trace!("[apply] Applying mutation: {:?}", event.mutation);
        self.apply_mutation(event, ref_state).await;
    }

    async fn apply_trigger_slash_command(&mut self, block_id: &holon_api::EntityUri) {
        let resolved_block_id = self.resolve_uri(block_id);
        tracing::trace!(
            "[apply] TriggerSlashCommand: block={block_id} (resolved={resolved_block_id})"
        );

        // 1. Query block data and render via render_entity() DSL
        let engine = self.engine();
        let sql = format!(
            "SELECT id, content, content_type, source_language, parent_id \
             FROM block_raw WHERE id = '{}'",
            resolved_block_id
        );
        let data_rows = engine
            .execute_query(sql, HashMap::new(), None)
            .await
            .expect("block query failed in TriggerSlashCommand");
        assert!(
            !data_rows.is_empty(),
            "[TriggerSlashCommand] Block {block_id} not found in database"
        );

        let render_expr = holon_api::render_types::RenderExpr::FunctionCall {
            name: "render_entity".to_string(),
            args: Vec::new(),
        };

        let engine_clone = Arc::clone(engine);
        let display_tree = tokio::task::spawn_blocking(move || {
            let services = holon_frontend::reactive::HeadlessBuilderServices::new(engine_clone);
            holon_frontend::interpret_pure(
                &render_expr,
                &data_rows
                    .iter()
                    .cloned()
                    .map(std::sync::Arc::new)
                    .collect::<Vec<_>>(),
                &services,
            )
            .snapshot()
        })
        .await
        .expect("spawn_blocking panicked");

        // 2. Find EditableText node for this block
        let editable = display_tree
            .find_editable_text(resolved_block_id.as_str())
            .unwrap_or_else(|| {
                panic!(
                    "[TriggerSlashCommand] No EditableText with id={resolved_block_id}.\n{}",
                    display_tree.pretty_print(0)
                )
            });

        // 3. Build EditorViewModel and simulate typing "/"
        let mut ctrl = holon_frontend::EditorViewModel::from_view_model(editable);
        let action = ctrl.on_text_changed("/", 1);
        assert!(
            matches!(action, holon_frontend::EditorAction::PopupActivated { .. }),
            "[TriggerSlashCommand] Expected PopupActivated for '/' on block {block_id}, got {:?}",
            action
        );

        // 4. Populate items synchronously (CommandProvider is sync)
        let context_params: HashMap<String, Value> = editable
            .entity
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let items = holon_frontend::command_provider::CommandProvider::build_command_items(
            &editable.operations,
            &context_params,
            "",
        );
        ctrl.set_popup_items(items);

        let popup_state = ctrl.popup_state().unwrap();
        let delete_idx = popup_state
            .items
            .iter()
            .position(|item| item.id == "delete")
            .unwrap_or_else(|| {
                panic!(
                    "[TriggerSlashCommand] No 'delete' operation in menu for block {block_id}.\n\
                     Available: {:?}",
                    popup_state.items.iter().map(|i| &i.id).collect::<Vec<_>>()
                )
            });

        // 5. Navigate to delete entry and select it
        for _ in 0..delete_idx {
            ctrl.on_key(holon_frontend::EditorKey::Down);
        }
        let action = ctrl.on_key(holon_frontend::EditorKey::Enter);
        match action {
            holon_frontend::EditorAction::Execute(intent) => {
                eprintln!(
                    "[TriggerSlashCommand] Executing {}.{} with {:?}",
                    intent.entity_name, intent.op_name, intent.params
                );
                let driver = self
                    .driver
                    .as_ref()
                    .expect("driver not installed — was start_app called?");
                driver
                    .apply_intent(intent)
                    .await
                    .expect("slash command operation failed");
            }
            other => panic!("[TriggerSlashCommand] Expected Execute, got {:?}", other),
        }
    }

    async fn apply_trigger_doc_link(
        &mut self,
        block_id: &holon_api::EntityUri,
        target_block_id: &holon_api::EntityUri,
        ref_state: &ReferenceState,
    ) {
        let resolved_block_id = self.resolve_uri(block_id);
        let resolved_target = self.resolve_uri(target_block_id);
        tracing::trace!(
            "[apply] TriggerDocLink: block={block_id} (resolved={resolved_block_id}) target={target_block_id}"
        );

        // 1. Render block → shadow interpret → ViewModel
        let engine = self.ctx.engine().clone();
        let data_rows = [{
            let mut row = HashMap::new();
            row.insert(
                "id".to_string(),
                Value::String(resolved_block_id.as_str().to_string()),
            );
            row
        }];

        let render_expr = holon_api::render_types::RenderExpr::FunctionCall {
            name: "render_entity".to_string(),
            args: Vec::new(),
        };

        let engine_clone = Arc::clone(&engine);
        let display_tree = tokio::task::spawn_blocking(move || {
            let services = holon_frontend::reactive::HeadlessBuilderServices::new(engine_clone);
            holon_frontend::interpret_pure(
                &render_expr,
                &data_rows
                    .iter()
                    .cloned()
                    .map(std::sync::Arc::new)
                    .collect::<Vec<_>>(),
                &services,
            )
            .snapshot()
        })
        .await
        .expect("spawn_blocking panicked");

        // 2. Find EditableText and build EditorViewModel
        let editable = display_tree
            .find_editable_text(resolved_block_id.as_str())
            .unwrap_or_else(|| {
                panic!(
                    "[TriggerDocLink] No EditableText with id={resolved_block_id}.\n{}",
                    display_tree.pretty_print(0)
                )
            });

        // 3. Verify triggers include doc_link
        assert!(
            editable.triggers.iter().any(|t| matches!(
                t,
                holon_frontend::input_trigger::InputTrigger::TextPrefix { action, .. }
                    if action == "doc_link"
            )),
            "[TriggerDocLink] EditableText for {block_id} has no doc_link trigger.\n\
             Triggers: {:?}",
            editable.triggers
        );

        // 4. Simulate typing "see [[proj" via EditorViewModel
        // Without async context, doc_link returns None (no LinkProvider).
        let mut ctrl = holon_frontend::EditorViewModel::from_view_model(editable);
        let action = ctrl.on_text_changed("see [[proj", 10);
        assert!(
            matches!(action, holon_frontend::EditorAction::None),
            "[TriggerDocLink] Expected None without async context, got {:?}",
            action
        );

        // 5. Test the InsertText result path directly via PopupMenu + manual items.
        // This bypasses the async LinkProvider but validates the full menu flow.
        let target_id = resolved_target.as_str().to_string();
        let target_label = ref_state
            .block_state
            .blocks
            .get(target_block_id)
            .map(|b| b.content.clone())
            .unwrap_or_else(|| "untitled".to_string());

        let items = vec![
            holon_frontend::popup_menu::PopupItem {
                id: target_id.clone(),
                label: target_label.clone(),
                icon: None,
            },
            holon_frontend::popup_menu::PopupItem {
                id: "__create_new__".to_string(),
                label: "Create new: proj".to_string(),
                icon: Some("\u{2795}".to_string()),
            },
        ];

        let mut menu = holon_frontend::popup_menu::PopupMenu::new();
        // Use a trivial mock provider to test menu mechanics
        struct LinkMockProvider;
        impl holon_frontend::popup_menu::PopupProvider for LinkMockProvider {
            fn source(&self) -> &str {
                "doc_link"
            }
            fn candidates(
                &self,
                _: std::pin::Pin<
                    Box<dyn futures_signals::signal::Signal<Item = String> + Send + Sync>,
                >,
            ) -> std::pin::Pin<
                Box<
                    dyn futures_signals::signal_vec::SignalVec<
                            Item = holon_frontend::popup_menu::PopupItem,
                        > + Send,
                >,
            > {
                Box::pin(futures_signals::signal_vec::always(vec![]))
            }
            fn on_select(
                &self,
                item: &holon_frontend::popup_menu::PopupItem,
                filter: &str,
            ) -> holon_frontend::popup_menu::PopupResult {
                let replacement = if item.id == "__create_new__" {
                    format!("[[{}]]", filter)
                } else {
                    format!("[[{}][{}]]", item.id, item.label)
                };
                holon_frontend::popup_menu::PopupResult::InsertText {
                    replacement,
                    prefix_start: 4,
                }
            }
        }

        let _signal = menu.activate(Arc::new(LinkMockProvider), "proj");
        menu.set_items(items);

        // Select existing entity (first item)
        let result = menu.on_key(holon_frontend::popup_menu::MenuKey::Enter);
        match result {
            holon_frontend::popup_menu::PopupResult::InsertText {
                replacement,
                prefix_start,
            } => {
                let expected = format!("[[{}][{}]]", target_id, target_label);
                assert_eq!(
                    replacement, expected,
                    "[TriggerDocLink] InsertText replacement mismatch"
                );
                assert_eq!(prefix_start, 4, "[TriggerDocLink] prefix_start mismatch");
            }
            other => panic!("[TriggerDocLink] Expected InsertText, got {:?}", other),
        }

        // Read-only transition — no state change
    }

    async fn apply_indent(&mut self, block_id: &holon_api::EntityUri) {
        tracing::trace!("[apply] Indent: block={block_id}");
        let resolved_id = self.resolve_uri(block_id);
        self.dispatch_block_op_via_chord("indent", resolved_id.as_str(), HashMap::new())
            .await;
    }

    async fn apply_outdent(&mut self, block_id: &holon_api::EntityUri) {
        tracing::trace!("[apply] Outdent: block={block_id}");
        let resolved_id = self.resolve_uri(block_id);
        self.dispatch_block_op_via_chord("outdent", resolved_id.as_str(), HashMap::new())
            .await;
    }

    async fn apply_move_up(&mut self, block_id: &holon_api::EntityUri) {
        tracing::trace!("[apply] MoveUp: block={block_id}");
        let resolved_id = self.resolve_uri(block_id);
        self.dispatch_block_op_via_chord("move_up", resolved_id.as_str(), HashMap::new())
            .await;
    }

    async fn apply_move_down(&mut self, block_id: &holon_api::EntityUri) {
        tracing::trace!("[apply] MoveDown: block={block_id}");
        let resolved_id = self.resolve_uri(block_id);
        self.dispatch_block_op_via_chord("move_down", resolved_id.as_str(), HashMap::new())
            .await;
    }

    async fn apply_drag_drop_block(
        &mut self,
        source: &holon_api::EntityUri,
        target: &holon_api::EntityUri,
    ) {
        tracing::trace!("[apply] DragDropBlock: source={source} target={target}");
        let resolved_source = self.resolve_uri(source);
        let resolved_target = self.resolve_uri(target);
        let (root_id, _root_tree) = self
            .current_reactive_tree()
            .expect("[DragDropBlock] No reactive tree — was start_app called?");
        self.wait_for_entity_bounds(resolved_source.as_str(), Duration::from_secs(5))
            .await
            .expect("[DragDropBlock] source bounds never appeared");
        self.wait_for_entity_bounds(resolved_target.as_str(), Duration::from_secs(5))
            .await
            .expect("[DragDropBlock] target bounds never appeared");
        let driver = self
            .driver
            .as_ref()
            .expect("[DragDropBlock] driver not installed");
        let dispatched = driver
            .drop_entity(
                root_id.as_str(),
                resolved_source.as_str(),
                resolved_target.as_str(),
            )
            .await
            .expect("[DragDropBlock] drop_entity failed");
        assert!(
            dispatched,
            "[DragDropBlock] drop_entity returned false for {source} → {target}"
        );
    }

    async fn apply_click_block(
        &mut self,
        region: holon_api::Region,
        block_id: &holon_api::EntityUri,
    ) {
        let resolved_id = self.resolve_uri(block_id);
        let _click_span = tracing::info_span!(
            "ClickBlock",
            region = ?region,
            block_id = %block_id,
            resolved = %resolved_id,
        )
        .entered();
        tracing::trace!(
            "[apply] ClickBlock: region={region:?} block={block_id} (resolved={resolved_id})"
        );

        // Wait for the entity to actually render — sidebar `live_block`
        // slots are populated asynchronously by their UiWatchers, and
        // clicking before they appear is the headless equivalent of
        // clicking dead pixels. We poll the engine's fully-resolved
        // `ViewModel` (which calls `ensure_watching` per nested block,
        // so all watchers fire) until our target entity shows up.
        let resolved = match self
            .wait_for_entity_in_resolved_view_model(resolved_id.as_str(), Duration::from_secs(5))
            .await
        {
            Some(vm) => vm,
            None => panic!(
                "[ClickBlock] entity {resolved_id} did not appear in the \
                 resolved ViewModel within 5s. Region={region:?}."
            ),
        };

        // Dispatch the bound click action if the rendered widget at this
        // entity has one (e.g. a sidebar selectable's `navigation.focus`).
        // Otherwise fall back to `navigation.editor_focus`, mirroring
        // GPUI's `render_entity` click handler.
        let driver = self
            .driver
            .as_ref()
            .expect("driver not installed — was start_app called?");
        // Region-scoped lookup: production GPUI's click handler runs on a
        // specific element in the clicked region, not across the whole tree.
        // The same entity_id may appear in multiple regions (e.g. `block:journals`
        // is both a LeftSidebar list item and a Main-panel doc) and bind
        // different actions per region. See FU-15.
        let bound_intent = holon_frontend::focus_path::find_click_intent_in_region(
            &resolved,
            resolved_id.as_str(),
            region.as_str(),
        );
        // Dispatch policy:
        //  * GPUI variant (frontend_geometry.is_some()) — drive a
        //    real mouse click so focus, editor mounting, chord
        //    resolution, and the bound action all run through
        //    production code. Geometry lookup falls back to
        //    `selectable-{id}` (default index.org sidebar) and then
        //    to entity_id scan, so sidebar selectables resolve.
        //  * Headless variants — no real input pipeline. Use the
        //    bound action's `synthetic_dispatch` if present;
        //    otherwise fall back to a synthesized click verb.
        //
        // Mouse-driven dispatch is fire-and-forget
        // (`services.dispatch_intent`, `reactive.rs:1448`) — unlike
        // the awaitable `dispatch_intent_sync` used by
        // `apply_intent`. Add an explicit focus-await barrier
        // before returning so subsequent transitions see a
        // populated focus.
        let dispatched_action = if self.frontend_geometry.is_some() {
            self.wait_for_entity_bounds(resolved_id.as_str(), Duration::from_secs(5))
                .await
                .unwrap_or_else(|e| {
                    panic!("[ClickBlock] {e} Region={region:?}.");
                });
            driver
                .click_entity(resolved_id.as_str(), region.as_str())
                .await
                .expect("[ClickBlock] click_entity failed");

            self.wait_for_focus_to_match(resolved_id.as_str(), Duration::from_secs(2))
                .await
                .unwrap_or_else(|e| {
                    panic!(
                        "[ClickBlock] focus did not propagate within 2s: {e} \
                     Region={region:?} expected={resolved_id}."
                    );
                });
            false
        } else if let Some(intent) = bound_intent {
            driver
                .apply_intent(intent)
                .await
                .expect("[ClickBlock] apply_intent failed");
            true
        } else {
            self.wait_for_entity_bounds(resolved_id.as_str(), Duration::from_secs(5))
                .await
                .unwrap_or_else(|e| {
                    panic!("[ClickBlock] {e} Region={region:?}.");
                });
            driver
                .click_entity(resolved_id.as_str(), region.as_str())
                .await
                .expect("[ClickBlock] click_entity failed");
            false
        };
        eprintln!(
            "[ClickBlock] {} (entity={resolved_id})",
            if dispatched_action {
                "dispatched bound action"
            } else {
                "real input pipeline / editor_focus"
            }
        );

        // Let CDC propagate (mirrors the yield_now dance ToggleState uses).
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
    }

    async fn apply_split_block(
        &mut self,
        block_id: &holon_api::EntityUri,
        position: usize,
        ref_state: &ReferenceState,
    ) {
        tracing::trace!("[apply] SplitBlock: block={block_id} position={position}");
        let resolved_id = self.resolve_uri(block_id);

        // Real users press Enter, not Ctrl+x. The Enter handler at
        // `editor_view.rs:543-575` is a capture_action that reads
        // `input.read(cx).cursor()` from the live `InputState` and
        // dispatches `split_block` directly — a separate code path
        // from the bubble-phase chord resolver that `Ctrl+x` hits.
        // Driving Enter exercises that production path.
        if let Err(e) = self
            .wait_for_entity_bounds(resolved_id.as_str(), Duration::from_secs(5))
            .await
        {
            let sql_probe = self.probe_block_sql_state(resolved_id.as_str()).await;
            panic!(
                "[SplitBlock] bounds unavailable for {resolved_id}: {e:#}\nSQL probe for missing entity:\n{sql_probe}"
            );
        }
        // Children-settled gate. `wait_for_entity_bounds` confirms the target
        // appears *somewhere* in the geometry, but coords resolved against a
        // partial first-render get invalidated by the next CDC batch that
        // adds siblings. Wait until every non-Page child of this block's
        // parent — as the PRE-transition ref-state predicted — has rendered
        // so `require_element_center` returns stable bounds. Uses the
        // pre-state instead of `ref_state` (post-transition) so the
        // predicate matches what the user can see right now. No-op when
        // pre-state isn't recorded yet or the parent has no children.
        let parent_for_settle = self
            .pre_ref_state
            .as_ref()
            .and_then(|s| s.block_state.blocks.get(block_id))
            .map(|b| b.parent_id.clone());
        if let Some(parent_id) = parent_for_settle {
            self.wait_for_children_settled(&parent_id, Duration::from_secs(5))
                .await
                .unwrap_or_else(|e| {
                    panic!(
                        "[SplitBlock] children of parent {parent_id} not settled before click: {e:#}"
                    )
                });
        }
        // Stronger precondition than bounds-exist: the block must be rendered
        // as an interactive widget (editable_text or its read-only sibling
        // rendered_text) so a click can either focus the editor or promote
        // the read-only variant. Mismatch here (e.g. block rendered as
        // `text` or not promoted at all) used to surface 1 s later as a
        // confusing "click didn't change focus" timeout.
        self.wait_for_widget_kind(
            resolved_id.as_str(),
            &["editable_text", "rendered_text"],
            Duration::from_secs(2),
        )
        .await
        .unwrap_or_else(|e| {
            panic!("[SplitBlock] target not rendered as editable_text/rendered_text: {e:#}")
        });
        let driver = self.driver.as_ref().expect("driver not installed");
        driver
            .click_entity(resolved_id.as_str(), "main")
            .await
            .unwrap_or_else(|e| {
                panic!("[SplitBlock] click_entity failed for {resolved_id}: {e:#}")
            });
        // Fail loud if the click didn't move keyboard focus to the target
        // editor. The Enter handler at `editor_view.rs:543-575` dispatches
        // `split_block` against whichever block's editor owns focus when
        // Enter fires — so a silent focus drift would have us splitting
        // the wrong block. The previous `dispatch_block_op_via_chord`
        // path bypassed this because it passed `id` as an explicit op
        // param; Enter reads `input.read(cx).cursor()` and `row_id` from
        // the focused editor.
        self.wait_for_focus_to_match(resolved_id.as_str(), Duration::from_secs(1))
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "[SplitBlock] click_entity did not focus {resolved_id} \
                     before Enter — split would have hit the wrong block: {e:#}"
                )
            });
        // Pre-Enter SQL snapshot: log the live content + length so panic-time
        // analysis can distinguish "cursor position drift" from "content
        // diverged before the split" cases.
        {
            let sql_pre = self.probe_block_sql_state(resolved_id.as_str()).await;
            let ref_content_len = ref_state
                .block_state
                .blocks
                .get(block_id)
                .map(|b| b.content_text().len());
            eprintln!(
                "[SplitBlock-presplit] target={resolved_id} position={position} \
                 ref_content_len={ref_content_len:?}\n{sql_pre}"
            );
        }
        driver
            .send_raw_keystroke("home", &[])
            .await
            .expect("[SplitBlock] home failed");
        for _ in 0..position {
            driver
                .send_raw_keystroke("right", &[])
                .await
                .expect("[SplitBlock] right failed");
        }
        driver
            .send_raw_keystroke("enter", &[])
            .await
            .expect("[SplitBlock] enter failed");

        let expected_count = Self::expected_content_block_count(ref_state);
        let expected_ids = self.expected_block_ids(ref_state);
        let timeout = std::time::Duration::from_secs(5);
        let db_rows = self.wait_for_blocks_synced(&expected_ids, timeout).await;
        if db_rows.len() != expected_count {
            let id_vec: Vec<String> = db_rows
                .iter()
                .filter_map(|r| r.get("id").and_then(|v| v.as_string()).map(String::from))
                .collect();
            let actual_ids: HashSet<String> = id_vec.iter().cloned().collect();
            let mut id_counts: std::collections::HashMap<String, u32> = HashMap::new();
            for id in &id_vec {
                *id_counts.entry(id.clone()).or_insert(0) += 1;
            }
            let duplicates: Vec<(String, u32)> = id_counts
                .iter()
                .filter(|(_, c)| **c > 1)
                .map(|(k, v)| (k.clone(), *v))
                .collect();
            let missing: Vec<&String> = expected_ids.difference(&actual_ids).collect();
            let extra: Vec<&String> = actual_ids.difference(&expected_ids).collect();
            eprintln!(
                "[SplitBlock count-mismatch diag] expected={} db_rows={} unique_ids={} duplicates={:?} missing_from_block_raw={:?} extra_in_block_raw={:?}",
                expected_count,
                db_rows.len(),
                actual_ids.len(),
                duplicates,
                missing,
                extra,
            );
        }
        assert_eq!(
            db_rows.len(),
            expected_count,
            "[SplitBlock] Block count mismatch after split"
        );

        // Capture pre-split known real ids so we can identify the freshly
        // created block among `db_rows` (mirrors map_unmapped_split_synthetic_ids).
        let pre_known: HashSet<String> = {
            let mut ids: HashSet<String> =
                self.doc_uri_map.values().map(|u| u.to_string()).collect();
            for ref_id in ref_state.block_state.blocks.keys() {
                if !self.doc_uri_map.contains_key(ref_id) && !ref_id.as_str().contains(":split-") {
                    ids.insert(ref_id.to_string());
                }
            }
            ids
        };
        self.map_unmapped_split_synthetic_ids(ref_state, &db_rows, "[SplitBlock]");

        // Production fires editor_focus(new_block) as a follow-up of
        // split_block. The chain is: SQL editor_cursor write → watch_editor_cursor
        // reactor → GPUI window.focus(new_input) → InputEvent::Focus →
        // services.set_focus(new_block) → engine.focused_block mirrors new.
        // This chain isn't synchronous with the Enter dispatch, so without
        // an explicit wait the next inv-focus-matches-ref check sees the
        // pre-split click_entity target (c2f12z-s) instead of the new block.
        // Mirrors the wait_for_focus_to_match barriers used by ClickBlock /
        // NavigateFocus.
        let new_block_real_id: Option<String> = db_rows
            .iter()
            .filter_map(|row| row.get("id")?.as_string().map(|s| s.to_string()))
            .find(|id| !pre_known.contains(id));
        if self.frontend_geometry.is_some() {
            if let Some(new_id) = new_block_real_id.as_deref() {
                // Best-effort barrier: try to absorb the
                // editor_cursor → window.focus(new) → InputEvent::Focus →
                // set_focus(new) propagation chain before the next step. If
                // it doesn't converge in 2s, fall through and let the
                // downstream inv-focus-matches-ref polling catch real
                // regressions — the new EditorView may not have mounted
                // yet (a real, separate fragility worth surfacing there).
                let _ = self
                    .wait_for_focus_to_match(new_id, Duration::from_secs(2))
                    .await;
            }
        }
    }

    async fn apply_join_block(
        &mut self,
        block_id: &holon_api::EntityUri,
        ref_state: &ReferenceState,
    ) {
        tracing::trace!("[apply] JoinBlock: block={block_id}");
        let resolved_id = self.resolve_uri(block_id);
        let mut extra_params = HashMap::new();
        extra_params.insert("position".into(), Value::Integer(0));
        self.dispatch_block_op_via_chord("join_block", resolved_id.as_str(), extra_params)
            .await;

        let expected_ids = self.expected_block_ids(ref_state);
        self.wait_for_blocks_synced(&expected_ids, Duration::from_secs(5))
            .await;
    }

    async fn apply_undo_last_mutation(&mut self, ref_state: &ReferenceState) {
        tracing::trace!("[apply] UndoLastMutation");
        let result = self.ctx.engine().undo().await;
        assert!(result.is_ok(), "undo failed: {:?}", result.err());
        assert!(result.unwrap(), "undo returned false (nothing to undo)");
        let expected_ids = self.expected_block_ids(ref_state);
        self.wait_for_blocks_synced(&expected_ids, Duration::from_secs(5))
            .await;
    }

    async fn apply_redo(&mut self, ref_state: &ReferenceState) {
        tracing::trace!("[apply] Redo");
        let result = self.ctx.engine().redo().await;
        assert!(result.is_ok(), "redo failed: {:?}", result.err());
        assert!(result.unwrap(), "redo returned false (nothing to redo)");
        let expected_ids = self.expected_block_ids(ref_state);
        self.wait_for_blocks_synced(&expected_ids, Duration::from_secs(5))
            .await;
    }

    async fn apply_emit_mcp_data(&mut self) {
        // EmitMcpData doesn't use ref_state, create a dummy call path.
        // We route through the transition enum to keep the logic centralized.
        // Since EmitMcpData has no payload and doesn't use ref_state,
        // we replicate the body inline here.
        tracing::trace!("[apply] EmitMcpData");
        if let Some(ref mcp) = self.pbt_mcp {
            mcp.emit_update()
                .await
                .expect("PbtMcpIntegration::emit_update failed");
        }
    }

    async fn apply_focus_editable_text(&mut self, block_id: &holon_api::EntityUri) {
        let resolved_id = self.resolve_uri(block_id);
        tracing::trace!("[apply] FocusEditableText: block={block_id} (resolved={resolved_id})");
        let driver = self
            .driver
            .as_ref()
            .expect("driver not installed — was start_app called?");
        // Fail loud: bounds must be present, click_entity must succeed.
        // The previous version fell back to `synthetic_dispatch` (engine
        // fast-path) and printed a warning, which silently masked any
        // bug in the keyboard nav / Enter pipeline. Per CLAUDE.md
        // ("fail loud, never fake") let both errors propagate.
        // 5s budget mirrors the other input-bearing call sites:
        // `wait_for_entity_bounds` now polls for ~200ms, RPCs a scroll-
        // into-view on the GPUI main thread (oneshot + layout + flush is
        // 50–200ms), and keeps polling. The 1s budget that worked when
        // generators only proposed visible candidates is too tight once
        // offscreen-virtualized-list entities become legal targets.
        self.wait_for_entity_bounds(resolved_id.as_str(), Duration::from_secs(5))
            .await
            .unwrap_or_else(|e| {
                panic!("[FocusEditableText] bounds unavailable for {resolved_id}: {e:#}")
            });
        driver
            .click_entity(resolved_id.as_str(), "main")
            .await
            .unwrap_or_else(|e| {
                panic!("[FocusEditableText] click_entity failed for {resolved_id}: {e:#}")
            });
    }

    async fn apply_move_cursor(&mut self, byte_position: usize) {
        tracing::trace!("[apply] MoveCursor: byte_position={byte_position}");
        let driver = self.driver.as_ref().expect("driver not installed");
        driver
            .send_raw_keystroke("home", &[])
            .await
            .expect("MoveCursor: home failed");
        for _ in 0..byte_position {
            driver
                .send_raw_keystroke("right", &[])
                .await
                .expect("MoveCursor: right failed");
        }
    }

    async fn apply_type_chars(&mut self, text: &str) {
        tracing::trace!("[apply] TypeChars: {:?}", text);
        let driver = self.driver.as_ref().expect("driver not installed");
        for ch in text.chars() {
            let keystroke = ch.to_string();
            driver
                .send_raw_keystroke(&keystroke, &[])
                .await
                .expect("TypeChars: send_raw_keystroke failed");
        }
    }

    async fn apply_delete_backward(&mut self, count: usize) {
        tracing::trace!("[apply] DeleteBackward: count={count}");
        let driver = self.driver.as_ref().expect("driver not installed");
        for _ in 0..count {
            driver
                .send_raw_keystroke("backspace", &[])
                .await
                .expect("DeleteBackward: backspace failed");
        }
    }

    async fn apply_press_key(&mut self, chord: &holon_api::KeyChord, ref_state: &ReferenceState) {
        tracing::trace!("[apply] PressKey: chord={:?}", chord);
        let driver = self.driver.as_ref().expect("driver not installed");
        use holon_api::Key;
        let modifiers: Vec<String> = chord
            .0
            .iter()
            .filter_map(|k| match k {
                Key::Cmd => Some("cmd".to_string()),
                Key::Ctrl => Some("ctrl".to_string()),
                Key::Alt => Some("alt".to_string()),
                Key::Shift => Some("shift".to_string()),
                _ => None,
            })
            .collect();
        let regulars: Vec<&'static str> = chord
            .0
            .iter()
            .filter_map(|k| match k {
                Key::Enter => Some("enter"),
                Key::Backspace => Some("backspace"),
                Key::Tab => Some("tab"),
                Key::Escape => Some("escape"),
                _ => None,
            })
            .collect();
        let has_enter = regulars.iter().any(|k| *k == "enter");
        let mod_refs: Vec<&str> = modifiers.iter().map(|s| s.as_str()).collect();
        for key in regulars {
            driver
                .send_raw_keystroke(key, &mod_refs)
                .await
                .expect("PressKey: send_raw_keystroke failed");
        }
        // Enter dispatches `split_block`, which materializes a fresh
        // UUID for the suffix block. Hand that back to the synthetic
        // `block::split-N` slot the ref-state allocated, mirroring
        // `apply_split_block`'s mapping step. Without this the next
        // step's `assert_blocks_equivalent` compares prod-UUID against
        // ref-synthetic-id and panics on what is logically the same
        // block.
        if has_enter {
            let expected_ids = self.expected_block_ids(ref_state);
            let timeout = std::time::Duration::from_secs(5);
            let db_rows = self.wait_for_blocks_synced(&expected_ids, timeout).await;
            self.map_unmapped_split_synthetic_ids(ref_state, &db_rows, "[PressKey-Enter]");
        }
    }

    async fn apply_arrow_navigate(
        &mut self,
        region: holon_api::Region,
        direction: holon_frontend::navigation::NavDirection,
        steps: u8,
        ref_state: &ReferenceState,
    ) {
        tracing::trace!(
            "[apply] ArrowNavigate: region={region:?} direction={direction:?} steps={steps}"
        );

        // Diagnostic only — informs panic messages further down. We
        // no longer poke `engine.ui_state().set_focus()` here; the
        // production handler in `app_main.rs` (`advance_focus`) is
        // what walks selectables in response to arrow keys, so we
        // emit the keystrokes and let it do the work. If
        // `predicted_focus` and `actual_focus` diverge, an inv-focus-matches-ref-style
        // assertion downstream catches it as a real bug instead of
        // forcing a match.
        let predicted_focus = ref_state
            .focused_entity_id
            .get(&region)
            .expect("ArrowNavigate requires focused entity")
            .clone();
        eprintln!(
            "[ArrowNavigate] {steps}×{direction:?} → predicted final focus: {predicted_focus}"
        );

        // Map NavDirection → raw keystroke name accepted by
        // `raw_keystroke_to_input_event`. The TUI's production handler
        // dispatches arrow keys to `advance_focus`/`switch_region`
        // (no leader involved).
        use holon_frontend::navigation::NavDirection::{Down, Left, Right, Up};
        let keystroke = match direction {
            Up => "up",
            Down => "down",
            Left => "left",
            Right => "right",
        };
        let driver = self
            .driver
            .as_ref()
            .expect("ArrowNavigate: driver not installed — was start_app called?");
        for _ in 0..steps {
            driver
                .send_raw_keystroke(keystroke, &[])
                .await
                .unwrap_or_else(|e| {
                    panic!("[ArrowNavigate] keystroke '{keystroke}' failed: {e:#}")
                });
        }
    }

    async fn apply_add_peer(&mut self) {
        tracing::trace!("[apply] AddPeer (peer_idx={})", self.peers.len());
        let doc_store = self
            .ctx
            .doc_store()
            .expect("AddPeer requires Loro to be enabled");
        let store = doc_store.read().await;
        let global_doc = store
            .get_global_doc()
            .await
            .expect("Failed to get global doc for AddPeer");
        let snapshot = global_doc
            .export_snapshot()
            .expect("Failed to export snapshot for AddPeer");
        let peer_id = (self.peers.len() as u64) + 100;
        let peer_doc = holon::sync::multi_peer::init_doc(peer_id);
        peer_doc
            .import(&snapshot)
            .expect("Failed to import snapshot into peer");
        self.peers.push(holon::sync::multi_peer::PeerState {
            doc: peer_doc,
            peer_id,
            online: true,
            data: (),
        });
    }

    async fn apply_peer_edit(&mut self, peer_idx: usize, op: &crate::pbt::transitions::PeerEditOp) {
        use super::transitions::PeerEditOp;
        let peer = &self.peers[peer_idx];
        tracing::trace!("[apply] PeerEdit peer_idx={} op={:?}", peer_idx, op);
        match op {
            PeerEditOp::Create {
                parent_stable_id,
                content,
                stable_id,
            } => {
                super::peer_ops::peer_create_block(
                    &peer.doc,
                    parent_stable_id.as_deref(),
                    content,
                    stable_id,
                );
            }
            PeerEditOp::Update { stable_id, content } => {
                let resolved = self.resolve_stable_id(stable_id);
                super::peer_ops::peer_update_block(&peer.doc, &resolved, content);
            }
            PeerEditOp::Delete { stable_id } => {
                let resolved = self.resolve_stable_id(stable_id);
                super::peer_ops::peer_delete_block(&peer.doc, &resolved);
            }
        }
    }

    async fn apply_sync_with_peer(&mut self, peer_idx: usize) {
        tracing::trace!("[apply] SyncWithPeer peer_idx={}", peer_idx);
        let doc_store = self
            .ctx
            .doc_store()
            .expect("SyncWithPeer requires Loro to be enabled");
        let store = doc_store.read().await;
        let global_doc = store
            .get_global_doc()
            .await
            .expect("Failed to get global doc for SyncWithPeer");
        let primary_doc = global_doc.doc();
        let primary = &*primary_doc;
        let peer = &self.peers[peer_idx];
        holon::sync::multi_peer::sync_docs_direct(&primary, &peer.doc);
        drop(store);
        // Give the controller's spawned task time to process the
        // peer import via subscribe_root → on_loro_changed → SQL.
        self.ctx
            .wait_for_loro_quiescence(Duration::from_secs(10))
            .await;
    }

    async fn apply_merge_from_peer(&mut self, peer_idx: usize) {
        tracing::trace!("[apply] MergeFromPeer peer_idx={}", peer_idx);
        let doc_store = self
            .ctx
            .doc_store()
            .expect("MergeFromPeer requires Loro to be enabled");
        let store = doc_store.read().await;
        let global_doc = store
            .get_global_doc()
            .await
            .expect("Failed to get global doc for MergeFromPeer");
        // One-directional merge: export the peer's delta relative
        // to the primary's current version and import it into the
        // primary. The raw `doc.import` is enough — the
        // `LoroSyncController`'s `subscribe_root` will fire and
        // reconcile the diff into SQL via the command bus.
        let primary_doc = global_doc.doc();
        let primary = &*primary_doc;
        let peer = &self.peers[peer_idx];
        let peer_vv = primary.oplog_vv();
        let delta = peer
            .doc
            .export(loro::ExportMode::updates(&peer_vv))
            .expect("Failed to export peer delta");
        if !delta.is_empty() {
            primary.import(&delta).expect("Failed to import peer delta");
        }
        drop(store);
        self.ctx
            .wait_for_loro_quiescence(Duration::from_secs(10))
            .await;
    }

    async fn apply_peer_char_edit(
        &mut self,
        peer_idx: usize,
        block_id: &str,
        op: &crate::pbt::transitions::TextOp,
    ) {
        use super::transitions::TextOp;
        let peer = &self.peers[peer_idx];
        let resolved_id = self.resolve_stable_id(block_id);
        match op {
            TextOp::Insert {
                pos_codepoint,
                text,
            } => {
                super::peer_ops::peer_insert_text(&peer.doc, &resolved_id, *pos_codepoint, text);
            }
            TextOp::Delete {
                pos_codepoint,
                len_codepoint,
            } => {
                super::peer_ops::peer_delete_text(
                    &peer.doc,
                    &resolved_id,
                    *pos_codepoint,
                    *len_codepoint,
                );
            }
        }
    }

    async fn apply_pin_block(
        &mut self,
        region: holon_api::Region,
        block_id: &holon_api::EntityUri,
    ) {
        let resolved_id = self.resolve_uri(block_id);
        tracing::trace!(
            "[apply] PinBlock: region={region:?} block={block_id} (resolved={resolved_id})"
        );
        let driver = self
            .driver
            .as_ref()
            .expect("driver not installed — was start_app called?");
        // Production binding is shift+click on a bullet — no leader chord
        // exists. The headless PBT mirrors the dispatch faithfully via
        // `synthetic_dispatch`. Architecture rule: the smell is
        // `execute_op("navigation", ...)`, NOT `synthetic_dispatch`
        // (`archlint/smells/focus.toml`).
        let mut params = HashMap::new();
        params.insert(
            "region".to_string(),
            Value::String(region.as_str().to_string()),
        );
        params.insert(
            "block_id".to_string(),
            Value::String(resolved_id.as_str().to_string()),
        );
        driver
            .synthetic_dispatch("navigation", "focus_pin", params)
            .await
            .unwrap_or_else(|e| {
                panic!("[PinBlock] synthetic_dispatch(navigation, focus_pin) failed: {e:#}")
            });
        self.ctx.drain_region_cdc_events().await;
        self.dump_nav_tables("after PinBlock").await;
    }

    async fn apply_unpin_block(&mut self, history_id: i64) {
        tracing::trace!("[apply] UnpinBlock: history_id={history_id}");
        let driver = self
            .driver
            .as_ref()
            .expect("driver not installed — was start_app called?");
        let mut params = HashMap::new();
        params.insert("history_id".to_string(), Value::Integer(history_id));
        driver
            .synthetic_dispatch("navigation", "close", params)
            .await
            .unwrap_or_else(|e| {
                panic!("[UnpinBlock] synthetic_dispatch(navigation, close) failed: {e:#}")
            });
        self.ctx.drain_region_cdc_events().await;
        self.dump_nav_tables("after UnpinBlock").await;
    }

    async fn apply_expand_toggle(&mut self, block_id: &holon_api::EntityUri) {
        self.set_expand_toggle_gate(block_id, true).await;
    }

    async fn apply_collapse_toggle(&mut self, block_id: &holon_api::EntityUri) {
        self.set_expand_toggle_gate(block_id, false).await;
    }
}
