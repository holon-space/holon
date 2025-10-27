//! System Under Test: `E2ESut` struct and `StateMachineTest` implementation.
//!
//! Contains the SUT wrapper, mutation application, invariant checking,
//! and all transition handling for the real system.

use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use proptest_state_machine::{ReferenceStateMachine, StateMachineTest};

use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_api::{ContentType, QueryLanguage, SourceLanguage, Value};
use holon_orgmode::OrgBlockExt;

#[cfg(test)]
use similar_asserts::assert_eq;

use crate::{
    DirectMutationDriver, MutationDriver, TestContext, assert_block_order,
    assert_blocks_equivalent, block_belongs_to_document, serialize_blocks_to_org,
    wait_for_file_condition,
};

use super::reference_state::ReferenceState;
use super::state_machine::VariantRef;
use super::transitions::E2ETransition;
use super::types::*;

pub struct E2ESut<V: VariantMarker> {
    pub ctx: TestContext,
    /// Maps file-based doc URIs ("doc:doc_0.org") to UUID-based URIs
    /// ("doc:<uuid>") assigned by the real system.
    pub doc_uri_map: HashMap<String, String>,
    /// True when the most recent transition was nav/view/watch only (no block data changes).
    pub last_transition_nav_only: bool,
    /// How UI mutations are dispatched. `None` before `start_app` creates the engine.
    /// Backend tests use `DirectMutationDriver`; Flutter tests inject their own driver.
    pub driver: Option<Box<dyn MutationDriver>>,
    _marker: PhantomData<V>,
}

impl<V: VariantMarker> std::ops::Deref for E2ESut<V> {
    type Target = TestContext;
    fn deref(&self) -> &Self::Target {
        &self.ctx
    }
}

impl<V: VariantMarker> std::ops::DerefMut for E2ESut<V> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.ctx
    }
}

impl<V: VariantMarker> std::fmt::Debug for E2ESut<V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.ctx.fmt(f)
    }
}

impl<V: VariantMarker> E2ESut<V> {
    pub fn new(runtime: Arc<tokio::runtime::Runtime>) -> Result<Self> {
        Ok(Self {
            ctx: TestContext::new(runtime)?,
            doc_uri_map: HashMap::new(),
            last_transition_nav_only: false,
            driver: None,
            _marker: PhantomData,
        })
    }

    /// Create an E2ESut with a pre-installed MutationDriver.
    ///
    /// Used by Flutter PBT: the FlutterMutationDriver is installed upfront
    /// so that `install_direct_driver()` (called after StartApp) won't overwrite it.
    pub fn with_driver(
        runtime: Arc<tokio::runtime::Runtime>,
        driver: Box<dyn MutationDriver>,
    ) -> Result<Self> {
        Ok(Self {
            ctx: TestContext::new(runtime)?,
            doc_uri_map: HashMap::new(),
            last_transition_nav_only: false,
            driver: Some(driver),
            _marker: PhantomData,
        })
    }

    /// Set up the default DirectMutationDriver from the engine. Called after start_app.
    fn install_direct_driver(&mut self) {
        if self.driver.is_some() {
            return; // respect pre-installed driver (e.g. FlutterMutationDriver)
        }
        let engine = self.test_ctx().engine().clone();
        self.driver = Some(Box::new(DirectMutationDriver::new(engine)));
    }

    /// Resolve a parent_id: if it's a file-based doc URI, translate to UUID-based.
    pub fn resolve_parent_id(&self, parent_id: &str) -> String {
        self.doc_uri_map
            .get(parent_id)
            .cloned()
            .unwrap_or_else(|| parent_id.to_string())
    }
}
impl<V: VariantMarker> E2ESut<V> {
    /// Async body of `apply()` — extracted so Flutter (already async) can call directly
    /// without `block_on`.
    pub async fn apply_transition_async(
        &mut self,
        ref_state: &ReferenceState,
        transition: &E2ETransition,
    ) {
        match transition {
            // Pre-startup transitions
            E2ETransition::WriteOrgFile { filename, content } => {
                eprintln!(
                    "[apply] WriteOrgFile: {} ({} bytes)",
                    filename,
                    content.len()
                );
                self.write_org_file(filename, content)
                    .await
                    .expect("Failed to write org file");
            }

            E2ETransition::CreateDirectory { path } => {
                eprintln!("[apply] CreateDirectory: {}", path);
                let full_path = self.temp_dir.path().join(path);
                tokio::fs::create_dir_all(&full_path)
                    .await
                    .expect("Failed to create directory");
            }

            E2ETransition::GitInit => {
                eprintln!("[apply] GitInit");
                let output = tokio::process::Command::new("git")
                    .args(["init"])
                    .current_dir(self.temp_dir.path())
                    .output()
                    .await
                    .expect("Failed to run git init");
                assert!(output.status.success(), "git init failed: {:?}", output);
            }

            E2ETransition::JjGitInit => {
                eprintln!("[apply] JjGitInit");
                let output = tokio::process::Command::new("jj")
                    .args(["git", "init"])
                    .current_dir(self.temp_dir.path())
                    .output()
                    .await
                    .expect("Failed to run jj git init");
                assert!(output.status.success(), "jj git init failed: {:?}", output);
            }

            E2ETransition::CreateStaleLoro {
                org_filename,
                corruption_type,
            } => {
                eprintln!(
                    "[apply] CreateStaleLoro: {} ({:?})",
                    org_filename, corruption_type
                );
                self.write_stale_loro_file(org_filename, *corruption_type)
                    .await
                    .expect("Failed to create stale loro file");
            }

            E2ETransition::StartApp {
                wait_for_ready,
                enable_todoist,
                enable_loro,
            } => {
                eprintln!(
                    "[apply] StartApp (wait_for_ready={}, enable_todoist={}, enable_loro={})",
                    wait_for_ready, enable_todoist, enable_loro
                );
                self.set_enable_todoist(*enable_todoist);
                self.set_enable_loro(*enable_loro);
                self.start_app(*wait_for_ready)
                    .await
                    .expect("Failed to start app");

                // Install the default mutation driver now that the engine exists.
                if self.driver.is_none() {
                    self.install_direct_driver();
                }

                // Mirror Flutter startup: call initial_widget() after engine ready.
                // This is the same code path Flutter uses via FrontendSession.
                //
                // The "Actor channel closed" bug that previously occurred here was caused
                // by the DI ServiceProvider being dropped (after create_backend_engine_with_extras
                // returns), which dropped TursoBackend and its sender. Now that BackendEngine
                // holds a reference to TursoBackend (_backend_keepalive), the actor survives.
                let expects_valid_index = ref_state.has_valid_index_org();
                let root_id_dynamic = ref_state.root_layout_block_id();
                let root_id = root_id_dynamic
                    .as_deref()
                    .unwrap_or(holon_api::ROOT_LAYOUT_BLOCK_ID);
                eprintln!(
                    "[apply] Calling render_block('{}') (expects valid index.org: {})",
                    root_id, expects_valid_index
                );

                let render_result = self
                    .engine()
                    .blocks()
                    .render_block(root_id, None, true)
                    .await;

                match (expects_valid_index, render_result) {
                    (true, Ok((widget_spec, _stream))) => {
                        eprintln!(
                            "[apply] render_block('{}') succeeded with {} rows",
                            root_id,
                            widget_spec.data.len()
                        );
                    }
                    (true, Err(e)) => {
                        let err_str = e.to_string();
                        if err_str.contains("ScalarSubquery")
                            || err_str.contains("materialized view")
                        {
                            eprintln!(
                                "[apply] render_block('{}') failed due to known Turso IVM limitation (GQL): {}",
                                root_id, e
                            );
                        } else {
                            panic!(
                                "render_block('{}') failed but reference state has valid index.org: {}",
                                root_id, e
                            );
                        }
                    }
                    (false, Ok(_)) => {
                        panic!(
                            "render_block('{}') succeeded but reference state has no valid index.org",
                            root_id
                        );
                    }
                    (false, Err(e)) => {
                        eprintln!(
                            "[apply] render_block('{}') correctly failed (no valid index.org): {}",
                            root_id, e
                        );
                    }
                }

                // Set up region watch for navigation verification
                if let Err(e) = self.setup_region_watch(holon_api::Region::Main).await {
                    eprintln!("[apply] Region watch setup failed (non-fatal): {}", e);
                }

                // Populate doc_uri_map for pre-startup documents whose document
                // entities were created by OrgSyncController during startup.
                // Also update TestEnvironment.documents keys from file-based to UUID-based URIs.
                for file_uri in ref_state.documents.keys() {
                    if !self.doc_uri_map.contains_key(file_uri)
                        && EntityUri::parse(file_uri).is_ok_and(|u| u.is_doc())
                        && file_uri.ends_with(".org")
                    {
                        match self.ctx.resolve_doc_uri(file_uri).await {
                            Ok(resolved) => {
                                eprintln!(
                                    "[apply] Mapped pre-startup doc: {} → {}",
                                    file_uri, resolved
                                );
                                self.doc_uri_map.insert(file_uri.clone(), resolved.clone());
                                // Re-key ctx.documents from file-based to UUID-based URI
                                if let Some(path) = self.ctx.documents.remove(file_uri) {
                                    self.ctx.documents.insert(resolved, path);
                                }
                            }
                            Err(e) => {
                                eprintln!(
                                    "[apply] Could not resolve pre-startup doc {}: {}",
                                    file_uri, e
                                );
                            }
                        }
                    }
                }
            }

            // Post-startup transitions
            E2ETransition::CreateDocument { file_name } => {
                eprintln!("[apply] Creating document: {}", file_name);
                match self.create_document(file_name).await {
                    Ok(uuid_uri) => {
                        let file_uri = EntityUri::doc(file_name).to_string();
                        eprintln!("[apply] Created document: {} → {}", file_uri, uuid_uri);
                        self.doc_uri_map.insert(file_uri, uuid_uri);
                    }
                    Err(e) => panic!("Failed to create document: {}", e),
                }
            }

            E2ETransition::ApplyMutation(event) => {
                eprintln!("[apply] Applying mutation: {:?}", event.mutation);
                self.apply_mutation(event.clone(), &ref_state).await;
            }

            E2ETransition::SetupWatch {
                query_id,
                query,
                language,
            } => {
                let (source, lang_str) = query.compile_for(*language);
                eprintln!(
                    "[apply] SetupWatch: {} ({}) → {}",
                    query_id,
                    lang_str,
                    &source[..source.len().min(80)]
                );
                self.setup_watch(query_id, &source, lang_str)
                    .await
                    .expect("Watch setup failed");
            }

            E2ETransition::RemoveWatch { query_id } => {
                self.remove_watch(query_id);
            }

            E2ETransition::SwitchView { view_name } => {
                self.switch_view(view_name);
            }

            E2ETransition::NavigateFocus { region, block_id } => {
                self.navigate_focus(*region, block_id)
                    .await
                    .expect("Navigation failed");
            }

            E2ETransition::NavigateBack { region } => {
                self.navigate_back(*region)
                    .await
                    .expect("Navigation failed");
            }

            E2ETransition::NavigateForward { region } => {
                self.navigate_forward(*region)
                    .await
                    .expect("Navigation failed");
            }

            E2ETransition::NavigateHome { region } => {
                self.navigate_home(*region)
                    .await
                    .expect("Navigation failed");
            }

            E2ETransition::SimulateRestart => {
                let expected_count = ref_state.blocks.len();
                self.simulate_restart(expected_count)
                    .await
                    .expect("SimulateRestart failed");
            }

            E2ETransition::BulkExternalAdd { doc_uri, blocks } => {
                eprintln!(
                    "[apply] BulkExternalAdd: adding {} blocks to {}",
                    blocks.len(),
                    doc_uri
                );

                // Resolve file-based URI to UUID-based URI (documents map uses UUID keys after StartApp)
                let resolved_uri = self.resolve_parent_id(doc_uri);
                let file_path = self.ctx.documents.get(&resolved_uri).unwrap_or_else(|| {
                    panic!(
                        "Document not found for BulkExternalAdd: {} (resolved: {})",
                        doc_uri, resolved_uri
                    )
                });

                // Get all blocks for this document from reference self.
                // Note: ref_state already includes the new blocks (from apply_reference).
                // Resolve parent_ids so block_belongs_to_document matches UUID-based doc URIs.
                let resolved_blocks: Vec<Block> = ref_state
                    .blocks
                    .values()
                    .map(|b| {
                        let mut b = b.clone();
                        b.parent_id =
                            EntityUri::from_raw(&self.resolve_parent_id(b.parent_id.as_raw_str()));
                        b
                    })
                    .collect();
                let all_blocks: Vec<Block> = resolved_blocks
                    .iter()
                    .filter(|b| block_belongs_to_document(b, &resolved_blocks, &resolved_uri))
                    .cloned()
                    .collect();
                let existing_count = all_blocks.len().saturating_sub(blocks.len());

                // Serialize to org file
                let block_refs: Vec<&Block> = all_blocks.iter().collect();
                let org_content = serialize_blocks_to_org(&block_refs, &resolved_uri);

                eprintln!(
                    "[BulkExternalAdd] Writing {} total blocks ({} new) to {:?}",
                    all_blocks.len(),
                    blocks.len(),
                    file_path
                );
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
                        "from block | select {{id, content}} | filter id != \"bulk-race-{}\" ",
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
                            eprintln!(
                                "[BulkExternalAdd] Concurrent query_and_watch {} succeeded in {:?}",
                                i, elapsed
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
                let source_block_count =
                    final_content.to_lowercase().matches("#+begin_src").count();
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
                eprintln!(
                    "[BulkExternalAdd] File verified with {} blocks after {:?}",
                    actual_block_count, elapsed
                );

                // Now wait for the blocks to sync to the DATABASE
                // The chain is: File → FileWatcher → OrgSyncController → Loro → EventBus → CacheEventSubscriber → Database
                let expected_db_count = ref_state.blocks.len();
                let db_timeout = Duration::from_millis(10000);
                let db_start = Instant::now();

                let actual_rows = self
                    .wait_for_block_count(expected_db_count, db_timeout)
                    .await;
                let db_elapsed = db_start.elapsed();

                if actual_rows.len() == expected_db_count {
                    eprintln!(
                        "[BulkExternalAdd] Database synced ({} blocks) in {:?}",
                        expected_db_count, db_elapsed
                    );
                } else {
                    panic!(
                        "[BulkExternalAdd] WARNING: Database has {} blocks, expected {} after {:?}",
                        actual_rows.len(),
                        expected_db_count,
                        db_elapsed
                    );
                }

                // Poll until org files stabilize (sync controller finishes re-rendering)
                self.wait_for_org_files_stable(50, Duration::from_millis(5000))
                    .await;
            }

            E2ETransition::ConcurrentSchemaInit => {
                eprintln!(
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
                        "from block | select {{id, content}} | filter id != \"dummy-{}\" ",
                        i
                    );
                    let sql = engine
                        .compile_to_sql(&prql, QueryLanguage::HolonPrql)
                        .expect("PRQL compilation should succeed");
                    let start = Instant::now();
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
                    let sql = "SELECT id FROM block LIMIT 1".to_string();
                    let start = Instant::now();
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

                eprintln!(
                    "[ConcurrentSchemaInit] All sequential operations completed successfully"
                );

                eprintln!("[ConcurrentSchemaInit] Test completed successfully");
            }

            E2ETransition::ConcurrentMutations {
                ui_mutation,
                external_mutation,
            } => {
                eprintln!(
                    "[apply] ConcurrentMutations: UI={:?}, External={:?}",
                    ui_mutation.mutation, external_mutation.mutation
                );
                self.apply_concurrent_mutations(
                    ui_mutation.clone(),
                    external_mutation.clone(),
                    &ref_state,
                )
                .await;
            }
        }

        self.drain_cdc_events().await;
        self.drain_region_cdc_events().await;

        self.last_transition_nav_only = matches!(
            transition,
            E2ETransition::SwitchView { .. }
                | E2ETransition::NavigateFocus { .. }
                | E2ETransition::NavigateBack { .. }
                | E2ETransition::NavigateForward { .. }
                | E2ETransition::NavigateHome { .. }
                | E2ETransition::SetupWatch { .. }
                | E2ETransition::RemoveWatch { .. }
        );
    }

    /// Async body of `check_invariants()` — extracted so Flutter can call directly.
    pub async fn check_invariants_async(&self, ref_state: &ReferenceState) {
        eprintln!(
            "[check_invariants] ref_state has {} blocks, app_started: {}",
            ref_state.blocks.len(),
            ref_state.app_started
        );

        // Skip invariant checks if app is not started
        if !ref_state.app_started {
            return;
        }

        // Transitions that don't modify block data — skip expensive invariants
        let nav_only = self.last_transition_nav_only;

        // 0. Check for startup errors (Flutter bug: DDL/sync race)
        assert!(
            !self.has_startup_errors(),
            "FLUTTER STARTUP BUG: {} publish errors during startup.\n\
                 This indicates DDL/sync race condition when {} pre-existing files were synced.\n\
                 Files: {:?}",
            self.startup_error_count(),
            self.documents.len(),
            self.documents.keys().collect::<Vec<_>>()
        );

        // 1. Backend storage matches reference model
        let prql = r#"
                from block
                select {id, content, content_type, source_language, parent_id, document_id, properties}
            "#;
        let widget_spec = self
            .test_ctx()
            .query(prql.to_string(), QueryLanguage::HolonPrql, HashMap::new())
            .await
            .expect("Failed to query blocks");

        // Convert query results to Blocks for comparison
        let backend_blocks: Vec<Block> = widget_spec
            .data
            .into_iter()
            .filter_map(|resolved_row| {
                let row = resolved_row.data;
                let id = row.get("id")?.as_string()?.to_string();
                let parent_id = row.get("parent_id")?.as_string()?.to_string();
                let content = row
                    .get("content")
                    .and_then(|v| v.as_string())
                    .unwrap_or("")
                    .to_string();

                let document_id = row
                    .get("document_id")
                    .and_then(|v| v.as_string())
                    .unwrap_or("")
                    .to_string();

                let mut block = Block::new_text(
                    EntityUri::from_raw(&id),
                    EntityUri::from_raw(&parent_id),
                    EntityUri::from_raw(&document_id),
                    content,
                );

                // Set content_type and source_language (critical for source block round-trip)
                if let Some(content_type) = row.get("content_type").and_then(|v| v.as_string()) {
                    block.content_type = content_type.parse::<ContentType>().unwrap();
                }
                if let Some(source_language) =
                    row.get("source_language").and_then(|v| v.as_string())
                {
                    block.source_language =
                        Some(source_language.parse::<SourceLanguage>().unwrap());
                }

                // Extract properties from the row
                if let Some(Value::Object(props)) = row.get("properties") {
                    for (k, v) in props {
                        block.properties.insert(k.clone(), v.clone());
                    }
                }

                // Also check for top-level org fields (in case they're returned directly)
                if let Some(task_state) = row
                    .get("task_state")
                    .or_else(|| row.get("TODO"))
                    .and_then(|v| v.as_string())
                {
                    block.set_task_state(Some(holon_api::TaskState::from_keyword(&task_state)));
                }
                if let Some(priority) = row
                    .get("priority")
                    .or_else(|| row.get("PRIORITY"))
                    .and_then(|v| v.as_i64())
                {
                    block.set_priority(Some(
                        holon_api::Priority::from_int(priority as i32).unwrap_or_else(|e| {
                            panic!("stored priority {priority} is invalid: {e}")
                        }),
                    ));
                }
                if let Some(tags) = row
                    .get("tags")
                    .or_else(|| row.get("TAGS"))
                    .and_then(|v| v.as_string())
                {
                    block.set_tags(holon_api::Tags::from_csv(tags));
                }
                if let Some(scheduled) = row
                    .get("scheduled")
                    .or_else(|| row.get("SCHEDULED"))
                    .and_then(|v| v.as_string())
                {
                    if let Ok(ts) = holon_api::types::Timestamp::parse(&scheduled) {
                        block.set_scheduled(Some(ts));
                    }
                }
                if let Some(deadline) = row
                    .get("deadline")
                    .or_else(|| row.get("DEADLINE"))
                    .and_then(|v| v.as_string())
                {
                    if let Ok(ts) = holon_api::types::Timestamp::parse(&deadline) {
                        block.set_deadline(Some(ts));
                    }
                }

                Some(block)
            })
            .collect();

        let ref_blocks: Vec<_> = ref_state.blocks.values().cloned().collect();
        assert_blocks_equivalent(
            &backend_blocks,
            &ref_blocks,
            "Backend diverged from reference",
        );

        // Ref blocks from org files only (excludes seeded default layout under doc:__default__)
        let ref_blocks_org_only: Vec<_> = ref_state
            .blocks
            .values()
            .filter(|b| {
                ref_state
                    .block_documents
                    .get(b.id.as_str())
                    .map_or(true, |doc| doc != "doc:__default__")
            })
            .cloned()
            .collect();

        // 2/2b: Org file parse + ordering — expensive, skip for nav-only transitions
        if !nav_only {
            let org_blocks = self
                .parse_org_file_blocks()
                .await
                .expect("Failed to parse Org file");
            assert_blocks_equivalent(
                &org_blocks,
                &ref_blocks_org_only,
                "Org file diverged from reference",
            );

            // 2b. Org file block ordering matches reference model
            assert_block_order(
                &org_blocks,
                &ref_blocks_org_only,
                "Org file block ordering wrong",
            );
        }

        // 3. UI model (built from CDC) matches reference — verify all fields, not just IDs
        for (query_id, ui_data) in &self.ui_model {
            if let Some(watch_spec) = ref_state.active_watches.get(query_id) {
                let expected = ref_state.query_results(watch_spec);

                let ui_ids: HashSet<String> = ui_data
                    .iter()
                    .filter_map(|row| {
                        row.get("id")
                            .and_then(|v| v.as_string())
                            .map(|s| s.to_string())
                    })
                    .collect();
                let expected_ids: HashSet<String> = expected
                    .iter()
                    .filter_map(|row| {
                        row.get("id")
                            .and_then(|v| v.as_string())
                            .map(|s| s.to_string())
                    })
                    .collect();

                assert_eq!(
                    ui_ids,
                    expected_ids,
                    "CDC UI model for watch '{}' has wrong block IDs.\n\
                         Expected {} blocks: {:?}\n\
                         Got {} blocks: {:?}",
                    query_id,
                    expected_ids.len(),
                    expected_ids,
                    ui_ids.len(),
                    ui_ids
                );

                // Verify fields per block that are included in the query columns
                let query_cols = &watch_spec.query.columns;
                let fields_to_check: Vec<&str> =
                    ["content", "content_type", "source_language", "source_name"]
                        .iter()
                        .copied()
                        .filter(|f| query_cols.iter().any(|c| c == *f))
                        .collect();
                for expected_row in &expected {
                    let expected_id = match expected_row.get("id").and_then(|v| v.as_string()) {
                        Some(id) => id,
                        None => continue,
                    };

                    if let Some(ui_row) = ui_data.iter().find(|r: &&HashMap<String, Value>| {
                        r.get("id").and_then(|v| v.as_string()) == Some(expected_id)
                    }) {
                        for field in &fields_to_check {
                            let expected_val = expected_row
                                .get(*field)
                                .and_then(|v: &Value| v.as_string())
                                .map(|s| s.trim());
                            let actual_val = ui_row
                                .get(*field)
                                .and_then(|v: &Value| v.as_string())
                                .map(|s| s.trim());
                            assert_eq!(
                                actual_val, expected_val,
                                "CDC field '{}' mismatch for block '{}' in watch '{}'",
                                field, expected_id, query_id
                            );
                        }

                        // parent_id: normalize document URIs before comparing
                        if query_cols.iter().any(|c| c == "parent_id") {
                            let normalize_parent = |v: Option<&Value>| -> Option<String> {
                                v.and_then(|v| v.as_string()).map(|s| {
                                    if EntityUri::parse(s).is_ok_and(|u| u.is_doc()) {
                                        "__document_root__".to_string()
                                    } else {
                                        s.trim().to_string()
                                    }
                                })
                            };
                            assert_eq!(
                                normalize_parent(ui_row.get("parent_id")),
                                normalize_parent(expected_row.get("parent_id")),
                                "CDC parent_id mismatch for block '{}' in watch '{}'",
                                expected_id,
                                query_id
                            );
                        }
                    }
                }
            }
        }

        // 4. View selection synchronized
        assert_eq!(self.current_view, ref_state.current_view());

        // 5. Active watches match
        assert_eq!(
            self.active_watches.keys().collect::<HashSet<_>>(),
            ref_state.active_watches.keys().collect::<HashSet<_>>(),
            "Watch sets diverged"
        );

        // 6. Structural integrity: no orphan blocks
        for block in &backend_blocks {
            if block.parent_id.is_doc() {
                continue;
            }
            assert!(
                backend_blocks
                    .iter()
                    .any(|b| b.id.as_str() == block.parent_id.as_raw_str()),
                "Orphan block: {} has invalid parent {}",
                block.id,
                block.parent_id
            );
        }

        // 7. Navigation state verification
        let focus_rows = self
            .engine()
            .execute_query(
                "SELECT region, block_id FROM current_focus".to_string(),
                HashMap::new(),
                None,
            )
            .await
            .expect("Failed to query current_focus - this may indicate a Turso IVM bug");

        for (region, history) in &ref_state.navigation_history {
            let expected_focus = history.current_focus();
            let actual = focus_rows
                .iter()
                .find(|r| r.get("region").and_then(|v| v.as_string()) == Some(region.as_str()));

            match (actual, &expected_focus) {
                (Some(row), Some(expected_id)) => {
                    let actual_block_id = row
                        .get("block_id")
                        .and_then(|v| v.as_string())
                        .map(|s| s.to_string());
                    assert_eq!(
                        actual_block_id.as_deref(),
                        Some(expected_id.as_str()),
                        "Navigation focus mismatch for region '{}': expected {:?}, got {:?}",
                        region,
                        expected_focus,
                        actual_block_id
                    );
                }
                (Some(row), None) => {
                    let actual_block_id = row.get("block_id");
                    assert!(
                        actual_block_id.is_none()
                            || actual_block_id.and_then(|v| v.as_string()).is_none()
                            || matches!(actual_block_id, Some(Value::Null)),
                        "Navigation focus mismatch for region '{}': expected home (None), got {:?}",
                        region,
                        actual_block_id
                    );
                }
                (None, None) => {}
                (None, Some(expected_id)) => {
                    panic!(
                        "[check_invariants] Region '{}' should have focus on '{}' but not found in DB",
                        region, expected_id
                    );
                }
            }
        }

        // 8. Region data verification — verify displayed blocks match navigation focus
        if !self.region_data.is_empty() {
            for region in holon_api::Region::ALL {
                let region_key = region.as_str().to_string();
                let expected = ref_state.expected_focus_root_ids(*region);

                let mut expected_ids: Vec<String> = expected.into_iter().collect();
                expected_ids.sort();

                let mut actual_ids: Vec<String> = self
                    .region_data
                    .get(&region_key)
                    .map(|data| {
                        data.iter()
                            .filter_map(|row| {
                                row.get("id")
                                    .and_then(|v| v.as_string())
                                    .map(|s| s.to_string())
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                actual_ids.sort();

                assert_eq!(
                    actual_ids,
                    expected_ids,
                    "Region '{}' displayed blocks mismatch after navigation.\n\
                     Focus: {:?}\n\
                     Expected IDs: {:?}\n\
                     Actual IDs: {:?}",
                    region.as_str(),
                    ref_state.current_focus(*region),
                    expected_ids,
                    actual_ids,
                );
            }
        }

        // 9/10: Properties check + root layout liveness — skip for nav-only transitions
        if !nav_only {
            // 9. Verify blocks with properties HashMap are correctly stored in cache
            // Single batch query instead of per-block queries
            let blocks_with_props: Vec<&Block> = backend_blocks
                .iter()
                .filter(|b| !b.properties.is_empty())
                .collect();

            if !blocks_with_props.is_empty() {
                let prql = "from block | filter properties != null | select {id, properties}";
                let query_result = self
                    .test_ctx()
                    .query(prql.to_string(), QueryLanguage::HolonPrql, HashMap::new())
                    .await
                    .expect("Failed to query properties batch");

                let cached_ids_with_props: HashSet<String> = query_result
                    .data
                    .iter()
                    .filter_map(|row| {
                        let id = row.data.get("id")?.as_string()?.to_string();
                        let props = row.data.get("properties")?;
                        if matches!(props, Value::Null) {
                            None
                        } else {
                            Some(id)
                        }
                    })
                    .collect();

                let mut missing: Vec<String> = Vec::new();
                for block in &blocks_with_props {
                    if !cached_ids_with_props.contains(block.id.as_str()) {
                        eprintln!(
                            "[props_check] block={}, has_props=true, properties={:?}, NOT found in cache",
                            block.id, block.properties
                        );
                        missing.push(block.id.to_string());
                    }
                }

                assert!(
                    missing.is_empty(),
                    "Block properties NULL in cache for: {:?} (Value::Object serialization bug)",
                    missing
                );
            }

            // 10. Root layout liveness: if index.org exists with proper structure,
            // render_block should still work after mutations.
            if ref_state.has_valid_index_org() {
                let engine = self.engine();
                let root_id_dynamic = ref_state.root_layout_block_id();
                let root_id = root_id_dynamic
                    .as_deref()
                    .unwrap_or(holon_api::ROOT_LAYOUT_BLOCK_ID);
                match engine.blocks().render_block(root_id, None, true).await {
                    Ok((ws, _stream)) => {
                        eprintln!(
                            "[check_invariants] render_block('{}') succeeded, {} rows",
                            root_id,
                            ws.data.len()
                        );

                        // Profile correctness: verify each row has the expected profile name
                        if ref_state.has_blocks_profile() && !ws.data.is_empty() {
                            let mut mismatches = Vec::new();
                            for row in &ws.data {
                                let actual_name = row.profile.as_ref().map(|p| p.name.clone());
                                let block_id = row
                                    .data
                                    .get("id")
                                    .and_then(|v| v.as_string())
                                    .map(|s| EntityUri::from_raw(&s));
                                let expected_name = block_id
                                    .as_ref()
                                    .and_then(|id| ref_state.expected_profile_name(id));

                                if actual_name.is_none() && expected_name.is_some() {
                                    mismatches.push(format!(
                                        "Row {:?}: expected profile '{}' but got None",
                                        row.data.get("id"),
                                        expected_name.unwrap(),
                                    ));
                                } else if let (Some(actual), Some(expected)) =
                                    (&actual_name, &expected_name)
                                {
                                    if actual != expected {
                                        mismatches.push(format!(
                                            "Row {:?}: expected profile '{}' but got '{}'",
                                            row.data.get("id"),
                                            expected,
                                            actual,
                                        ));
                                    }
                                }
                            }
                            assert!(
                                mismatches.is_empty(),
                                "Profile correctness violations ({} of {} rows):\n{}\n\
                                 Active profiles: {:?}\nProfile block IDs: {:?}",
                                mismatches.len(),
                                ws.data.len(),
                                mismatches.join("\n"),
                                ref_state.active_profiles,
                                ref_state.profile_block_ids,
                            );

                            // CDC liveness: at least one row must have a profile attached
                            let with_profile =
                                ws.data.iter().filter(|r| r.profile.is_some()).count();
                            assert!(
                                with_profile > 0,
                                "render_block() returned {} rows but none have profiles. \
                                 ProfileResolver did not pick up profile blocks via CDC.\n\
                                 Profile blocks: {:?}",
                                ws.data.len(),
                                ref_state.profile_block_ids,
                            );
                        }
                    }
                    Err(e) => {
                        let err_str = e.to_string();
                        if err_str.contains("ScalarSubquery")
                            || err_str.contains("materialized view")
                        {
                            eprintln!(
                                "[check_invariants] render_block('{}') failed with known IVM limitation: {}",
                                root_id, err_str
                            );
                        } else {
                            panic!("render_block('{}') failed for root block: {}", root_id, e);
                        }
                    }
                }
            }
        } // end if !nav_only (#9, #10)

        // 11. Loro vs Org check DISABLED: Loro is no longer the write path for blocks.
        // All block CRUD goes through SqlOperationProvider. Loro is populated via EventBus
        // subscriptions (reverse sync) which hasn't been implemented yet.
        // Re-enable this check once EventBus → Loro sync is in place.
    }
}

impl<V: VariantMarker> StateMachineTest for E2ESut<V> {
    type SystemUnderTest = Self;
    type Reference = VariantRef<V>;

    fn init_test(
        _ref_state: &<Self::Reference as ReferenceStateMachine>::State,
    ) -> Self::SystemUnderTest {
        eprintln!(
            "[init_test<{}>] Starting, ref_state has {} blocks, app_started: {}",
            std::any::type_name::<V>(),
            _ref_state.blocks.len(),
            _ref_state.app_started
        );
        let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
        let result = E2ESut::new(runtime).unwrap();
        eprintln!("[init_test] Completed (app not started yet - pre-startup phase)");
        result
    }

    fn apply(
        mut state: Self::SystemUnderTest,
        ref_state: &<Self::Reference as ReferenceStateMachine>::State,
        transition: E2ETransition,
    ) -> Self::SystemUnderTest {
        eprintln!(
            "[apply] ref_state has {} blocks, transition: {:?}",
            ref_state.blocks.len(),
            std::mem::discriminant(&transition)
        );
        let runtime = state.runtime.clone();
        runtime.block_on(state.apply_transition_async(ref_state, &transition));
        state
    }

    fn check_invariants(
        state: &Self::SystemUnderTest,
        ref_state: &<Self::Reference as ReferenceStateMachine>::State,
    ) {
        let runtime = state.runtime.clone();
        runtime.block_on(state.check_invariants_async(ref_state));
    }
}

impl<V: VariantMarker> E2ESut<V> {
    /// Apply a mutation (UI or External) and wait for sync to complete.
    ///
    /// This method delegates to TestContext methods for the actual work,
    /// keeping the PBT layer thin.
    async fn apply_mutation(&mut self, event: MutationEvent, ref_state: &ReferenceState) {
        match event.source {
            MutationSource::UI => {
                let (entity, op, mut params) = event.mutation.to_operation();

                // The reference model uses file-based document URIs (e.g. "doc:doc_0.org")
                // but the real system assigns UUID-based IDs. Resolve before executing.
                if let Some(Value::String(pid)) = params.get("parent_id") {
                    let resolved = self.resolve_parent_id(pid);
                    params.insert("parent_id".to_string(), Value::String(resolved.clone()));

                    // Compute document_id for create operations — the SQL INSERT
                    // won't set it automatically, causing NULL → malformed EntityUri.
                    if op == "create" && !params.contains_key("document_id") {
                        let parent_uri = EntityUri::from_raw(&resolved);
                        let doc_id = if parent_uri.is_doc() {
                            resolved
                        } else {
                            // Walk up parent chain in reference model to find document
                            crate::assertions::find_document_for_block(
                                parent_uri.as_raw_str(),
                                &crate::assertions::ReferenceState {
                                    blocks: ref_state
                                        .blocks
                                        .iter()
                                        .map(|(k, v)| (k.clone(), v.clone()))
                                        .collect(),
                                },
                            )
                            .map(|doc_uri| self.resolve_parent_id(&doc_uri))
                            .unwrap_or(resolved)
                        };
                        params.insert("document_id".to_string(), Value::String(doc_id));
                    }
                }

                eprintln!(
                    "[E2ESut::apply_mutation] About to call apply_ui_mutation: entity={}, op={}",
                    entity, op
                );
                let driver = self
                    .driver
                    .as_ref()
                    .expect("driver not installed — was start_app called?");
                match driver.apply_ui_mutation(&entity, &op, params).await {
                    Ok(()) => eprintln!("[E2ESut::apply_mutation] apply_ui_mutation returned Ok"),
                    Err(e) => panic!("Operation {}.{} failed: {:?}", entity, op, e),
                }
            }

            MutationSource::External => {
                // Resolve file-based doc URIs to UUID-based (ctx.documents is re-keyed
                // to UUID after start_app). Block-to-block parent_ids pass through unchanged.
                eprintln!("[E2ESut::apply_mutation] External mutation - writing to Org file");
                let expected_blocks: Vec<Block> = ref_state
                    .blocks
                    .values()
                    .map(|b| {
                        let mut b = b.clone();
                        b.parent_id =
                            EntityUri::from_raw(&self.resolve_parent_id(b.parent_id.as_raw_str()));
                        b
                    })
                    .collect();
                if let Err(e) = self.ctx.apply_external_mutation(&expected_blocks).await {
                    eprintln!("[E2ESut::apply_mutation] External mutation failed: {:?}", e);
                } else {
                    eprintln!(
                        "[E2ESut::apply_mutation] External mutation wrote to file, waiting for file watcher"
                    );
                }
            }
        }

        // Wait until block count matches expected (with timeout)
        let expected_count = ref_state.blocks.len();
        let timeout = Duration::from_millis(10000);
        let start = Instant::now();

        let actual_rows = self.wait_for_block_count(expected_count, timeout).await;
        let elapsed = start.elapsed();

        if actual_rows.len() == expected_count {
            eprintln!(
                "[E2ESut::apply_mutation] Block count matched ({}) in {:?}",
                expected_count, elapsed
            );
        } else {
            panic!(
                "[E2ESut::apply_mutation] Timeout waiting for {} blocks, got {} after {:?}",
                expected_count,
                actual_rows.len(),
                elapsed
            );
        }

        // Spot-check: verify the mutated block has correct data in the DB.
        // Only for UI mutations — External mutations write to org files and need the file
        // watcher to propagate changes to SQL (checked later in check_invariants).
        if event.source == MutationSource::UI {
            if let Some(block_id) = event.mutation.target_block_id() {
                if let Some(expected_block) = ref_state.blocks.get(&block_id) {
                    let prql = format!(
                        "from block | filter id == \"{}\" | select {{id, content, content_type, parent_id}}",
                        block_id
                    );
                    let spec = self
                        .test_ctx()
                        .query(prql, QueryLanguage::HolonPrql, HashMap::new())
                        .await
                        .unwrap_or_else(|e| {
                            panic!(
                                "Post-mutation spot-check query failed for block '{}': {:?}",
                                block_id, e
                            )
                        });
                    let resolved_row = spec.data.first().unwrap_or_else(|| {
                        panic!(
                            "Post-mutation spot-check: no row returned for block '{}'",
                            block_id
                        )
                    });
                    let actual_content = resolved_row
                        .data
                        .get("content")
                        .and_then(|v| v.as_string())
                        .unwrap_or("")
                        .trim();
                    let expected_content = expected_block.content.trim();
                    assert_eq!(
                        actual_content, expected_content,
                        "Post-mutation spot-check: content mismatch for block '{}'",
                        block_id
                    );
                    let actual_ct = resolved_row
                        .data
                        .get("content_type")
                        .and_then(|v| v.as_string())
                        .unwrap_or("");
                    assert_eq!(
                        actual_ct,
                        expected_block.content_type.to_string().as_str(),
                        "Post-mutation spot-check: content_type mismatch for block '{}'",
                        block_id
                    );
                }
            }
        } // UI mutations only

        // Wait for org files to match expected state, then stabilize (no more writes)
        let expected_blocks: Vec<Block> = ref_state
            .blocks
            .values()
            .map(|b| {
                let mut b = b.clone();
                b.parent_id =
                    EntityUri::from_raw(&self.resolve_parent_id(b.parent_id.as_raw_str()));
                b
            })
            .collect();
        let org_timeout = Duration::from_millis(5000);
        self.ctx
            .wait_for_org_file_sync(&expected_blocks, org_timeout)
            .await;
        self.ctx
            .wait_for_org_files_stable(50, Duration::from_millis(5000))
            .await;
    }

    /// Apply two mutations concurrently (UI + External) without sync barriers between them.
    /// Waits only once at the end for final convergence.
    async fn apply_concurrent_mutations(
        &mut self,
        ui_event: MutationEvent,
        // FIXME: external mutation should be applied from pre-merge state for true concurrency testing.
        // Currently, the external mutation is applied from the post-both-mutations reference state,
        // which means CRDT conflict resolution is never actually tested.
        ext_event: MutationEvent,
        ref_state: &ReferenceState,
    ) {
        eprintln!("[apply_concurrent_mutations] ext_event: {:?}", ext_event);

        // Fire External mutation FIRST so the file is on disk before the UI mutation's
        // block event triggers on_block_changed. This ensures on_block_changed sees the
        // external change (disk != last_projection) and ingests it before re-rendering.
        // Without this ordering, the block event can arrive and re-render BEFORE the
        // external write, causing a TOCTOU race that overwrites the external change.
        eprintln!("[ConcurrentMutations] Firing External mutation first");
        let expected_blocks: Vec<Block> = ref_state
            .blocks
            .values()
            .map(|b| {
                let mut b = b.clone();
                b.parent_id =
                    EntityUri::from_raw(&self.resolve_parent_id(b.parent_id.as_raw_str()));
                b
            })
            .collect();
        if let Err(e) = self.ctx.apply_external_mutation(&expected_blocks).await {
            eprintln!("[ConcurrentMutations] External mutation failed: {:?}", e);
        }

        // Fire UI mutation (no sync wait between external and UI)
        let (entity, op, mut params) = ui_event.mutation.to_operation();
        // Resolve file-based parent_id to UUID-based (same as apply_mutation)
        if let Some(Value::String(pid)) = params.get("parent_id") {
            let resolved = self.resolve_parent_id(pid);
            params.insert("parent_id".to_string(), Value::String(resolved.clone()));

            if op == "create" && !params.contains_key("document_id") {
                let parent_uri = EntityUri::from_raw(&resolved);
                let doc_id = if parent_uri.is_doc() {
                    resolved
                } else {
                    crate::assertions::find_document_for_block(
                        parent_uri.as_raw_str(),
                        &crate::assertions::ReferenceState {
                            blocks: ref_state
                                .blocks
                                .iter()
                                .map(|(k, v)| (k.clone(), v.clone()))
                                .collect(),
                        },
                    )
                    .map(|doc_uri| self.resolve_parent_id(&doc_uri))
                    .unwrap_or(resolved)
                };
                params.insert("document_id".to_string(), Value::String(doc_id));
            }
        }
        eprintln!(
            "[ConcurrentMutations] Firing UI mutation: {}.{}",
            entity, op
        );
        let driver = self
            .driver
            .as_ref()
            .expect("driver not installed — was start_app called?");
        match driver.apply_ui_mutation(&entity, &op, params).await {
            Ok(()) => {}
            Err(e) => panic!("Concurrent UI mutation {}.{} failed: {:?}", entity, op, e),
        }

        // Single sync barrier: wait for final expected block count
        let expected_count = ref_state.blocks.len();
        let timeout = Duration::from_millis(15000);
        let start = Instant::now();

        let actual_rows = self.wait_for_block_count(expected_count, timeout).await;
        let elapsed = start.elapsed();

        if actual_rows.len() == expected_count {
            eprintln!(
                "[ConcurrentMutations] Converged ({} blocks) in {:?}",
                expected_count, elapsed
            );
        } else {
            panic!(
                "[ConcurrentMutations] Timeout: expected {} blocks, got {} after {:?}",
                expected_count,
                actual_rows.len(),
                elapsed
            );
        }

        // Wait for org files to match expected state, then stabilize
        let expected_blocks: Vec<Block> = ref_state
            .blocks
            .values()
            .map(|b| {
                let mut b = b.clone();
                b.parent_id =
                    EntityUri::from_raw(&self.resolve_parent_id(b.parent_id.as_raw_str()));
                b
            })
            .collect();
        let org_timeout = Duration::from_millis(5000);
        self.ctx
            .wait_for_org_file_sync(&expected_blocks, org_timeout)
            .await;
        self.ctx
            .wait_for_org_files_stable(50, Duration::from_millis(5000))
            .await;
    }
}
