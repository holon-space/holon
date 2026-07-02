//! The SUT component of the frontend slice: a [`CapProvider`] wrapping a
//! **real** headless frontend stack — the production `FrontendSession` +
//! `ReactiveEngine` over a Turso `BackendEngine`, built through the exact DI path
//! the GPUI/CLI frontends use ([`holon_app::new_from_config_with_di`]) — but
//! **windowless**: no GPUI, no geometry, no display link. This is the
//! ViewModel/Renderer slice of the future `E2ESut` replacement.
//!
//! It provides [`SutRenderer`] over the same headless interpret pipeline
//! `E2ESut` uses for its render invariants: `ReactiveEngine::ensure_watching` →
//! `ReactiveRenderedRows::snapshot` → `holon_frontend::interpret_pure` against a
//! `HeadlessBuilderServices`, then the shared `view_model_to_snapshot`. So the
//! catalog's renderer invariants run over the **real** CDC→watch→render path,
//! not a re-implementation.
//!
//! It also provides [`SutBackend`] over `block_raw` (so the block-tree catalog
//! runs over this realization too, §6) and hosts the sync owned-return
//! [`RefRender`] cap (folded in here so it is never dead code, §F4).

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use holon::api::BackendEngine;
use holon_api::{Block, EntityUri, QueryLanguage, StorageEntity, Value};
use holon_app::HeadlessBuilderServices;
use holon_frontend::reactive::{BuilderServices, ReactiveEngine, ReactiveRenderedRows, table_expr};
use holon_frontend::{FrontendSession, ReactiveEngineDriver, UserDriver};
use holon_pbt_core::capabilities::{
    CapRegion, FrontendRootVm, ProviderStabilityReport, SutBackend, SutBlockTreeWrite,
    SutEditorMirrorRead, SutEditorMirrorWrite, SutErrorLog, SutFocusWrite, SutHistoryWrite,
    SutMcpEmit, SutNavHistoryDrive, SutNavHistoryWrite, SutOrgRead, SutOrgRender, SutQueryResults,
    SutRenderer, SutSqlProjection, SutViewControl, SutViewModel, SutWatchRegister, SutWatchRows,
    ViewportHint, WatchRow, WidgetSnapshot,
};
use holon_pbt_core::composition::{CapMap, CapProvider};
use tempfile::TempDir;

use crate::pbt::local_caps::{SutAppLifecycle, SutMutate, SutSeamMutate};
use crate::pbt::query::TestQuery;
use crate::pbt::transitions::toggle_state::{CycleTarget, cycle_click_count};
use crate::pbt::types::{Mutation, MutationEvent};

use crate::pbt::sut_capabilities::view_model_to_snapshot;
use crate::pbt::sut_row_parsing::{
    BLOCK_MATVIEW_SNAPSHOT_SQL, BLOCK_RAW_SNAPSHOT_SQL, parse_block_rows,
};

/// A composition component wrapping a real headless frontend stack. Owns the
/// `TempDir`, `FrontendSession`, and `ReactiveEngine` so background tasks and
/// the on-disk (in-memory FS) org root stay alive for the component's lifetime.
pub struct HeadlessFrontendComponent {
    engine: Arc<BackendEngine>,
    reactive: Arc<ReactiveEngine>,
    /// The production headless `UserDriver` over `reactive` — the SAME
    /// `ReactiveEngineDriver` the GPUI/TUI/CLI frontends install. Hosts the
    /// production `HeadlessEditorMirror`, so `apply_focus_editable_text` (open an
    /// editor = `click_entity`) and the keystroke-driven `SutEditorMirrorWrite`
    /// caps (`apply_type_chars`/`apply_delete_backward`/`apply_move_cursor` →
    /// `send_raw_keystroke`) drive the EXACT production headless editor pipeline —
    /// no `InMemEditorComponent` stand-in, no GPUI window/geometry. Caret reads
    /// (`SutEditorMirrorRead::editor_caret_byte`) come from this driver's mirror.
    driver: Arc<ReactiveEngineDriver>,
    /// The production `FrontendSession` — drives navigation through the same
    /// `execute_operation("navigation", "focus", …)` op path the GPUI/CLI
    /// frontends use (`SutFocusWrite`). Retained (no longer `_`-prefixed) so the
    /// focus write cap can dispatch through it.
    session: Arc<FrontendSession>,
    _temp: TempDir,
    /// `query_id → query:<hash>` registry-key mapping for the watches this
    /// component has registered (E1: `SutWatchRows` over the PRODUCTION reactive
    /// watch surface). Production keys query watches by content hash, so the
    /// component tracks the test's `query_id` against the engine key it got back
    /// from [`ReactiveEngine::watch_query_live`] — the only bookkeeping needed.
    watches: Mutex<Vec<(String, EntityUri)>>,
    /// The in-memory org FS, its root, and the tracked org file paths — retained
    /// so the component can provide `SutOrgRead` by parsing the on-disk org files
    /// back into blocks (E1: org block-equivalence over the PRODUCTION
    /// `holon_orgmode::parser::parse_org_file`, no `FileSyncController` needed).
    org_fs: Arc<holon_filesystem::InMemoryFileSystem>,
    org_root: PathBuf,
    org_paths: Vec<PathBuf>,
    /// `(resolved doc-block id, file path)` per tracked org file, cached from a
    /// clean boot parse — the disk-independent doc mapping `SutOrgRender` renders by.
    /// Tracked user-doc org files (doc page id → path). Interior-mutable: seeded at
    /// boot AND appended by `create_document` so a mid-run `CreateDocument` doc becomes a
    /// valid target for `BulkExternalAdd`/External `ApplyMutation` (which look up the file).
    documents: Mutex<Vec<(EntityUri, PathBuf)>>,
    /// The captured DI injector — `SutOrgRender` resolves the
    /// `QueryableCache<Block>` from it to build the production `CacheBlockReader`
    /// (the doc-scoped recursive CTE ordered by `sort_key, id`, so descendants
    /// render in the exact order the `FileSyncController` writes them).
    injector: fluxdi::Injector,
    /// The active view/mode name (`SutViewControl::switch_view` writes it,
    /// `SutViewModel::current_view` reads it). Honest tracked state replacing the
    /// former hardcoded `"all"` stub — a faithful port of `TestEnvironment`'s
    /// `current_view` (default `"all"`). Drives the `SwitchView` PBT transition's
    /// effect so the view-selection oracle observes it on the composed path.
    current_view: Mutex<String>,
    /// Shared oracle-synthetic → SUT-real id map (the same [`IdResolver`] the
    /// composed [`OpDispatchWriter`] accumulates split reconciliations into). Set by
    /// the composed builder so the id-taking nav/focus caps
    /// (`pin_block`/`apply_navigate_focus`/`apply_focus_editable_text`) translate an
    /// oracle id (e.g. a synthetic `block::split-N` the generator drew from the
    /// oracle's descendants) to the real minted id before dispatching — exactly as the
    /// block-tree writer already does. Unset (`OnceLock` empty) ⇒ identity resolution
    /// (the fixed-id slices, where oracle id == store id).
    resolver: std::sync::OnceLock<crate::pbt::op_write_cap::IdResolver>,
}

impl HeadlessFrontendComponent {
    /// Stand up a windowless frontend session over the given org files (written
    /// to an in-memory FS before the engine boots, exactly as a real frontend
    /// finds files already on disk), then settle briefly for the initial CDC
    /// sync. `org_files` is `(filename, content)`. Loro is OFF (Turso-only
    /// storage) — the navigation/structural slices don't need the CRDT layer.
    pub async fn new(org_files: &[(&str, &str)], settle: Duration) -> Self {
        Self::new_with_loro(org_files, settle, false).await
    }

    /// Like [`Self::new`] but with the Loro CRDT layer ENABLED — the production
    /// bootstrap then registers `LoroModule` (the `BlockCellRegistry` backing
    /// `MutableText`) and ingests the org tree through Loro storage, so the editor
    /// caps (`SutEditorMirrorWrite`) can resolve a block's `content_raw` cell and
    /// type into it. Required for any config that drives `TypeChars`/`DeleteBackward`
    /// (the editor primitives are no-ops without a `MutableText`, exactly why
    /// `general_e2e_pbt_sql_only` can't run them).
    pub async fn new_with_loro(
        org_files: &[(&str, &str)],
        settle: Duration,
        loro_enabled: bool,
    ) -> Self {
        use holon_frontend::{HolonConfig, SessionConfig};

        let temp = TempDir::new().expect("temp dir");
        let org_root = std::fs::canonicalize(temp.path()).expect("canonicalize temp dir");
        let org_fs = Arc::new(holon_filesystem::InMemoryFileSystem::new());
        org_fs.mkdir_all(&org_root);

        let mut org_paths: Vec<PathBuf> = Vec::new();
        for (filename, content) in org_files {
            let file_path = org_root.join(filename);
            if let Some(parent) = file_path.parent() {
                org_fs.mkdir_all(parent);
            }
            holon_filesystem::FileSystem::write(org_fs.as_ref(), &file_path, content.as_bytes())
                .await
                .expect("write seed org file");
            org_paths.push(file_path);
        }

        let holon_config = HolonConfig {
            db_path: Some(temp.path().join("test.db")),
            vault: holon_frontend::config::VaultConfig {
                root: Some(temp.path().to_path_buf()),
            },
            crdt: holon_frontend::config::CrdtPreferences {
                enabled: Some(loro_enabled),
                ..Default::default()
            },
            ..Default::default()
        };
        let config_dir = temp.path().to_path_buf();
        let session_config = SessionConfig::new(holon_api::UiInfo::permissive()).without_wait();
        let org_fs_for_di = org_fs.clone();
        // Capture the DI injector (for `SutOrgRender`'s `QueryableCache<Block>` →
        // `CacheBlockReader`, the production ordered doc-scoped read).
        let injector_slot: Arc<std::sync::OnceLock<fluxdi::Injector>> =
            Arc::new(std::sync::OnceLock::new());
        let injector_slot_c = injector_slot.clone();

        let (session, engine, reactive) = holon_app::new_from_config_with_di(
            holon_config,
            session_config,
            config_dir,
            std::collections::HashSet::new(),
            move |injector| {
                use holon_frontend::reactive::{BuilderServicesSlot, RenderInterpreterInjectorExt};
                crate::test_environment::override_org_fs_bindings(injector, &org_fs_for_di);
                let slot = injector.resolve::<BuilderServicesSlot>();
                injector.set_render_interpreter(holon_frontend::reactive::make_interpret_fn(
                    slot.0.clone(),
                ));
                Ok(())
            },
            move |injector| {
                use holon_frontend::reactive::{BuilderServicesSlot, ReactiveEngine};
                let engine = injector.resolve::<ReactiveEngine>();
                let slot = injector.resolve::<BuilderServicesSlot>();
                let services: Arc<dyn BuilderServices> = engine.clone();
                slot.0.set(services).ok(); // ALLOW(ok): OnceLock set — idempotent
                injector_slot_c.set(injector.clone()).ok(); // ALLOW(ok): OnceLock set
                engine
            },
        )
        .await
        .expect("build headless frontend session");

        // Wire the Loro-backed `BlockCellRegistry` into the reactive engine — the
        // editor's `MutableText` cells resolve through it. The real GPUI/TUI
        // frontends do this in their own `on_start`; the windowless build bypasses
        // that, so `editable_text` would return Err and the keystroke pipeline
        // would bail ("no MutableText for focused block"). `LoroModule` (enabled by
        // `loro.enabled`) registers the registry built over the global Loro doc, so
        // resolving it here gives the SAME doc the op pipeline + `block_raw`
        // projection share — typed text lands in the projection the invariant reads.
        // Mirrors `E2ESut`'s `ensure_reactive_engine` registry wiring (`sut.rs`).
        if loro_enabled {
            let injector = injector_slot
                .get()
                .expect("DI injector captured during build");
            let registry: Arc<holon::sync::block_cell_registry::BlockCellRegistry> = injector
                .resolve_async::<holon::sync::block_cell_registry::BlockCellRegistry>()
                .await;
            let registry_dyn: Arc<dyn holon_frontend::cell::EntityCellRegistry> = registry;
            reactive
                .block_cell_registry
                .lock()
                .unwrap()
                .replace(registry_dyn);
        }

        if settle > Duration::ZERO {
            tokio::time::sleep(settle).await;
        }

        // Cache each tracked file's resolved doc-block id from a CLEAN parse at boot
        // (disk now carries the session-persisted `:ID:` drawer == the block_raw doc
        // row). `SutOrgRender` uses this disk-INDEPENDENT mapping so a later disk
        // divergence is detected, not silently skipped (deriving the doc id from a
        // corrupted disk would miss the block_raw row and vacuously pass).
        let mut documents: Vec<(EntityUri, PathBuf)> = Vec::new();
        for path in &org_paths {
            let raw = holon_filesystem::FileSystem::read_to_string(org_fs.as_ref(), path)
                .await
                .expect("cache doc ids: read org file");
            let parsed = holon_orgmode::parser::parse_org_file(
                path,
                &raw,
                &EntityUri::no_parent(),
                &org_root,
            )
            .expect("cache doc ids: parse org file");
            if let Some(doc_id) = parsed.blocks.first().map(|b| b.parent_id.clone()) {
                documents.push((doc_id, path.clone()));
            }
        }

        let driver = Arc::new(ReactiveEngineDriver::new(reactive.clone()));

        Self {
            engine,
            reactive,
            driver,
            session,
            _temp: temp,
            watches: Mutex::new(Vec::new()),
            documents: Mutex::new(documents),
            org_fs,
            org_root,
            org_paths,
            injector: injector_slot
                .get()
                .expect("DI injector captured during build")
                .clone(),
            current_view: Mutex::new("all".to_string()),
            resolver: std::sync::OnceLock::new(),
        }
    }

    /// The production `BackendEngine` backing this session — shared (`Arc`) so a
    /// composed structural SUT (`frontend_slice::structural_pbt`) can build the
    /// resolver-sharing [`OpDispatchWriter`] over it and seed the working tree via
    /// the production create op (`crate::pbt::sql_slice::SqlProjectionComponent`).
    pub(crate) fn engine(&self) -> Arc<BackendEngine> {
        self.engine.clone()
    }

    /// The production windowless `FrontendSession` this component booted. Handed to a
    /// gpui window (`launch_holon_window_rebindable`) so the window RENDERS the same
    /// reactive tree the composed backend/storage caps read — the windowed repoint
    /// reuses this headless boot and attaches the window as a pure renderer (§ Round 5).
    pub(crate) fn session(&self) -> Arc<FrontendSession> {
        self.session.clone()
    }

    /// The production `ReactiveEngine` (the `BuilderServices` host). The wide PBT's
    /// resolver-sharing [`OpDispatchWriter`] uses it as a focus sink so `split_block`/
    /// `join_block` dispatch through the frontend's `dispatch_intent_sync` and the new/
    /// merged block becomes the focused block (the frontend split focus-handoff).
    pub(crate) fn reactive(&self) -> Arc<ReactiveEngine> {
        self.reactive.clone()
    }

    /// The production headless [`UserDriver`] (`ReactiveEngineDriver`) — the SAME
    /// instance the editor/focus caps drive through, so it hosts the one live
    /// `HeadlessEditorMirror`. Handed to the headless driver-backed input component
    /// (`DriverInputComponent::with_input_headless`) so the composed `CapMap`'s gesture
    /// caps (`SutBlockInteract`/`SutArrowNavigate`/`SutDriver`) drive the UI-adjacent
    /// logic layer over ONE driver (the VM rung, §8.11). MUST be this instance, not a
    /// fresh `ReactiveEngineDriver::new` — a second one would carry a separate editor
    /// mirror and desync caret/text from the editor-write caps.
    pub(crate) fn driver(&self) -> Arc<dyn UserDriver> {
        self.driver.clone()
    }

    /// The frontend's `LoroDocumentStore` — the authority store the production op
    /// pipeline writes (`LoroBlockOperations`) and `block_raw` projects from. `None`
    /// when Loro is disabled on this build (`new_with_loro(.., false)` → no
    /// `LoroModule` registered). The composed builder's Loro arm reads its
    /// `SutLoroTaskState`/`SutLoroLog` caps over THIS store's global doc (not a
    /// separate one) so a write through the frontend is visible to the Loro read
    /// caps — the read-doc unification (task #4). The clone shares the underlying
    /// `Arc<RwLock<Option<Arc<LoroDocument>>>>`, so it observes the SAME live doc.
    pub(crate) fn loro_doc_store(&self) -> Option<holon::sync::LoroDocumentStore> {
        self.injector
            .try_resolve::<holon::sync::LoroDocumentStore>()
            .ok() // ALLOW(ok): optional DI service — absent when Loro is disabled
            .map(|store| (*store).clone())
    }

    /// The frontend session's `LoroSyncController` handle — the controller that
    /// watches the authority doc (`subscribe_root`) and projects imported peer
    /// deltas into the Turso `block_raw` the block invariants read. The composed
    /// builder's Loro arm hands this to `LoroSut` in full mode so a `MergeFromPeer`
    /// can `wait_for_quiescence` on it before the merged block is read back (the
    /// projection is async — `loro_sync_controller.rs` runs it on a spawned loop).
    /// Mirrors E2ESut's `ctx.loro_sync_handle()` (`sut_handle.rs:197`).
    ///
    /// Resolution is RACE-prone: the headless build uses `without_wait()`, so the
    /// controller is started on a spawned `post_ready_work` task and is NOT awaited
    /// at boot (`wiring.rs:360`). Callers that need it present must poll until the
    /// boot settle completes (see the A0 readiness probe). `None` when Loro/sync is
    /// disabled OR the spawned start task has not yet resolved the handle.
    pub(crate) fn loro_sync_handle(&self) -> Option<Arc<holon::sync::LoroSyncControllerHandle>> {
        self.injector
            .try_resolve::<holon::sync::LoroSyncControllerHandle>()
            .ok() // ALLOW(ok): optional DI service — absent when Loro/sync is disabled
    }

    /// Share the composed runner's [`IdResolver`] so the id-taking nav/focus caps
    /// translate oracle ids to SUT-real ids (see the `resolver` field). Called once
    /// by the composed builder; the storage-only/fixed-id slices leave it unset
    /// (identity resolution).
    pub(crate) fn set_resolver(&self, resolver: crate::pbt::op_write_cap::IdResolver) {
        self.resolver
            .set(resolver)
            .map_err(|_| "HeadlessFrontendComponent resolver already set")
            .expect("set resolver once");
    }

    /// Resolve an oracle-space id to its SUT-space id (identity if the resolver is
    /// unset or the id is unmapped) — the component-side analog of
    /// [`OpDispatchWriter::resolve`].
    fn resolve_id(&self, id: &EntityUri) -> EntityUri {
        match self.resolver.get() {
            Some(r) => r
                .lock()
                .expect("resolver lock")
                .get(id)
                .cloned()
                .unwrap_or_else(|| id.clone()),
            None => id.clone(),
        }
    }

    /// Register a watched query through the **production** reactive watch surface
    /// ([`ReactiveEngine::watch_query_live`] → `ensure_query_watching` → the real
    /// `registry`/`watchers` + CDC pump into [`ReactiveRenderedRows`]), and record
    /// the `query_id → query:<hash>` key mapping. This is the production analogue of
    /// the E2ESut harness's hand-rolled `setup_watch`/`ui_model` — the same
    /// `TestQuery::compile_for` source the wide PBT uses, but driven through the
    /// reactive engine the headless frontend actually runs, not a bespoke
    /// `CdcAccumulator`.
    pub fn register_query_watch(&self, query_id: &str, query: &TestQuery, lang: QueryLanguage) {
        let (source, lang) = query.compile_for(lang);
        self.register_watch_compiled(query_id, source, lang);
    }

    /// Shared core: register a watch from an already-compiled `(source, lang)`
    /// through the production reactive watch surface. Used by both the test
    /// helper [`Self::register_query_watch`] and the `SutWatchRegister` cap (the
    /// decomposed `SetupWatch` drive path — INC 3), which receives the query
    /// pre-compiled at the transition boundary.
    fn register_watch_compiled(&self, query_id: &str, source: String, lang: QueryLanguage) {
        let services: Arc<dyn BuilderServices> = self.reactive.clone();
        let (key, _live) =
            self.reactive
                .watch_query_live(source, lang, table_expr(), None, services);
        self.watches
            .lock()
            .expect("watches lock")
            .push((query_id.to_string(), key));
    }

    /// Resolve a ready (non-loading) reactive watch for `uri`, polling the
    /// headless engine until its first results load (background tasks fill it on
    /// the shared runtime). Mirrors `E2ESut::resolve_watch`'s no-frontend-engine
    /// branch.
    async fn resolve_watch(&self, uri: &EntityUri) -> Option<Arc<ReactiveRenderedRows>> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            let rqr = self.reactive.ensure_watching(uri);
            if !rqr.is_loading() {
                return Some(rqr);
            }
            if tokio::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    fn services(&self) -> Arc<dyn BuilderServices> {
        Arc::new(HeadlessBuilderServices::new(self.engine.clone()))
    }

    /// Graft a block into the headless backend (used to attach a fixed-id subtree
    /// under the Main focus root so `inv-displayed-text` has known content to
    /// compare). Mirrors the windowed slice's `graft_displayed_text_tree`, but via
    /// the `BackendEngine` directly (no `TestEnvironment`).
    pub async fn create_block(&self, id: &str, parent_id: &str, content: &str) {
        use holon_api::types::ContentType;
        use holon_api::{EntityName, StorageEntity, Value};
        let mut params: StorageEntity = std::collections::HashMap::new();
        params.insert(
            "id".into(),
            Value::String(EntityUri::from_raw(id).to_string()),
        );
        params.insert(
            "parent_id".into(),
            Value::String(EntityUri::from_raw(parent_id).to_string()),
        );
        params.insert("content".into(), Value::String(content.to_string()));
        params.insert("content_type".into(), ContentType::Text.into());
        self.engine
            .execute_operation(&EntityName::new("block"), "create", params)
            .await
            .expect("headless create_block");
    }

    /// Overwrite the first tracked org file's on-disk bytes — the negative control
    /// for `inv-org-render-fixed-point` (make disk diverge from what the SQL state
    /// renders, so the fixed-point check must `Fail`).
    pub async fn overwrite_first_org_file(&self, content: &str) {
        use holon_filesystem::FileSystem;
        let path = self.org_paths.first().expect("≥1 tracked org file");
        FileSystem::write(self.org_fs.as_ref(), path, content.as_bytes())
            .await
            .expect("overwrite org file for test");
    }

    async fn all_blocks(&self) -> Vec<Block> {
        let rows = self
            .engine
            .db_handle()
            .query(BLOCK_RAW_SNAPSHOT_SQL, std::collections::HashMap::new())
            .await
            .expect("block_raw query");
        parse_block_rows(&rows)
    }

    /// Run a read-only SQL statement against the headless engine and return the
    /// raw rows (the `SutSqlProjection` read surface — mirrors the sql_slice's
    /// `query`). Fail-loud on error.
    async fn sql_query(&self, sql: &str) -> Vec<holon_api::StorageEntity> {
        self.engine
            .db_handle()
            .query(sql, std::collections::HashMap::new())
            .await
            .unwrap_or_else(|e| panic!("HeadlessFrontendComponent sql_query failed ({sql}): {e}"))
    }

    fn cell(row: &holon_api::StorageEntity, col: &str) -> Option<String> {
        row.get(col).and_then(|v| v.as_string()).map(str::to_string)
    }

    fn sorted_fields(row: holon_api::StorageEntity) -> Vec<String> {
        let mut fields: Vec<String> = row
            .into_values()
            .map(|v| v.as_string().unwrap_or_default().to_string())
            .collect();
        fields.sort();
        fields
    }

    /// Settle the navigation matviews (`current_focus` / `focus_roots`) to a fixed
    /// point after a `navigation.focus` write, so the focus invariants read a
    /// converged projection. A query watch / op clears `is_loading` before its CDC
    /// pump delivers, so poll the row counts to a stable fixed point (mirrors the
    /// `watch_rows` settle loop). Reaching the fixed point is what makes the
    /// `focus_roots` teeth produce a real `Fail` (not a CDC-lag `Skipped`, V4).
    async fn settle_focus_matviews(&self) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        let mut last = (usize::MAX, usize::MAX);
        let mut stable = 0u32;
        loop {
            let cf = self
                .sql_query("SELECT region, block_id FROM current_focus")
                .await
                .len();
            let fr = self
                .sql_query("SELECT region, root_id FROM focus_roots")
                .await
                .len();
            if (cf, fr) == last {
                stable += 1;
                if stable >= 3 {
                    break;
                }
            } else {
                stable = 0;
            }
            last = (cf, fr);
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Wait until the LeftSidebar has bound a clickable `navigation.focus` intent
    /// for `id` before a sidebar-nav click is issued. The sidebar page list is a
    /// nested `live_block` watch that streams its rows AND their bound `selectable`
    /// intents in asynchronously after boot/seed; a click that outruns it makes the
    /// production `click_entity` fall through to an in-memory `set_focus` (which
    /// writes NO `navigation_history` row — see the doc on `ReactiveEngine::set_focus`)
    /// so the `current_focus` matview stays on the boot-default `journals`. Poll the
    /// resolved layout for the exact predicate `click_entity` dispatches on
    /// (`find_click_intent_in_region`), and fail loud if the entry never binds rather
    /// than let the click silently fake focus.
    async fn await_sidebar_nav_intent(&self, id: &EntityUri) {
        let root_uri = holon_api::root_layout_block_uri();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let resolved = self.reactive.snapshot_resolved(&root_uri);
            if holon_frontend::focus_path::find_click_intent_in_region(
                &resolved,
                id,
                "left_sidebar",
            )
            .is_some()
            {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "[SutFocusWrite::apply_navigate_focus] LeftSidebar never bound a \
                 navigation.focus click-intent for {id} within 5s — the sidebar page list \
                 (nested live_block watch) did not stream the target's selectable, so a \
                 click would fall through to an in-memory set_focus (no navigation_history \
                 write) and leave current_focus on the boot default. Sidebar-render / \
                 CDC-settle faithfulness gap, not a fake-focus escape."
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Loud postcondition for a sidebar-nav click: `current_focus(main)` must
    /// reflect `id` once CDC settles. If it does not, the click dispatched no
    /// `navigation.focus` SQL write (silent set_focus fallthrough) or the matview
    /// lagged past `settle_focus_matviews` — either way, never fake focus: fail loud.
    async fn assert_navigate_focus_landed(&self, id: &EntityUri) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            let focus = self
                .sql_query("SELECT block_id FROM current_focus WHERE region = 'main'")
                .await
                .first()
                .and_then(|r| Self::cell(r, "block_id"));
            if focus.as_deref() == Some(id.as_str()) {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "[SutFocusWrite::apply_navigate_focus] after clicking the LeftSidebar entry \
                 for {id} and settling, current_focus(main) is {focus:?} — the navigation.focus \
                 SQL write did not land (click fell through to an in-memory set_focus, or the \
                 matview lagged). Never fake focus: failing loud."
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Settle `block_id`'s `block_raw.content` to a fixed point after a keystroke
    /// edit. A char insert mutates the editor's `MutableText` (Loro) cell; the
    /// per-keystroke pipeline then syncs that through to the `block_raw` projection
    /// where `inv-blocks-match-ref/block_raw` reads. That sync is CDC-driven and
    /// can lag the synchronous keystroke return, so poll the projected content to a
    /// stable value (3 equal reads) — the content analogue of
    /// `settle_focus_matviews`. This is what gives committed-content parity with the
    /// reference's eager `commit_active_editor_if_changed`.
    async fn settle_block_content(&self, block_id: &EntityUri) {
        let escaped = block_id.as_str().replace('\'', "''");
        let sql = format!("SELECT content FROM block_raw WHERE id = '{escaped}'");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        let mut last: Option<String> = None;
        let mut stable = 0u32;
        loop {
            let now = self
                .sql_query(&sql)
                .await
                .into_iter()
                .next()
                .and_then(|r| Self::cell(&r, "content"));
            if now == last {
                stable += 1;
                if stable >= 3 {
                    break;
                }
            } else {
                stable = 0;
            }
            last = now;
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Dispatch a `navigation` provider op through the windowless `FrontendSession`
    /// (the headless analogue of `E2ESut`'s driver `synthetic_dispatch` / leader
    /// chords), then settle the focus matviews. `block_id`/`history_id` are passed
    /// only for the ops that take them (`focus_pin` / `close`); `close` ignores the
    /// region. Drives `SutNavHistoryDrive`.
    async fn dispatch_navigation(
        &self,
        op: &str,
        region: holon_api::Region,
        block_id: Option<String>,
        history_id: Option<i64>,
    ) {
        use holon_api::{EntityName, Value};
        let mut params = std::collections::HashMap::new();
        params.insert(
            "region".to_string(),
            Value::String(region.as_str().to_string()),
        );
        if let Some(block_id) = block_id {
            params.insert("block_id".to_string(), Value::String(block_id));
        }
        if let Some(history_id) = history_id {
            params.insert("history_id".to_string(), Value::Integer(history_id));
        }
        self.session
            .execute_operation(&EntityName::new("navigation"), op, params)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "[SutNavHistoryDrive::dispatch_navigation] navigation.{op}(region=\
                     {region:?}) through the headless session failed: {e:#}"
                )
            });
        self.settle_focus_matviews().await;
    }
}

#[async_trait::async_trait(?Send)]
impl SutRenderer for HeadlessFrontendComponent {
    async fn render_tree_of(&self, id: &EntityUri) -> Option<String> {
        let rqr = self.resolve_watch(id).await?;
        let (render_expr, data_rows) = rqr.snapshot();
        let services = self.services();
        let vm = holon_frontend::interpret_pure(&render_expr, &data_rows, &*services).snapshot();
        Some(vm.pretty_print(0))
    }

    async fn widget_tree_snapshot(&self) -> WidgetSnapshot {
        let empty = || WidgetSnapshot {
            kind: "empty".into(),
            entity_id: None,
            props: Default::default(),
            operations: Vec::new(),
            children: Vec::new(),
        };
        let root_uri = holon_api::root_layout_block_uri();
        if self.resolve_watch(&root_uri).await.is_none() {
            return empty();
        }
        // Resolve the FULL tree via the engine's RECURSIVE `snapshot` — NOT the
        // shallow `interpret_pure` (whose `live_block` regions stay placeholders).
        // `ReactiveEngine::snapshot` recursively resolves each `live_block` via
        // `ensure_watching`, but it stops at the first still-loading child, so a
        // single call only warms one level deep. Headlessly there is no frontend
        // event-loop populating the nested watches, so we re-snapshot after a CDC
        // settle until the resolved tree reaches a fixed point — the headless
        // analogue of the windowed slice's pump-settle. (Rich ViewModel, thin
        // frontend: the shared `shadow_builders` produce the whole tree here; the
        // only thing a real frontend adds is *waiting* for CDC, which we do too.)
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut snap = view_model_to_snapshot(&self.reactive.snapshot(&root_uri));
        let mut last = (usize::MAX, usize::MAX);
        let mut stable = 0u32;
        loop {
            let total = snap.walk().count();
            let pending = snap
                .walk()
                .filter(|n| n.kind == "loading" || n.kind == "unknown")
                .count();
            if (total, pending) == last {
                stable += 1;
                if stable >= 4 {
                    return snap;
                }
            } else {
                stable = 0;
                last = (total, pending);
            }
            if tokio::time::Instant::now() >= deadline {
                return snap;
            }
            tokio::time::sleep(Duration::from_millis(120)).await;
            snap = view_model_to_snapshot(&self.reactive.snapshot(&root_uri));
        }
    }

    async fn root_data_row_ids(&self) -> std::collections::BTreeSet<EntityUri> {
        let root_uri = holon_api::root_layout_block_uri();
        let Some(rqr) = self.resolve_watch(&root_uri).await else {
            return std::collections::BTreeSet::new();
        };
        let (_, data_rows) = rqr.snapshot();
        data_rows
            .iter()
            .filter_map(|r| {
                r.get("id")
                    .and_then(|v| v.as_string())
                    .and_then(|s| EntityUri::parse(s).ok())
            })
            .collect()
    }

    async fn widget_tree_for(&self, block_id: &EntityUri) -> Option<WidgetSnapshot> {
        let rqr = self.resolve_watch(block_id).await?;
        let (render_expr, data_rows) = rqr.snapshot();
        let services = self.services();
        let vm = holon_frontend::interpret_pure(&render_expr, &data_rows, &*services).snapshot();
        Some(view_model_to_snapshot(&vm))
    }

    async fn root_content_comparison(
        &self,
        visible_columns: &[String],
    ) -> Option<(Vec<String>, Vec<String>)> {
        let root_uri = holon_api::root_layout_block_uri();
        let rqr = self.resolve_watch(&root_uri).await?;
        let (render_expr, data_rows) = rqr.snapshot();
        let services = self.services();
        let display_tree =
            holon_frontend::interpret_pure(&render_expr, &data_rows, &*services).snapshot();
        let rendered_rows = crate::display_assertions::extract_rendered_rows(&display_tree);
        if rendered_rows.is_empty() || visible_columns.is_empty() || data_rows.is_empty() {
            return None;
        }
        let data_content: Vec<String> = data_rows
            .iter()
            .map(|r| {
                r.iter()
                    .filter(|(k, _)| visible_columns.contains(k))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<std::collections::HashMap<String, holon_api::Value>>()
            })
            .filter_map(|r| {
                r.get("content")
                    .and_then(|v| v.as_string())
                    .map(str::to_string)
            })
            .collect();
        let rendered_content: Vec<String> = rendered_rows
            .iter()
            .filter_map(|r| {
                r.get("content")
                    .and_then(|v| v.as_string())
                    .map(str::to_string)
            })
            .collect();
        Some((rendered_content, data_content))
    }

    async fn root_render_ready(&self) -> bool {
        let root_uri = holon_api::root_layout_block_uri();
        let Some(rqr) = self.resolve_watch(&root_uri).await else {
            return false;
        };
        let (render_expr, data_rows) = rqr.snapshot();
        let placeholder = matches!(
            &render_expr,
            holon_api::RenderExpr::FunctionCall { name, .. } if name == "loading" || name == "spacer"
        );
        if placeholder {
            return false;
        }
        let services = self.services();
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            holon_frontend::interpret_pure(&render_expr, &data_rows, &*services).snapshot();
        }))
        .is_ok()
    }

    async fn root_render_kind(&self) -> Option<String> {
        let root_uri = holon_api::root_layout_block_uri();
        let rqr = self.resolve_watch(&root_uri).await?;
        match rqr.snapshot().0 {
            holon_api::RenderExpr::FunctionCall { name, .. }
                if name != "loading" && name != "spacer" =>
            {
                Some(name)
            }
            _ => None,
        }
    }
}

#[async_trait::async_trait(?Send)]
impl SutQueryResults for HeadlessFrontendComponent {
    async fn root_query_row_count(&self) -> Option<usize> {
        let root_uri = holon_api::root_layout_block_uri();
        let rqr = self.resolve_watch(&root_uri).await?;
        Some(rqr.snapshot().1.len())
    }
}

#[async_trait::async_trait(?Send)]
impl SutBackend for HeadlessFrontendComponent {
    async fn live_block_snapshot(&self) -> Vec<Block> {
        // The `inv-blocks-match-ref/matview` reader reads the `block` MATVIEW, which
        // carries the `tags`/`requires` edge fields as `json_group_array` columns —
        // NOT the base `block_raw` table (`all_blocks`), which has no `tags` column and
        // would parse junction-only edge fields (e.g. a CreateDocument `Page` tag) as
        // empty and falsely diverge from the reference.
        let rows = self
            .engine
            .db_handle()
            .query(BLOCK_MATVIEW_SNAPSHOT_SQL, std::collections::HashMap::new())
            .await
            .expect("block matview query");
        parse_block_rows(&rows)
    }
    async fn block_raw_snapshot(&self) -> Vec<Block> {
        self.all_blocks().await
    }
    /// The CDC-driven focus-root mirror `inv-focus-roots` reads. Headlessly there
    /// is no separate `LiveData<FocusRoot>` mirror, so this reads the same
    /// `focus_roots` matview as [`SutSqlProjection::focus_roots_rows`]. Reading one
    /// source for both means the invariant's mirror==matview check never triggers
    /// the CDC-lag → `Skipped` downgrade — so the navigation slice's teeth produce
    /// a real `Fail` (not `Skipped`) on divergence (V4).
    async fn live_focus_root_rows(&self) -> Vec<(String, String)> {
        self.focus_roots_rows().await
    }
}

#[async_trait::async_trait(?Send)]
impl SutViewModel for HeadlessFrontendComponent {
    /// Snapshot the headless engine's rendered ViewModel tree and count `Error`
    /// widgets — the **real** `inv-viewmodel-no-error-widgets` path (faithful
    /// port of `E2ESut::headless_error_node_count`). `None` when the root isn't
    /// watchable / still loading / a placeholder / interpretation panics.
    async fn headless_error_node_count(&self) -> Option<usize> {
        let root_id = holon_api::root_layout_block_uri();
        let results = self.reactive.ensure_watching(&root_id);
        if results.is_loading() {
            return None;
        }
        let (render_expr, data_rows) = results.snapshot();
        if matches!(&render_expr, holon_api::RenderExpr::FunctionCall { name, .. } if name == "loading" || name == "spacer")
        {
            return None;
        }
        let services = self.services();
        let tree = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            holon_frontend::interpret_pure(&render_expr, &data_rows, &*services).snapshot()
        }))
        .ok()?;
        Some(holon_layout_testing::display_assertions::count_error_nodes(
            &tree,
        ))
    }

    // ─── gpui-frontend-engine-specific / unwired methods ────────────────────
    // This slice has a headless `ReactiveEngine` but no separate gpui *frontend
    // engine* (no window). The methods below describe that frontend engine or
    // drive invariants not wired into the shared catalog yet, so they return the
    // honest "not applicable / nothing to report" value (§5.1) rather than a
    // fabricated one. Only `headless_error_node_count` is exercised today.
    async fn drain_vm_emissions(&mut self) -> Vec<String> {
        Vec::new()
    }
    async fn frontend_root_is_error(&self) -> bool {
        false
    }
    async fn current_view(&self) -> String {
        self.current_view.lock().expect("current_view lock").clone()
    }
    async fn frontend_root_vm(&self) -> Option<FrontendRootVm> {
        None
    }
    async fn provider_stability_report(&self, _: ViewportHint) -> Option<ProviderStabilityReport> {
        None
    }
    async fn drain_vm_emission_toggles(&self) -> Vec<(EntityUri, String)> {
        Vec::new()
    }
    async fn live_vs_fresh_tree_diff(&self) -> Option<Vec<String>> {
        None
    }
}

/// `SutWatchRows` over the **production** reactive watch surface (E1 relocation;
/// the redesign away from E2ESut's bespoke `ui_model`). `watch_query_ids` /
/// `watch_rows` read the live `ReactiveRenderedRows` the engine's CDC pump fills;
/// the two `block_raw` truth reads (used by the invariant's CDC-lag classifier) go
/// straight to the write-side base table via the `BackendEngine`.
#[async_trait::async_trait(?Send)]
impl SutWatchRows for HeadlessFrontendComponent {
    async fn watch_query_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .watches
            .lock()
            .expect("watches lock")
            .iter()
            .map(|(qid, _)| qid.clone())
            .collect();
        ids.sort();
        ids
    }

    async fn watch_rows(&self, query_id: &str) -> Vec<WatchRow> {
        let key = self
            .watches
            .lock()
            .expect("watches lock")
            .iter()
            .find(|(qid, _)| qid == query_id)
            .map(|(_, key)| key.clone());
        let Some(key) = key else {
            return Vec::new();
        };
        // Settle to a stable row count. A query watch clears `is_loading` (it has a
        // render expr) BEFORE its spawned CDC pump task delivers the initial result
        // batch from `session.watch_query`, so a single `!is_loading` read races to
        // empty. Poll the snapshot to a fixed point instead (count unchanged for a
        // few reads) — converges fast for an empty watch (0,0,0) and a populated one
        // (…,N,N,N alike).
        let rqr = self.reactive.ensure_watching(&key);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        let mut last = usize::MAX;
        let mut stable = 0u32;
        loop {
            let count = rqr.snapshot().1.len();
            if count == last {
                stable += 1;
                if stable >= 3 {
                    break;
                }
            } else {
                stable = 0;
            }
            last = count;
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let (_expr, rows) = rqr.snapshot();
        rows.into_iter()
            .map(|row| {
                row.iter()
                    .map(|(k, v)| (k.clone(), v.as_string().map(str::to_string)))
                    .collect()
            })
            .collect()
    }

    async fn block_raw_query_ids(&self, sql: &str) -> BTreeSet<EntityUri> {
        let rows = self
            .engine
            .db_handle()
            .query(sql, std::collections::HashMap::new())
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "[inv-watch-rows-match-ref truth check] block_raw query failed\n\
                     sql: {sql}\n error: {e}"
                )
            });
        rows.into_iter()
            .filter_map(|r| {
                r.get("id")
                    .and_then(|v| v.as_string())
                    .map(|s| EntityUri::parse(s).expect("invalid entity URI in block_raw row"))
            })
            .collect()
    }

    async fn block_raw_field(&self, id: &EntityUri, field: &str) -> Option<String> {
        let escaped_id = id.as_str().replace('\'', "''");
        let sql = format!("SELECT {field} FROM block_raw WHERE id = '{escaped_id}'");
        let rows = self
            .engine
            .db_handle()
            .query(&sql, std::collections::HashMap::new())
            .await
            .expect("SutWatchRows::block_raw_field query failed");
        rows.into_iter()
            .next()
            .and_then(|r| r.get(field).and_then(|v| v.as_string()).map(str::to_string))
    }
}

/// `SutOrgRead` over the **production** org parser (E1 org block-equivalence):
/// parse the on-disk org files back into blocks via
/// `holon_orgmode::parser::parse_org_file` — the same parser
/// `TestContext::parse_org_file_blocks` runs, no `TestContext`/`FileSyncController`
/// coupling. Binds `inv-blocks-match-ref/org` (org-parsed blocks vs the ref's org
/// view).
#[async_trait::async_trait(?Send)]
impl SutOrgRead for HeadlessFrontendComponent {
    async fn org_block_snapshot(&self) -> Vec<Block> {
        use holon_filesystem::FileSystem;
        use holon_orgmode::parser::parse_org_file;

        // Boot org files PLUS any doc files created mid-run (`create_document` tracks
        // them in `documents` but not the boot-fixed `org_paths`); without the union a
        // `CreateDocument`+`BulkExternalAdd` doc's on-disk blocks are never read and
        // `/org` false-diverges (oracle has them, SUT-org misses them).
        let mut paths: Vec<PathBuf> = self.org_paths.clone();
        for (_, p) in self.documents.lock().expect("documents lock").iter() {
            if !paths.contains(p) {
                paths.push(p.clone());
            }
        }
        let mut all_blocks = Vec::new();
        for path in &paths {
            let raw = FileSystem::read_to_string(self.org_fs.as_ref(), path)
                .await
                .expect("SutOrgRead: read org file");
            let result = parse_org_file(path, &raw, &EntityUri::no_parent(), &self.org_root)
                .expect("SutOrgRead: parse org file");
            all_blocks.extend(result.blocks);
        }
        all_blocks
    }
}

/// `SutOrgRender` over the **production** render path (E1): render each tracked org
/// file from the current SQL state through the same `CacheBlockReader` (doc-scoped
/// recursive CTE ordered by `sort_key, id`) + `OrgRenderer::render_document` the
/// `FileSyncController` uses, and pair it with the on-disk bytes. Binds
/// `inv-org-render-fixed-point` (disk == rendered). Mirrors
/// `TestContext::snapshot_org_render_pairs` but over the component's own injector +
/// org_fs. The doc-block id per file is the parent the production parser reconstructs
/// from the file's persisted `:ID:` drawer (== the block_raw doc row).
#[async_trait::async_trait(?Send)]
impl SutOrgRender for HeadlessFrontendComponent {
    async fn snapshot_org_render_pairs(&self) -> Vec<(String, String, String)> {
        use holon_app::turso_seams::CacheBlockReader;
        use holon_filesystem::BlockReader;
        use holon_filesystem::FileSystem;
        use holon_orgmode::org_renderer::OrgRenderer;

        let block_cache = self
            .injector
            .resolve_async::<holon::core::queryable_cache::QueryableCache<Block>>()
            .await;
        let reader = CacheBlockReader::new(block_cache);

        // All block_raw rows by id — to resolve each file's doc (header) block.
        let header_sql = "SELECT b.id, b.parent_id, b.depth, b.sort_key, b.content, \
             b.content_type, b.source_language, b.source_name, b.properties, b.marks, \
             b.collapsed, b.completed, b.block_type, b.created_at, b.updated_at, \
             COALESCE((SELECT json_group_array(tag) FROM block_tags WHERE block_id = b.id), '[]') AS tags, \
             COALESCE((SELECT json_group_array(required_id) FROM block_requires WHERE block_id = b.id), '[]') AS requires \
             FROM block_raw b";
        let rows = self
            .engine
            .db_handle()
            .query(header_sql, std::collections::HashMap::new())
            .await
            .expect("SutOrgRender: block_raw header query failed");
        let doc_blocks: std::collections::HashMap<String, Block> = rows
            .into_iter()
            .map(|row| Block::try_from(row).expect("SutOrgRender: Block::try_from failed"))
            .map(|b| (b.id.to_string(), b))
            .collect();

        let mut out = Vec::new();
        let docs_snapshot = self.documents.lock().expect("documents lock").clone();
        for (doc_id, path) in &docs_snapshot {
            // disk-INDEPENDENT doc id (cached at boot), so a corrupted disk is
            // compared, not skipped.
            let Some(doc_block) = doc_blocks.get(doc_id.as_str()) else {
                continue;
            };
            let descendants = reader
                .get_blocks(doc_id)
                .await
                .expect("SutOrgRender: get_blocks failed");
            let rendered =
                OrgRenderer::render_document(doc_block, &descendants, path, &doc_block.id);
            let disk = FileSystem::read_to_string(self.org_fs.as_ref(), path)
                .await
                .expect("SutOrgRender: read org file");
            out.push((path.to_string_lossy().to_string(), disk, rendered));
        }
        out
    }
}

/// `SutFocusWrite` over the **production** navigation op (SutHandle decomposition
/// — NavigateFocus onto SutFocusWrite): drive `navigation.focus(region, block_id)`
/// through the windowless `FrontendSession` (the same op the GPUI/CLI sidebar
/// click dispatches), then settle CDC to the focus-matview fixed point. The
/// `NavigateFocus` transition's `apply_to_sut(&mut CapMap)` reaches this through
/// the `#[capmap_adapter]`-generated `impl SutFocusWrite for CapMap`.
#[async_trait::async_trait(?Send)]
impl SutFocusWrite for HeadlessFrontendComponent {
    // ALLOW(unused_param): region is fixed to main by the click-driven focus path below
    async fn apply_navigate_focus(&self, _region: CapRegion, id: &EntityUri) {
        // Focus is set by CLICKING the LeftSidebar entry through the production
        // `ReactiveEngineDriver` — the SAME way a real user (and E2ESut, and this cap's sibling
        // `apply_focus_editable_text` below) does it, NOT a synthesized `navigation.focus`
        // dispatch that skips the UI. The click-intent resolver dispatches the entry's bound
        // `navigation.focus(region:"main")` action (find_click_intent -> apply_intent ->
        // dispatch_intent), which mirrors focus into `engine.focused_block()` AND writes the SQL
        // nav tables — so both the headless keystone (focus read deselected) and the WINDOWED SUT
        // (window `SutDriver` reads `engine.focused_block()`) see a faithful, consistent focus.
        // The generator restricts `NavigateFocus` to `Region::Main` on sidebar-listed pages, so
        // the click always targets the `left_sidebar` entry (`_region` is the nav DESTINATION,
        // carried by the entry's bound action, not the click location).
        let id = self.resolve_id(id);
        // Do not let the click outrun the async sidebar render: wait until the
        // target's `navigation.focus` intent is actually bound, so `click_entity`
        // dispatches the nav SQL write instead of silently falling through to an
        // in-memory `set_focus`.
        self.await_sidebar_nav_intent(&id).await;
        self.driver
            .click_entity(&id, "left_sidebar")
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "[SutFocusWrite::apply_navigate_focus] sidebar click_entity(left_sidebar, \
                     {id}) failed: {e:#}"
                )
            });
        self.settle_focus_matviews().await;
        self.assert_navigate_focus_landed(&id).await;
    }

    /// Open an editor on `id` the production way: a main-panel `click_entity`
    /// through the headless `UserDriver`. For an `editable_text` block the click
    /// binds no intent, so it falls through to `engine.set_focus(id)` (ADR 0010:
    /// focus is pure in-memory state) — exactly what `FocusEditableText`'s
    /// geometry path (`apply_focus_editable_text_to_sut`) does after
    /// `wait_for_bounds`, minus the GPUI layout wait. The mounting editor's caret
    /// is seeded lazily on the first keystroke (`HeadlessEditorMirror`'s
    /// `peek_caret_seed`-or-end init), matching production GPUI.
    async fn apply_focus_editable_text(&self, id: &EntityUri) {
        let id = &self.resolve_id(id);
        self.driver.click_entity(id, "main").await.unwrap_or_else(|e| {
            panic!("[SutFocusWrite::apply_focus_editable_text] click_entity(main, {id}) failed: {e:#}")
        });
    }
}

/// The keystroke-driven editor write caps over the PRODUCTION headless editor
/// pipeline (`ReactiveEngineDriver` → `HeadlessEditorMirror` → `MutableText`),
/// the headless analogue of `E2ESut`'s `send_raw_keystroke`-based
/// `SutEditorMirrorWrite` — no GPUI window, no `InMemEditorComponent` stand-in.
/// Each `apply_*` settles the focused block's `block_raw.content` to a fixed point
/// so committed-content parity with the reference's eager
/// `commit_active_editor_if_changed` holds for `inv-blocks-match-ref/block_raw`.
#[async_trait::async_trait(?Send)]
impl SutEditorMirrorWrite for HeadlessFrontendComponent {
    async fn apply_type_chars(&self, text: &str) {
        for ch in text.chars() {
            self.driver
                .send_raw_keystroke(&ch.to_string(), &[])
                .await
                .unwrap_or_else(|e| {
                    panic!("[SutEditorMirrorWrite::apply_type_chars] send_raw_keystroke({ch:?}) failed: {e:#}")
                });
        }
        if let Some(block) = self.reactive.focused_block() {
            self.settle_block_content(&block).await;
        }
    }

    async fn apply_delete_backward(&self, count: usize) {
        for _ in 0..count {
            self.driver
                .send_raw_keystroke("backspace", &[])
                .await
                .unwrap_or_else(|e| {
                    panic!("[SutEditorMirrorWrite::apply_delete_backward] backspace failed: {e:#}")
                });
        }
        if let Some(block) = self.reactive.focused_block() {
            self.settle_block_content(&block).await;
        }
    }

    async fn apply_move_cursor(&self, byte_position: usize) {
        // Convert the byte offset to `home` + N `right` keystrokes against the
        // focused block's live editor text, exactly as `E2ESut::apply_move_cursor`
        // does (each `right` advances one char). No content settle — MoveCursor
        // doesn't write block content (mirrors the reference).
        let block = self
            .reactive
            .focused_block()
            .expect("[apply_move_cursor] no focused block — FocusEditableText must run first");
        let services: &dyn BuilderServices = self.reactive.as_ref();
        let text = services
            .editable_text(&block, "content")
            .map(|c| c.current())
            .unwrap_or_default();
        assert!(
            text.is_char_boundary(byte_position),
            "[apply_move_cursor] byte_position {byte_position} not a char boundary of {text:?}"
        );
        let right_presses = text[..byte_position].chars().count();
        self.driver
            .send_raw_keystroke("home", &[])
            .await
            .unwrap_or_else(|e| panic!("[apply_move_cursor] home failed: {e:#}"));
        for _ in 0..right_presses {
            self.driver
                .send_raw_keystroke("right", &[])
                .await
                .unwrap_or_else(|e| panic!("[apply_move_cursor] right failed: {e:#}"));
        }
    }
}

/// Editor-mirror reads: caret from the driver's `HeadlessEditorMirror` (same map
/// the keystrokes advance), live text from the block's `MutableText` cell (the
/// pre-commit value, same source `E2ESut::editor_live_text` reads).
impl SutEditorMirrorRead for HeadlessFrontendComponent {
    fn editor_caret_byte(&self, block_id: &EntityUri) -> Result<Option<usize>, String> {
        self.driver.editor_cursor_byte(block_id)
    }

    fn editor_live_text(&self, block_id: &EntityUri) -> Result<String, String> {
        let services: &dyn BuilderServices = self.reactive.as_ref();
        services
            .editable_text(block_id, "content")
            .map(|cell| cell.current())
            .map_err(|e| format!("[editor_live_text] no MutableText for {block_id}: {e:#}"))
    }
}

/// `SutNavHistoryWrite` over the **production** `navigation.go_home` op (SutHandle
/// decomposition increment 2): drive `navigation.go_home(region)` through the
/// windowless `FrontendSession` (the same op the GPUI/CLI leader-`h` chord
/// dispatches — `set_focus(None)` + close the region's open pins), then settle CDC
/// to the focus-matview fixed point. The `NavigateHome` transition's
/// `apply_to_sut(&mut CapMap)` reaches this through the `#[capmap_adapter]`-generated
/// `impl SutNavHistoryWrite for CapMap`.
#[async_trait::async_trait(?Send)]
impl SutNavHistoryWrite for HeadlessFrontendComponent {
    async fn apply_navigate_home(&self, region: CapRegion) {
        use holon_api::{EntityName, Value};
        let region_str = match region {
            CapRegion::Main | CapRegion::Single => "main",
            CapRegion::Sidebar => "left_sidebar",
        };
        let mut params = std::collections::HashMap::new();
        params.insert("region".to_string(), Value::String(region_str.to_string()));
        self.session
            .execute_operation(&EntityName::new("navigation"), "go_home", params)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "[SutNavHistoryWrite::apply_navigate_home] navigation.go_home(region=\
                     {region_str}) through the headless session failed: {e:#}"
                )
            });
        self.settle_focus_matviews().await;
    }
}

/// `SutSqlProjection` over the headless engine's matviews/base tables. The focus
/// rows (`current_focus_rows` / `focus_roots_rows` / `nav_history_open_rows`) read
/// the live navigation matviews navigation wrote to; the block rows mirror the
/// sql_slice's projection (real `block`/`block_raw` reads). Wired only into the
/// navigation slice's CapMap (not the general `register`), so existing frontend
/// slices keep their current selection.
#[async_trait::async_trait(?Send)]
impl SutSqlProjection for HeadlessFrontendComponent {
    async fn block_row(&self, id: &EntityUri) -> Option<Vec<String>> {
        let escaped = id.as_str().replace('\'', "''");
        let rows = self
            .sql_query(&format!("SELECT * FROM block WHERE id = '{escaped}'"))
            .await;
        rows.into_iter().next().map(Self::sorted_fields)
    }

    async fn all_block_ids(&self) -> BTreeSet<EntityUri> {
        self.sql_query("SELECT id FROM block_raw")
            .await
            .iter()
            .filter_map(|r| {
                Self::cell(r, "id").map(|s| {
                    EntityUri::parse(&s).expect("block id from SQL must be a valid EntityUri")
                })
            })
            .collect()
    }

    async fn sorted_children(&self, parent: &EntityUri) -> Vec<EntityUri> {
        let escaped = parent.as_str().replace('\'', "''");
        self.sql_query(&format!(
            "SELECT id FROM block_raw WHERE parent_id = '{escaped}' ORDER BY sort_key, id"
        ))
        .await
        .iter()
        .filter_map(|r| {
            Self::cell(r, "id")
                .map(|s| EntityUri::parse(&s).expect("block id from SQL must be a valid EntityUri"))
        })
        .collect()
    }

    /// No `SutSqlProjection`-tracked CDC watch-count surface here (the watch set is
    /// `SutWatchRows`' concern); honest `None`.
    async fn watch_row_count(&self, _: &str) -> Option<usize> {
        None
    }

    async fn block_raw_row(&self, id: &EntityUri) -> Option<Vec<String>> {
        let escaped = id.as_str().replace('\'', "''");
        let rows = self
            .sql_query(&format!("SELECT * FROM block_raw WHERE id = '{escaped}'"))
            .await;
        rows.into_iter().next().map(Self::sorted_fields)
    }

    async fn block_tag_block_ids(&self) -> BTreeSet<EntityUri> {
        self.sql_query("SELECT DISTINCT block_id FROM block_tags")
            .await
            .iter()
            .filter_map(|r| {
                Self::cell(r, "block_id").map(|s| {
                    EntityUri::parse(&s).expect("block_tags.block_id must be a valid EntityUri")
                })
            })
            .collect()
    }

    async fn block_task_state(&self, id: &EntityUri) -> Option<String> {
        let escaped = id.as_str().replace('\'', "''");
        let rows = self
            .sql_query(&format!(
                "SELECT json_extract(properties, '$.task_state') AS task_state \
                 FROM block_raw WHERE id = '{escaped}'"
            ))
            .await;
        rows.into_iter()
            .next()
            .and_then(|r| Self::cell(&r, "task_state"))
    }

    async fn block_content(&self, id: &EntityUri) -> Option<String> {
        let escaped = id.as_str().replace('\'', "''");
        let rows = self
            .sql_query(&format!(
                "SELECT content FROM block_raw WHERE id = '{escaped}'"
            ))
            .await;
        rows.into_iter()
            .next()
            .and_then(|r| Self::cell(&r, "content"))
    }

    async fn current_focus_rows(&self) -> Vec<(String, Option<String>)> {
        self.sql_query("SELECT region, block_id FROM current_focus")
            .await
            .iter()
            .filter_map(|r| {
                Self::cell(r, "region").map(|region| (region, Self::cell(r, "block_id")))
            })
            .collect()
    }

    async fn focus_roots_rows(&self) -> Vec<(String, String)> {
        self.sql_query("SELECT region, root_id FROM focus_roots")
            .await
            .iter()
            .filter_map(|r| {
                let region = Self::cell(r, "region")?;
                let root_id = Self::cell(r, "root_id")?;
                Some((region, root_id))
            })
            .collect()
    }

    async fn nav_history_open_rows(&self) -> Vec<(String, String)> {
        self.sql_query(
            "SELECT region, block_id FROM navigation_history \
             WHERE closed_at IS NULL AND block_id IS NOT NULL",
        )
        .await
        .iter()
        .filter_map(|r| {
            let region = Self::cell(r, "region")?;
            let block_id = Self::cell(r, "block_id")?;
            Some((region, block_id))
        })
        .collect()
    }
}

/// `SutWatchRegister` over the **production** reactive watch surface (SutHandle
/// decomposition INC 3) — the write cap the decomposed `SetupWatch` transition
/// binds. Shares the `register_watch_compiled` core with the `register_query_watch`
/// test helper, so a composed `CapMap` registers a watch through the SAME
/// `ReactiveEngine::watch_query_live` path the existing B5 teeth already prove
/// deliver headlessly. The transition compiles `TestQuery → (source, lang)` at
/// the boundary; this takes the compiled form.
#[async_trait::async_trait(?Send)]
impl SutWatchRegister for HeadlessFrontendComponent {
    async fn register_watch(&self, query_id: &str, source: &str, lang: QueryLanguage) {
        self.register_watch_compiled(query_id, source.to_string(), lang);
    }

    async fn unregister_watch(&self, query_id: &str) {
        // Drop the tracked watch entry; the production reactive watch surface
        // reclaims the underlying watcher when its last `ReactiveRenderedRows`
        // handle is released.
        self.watches
            .lock()
            .expect("watches lock")
            .retain(|(id, _)| id != query_id);
    }
}

/// `SutViewControl` (the `SwitchView` transition): set the active view name.
/// Faithful port of `E2ESut`/`TestEnvironment::switch_view` — a pure interior-mut
/// write the `SutViewModel::current_view` oracle reads back.
#[async_trait::async_trait(?Send)]
impl SutViewControl for HeadlessFrontendComponent {
    async fn switch_view(&self, view_name: &str) {
        *self.current_view.lock().expect("current_view lock") = view_name.to_string();
    }
}

/// `SutMcpEmit` (the `EmitMcpData` transition): emit the current state over the MCP
/// integration. The windowless headless stack has no `PbtMcpIntegration` attached
/// (just as `E2ESut::emit_mcp_data` is a no-op when its `pbt_mcp` slot is empty), so
/// this is a faithful no-op — no invariant observes an MCP emission on this path.
#[async_trait::async_trait(?Send)]
impl SutMcpEmit for HeadlessFrontendComponent {
    async fn emit_mcp_data(&self) {
        tracing::trace!("[apply] EmitMcpData (headless frontend slice: no MCP integration, no-op)");
    }
}

/// `SutHistoryWrite` (the `UndoLastMutation` / `Redo` transitions): undo/redo the
/// last committed mutation through the production `BackendEngine` undo stack — the
/// same `engine().undo()/redo()` path `E2ESut` drives. The `ref_state`-dependent
/// block-convergence settle lives in the harness seam (`block_tree_post_action`),
/// so the cap is a pure `&self` action here.
#[async_trait::async_trait(?Send)]
impl SutHistoryWrite for HeadlessFrontendComponent {
    async fn undo_last_mutation(&self) {
        tracing::trace!("[apply] UndoLastMutation");
        let result = self.engine.undo().await;
        assert!(result.is_ok(), "undo failed: {:?}", result.err());
        assert!(result.unwrap(), "undo returned false (nothing to undo)");
    }

    async fn redo(&self) {
        tracing::trace!("[apply] Redo");
        let result = self.engine.redo().await;
        assert!(result.is_ok(), "redo failed: {:?}", result.err());
        assert!(result.unwrap(), "redo returned false (nothing to redo)");
    }
}

/// `SutNavHistoryDrive` (the `NavigateBack`/`NavigateForward`/`PinBlock`/`UnpinBlock`
/// transitions) over the **production** navigation provider ops, dispatched through
/// the windowless `FrontendSession` — the same `execute_operation("navigation", …)`
/// path `SutFocusWrite` (focus) and `SutNavHistoryWrite` (go_home) already drive.
/// `E2ESut` reaches these ops via the GPUI driver's `synthetic_dispatch` /
/// leader-chords; headlessly there is no driver, but every op (`go_back`,
/// `go_forward`, `focus_pin`, `close`) is a `navigation` provider op
/// (`holon/src/navigation/provider.rs`), so the session dispatches them directly.
/// Note: this realizes the *drive* path (op reachable + applied); whether the
/// headless reactive engine mirrors back/forward history *semantics* into the
/// nav matviews to oracle parity is a Phase-B concern, probed separately.
#[async_trait::async_trait(?Send)]
impl SutNavHistoryDrive for HeadlessFrontendComponent {
    async fn navigate_back(&self, region: holon_api::Region) {
        self.dispatch_navigation("go_back", region, None, None)
            .await;
    }

    async fn navigate_forward(&self, region: holon_api::Region) {
        self.dispatch_navigation("go_forward", region, None, None)
            .await;
    }

    async fn pin_block(&self, region: holon_api::Region, block_id: &holon_api::EntityUri) {
        // Resolve the oracle id → SUT-real id: the production `PinBlock` generator
        // draws its target from the oracle's editable descendants, which after a
        // `SplitBlock` include the synthetic `block::split-N`. `focus_pin` of a
        // synthetic id would pin a GHOST (the matview's `focus_roots` would then hold
        // the synthetic while the resolved oracle holds the real id → divergence).
        let resolved = self.resolve_id(block_id);
        self.dispatch_navigation("focus_pin", region, Some(resolved.to_string()), None)
            .await;
    }

    async fn unpin_block(&self, history_id: i64) {
        // `close` takes only `history_id` (no region — provider handles it before
        // region extraction). Pass `Main` as a placeholder; it is ignored.
        self.dispatch_navigation("close", holon_api::Region::Main, None, Some(history_id))
            .await;
    }
}

/// `SutMutate` over the headless engine. Only `toggle_state` does real work — and
/// FAITHFULLY: it dispatches the production `block`/`cycle_task_state` op (the one
/// Cmd+Enter / the `state_toggle` widget fires) `click_count` times, computed from
/// the current `task_state` exactly like E2ESut's `apply_toggle_state_to_sut`. This
/// drives `LoroBlockOperations::cycle_task_state` → the Loro authority doc → the
/// `block_raw` projection, rather than a `set_field` shortcut that bypasses the real
/// cycle. Combined with the read-doc unification (`compose_sut` builds the Loro read
/// cap over the frontend's authority doc), `inv-task-state-storage-coherence` and
/// `inv-blocks-match-ref` stay in lockstep with `ToggleState::apply_to_ref`.
/// `apply_mutation`/`bulk_external_add` are faithful `&self` no-ops, EXACTLY as on
/// `E2ESut`: their real, `ref_state`-dependent dispatch lives in the
/// `block_tree_post_action` seam, which the composed harness does not yet rebuild — so
/// those transitions stay out of the composed alphabet (driving them would diverge),
/// while `ToggleState` drives faithfully.
#[async_trait::async_trait(?Send)]
impl SutMutate for HeadlessFrontendComponent {
    async fn toggle_state(&self, block_id: &EntityUri, new_state: CycleTarget) {
        // Real production toggle: advance the cycle by dispatching `cycle_task_state`
        // `click_count` times (each op reads the current `task_state` off the Loro
        // backend and advances by one), where `click_count` is computed from the
        // pre-mutation state — the headless analogue of clicking the `state_toggle`
        // widget that many times. We read `current` from the settled SQL projection
        // (== the Loro doc at a settled point, since SQL is a pure Loro projection).
        let id = self.resolve_id(block_id);
        let current = self.block_task_state(&id).await.unwrap_or_default();
        let click_count = cycle_click_count(&current, new_state);
        assert!(
            click_count > 0,
            "[toggle_state] click_count=0 ({current:?} == {new_state:?}) — the generator \
             should exclude no-op toggles"
        );
        let entity = "block".to_string().into();
        for _ in 0..click_count {
            let mut params: StorageEntity = HashMap::new();
            params.insert("id".into(), Value::String(id.to_string()));
            self.engine
                .execute_operation(&entity, "cycle_task_state", params)
                .await
                .unwrap_or_else(|e| panic!("toggle_state cycle_task_state({id}) failed: {e}"));
        }
    }
}

impl HeadlessFrontendComponent {
    /// Settle barrier shared by the seam-mutate methods: poll `block_raw` until its
    /// id-set is stable across two consecutive reads (the live `FileSyncController`
    /// finished re-ingesting the org write). Same shape as `simulate_restart`'s settle.
    async fn settle_block_ids_stable(&self, timeout: Duration) {
        let start = std::time::Instant::now();
        let mut prev: BTreeSet<EntityUri> =
            self.all_blocks().await.into_iter().map(|b| b.id).collect();
        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let now: BTreeSet<EntityUri> =
                self.all_blocks().await.into_iter().map(|b| b.id).collect();
            if now == prev {
                break;
            }
            prev = now;
            assert!(
                start.elapsed() < timeout,
                "[settle_block_ids_stable] block_raw id-set never stabilized after org write"
            );
        }
    }

    /// Preserve the SUT's real sibling order across a seam org rewrite.
    /// `block_raw.sort_key` is the order authority (ADR 0005), but the matview
    /// snapshot drops it and leaves every sibling tied at `sequence()==0`, so
    /// `serialize_blocks_to_org_with_doc`'s `(group, sequence, id)` sort would
    /// collapse to id-order — scrambling the on-disk order (a split-minted
    /// block's random UUID lands wherever its hex sorts), which the live
    /// `FileSyncController` re-ingest then faithfully applies to SQL AND Loro.
    /// Stamp each block's `sequence` from its per-parent `sort_key` rank so the
    /// re-serialized file reproduces the order a faithful external rewrite sees.
    async fn stamp_sequence_from_sort_key(&self, blocks: &mut [Block]) {
        use holon_orgmode::models::OrgBlockExt;
        let order_rows = self
            .sql_query("SELECT id, parent_id, sort_key FROM block_raw ORDER BY sort_key, id")
            .await;
        let mut rank_per_parent: HashMap<String, i64> = HashMap::new();
        let mut seq_by_id: HashMap<String, i64> = HashMap::new();
        for row in &order_rows {
            let id = Self::cell(row, "id").expect("block_raw row missing id");
            let parent = Self::cell(row, "parent_id").unwrap_or_default();
            let rank = rank_per_parent.entry(parent).or_insert(0);
            seq_by_id.insert(id, *rank);
            *rank += 1;
        }
        for b in blocks.iter_mut() {
            if let Some(&seq) = seq_by_id.get(b.id.as_str()) {
                b.set_sequence(seq);
            }
        }
    }
}

/// Resolve a mutation's *referenced* ids (oracle synthetic → SUT real) via `resolve`, so
/// `Mutation::apply_to` matches the live `block_raw` rows. A `Create`'s NEW id is left as-is
/// (born-equal: the org write carries it in an `:ID:` drawer and both sides agree).
fn resolve_mutation_ids(
    mutation: &Mutation,
    resolve: &dyn Fn(&EntityUri) -> EntityUri,
) -> Mutation {
    let mut m = mutation.clone();
    match &mut m {
        Mutation::Create { parent_id, .. } => *parent_id = resolve(parent_id),
        Mutation::Update { id, .. } => *id = resolve(id),
        Mutation::Delete { id, .. } => *id = resolve(id),
        Mutation::Move {
            id, new_parent_id, ..
        } => {
            *id = resolve(id);
            *new_parent_id = resolve(new_parent_id);
        }
        Mutation::RestartApp => {}
    }
    m
}

/// `SutSeamMutate` over the headless component — the real composed equivalent of the
/// `E2ESut` `block_tree_post_action` seam (which is `ref_state`-driven). Both methods
/// rewrite the seeded USER docs' org files and let the live `FileSyncController`
/// re-ingest — `documents` excludes the layout `index.org`, so a full rewrite is safe.
/// `ref_state`-free: the post-state is reconstructed from the live `block` matview snapshot
/// plus the typed transition args, not the oracle. Hosting this un-narrows `ApplyMutation`'s
/// External arm AND `BulkExternalAdd` onto the composed alphabet. (The `BulkExternalAdd`
/// Flutter-startup concurrent-watch race the `E2ESut` seam adds is NOT replicated here — it
/// is a startup-scheduler probe already gated by `phantom_loro_exists_repro`; the composed
/// catalog verifies the blocks landed every tick.)
#[async_trait::async_trait(?Send)]
impl SutSeamMutate for HeadlessFrontendComponent {
    async fn apply_mutation(&self, event: MutationEvent) {
        use holon_filesystem::FileSystem;
        let resolved = resolve_mutation_ids(&event.mutation, &|id| self.resolve_id(id));
        // Source from the `block` MATVIEW (`live_block_snapshot`), NOT the base `block_raw`
        // table (`all_blocks`): block_raw has no `tags` column, so the doc block's `Page`
        // tag is lost and `blocks_by_document` finds no page → it would serialize an EMPTY
        // org file and the live re-ingest would WIPE the whole tree. The matview carries
        // tags/requires, so page-ness (and any other edge fields) round-trip faithfully.
        let mut current = self.live_block_snapshot().await;
        // Stamp real sibling order BEFORE applying the mutation: a `Create`'s
        // canonical slot is `max_sibling_seq + 1` (lands last, matching the
        // oracle's `Mutation::apply_to`), which is only meaningful over the
        // real per-parent ranks — over the matview's all-tied `sequence()==0`
        // the whole file would collapse to id-order on rewrite (the
        // SplitBlock+External-Create sibling-order scramble).
        self.stamp_sequence_from_sort_key(&mut current).await;
        resolved.apply_to(&mut current);
        let grouped = holon_api::blocks_by_document(&current);
        let docs_snapshot = self.documents.lock().expect("documents lock").clone();
        for (doc_uri, file_path) in &docs_snapshot {
            let doc_blocks: Vec<&Block> = grouped
                .iter()
                .find(|(u, _)| u == doc_uri)
                .map(|(_, b)| b.iter().collect())
                .unwrap_or_default();
            let doc_block = current.iter().find(|b| b.id == *doc_uri && b.is_page());
            let org = crate::serialize_blocks_to_org_with_doc(&doc_blocks, doc_uri, doc_block);
            FileSystem::write(self.org_fs.as_ref(), file_path, org.as_bytes())
                .await
                .unwrap_or_else(|e| {
                    panic!("[apply_mutation/External] write {file_path:?} failed: {e:#}")
                });
        }
        self.settle_block_ids_stable(Duration::from_secs(5)).await;
    }

    async fn bulk_external_add(&self, doc_uri: &EntityUri, blocks: &[Block]) {
        use holon_filesystem::FileSystem;
        let resolved_doc = self.resolve_id(doc_uri);
        let file_path = self
            .documents
            .lock()
            .expect("documents lock")
            .iter()
            .find(|(u, _)| *u == resolved_doc)
            .map(|(_, p)| p.clone())
            .unwrap_or_else(|| {
                panic!("[bulk_external_add] no file for doc {doc_uri} (resolved {resolved_doc})")
            });
        // New blocks are born with their oracle ids (`block:bulk-N-i`) → write verbatim
        // (matched born-equal). Only resolve parent refs to pre-existing entities (the doc);
        // refs to sibling bulk-N-k stay (born too, present in `current` after this loop).
        // Matview snapshot (not `all_blocks`) so the doc block's `Page` tag survives — see
        // `apply_mutation` above for why block_raw's missing tags would wipe the tree.
        let mut current = self.live_block_snapshot().await;
        // Stamp real sibling order first (see `stamp_sequence_from_sort_key`);
        // the new bulk blocks below keep `sequence()==0` (front), matching the
        // oracle's canonical assignment.
        self.stamp_sequence_from_sort_key(&mut current).await;
        for b in blocks {
            let mut nb = b.clone();
            nb.parent_id = self.resolve_id(&nb.parent_id);
            current.push(nb);
        }
        let grouped = holon_api::blocks_by_document(&current);
        let doc_blocks: Vec<&Block> = grouped
            .iter()
            .find(|(u, _)| *u == resolved_doc)
            .map(|(_, b)| b.iter().collect())
            .unwrap_or_default();
        let doc_block = current.iter().find(|b| b.id == resolved_doc && b.is_page());
        let org = crate::serialize_blocks_to_org_with_doc(&doc_blocks, &resolved_doc, doc_block);
        FileSystem::write(self.org_fs.as_ref(), &file_path, org.as_bytes())
            .await
            .unwrap_or_else(|e| panic!("[bulk_external_add] write {file_path:?} failed: {e:#}"));
        self.settle_block_ids_stable(Duration::from_secs(5)).await;
    }
}

/// `SutAppLifecycle` over the headless component — the seam-rebuild entry point.
/// Only `create_document` is realized so far: it writes an empty org file into the
/// session's watched `org_root` (the production `FileSyncController` watcher then
/// ingests it and mints the page block in `block_raw`), the headless analogue of
/// `TestContext::create_document`. No `ref_state` is read — the synthetic→real
/// doc-uri reconcile is the composed harness's generic per-tick id reconcile (the
/// minted page is one new `block_raw` id paired 1:1 with the oracle's one new
/// synthetic `block:ref-doc-N`). The action only WAITS until that page actually
/// lands so the harness's post-apply id snapshot observes it (mirrors
/// `TestContext`'s `resolve_page_uri_by_name` poll). `start_app`/`simulate_restart`/
/// `concurrent_schema_init` are not part of any composed alphabet yet (lifecycle is
/// the deferred-boot increment) — fail loud if ever dispatched.
#[async_trait::async_trait(?Send)]
impl SutAppLifecycle for HeadlessFrontendComponent {
    async fn start_app(&self, _: EntityUri, _: bool, _: bool, _: bool, _: bool) {
        unimplemented!(
            "[SutAppLifecycle::start_app] not yet ported to HeadlessFrontendComponent — \
             lifecycle (deferred-boot) is a later seam-rebuild increment; StartApp is not in \
             any composed alphabet"
        );
    }

    async fn simulate_restart(&self) {
        use holon_filesystem::FileSystem;
        // Faithful to `E2ESut`/`TestEnvironment::simulate_restart` (which is itself a
        // file-touch, NOT a true reboot): re-trigger the production `FileSyncController`
        // watcher by touch-writing each tracked org file (append a space, settle, restore),
        // forcing a re-parse. Blocks are PRESERVED — the `:ID:` drawers persisted on disk
        // make the re-parse id-stable — so `SimulateRestart::apply_to_ref` is a no-op and
        // this only re-exercises the ingest path. The post-action block-convergence settle
        // (E2ESut's `wait_for_blocks_synced`, relocated to the seam) lives HERE in the cap
        // since the composed harness has no seam: poll `block_raw` to a stable id-set.
        for path in &self.org_paths {
            let content = FileSystem::read_to_string(self.org_fs.as_ref(), path)
                .await
                .unwrap_or_else(|e| panic!("[simulate_restart] read {path:?} failed: {e:#}"));
            FileSystem::write(self.org_fs.as_ref(), path, format!("{content} ").as_bytes())
                .await
                .unwrap_or_else(|e| panic!("[simulate_restart] touch {path:?} failed: {e:#}"));
            tokio::time::sleep(Duration::from_millis(50)).await;
            FileSystem::write(self.org_fs.as_ref(), path, content.as_bytes())
                .await
                .unwrap_or_else(|e| panic!("[simulate_restart] restore {path:?} failed: {e:#}"));
        }

        // Settle: poll until the block_raw id-set is stable across two consecutive reads.
        let ids = || async {
            self.all_blocks()
                .await
                .into_iter()
                .map(|b| b.id)
                .collect::<std::collections::BTreeSet<_>>()
        };
        let timeout = Duration::from_secs(5);
        let start = std::time::Instant::now();
        let mut prev = ids().await;
        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let now = ids().await;
            if now == prev {
                break;
            }
            prev = now;
            assert!(
                start.elapsed() < timeout,
                "[simulate_restart] block_raw id-set never stabilized after restart"
            );
        }
    }

    async fn create_document(&self, file_name: &str) {
        use holon_filesystem::FileSystem;
        let file_path = self.org_root.join(file_name);
        FileSystem::write(self.org_fs.as_ref(), &file_path, b"")
            .await
            .unwrap_or_else(|e| {
                panic!("[SutAppLifecycle::create_document] write {file_name} failed: {e:#}")
            });

        // Wait for the production `FileSyncController` watcher to ingest the new file and
        // mint the doc block in `block_raw` (the convergence `TestContext::create_document`
        // polls for via `resolve_page_uri_by_name`). The doc block's title is the file stem
        // — exactly what `CreateDocument::apply_to_ref` sets the oracle page's content to —
        // so poll the `block_raw` snapshot for a block with that title. (NB: `is_page()` is
        // false on these projected rows — page-ness is a `block_tags` Page tag, not a
        // `Block` field post-projection — so match on title alone.) Self-contained: no
        // `ref_state`, no resolver; the harness reconcile maps the minted id afterwards.
        let stem = std::path::Path::new(file_name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(file_name)
            .to_string();
        let timeout = Duration::from_secs(5);
        let start = std::time::Instant::now();
        let doc_id = loop {
            if let Some(b) = self
                .all_blocks()
                .await
                .into_iter()
                .find(|b| b.title() == stem)
            {
                break b.id;
            }
            assert!(
                start.elapsed() < timeout,
                "[SutAppLifecycle::create_document] timeout waiting for the doc block \
                 (title {stem:?}) to land in block_raw after writing {file_name}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        // Track the new doc so a later `BulkExternalAdd` / External `ApplyMutation` targeting
        // it resolves a file path. `doc_id` is the minted doc-page block (title == file stem)
        // — the SAME real id the harness reconcile maps the oracle's `block:ref-doc-N` to (the
        // single block minted by this ingest), so the seam lookup keyed on `resolve_id(...)`
        // hits. Idempotent: skip if already tracked (re-create of the same file).
        {
            let mut docs = self.documents.lock().expect("documents lock");
            if !docs.iter().any(|(u, _)| *u == doc_id) {
                docs.push((doc_id, file_path.clone()));
            }
        }
    }

    async fn concurrent_schema_init(&self) {
        unimplemented!(
            "[SutAppLifecycle::concurrent_schema_init] not yet ported — not in any composed \
             alphabet"
        );
    }

    async fn assert_epoch_flip_rejected(&self) {
        // Spec 0008 §4.2(b). This component boots a REAL windowless session over a
        // durable on-disk Turso db (`new_with_loro`, un-canonicalized `_temp`), so
        // its `.holon/consolidator` marker really exists. Loro-on iff a doc store
        // was resolved. See `run_epoch_flip_rejection_check` for the rejection logic.
        crate::test_environment::run_epoch_flip_rejection_check(
            self._temp.path(),
            self.loro_doc_store().is_some(),
        )
        .await;
    }
}

#[async_trait::async_trait(?Send)]
impl SutErrorLog for HeadlessFrontendComponent {
    /// Flutter/event publish errors logged during the initial document sync —
    /// the SAME production `FrontendSession` publish-error tracker `E2ESut` read.
    async fn app_error_count(&self) -> usize {
        self.session.startup_error_count()
    }

    /// The documents resolved at boot — context for the failure message.
    async fn app_error_context(&self) -> Vec<String> {
        self.documents
            .lock()
            .expect("documents lock")
            .iter()
            .map(|(uri, _)| uri.to_string())
            .collect()
    }
}

impl CapProvider for HeadlessFrontendComponent {
    fn register(self: Arc<Self>, caps: &mut CapMap) {
        caps.insert(self.clone() as Arc<dyn SutErrorLog>);
        caps.insert(self.clone() as Arc<dyn SutRenderer>);
        caps.insert(self.clone() as Arc<dyn SutViewModel>);
        caps.insert(self.clone() as Arc<dyn SutBackend>);
        caps.insert(self.clone() as Arc<dyn SutWatchRows>);
        caps.insert(self.clone() as Arc<dyn SutOrgRead>);
        // `SutFocusWrite` is a write cap — no invariant `Needs` it, so registering
        // it here is selection-neutral; it lets the `NavigateFocus` transition
        // drive this component through `apply_to_sut(&mut CapMap)`. `SutSqlProjection`
        // is deliberately NOT registered here (it would newly select
        // `block_content_sql`); the navigation slice adds it on its own CapMap.
        caps.insert(self.clone() as Arc<dyn SutFocusWrite>);
        // `SutNavHistoryWrite` (go_home) — same selection-neutral rationale as
        // `SutFocusWrite`: no invariant `Needs` it, it just lets the `NavigateHome`
        // transition drive this component through `apply_to_sut(&mut CapMap)`.
        caps.insert(self.clone() as Arc<dyn SutNavHistoryWrite>);
        // `SutWatchRegister` (setup_watch) — same selection-neutral rationale: no
        // invariant `Needs` a write cap; it lets the `SetupWatch` transition drive
        // this component's production reactive watch surface through
        // `apply_to_sut(&mut CapMap)` (SutHandle decomposition INC 3). The watch
        // *read* cap (`SutWatchRows`) is already registered above, so a slice that
        // also supplies `RefWatches` makes the B5 watch invariants bite over a
        // composed-driven watch.
        caps.insert(self.clone() as Arc<dyn SutWatchRegister>);
        // Structural block-tree writes through the production op dispatcher (the
        // session is built via full DI, so `SqlBlockOperations` is registered).
        // Reuses the single-sourced `OpDispatchWriter` — no per-component forwarding.
        // Selection-neutral (no invariant `Needs` a write cap); lets the structural
        // transitions drive this component through `apply_to_sut(&mut CapMap)`.
        caps.insert(Arc::new(crate::pbt::op_write_cap::OpDispatchWriter::new(
            self.engine.clone(),
        )) as Arc<dyn SutBlockTreeWrite>);
        // A1 drive caps (E3 provider-gap port): `SutViewControl` (SwitchView),
        // `SutMcpEmit` (EmitMcpData), `SutHistoryWrite` (Undo/Redo). All
        // selection-neutral write caps — no invariant `Needs` them; they let the
        // corresponding transitions drive this component through
        // `apply_to_sut(&mut CapMap)` on the path to retiring `E2ESut` as the SUT.
        caps.insert(self.clone() as Arc<dyn SutViewControl>);
        caps.insert(self.clone() as Arc<dyn SutMcpEmit>);
        caps.insert(self.clone() as Arc<dyn SutHistoryWrite>);
        // A2 nav-history drive cap (NavigateBack/Forward, PinBlock, UnpinBlock)
        // over the production navigation provider ops via the headless session —
        // same selection-neutral rationale (no invariant `Needs` it).
        caps.insert(self.clone() as Arc<dyn SutNavHistoryDrive>);
        // `SutMutate` (ToggleState) — selection-neutral write cap (no invariant
        // `Needs` it); lets the `ToggleState` transition drive this component's
        // headless `set_field task_state` op through `apply_to_sut(&mut CapMap)`.
        caps.insert(self.clone() as Arc<dyn SutMutate>);
        // `SutSeamMutate` over the live `FileSyncController`: the real composed home for
        // `ApplyMutation`'s External (org) arm and `BulkExternalAdd`, un-narrowing both onto
        // any frontend CapMap. A write cap (no invariant `Needs` it), safe in `register`.
        caps.insert(self.clone() as Arc<dyn SutSeamMutate>);
        // `SutEditorMirrorWrite` (TypeChars/DeleteBackward/MoveCursor) over the
        // production headless editor pipeline — selection-neutral write cap (no
        // invariant `Needs` a write cap), so it's safe in the general `register`;
        // it lets the editor transitions drive this component through
        // `apply_to_sut(&mut CapMap)`. The READ cap (`SutEditorMirrorRead`) is
        // deliberately NOT registered here — it pairs with `RefEditorMirror` to
        // select the `inv-editor-{caret,text}-matches-ref` invariants, which the
        // navigation/structural slices don't drive an editor for; the wide PBT (and
        // any editor-driving CapMap) adds it explicitly. Same pattern as
        // `SutSqlProjection` above.
        caps.insert(self.clone() as Arc<dyn SutEditorMirrorWrite>);
        // `SutAppLifecycle` (CreateDocument) — selection-neutral lifecycle cap (no
        // invariant `Needs` it); lets the `CreateDocument` transition mint a doc through
        // `apply_to_sut(&mut CapMap)`. Only `create_document` is realized; the other
        // lifecycle methods fail loud (not in any composed alphabet yet). This is the
        // seam-rebuild entry point — the synthetic→real doc-uri mapping is the harness's
        // generic per-tick reconcile, not E2ESut's `block_tree_post_action`.
        caps.insert(self.clone() as Arc<dyn SutAppLifecycle>);
        caps.insert(self as Arc<dyn SutOrgRender>);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use holon_api::{EntityName, Value};

    /// A0 make-or-break PROBE (full-mode peer mesh): when the headless frontend
    /// boots with Loro ON (the `full_headless` shape), does its DI injector
    /// eventually resolve a `LoroSyncControllerHandle`, and is the controller
    /// watching the SAME global doc the builder's Loro arm reads? Resolution is
    /// RACE-prone (`without_wait()` → spawned `post_ready_work`, `wiring.rs:360`),
    /// so this POLLS for readiness rather than snapshotting — a flaky one-shot
    /// `assert Ok` would be the wrong probe. If the handle never resolves headless,
    /// full-mode peer projection has no controller and Part A must fall back to an
    /// explicit `project()` drive (don't fake it — surface the absence).
    #[tokio::test(flavor = "multi_thread")]
    async fn headless_loro_sync_controller_resolves_after_boot() {
        const TREE_ORG: &str = "#+ID: structural-page\n\
            * parent\n:PROPERTIES:\n:ID: parent\n:END:\n\
            * c1\n:PROPERTIES:\n:ID: c1\n:END:\n";
        let comp = HeadlessFrontendComponent::new_with_loro(
            &[("structural-page.org", TREE_ORG)],
            Duration::from_millis(300),
            true, // Loro ON — full_headless shape
        )
        .await;

        // Readiness wait: the spawned start task first awaits `ready_signal`, then
        // resolves the handle (`wiring.rs:350-362`). Poll up to ~2s.
        let mut handle = None;
        for _ in 0..40 {
            if let Some(h) = comp.loro_sync_handle() {
                handle = Some(h);
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(
            handle.is_some(),
            "[A0-probe] LoroSyncControllerHandle never resolved headless within 2s — \
             full-mode peer projection has no controller; fall back to explicit project()"
        );
        eprintln!("[A0-probe] sync controller handle resolved headless ✓");

        // The Loro authority store the controller watches must be the cached/shared
        // global doc (so a peer import into it wakes the controller). Two reads of
        // `get_global_doc()` must return the SAME doc (not a fresh one each call).
        let store = comp
            .loro_doc_store()
            .expect("[A0-probe] loro_doc_store present when Loro on");
        let doc_a = store
            .get_global_doc()
            .await
            .expect("[A0-probe] global doc #1");
        let doc_b = store
            .get_global_doc()
            .await
            .expect("[A0-probe] global doc #2");
        assert!(
            Arc::ptr_eq(&doc_a, &doc_b),
            "[A0-probe] get_global_doc() must return the cached doc (same Arc), else a \
             peer import would not wake the controller"
        );
        eprintln!("[A0-probe] global doc is cached/shared ✓");
    }

    /// Step 0 make-or-break PROBE (SutHandle decomposition / NavigateFocus): does
    /// driving the production `navigation.focus` op through the **windowless**
    /// `FrontendSession` actually update the `current_focus` / `focus_roots`
    /// matviews — with no GPUI window, no driver, no geometry pump? If yes, the
    /// `NavigateFocus` transition can rebind onto `SutFocusWrite` realized on this
    /// component. If the headless session has no operation engine, or the matviews
    /// need a window/driver pump, this STOPS the increment (don't fake the read,
    /// don't swallow the `execute_operation` error).
    #[tokio::test(flavor = "multi_thread")]
    async fn headless_navigate_focus_updates_matview() {
        // Two pinned-id page docs (`#+ID: <bare-id>`): production's parser adds the
        // `block:` scheme at the boundary, so the doc blocks land at the exact ids
        // the reference would mint — no doc-id remapping for this slice.
        let doc0 = "#+ID: ref-doc-0\n* Doc zero heading\n";
        let doc1 = "#+ID: ref-doc-1\n* Doc one heading\n";
        let comp = HeadlessFrontendComponent::new(
            &[("doc0.org", doc0), ("doc1.org", doc1)],
            Duration::from_millis(300),
        )
        .await;

        // Discover the actual doc-block id for doc1 from `block_raw` (robust to the
        // scheme the parser assigns) — this is the id we navigate focus to.
        let rows = comp
            .engine
            .db_handle()
            .query("SELECT id FROM block_raw", std::collections::HashMap::new())
            .await
            .expect("[nav-probe] block_raw query");
        let all_ids: Vec<String> = rows
            .iter()
            .filter_map(|r| r.get("id").and_then(|v| v.as_string()).map(str::to_string))
            .collect();
        eprintln!("[nav-probe] block_raw ids: {all_ids:?}");
        let target = all_ids
            .iter()
            .find(|id| id.contains("ref-doc-1"))
            .unwrap_or_else(|| {
                panic!(
                    "[nav-probe] no doc block carrying 'ref-doc-1' in block_raw; ids={all_ids:?}"
                )
            })
            .clone();

        // Drive the REAL navigation op through the windowless session. This is
        // fallible (`require_operation_engine`): assert the `Ok` explicitly — an
        // operation-engine-less session firing here IS the make-or-break.
        let mut params = std::collections::HashMap::new();
        params.insert("region".to_string(), Value::String("main".to_string()));
        params.insert("block_id".to_string(), Value::String(target.clone()));
        let result = comp
            .session
            .execute_operation(&EntityName::new("navigation"), "focus", params)
            .await;
        assert!(
            result.is_ok(),
            "[nav-probe] navigation.focus through the headless session failed — \
             the windowless session has no operation engine (make-or-break): {:?}",
            result.err()
        );

        // Settle CDC so the matview projection lands.
        tokio::time::sleep(Duration::from_millis(500)).await;

        let focus_rows = comp
            .engine
            .db_handle()
            .query(
                "SELECT region, block_id FROM current_focus WHERE region = 'main'",
                std::collections::HashMap::new(),
            )
            .await
            .expect("[nav-probe] current_focus query (matview must exist headlessly)");
        let focused = focus_rows
            .first()
            .and_then(|r| r.get("block_id"))
            .and_then(|v| v.as_string())
            .map(str::to_string);
        eprintln!("[nav-probe] current_focus(main) = {focused:?}");
        assert_eq!(
            focused.as_deref(),
            Some(target.as_str()),
            "[nav-probe] headless navigation.focus must move current_focus(main) to {target} — \
             matview did not update without a window"
        );

        let root_rows = comp
            .engine
            .db_handle()
            .query(
                "SELECT region, root_id FROM focus_roots WHERE region = 'main'",
                std::collections::HashMap::new(),
            )
            .await
            .expect("[nav-probe] focus_roots query (matview must exist headlessly)");
        eprintln!("[nav-probe] focus_roots(main) rows = {}", root_rows.len());
        assert!(
            !root_rows.is_empty(),
            "[nav-probe] focus_roots(main) must be non-empty after navigating focus to a block"
        );
    }

    /// Editor-keystone make-or-break PROBE (`SutEditorMirrorWrite` over the
    /// production `HeadlessEditorMirror`): does opening an editor headlessly
    /// (`apply_focus_editable_text` = `click_entity`) + typing a char
    /// (`apply_type_chars` = `send_raw_keystroke`) actually land the typed text in
    /// `block_raw.content` — the projection `inv-blocks-match-ref/block_raw` reads?
    /// The reference commits typed text into block content on every TypeChars
    /// (`commit_active_editor_if_changed`), so committed-content parity requires the
    /// SUT's MutableText edit to sync through to `block_raw`. If the headless
    /// pipeline never propagates the edit (no automatic Loro→Turso sync without a
    /// window), this STOPS the keystone — don't fake the commit, surface it.
    #[tokio::test(flavor = "multi_thread")]
    async fn headless_type_chars_commits_to_block_raw() {
        // The wide PBT's working tree: a seed page with three text-leaf children.
        const TREE_ORG: &str = "#+ID: structural-page\n\
            * parent\n:PROPERTIES:\n:ID: parent\n:END:\n\
            * c1\n:PROPERTIES:\n:ID: c1\n:END:\n\
            * c2\n:PROPERTIES:\n:ID: c2\n:END:\n";
        let comp = HeadlessFrontendComponent::new_with_loro(
            &[("structural-page.org", TREE_ORG)],
            Duration::from_millis(300),
            true,
        )
        .await;

        // Focus the page root so its children render in the main panel (so
        // `click_entity` can resolve the leaf there).
        let page = EntityUri::block("structural-page");
        comp.apply_navigate_focus(CapRegion::Main, &page).await;

        let c1 = EntityUri::block("c1");
        let c1_sql = format!(
            "SELECT content FROM block_raw WHERE id = '{}'",
            c1.as_str().replace('\'', "''")
        );
        let before = comp
            .sql_query(&c1_sql)
            .await
            .into_iter()
            .next()
            .and_then(|r| HeadlessFrontendComponent::cell(&r, "content"));
        eprintln!("[editor-probe] c1 content before = {before:?}");
        assert_eq!(
            before.as_deref(),
            Some("c1"),
            "[editor-probe] seed content for c1 must be the heading text"
        );

        // Open an editor on c1 (production click → focus), then type one char.
        comp.apply_focus_editable_text(&c1).await;
        assert_eq!(
            comp.reactive.focused_block().as_ref(),
            Some(&c1),
            "[editor-probe] apply_focus_editable_text must focus c1"
        );
        comp.apply_type_chars("X").await;

        let after = comp
            .sql_query(&c1_sql)
            .await
            .into_iter()
            .next()
            .and_then(|r| HeadlessFrontendComponent::cell(&r, "content"));
        eprintln!("[editor-probe] c1 content after typing 'X' = {after:?}");
        assert_eq!(
            after.as_deref(),
            Some("c1X"),
            "[editor-probe] typing 'X' at end-of-text must commit 'c1X' to block_raw.content — \
             the headless editor edit did not sync to the block projection the invariant reads"
        );
    }

    /// A2 make-or-break PROBE (`SutNavHistoryDrive`): can the **windowless**
    /// `FrontendSession` drive the nav-history ops (`focus_pin`, `go_back`,
    /// `go_forward`) the way `E2ESut` drives them through the GPUI driver's
    /// `synthetic_dispatch` / leader chords? The memory flagged back/forward as
    /// historically "driver-realized only — the headless slice does not drive
    /// these". This asserts: `focus_pin` reaches the matviews (observable effect),
    /// and `go_back`/`go_forward` dispatch headlessly without error (reachability —
    /// their full history *semantics* are a Phase-B oracle-parity concern). If the
    /// session has no operation engine for these ops, this STOPS A2 (don't fake it).
    #[tokio::test(flavor = "multi_thread")]
    async fn headless_nav_history_ops_dispatch() {
        let doc0 = "#+ID: ref-doc-0\n* Doc zero heading\n";
        let doc1 = "#+ID: ref-doc-1\n* Doc one heading\n";
        let comp = HeadlessFrontendComponent::new(
            &[("doc0.org", doc0), ("doc1.org", doc1)],
            Duration::from_millis(300),
        )
        .await;

        let rows = comp
            .engine
            .db_handle()
            .query("SELECT id FROM block_raw", std::collections::HashMap::new())
            .await
            .expect("[nav-history-probe] block_raw query");
        let target_id = rows
            .iter()
            .filter_map(|r| r.get("id").and_then(|v| v.as_string()).map(str::to_string))
            .find(|id| id.contains("ref-doc-1"))
            .expect("[nav-history-probe] no doc block carrying 'ref-doc-1' in block_raw");
        let target = EntityUri::parse(&target_id).expect("[nav-history-probe] target id parses");

        // `focus_pin` (shift+click in production) — reachable headlessly with an
        // observable matview effect (it focuses + pins the block).
        SutNavHistoryDrive::pin_block(&comp, holon_api::Region::Main, &target).await;
        let root_rows = comp
            .engine
            .db_handle()
            .query(
                "SELECT region, root_id FROM focus_roots WHERE region = 'main'",
                std::collections::HashMap::new(),
            )
            .await
            .expect("[nav-history-probe] focus_roots query");
        eprintln!(
            "[nav-history-probe] focus_roots(main) rows = {}",
            root_rows.len()
        );
        assert!(
            !root_rows.is_empty(),
            "[nav-history-probe] focus_pin through the headless session must populate \
             focus_roots(main) — the nav matview did not update without a window"
        );

        // `go_back` / `go_forward` — the historically-doubted ops. Assert they
        // dispatch headlessly without error (the cap `unwrap`s the op result, so a
        // failure panics here). History *semantics* parity is deferred to Phase B.
        SutNavHistoryDrive::navigate_back(&comp, holon_api::Region::Main).await;
        SutNavHistoryDrive::navigate_forward(&comp, holon_api::Region::Main).await;
        eprintln!("[nav-history-probe] go_back / go_forward dispatched headlessly without error");
    }

    /// C1 PinBlock make-or-break PROBE (diagnostic — prints, asserts only the
    /// dispatch). Two unknowns gate adding `PinBlock` to the composed nav alphabet:
    /// (a) does the seed contain a pinnable `ContentType::Text`, non-page block under
    /// Main? (b) does headless `focus_pin(RightSidebar, block)` populate
    /// `focus_roots(right_sidebar)` (which inv-focus-roots reads) with NO window?
    /// Uses an enriched seed (a paragraph under a heading = a Text child block).
    #[tokio::test(flavor = "multi_thread")]
    async fn headless_pin_block_right_sidebar_probe() {
        let doc0 = "#+ID: ref-doc-0\n* Heading zero\n:PROPERTIES:\n:ID: ref-block-0\n:END:\nFirst pinnable paragraph\n";
        let comp =
            HeadlessFrontendComponent::new(&[("doc0.org", doc0)], Duration::from_millis(300)).await;

        // Dump block_raw with content_type + parent so we can see what is pinnable.
        let rows = comp
            .engine
            .db_handle()
            .query(
                "SELECT id, parent_id, content_type, content FROM block_raw",
                std::collections::HashMap::new(),
            )
            .await
            .expect("[pin-probe] block_raw query");
        eprintln!("[pin-probe] block_raw has {} rows:", rows.len());
        for r in &rows {
            let id = r.get("id").and_then(|v| v.as_string()).unwrap_or("?");
            let parent = r
                .get("parent_id")
                .and_then(|v| v.as_string())
                .unwrap_or("?");
            let ct = r
                .get("content_type")
                .and_then(|v| v.as_string())
                .unwrap_or("?");
            let content = r.get("content").and_then(|v| v.as_string()).unwrap_or("");
            eprintln!(
                "[pin-probe]   id={id} parent={parent} content_type={ct} content={content:?}"
            );
        }

        // Pick a Text, non-doc block (content_type text + has a parent that isn't no_parent).
        let pinnable: Option<String> = rows
            .iter()
            .filter(|r| {
                r.get("content_type")
                    .and_then(|v| v.as_string())
                    .map(|ct| ct.eq_ignore_ascii_case("text"))
                    .unwrap_or(false)
            })
            .filter_map(|r| r.get("id").and_then(|v| v.as_string()).map(str::to_string))
            .find(|id| !id.contains("journals") && !id.contains("ref-doc-0"));
        eprintln!("[pin-probe] chosen pinnable id = {pinnable:?}");

        let pin_id = pinnable.expect(
            "[pin-probe] no pinnable Text block in the enriched seed — the `:ID:` content block \
             did not parse as a Text descendant of Main (PinBlock would hit NoPinCandidates)",
        );
        assert_eq!(
            pin_id, "block:ref-block-0",
            "[pin-probe] the `:PROPERTIES: :ID: ref-block-0` drawer must give the content block a \
             stable id (so the nav slice can name it by constant), got {pin_id}"
        );
        let pin_uri = EntityUri::parse(&pin_id).expect("[pin-probe] pin id parses");

        // Dispatch focus_pin into the RIGHT sidebar (PinBlock's region) and assert it
        // populates `focus_roots(right_sidebar)` headlessly — the make-or-break:
        // inv-focus-roots reads this matview, and without a window it might never update.
        SutNavHistoryDrive::pin_block(&comp, holon_api::Region::RightSidebar, &pin_uri).await;

        let mut params = std::collections::HashMap::new();
        params.insert("r".to_string(), Value::String("right_sidebar".to_string()));
        let fr = comp
            .engine
            .db_handle()
            .query(
                "SELECT region, root_id FROM focus_roots WHERE region = $r",
                params,
            )
            .await
            .expect("[pin-probe] focus_roots query");
        let roots: Vec<String> = fr
            .iter()
            .filter_map(|r| {
                r.get("root_id")
                    .and_then(|v| v.as_string())
                    .map(str::to_string)
            })
            .collect();
        eprintln!("[pin-probe] focus_roots(right_sidebar) = {roots:?}");
        assert!(
            roots.contains(&pin_id),
            "[pin-probe] headless focus_pin(right_sidebar, {pin_id}) must populate \
             focus_roots(right_sidebar) — the matview did not update without a window; got {roots:?}"
        );
    }

    /// Read `current_focus(main)`'s block_id from the matview (None when empty).
    async fn current_focus_main(comp: &HeadlessFrontendComponent) -> Option<String> {
        let rows = comp
            .engine
            .db_handle()
            .query(
                "SELECT block_id FROM current_focus WHERE region = 'main'",
                std::collections::HashMap::new(),
            )
            .await
            .expect("[back-fwd-probe] current_focus query");
        rows.first()
            .and_then(|r| r.get("block_id"))
            .and_then(|v| v.as_string())
            .map(str::to_string)
    }

    /// C1 back/forward make-or-break PROBE: the historically-doubted question —
    /// does headless `go_back`/`go_forward` move the `current_focus(main)` matview
    /// the way the ref's `navigation_history.cursor` moves (so inv-navigation-focus
    /// would stay green), with NO window? Build history journals(boot)→d0→d1, then
    /// `go_back` must return focus to d0 and `go_forward` back to d1. If focus does
    /// NOT track the cursor, NavigateBack/Forward must NOT join the composed alphabet
    /// (they stay E4/windowed) — this probe is the gate.
    #[tokio::test(flavor = "multi_thread")]
    async fn headless_back_forward_focus_parity_probe() {
        let doc0 = "#+ID: ref-doc-0\n* Doc zero\n";
        let doc1 = "#+ID: ref-doc-1\n* Doc one\n";
        let comp = HeadlessFrontendComponent::new(
            &[("doc0.org", doc0), ("doc1.org", doc1)],
            Duration::from_millis(300),
        )
        .await;
        let d0 = EntityUri::parse("block:ref-doc-0").expect("d0");
        let d1 = EntityUri::parse("block:ref-doc-1").expect("d1");

        // Build nav history: boot focus (journals) → d0 → d1.
        SutFocusWrite::apply_navigate_focus(&comp, CapRegion::Main, &d0).await;
        SutFocusWrite::apply_navigate_focus(&comp, CapRegion::Main, &d1).await;
        assert_eq!(
            current_focus_main(&comp).await.as_deref(),
            Some("block:ref-doc-1"),
            "[back-fwd-probe] precondition: after focusing d0 then d1, current_focus(main)=d1"
        );

        SutNavHistoryDrive::navigate_back(&comp, holon_api::Region::Main).await;
        let after_back = current_focus_main(&comp).await;
        eprintln!("[back-fwd-probe] current_focus(main) after go_back = {after_back:?}");

        SutNavHistoryDrive::navigate_forward(&comp, holon_api::Region::Main).await;
        let after_fwd = current_focus_main(&comp).await;
        eprintln!("[back-fwd-probe] current_focus(main) after go_forward = {after_fwd:?}");

        // The verdict. If these fail, headless back/forward do NOT mirror history
        // semantics → keep NavigateBack/Forward out of the composed alphabet (E4).
        assert_eq!(
            after_back.as_deref(),
            Some("block:ref-doc-0"),
            "[back-fwd-probe] go_back must move current_focus(main) to the previous block (d0)"
        );
        assert_eq!(
            after_fwd.as_deref(),
            Some("block:ref-doc-1"),
            "[back-fwd-probe] go_forward must return current_focus(main) to d1"
        );
    }

    /// C1 UnpinBlock make-or-break PROBE: (a) what `history_id` does the headless SUT
    /// assign to a right-sidebar pin (the `navigation_history.id` AUTOINCREMENT), and
    /// (b) does `close(history_id)` actually remove the pin (clear
    /// `focus_roots(right_sidebar)`) headlessly? `UnpinBlock`'s generator draws the
    /// `history_id` from the ref's `open_pins` — so the ref's predicted id must equal
    /// the SUT's real row id (the "risk C" alignment). This probe establishes the SUT
    /// side: pin, read the assigned id, unpin it, confirm the pin is gone.
    #[tokio::test(flavor = "multi_thread")]
    async fn headless_unpin_block_probe() {
        let doc0 = "#+ID: ref-doc-0\n* Heading zero\n:PROPERTIES:\n:ID: ref-block-0\n:END:\nFirst pinnable paragraph\n";
        let comp =
            HeadlessFrontendComponent::new(&[("doc0.org", doc0)], Duration::from_millis(300)).await;
        let pin_uri = EntityUri::parse("block:ref-block-0").expect("pin id");

        SutNavHistoryDrive::pin_block(&comp, holon_api::Region::RightSidebar, &pin_uri).await;

        // Dump navigation_history to find the pin row's `id` (the SUT-assigned history_id).
        let rows = comp
            .engine
            .db_handle()
            .query(
                "SELECT id, region, block_id FROM navigation_history",
                std::collections::HashMap::new(),
            )
            .await
            .expect("[unpin-probe] navigation_history query");
        eprintln!("[unpin-probe] navigation_history has {} rows:", rows.len());
        for r in &rows {
            let id = r.get("id").map(|v| format!("{v:?}")).unwrap_or_default();
            let region = r.get("region").and_then(|v| v.as_string()).unwrap_or("?");
            let block = r.get("block_id").and_then(|v| v.as_string()).unwrap_or("?");
            eprintln!("[unpin-probe]   id={id} region={region} block_id={block}");
        }
        // The right-sidebar pin row's id.
        let pin_hid: i64 = rows
            .iter()
            .find(|r| {
                r.get("block_id")
                    .and_then(|v| v.as_string())
                    .map(|b| b == "block:ref-block-0")
                    .unwrap_or(false)
                    && r.get("region")
                        .and_then(|v| v.as_string())
                        .map(|reg| reg.contains("right"))
                        .unwrap_or(false)
            })
            .and_then(|r| r.get("id"))
            .and_then(|v| v.as_i64())
            .expect("[unpin-probe] no right-sidebar pin row for ref-block-0 in navigation_history");
        eprintln!("[unpin-probe] SUT-assigned pin history_id = {pin_hid}");

        // Unpin it via the cap (close(history_id)) and confirm focus_roots clears.
        SutNavHistoryDrive::unpin_block(&comp, pin_hid).await;

        let mut params = std::collections::HashMap::new();
        params.insert("r".to_string(), Value::String("right_sidebar".to_string()));
        let fr = comp
            .engine
            .db_handle()
            .query(
                "SELECT region, root_id FROM focus_roots WHERE region = $r",
                params,
            )
            .await
            .expect("[unpin-probe] focus_roots query");
        let roots: Vec<String> = fr
            .iter()
            .filter_map(|r| {
                r.get("root_id")
                    .and_then(|v| v.as_string())
                    .map(str::to_string)
            })
            .collect();
        eprintln!("[unpin-probe] focus_roots(right_sidebar) after unpin = {roots:?}");
        assert!(
            !roots.contains(&"block:ref-block-0".to_string()),
            "[unpin-probe] close(history_id={pin_hid}) must remove the pin from \
             focus_roots(right_sidebar); still present: {roots:?}"
        );
    }

    /// PROBE — is headless `go_home` IDEMPOTENT when already home? The
    /// `NavigateHome → NavigateBack` divergence hinges on this: the ref's
    /// `navigate_home::apply_to_ref` pushes a `None` entry on EVERY call (no
    /// already-home guard), so `NavigateHome`×N → N home entries. If the headless SUT
    /// (production `navigation.focus(None)`) instead writes NO new row when already
    /// home (like `navigate_focus`'s same-target idempotency), then the ref over-counts
    /// and `NavigateBack` walks it through phantom home entries the SUT lacks — a
    /// ref-model bug. Drive `go_home`×3 from the boot (journals) state and count the
    /// NULL (home) rows in `navigation_history`.
    #[tokio::test(flavor = "multi_thread")]
    async fn headless_go_home_idempotency_probe() {
        let comp = HeadlessFrontendComponent::new(
            &[("doc0.org", "#+ID: ref-doc-0\n* Doc zero\n")],
            Duration::from_millis(300),
        )
        .await;

        for _ in 0..3 {
            SutNavHistoryWrite::apply_navigate_home(&comp, CapRegion::Main).await;
        }

        let rows = comp
            .engine
            .db_handle()
            .query(
                "SELECT id, region, block_id FROM navigation_history WHERE region = 'main'",
                std::collections::HashMap::new(),
            )
            .await
            .expect("[gohome-probe] navigation_history query");
        let home_rows = rows
            .iter()
            .filter(|r| {
                r.get("block_id")
                    .map(|v| v.as_string().is_none())
                    .unwrap_or(true)
            })
            .count();
        for r in &rows {
            let id = r.get("id").map(|v| format!("{v:?}")).unwrap_or_default();
            let block = r
                .get("block_id")
                .and_then(|v| v.as_string())
                .unwrap_or("<NULL/home>");
            eprintln!("[gohome-probe]   id={id} block_id={block}");
        }
        eprintln!(
            "[gohome-probe] go_home×3 → {home_rows} home(NULL) row(s) in navigation_history(main)"
        );
        assert_eq!(
            home_rows, 1,
            "[gohome-probe] headless go_home must be IDEMPOTENT when already home (1 NULL row \
             after 3 calls). If this is 3, the SUT also accumulates and the ref is NOT the bug — \
             revisit. Got {home_rows}."
        );
    }

    /// PROBE (nav-history fold into the wide PBT): two questions, answered empirically.
    /// (1) What `navigation_history.id`s does the WIDE boot assign (boot focus + the
    /// driven `NavigateFocus(page_root)`)? These set the exact `next_history_id` /
    /// `open_pins.history_id` constants the wide oracle must mirror to fold Pin/Unpin.
    /// (2) Do `FocusEditableText` / `create_document` — already in the wide alphabet —
    /// write SUT `navigation_history` rows the oracle wouldn't mirror? If so, they
    /// silently advance the AUTOINCREMENT and would desync Pin/Unpin id alignment +
    /// Back/Forward stack depth. The counts after each step decide the fold scope.
    #[tokio::test(flavor = "multi_thread")]
    async fn wide_boot_navigation_history_id_probe() {
        const TREE_ORG: &str = "#+ID: structural-page\n\
            * parent\n:PROPERTIES:\n:ID: parent\n:END:\n\
            * c1\n:PROPERTIES:\n:ID: c1\n:END:\n\
            * c2\n:PROPERTIES:\n:ID: c2\n:END:\n";
        let comp = HeadlessFrontendComponent::new_with_loro(
            &[("structural-page.org", TREE_ORG)],
            Duration::from_millis(300),
            true,
        )
        .await;

        async fn nav_rows(comp: &HeadlessFrontendComponent) -> Vec<(i64, String, String)> {
            let rows = comp
                .engine
                .db_handle()
                .query(
                    "SELECT id, region, block_id FROM navigation_history ORDER BY id",
                    std::collections::HashMap::new(),
                )
                .await
                .expect("[navid-probe] navigation_history query");
            rows.iter()
                .map(|r| {
                    (
                        r.get("id").and_then(|v| v.as_i64()).unwrap_or(-1),
                        r.get("region")
                            .and_then(|v| v.as_string())
                            .unwrap_or("?")
                            .to_string(),
                        r.get("block_id")
                            .and_then(|v| v.as_string())
                            .unwrap_or("<NULL>")
                            .to_string(),
                    )
                })
                .collect()
        }

        let boot = nav_rows(&comp).await;
        eprintln!("[navid-probe/boot] {boot:?}");

        SutFocusWrite::apply_navigate_focus(
            &comp,
            CapRegion::Main,
            &EntityUri::parse("block:structural-page").unwrap(),
        )
        .await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let after_nav = nav_rows(&comp).await;
        eprintln!("[navid-probe/after-nav-page] {after_nav:?}");

        SutFocusWrite::apply_focus_editable_text(&comp, &EntityUri::parse("block:c1").unwrap())
            .await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let after_editor = nav_rows(&comp).await;
        eprintln!("[navid-probe/after-focus-editable] {after_editor:?}");

        SutAppLifecycle::create_document(&comp, "probe-doc.org").await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let after_doc = nav_rows(&comp).await;
        eprintln!("[navid-probe/after-create-doc] {after_doc:?}");

        eprintln!(
            "[navid-probe] SUMMARY rows: boot={} after_nav={} after_editor={} after_doc={} \
             (FocusEditableText/create_document MUST NOT add nav rows to be foldable safely)",
            boot.len(),
            after_nav.len(),
            after_editor.len(),
            after_doc.len()
        );
    }

    /// **C2.0 make-or-break PROBE — does the real headless component's `block_raw`
    /// reduce to the spike's fixed `parent/c1/c2` tree after a production-create
    /// seed, so the structural reconcile loop can run over it?** The spike proves
    /// reconcile over a *bare* `new_sql_engine_with_structural_ops` (no boot
    /// bootstrap). `HeadlessFrontendComponent` runs the FULL production boot — which
    /// may create journals/default pages (the nav slice's `block:journals` came from
    /// here). If boot leaves extra blocks, `inv-blocks-match-ref/block_raw` (a
    /// full-set compare) would false-RED against `build_started_ref`'s parent/c1/c2.
    /// This probe DUMPS the booted block_raw, seeds the tree via the SAME production
    /// create op the spike uses, dumps again, then runs split→reconcile→catalog so
    /// the seed/oracle alignment is decided EMPIRICALLY before the SUT is built.
    #[tokio::test(flavor = "multi_thread")]
    async fn headless_structural_seed_and_reconcile_probe() {
        use crate::pbt::composed::seed_primitives::{C1, C2, PARENT, fixed_ids};
        use crate::pbt::composed::subsystem_seed::{build_started_ref, run_with_seeded_ref};
        use crate::pbt::is_synthetic_ref_id;
        use crate::pbt::op_write_cap::{IdResolver, OpDispatchWriter};
        use crate::pbt::sql_slice::SqlProjectionComponent;
        use holon_api::EntityUri;
        use holon_pbt_core::TransitionRef;
        use holon_pbt_core::capabilities::{SutBackend, SutBlockTreeWrite};
        use std::collections::BTreeSet;

        async fn dump(comp: &HeadlessFrontendComponent, tag: &str) {
            let rows = comp
                .engine
                .db_handle()
                .query(
                    "SELECT id, parent_id, content FROM block_raw ORDER BY id",
                    std::collections::HashMap::new(),
                )
                .await
                .expect("block_raw dump");
            eprintln!("[struct-probe] {tag}: {} block_raw rows", rows.len());
            for r in &rows {
                let id = r.get("id").and_then(|v| v.as_string()).unwrap_or("?");
                let parent = r
                    .get("parent_id")
                    .and_then(|v| v.as_string())
                    .unwrap_or("<none>");
                let content = r.get("content").and_then(|v| v.as_string()).unwrap_or("");
                eprintln!("[struct-probe]   id={id} parent={parent} content={content:?}");
            }
        }

        // Boot with a SINGLE minimal org page so we see the pure production
        // bootstrap, then the seed delta.
        let comp = HeadlessFrontendComponent::new(
            &[("doc0.org", "#+ID: ref-doc-0\n* Doc zero\n")],
            Duration::from_millis(300),
        )
        .await;
        dump(&comp, "after-boot").await;

        // Capture the booted scaffold ids (everything present BEFORE we seed the
        // working tree) — these become the oracle's seed set so they filter out of
        // the SUT-side id comparison.
        let booted: BTreeSet<EntityUri> = comp
            .engine
            .db_handle()
            .query("SELECT id FROM block_raw", std::collections::HashMap::new())
            .await
            .expect("booted id query")
            .iter()
            .map(|r| {
                let s = r
                    .get("id")
                    .and_then(|v| v.as_string())
                    .expect("booted id is a string");
                EntityUri::parse(s).expect("parse booted id")
            })
            .collect();

        // Seed the fixed parent/c1/c2 tree via the production create op — EXACTLY
        // `spike::seed_sql` but over the headless component's real engine.
        let ids = fixed_ids();
        let seeder = SqlProjectionComponent::new(comp.engine.clone());
        seeder
            .create_block(&ids.parent, &EntityUri::no_parent(), PARENT)
            .await;
        seeder.create_block(&ids.c1, &ids.parent, C1).await;
        seeder.create_block(&ids.c2, &ids.parent, C2).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        dump(&comp, "after-seed").await;

        // Build a MINIMAL structural capmap: the component as `SutBackend` + the
        // resolver-sharing writer (so split-minted real ids reconcile). Mirror of
        // `sql_structural_wide`, sourced from the headless component.
        let resolver: IdResolver =
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::new()));
        let comp = std::sync::Arc::new(comp);
        let mut caps = CapMap::new();
        caps.insert(comp.clone() as Arc<dyn SutBackend>);
        caps.insert(Arc::new(OpDispatchWriter::with_resolver(
            comp.engine.clone(),
            resolver.clone(),
        )) as Arc<dyn SutBlockTreeWrite>);

        async fn sut_ids(caps: &CapMap) -> BTreeSet<EntityUri> {
            caps.expect::<dyn SutBackend>()
                .block_raw_snapshot()
                .await
                .into_iter()
                .map(|b| b.id.clone())
                .collect()
        }

        // The oracle: `build_started_ref` seeds parent/c1/c2 as NON-seed (no
        // `block_documents` entry → compared every tick). Inject each booted
        // scaffold id as `block_documents[id]=no_parent` so it joins
        // `seed_block_ids()` and is filtered from the SUT-side id-set-exact
        // `compare_block_subset`, reducing the comparison to {parent,c1,c2}(+split)
        // on both sides. (Headless analog of E1 `SutOrgRead` seeding the oracle from
        // booted blocks — the spike's bare engine has no scaffold to filter.)
        let scaffold_ids: BTreeSet<EntityUri> = booted
            .iter()
            .filter(|id| !is_synthetic_ref_id(id))
            .cloned()
            .collect();
        let inject_seed = |oracle: &mut crate::pbt::reference_state::ReferenceState| {
            for id in &scaffold_ids {
                oracle
                    .domain
                    .block_state
                    .block_documents
                    .insert(id.clone(), EntityUri::no_parent());
            }
        };

        // (1) Catalog must be green on the SEEDED state (no split yet).
        {
            let mut oracle = build_started_ref(&BTreeSet::new());
            inject_seed(&mut oracle);
            let report = run_with_seeded_ref(
                &crate::pbt::composed::composed_invariant_catalog(),
                &caps,
                crate::pbt::reference_state::Resolved::identity(oracle),
            )
            .await;
            assert!(
                report.failures().is_empty(),
                "[struct-probe] seeded (pre-split) catalog must be green over the headless \
                 component: {:?}",
                report.failures()
            );
            assert!(
                report.ran_ids().contains(&"inv-blocks-match-ref/block_raw"),
                "[struct-probe] non-vacuity: inv-blocks-match-ref/block_raw must RUN (ran: {:?})",
                report.ran_ids()
            );
        }

        // (2) Drive a split through the CapMap (real uuid minted), reconcile the
        // oracle's synthetic `block::split-N` against it, re-run the catalog.
        use crate::pbt::transitions::SplitBlock;
        use holon_pbt_core::TransitionImpl;
        let mut oracle = build_started_ref(&BTreeSet::new());
        inject_seed(&mut oracle);
        let before = sut_ids(&caps).await;
        let split = SplitBlock {
            block_id: ids.c1.clone(),
            position: 1,
        };
        split.apply_to_ref(&mut oracle); // oracle mints synthetic block::split-N
        TransitionImpl::apply_to_sut(&split, &oracle, &mut caps).await; // SUT mints uuid
        tokio::time::sleep(Duration::from_millis(300)).await;
        let after = sut_ids(&caps).await;

        let synthetic: Vec<EntityUri> = oracle
            .domain
            .block_state
            .blocks
            .keys()
            .filter(|id| is_synthetic_ref_id(id))
            .cloned()
            .collect();
        let real_new: Vec<EntityUri> = after.difference(&before).cloned().collect();
        eprintln!(
            "[struct-probe] split: {} synthetic {synthetic:?} ↔ {} real {real_new:?}",
            synthetic.len(),
            real_new.len()
        );
        assert_eq!(
            synthetic.len(),
            1,
            "[struct-probe] one synthetic oracle split id"
        );
        assert_eq!(
            real_new.len(),
            1,
            "[struct-probe] one real minted id (before={before:?}, after={after:?})"
        );
        let mut map = std::collections::BTreeMap::new();
        map.insert(synthetic[0].clone(), real_new[0].clone());
        let resolved = oracle.with_resolved_doc_uris(&map);
        std::thread::spawn(move || drop(oracle))
            .join()
            .expect("drop oracle off the async executor");

        let report = run_with_seeded_ref(
            &crate::pbt::composed::composed_invariant_catalog(),
            &caps,
            resolved,
        )
        .await;
        assert!(
            report.failures().is_empty(),
            "[struct-probe] reconciled (post-split) catalog must be green over the headless \
             component: {:?}",
            report.failures()
        );
        assert!(
            report.ran_ids().contains(&"inv-blocks-match-ref/block_raw"),
            "[struct-probe] non-vacuity (post-split): inv-blocks-match-ref/block_raw must RUN (ran: {:?})",
            report.ran_ids()
        );
        eprintln!(
            "[struct-probe] OK — reconciled structural catalog green over HeadlessFrontendComponent"
        );
    }
}
