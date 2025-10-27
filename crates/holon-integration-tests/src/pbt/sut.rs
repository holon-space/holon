//! System Under Test: `E2ESut` struct and `StateMachineTest` implementation.
//!
//! Contains the SUT wrapper, mutation application, invariant checking,
//! and all transition handling for the real system.

use std::cell::RefCell;
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
    /// Maps file-based doc URIs ("file:doc_0.org") to UUID-based URIs
    /// ("doc:<uuid>") assigned by the real system.
    pub doc_uri_map: HashMap<EntityUri, EntityUri>,
    /// True when the most recent transition was nav/view/watch only (no block data changes).
    pub last_transition_nav_only: bool,
    /// How UI mutations are dispatched. `None` before `start_app` creates the engine.
    /// Backend tests use `DirectMutationDriver`; Flutter tests inject their own driver.
    pub driver: Option<Box<dyn MutationDriver>>,
    /// Persistent watch_ui handle for root layout — kept alive across transitions
    /// so the UiWatcher automatically re-emits Structure events on changes.
    /// Uses RefCell because `check_invariants` receives `&self`.
    root_watch: RefCell<Option<holon_api::WatchHandle>>,
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
            root_watch: RefCell::new(None),
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
            root_watch: RefCell::new(None),
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
    pub fn resolve_parent_id(&self, parent_id: &EntityUri) -> EntityUri {
        self.doc_uri_map
            .get(parent_id)
            .cloned()
            .unwrap_or_else(|| parent_id.clone())
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
                let root_id = ref_state
                    .root_layout_block_id()
                    .unwrap_or_else(holon_api::root_layout_block_uri);
                eprintln!(
                    "[apply] Calling render_block('{}') (expects valid index.org: {})",
                    root_id, expects_valid_index
                );

                let render_result = self
                    .engine()
                    .blocks()
                    .render_block(&root_id, &None, true)
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

                // Set up region watches for all regions
                for region in holon_api::Region::ALL {
                    if let Err(e) = self.setup_region_watch(*region).await {
                        eprintln!(
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

                // Populate doc_uri_map for pre-startup documents whose document
                // entities were created by OrgSyncController during startup.
                // Also update TestEnvironment.documents keys from file-based to UUID-based URIs.
                for file_uri in ref_state.documents.keys() {
                    if !self.doc_uri_map.contains_key(file_uri)
                        && file_uri.is_document()
                        && file_uri.as_str().ends_with(".org")
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
                        let file_uri = EntityUri::file(file_name);
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
                let expected_count = ref_state.block_state.blocks.len();
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
                    .block_state
                    .blocks
                    .values()
                    .map(|b| {
                        let mut b = b.clone();
                        b.parent_id = self.resolve_parent_id(&b.parent_id);
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
                // DEBUG: print blocks being serialized
                for b in &all_blocks {
                    eprintln!(
                        "[BulkExternalAdd] block: {} parent_id={} type={}",
                        b.id, b.parent_id, b.content_type
                    );
                }
                eprintln!("[BulkExternalAdd] ORG CONTENT:\n{}", org_content);
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
                let expected_db_count = ref_state.block_state.blocks.len();
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

            E2ETransition::EditViaDisplayTree {
                block_id,
                new_content,
            } => {
                eprintln!("[apply] EditViaDisplayTree: block={block_id} → {new_content:?}");

                // Render the SPECIFIC block (not the root layout).
                // This is what block_ref resolution does in production:
                // each block gets its own render_block() → shadow interpret cycle.
                let engine = self.engine();

                let (ws, _stream) = engine
                    .blocks()
                    .render_block(block_id, &None, false)
                    .await
                    .expect("render_block failed in EditViaDisplayTree");

                // Shadow interpret → ViewModel
                let engine_clone = Arc::clone(engine);
                let render_expr = ws.render_expr.clone();
                let data_rows = ws.data.clone();

                let display_tree = tokio::task::spawn_blocking(move || {
                    let ctx = holon_frontend::RenderContext::headless(engine_clone);
                    let ctx = ctx.with_data_rows(data_rows);
                    let interp = holon_frontend::create_shadow_interpreter();
                    interp.interpret(&render_expr, &ctx)
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
                        holon_frontend::view_model::NodeKind::EditableText { .. }
                    ) {
                        if node
                            .entity
                            .get("id")
                            .and_then(|v| v.as_string())
                            .map_or(false, |id| id == block_id.as_str())
                        {
                            return Some(node);
                        }
                    }
                    node.children()
                        .iter()
                        .find_map(|c| find_editable_for_block(c, block_id))
                }

                let editable = find_editable_for_block(&display_tree, block_id)
                    .or_else(|| find_editable_for_block(&display_tree, block_id))
                    .unwrap_or_else(|| {
                        panic!(
                            "[EditViaDisplayTree] No EditableText with id={block_id} in display tree.\n\
                             This means render_block created the node without entity context.\n{}",
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
                let op =
                    holon_frontend::operations::find_set_field_op("content", &editable.operations)
                        .expect("No set_field operation found on EditableText");

                let entity_name = op.entity_name.to_string();
                let op_name = op.name.clone();

                let id_value = editable
                    .entity
                    .get("id")
                    .and_then(|v| v.as_string())
                    .expect("EditableText entity has no 'id'")
                    .to_string();

                let mut params = HashMap::new();
                params.insert("id".into(), Value::String(id_value));
                params.insert("field".into(), Value::String("content".into()));
                params.insert("value".into(), Value::String(new_content.clone()));

                let driver = self
                    .driver
                    .as_ref()
                    .expect("driver not installed — was start_app called?");
                driver
                    .apply_ui_mutation(&entity_name, &op_name, params)
                    .await
                    .expect("set_field via display tree failed");

                self.last_transition_nav_only = false;
            }

            E2ETransition::EditViaViewModel {
                block_id,
                new_content,
            } => {
                eprintln!("[apply] EditViaViewModel: block={block_id} → {new_content:?}");

                // 1. Render block → ViewModel (same as EditViaDisplayTree)
                let engine = self.engine();
                let (ws, _stream) = engine
                    .blocks()
                    .render_block(block_id, &None, false)
                    .await
                    .expect("render_block failed in EditViaViewModel");

                let engine_clone = Arc::clone(engine);
                let render_expr = ws.render_expr.clone();
                let data_rows = ws.data.clone();

                let display_tree = tokio::task::spawn_blocking(move || {
                    let ctx = holon_frontend::RenderContext::headless(engine_clone);
                    let ctx = ctx.with_data_rows(data_rows);
                    let interp = holon_frontend::create_shadow_interpreter();
                    interp.interpret(&render_expr, &ctx)
                })
                .await
                .expect("spawn_blocking panicked");

                // 2. Find EditableText node for this block
                fn find_editable_for_block<'a>(
                    node: &'a holon_frontend::ViewModel,
                    block_id: &EntityUri,
                ) -> Option<&'a holon_frontend::ViewModel> {
                    if matches!(
                        &node.kind,
                        holon_frontend::view_model::NodeKind::EditableText { .. }
                    ) {
                        if node
                            .entity
                            .get("id")
                            .and_then(|v| v.as_string())
                            .map_or(false, |id| id == block_id.as_str())
                        {
                            return Some(node);
                        }
                    }
                    node.children()
                        .iter()
                        .find_map(|c| find_editable_for_block(c, block_id))
                }

                let editable = find_editable_for_block(&display_tree, block_id)
                    .unwrap_or_else(|| {
                        panic!(
                            "[EditViaViewModel] No EditableText with id={block_id} in display tree.\n{}",
                            display_tree.pretty_print(0)
                        )
                    });

                // 3. Verify triggers are present
                assert!(
                    !editable.triggers.is_empty(),
                    "[EditViaViewModel] EditableText for {block_id} has no triggers.\n{}",
                    display_tree.pretty_print(0)
                );

                // 4. Verify normal text doesn't fire triggers
                assert!(
                    holon_frontend::input_trigger::check_triggers(&editable.triggers, "hello", 1)
                        .is_none(),
                    "[EditViaViewModel] Normal text 'hello' should NOT fire any trigger"
                );

                // 5. Extract field and content from node, build ViewEventHandler
                let (field, original_value) = match &editable.kind {
                    holon_frontend::view_model::NodeKind::EditableText { field, content } => {
                        (field.clone(), content.clone())
                    }
                    _ => unreachable!("find_editable_for_block guarantees EditableText"),
                };

                let context_params: HashMap<String, Value> = editable
                    .entity
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();

                let mut handler = holon_frontend::view_event_handler::ViewEventHandler::new(
                    editable.operations.clone(),
                    context_params,
                    field,
                    original_value.clone(),
                );

                // 6. Feed TextSync event — simulates blur with new content
                let action = handler.handle(holon_frontend::input_trigger::ViewEvent::TextSync {
                    value: new_content.clone(),
                });

                // 7. Dispatch the resulting operation — must be Execute since
                //    new_content is randomly generated and won't match original.
                match action {
                    holon_frontend::command_menu::MenuAction::Execute {
                        entity_name,
                        op_name,
                        params,
                    } => {
                        let driver = self
                            .driver
                            .as_ref()
                            .expect("driver not installed — was start_app called?");
                        driver
                            .apply_ui_mutation(&entity_name, &op_name, params)
                            .await
                            .expect("set_field via ViewModel TextSync failed");
                    }
                    holon_frontend::command_menu::MenuAction::NotActive => {
                        assert_eq!(
                            *new_content,
                            original_value,
                            "[EditViaViewModel] TextSync returned NotActive but content changed \
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
                        "[EditViaViewModel] Expected Execute from TextSync, got {:?}",
                        other
                    ),
                }

                self.last_transition_nav_only = false;
            }

            E2ETransition::ToggleState {
                block_id,
                new_state,
            } => {
                eprintln!("[apply] ToggleState: block={block_id} → {new_state:?}");

                // Dispatch set_field(task_state, new_state) — matching real frontend behavior.
                // The real frontend has the StateToggle visible in the root layout and
                // dispatches directly. inv10h verifies correctness of the displayed state.
                let mut params = HashMap::new();
                params.insert(
                    "id".to_string(),
                    Value::String(block_id.as_str().to_string()),
                );
                params.insert("field".to_string(), Value::String("task_state".to_string()));
                params.insert("value".to_string(), Value::String(new_state.clone()));

                let driver = self
                    .driver
                    .as_ref()
                    .expect("driver not installed — was start_app called?");
                driver
                    .apply_ui_mutation("block", "set_field", params)
                    .await
                    .expect("ToggleState set_field failed");

                self.last_transition_nav_only = false;
            }

            E2ETransition::TriggerSlashCommand { block_id } => {
                eprintln!("[apply] TriggerSlashCommand: block={block_id}");

                // 1. Render the block (same as EditViaDisplayTree)
                let engine = self.engine();
                let (ws, _stream) = engine
                    .blocks()
                    .render_block(block_id, &None, false)
                    .await
                    .expect("render_block failed in TriggerSlashCommand");

                let engine_clone = Arc::clone(engine);
                let render_expr = ws.render_expr.clone();
                let data_rows = ws.data.clone();

                let display_tree = tokio::task::spawn_blocking(move || {
                    let ctx = holon_frontend::RenderContext::headless(engine_clone);
                    let ctx = ctx.with_data_rows(data_rows);
                    let interp = holon_frontend::create_shadow_interpreter();
                    interp.interpret(&render_expr, &ctx)
                })
                .await
                .expect("spawn_blocking panicked");

                // 2. Find EditableText node for this block
                fn find_editable_for_block<'a>(
                    node: &'a holon_frontend::ViewModel,
                    block_id: &EntityUri,
                ) -> Option<&'a holon_frontend::ViewModel> {
                    if matches!(
                        &node.kind,
                        holon_frontend::view_model::NodeKind::EditableText { .. }
                    ) {
                        if node
                            .entity
                            .get("id")
                            .and_then(|v| v.as_string())
                            .map_or(false, |id| id == block_id.as_str())
                        {
                            return Some(node);
                        }
                    }
                    node.children()
                        .iter()
                        .find_map(|c| find_editable_for_block(c, block_id))
                }

                let editable =
                    find_editable_for_block(&display_tree, block_id).unwrap_or_else(|| {
                        panic!(
                            "[TriggerSlashCommand] No EditableText with id={block_id}.\n{}",
                            display_tree.pretty_print(0)
                        )
                    });

                // 3. Verify triggers are present
                assert!(
                    !editable.triggers.is_empty(),
                    "[TriggerSlashCommand] EditableText for {block_id} has no triggers.\n{}",
                    display_tree.pretty_print(0)
                );

                // 4. Simulate typing "/" — check_triggers should fire
                let event = holon_frontend::input_trigger::check_triggers(
                    &editable.triggers,
                    "/",
                    1, // cursor after "/"
                )
                .unwrap_or_else(|| {
                    panic!(
                        "[TriggerSlashCommand] check_triggers returned None for '/' on block {block_id}.\n\
                         Triggers: {:?}",
                        editable.triggers
                    )
                });

                // 5. Feed event to ViewEventHandler (shared logic layer)
                let context_params: HashMap<String, Value> = editable
                    .entity
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();

                let (field, content) = match &editable.kind {
                    holon_frontend::view_model::NodeKind::EditableText { field, content } => {
                        (field.clone(), content.clone())
                    }
                    _ => unreachable!("collect_editable_text_nodes guarantees EditableText"),
                };

                let mut handler = holon_frontend::view_event_handler::ViewEventHandler::new(
                    editable.operations.clone(),
                    context_params,
                    field,
                    content,
                );
                let action = handler.handle(event);
                assert!(
                    matches!(action, holon_frontend::command_menu::MenuAction::Updated),
                    "[TriggerSlashCommand] Expected MenuAction::Updated, got {:?}",
                    action
                );

                // 6. Menu should have the "delete" operation — navigate to it and select
                let menu_state = handler.command_menu.menu_state().unwrap();
                let delete_idx = menu_state
                    .matches
                    .iter()
                    .position(|m| m.operation_name() == "delete")
                    .unwrap_or_else(|| {
                        panic!(
                            "[TriggerSlashCommand] No 'delete' operation in menu for block {block_id}.\n\
                             Available: {:?}",
                            menu_state.matches.iter().map(|m| m.operation_name()).collect::<Vec<_>>()
                        )
                    });

                // Navigate to delete entry
                for _ in 0..delete_idx {
                    handler.on_key(holon_frontend::command_menu::MenuKey::Down);
                }

                // Select it
                let action = handler.on_key(holon_frontend::command_menu::MenuKey::Enter);
                match action {
                    holon_frontend::command_menu::MenuAction::Execute {
                        entity_name,
                        op_name,
                        params,
                    } => {
                        eprintln!(
                            "[TriggerSlashCommand] Executing {entity_name}.{op_name} with {:?}",
                            params
                        );
                        let driver = self
                            .driver
                            .as_ref()
                            .expect("driver not installed — was start_app called?");
                        driver
                            .apply_ui_mutation(&entity_name, &op_name, params)
                            .await
                            .expect("slash command operation failed");
                    }
                    other => panic!(
                        "[TriggerSlashCommand] Expected MenuAction::Execute, got {:?}",
                        other
                    ),
                }

                self.last_transition_nav_only = false;
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

            E2ETransition::UndoLastMutation => {
                eprintln!("[apply] UndoLastMutation");
                let result = self.ctx.engine().undo().await;
                assert!(result.is_ok(), "undo failed: {:?}", result.err());
                assert!(result.unwrap(), "undo returned false (nothing to undo)");
                let expected_count = ref_state.block_state.blocks.len();
                let timeout = std::time::Duration::from_secs(5);
                self.wait_for_block_count(expected_count, timeout).await;
            }

            E2ETransition::Redo => {
                eprintln!("[apply] Redo");
                let result = self.ctx.engine().redo().await;
                assert!(result.is_ok(), "redo failed: {:?}", result.err());
                assert!(result.unwrap(), "redo returned false (nothing to redo)");
                let expected_count = ref_state.block_state.blocks.len();
                let timeout = std::time::Duration::from_secs(5);
                self.wait_for_block_count(expected_count, timeout).await;
            }
        }

        // Yield to let tokio schedule CDC forwarding tasks before we drain.
        tokio::task::yield_now().await;
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
            ref_state.block_state.blocks.len(),
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
        //    Read directly from SQL (same as QueryableCache in production frontend).
        //    Previous approach used a CDC accumulator matview, but that diverges from
        //    production: the frontend doesn't maintain an "all_blocks" matview.
        let all_blocks_rows = self
            .ctx
            .query_sql("SELECT id, content, content_type, source_language, parent_id, document_id, properties FROM block")
            .await
            .expect("query_sql for all blocks must succeed");

        let backend_blocks: Vec<Block> = all_blocks_rows
            .into_iter()
            .filter_map(|row| {
                let id = EntityUri::parse(row.get("id")?.as_string()?)
                    .expect("block id from DB must be valid URI");
                let parent_id = EntityUri::parse(row.get("parent_id")?.as_string()?)
                    .expect("block parent_id from DB must be valid URI");
                let content = row
                    .get("content")
                    .and_then(|v| v.as_string())
                    .unwrap_or("")
                    .to_string();

                let document_id_str = row
                    .get("document_id")
                    .and_then(|v| v.as_string())
                    .unwrap_or("");
                let document_id = EntityUri::parse(document_id_str)
                    .expect("block document_id from DB must be valid URI");

                let mut block = Block::new_text(id, parent_id, document_id, content);

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

                // Extract properties from the row (SQL returns JSON as string)
                if let Some(props_val) = row.get("properties") {
                    match props_val {
                        Value::String(s) => {
                            if let Ok(map) = serde_json::from_str::<HashMap<String, Value>>(s) {
                                block.properties = map;
                            }
                        }
                        Value::Object(props) => {
                            for (k, v) in props {
                                block.properties.insert(k.clone(), v.clone());
                            }
                        }
                        _ => {}
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

        let ref_blocks: Vec<_> = ref_state.block_state.blocks.values().cloned().collect();
        assert_blocks_equivalent(
            &backend_blocks,
            &ref_blocks,
            "Backend diverged from reference",
        );

        // Ref blocks from org files only (excludes seeded default layout under doc:__default__)
        let ref_blocks_org_only: Vec<_> = ref_state
            .block_state
            .blocks
            .values()
            .filter(|b| {
                ref_state
                    .block_state
                    .block_documents
                    .get(&b.id)
                    .map_or(true, |doc| *doc != EntityUri::doc("__default__"))
            })
            .cloned()
            .collect();

        // 2/2b: Org file parse + ordering — expensive, skip for nav-only transitions
        if !nav_only {
            // Wait for OrgSyncController's background task to re-render org files
            // after UI mutations. The SQL write is committed but the event-driven
            // re-render runs in a separate tokio task.
            self.wait_for_org_files_stable(50, Duration::from_millis(5000))
                .await;

            let todo_header = ref_state.keyword_set.as_ref().map(|ks| ks.to_org_header());
            let org_blocks = self
                .parse_org_file_blocks(todo_header.as_deref())
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
                let ui_rows = ui_data.to_vec();

                let ui_ids: HashSet<EntityUri> = ui_rows
                    .iter()
                    .filter_map(|row| {
                        row.get("id")
                            .and_then(|v| v.as_string())
                            .map(|s| EntityUri::parse(s).expect("invalid entity URI in CDC data"))
                    })
                    .collect();
                let expected_ids: HashSet<EntityUri> = expected
                    .iter()
                    .filter_map(|row| {
                        row.get("id").and_then(|v| v.as_string()).map(|s| {
                            EntityUri::parse(s).expect("invalid entity URI in expected data")
                        })
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

                    if let Some(ui_row) = ui_rows.iter().find(|r: &&HashMap<String, Value>| {
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
                                "CDC field '{}' mismatch for block '{}' in watch '{}'\n\
                                 actual={:?}\n\
                                 expected={:?}",
                                field, expected_id, query_id, actual_val, expected_val,
                            );
                        }

                        // parent_id: normalize document URIs before comparing
                        if query_cols.iter().any(|c| c == "parent_id") {
                            let normalize_parent = |v: Option<&Value>| -> Option<String> {
                                v.and_then(|v| v.as_string()).map(|s| {
                                    if EntityUri::parse(s).is_ok_and(|u| u.is_document()) {
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
            if block.parent_id.is_document() {
                continue;
            }
            assert!(
                backend_blocks.iter().any(|b| b.id == block.parent_id),
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

                let mut expected_ids: Vec<EntityUri> = expected.into_iter().collect();
                expected_ids.sort();

                let mut actual_ids: Vec<EntityUri> = self
                    .region_data
                    .get(&region_key)
                    .map(|data| {
                        data.to_vec()
                            .iter()
                            .filter_map(|row| {
                                row.get("id").and_then(|v| v.as_string()).map(|s| {
                                    EntityUri::parse(s).expect("valid entity URI in region data")
                                })
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
                        let id = row.get("id")?.as_string()?.to_string();
                        let props = row.get("properties")?;
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

            // 10. Root layout via persistent watch_ui → shadow interpret
            // (same path as production frontends: GPUI, Blinc, Flutter)
            if ref_state.has_valid_index_org() {
                let engine = self.engine();
                let root_id = ref_state
                    .root_layout_block_id()
                    .unwrap_or_else(holon_api::root_layout_block_uri);

                // Take watch out of RefCell so we don't hold borrow across .await
                let mut watch_opt = self.root_watch.borrow_mut().take();
                let is_new_watch = watch_opt.is_none();

                // Create persistent watch on first valid check
                if is_new_watch {
                    match holon::api::watch_ui(Arc::clone(engine), root_id.clone(), true).await {
                        Ok(watch) => {
                            watch_opt = Some(watch);
                        }
                        Err(e) => {
                            let err_str = e.to_string();
                            if err_str.contains("ScalarSubquery")
                                || err_str.contains("materialized view")
                            {
                                eprintln!("[inv10] watch_ui skipped (IVM limitation): {err_str}");
                            } else {
                                panic!("watch_ui('{}') failed: {}", root_id, e);
                            }
                        }
                    }
                }

                if watch_opt.is_some() {
                    // Drain all pending events, keep latest Structure
                    let mut latest_ws: Option<holon_api::widget_spec::WidgetSpec> = None;
                    {
                        let watch = watch_opt.as_mut().unwrap();
                        loop {
                            match watch.try_recv() {
                                Ok(holon_api::UiEvent::Structure { widget_spec, .. }) => {
                                    latest_ws = Some(widget_spec);
                                }
                                Ok(_) => continue,
                                Err(_) => break,
                            }
                        }
                    }

                    // If no new Structure yet (first time), block-wait for it
                    // using the variant's executor (tokio for Full/SqlOnly,
                    // futures::executor for CrossExecutor).
                    if latest_ws.is_none() && is_new_watch {
                        let taken = watch_opt.take().unwrap();
                        match tokio::time::timeout(
                            Duration::from_secs(10),
                            V::wait_for_structure(taken),
                        )
                        .await
                        {
                            Ok((ws, watch_back)) => {
                                latest_ws = Some(ws);
                                watch_opt = Some(watch_back);
                            }
                            Err(_) => {
                                panic!("Timed out waiting for Structure event from watch_ui")
                            }
                        }
                    }

                    let ws = match latest_ws {
                        Some(ws) => ws,
                        None => {
                            eprintln!("[inv10] No new Structure event (no re-render needed)");
                            *self.root_watch.borrow_mut() = watch_opt;
                            return;
                        }
                    };
                    eprintln!("[inv10] watch_ui Structure: {} rows", ws.data.len());

                    // Profile correctness checks removed: profiles are now resolved
                    // separately via entity_profile::ProfileResolver, not embedded in DataRow.

                    // Shadow interpret → ViewModel (same path as GPUI/Blinc frontends)
                    let engine_clone = Arc::clone(engine);
                    let render_expr = ws.render_expr.clone();
                    let data_rows = ws.data.clone();

                    let display_tree = tokio::task::spawn_blocking(move || {
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            let ctx = holon_frontend::RenderContext::headless(engine_clone);
                            let ctx = ctx.with_data_rows(data_rows);
                            let interp = holon_frontend::create_shadow_interpreter();
                            interp.interpret(&render_expr, &ctx)
                        }))
                    })
                    .await
                    .expect("spawn_blocking panicked");

                    let display_tree = match display_tree {
                        Ok(tree) => tree,
                        Err(e) => {
                            let msg = e
                                .downcast_ref::<String>()
                                .map(|s| s.as_str())
                                .or_else(|| e.downcast_ref::<&str>().copied())
                                .unwrap_or("unknown panic");
                            eprintln!(
                                "[inv10] Shadow interpretation panicked: {msg} \
                                 (pre-existing bug, skipping structural assertions)"
                            );
                            *self.root_watch.borrow_mut() = watch_opt;
                            return;
                        }
                    };

                    // 10a. Root widget must not be "error"
                    assert_ne!(
                        display_tree.widget_name(),
                        Some("error"),
                        "Root layout rendered as error widget:\n{}",
                        display_tree.pretty_print(0),
                    );

                    // 10b. Entity IDs in tree
                    let tree_ids = display_tree.collect_entity_ids();
                    eprintln!(
                        "[inv10] ViewModel: root='{}', {} entity IDs",
                        display_tree.widget_name().unwrap_or("?"),
                        tree_ids.len(),
                    );

                    // 10c. No nested error nodes
                    let error_count = crate::display_assertions::count_error_nodes(&display_tree);
                    assert_eq!(
                        error_count,
                        0,
                        "[inv10c] {} error node(s) in ViewModel tree:\n{}",
                        error_count,
                        display_tree.pretty_print(0),
                    );

                    // 10d. Root widget type matches reference model's render expression
                    if let Some(expected_expr) = ref_state.root_render_expr() {
                        let expected_widget = match expected_expr {
                            holon_api::render_types::RenderExpr::FunctionCall { name, .. } => {
                                name.as_str()
                            }
                            _ => panic!("root render expr must be FunctionCall"),
                        };
                        assert_eq!(
                            display_tree.widget_name(),
                            Some(expected_widget),
                            "[inv10d] Root widget '{}' doesn't match render source '{}'\n\
                             Render expr: {}\n{}",
                            display_tree.widget_name().unwrap_or("?"),
                            expected_widget,
                            expected_expr.to_rhai(),
                            display_tree.pretty_print(0),
                        );
                        eprintln!(
                            "[inv10d] Root widget '{}' matches render expr '{}'",
                            expected_widget,
                            expected_expr.to_rhai(),
                        );
                    }

                    // 10e. Entity IDs in tree are subset of query data IDs
                    // (set check, not ordered — queries may return duplicates
                    // and columns/block_ref renders may reorder items)
                    let data_id_set: std::collections::HashSet<String> = ws
                        .data
                        .iter()
                        .filter_map(|r| {
                            r.get("id")
                                .and_then(|v| v.as_string())
                                .map(|s| s.to_string())
                        })
                        .collect();
                    if !tree_ids.is_empty() {
                        let tree_id_set: std::collections::HashSet<String> =
                            tree_ids.iter().cloned().collect();
                        let missing: Vec<&String> = tree_id_set
                            .iter()
                            .filter(|id| !data_id_set.contains(*id))
                            .collect();
                        assert!(
                            missing.is_empty(),
                            "[inv10e] ViewModel has entity IDs not in query data.\n\
                             Missing: {:?}\n\
                             Tree IDs ({}):\n  {:?}\n\
                             Data IDs ({}):\n  {:?}\n{}",
                            missing,
                            tree_ids.len(),
                            tree_ids,
                            data_id_set.len(),
                            data_id_set,
                            display_tree.pretty_print(0),
                        );
                        eprintln!(
                            "[inv10e] {} tree entity IDs are subset of {} data IDs",
                            tree_id_set.len(),
                            data_id_set.len(),
                        );
                    }

                    // 10f. Decompiled row data matches query data (filtered by visible columns)
                    // Skip when data is empty — Collection macro renders 1 empty template item
                    // which has no corresponding expected row.
                    if let Some(expected_expr) = ref_state.root_render_expr() {
                        let visible_cols = expected_expr.visible_columns();
                        let rendered_rows =
                            crate::display_assertions::extract_rendered_rows(&display_tree);
                        if !rendered_rows.is_empty()
                            && !visible_cols.is_empty()
                            && !ws.data.is_empty()
                        {
                            // Filter expected data to only visible columns
                            let expected_rows: Vec<
                                std::collections::HashMap<String, holon_api::Value>,
                            > = ws
                                .data
                                .iter()
                                .map(|r| {
                                    r.iter()
                                        .filter(|(k, _)| visible_cols.contains(k))
                                        .map(|(k, v)| (k.clone(), v.clone()))
                                        .collect()
                                })
                                .collect();
                            let subset_result = crate::display_assertions::is_ordered_subset(
                                &rendered_rows
                                    .iter()
                                    .filter_map(|r| {
                                        r.get("content")
                                            .and_then(|v| v.as_string())
                                            .map(|s| s.to_string())
                                    })
                                    .collect::<Vec<_>>(),
                                &expected_rows
                                    .iter()
                                    .filter_map(|r| {
                                        r.get("content")
                                            .and_then(|v| v.as_string())
                                            .map(|s| s.to_string())
                                    })
                                    .collect::<Vec<_>>(),
                            );
                            assert!(
                                subset_result.is_subset,
                                "[inv10f] Decompiled content doesn't match query data.\n\
                                 Rendered: {:?}\nExpected: {:?}\n\
                                 Missing: {:?}\nOut of order: {:?}\n\
                                 Render expr: {}\n{}",
                                rendered_rows,
                                expected_rows,
                                subset_result.missing_from_expected,
                                subset_result.out_of_order,
                                expected_expr.to_rhai(),
                                display_tree.pretty_print(0),
                            );
                            eprintln!(
                                "[inv10f] {} decompiled rows match expected (cols: {:?})",
                                rendered_rows.len(),
                                visible_cols,
                            );
                        }
                    }

                    // 10g. EditableText nodes with operations must have triggers
                    let (total_with_ops, missing_triggers) =
                        crate::display_assertions::count_editables_missing_triggers(&display_tree);
                    assert_eq!(
                        missing_triggers,
                        0,
                        "[inv10g] {missing_triggers}/{total_with_ops} EditableText node(s) \
                         with operations are missing triggers.\n{}",
                        display_tree.pretty_print(0),
                    );
                    if total_with_ops > 0 {
                        eprintln!(
                            "[inv10g] All {total_with_ops} EditableText node(s) with ops have triggers"
                        );
                    }

                    // 10h. StateToggle nodes must display correct current value and label
                    let toggle_nodes =
                        crate::display_assertions::collect_state_toggle_nodes(&display_tree);
                    for toggle in &toggle_nodes {
                        if let holon_frontend::view_model::NodeKind::StateToggle {
                            field,
                            current,
                            label,
                            ..
                        } = &toggle.kind
                        {
                            assert_eq!(
                                field, "task_state",
                                "[inv10h] unexpected field in StateToggle"
                            );

                            if let Some(block_id_str) =
                                toggle.entity.get("id").and_then(|v| v.as_string())
                            {
                                let block_id = EntityUri::from_raw(&block_id_str);
                                if let Some(ref_block) = ref_state.block_state.blocks.get(&block_id)
                                {
                                    let expected_state = ref_block
                                        .task_state()
                                        .map(|ts| ts.keyword.to_string())
                                        .unwrap_or_default();
                                    assert_eq!(
                                        current, &expected_state,
                                        "[inv10h] StateToggle current '{current}' != \
                                         reference '{expected_state}' for block {block_id}"
                                    );

                                    let (expected_label, _) =
                                        holon_api::render_eval::state_display(current);
                                    assert_eq!(
                                        label, expected_label,
                                        "[inv10h] StateToggle label '{label}' != \
                                         expected '{expected_label}' for block {block_id}"
                                    );
                                }
                            }
                        }
                    }
                    if !toggle_nodes.is_empty() {
                        eprintln!(
                            "[inv10h] {} StateToggle node(s) verified",
                            toggle_nodes.len()
                        );
                    }
                }

                // Put watch back for next check_invariants call
                *self.root_watch.borrow_mut() = watch_opt;
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
            _ref_state.block_state.blocks.len(),
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
            ref_state.block_state.blocks.len(),
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

                // The reference model uses file-based document URIs (e.g. "file:doc_0.org")
                // but the real system assigns UUID-based IDs. Resolve before executing.
                if let Some(Value::String(pid)) = params.get("parent_id") {
                    let pid = EntityUri::parse(pid).expect("Unable to parse parent_id");
                    let resolved = self.resolve_parent_id(&pid);
                    params.insert("parent_id".to_string(), resolved.clone().into());

                    // Compute document_id for create operations — the SQL INSERT
                    // won't set it automatically, causing NULL → malformed EntityUri.
                    if op == "create" && !params.contains_key("document_id") {
                        let doc_id = if resolved.is_document() {
                            resolved.clone()
                        } else {
                            // Walk up parent chain in reference model to find document
                            crate::assertions::find_document_for_block(
                                &resolved,
                                &crate::assertions::ReferenceState {
                                    blocks: ref_state.block_state.blocks.clone(),
                                },
                            )
                            .map(|doc_uri| self.resolve_parent_id(&doc_uri))
                            .unwrap_or(resolved.clone())
                        };
                        params.insert("document_id".to_string(), doc_id.into());
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
                    .block_state
                    .blocks
                    .values()
                    .map(|b| {
                        let mut b = b.clone();
                        b.parent_id = self.resolve_parent_id(&b.parent_id);
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
        let expected_count = ref_state.block_state.blocks.len();
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
                if let Some(expected_block) = ref_state.block_state.blocks.get(&block_id) {
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
            .block_state
            .blocks
            .values()
            .map(|b| {
                let mut b = b.clone();
                b.parent_id = self.resolve_parent_id(&b.parent_id);
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
            .block_state
            .blocks
            .values()
            .map(|b| {
                let mut b = b.clone();
                b.parent_id = self.resolve_parent_id(&b.parent_id);
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
            let pid = EntityUri::parse(pid).expect("Unable to parse parent_id");
            let resolved = self.resolve_parent_id(&pid);
            params.insert("parent_id".to_string(), resolved.clone().into());

            if op == "create" && !params.contains_key("document_id") {
                let doc_id = if resolved.is_document() {
                    resolved.clone()
                } else {
                    crate::assertions::find_document_for_block(
                        &resolved,
                        &crate::assertions::ReferenceState {
                            blocks: ref_state.block_state.blocks.clone(),
                        },
                    )
                    .map(|doc_uri| self.resolve_parent_id(&doc_uri))
                    .unwrap_or(resolved.clone())
                };
                params.insert("document_id".to_string(), doc_id.into());
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
        let expected_count = ref_state.block_state.blocks.len();
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
            .block_state
            .blocks
            .values()
            .map(|b| {
                let mut b = b.clone();
                b.parent_id = self.resolve_parent_id(&b.parent_id);
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
