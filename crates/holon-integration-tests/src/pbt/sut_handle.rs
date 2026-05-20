//! `SutHandle` trait impl for `E2ESut` — the per-transition SUT-side
//! dispatch surface. Each method here is the concrete reaction the wide
//! PBT runs when proptest hands it a transition variant.
//!
//! The trait itself lives in [`crate::pbt::transition_dispatch::SutHandle`].
//! Many methods are thin chord/driver dispatches; a few
//! (`apply_start_app`, `apply_bulk_external_add`, `apply_split_block`,
//! `apply_trigger_doc_link`, `apply_trigger_slash_command`) still carry
//! inline business logic that Phase C migration will move into
//! per-transition modules under `pbt/transitions/`.
//!
//! `apply_edit_via_display_tree` + `apply_edit_via_view_model` were
//! deleted in Phase C #5: both were `apply_intent` shortcuts that the
//! atomic-editor primitives (FocusEditableText + TypeChars +
//! DeleteBackward) already cover with real user input. See TUI TODO A6.
//!
//! Extracted from `sut.rs` (Phase D3).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use holon_api::{QueryLanguage, Value};
use holon_frontend::reactive::BuilderServices;
use holon_orgmode::OrgBlockExt;
use holon_pbt_core::capabilities::{EngineFocus, SutDriver};

use super::reference_state::ReferenceState;
use super::sut::E2ESut;
use super::types::*;

#[allow(async_fn_in_trait)]
impl crate::pbt::transition_dispatch::SutHandle for E2ESut {
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
        // The driver's click-to-focus fallback requires a valid region. Infer
        // from the element_id prefix; default to "main" for generic clicks.
        let region = if element_id.contains("left-sidebar") || element_id.contains("left_sidebar") {
            "left_sidebar"
        } else if element_id.contains("right-sidebar") || element_id.contains("right_sidebar") {
            "right_sidebar"
        } else {
            "main"
        };
        // Element ids come in two shapes: a plain block EntityUri, or a
        // geometry HANDLE `<kind>::<block-uri>` minted by
        // `holon_frontend::geometry::{drawer_toggle,expand_toggle,vms_button}_id_for`.
        // Handles are NOT EntityUris (the kind prefix isn't a scheme); the
        // headless intent walker resolves clicks by the TARGET block's uri,
        // so unwrap the handle and click that. Parse fail-loud either way —
        // a bare unmappable id means a transition generated a bogus handle.
        let target = match element_id.split_once("::") {
            // `block::split-N` synthetic ids also contain "::" but their
            // prefix is the `block` scheme, not a widget kind — only unwrap
            // when the suffix itself parses as a schemed uri.
            Some((kind, suffix)) if !kind.contains(':') && suffix.contains(':') => suffix,
            _ => element_id,
        };
        let element_uri = holon_api::EntityUri::parse(target).unwrap_or_else(|e| {
            panic!(
                "[LayoutPBT::click_at_element] {element_id:?} (target {target:?}) \
                 is not an EntityUri: {e}"
            )
        });
        driver
            .click_entity(&element_uri, region)
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

        // If app is running, wait for FileSyncController to ingest the file
        // and re-key ctx.documents from `file:<filename>` to the resolved
        // doc URI. Mirrors the start_app loop (see apply_start_app body):
        // without this, subsequent transitions like apply_bulk_external_add
        // that resolve the doc via `resolve_uri` (which checks doc_uri_map)
        // and then `ctx.documents.get(&resolved)` will miss because docs
        // added post-startup never got re-keyed. Backend-agnostic now:
        // `resolve_doc_uri_by_name` reads Turso's `block_raw` or the Loro
        // snapshot per the active storage (the no-Turso org→Loro ingest runs
        // through the Loro-wired `FileSyncController`).
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
                    let mut map = self.doc_uri_map.lock().unwrap();
                    if !map.contains_key(&resolved) {
                        map.insert(resolved.clone(), resolved.clone());
                    }
                    break;
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
    }

    async fn apply_create_directory(&mut self, path: &str) {
        tracing::trace!("[apply] CreateDirectory: {}", path);
        let full_path = self.org_root().join(path);
        self.org_fs.mkdir_all(&full_path);
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
        fields(wait_for_ready, enable_fake_mcp, enable_loro),
        name = "pbt.apply_start_app"
    )]
    async fn apply_start_app(
        &mut self,
        ref_state: &ReferenceState,
        wait_for_ready: bool,
        enable_fake_mcp: bool,
        enable_loro: bool,
    ) {
        tracing::trace!(
            "[apply] StartApp (wait_for_ready={}, enable_fake_mcp={}, enable_loro={})",
            wait_for_ready,
            enable_fake_mcp,
            enable_loro
        );
        self.set_enable_fake_mcp(enable_fake_mcp);
        self.set_enable_loro(enable_loro);
        self.start_app(wait_for_ready)
            .await
            .expect("Failed to start app");

        // Install the default mutation driver now that the engine exists.
        if self.driver.is_none() {
            self.install_driver();
        }

        // No-Turso (Loro-only) session: the Turso machinery below — MCP
        // `DebugServices`, `render_entity`, CDC region/all-blocks watches, org
        // doc-uri resolution, seed priming — does not exist in this wiring. This
        // is the one place that branches on the *backend the harness chose to
        // start* (an explicit `StorageSelector`, not a capability-presence proxy),
        // because it sets up backend-specific test scaffolding rather than reading
        // through a unified capability. The reactive engine renders structural
        // blocks straight from `block_query`, so the driver install above is the
        // whole start-time setup. Kick off a root watcher so geometry/data exist.
        if matches!(self.ctx.storage(), holon::di::StorageSelector::LoroMemory) {
            // No-Turso seeds through the real org→Loro ingestion path: the
            // pre-startup org files written by `WriteOrgFile` are scanned and
            // parsed into the Loro backend by the Loro-wired `FileSyncController`
            // at container build time (see `build_no_turso_container`). Ref and
            // SUT each derive from the rendered org independently — no reference
            // state is read to populate the SUT.
            let root_id = ref_state
                .root_layout_block_id()
                .unwrap_or_else(holon_api::root_layout_block_uri);
            if let Some(reactive) = self.ctx.reactive_engine.as_ref() {
                reactive.ensure_watching(&root_id);
            }

            // Map pre-startup documents the same way the Turso path does below:
            // the FileSyncController ingested each `WriteOrgFile` file into Loro,
            // so resolve its page block by name and re-key `ctx.documents` from
            // the `file:<filename>` placeholder to the real doc URI. Reading
            // `ref_state.files.documents` here is only `#+ID` identity-pinning
            // (the synthetic URI was injected into the file the SUT itself
            // wrote) — not seeding SUT content from ref output.
            for (synthetic_uri, filename) in &ref_state.files.documents {
                if self.doc_uri_map.lock().unwrap().contains_key(synthetic_uri) {
                    continue;
                }
                match self.ctx.resolve_doc_uri_by_name(filename).await {
                    Ok(resolved) => {
                        self.doc_uri_map
                            .lock()
                            .unwrap()
                            .insert(synthetic_uri.clone(), resolved.clone());
                        let file_key = holon_api::EntityUri::file(filename);
                        if let Some(path) = self.ctx.documents.remove(&file_key) {
                            self.ctx.documents.insert(resolved, path);
                        }
                    }
                    Err(e) => {
                        tracing::trace!(
                            "[apply] (no-Turso) could not resolve pre-startup doc {}: {}",
                            synthetic_uri,
                            e
                        );
                    }
                }
            }

            // Install the LoroSut peer surface so CRDT peer transitions
            // (AddPeer, PeerEdit, …) — gated `HasStorage(Loro)`, hence generated
            // under {Loro} — have their owned peer state. The no-Turso session
            // has a doc_store but no LoroSyncController, so `sync_handle` is None.
            if self.ctx.doc_store().is_some() {
                let doc_store = self.ctx.doc_store().unwrap().clone();
                let sync_handle = self.ctx.loro_sync_handle().cloned();
                let doc_uri_map = self.doc_uri_map.clone();
                self.loro_sut = Some(crate::pbt::sut_loro::LoroSut::new(
                    doc_store,
                    sync_handle,
                    doc_uri_map,
                ));
            }
            return;
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

        // Populate doc_uri_map for pre-startup documents whose document
        // entities were created by FileSyncController during startup.
        for (synthetic_uri, filename) in &ref_state.files.documents {
            // Short-lived lock (never held across the `.await` below).
            if self.doc_uri_map.lock().unwrap().contains_key(synthetic_uri) {
                continue;
            }
            match self.ctx.resolve_doc_uri_by_name(filename).await {
                Ok(resolved) => {
                    tracing::trace!(
                        "[apply] Mapped pre-startup doc: {} → {}",
                        synthetic_uri,
                        resolved
                    );
                    self.doc_uri_map
                        .lock()
                        .unwrap()
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

        // Initialize LoroSut if Loro is enabled. It owns the peer surface and
        // is self-sufficient: it gets the primary doc_store, the sync-controller
        // handle (for quiescence), and a clone of the shared doc_uri_map (for
        // stable-id resolution that sees ids minted after this point).
        if self.ctx.doc_store().is_some() {
            tracing::trace!("[apply] Loro enabled — initializing LoroSut for invariant checking");
            let doc_store = self.ctx.doc_store().unwrap().clone();
            let sync_handle = self.ctx.loro_sync_handle().cloned();
            let doc_uri_map = self.doc_uri_map.clone();
            self.loro_sut = Some(crate::pbt::sut_loro::LoroSut::new(
                doc_store,
                sync_handle,
                doc_uri_map,
            ));
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
        // The headless `click_entity` silently falls through to a plain
        // `set_focus` (click-to-focus) when the sidebar entry's bound
        // `navigation.focus` intent isn't yet resolvable — the sidebar
        // `live_block` streams in asynchronously after a fresh layout load /
        // block-matview propagation, so a click that lands before it paints
        // hits that focus-only fallback. The fallback does NOT write
        // `navigation_history`, so `current_focus` stays on the journals
        // default while the ref records the focus move — a silent divergence
        // that only surfaces ~1000 lines later in check_invariants.
        //
        // A real user waits for the sidebar to paint before clicking. Mirror
        // that: click, verify `current_focus` actually moved to the target,
        // and retry until it does. Fail loud (dumping what the sidebar can
        // see) if it never does within the deadline — that's a genuine
        // render/projection bug, not something to paper over.
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let driver = self
                .driver
                .clone()
                .expect("driver not installed — was start_app called?");
            driver
                .click_entity(&resolved_id, "left_sidebar")
                .await
                .unwrap_or_else(|e| {
                    panic!(
                        "[NavigateFocus] click_entity failed for sidebar entry {resolved_id}: {e:#}"
                    )
                });
            self.ctx.drain_region_cdc_events().await;

            let focus_rows = self
                .engine()
                .execute_query(
                    "SELECT block_id FROM current_focus WHERE region = 'main'".to_string(),
                    HashMap::new(),
                    None,
                )
                .await
                .expect("[NavigateFocus] query current_focus");
            let actual = focus_rows
                .first()
                .and_then(|r| r.get("block_id"))
                .and_then(|v| v.as_string())
                .map(str::to_string);
            if actual.as_deref() == Some(resolved_id.as_str()) {
                break;
            }
            if Instant::now() >= deadline {
                let page_rows = self
                    .engine()
                    .execute_query(
                        "SELECT b.id FROM block b JOIN block_tags bt ON bt.block_id = b.id \
                         WHERE bt.tag = 'Page'"
                            .to_string(),
                        HashMap::new(),
                        None,
                    )
                    .await
                    .expect("[NavigateFocus] query Page rows");
                let pages: Vec<String> = page_rows
                    .iter()
                    .filter_map(|r| r.get("id").and_then(|v| v.as_string()).map(str::to_string))
                    .collect();
                let intent = self
                    .driver
                    .as_ref()
                    .and_then(|d| d.click_intent_of(&resolved_id));
                self.dump_nav_tables("NavigateFocus FAILED").await;
                panic!(
                    "[NavigateFocus] sidebar click never moved current_focus(main) to \
                     {resolved_id} within 10s (last seen {actual:?}). The LeftSidebar entry's \
                     navigation.focus intent was not dispatched — the sidebar live_block has not \
                     rendered the target. sidebar Page rows={pages:?}; click_intent_of={intent:?}"
                );
            }
            // Re-click as soon as a fresh frame commits (the usual reason the
            // click missed is the sidebar hadn't painted yet); the timeout
            // keeps the old 50 ms cadence as a floor for non-painting windows
            // and headless providers on the default 20 ms tick.
            match self.render.frontend_geometry.as_ref() {
                Some(geometry) => {
                    let _ =
                        tokio::time::timeout(Duration::from_millis(50), geometry.changed()).await;
                }
                None => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
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
                    .files
                    .documents
                    .iter()
                    .find(|(_, name)| *name == file_name)
                    .map(|(uri, _)| uri.clone())
                    .expect("CreateDocument: synthetic URI not found in reference state");
                tracing::trace!("[apply] Created document: {} → {}", synthetic_uri, uuid_uri);
                self.doc_uri_map
                    .lock()
                    .unwrap()
                    .insert(synthetic_uri, uuid_uri);
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

    async fn apply_toggle_state(
        &mut self,
        block_id: &holon_api::EntityUri,
        new_state: crate::pbt::transitions::toggle_state::CycleTarget,
    ) {
        let resolved_block_id = self.resolve_uri(block_id);
        // Real-user-input dispatch: compute click_count from the
        // pre-mutation task_state, then click the state_toggle widget
        // that many times. Replaces the previous apply_intent backend
        // shortcut and the ViewModel-walking assertions (those concerns
        // — keychord-joined op, post-CDC enrichment — are invariant-
        // shaped; promote to `pbt/invariants/bodies/` if needed).
        let current_state: String = self
            .pre_ref_state
            .as_ref()
            .and_then(|s| s.domain.block_state.blocks.get(block_id))
            .and_then(|b| b.task_state())
            .map(|ts| ts.keyword.to_string())
            .unwrap_or_default();
        let click_count =
            crate::pbt::transitions::toggle_state::cycle_click_count(&current_state, new_state);
        tracing::trace!(
            "[apply] ToggleState: block={block_id} (resolved={resolved_block_id}) \
             {current_state:?} → {new_state:?} ({click_count} clicks)"
        );
        assert!(
            click_count > 0,
            "[ToggleState] click_count=0 ({current_state:?} == {new_state:?}) \
             — generator should exclude no-op transitions"
        );
        crate::pbt::transitions::toggle_state::apply_toggle_state_to_sut(
            self,
            &resolved_block_id,
            click_count,
        )
        .await;
    }

    async fn apply_bulk_external_add(
        &mut self,
        doc_uri: &holon_api::EntityUri,
        blocks: &[holon_api::block::Block],
        ref_state: &ReferenceState,
    ) {
        crate::pbt::transitions::bulk_external_add::apply_bulk_external_add_to_sut(
            self, doc_uri, blocks, ref_state,
        )
        .await;
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
        crate::pbt::transitions::trigger_slash_command::apply_trigger_slash_command_to_sut(
            self,
            &resolved_block_id,
        )
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
            .drop_entity(&root_id, &resolved_source, &resolved_target)
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
        crate::pbt::transitions::click_block::apply_click_block_to_sut(
            self,
            region.as_str(),
            &resolved_id,
        )
        .await;
        // Let CDC propagate (mirrors the yield_now dance ToggleState uses).
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
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
        crate::pbt::transitions::focus_editable_text::apply_focus_editable_text_to_sut(
            self,
            &resolved_id,
        )
        .await;
    }

    // `apply_type_chars` / `apply_delete_backward` / `apply_move_cursor`
    // moved to `impl SutEditorMirrorWrite for E2ESut` (sut_capabilities.rs);
    // `SutHandle` lists `SutEditorMirrorWrite` as a supertrait.

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
        let has_enter = regulars.contains(&"enter");
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
            // Turso barrier: let block_raw converge to the projected split row
            // before the mapper reads it. The placeholder split id is treated as
            // count-only by `wait_for_blocks_synced` (synthetic ids never reach
            // CDC), so this converges as soon as the real split row lands —
            // non-convergence surfaces in the mapper's count assert below.
            // No-Turso's Loro split is synchronous — the mapper reads the
            // snapshot directly, no barrier needed.
            if matches!(self.ctx.storage(), holon::di::StorageSelector::Turso) {
                let expected_ids = self.expected_block_ids(ref_state);
                let timeout = std::time::Duration::from_secs(5);
                self.wait_for_blocks_synced(&expected_ids, timeout).await;
            }
            self.map_unmapped_split_synthetic_ids(ref_state, "[PressKey-Enter]")
                .await;
            // Prod's split sets focus + caret on the new block (caret 0) via
            // the op response, applied in-process (ADR 0010). VERIFY the SUT's
            // own focus landed where the ref expects before parking the
            // mirror's caret — deriving the target from `ref_state` alone
            // would re-impose the expected focus and mask a regressed
            // focus handoff (the oracle-circularity the Jun-2026 review
            // flagged). The caret seed itself (`home`) stays: the headless
            // mirror tracks its caret independently and defaults to
            // end-of-text.
            if let Some(active) = ref_state.ui.tab.active_editor.as_ref() {
                let expected_id = self.resolve_uri(&active.block_id);
                // The op-response focus handoff (`apply_structural_focus`,
                // ADR 0010) runs in the spawned dispatch task — block_raw
                // converging (the barrier above) does NOT imply focus has
                // moved yet. A single sample here raced that task and
                // produced flaky "handoff DIVERGED, engine focused <old
                // block>" failures (2026-06-11, window-active runs where
                // the busier main thread widened the race window). Poll
                // until convergence; the deadline keeps a genuinely
                // regressed handoff loud.
                let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
                loop {
                    match SutDriver::engine_focused_block(self).await {
                        EngineFocus::Focused(actual) => {
                            if actual == expected_id {
                                break;
                            }
                            if tokio::time::Instant::now() >= deadline {
                                panic!(
                                    "[PressKey-Enter] split focus handoff DIVERGED: engine \
                                     focused {actual}, ref expects the new split block \
                                     {expected_id} (after 2s — async op-response focus \
                                     application never converged)"
                                );
                            }
                        }
                        EngineFocus::Unfocused => {
                            if tokio::time::Instant::now() >= deadline {
                                panic!(
                                    "[PressKey-Enter] split focus handoff LOST: engine has no \
                                     focused block, ref expects the new split block \
                                     {expected_id} (after 2s)"
                                );
                            }
                        }
                        // No frontend engine wired (SqlOnly headless): the op-response
                        // focus is unobservable here — disclosed, not silently skipped.
                        EngineFocus::NoEngine => {
                            eprintln!(
                                "[PressKey-Enter] split focus handoff UNVERIFIED \
                                 (no frontend engine); seeding caret on ref expectation \
                                 {expected_id}"
                            );
                            break;
                        }
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                }
                self.sync_caret_to_new_split_block(&expected_id).await;
            }
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
            .ui
            .tab
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
            // Retry-until-consumed: after a focus-moving op (e.g. a split
            // landing focus on a freshly created row) the consuming editor
            // may mount on a later render pass — or need the focused row
            // scrolled back into the virtualized viewport first.
            driver
                .send_raw_keystroke_until_handled(keystroke, &[], Duration::from_secs(2))
                .await
                .unwrap_or_else(|e| {
                    panic!("[ArrowNavigate] keystroke '{keystroke}' failed: {e:#}")
                });
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
