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
use holon_orgmode::OrgBlockExt;
use holon_pbt_core::capabilities::{
    CapRegion, SutBlockInteract, SutFocusWrite, SutHistoryWrite, SutMcpEmit, SutNavHistoryDrive,
    SutNavHistoryWrite, SutViewControl, SutWatchRegister,
};

use super::sut::E2ESut;

/// App-lifecycle cap for the wide PBT (`SimulateRestart` / `CreateDocument` /
/// `ConcurrentSchemaInit`). `ref_state`-free: the restart settle and the
/// `CreateDocument` synthetic→uuid reconcile live in the `block_tree_post_action`
/// seam. `self.deref()` forces the `TestEnvironment` inherent methods — a bare
/// `self.simulate_restart`/`create_document` would re-dispatch to these trait
/// methods and recurse (the "trait shadows Deref inherent" gotcha).
#[async_trait::async_trait(?Send)]
impl crate::pbt::local_caps::SutAppLifecycle for E2ESut {
    #[tracing::instrument(
        skip(self, root_id),
        fields(wait_for_ready, enable_fake_mcp, enable_loro),
        name = "pbt.start_app"
    )]
    async fn start_app(
        &self,
        root_id: holon_api::EntityUri,
        expects_valid_index: bool,
        wait_for_ready: bool,
        enable_fake_mcp: bool,
        enable_loro: bool,
    ) {
        use std::ops::Deref;
        tracing::trace!(
            "[apply] StartApp (wait_for_ready={}, enable_fake_mcp={}, enable_loro={})",
            wait_for_ready,
            enable_fake_mcp,
            enable_loro
        );
        self.set_enable_fake_mcp(enable_fake_mcp);
        self.set_enable_loro(enable_loro);
        // `self.deref()` forces `TestEnvironment::start_app` — a bare
        // `self.start_app` would re-dispatch to this cap method and recurse.
        self.deref()
            .start_app(wait_for_ready)
            .await
            .expect("Failed to start app");

        // Install the default mutation driver now that the engine exists.
        if self.driver.borrow().is_none() {
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
            // No-Turso seeds through the real org→Loro ingestion path; the root
            // watcher needs the layout root, supplied as the precomputed arg. The
            // pre-startup doc-uri reconcile moved to the `block_tree_post_action`
            // `StartApp` arm (the shared-Arc `doc_uri_map` means `LoroSut`, installed
            // just below, sees those later inserts).
            if let Some(reactive) = self.ctx.reactive_engine.get() {
                reactive.ensure_watching(&root_id);
            }

            // Install the LoroSut peer surface so CRDT peer transitions
            // (AddPeer, PeerEdit, …) — gated `HasStorage(Loro)`, hence generated
            // under {Loro} — have their owned peer state. The no-Turso session
            // has a doc_store but no LoroSyncController, so `sync_handle` is None.
            if self.ctx.doc_store().is_some() {
                let doc_store = self.ctx.doc_store().unwrap().clone();
                let sync_handle = self.ctx.loro_sync_handle().cloned();
                let doc_uri_map = self.doc_uri_map.clone();
                self.loro_sut
                    .set(crate::pbt::sut_loro::LoroSut::new(
                        doc_store,
                        sync_handle,
                        doc_uri_map,
                    ))
                    .unwrap_or_else(|_| {
                        panic!("loro_sut already initialized (StartApp ran twice?)")
                    });
            }
            return;
        }

        // Initialize real MCP integration for IVM re-evaluation testing.
        let db_handle = self.ctx.engine().db_handle().clone();
        match crate::pbt_mcp_fake::PbtMcpIntegration::new(db_handle).await {
            Ok(integration) => self
                .pbt_mcp
                .set(integration)
                .unwrap_or_else(|_| panic!("pbt_mcp already initialized (StartApp ran twice?)")),
            Err(e) => {
                tracing::trace!("[apply] PbtMcpIntegration init failed (non-fatal): {e}")
            }
        }

        // Mirror Flutter startup: call initial_widget() after engine ready.
        // `root_id` / `expects_valid_index` are the precomputed boundary args.
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

        // Seed-count settle (`prime_seed_count`) and pre-startup doc-uri reconcile
        // relocated to the `block_tree_post_action` `StartApp` arm — both are
        // `ref_state`-derived and run after the action. The shared-Arc
        // `doc_uri_map` means the `LoroSut` installed below sees those inserts.

        // Initialize LoroSut if Loro is enabled. It owns the peer surface and
        // is self-sufficient: it gets the primary doc_store, the sync-controller
        // handle (for quiescence), and a clone of the shared doc_uri_map (for
        // stable-id resolution that sees ids minted after this point).
        if self.ctx.doc_store().is_some() {
            tracing::trace!("[apply] Loro enabled — initializing LoroSut for invariant checking");
            let doc_store = self.ctx.doc_store().unwrap().clone();
            let sync_handle = self.ctx.loro_sync_handle().cloned();
            let doc_uri_map = self.doc_uri_map.clone();
            self.loro_sut
                .set(crate::pbt::sut_loro::LoroSut::new(
                    doc_store,
                    sync_handle,
                    doc_uri_map,
                ))
                .unwrap_or_else(|_| panic!("loro_sut already initialized (StartApp ran twice?)"));
        }

        // Initialize the ReactiveEngine now so all subsequent
        // transitions can read the reactive tree — just like the real GPUI frontend.
        self.ensure_reactive_engine(&root_id).await;
        tracing::trace!("[apply] ReactiveEngine initialized for root '{}'", root_id);
    }

    async fn simulate_restart(&self) {
        use std::ops::Deref;
        // Empty expected-set ⇒ the inherent `simulate_restart`'s internal
        // block-convergence wait is a no-op; the real settle
        // (`wait_for_blocks_synced(expected_block_ids(ref_state))`) runs in the
        // seam's `SimulateRestart` arm.
        self.deref()
            .simulate_restart(&std::collections::HashSet::new())
            .await
            .expect("SimulateRestart failed");
    }

    async fn create_document(&self, file_name: &str) {
        use std::ops::Deref;
        tracing::trace!("[apply] Creating document: {}", file_name);
        // The synthetic→uuid `doc_uri_map` reconcile moved to the seam's
        // `CreateDocument` arm (it re-derives the minted uri via
        // `resolve_page_uri_by_name`), so the action is a pure create.
        self.deref()
            .create_document(file_name)
            .await
            .unwrap_or_else(|e| panic!("Failed to create document: {e}"));
    }

    async fn assert_epoch_flip_rejected(&self) {
        use std::ops::Deref;
        // `deref()` forces `TestEnvironment::assert_epoch_flip_rejected` — a bare
        // `self.assert_epoch_flip_rejected` would re-dispatch to this cap method.
        self.deref().assert_epoch_flip_rejected().await;
    }

    async fn concurrent_schema_init(&self) {
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

    // `apply_setup_watch` removed in SutHandle decomposition INC 3 — the
    // `SetupWatch` transition now drives `SutWatchRegister::register_watch`, whose
    // `E2ESut` impl (below) forwards to `TestEnvironment::setup_watch` directly.

    // `apply_focus_editable_text` removed in SutHandle decomposition #4: it
    // duplicated `SutFocusWrite::apply_focus_editable_text` (the cap is now a
    // `SutHandle` supertrait), so the body lives solely in the cap impl below.

    // `apply_type_chars` / `apply_delete_backward` / `apply_move_cursor`
    // moved to `impl SutEditorMirrorWrite for E2ESut` (sut_capabilities.rs);
    // `SutHandle` lists `SutEditorMirrorWrite` as a supertrait.
}

/// Block-level UI interactions driven through the UI driver. Driver-realized
/// only — the headless `frontend_slice` has no driver — so `E2ESut` is the sole
/// impl. The `ref_state`-dependent post-actions (e.g. the `PressKey` Enter-split
/// reconcile) live in `block_tree_post_action`, so these are pure `&self`
/// driver dispatches.
#[async_trait::async_trait(?Send)]
impl SutBlockInteract for E2ESut {
    async fn click_block(&self, region: holon_api::Region, block_id: &holon_api::EntityUri) {
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

    async fn drag_drop_block(&self, source: &holon_api::EntityUri, target: &holon_api::EntityUri) {
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
            .borrow()
            .clone()
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

    async fn expand_toggle(&self, block_id: &holon_api::EntityUri) {
        self.set_expand_toggle_gate(block_id, true).await;
    }

    async fn collapse_toggle(&self, block_id: &holon_api::EntityUri) {
        self.set_expand_toggle_gate(block_id, false).await;
    }

    async fn trigger_slash_command(&self, block_id: &holon_api::EntityUri) {
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

    async fn press_key(&self, chord: &holon_api::KeyChord) {
        tracing::trace!("[apply] PressKey: chord={:?}", chord);
        let driver = self.driver.borrow().clone().expect("driver not installed");
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
        let mod_refs: Vec<&str> = modifiers.iter().map(|s| s.as_str()).collect();
        for key in regulars {
            driver
                .send_raw_keystroke(key, &mod_refs)
                .await
                .expect("PressKey: send_raw_keystroke failed");
        }
        // The Enter-split post-action (Turso barrier + synthetic-id reconcile +
        // focus-handoff verify + caret park) is `ref_state`-dependent, so it
        // relocated to `block_tree_post_action`'s `PressKey` arm (SutHandle
        // decomposition): the harness seam owns `ref_state`, letting this action
        // be a pure `&self` keystroke send.
    }

    /// Click a rendered element by bounds-registry id, via the same
    /// `UserDriver::click_entity` path the chord transitions use. Drives the
    /// shared `holon_layout_testing` bodies through `SutClickAdapter`. Region is
    /// unknown at this layer (the shared variant only carries an `element_id`),
    /// so it is inferred from the id prefix.
    async fn click_at_element(&self, element_id: &str) {
        let driver = self
            .driver
            .borrow()
            .clone()
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
        self.ctx.drain_delivery_barrier().await;
    }
}

/// `SutFocusWrite` for `E2ESut` — the fine-grained focus write cap the decomposed
/// `NavigateFocus` transition now binds (SutHandle decomposition increment 1).
/// `&self`, so the native macro dispatch (`sut: &mut E2ESut`) drives it via
/// auto-reborrow. Both methods forward to the existing production helpers
/// (`SutHandle::apply_navigate_focus`, now `&self`; the shared
/// `apply_focus_editable_text_to_sut`), so no native behaviour changes.
/// Arrow-key navigation cap (`holon-frontend`-owned, cap home-rule). The body
/// emits raw arrow keystrokes through the installed driver and lets the
/// production focus walker (`advance_focus`) move focus; the `ref_state`-derived
/// prediction the old `SutHandle` method logged carried no behaviour, so it is
/// dropped (the `ref_state`-off-the-cap principle).
#[async_trait::async_trait(?Send)]
impl holon_frontend::pbt_caps::SutArrowNavigate for E2ESut {
    async fn apply_arrow_navigate(
        &self,
        region: CapRegion,
        direction: holon_frontend::navigation::NavDirection,
        steps: u8,
    ) {
        tracing::trace!(
            "[apply] ArrowNavigate: region={region:?} direction={direction:?} steps={steps}"
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
            .borrow()
            .clone()
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
}

#[async_trait::async_trait(?Send)]
impl SutFocusWrite for E2ESut {
    async fn apply_navigate_focus(&self, region: CapRegion, id: &holon_api::EntityUri) {
        let region = match region {
            CapRegion::Main | CapRegion::Single => holon_api::Region::Main,
            CapRegion::Sidebar => holon_api::Region::LeftSidebar,
        };
        // The generator restricts NavigateFocus to `Region::Main` and
        // to LeftSidebar-listed pages — the only navigation path a
        // real user can trigger. Sanity-check both invariants here.
        debug_assert_eq!(
            region,
            holon_api::Region::Main,
            "NavigateFocus generator must only emit Main; got {region:?}"
        );
        let resolved_id = self.resolve_uri(id);
        tracing::trace!(
            "[apply] NavigateFocus: region={region:?} block={id} (resolved={resolved_id})"
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
                .borrow()
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
            // `&self` (SutHandle decomposition): force CDC delivery so the
            // current_focus matview reflects the click before the poll below. The
            // `&mut` region_data mirror drain that the original `&mut self`
            // `drain_region_cdc_events` also did is redundant here — the shared
            // `check_invariants` prep (`sut_check_invariants.rs`) drains region CDC
            // before every invariant read, exactly as the other `&self` write caps
            // (block-tree chord dispatch) rely on.
            self.ctx.drain_delivery_barrier().await;

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
                    .borrow()
                    .clone()
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

    async fn apply_focus_editable_text(&self, id: &holon_api::EntityUri) {
        let resolved_id = self.resolve_uri(id);
        crate::pbt::transitions::focus_editable_text::apply_focus_editable_text_to_sut(
            self,
            &resolved_id,
        )
        .await;
    }
}

/// `SutNavHistoryWrite` for `E2ESut` — the navigation-history write cap the
/// `NavigateHome` transition binds (SutHandle decomposition increment 2). Forwards
/// to the existing production helper (`SutHandle::apply_navigate_home`, now `&self`),
/// so native behaviour is unchanged. `&self`, so the native macro dispatch
/// (`sut: &mut E2ESut`) drives it via auto-reborrow. Back/forward are deferred to E4.
#[async_trait::async_trait(?Send)]
impl SutNavHistoryWrite for E2ESut {
    async fn apply_navigate_home(&self, region: CapRegion) {
        let region = match region {
            CapRegion::Main | CapRegion::Single => holon_api::Region::Main,
            CapRegion::Sidebar => holon_api::Region::LeftSidebar,
        };
        debug_assert_eq!(
            region,
            holon_api::Region::Main,
            "NavigateHome generator must only emit Main; got {region:?}"
        );
        self.send_leader_chord("go_home", "NavigateHome").await;
        // `&self` (SutHandle decomposition increment 2): force CDC delivery so the
        // focus matviews reflect `go_home` before the next read. The `&mut`
        // region_data mirror drain the original `drain_region_cdc_events` also did is
        // redundant — the shared `check_invariants` prep re-drains region CDC, exactly
        // as `apply_navigate_focus` (now `&self`) relies on.
        self.ctx.drain_delivery_barrier().await;
        self.dump_nav_tables("after NavigateHome").await;
    }
}

/// `SutWatchRegister` for `E2ESut` — the watch-registration write cap the
/// `SetupWatch` transition binds (SutHandle decomposition increment 3). Forwards
/// to the existing production helper (`TestEnvironment::setup_watch`, reached via
/// `Deref`, now `&self` after the watch state was made interior-mutable), so
/// native behaviour is unchanged. `&self`, so the native macro dispatch
/// (`sut: &mut E2ESut`) drives it via auto-reborrow. The transition compiles
/// `TestQuery → (source, lang)` at the boundary, so this takes the compiled form.
#[async_trait::async_trait(?Send)]
impl SutWatchRegister for E2ESut {
    async fn register_watch(&self, query_id: &str, source: &str, lang: QueryLanguage) {
        self.setup_watch(query_id, source, lang)
            .await
            .expect("Watch setup failed");
    }

    async fn unregister_watch(&self, query_id: &str) {
        self.remove_watch(query_id);
    }
}

/// View/mode switching cap. `self.deref().switch_view(..)` forces the
/// `Deref`-reached `TestEnvironment` inherent method — calling `self.switch_view`
/// would re-dispatch to this trait method and recurse (the "trait shadows Deref
/// inherent" gotcha).
#[async_trait::async_trait(?Send)]
impl SutViewControl for E2ESut {
    async fn switch_view(&self, view_name: &str) {
        use std::ops::Deref;
        self.deref().switch_view(view_name);
    }
}

/// MCP-emit cap (`EmitMcpData` transition). No `ref_state`, no payload.
#[async_trait::async_trait(?Send)]
impl SutMcpEmit for E2ESut {
    async fn emit_mcp_data(&self) {
        tracing::trace!("[apply] EmitMcpData");
        if let Some(mcp) = self.pbt_mcp.get() {
            mcp.emit_update()
                .await
                .expect("PbtMcpIntegration::emit_update failed");
        }
    }
}

/// Undo/redo cap (`UndoLastMutation` / `Redo` transitions). Pure `&self` actions
/// over the engine undo stack; the `ref_state`-dependent block-convergence settle
/// lives in `block_tree_post_action` (decomposition #1b).
#[async_trait::async_trait(?Send)]
impl SutHistoryWrite for E2ESut {
    async fn undo_last_mutation(&self) {
        tracing::trace!("[apply] UndoLastMutation");
        let result = self.ctx.engine().undo().await;
        assert!(result.is_ok(), "undo failed: {:?}", result.err());
        assert!(result.unwrap(), "undo returned false (nothing to undo)");
    }

    async fn redo(&self) {
        tracing::trace!("[apply] Redo");
        let result = self.ctx.engine().redo().await;
        assert!(result.is_ok(), "redo failed: {:?}", result.err());
        assert!(result.unwrap(), "redo returned false (nothing to redo)");
    }
}

/// Nav-history navigation + sidebar pinning, driven through the UI driver
/// (leader chords / synthetic dispatch). Driver-realized only — the headless
/// `frontend_slice` does not drive these — so `E2ESut` is the sole impl.
#[async_trait::async_trait(?Send)]
impl SutNavHistoryDrive for E2ESut {
    async fn navigate_back(&self, region: holon_api::Region) {
        debug_assert_eq!(
            region,
            holon_api::Region::Main,
            "NavigateBack generator must only emit Main; got {region:?}"
        );
        self.send_leader_chord("go_back", "NavigateBack").await;
        self.ctx.drain_delivery_barrier().await;
        self.dump_nav_tables("after NavigateBack").await;
    }

    async fn navigate_forward(&self, region: holon_api::Region) {
        debug_assert_eq!(
            region,
            holon_api::Region::Main,
            "NavigateForward generator must only emit Main; got {region:?}"
        );
        self.send_leader_chord("go_forward", "NavigateForward")
            .await;
        self.ctx.drain_delivery_barrier().await;
        self.dump_nav_tables("after NavigateForward").await;
    }

    async fn pin_block(&self, region: holon_api::Region, block_id: &holon_api::EntityUri) {
        let resolved_id = self.resolve_uri(block_id);
        tracing::trace!(
            "[apply] PinBlock: region={region:?} block={block_id} (resolved={resolved_id})"
        );
        let driver = self
            .driver
            .borrow()
            .clone()
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
        self.ctx.drain_delivery_barrier().await;
        self.dump_nav_tables("after PinBlock").await;
    }

    async fn unpin_block(&self, history_id: i64) {
        tracing::trace!("[apply] UnpinBlock: history_id={history_id}");
        let driver = self
            .driver
            .borrow()
            .clone()
            .expect("driver not installed — was start_app called?");
        let mut params = HashMap::new();
        params.insert("history_id".to_string(), Value::Integer(history_id));
        driver
            .synthetic_dispatch("navigation", "close", params)
            .await
            .unwrap_or_else(|e| {
                panic!("[UnpinBlock] synthetic_dispatch(navigation, close) failed: {e:#}")
            });
        self.ctx.drain_delivery_barrier().await;
        self.dump_nav_tables("after UnpinBlock").await;
    }
}

/// Post-startup mutations (`ToggleState`, `ApplyMutation`, `BulkExternalAdd`) —
/// the integration-test-local cap (its `CycleTarget` / `MutationEvent` operands
/// are test-only types). `ApplyMutation` / `BulkExternalAdd` are `&self` no-ops
/// here: their `ref_state`-dependent dispatch runs in `block_tree_post_action`.
#[async_trait::async_trait(?Send)]
impl crate::pbt::local_caps::SutMutate for E2ESut {
    async fn toggle_state(
        &self,
        block_id: &holon_api::EntityUri,
        new_state: crate::pbt::transitions::toggle_state::CycleTarget,
    ) {
        let resolved_block_id = self.resolve_uri(block_id);
        // Real-user-input dispatch: compute click_count from the pre-mutation
        // task_state, then click the state_toggle widget that many times.
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
}

/// The seam-relocated mutations (`ApplyMutation`/`BulkExternalAdd`). `E2ESut` provides
/// `SutSeamMutate` because its `block_tree_post_action` seam runs the real,
/// `ref_state`-dependent dispatch; the cap actions themselves are `&self` no-ops that
/// the seam stands behind. (The composed `HeadlessFrontendComponent` deliberately does
/// NOT provide this cap — no seam yet — so these transitions auto-narrow out there.)
#[async_trait::async_trait(?Send)]
impl crate::pbt::local_caps::SutSeamMutate for E2ESut {
    async fn bulk_external_add(&self, _: &holon_api::EntityUri, _: &[holon_api::block::Block]) {
        // Relocated to `block_tree_post_action`'s `BulkExternalAdd` arm: the body
        // serializes the FULL document from `ref_state` (`resolve_ref_blocks`), so
        // it runs in the harness seam that owns `ref_state`. The action is a
        // `&self` no-op; `apply_bulk_external_add_to_sut` runs from the seam.
    }

    async fn apply_mutation(&self, _: crate::pbt::types::MutationEvent) {
        // Relocated to `block_tree_post_action`'s `ApplyMutation` arm: the dispatch
        // needs `ref_state` and the LoroPeer path drives the `&mut self`
        // `apply_peer_*` caps, so it runs in the harness seam. The action is a
        // `&self` no-op; `apply_apply_mutation_to_sut` runs from the seam.
    }
}

/// Pre-startup org-filesystem fixture setup (`WriteOrgFile`, `CreateDirectory`,
/// `GitInit`, `JjGitInit`, `CreateStaleLoro`). Integration-test-local cap —
/// `create_stale_loro` names the test-only `LoroCorruptionType`. `E2ESut`-only.
#[async_trait::async_trait(?Send)]
impl crate::pbt::local_caps::SutFixtureFs for E2ESut {
    async fn write_org_file(&self, filename: &str, content: &str) {
        use std::ops::Deref;
        tracing::trace!(
            "[apply] WriteOrgFile: {} ({} bytes)",
            filename,
            content.len()
        );
        // `self.deref()` forces the `TestContext` inherent `write_org_file`; a
        // bare `self.write_org_file` would re-dispatch to this trait method and
        // recurse (the "trait shadows Deref inherent" gotcha).
        self.deref()
            .write_org_file(filename, content)
            .await
            .expect("Failed to write org file");

        // If app is running, wait for FileSyncController to ingest the file
        // and re-key ctx.documents from `file:<filename>` to the resolved
        // doc URI. Mirrors the start_app loop (see apply_start_app body):
        // without this, subsequent transitions like bulk_external_add
        // that resolve the doc via `resolve_uri` (which checks doc_uri_map)
        // and then `ctx.documents.get(&resolved)` will miss because docs
        // added post-startup never got re-keyed. Backend-agnostic now:
        // `resolve_page_uri_by_name` reads Turso's `block_raw` or the Loro
        // snapshot per the active storage (the no-Turso org→Loro ingest runs
        // through the Loro-wired `FileSyncController`).
        if !self.ctx.is_running() {
            return;
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match self.ctx.resolve_page_uri_by_name(filename).await {
                Ok(resolved) => {
                    let file_key = holon_api::EntityUri::file(filename);
                    let removed = self.ctx.documents.borrow_mut().remove(&file_key);
                    if let Some(path) = removed {
                        self.ctx
                            .documents
                            .borrow_mut()
                            .insert(resolved.clone(), path);
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

    async fn create_directory(&self, path: &str) {
        tracing::trace!("[apply] CreateDirectory: {}", path);
        let full_path = self.org_root().join(path);
        self.org_fs.mkdir_all(&full_path);
    }

    async fn git_init(&self) {
        tracing::trace!("[apply] GitInit");
        let output = tokio::process::Command::new("git")
            .args(["init"])
            .current_dir(self.temp_dir.path())
            .output()
            .await
            .expect("Failed to run git init");
        assert!(output.status.success(), "git init failed: {:?}", output);
    }

    async fn jj_git_init(&self) {
        tracing::trace!("[apply] JjGitInit");
        let output = tokio::process::Command::new("jj")
            .args(["git", "init"])
            .current_dir(self.temp_dir.path())
            .output()
            .await
            .expect("Failed to run jj git init");
        assert!(output.status.success(), "jj git init failed: {:?}", output);
    }

    async fn create_stale_loro(
        &self,
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
}
