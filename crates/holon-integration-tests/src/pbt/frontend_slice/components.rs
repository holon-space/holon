//! The SUT component of the frontend slice: a [`CapProvider`] wrapping a
//! **real** headless frontend stack — the production `FrontendSession` +
//! `ReactiveEngine` over a Turso `BackendEngine`, built through the exact DI
//! path the GPUI/CLI frontends use ([`holon_app::new_from_config_with_di`]) —
//! but **windowless**: no GPUI, no geometry, no display link. This is the
//! ViewModel/Renderer slice of the future `E2ESut` replacement.
//!
//! It provides [`SutRenderer`] over the same headless interpret pipeline
//! `E2ESut` uses for its render invariants: `ReactiveEngine::ensure_watching` →
//! `ReactiveRenderedRows::snapshot` → `holon_frontend::interpret_pure` against
//! a `HeadlessBuilderServices`, then the shared `view_model_to_snapshot`. So
//! the catalog's renderer invariants run over the **real** CDC→watch→render
//! path, not a re-implementation.
//!
//! It also provides [`SutBackend`] over `block_raw` (so the block-tree catalog
//! runs over this realization too, §6) and hosts the sync owned-return
//! [`RefViewSelection`] cap (folded in here so it is never dead code, §F4).

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use holon::api::BackendEngine;
use holon_api::Block;
use holon_api::EntityUri;
use holon_api::QueryLanguage;
use holon_app::HeadlessBuilderServices;
use holon_frontend::FrontendSession;
use holon_frontend::ReactiveEngineDriver;
use holon_frontend::UserDriver;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive::ReactiveEngine;
use holon_frontend::reactive::ReactiveRenderedRows;
use holon_frontend::reactive::table_expr;
use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::SutAdviceMatview;
use holon_pbt_core::capabilities::SutAppLifecycle;
use holon_pbt_core::capabilities::SutBackend;
use holon_pbt_core::capabilities::SutBlockCreate;
use holon_pbt_core::capabilities::SutBlockTreeWrite;
use holon_pbt_core::capabilities::SutClockAdvance;
use holon_pbt_core::capabilities::SutEditorMirrorRead;
use holon_pbt_core::capabilities::SutEditorMirrorWrite;
use holon_pbt_core::capabilities::SutErrorLog;
use holon_pbt_core::capabilities::SutFocus;
use holon_pbt_core::capabilities::SutFocusWrite;
use holon_pbt_core::capabilities::SutHistoryWrite;
use holon_pbt_core::capabilities::SutMcpEmit;
use holon_pbt_core::capabilities::SutMutate;
use holon_pbt_core::capabilities::SutNavHistoryDrive;
use holon_pbt_core::capabilities::SutNavHistoryWrite;
use holon_pbt_core::capabilities::SutOrgRead;
use holon_pbt_core::capabilities::SutOrgRender;
use holon_pbt_core::capabilities::SutQueryResults;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::capabilities::SutSeamMutate;
use holon_pbt_core::capabilities::SutSqlProjection;
use holon_pbt_core::capabilities::SutViewControl;
use holon_pbt_core::capabilities::SutViewSelection;
use holon_pbt_core::capabilities::SutWatch;
use holon_pbt_core::capabilities::SutWatchRegister;
use holon_pbt_core::capabilities::WatchRow;
use holon_pbt_core::capabilities::WidgetSnapshot;
use holon_pbt_core::composition::CapMap;
use holon_pbt_core::composition::CapProvider;
use holon_pbt_core::types::CycleTarget;
use holon_pbt_core::types::Mutation;
use holon_pbt_core::types::MutationEvent;
use tempfile::TempDir;

use crate::pbt::query::TestQuery;
use crate::pbt::sut_row_parsing::BLOCK_MATVIEW_SNAPSHOT_SQL;
use crate::pbt::sut_row_parsing::BLOCK_RAW_SNAPSHOT_SQL;
use crate::pbt::sut_row_parsing::parse_block_rows;
use crate::pbt::transitions::toggle_state::cycle_click_count;
use crate::pbt::types::MutationApply;
use crate::pbt::vm_snapshot::view_model_to_snapshot;

/// Raise a fixed test deadline to the scale-soak settle budget
/// (`HOLON_SOAK_SETTLE_MS`) when the soak is on. The frontend component's
/// settle/bind/postcondition deadlines (3-10s) are tuned for the keystone's
/// 3-block doc; at a 5-10k-block soak vault the sidebar/CDC streams
/// legitimately take longer, and a too-small deadline turns honest lag into a
/// false fail-loud. No-op (the fixed default) when the env var is unset.
fn soak_deadline(default: Duration) -> Duration {
    match std::env::var("HOLON_SOAK_SETTLE_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
    {
        Some(ms) => default.max(Duration::from_millis(ms)),
        None => default,
    }
}

/// A composition component wrapping a real headless frontend stack. Owns the
/// `TempDir`, `FrontendSession`, and `ReactiveEngine` so background tasks and
/// the on-disk (in-memory FS) org root stay alive for the component's lifetime.
pub struct HeadlessFrontendComponent {
    engine: Arc<BackendEngine>,
    reactive: Arc<ReactiveEngine>,
    /// The production headless `UserDriver` over `reactive` — the SAME
    /// `ReactiveEngineDriver` the GPUI/TUI/CLI frontends install. Hosts the
    /// production `HeadlessEditorMirror`, so `apply_focus_editable_text` (open
    /// an editor = `click_entity`) and the keystroke-driven
    /// `SutEditorMirrorWrite` caps (`apply_type_chars`/
    /// `apply_delete_backward`/`apply_move_cursor` → `send_raw_keystroke`)
    /// drive the EXACT production headless editor pipeline —
    /// no `InMemEditorComponent` stand-in, no GPUI window/geometry. Caret reads
    /// (`SutEditorMirrorRead::editor_caret_byte`) come from this driver's
    /// mirror.
    driver: Arc<ReactiveEngineDriver>,
    /// The production `FrontendSession` — drives navigation through the same
    /// `execute_operation("navigation", "focus", …)` op path the GPUI/CLI
    /// frontends use (`SutFocusWrite`). Retained (no longer `_`-prefixed) so
    /// the focus write cap can dispatch through it.
    session: Arc<FrontendSession>,
    _temp: TempDir,
    /// `query_id → query:<hash>` registry-key mapping for the watches this
    /// component has registered (E1: `SutWatch` over the PRODUCTION reactive
    /// watch surface). Production keys query watches by content hash, so the
    /// component tracks the test's `query_id` against the engine key it got
    /// back from [`ReactiveEngine::watch_query_live`], plus the
    /// `WatchGuard` that keeps the query watcher alive (dropping the entry
    /// releases it).
    watches: Mutex<Vec<(String, EntityUri, holon_frontend::WatchGuard)>>,
    /// The in-memory org FS, its root, and the tracked org file paths —
    /// retained so the component can provide `SutOrgRead` by parsing the
    /// on-disk org files back into blocks (E1: org block-equivalence over
    /// the PRODUCTION `holon_orgmode::parser::parse_org_file`, no
    /// `FileSyncController` needed).
    org_fs: Arc<holon_filesystem::InMemoryFileSystem>,
    org_root: PathBuf,
    org_paths: Vec<PathBuf>,
    /// `(resolved doc-block id, file path)` per tracked org file, cached from a
    /// clean boot parse — the disk-independent doc mapping `SutOrgRender`
    /// renders by. Tracked user-doc org files (doc page id → path).
    /// Interior-mutable: seeded at boot AND appended by `create_document`
    /// so a mid-run `CreateDocument` doc becomes a valid target for
    /// `BulkExternalAdd`/External `ApplyMutation` (which look up the file).
    documents: Mutex<Vec<(EntityUri, PathBuf)>>,
    /// The captured DI injector — `SutOrgRender` resolves the
    /// `QueryableCache<Block>` from it to build the production
    /// `CacheBlockReader` (the doc-scoped recursive CTE ordered by
    /// `sort_key, id`, so descendants render in the exact order the
    /// `FileSyncController` writes them).
    injector: fluxdi::Injector,
    /// The active view/mode name (`SutViewControl::switch_view` writes it,
    /// `SutViewSelection::current_view` reads it). Honest tracked state
    /// replacing the former hardcoded `"all"` stub — a faithful port of
    /// `TestEnvironment`'s `current_view` (default `"all"`). Drives the
    /// `SwitchView` PBT transition's effect so the view-selection oracle
    /// observes it on the composed path.
    current_view: Mutex<String>,
    /// Shared oracle-synthetic → SUT-real id map (the same [`IdResolver`] the
    /// composed [`OpDispatchWriter`] accumulates split reconciliations into).
    /// Set by the composed builder so the id-taking nav/focus caps
    /// (`pin_block`/`apply_navigate_focus`/`apply_focus_editable_text`)
    /// translate an oracle id (e.g. a synthetic `block::split-N` the
    /// generator drew from the oracle's descendants) to the real minted id
    /// before dispatching — exactly as the block-tree writer already does.
    /// Unset (`OnceLock` empty) ⇒ identity resolution (the fixed-id slices,
    /// where oracle id == store id).
    resolver: std::sync::OnceLock<crate::pbt::op_write_cap::IdResolver>,
    /// The controllable clock this boot injected into the engine's DI (as
    /// `InjectedClock`), so the `ClockScheduler` ticks on it instead of the OS
    /// clock. `Some` only when the caller asked for an injected clock (the
    /// `AdvanceDay` keystone driver); `None` for the ordinary SystemClock boot.
    /// `SutClockAdvance` advances THIS clock and re-runs the scheduler's own
    /// `reconcile_clock`, so a day-rollover CDC re-fires the journal rule.
    clock: Option<Arc<holon_api::TestClock>>,
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

    /// Like [`Self::new_with_loro`] but injects a controllable [`TestClock`]
    /// into the engine's DI as `InjectedClock`, so the production
    /// `ClockScheduler` ticks on it. The keystone `AdvanceDay` transition
    /// (ADR 0024 §6) then advances this clock through `SutClockAdvance`,
    /// driving day-rollover re-fires of the journal rule down the real
    /// reactive path.
    pub async fn new_with_clock(
        org_files: &[(&str, &str)],
        settle: Duration,
        loro_enabled: bool,
        clock: Arc<holon_api::TestClock>,
    ) -> Self {
        Self::new_impl(org_files, settle, loro_enabled, Some(clock)).await
    }

    /// Like [`Self::new`] but with the Loro CRDT layer ENABLED — the production
    /// bootstrap then registers `LoroModule` (the `BlockCellRegistry` backing
    /// `MutableText`) and ingests the org tree through Loro storage, so the
    /// editor caps (`SutEditorMirrorWrite`) can resolve a block's
    /// `content_raw` cell and type into it. Required for any config that
    /// drives `TypeChars`/`DeleteBackward` (the editor primitives are
    /// no-ops without a `MutableText`, exactly why
    /// `general_e2e_pbt_sql_only` can't run them).
    pub async fn new_with_loro(
        org_files: &[(&str, &str)],
        settle: Duration,
        loro_enabled: bool,
    ) -> Self {
        Self::new_impl(org_files, settle, loro_enabled, None).await
    }

    async fn new_impl(
        org_files: &[(&str, &str)],
        settle: Duration,
        loro_enabled: bool,
        clock: Option<Arc<holon_api::TestClock>>,
    ) -> Self {
        use holon_frontend::HolonConfig;
        use holon_frontend::SessionConfig;

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
            // Scale-soak vault docs (`soak-*.org`) are background LOAD: written to disk
            // (so the session's file-sync ingests them into the store — the whole point)
            // but NOT tracked in `org_paths`. Tracking them would drag thousands of
            // oracle-unknown blocks into the org readers (`SutOrgRead` /
            // `SutOrgRender` / the external-mutation doc rewriter) and false-RED
            // `inv-blocks-match-ref/org`. The org invariants keep the keystone-sized
            // comparison surface; the soak load is exercised via store/CDC/Loro.
            if !filename.starts_with("soak-") {
                org_paths.push(file_path);
            }
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
        // Clock DI seam (ADR 0024 §6): when the caller injects a controllable
        // clock, register it as `InjectedClock` BEFORE the engine resolves, so the
        // `ClockScheduler` (spawned in `create_initialized_engine`) ticks on it
        // instead of the OS `SystemClock`. `None` → the factory falls back to
        // `SystemClock`, unchanged.
        let clock_for_di = clock.clone();

        let (session, engine, reactive) = holon_app::new_from_config_with_di(
            holon_config,
            session_config,
            config_dir,
            std::collections::HashSet::new(),
            move |injector| {
                crate::test_environment::install_headless_render_interpreter(
                    injector,
                    &org_fs_for_di,
                );
                if let Some(test_clock) = clock_for_di.clone() {
                    let injected =
                        holon_api::InjectedClock(test_clock as Arc<dyn holon_api::Clock>);
                    injector.provide::<holon_api::InjectedClock>(fluxdi::Provider::root(
                        move |_| injected.clone().into(),
                    ));
                }
                Ok(())
            },
            move |injector| {
                let engine = crate::test_environment::publish_reactive_builder_services(injector);
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
            // Boot settle: CONVERGE instead of sleeping the full budget — the same
            // three signals as the composed per-transition settle
            // (`convergence::converge_signals`), with `settle` as the CAP (worst
            // case = the former flat sleep; a quiescent boot returns in ms). The
            // doc-id caching below reads the session-persisted `:ID:` drawers from
            // disk, so the org drain must complete first. The org idle signal (and
            // the Loro handles) resolve on a spawned `post_ready_work` task, so
            // poll for the signal within the budget — a config with no org
            // file-sync never resolves it and pays the full budget, exactly the
            // old behavior.
            let deadline = tokio::time::Instant::now() + settle;
            let injector = injector_slot
                .get()
                .expect("DI injector captured during build");
            let mut org_idle = injector
                .try_resolve::<holon_orgmode::OrgSyncIdleSignal>()
                .ok(); // ALLOW(ok): optional DI service — absent when org sync is off
            while org_idle.is_none() && tokio::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(5)).await;
                org_idle = injector
                    .try_resolve::<holon_orgmode::OrgSyncIdleSignal>()
                    .ok(); // ALLOW(ok): optional DI service — absent when org sync is off
            }
            let sync = injector
                .try_resolve::<holon::sync::LoroSyncControllerHandle>()
                .ok(); // ALLOW(ok): optional DI service — absent when Loro/sync is off
            let store = injector
                .try_resolve::<holon::sync::LoroDocumentStore>()
                .ok() // ALLOW(ok): optional DI service — absent when Loro is off
                .map(|s| (*s).clone());
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            // Boot settle: tolerate non-convergence (returns bool) — the sync
            // controller / idle signal resolve on a spawned post_ready_work task,
            // so a not-yet-wired signal at boot is expected, not a race. The
            // per-transition composed settle is the one that fails loud.
            let _boot_converged = crate::pbt::convergence::converge_signals(
                Some(&engine),
                sync,
                store,
                org_idle,
                remaining,
            )
            .await;
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
            // Register the file's tracked doc id. A file WITH headline blocks
            // takes the doc id from the first block's `parent_id` (the doc root
            // the parser reconstructs from `#+ID:`). A file with ZERO headline
            // blocks — a bare page-file like `<page>.org` = `#+ID: <page>\n`, the
            // on-disk form of a page that owns its own file — must STILL be
            // tracked: its doc root is `parsed.document.id`, and `SutOrgRender`
            // has to surface it so the writeback oracles can observe that the page
            // has its own file and that a folder companion de-inlined it (Fork B).
            // Skipping zero-block files (the old behavior) made every page-file
            // invisible to the org readers, so a companion could silently keep or
            // drop it with no oracle able to see either.
            let doc_id = parsed
                .blocks
                .first()
                .map(|b| b.parent_id.clone())
                .unwrap_or_else(|| parsed.document.id.clone());
            documents.push((doc_id, path.clone()));
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
            clock,
        }
    }

    /// The production `BackendEngine` backing this session — shared (`Arc`) so
    /// a composed structural SUT (`frontend_slice::structural_pbt`) can
    /// build the resolver-sharing [`OpDispatchWriter`] over it and seed the
    /// working tree via the production create op
    /// (`crate::pbt::sql_slice::SqlProjectionComponent`).
    pub(crate) fn engine(&self) -> Arc<BackendEngine> {
        self.engine.clone()
    }

    /// The production windowless `FrontendSession` this component booted.
    /// Handed to a gpui window (`launch_holon_window_rebindable`) so the
    /// window RENDERS the same reactive tree the composed backend/storage
    /// caps read — the windowed repoint reuses this headless boot and
    /// attaches the window as a pure renderer (§ Round 5).
    pub(crate) fn session(&self) -> Arc<FrontendSession> {
        self.session.clone()
    }

    /// The production `ReactiveEngine` (the `BuilderServices` host). The wide
    /// PBT's resolver-sharing [`OpDispatchWriter`] uses it as a focus sink
    /// so `split_block`/ `join_block` dispatch through the frontend's
    /// `dispatch_intent_sync` and the new/ merged block becomes the focused
    /// block (the frontend split focus-handoff).
    pub(crate) fn reactive(&self) -> Arc<ReactiveEngine> {
        self.reactive.clone()
    }

    /// The production headless [`UserDriver`] (`ReactiveEngineDriver`) — the
    /// SAME instance the editor/focus caps drive through, so it hosts the
    /// one live `HeadlessEditorMirror`. Handed to the headless
    /// driver-backed input component
    /// (`DriverInputComponent::with_input_headless`) so the composed `CapMap`'s
    /// gesture caps (`SutBlockInteract`/`SutArrowNavigate`/`SutDriver`)
    /// drive the UI-adjacent logic layer over ONE driver (the VM rung,
    /// §8.11). MUST be this instance, not a
    /// fresh `ReactiveEngineDriver::new` — a second one would carry a separate
    /// editor mirror and desync caret/text from the editor-write caps.
    pub(crate) fn driver(&self) -> Arc<dyn UserDriver> {
        self.driver.clone()
    }

    /// Build a `KeystrokeBlockTreeWriter` backed by `driver` (§8.12 C-3
    /// mechanism 1): the SAME production keystroke sequences the base
    /// install uses, but over the given driver — so the windowed overlay
    /// can rebind `SutBlockTreeWrite` onto the window's `GpuiUserDriver`/
    /// `SimUserDriver`. Reuses the component's own `reactive` (live
    /// editor content reads + the chord-binding registry) and the shared
    /// `resolver` (oracle→minted id remap). The whole structural family —
    /// including `move_up`/ `move_down` (C-3 mechanism 3, chord-resolved
    /// via `send_key_chord`) — now rides `driver`; nothing is left on raw
    /// op dispatch. Fail-loud if the resolver was never set (only the
    /// composed builder wires it; a caller without it drove nothing).
    pub(crate) fn keystroke_writer_with(
        &self,
        driver: Arc<dyn UserDriver>,
    ) -> crate::pbt::op_write_cap::KeystrokeBlockTreeWriter {
        use crate::pbt::op_write_cap::KeystrokeBlockTreeWriter;
        let resolver = self
            .resolver
            .get()
            .expect("keystroke_writer_with: resolver must be set by the composed builder")
            .clone();
        KeystrokeBlockTreeWriter::new(driver, self.reactive.clone(), resolver)
    }

    /// The frontend's `LoroDocumentStore` — the authority store the production
    /// op pipeline writes (`LoroBlockOperations`) and `block_raw` projects
    /// from. `None` when Loro is disabled on this build (`new_with_loro(..,
    /// false)` → no `LoroModule` registered). The composed builder's Loro
    /// arm reads its `SutLoroTaskState`/`SutLoroLog` caps over THIS store's
    /// global doc (not a separate one) so a write through the frontend is
    /// visible to the Loro read caps — the read-doc unification (task #4).
    /// The clone shares the underlying
    /// `Arc<RwLock<Option<Arc<LoroDocument>>>>`, so it observes the SAME live
    /// doc.
    pub(crate) fn loro_doc_store(&self) -> Option<holon::sync::LoroDocumentStore> {
        self.injector
            .try_resolve::<holon::sync::LoroDocumentStore>()
            .ok() // ALLOW(ok): optional DI service — absent when Loro is disabled
            .map(|store| (*store).clone())
    }

    /// The frontend session's `LoroSyncController` handle — the controller that
    /// watches the authority doc (`subscribe_root`) and projects imported peer
    /// deltas into the Turso `block_raw` the block invariants read. The
    /// composed builder's Loro arm hands this to `LoroSut` in full mode so
    /// a `MergeFromPeer` can `wait_for_quiescence` on it before the merged
    /// block is read back (the projection is async —
    /// `loro_sync_controller.rs` runs it on a spawned loop).
    /// Mirrors E2ESut's `ctx.loro_sync_handle()` (`sut_handle.rs:197`).
    ///
    /// Resolution is RACE-prone: the headless build uses `without_wait()`, so
    /// the controller is started on a spawned `post_ready_work` task and is
    /// NOT awaited at boot (`wiring.rs:360`). Callers that need it present
    /// must poll until the boot settle completes (see the A0 readiness
    /// probe). `None` when Loro/sync is disabled OR the spawned start task
    /// has not yet resolved the handle.
    pub(crate) fn loro_sync_handle(&self) -> Option<Arc<holon::sync::LoroSyncControllerHandle>> {
        self.injector
            .try_resolve::<holon::sync::LoroSyncControllerHandle>()
            .ok() // ALLOW(ok): optional DI service — absent when Loro/sync is disabled
    }

    /// The file-sync controller's [`OrgSyncIdleSignal`] — advanced
    /// (`mark_progress`) after every processed file/block change, so a
    /// settle can `wait_quiescent` on it instead of a flat sleep. `None`
    /// when this build has no org file-sync wired. Resolved lazily (like
    /// [`Self::loro_sync_handle`]): the controller is started on a
    /// spawned `post_ready_work` task, so callers that settle after a write get
    /// the resolved signal by then. Mirrors `TestEnvironment`'s
    /// `org_sync_idle` latch.
    pub(crate) fn org_idle_signal(&self) -> Option<Arc<holon_orgmode::OrgSyncIdleSignal>> {
        self.injector
            .try_resolve::<holon_orgmode::OrgSyncIdleSignal>()
            .ok() // ALLOW(ok): optional DI service — absent when org file-sync is disabled
    }

    /// Share the composed runner's [`IdResolver`] so the id-taking nav/focus
    /// caps translate oracle ids to SUT-real ids (see the `resolver`
    /// field). Called once by the composed builder; the
    /// storage-only/fixed-id slices leave it unset (identity resolution).
    pub(crate) fn set_resolver(&self, resolver: crate::pbt::op_write_cap::IdResolver) {
        self.resolver
            .set(resolver)
            .map_err(|_| "HeadlessFrontendComponent resolver already set")
            .expect("set resolver once");
    }

    /// Resolve an oracle-space id to its SUT-space id (identity if the resolver
    /// is unset or the id is unmapped) — the component-side analog of
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

    /// Register a watched query through the **production** reactive watch
    /// surface ([`ReactiveEngine::watch_query_live`] →
    /// `ensure_query_watching` → the real `registry`/`watchers` + CDC pump
    /// into [`ReactiveRenderedRows`]), and record the `query_id →
    /// query:<hash>` key mapping. This is the production analogue of
    /// the E2ESut harness's hand-rolled `setup_watch`/`ui_model` — the same
    /// `TestQuery::compile_for` source the wide PBT uses, but driven through
    /// the reactive engine the headless frontend actually runs, not a
    /// bespoke `CdcAccumulator`.
    pub fn register_query_watch(&self, query_id: &str, query: &TestQuery, lang: QueryLanguage) {
        let (source, lang) = query.compile_for(lang);
        self.register_watch_compiled(query_id, source, lang);
    }

    /// Shared core: register a watch from an already-compiled `(source, lang)`
    /// through the production reactive watch surface. Used by both the test
    /// helper [`Self::register_query_watch`] and the `SutWatchRegister` cap
    /// (the decomposed `SetupWatch` drive path — INC 3), which receives the
    /// query pre-compiled at the transition boundary.
    fn register_watch_compiled(&self, query_id: &str, source: String, lang: QueryLanguage) {
        let services: Arc<dyn BuilderServices> = self.reactive.clone();
        let (key, mut live) =
            self.reactive
                .watch_query_live(source, lang, table_expr(), None, services);
        let guard = live
            .watch_guard
            .take()
            .expect("watch_query_live must return a WatchGuard");
        self.watches
            .lock()
            .expect("watches lock")
            .push((query_id.to_string(), key, guard));
    }

    /// Resolve a ready (non-loading) reactive watch for `uri`, polling the
    /// headless engine until its first results load (background tasks fill it
    /// on the shared runtime). Mirrors `E2ESut::resolve_watch`'s
    /// no-frontend-engine branch.
    async fn resolve_watch(&self, uri: &EntityUri) -> Option<Arc<ReactiveRenderedRows>> {
        let deadline = tokio::time::Instant::now() + soak_deadline(Duration::from_secs(3));
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

    /// Graft a block into the headless backend (used to attach a fixed-id
    /// subtree under the Main focus root so `inv-displayed-text` has known
    /// content to compare). Mirrors the windowed slice's
    /// `graft_displayed_text_tree`, but via the `BackendEngine` directly
    /// (no `TestEnvironment`).
    pub async fn create_block(&self, id: &str, parent_id: &str, content: &str) {
        use holon_api::EntityName;
        use holon_api::StorageEntity;
        use holon_api::Value;
        use holon_api::types::ContentType;
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
            .execute_operation(
                &EntityName::new("block"),
                "create",
                params,
                holon_api::OpOrigin::User,
            )
            .await
            .expect("headless create_block");
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

    /// Settle the navigation matviews (`current_focus` / `focus_roots`) to a
    /// fixed point after a `navigation.focus` write, so the focus
    /// invariants read a converged projection. A query watch / op clears
    /// `is_loading` before its CDC pump delivers, so poll the row counts to
    /// a stable fixed point (mirrors the `watch_rows` settle loop).
    /// Reaching the fixed point is what makes the `focus_roots` teeth
    /// produce a real `Fail` (not a CDC-lag `Skipped`, V4).
    async fn settle_focus_matviews(&self) {
        let deadline = tokio::time::Instant::now() + soak_deadline(Duration::from_secs(3));
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

    /// Wait until the LeftSidebar has bound a clickable `navigation.focus`
    /// intent for `id` before a sidebar-nav click is issued. The sidebar
    /// page list is a nested `live_block` watch that streams its rows AND
    /// their bound `selectable` intents in asynchronously after boot/seed;
    /// a click that outruns it makes the production `click_entity` fall
    /// through to an in-memory `set_focus` (which
    /// writes NO `navigation_history` row — see the doc on
    /// `ReactiveEngine::set_focus`) so the `current_focus` matview stays on
    /// the boot-default `journals`. Poll the resolved layout for the exact
    /// predicate `click_entity` dispatches on
    /// (`find_click_intent_in_region`), and fail loud if the entry never binds
    /// rather than let the click silently fake focus.
    async fn await_sidebar_nav_intent(&self, id: &EntityUri) {
        let root_uri = holon_api::root_layout_block_uri();
        let deadline = tokio::time::Instant::now() + soak_deadline(Duration::from_secs(5));
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
                "[SutFocusWrite::apply_navigate_focus] LeftSidebar never bound a navigation.focus \
                 click-intent for {id} within 5s — the sidebar page list (nested live_block \
                 watch) did not stream the target's selectable, so a click would fall through to \
                 an in-memory set_focus (no navigation_history write) and leave current_focus on \
                 the boot default. Sidebar-render / CDC-settle faithfulness gap, not a fake-focus \
                 escape."
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Loud postcondition for a sidebar-nav click: `current_focus(main)` must
    /// reflect `id` once CDC settles. If it does not, the click dispatched no
    /// `navigation.focus` SQL write (silent set_focus fallthrough) or the
    /// matview lagged past `settle_focus_matviews` — either way, never fake
    /// focus: fail loud.
    async fn assert_navigate_focus_landed(&self, id: &EntityUri) {
        let deadline = tokio::time::Instant::now() + soak_deadline(Duration::from_secs(3));
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
                "[SutFocusWrite::apply_navigate_focus] after clicking the LeftSidebar entry for \
                 {id} and settling, current_focus(main) is {focus:?} — the navigation.focus SQL \
                 write did not land (click fell through to an in-memory set_focus, or the matview \
                 lagged). Never fake focus: failing loud."
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Settle `block_id`'s `block_raw.content` to a fixed point after a
    /// keystroke edit. A char insert mutates the editor's `MutableText`
    /// (Loro) cell; the per-keystroke pipeline then syncs that through to
    /// the `block_raw` projection where `inv-blocks-match-ref/block_raw`
    /// reads. That sync is CDC-driven and can lag the synchronous keystroke
    /// return, so poll the projected content to a stable value (3 equal
    /// reads) — the content analogue of `settle_focus_matviews`. This is
    /// what gives committed-content parity with the reference's eager
    /// `commit_active_editor_if_changed`.
    async fn settle_block_content(&self, block_id: &EntityUri) {
        let escaped = block_id.as_str().replace('\'', "''");
        let sql = format!("SELECT content FROM block_raw WHERE id = '{escaped}'");
        let deadline = tokio::time::Instant::now() + soak_deadline(Duration::from_secs(3));
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

    /// Dispatch a `navigation` provider op through the windowless
    /// `FrontendSession` (the headless analogue of `E2ESut`'s driver
    /// `synthetic_dispatch` / leader chords), then settle the focus
    /// matviews. `block_id`/`history_id` are passed only for the ops that
    /// take them (`focus_pin` / `close`); `close` ignores the
    /// region. Drives `SutNavHistoryDrive`.
    async fn dispatch_navigation(
        &self,
        op: &str,
        region: holon_api::Region,
        block_id: Option<String>,
        history_id: Option<i64>,
    ) {
        use holon_api::EntityName;
        use holon_api::Value;
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
                    "[SutNavHistoryDrive::dispatch_navigation] navigation.{op}(region={region:?}) \
                     through the headless session failed: {e:#}"
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
        let deadline = tokio::time::Instant::now() + soak_deadline(Duration::from_secs(5));
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
                // FULLY-RESOLVED fixed point (no loading/unknown placeholders):
                // one confirming resample suffices — the composed harness has
                // already converged CDC+Loro+org before any check
                // (`settle_after_apply` → `converge_projections`), so nothing
                // async is still due. The former unconditional 4×120 ms
                // resample predates that settle and was measured at ~83% of
                // keystone wall time. A tree still holding placeholders keeps
                // the cautious exit (4 stable samples at 120 ms) so slow watch
                // delivery isn't cut short.
                if pending == 0 || stable >= 4 {
                    return snap;
                }
            } else {
                stable = 0;
                last = (total, pending);
            }
            if tokio::time::Instant::now() >= deadline {
                return snap;
            }
            // Keep the proven 120 ms cadence: each resample drives
            // `ensure_watching` (watch views + SQL), and sampling faster was
            // measured to churn CDC enough to inflate the NEXT transition's
            // quiet-floor settle (p50 5 ms → 70 ms) — the early exit above is
            // the win, not a tighter poll.
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
    /// The CDC-driven focus-root mirror `inv-focus-roots` reads. Headlessly
    /// there is no separate `LiveData<FocusRoot>` mirror, so this reads the
    /// same `focus_roots` matview as
    /// [`SutSqlProjection::focus_roots_rows`]. Reading one source for both
    /// means the invariant's mirror==matview check never triggers
    /// the CDC-lag → `Skipped` downgrade — so the navigation slice's teeth
    /// produce a real `Fail` (not `Skipped`) on divergence (V4).
    async fn live_focus_root_rows(&self) -> Vec<(String, String)> {
        self.focus_roots_rows().await
    }
}

#[async_trait::async_trait(?Send)]
impl SutViewSelection for HeadlessFrontendComponent {
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

    // This slice has a headless `ReactiveEngine` but no window, so it does NOT
    // register the windowed `SutFrontendEngine` / `SutFrontendEmissions` caps
    // (C-5 split, 2026-07-02) — the root-VM / emission invariants honestly
    // DESELECT here instead of running vacuously against honest-empty shadows.
    // `current_view` is real tracked state; `drain_vm_emissions` dies with
    // `CachingProxy`. Only `headless_error_node_count` carries real teeth today.
    async fn drain_vm_emissions(&mut self) -> Vec<String> {
        Vec::new()
    }
    async fn current_view(&self) -> String {
        self.current_view.lock().expect("current_view lock").clone()
    }
}

/// `SutWatch` over the **production** reactive watch surface (E1 relocation;
/// the redesign away from E2ESut's bespoke `ui_model`). `watch_query_ids` /
/// `watch_rows` read the live `ReactiveRenderedRows` the engine's CDC pump
/// fills; the two `block_raw` truth reads (used by the invariant's CDC-lag
/// classifier) go straight to the write-side base table via the
/// `BackendEngine`.
#[async_trait::async_trait(?Send)]
impl SutWatch for HeadlessFrontendComponent {
    async fn watch_query_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .watches
            .lock()
            .expect("watches lock")
            .iter()
            .map(|(qid, _, _)| qid.clone())
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
            .find(|(qid, _, _)| qid == query_id)
            .map(|(_, key, _)| key.clone());
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
        let deadline = tokio::time::Instant::now() + soak_deadline(Duration::from_secs(3));
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
                    "[inv-watch-rows-match-ref truth check] block_raw query failed\nsql: {sql}\n \
                     error: {e}"
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
}

/// `SutOrgRead` over the **production** org parser (E1 org block-equivalence):
/// parse the on-disk org files back into blocks via
/// `holon_orgmode::parser::parse_org_file` — the same parser
/// `TestContext::parse_org_file_blocks` runs, no
/// `TestContext`/`FileSyncController` coupling. Binds
/// `inv-blocks-match-ref/org` (org-parsed blocks vs the ref's org view).
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

/// `SutOrgRender` over the **production** render path (E1): render each tracked
/// org file from the current SQL state through the same `CacheBlockReader`
/// (doc-scoped recursive CTE ordered by `sort_key, id`) +
/// `OrgRenderer::render_document` the `FileSyncController` uses, and pair it
/// with the on-disk bytes. Binds `inv-org-render-fixed-point` (disk ==
/// rendered). Mirrors `TestContext::snapshot_org_render_pairs` but over the
/// component's own injector + org_fs. The doc-block id per file is the parent
/// the production parser reconstructs from the file's persisted `:ID:` drawer
/// (== the block_raw doc row).
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
        let header_sql = "SELECT b.id, b.parent_id, b.depth, b.sort_key, b.content, b.content_type, \
             b.source_language, b.source_name, b.properties, b.marks, b.collapsed, b.completed, \
             b.block_type, b.created_at, b.updated_at, COALESCE((SELECT json_group_array(tag) \
             FROM block_tags WHERE block_id = b.id), '[]') AS tags, COALESCE((SELECT \
             json_group_array(required_id) FROM block_requires WHERE block_id = b.id), '[]') AS \
             requires, COALESCE((SELECT json_group_array(lesson_id) FROM advice_suppressed WHERE \
             anchor_id = b.id), '[]') AS advice_suppressed FROM block_raw b";
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

/// `SutFocusWrite` over the **production** navigation op (SutHandle
/// decomposition — NavigateFocus onto SutFocusWrite): drive
/// `navigation.focus(region, block_id)` through the windowless
/// `FrontendSession` (the same op the GPUI/CLI sidebar click dispatches), then
/// settle CDC to the focus-matview fixed point. The `NavigateFocus`
/// transition's `apply_to_sut(&mut CapMap)` reaches this through
/// the `#[capmap_adapter]`-generated `impl SutFocusWrite for CapMap`.
impl HeadlessFrontendComponent {
    /// `NavigateFocus` driven through an explicit `driver` (§8.12 C-3 mechanism
    /// 2). The sidebar click + intent-await + focus-matview settle +
    /// landing assert are driver-invariant; parameterizing the driver is
    /// what lets the windowed sibling (`window_slice::WindowFrontendWrite`)
    /// rebind the click onto the window's `GpuiUserDriver`/`SimUserDriver`
    /// (the HIGHEST-available rung when a window exists) while the headless
    /// base keeps its VM-rung `ReactiveEngineDriver`.
    pub(crate) async fn apply_navigate_focus_via(&self, driver: &dyn UserDriver, id: &EntityUri) {
        // Focus is set by CLICKING the LeftSidebar entry through `driver` — the SAME
        // way a real user (and E2ESut, and the sibling
        // `apply_focus_editable_text_via`) does it, NOT a synthesized
        // `navigation.focus` dispatch that skips the UI. The click-intent
        // resolver dispatches the entry's bound `navigation.focus(region:"main")`
        // action (find_click_intent -> apply_intent -> dispatch_intent), which
        // mirrors focus into `engine.focused_block()` AND writes the SQL nav
        // tables — so both the headless keystone (focus read deselected) and
        // the WINDOWED SUT (window `SutDriver` reads `engine.focused_block()`)
        // see a faithful, consistent focus. The generator
        // restricts `NavigateFocus` to `Region::Main` on sidebar-listed pages, so the
        // click always targets the `left_sidebar` entry.
        let id = self.resolve_id(id);
        // Do not let the click outrun the async sidebar render: wait until the
        // target's `navigation.focus` intent is actually bound, so `click_entity`
        // dispatches the nav SQL write instead of silently falling through to an
        // in-memory `set_focus`.
        self.await_sidebar_nav_intent(&id).await;
        driver
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

    /// `FocusEditableText` driven through an explicit `driver` (§8.12 C-3
    /// mechanism 2). Open an editor on `id` the production way: a
    /// main-panel `click_entity`. For an `editable_text` block the click
    /// binds no intent, so it falls through to `engine.set_focus(id)` (ADR
    /// 0010: focus is pure in-memory state). The windowed sibling passes
    /// the window driver so the click is a real geometry hit-test.
    pub(crate) async fn apply_focus_editable_text_via(
        &self,
        driver: &dyn UserDriver,
        id: &EntityUri,
    ) {
        let id = &self.resolve_id(id);
        driver.click_entity(id, "main").await.unwrap_or_else(|e| {
            panic!(
                "[SutFocusWrite::apply_focus_editable_text] click_entity(main, {id}) failed: {e:#}"
            )
        });
    }
}

/// The keystroke-driven editor write caps over the PRODUCTION headless editor
/// pipeline (`ReactiveEngineDriver` → `HeadlessEditorMirror` → `MutableText`),
/// the headless analogue of `E2ESut`'s `send_raw_keystroke`-based
/// `SutEditorMirrorWrite` — no GPUI window, no `InMemEditorComponent` stand-in.
/// Each `apply_*` settles the focused block's `block_raw.content` to a fixed
/// point so committed-content parity with the reference's eager
/// `commit_active_editor_if_changed` holds for
/// `inv-blocks-match-ref/block_raw`.
impl HeadlessFrontendComponent {
    /// `TypeChars` driven through an explicit `driver` (§8.12 C-3 mechanism 1,
    /// editor family). Each char rides `send_raw_keystroke` —
    /// driver-parameterized so the windowed sibling routes keystrokes
    /// through the window's real `InputState` (`GpuiUserDriver`/
    /// `SimUserDriver`) while headless keeps the `HeadlessEditorMirror`.
    pub(crate) async fn apply_type_chars_via(&self, driver: &dyn UserDriver, text: &str) {
        for ch in text.chars() {
            driver
                .send_raw_keystroke(&ch.to_string(), &[])
                .await
                .unwrap_or_else(|e| {
                    panic!(
                        "[SutEditorMirrorWrite::apply_type_chars] send_raw_keystroke({ch:?}) \
                         failed: {e:#}"
                    )
                });
        }
        if let Some(block) = self.reactive.focused_block() {
            self.settle_block_content(&block).await;
        }
    }

    /// `DeleteBackward` driven through an explicit `driver` (§8.12 C-3
    /// mechanism 1).
    pub(crate) async fn apply_delete_backward_via(&self, driver: &dyn UserDriver, count: usize) {
        for _ in 0..count {
            driver
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

    /// `MoveCursor` driven through an explicit `driver` (§8.12 C-3 mechanism
    /// 1). Convert the byte offset to `home` + N `right` keystrokes against
    /// the focused block's live editor text, exactly as
    /// `E2ESut::apply_move_cursor` does (each `right` advances one
    /// char). No content settle — MoveCursor doesn't write block content
    /// (mirrors the ref).
    pub(crate) async fn apply_move_cursor_via(
        &self,
        driver: &dyn UserDriver,
        byte_position: usize,
    ) {
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
        driver
            .send_raw_keystroke("home", &[])
            .await
            .unwrap_or_else(|e| panic!("[apply_move_cursor] home failed: {e:#}"));
        for _ in 0..right_presses {
            driver
                .send_raw_keystroke("right", &[])
                .await
                .unwrap_or_else(|e| panic!("[apply_move_cursor] right failed: {e:#}"));
        }
    }
}

/// Editor-mirror reads: caret from the driver's `HeadlessEditorMirror` (same
/// map the keystrokes advance), live text from the block's `MutableText` cell
/// (the pre-commit value, same source `E2ESut::editor_live_text` reads).
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

/// `SutNavHistoryWrite` over the **production** `navigation.go_home` op
/// (SutHandle decomposition increment 2): drive `navigation.go_home(region)`
/// through the windowless `FrontendSession` (the same op the GPUI/CLI
/// leader-`h` chord dispatches — `set_focus(None)` + close the region's open
/// pins), then settle CDC to the focus-matview fixed point. The `NavigateHome`
/// transition's `apply_to_sut(&mut CapMap)` reaches this through the
/// `#[capmap_adapter]`-generated `impl SutNavHistoryWrite for CapMap`.
#[async_trait::async_trait(?Send)]
impl SutNavHistoryWrite for HeadlessFrontendComponent {
    async fn apply_navigate_home(&self, region: CapRegion) {
        use holon_api::EntityName;
        use holon_api::Value;
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
                    "[SutNavHistoryWrite::apply_navigate_home] \
                     navigation.go_home(region={region_str}) through the headless session failed: \
                     {e:#}"
                )
            });
        self.settle_focus_matviews().await;
    }
}

/// `SutSqlProjection` over the headless engine's matviews/base tables. The
/// focus rows (`current_focus_rows` / `focus_roots_rows` /
/// `nav_history_open_rows`) read the live navigation matviews navigation wrote
/// to; the block rows mirror the sql_slice's projection (real
/// `block`/`block_raw` reads). Wired only into the navigation slice's CapMap
/// (not the general `register`), so existing frontend slices keep their current
/// selection.
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

    /// No `SutSqlProjection`-tracked CDC watch-count surface here (the watch
    /// set is `SutWatch`' concern); honest `None`.
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
                "SELECT json_extract(properties, '$.task_state') AS task_state FROM block_raw \
                 WHERE id = '{escaped}'"
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
}

/// `SutAdviceMatview` over the live Turso projection — the SQL-level twin
/// `inv-advice-matview-matches-ref` reads this. Discovers the `advice_rule_%`
/// materialized views from `sqlite_master` (same `sql_query` plumbing every
/// other SUT SQL read uses) and reads each one's full row set. Pre-step-6 there
/// is no such matview, so this returns empty (observed-absent).
#[async_trait::async_trait(?Send)]
impl SutAdviceMatview for HeadlessFrontendComponent {
    async fn advice_matviews(&self) -> Vec<(String, Vec<(String, String, u32)>)> {
        let names: Vec<String> = self
            .sql_query("SELECT name FROM sqlite_master WHERE name LIKE 'advice_rule_%'")
            .await
            .iter()
            .filter_map(|r| Self::cell(r, "name"))
            .collect();
        let mut out = Vec::new();
        for name in names {
            // `name` came verbatim from sqlite_master, so it is a live identifier;
            // interpolating it is sound (no user-controlled string reaches here).
            let rows = self
                .sql_query(&format!(
                    "SELECT anchor_id, lesson_id, shared_tag_count FROM {name}"
                ))
                .await
                .iter()
                .map(|r| {
                    let anchor = Self::cell(r, "anchor_id")
                        .expect("advice matview row must carry anchor_id");
                    let lesson = Self::cell(r, "lesson_id")
                        .expect("advice matview row must carry lesson_id");
                    let count = r
                        .get("shared_tag_count")
                        .and_then(|v| v.as_i64())
                        .expect("advice matview row must carry integer shared_tag_count")
                        as u32;
                    (anchor, lesson, count)
                })
                .collect();
            out.push((name, rows));
        }
        out
    }
}

/// `SutFocus` over the live Turso navigation projection — the real
/// teeth for `inv-navigation-focus` / `inv-focus-roots`. Split off
/// `SutSqlProjection` (C-5, 2026-07-02) so a storage-only slice that drives no
/// navigation does not register it and those invariants deselect honestly
/// there.
#[async_trait::async_trait(?Send)]
impl SutFocus for HeadlessFrontendComponent {
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
            "SELECT region, block_id FROM navigation_history WHERE closed_at IS NULL AND block_id \
             IS NOT NULL",
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
/// binds. Shares the `register_watch_compiled` core with the
/// `register_query_watch` test helper, so a composed `CapMap` registers a watch
/// through the SAME `ReactiveEngine::watch_query_live` path the existing B5
/// teeth already prove deliver headlessly. The transition compiles `TestQuery →
/// (source, lang)` at the boundary; this takes the compiled form.
#[async_trait::async_trait(?Send)]
impl SutWatchRegister for HeadlessFrontendComponent {
    async fn register_watch(&self, query_id: &str, source: &str, lang: QueryLanguage) {
        self.register_watch_compiled(query_id, source.to_string(), lang);
    }

    async fn unregister_watch(&self, query_id: &str) {
        // Drop the tracked watch entry; dropping its `WatchGuard` releases
        // the underlying query watcher when this was the last consumer.
        self.watches
            .lock()
            .expect("watches lock")
            .retain(|(id, _, _)| id != query_id);
    }
}

/// `SutViewControl` (the `SwitchView` transition): set the active view name.
/// Faithful port of `E2ESut`/`TestEnvironment::switch_view` — a pure
/// interior-mut write the `SutViewSelection::current_view` oracle reads back.
#[async_trait::async_trait(?Send)]
impl SutViewControl for HeadlessFrontendComponent {
    async fn switch_view(&self, view_name: &str) {
        *self.current_view.lock().expect("current_view lock") = view_name.to_string();
    }
}

/// `SutMcpEmit` (the `EmitMcpData` transition): emit the current state over the
/// MCP integration. The windowless headless stack has no `PbtMcpIntegration`
/// attached (just as `E2ESut::emit_mcp_data` is a no-op when its `pbt_mcp` slot
/// is empty), so this is a faithful no-op — no invariant observes an MCP
/// emission on this path.
#[async_trait::async_trait(?Send)]
impl SutMcpEmit for HeadlessFrontendComponent {
    async fn emit_mcp_data(&self) {
        tracing::trace!("[apply] EmitMcpData (headless frontend slice: no MCP integration, no-op)");
    }
}

/// `SutClockAdvance` (the `AdvanceDay` transition, ADR 0024 §6): advance the
/// injected fake clock and re-run the scheduler's OWN reconcile so a
/// day-rollover CDC re-fires the journal rule — the prod-faithful path (WP1),
/// never a raw `clock`-relation UPDATE. `days == 0` re-ticks the same day
/// (idempotence probe: `reconcile_clock` sees `Unchanged`, no CDC, no re-fire).
/// Hosted only on a clock-injected boot; a `None`-clock component would fail
/// loud rather than silently no-op.
#[async_trait::async_trait(?Send)]
impl SutClockAdvance for HeadlessFrontendComponent {
    async fn advance_clock_days(&self, days: i64) -> String {
        let clock = self
            .clock
            .as_ref()
            .expect("SutClockAdvance requires an injected TestClock (new_with_clock boot)");
        const MS_PER_DAY: i64 = 86_400_000;
        clock.advance(days * MS_PER_DAY);
        holon::sync::clock_scheduler::reconcile_clock(self.engine.db_handle(), clock.as_ref())
            .await
            .expect("reconcile_clock after advancing the injected clock");
        holon_api::CalendarDate::from_clock(clock.as_ref()).ymd()
    }
}

/// `SutHistoryWrite` (the `UndoLastMutation` / `Redo` transitions): undo/redo
/// the last committed mutation through the production `BackendEngine` undo
/// stack — the same `engine().undo()/redo()` path `E2ESut` drives. The
/// `ref_state`-dependent block-convergence settle lives in the harness seam
/// (`block_tree_post_action`), so the cap is a pure `&self` action here.
#[async_trait::async_trait(?Send)]
impl SutHistoryWrite for HeadlessFrontendComponent {
    async fn undo_last_mutation(&self) {
        tracing::trace!("[apply] UndoLastMutation");
        let result = self.engine.undo().await;
        assert!(result.is_ok(), "undo failed: {:?}", result.err());
        assert!(
            result.unwrap().applied(),
            "undo returned non-applied (nothing to undo or stale)"
        );
    }

    async fn redo(&self) {
        tracing::trace!("[apply] Redo");
        let result = self.engine.redo().await;
        assert!(result.is_ok(), "redo failed: {:?}", result.err());
        assert!(
            result.unwrap().applied(),
            "redo returned non-applied (nothing to redo or stale)"
        );
    }
}

/// `SutNavHistoryDrive` (the
/// `NavigateBack`/`NavigateForward`/`PinBlock`/`UnpinBlock` transitions) over
/// the **production** navigation provider ops, dispatched through
/// the windowless `FrontendSession` — the same `execute_operation("navigation",
/// …)` path `SutFocusWrite` (focus) and `SutNavHistoryWrite` (go_home) already
/// drive. `E2ESut` reaches these ops via the GPUI driver's `synthetic_dispatch`
/// / leader-chords; headlessly there is no driver, but every op (`go_back`,
/// `go_forward`, `focus_pin`, `close`) is a `navigation` provider op
/// (`holon/src/navigation/provider.rs`), so the session dispatches them
/// directly. Note: this realizes the *drive* path (op reachable + applied);
/// whether the headless reactive engine mirrors back/forward history
/// *semantics* into the nav matviews to oracle parity is a Phase-B concern,
/// probed separately.
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

/// `SutMutate` over the headless engine. Only `toggle_state` does real work —
/// and FAITHFULLY: it dispatches the production `block`/`cycle_task_state` op
/// (the one Cmd+Enter / the `state_toggle` widget fires) `click_count` times,
/// computed from the current `task_state` exactly like E2ESut's
/// `apply_toggle_state_to_sut`. This
/// drives `LoroBlockOperations::cycle_task_state` → the Loro authority doc →
/// the `block_raw` projection, rather than a `set_field` shortcut that bypasses
/// the real cycle. Combined with the read-doc unification (`compose_sut` builds
/// the Loro read cap over the frontend's authority doc),
/// `inv-task-state-storage-coherence` and `inv-blocks-match-ref` stay in
/// lockstep with `ToggleState::apply_to_ref`. `apply_mutation`/
/// `bulk_external_add` are faithful `&self` no-ops, EXACTLY as on
/// `E2ESut`: their real, `ref_state`-dependent dispatch lives in the
/// `block_tree_post_action` seam, which the composed harness does not yet
/// rebuild — so those transitions stay out of the composed alphabet (driving
/// them would diverge), while `ToggleState` drives faithfully.
impl HeadlessFrontendComponent {
    /// Shared `ToggleState` click-count math: how many `state_toggle` clicks
    /// advance the cycle from the pre-mutation state to `new_state`. Read
    /// `current` from the settled projection (== the Loro doc at a settled
    /// point). Fail loud on a no-op toggle (the generator excludes them).
    async fn toggle_click_count(&self, id: &EntityUri, new_state: CycleTarget) -> u8 {
        let current = self.block_task_state(id).await.unwrap_or_default();
        let click_count = cycle_click_count(&current, new_state);
        assert!(
            click_count > 0,
            "[toggle_state] click_count=0 ({current:?} == {new_state:?}) — the generator should \
             exclude no-op toggles"
        );
        click_count
    }

    /// `ToggleState` driven through an explicit `driver` (§8.12 C-3 mechanism
    /// 2). CLICK the `state_toggle` widget `click_count` times — the
    /// faithful user gesture the generic `apply_toggle_state_to_sut` runs —
    /// so the windowed sibling drives the real window `state_toggle` while
    /// headless keeps the direct-dispatch path below. Each click's bound
    /// `cycle_task_state` reads the current state off the Loro backend and
    /// advances by one, so the up-front `click_count` is stable across the
    /// loop.
    pub(crate) async fn toggle_state_via(
        &self,
        driver: &dyn UserDriver,
        block_id: &EntityUri,
        new_state: CycleTarget,
    ) {
        let id = self.resolve_id(block_id);
        let click_count = self.toggle_click_count(&id, new_state).await;
        let mut current = self.block_task_state(&id).await.unwrap_or_default();
        for n in 0..click_count {
            // Click the `state_toggle` GLYPH (not a plain row click, which would
            // just focus): `cycle_state_toggle` targets the widget's own
            // `set_field` cycle dispatch — geometry hit-test on a window driver,
            // resolved-tree intent on the headless driver.
            driver
                .cycle_state_toggle(&id, "main")
                .await
                .unwrap_or_else(|e| {
                    panic!("[toggle_state] click #{} failed for {id}: {e:#}", n + 1)
                });
            // WAIT until the projection shows THIS click landed before the
            // next one, RE-CLICKING while the resolved view is stale. Each
            // click's intent computes `next` from the resolved view's
            // `current` prop; while that view still serves the pre-click
            // value, a click re-dispatches the SAME keyword — a no-op write
            // that can never advance the cycle (the stale-read double
            // dispatch `inv-viewmodel-state-toggle-correct` caught when
            // ToggleState first fired in the keystone: DOING != DONE, and
            // the pure block_raw-settle variant of this loop then hung on a
            // no-op click). Stale re-clicks are idempotent (same value), and
            // the first click after the view refreshes advances exactly one
            // step — a user hammering an unresponsive toggle sees the same.
            // The generator excludes no-op toggles, so every landed click
            // changes `task_state`.
            let overall = tokio::time::Instant::now() + soak_deadline(Duration::from_secs(10));
            'landing: loop {
                let attempt_deadline =
                    (tokio::time::Instant::now() + Duration::from_millis(500)).min(overall);
                while tokio::time::Instant::now() < attempt_deadline {
                    let now_state = self.block_task_state(&id).await.unwrap_or_default();
                    if now_state != current {
                        current = now_state;
                        break 'landing;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                assert!(
                    tokio::time::Instant::now() < overall,
                    "[toggle_state] click #{} never landed for {id} (task_state still {current:?} \
                     after 10s of re-clicks)",
                    n + 1
                );
                driver
                    .cycle_state_toggle(&id, "main")
                    .await
                    .unwrap_or_else(|e| {
                        panic!("[toggle_state] re-click #{} failed for {id}: {e:#}", n + 1)
                    });
            }
        }
    }
}

impl HeadlessFrontendComponent {
    /// Settle barrier shared by the seam-mutate methods: poll `block_raw` until
    /// its id-set is stable across two consecutive reads (the live
    /// `FileSyncController` finished re-ingesting the org write). Same
    /// shape as `simulate_restart`'s settle.
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
    /// re-serialized file reproduces the order a faithful external rewrite
    /// sees.
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

/// Resolve a mutation's *referenced* ids (oracle synthetic → SUT real) via
/// `resolve`, so `Mutation::apply_to` matches the live `block_raw` rows. A
/// `Create`'s NEW id is left as-is (born-equal: the org write carries it in an
/// `:ID:` drawer and both sides agree).
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

/// `SutSeamMutate` over the headless component — the real composed equivalent
/// of the `E2ESut` `block_tree_post_action` seam (which is `ref_state`-driven).
/// Both methods rewrite the seeded USER docs' org files and let the live
/// `FileSyncController` re-ingest — `documents` excludes the layout
/// `index.org`, so a full rewrite is safe. `ref_state`-free: the post-state is
/// reconstructed from the live `block` matview snapshot plus the typed
/// transition args, not the oracle. Hosting this un-narrows `ApplyMutation`'s
/// External arm AND `BulkExternalAdd` onto the composed alphabet. (The
/// `BulkExternalAdd` Flutter-startup concurrent-watch race the `E2ESut` seam
/// adds is NOT replicated here — it is a startup-scheduler probe already gated
/// by `phantom_loro_exists_repro`; the composed catalog verifies the blocks
/// landed every tick.)
#[async_trait::async_trait(?Send)]
impl SutSeamMutate for HeadlessFrontendComponent {
    async fn apply_mutation(&self, event: MutationEvent) {
        use holon_filesystem::FileSystem;
        let resolved = resolve_mutation_ids(&event.mutation, &|id| self.resolve_id(id));
        // Source from the `block` MATVIEW (`live_block_snapshot`), NOT the base
        // `block_raw` table (`all_blocks`): block_raw has no `tags` column, so
        // the doc block's `Page` tag is lost and `blocks_by_document` finds no
        // page → it would serialize an EMPTY org file and the live re-ingest
        // would WIPE the whole tree. The matview carries tags/requires, so
        // page-ness (and any other edge fields) round-trip faithfully.
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
        // (matched born-equal). Only resolve parent refs to pre-existing entities (the
        // doc); refs to sibling bulk-N-k stay (born too, present in `current`
        // after this loop). Matview snapshot (not `all_blocks`) so the doc
        // block's `Page` tag survives — see `apply_mutation` above for why
        // block_raw's missing tags would wipe the tree.
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

/// `SutBlockCreate` over the headless component — the composed home for the
/// `CreateBlockUnderFocus` transition. Delegates to the PRODUCTION
/// creation-slot commit seam on this component's own `ReactiveEngineDriver`
/// (`commit_creation_slot` → `ViewEventHandler::handle_text_sync` →
/// `block.create`), so the "type here to create" gesture materializes a block
/// under the query's focus root exactly as GPUI's on-blur does — the parent is
/// read from the live `:__virtual:<parent>` slot id WP-E resolved, never
/// re-derived here. The one minted block pairs 1:1 with the oracle's synthetic
/// `block::create-N` via the harness's generic per-tick id reconcile. Fails
/// loud (never a silent no-op) if no slot renders or the commit yields no
/// intent.
#[async_trait::async_trait(?Send)]
impl SutBlockCreate for HeadlessFrontendComponent {
    async fn apply_create_under_focus(
        &self,
        parent: &EntityUri,
        content: &str,
        id: Option<&EntityUri>,
    ) {
        match id {
            // Explicit id: no slot gesture exists for a born-equal id, so dispatch
            // the production `block.create{id, parent_id, content}` intent directly
            // under the ref-resolved focus root.
            Some(uri) => self
                .driver
                .create_block_with_id(parent, content, uri)
                .await
                .unwrap_or_else(|e| panic!("[SutBlockCreate::apply_create_under_focus] {e:#}")),
            // No id: drive the PRODUCTION creation-slot gesture EXACTLY as today —
            // it re-resolves the parent from its own live rendered rowset (WP-E
            // focus-root cross-check preserved) and mints via `block.create`.
            None => self
                .driver
                .commit_creation_slot(content)
                .await
                .map(|_| ())
                .unwrap_or_else(|e| panic!("[SutBlockCreate::apply_create_under_focus] {e:#}")),
        }
        self.settle_block_ids_stable(Duration::from_secs(5)).await;
    }
}

/// `SutAppLifecycle` over the headless component — the seam-rebuild entry
/// point. Only `create_document` is realized so far: it writes an empty org
/// file into the session's watched `org_root` (the production
/// `FileSyncController` watcher then ingests it and mints the page block in
/// `block_raw`), the headless analogue of `TestContext::create_document`. No
/// `ref_state` is read — the synthetic→real doc-uri reconcile is the composed
/// harness's generic per-tick id reconcile (the minted page is one new
/// `block_raw` id paired 1:1 with the oracle's one new synthetic `block:
/// ref-doc-N`). The action only WAITS until that page actually lands so the
/// harness's post-apply id snapshot observes it (mirrors `TestContext`'s
/// `resolve_page_uri_by_name` poll). `simulate_restart` and
/// `concurrent_schema_init` ARE dispatched by the composed alphabet (both
/// ported); `start_app` is not part of any composed alphabet yet
/// (lifecycle/deferred-boot is a later increment) — it fails loud if ever
/// dispatched.
#[async_trait::async_trait(?Send)]
impl SutAppLifecycle for HeadlessFrontendComponent {
    async fn start_app(&self, _: EntityUri, _: bool, _: bool, _: bool, _: bool) {
        unimplemented!(
            "[SutAppLifecycle::start_app] not yet ported to HeadlessFrontendComponent — lifecycle \
             (deferred-boot) is a later seam-rebuild increment; StartApp is not in any composed \
             alphabet"
        );
    }

    async fn simulate_restart(&self) {
        use holon_filesystem::FileSystem;
        // Faithful to `E2ESut`/`TestEnvironment::simulate_restart` (which is itself a
        // file-touch, NOT a true reboot): re-trigger the production
        // `FileSyncController` watcher by touch-writing each tracked org file
        // (append a space, settle, restore), forcing a re-parse. Blocks are
        // PRESERVED — the `:ID:` drawers persisted on disk make the re-parse
        // id-stable — so `SimulateRestart::apply_to_ref` is a no-op and
        // this only re-exercises the ingest path. The post-action block-convergence
        // settle (E2ESut's `wait_for_blocks_synced`, relocated to the seam)
        // lives HERE in the cap since the composed harness has no seam: poll
        // `block_raw` to a stable id-set.
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

        // Settle: poll until the block_raw id-set is stable across two consecutive
        // reads.
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

        // Wait for the production `FileSyncController` watcher to ingest the new file
        // and mint the doc block in `block_raw` (the convergence
        // `TestContext::create_document` polls for via
        // `resolve_page_uri_by_name`). The doc block's title is the file stem —
        // exactly what `CreateDocument::apply_to_ref` sets the oracle page's content to
        // — so poll the `block_raw` snapshot for a block with that title. (NB:
        // `is_page()` is false on these projected rows — page-ness is a
        // `block_tags` Page tag, not a `Block` field post-projection — so match
        // on title alone.) Self-contained: no `ref_state`, no resolver; the
        // harness reconcile maps the minted id afterwards.
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
                "[SutAppLifecycle::create_document] timeout waiting for the doc block (title \
                 {stem:?}) to land in block_raw after writing {file_name}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        // Track the new doc so a later `BulkExternalAdd` / External `ApplyMutation`
        // targeting it resolves a file path. `doc_id` is the minted doc-page
        // block (title == file stem) — the SAME real id the harness reconcile
        // maps the oracle's `block:ref-doc-N` to (the single block minted by
        // this ingest), so the seam lookup keyed on `resolve_id(...)`
        // hits. Idempotent: skip if already tracked (re-create of the same file).
        {
            let mut docs = self.documents.lock().expect("documents lock");
            if !docs.iter().any(|(u, _)| *u == doc_id) {
                docs.push((doc_id, file_path.clone()));
            }
        }
    }

    async fn delete_document(&self, file_name: &str) {
        use holon_filesystem::FileSystem;
        let file_path = self.org_root.join(file_name);
        FileSystem::remove(self.org_fs.as_ref(), &file_path)
            .await
            .unwrap_or_else(|e| {
                panic!("[SutAppLifecycle::delete_document] remove {file_name} failed: {e:#}")
            });

        // Inverse of `create_document`'s poll: wait for the page block whose
        // title is the file stem to VANISH from `block_raw`. The removal went
        // through the bare fs port — the harness analog of the user deleting
        // the file OUTSIDE Holon (the scenario the prod bug was observed in) —
        // and the in-memory fs emitted a `Remove` change, so the production
        // `FileSyncController::on_file_deleted` cascade ran for the vanished
        // path. Fail loud on timeout — it means that cascade regressed and
        // blocks linger after an external deletion.
        let stem = std::path::Path::new(file_name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(file_name)
            .to_string();
        let timeout = Duration::from_secs(5);
        let start = std::time::Instant::now();
        loop {
            if !self
                .all_blocks()
                .await
                .into_iter()
                .any(|b| b.title() == stem)
            {
                break;
            }
            assert!(
                start.elapsed() < timeout,
                "[SutAppLifecycle::delete_document] timeout: the doc block (title {stem:?}) is \
                 still in the block set 5s after removing {file_name} — prod is not deleting \
                 blocks when an org file is deleted outside Holon (regression of \
                 FileSyncController::on_file_deleted's cascade)"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // Untrack the doc so later file-seam lookups don't resolve a dead path.
        self.documents
            .lock()
            .expect("documents lock")
            .retain(|(_, p)| *p != file_path);
    }

    async fn concurrent_schema_init(&self) {
        // Ported from the E2ESut/`SutHandle` impl: the regression this guards is the
        // double-`ensure_navigation_schema` "database is locked" bug — sequential
        // schema ops (each `query_and_watch` creates a matview) must NOT lock
        // the DB. The test's only ASSERTION is the absence of a "database is
        // locked" error; other errors (e.g. transient "Database schema changed"
        // from concurrent IVM) are tolerated. `apply_to_ref` is a no-op, so
        // this must not perturb the compared projections — the watches created
        // here are anonymous (`None` query id), not the named watches
        // `SutWatchRegister`/`inv-active-watches-match-ref` track.
        let engine = self.engine();
        for i in 0..3 {
            let prql =
                format!("from block_raw | select {{id, content}} | filter id != \"dummy-{i}\" ");
            let sql = engine
                .compile_to_sql(&prql, QueryLanguage::HolonPrql)
                .expect("ConcurrentSchemaInit: PRQL compilation should succeed");
            if let Err(e) = engine
                .query_and_watch(sql, std::collections::HashMap::new(), None)
                .await
            {
                let error_str = format!("{e:?}");
                assert!(
                    !error_str.contains("database is locked"),
                    "DATABASE LOCK BUG: sequential query_and_watch {i} hit 'database is locked' — \
                     ensure_navigation_schema is being called concurrently again: {error_str}"
                );
            }
        }
        for _ in 0..2 {
            if let Err(e) = engine
                .execute_query(
                    "SELECT id FROM block_raw LIMIT 1".to_string(),
                    std::collections::HashMap::new(),
                    None,
                )
                .await
            {
                let error_str = format!("{e:?}");
                assert!(
                    !error_str.contains("database is locked"),
                    "DATABASE LOCK BUG: sequential simple query hit 'database is locked': \
                     {error_str}"
                );
            }
        }
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

/// `SutFixtureFs` over the headless component — the org-file fixture rung.
/// Only `write_org_file` is realized: it writes rendered org into the session's
/// watched `org_root` (the production `FileSyncController` watcher ingests it,
/// the same path `create_document` exercises), which is what makes
/// `WriteOrgFile` — and with it the advice-rule minting arm (ADR 0022 step 4) —
/// reachable in the composed keystone at all. The other four fixture ops
/// (`create_directory`, `git_init`, `jj_git_init`, `create_stale_loro`) back
/// transitions that are all gated on `!app_started`; the composed keystone's
/// oracle boots pre-started (`build_started_ref`), so they can never be
/// dispatched here — fail loud if that ever changes (same convention as
/// `start_app` above).
#[async_trait::async_trait(?Send)]
impl holon_pbt_core::capabilities::SutFixtureFs for HeadlessFrontendComponent {
    async fn write_org_file(&self, filename: &str, content: &str) {
        use holon_filesystem::FileSystem;
        let file_path = self.org_root.join(filename);
        if let Some(parent) = file_path.parent() {
            self.org_fs.mkdir_all(parent);
        }
        FileSystem::write(self.org_fs.as_ref(), &file_path, content.as_bytes())
            .await
            .unwrap_or_else(|e| {
                panic!("[SutFixtureFs::write_org_file] write {filename} failed: {e:#}")
            });

        // Wait for the watcher ingest to mint the doc-page block (title == file
        // stem — `WriteOrgFile::apply_to_ref` gives the oracle page the same
        // content), mirroring `SutAppLifecycle::create_document`'s poll; then
        // settle the full block id-set so the content blocks landed too.
        let stem = std::path::Path::new(filename)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(filename)
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
                "[SutFixtureFs::write_org_file] timeout waiting for the doc block (title \
                 {stem:?}) to land in block_raw after writing {filename}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        self.settle_block_ids_stable(Duration::from_secs(5)).await;

        // Track the doc for later file-seam lookups (BulkExternalAdd / External
        // ApplyMutation) — idempotent for a same-file rewrite.
        let mut docs = self.documents.lock().expect("documents lock");
        if !docs.iter().any(|(u, _)| *u == doc_id) {
            docs.push((doc_id, file_path));
        }
    }

    async fn create_directory(&self, path: &str) {
        unimplemented!(
            "[SutFixtureFs::create_directory] CreateDirectory is `!app_started`-gated and the \
             composed oracle boots pre-started — unreachable until a deferred-boot increment \
             lands (path: {path})"
        );
    }

    async fn git_init(&self) {
        unimplemented!(
            "[SutFixtureFs::git_init] GitInit is `!app_started`-gated and the composed oracle \
             boots pre-started — unreachable until a deferred-boot increment lands"
        );
    }

    async fn jj_git_init(&self) {
        unimplemented!(
            "[SutFixtureFs::jj_git_init] JjGitInit is `!app_started`-gated and the composed \
             oracle boots pre-started — unreachable until a deferred-boot increment lands"
        );
    }

    async fn create_stale_loro(
        &self,
        org_filename: &str,
        _: holon_pbt_core::types::LoroCorruptionType,
    ) {
        unimplemented!(
            "[SutFixtureFs::create_stale_loro] CreateStaleLoro is `!app_started`-gated and the \
             composed oracle boots pre-started — unreachable until a deferred-boot increment \
             lands (file: {org_filename})"
        );
    }
}

#[async_trait::async_trait(?Send)]
impl SutErrorLog for HeadlessFrontendComponent {
    /// Flutter/event publish errors logged during the initial document sync —
    /// the SAME production `FrontendSession` publish-error tracker `E2ESut`
    /// read.
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

// Direct gesture-write trait impls over the component's OWN headless driver —
// thin wrappers on the `*_via` bodies. Registered CapMaps route these caps
// through `DriverBoundFrontendWrite` (`register_gesture_writes`); these direct
// impls exist for the make-or-break PROBE tests (and the `assert_cap_union`
// compile check) that exercise the component directly rather than through a
// composed `CapMap`. Same bodies, bound to `self.driver()`. `SutMutate` is
// intentionally NOT among them — no direct caller needs it, so `state_toggle`
// toggling has a single path (the shim's `toggle_state_via`).
#[async_trait::async_trait(?Send)]
impl SutFocusWrite for HeadlessFrontendComponent {
    // ALLOW(unused_param): region is fixed to main by the click-driven focus path
    async fn apply_navigate_focus(&self, _region: CapRegion, id: &EntityUri) {
        self.apply_navigate_focus_via(self.driver.as_ref(), id)
            .await;
    }

    async fn apply_focus_editable_text(&self, id: &EntityUri) {
        self.apply_focus_editable_text_via(self.driver.as_ref(), id)
            .await;
    }
}

#[async_trait::async_trait(?Send)]
impl SutEditorMirrorWrite for HeadlessFrontendComponent {
    async fn apply_type_chars(&self, text: &str) {
        self.apply_type_chars_via(self.driver.as_ref(), text).await;
    }

    async fn apply_delete_backward(&self, count: usize) {
        self.apply_delete_backward_via(self.driver.as_ref(), count)
            .await;
    }

    async fn apply_move_cursor(&self, byte_position: usize) {
        self.apply_move_cursor_via(self.driver.as_ref(), byte_position)
            .await;
    }
}

impl CapProvider for HeadlessFrontendComponent {
    /// Backward-compatible entry: the non-gesture caps PLUS the gesture-write
    /// caps bound to this component's OWN headless driver. Direct callers
    /// (lib slices that build a CapMap via `Config`/`register` and never
    /// attach a window) get the full set. The composed builder instead
    /// calls [`Self::register_non_gesture`] +
    /// [`GestureWriteSource::register_gesture_writes`] separately so the
    /// windowed `DriverPlacement::Deferred` base can withhold the gesture
    /// rung until a window exists (§8.12 insert-only overlay).
    fn register(self: Arc<Self>, caps: &mut CapMap) {
        let driver = self.driver();
        self.clone().register_non_gesture(caps);
        self.register_gesture_writes(caps, driver);
    }
}

impl HeadlessFrontendComponent {
    /// Every cap this component provides EXCEPT the driver-bound gesture-write
    /// family (`SutBlockTreeWrite`/`SutFocusWrite`/`SutEditorMirrorWrite`/
    /// `SutMutate`), which is enumerated once in
    /// [`GestureWriteSource::register_gesture_writes`]. Split out
    /// so `DriverPlacement` can gate the gesture rung (the windowed base
    /// registers the reads/projections here and inserts the gesture writes
    /// only once a window exists).
    pub(crate) fn register_non_gesture(self: Arc<Self>, caps: &mut CapMap) {
        caps.insert(self.clone() as Arc<dyn SutErrorLog>);
        caps.insert(self.clone() as Arc<dyn SutRenderer>);
        caps.insert(self.clone() as Arc<dyn SutViewSelection>);
        caps.insert(self.clone() as Arc<dyn SutBackend>);
        caps.insert(self.clone() as Arc<dyn SutWatch>);
        caps.insert(self.clone() as Arc<dyn SutOrgRead>);
        // `SutAdviceMatview` — the SQL-level advice twin's SUT read. Selecting
        // `inv-advice-matview-matches-ref` here is intended (unlike
        // `SutSqlProjection` below): the twin must run wherever the block matviews
        // do so the driver-ladder localization (matview twin green vs weave red)
        // holds. Pre-step-6 it observes no `advice_rule_%` matview.
        caps.insert(self.clone() as Arc<dyn SutAdviceMatview>);
        // `SutNavHistoryWrite` (go_home) — selection-neutral write cap (no invariant
        // `Needs` it), it just lets the `NavigateHome` transition drive this component
        // through `apply_to_sut(&mut CapMap)`. `SutSqlProjection` is deliberately NOT
        // registered here (it would newly select `block_content_sql`); the navigation
        // slice adds it on its own CapMap.
        caps.insert(self.clone() as Arc<dyn SutNavHistoryWrite>);
        // `SutWatchRegister` (setup_watch) — same selection-neutral rationale: no
        // invariant `Needs` a write cap; it lets the `SetupWatch` transition drive
        // this component's production reactive watch surface through
        // `apply_to_sut(&mut CapMap)` (SutHandle decomposition INC 3). The watch
        // *read* cap (`SutWatch`) is already registered above, so a slice that
        // also supplies `RefWatch` makes the B5 watch invariants bite over a
        // composed-driven watch.
        caps.insert(self.clone() as Arc<dyn SutWatchRegister>);
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
        // `SutSeamMutate` over the live `FileSyncController`: the real composed home
        // for `ApplyMutation`'s External (org) arm and `BulkExternalAdd`,
        // un-narrowing both onto any frontend CapMap. A write cap (no invariant
        // `Needs` it), safe here.
        caps.insert(self.clone() as Arc<dyn SutSeamMutate>);
        // `SutBlockCreate` (CreateBlockUnderFocus) — creation-slot gesture over this
        // component's own `ReactiveEngineDriver` commit seam. A write cap (no invariant
        // `Needs` it); its presence makes the WP-E creation-slot create cap-feasible on
        // any frontend CapMap, so the transition auto-narrows to configs whose default
        // layout renders the `creation_slot: true` collection.
        caps.insert(self.clone() as Arc<dyn SutBlockCreate>);
        // `SutAppLifecycle` (CreateDocument) — selection-neutral lifecycle cap (no
        // invariant `Needs` it); lets the `CreateDocument` transition mint a doc
        // through `apply_to_sut(&mut CapMap)`. Only `create_document` is
        // realized; the other lifecycle methods fail loud (not in any composed
        // alphabet yet). This is the seam-rebuild entry point — the
        // synthetic→real doc-uri mapping is the harness's generic per-tick
        // reconcile, not E2ESut's `block_tree_post_action`.
        caps.insert(self.clone() as Arc<dyn SutAppLifecycle>);
        caps.insert(self as Arc<dyn SutOrgRender>);
    }
}

impl HeadlessFrontendComponent {
    /// The driver-parameterized registration of the gesture-write cap family —
    /// the ONE place that enumerates which caps are gesture writes. Given a
    /// `UserDriver`, it binds all of them to that driver: the headless base
    /// passes its own `ReactiveEngineDriver` (§8.11 VM rung); the windowed
    /// overlay passes the live window's `GpuiUserDriver`/`SimUserDriver`
    /// (§8.11 highest-available). Both go through the SAME production
    /// keystroke/click/toggle bodies via [`DriverBoundFrontendWrite`] — only
    /// the driver changes. This is the insert-only realization the §8.12
    /// C-3 windowed repoint wants: the deferred base registers none of
    /// these, the overlay `insert`s them once.
    pub(crate) fn register_gesture_writes(
        self: Arc<Self>,
        caps: &mut CapMap,
        driver: Arc<dyn UserDriver>,
    ) {
        use crate::pbt::op_write_cap::OpDispatchWriter;
        // `SutBlockTreeWrite`: the production keystroke pipeline over `driver` when a
        // reconcile resolver is wired (the composed/windowed builder shares one so a
        // split-minted id maps oracle→real). When no resolver is set — the fixed-id lib
        // slices that build via `register` and never mint — fall back to the plain
        // `OpDispatchWriter` dispatch floor (behaviour-identical to the former
        // default), so `keystroke_writer_with`'s fail-loud resolver assert
        // never trips here.
        let block_tree: Arc<dyn SutBlockTreeWrite> = if self.resolver.get().is_some() {
            Arc::new(self.keystroke_writer_with(driver.clone()))
        } else {
            Arc::new(OpDispatchWriter::new(self.engine.clone()))
        };
        caps.insert(block_tree);
        // `SutFocusWrite`/`SutEditorMirrorWrite`/`SutMutate`: the driver-bound shim
        // that delegates to the component's `*_via` bodies (sidebar-click
        // focus, editor keystrokes, `state_toggle` clicks) so the whole family
        // rides ONE driver.
        let shim = Arc::new(DriverBoundFrontendWrite::new(self.clone(), driver));
        caps.insert(shim.clone() as Arc<dyn SutFocusWrite>);
        caps.insert(shim.clone() as Arc<dyn SutEditorMirrorWrite>);
        caps.insert(shim as Arc<dyn SutMutate>);
    }
}

/// Rebinds the frontend's gesture-write caps ([`SutFocusWrite`],
/// [`SutEditorMirrorWrite`], [`SutMutate`]) onto a chosen [`UserDriver`] by
/// delegating every method to the base [`HeadlessFrontendComponent`]'s
/// driver-parameterized `*_via` bodies — the sidebar-click / editor-keystroke /
/// `state_toggle`-click logic is driver-invariant, so no logic is
/// re-implemented, only the driver is swapped. Used for BOTH the headless base
/// (driver = the component's own `ReactiveEngineDriver`) and the
/// windowed overlay (driver = the window's `GpuiUserDriver`/`SimUserDriver`),
/// §8.12 C-3.
///
/// [`SutMutate`]: holon_pbt_core::capabilities::SutMutate
pub(crate) struct DriverBoundFrontendWrite {
    base: Arc<HeadlessFrontendComponent>,
    driver: Arc<dyn UserDriver>,
}

impl DriverBoundFrontendWrite {
    pub(crate) fn new(base: Arc<HeadlessFrontendComponent>, driver: Arc<dyn UserDriver>) -> Self {
        Self { base, driver }
    }
}

#[async_trait::async_trait(?Send)]
impl SutFocusWrite for DriverBoundFrontendWrite {
    // ALLOW(unused_param): region is fixed to main by the click-driven focus path
    async fn apply_navigate_focus(&self, _region: CapRegion, id: &EntityUri) {
        self.base
            .apply_navigate_focus_via(self.driver.as_ref(), id)
            .await;
    }

    async fn apply_focus_editable_text(&self, id: &EntityUri) {
        self.base
            .apply_focus_editable_text_via(self.driver.as_ref(), id)
            .await;
    }
}

#[async_trait::async_trait(?Send)]
impl SutEditorMirrorWrite for DriverBoundFrontendWrite {
    async fn apply_type_chars(&self, text: &str) {
        self.base
            .apply_type_chars_via(self.driver.as_ref(), text)
            .await;
    }

    async fn apply_delete_backward(&self, count: usize) {
        self.base
            .apply_delete_backward_via(self.driver.as_ref(), count)
            .await;
    }

    async fn apply_move_cursor(&self, byte_position: usize) {
        self.base
            .apply_move_cursor_via(self.driver.as_ref(), byte_position)
            .await;
    }
}

#[async_trait::async_trait(?Send)]
impl SutMutate for DriverBoundFrontendWrite {
    async fn toggle_state(&self, block_id: &EntityUri, new_state: CycleTarget) {
        self.base
            .toggle_state_via(self.driver.as_ref(), block_id, new_state)
            .await;
    }
}

#[cfg(test)]
mod tests {
    use holon_api::EntityName;
    use holon_api::Value;

    use super::*;

    /// A0 make-or-break PROBE (full-mode peer mesh): when the headless frontend
    /// boots with Loro ON (the `full_headless` shape), does its DI injector
    /// eventually resolve a `LoroSyncControllerHandle`, and is the controller
    /// watching the SAME global doc the builder's Loro arm reads? Resolution is
    /// RACE-prone (`without_wait()` → spawned `post_ready_work`,
    /// `wiring.rs:360`), so this POLLS for readiness rather than
    /// snapshotting — a flaky one-shot `assert Ok` would be the wrong
    /// probe. If the handle never resolves headless, full-mode peer
    /// projection has no controller and Part A must fall back to an
    /// explicit `project()` drive (don't fake it — surface the absence).
    #[tokio::test(flavor = "multi_thread")]
    async fn headless_loro_sync_controller_resolves_after_boot() {
        const TREE_ORG: &str = "#+ID: structural-page\n* parent\n:PROPERTIES:\n:ID: \
                                parent\n:END:\n* c1\n:PROPERTIES:\n:ID: c1\n:END:\n";
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
            "[A0-probe] LoroSyncControllerHandle never resolved headless within 2s — full-mode \
             peer projection has no controller; fall back to explicit project()"
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
            "[A0-probe] get_global_doc() must return the cached doc (same Arc), else a peer \
             import would not wake the controller"
        );
        eprintln!("[A0-probe] global doc is cached/shared ✓");
    }

    // ── ADR 0024 §6 capstone: the AdvanceDay property as a directed test ──────
    //
    // Boots the real headless frontend over the FULL journal rule (trigger joins
    // the `clock` relation, action is `holon_rule`) with an INJECTED fake clock,
    // then drives day-rollover through `SutClockAdvance` and asserts the §6
    // invariant directly: N advances spanning D distinct days ⇒ exactly D journal
    // day-blocks (one per day, deterministic id), re-ticking the same day adds
    // nothing, and each journal block is ordinary content (not program-marked).
    // This exercises the whole prod path — injected clock → scheduler reconcile →
    // clock CDC → journal trigger matview → action watcher → `block.create` with a
    // WP2 deterministic id — so the composed generator rolling few AdvanceDay steps
    // never leaves the property un-driven.

    /// A fixed local-noon millis for `y-m-d`, timezone-robust (noon UTC lands
    /// on the same civil date from UTC-12..+12), so the boot day is
    /// deterministic regardless of the test host's TZ.
    fn noon_millis(y: i32, m: u32, d: u32) -> i64 {
        chrono::NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis()
    }

    /// The journal DAY children under `block:journals`: `(id, name)` pairs
    /// whose `name` parses as a `YYYY-MM-DD` date. Excludes the rule's
    /// source blocks and the `Journal Auto-Create` heading (also children
    /// of `block:journals`, but not date-named).
    async fn journal_day_children(comp: &HeadlessFrontendComponent) -> Vec<(String, String)> {
        // ORG/RENDER truth: a heading's text IS its `content` — the date the
        // action creates lands in the block's top-level `content` column (not a
        // `name` property), so it renders as a non-empty row and org-round-trips
        // to a `* <date>` headline (Bug-3 ORACLE fix). Read `content` here.
        let rows = comp
            .engine
            .db_handle()
            .query(
                "SELECT id, content FROM block_raw WHERE parent_id = 'block:journals'",
                std::collections::HashMap::new(),
            )
            .await
            .expect("journal children query");
        rows.iter()
            .filter_map(|r| {
                let id = r.get("id").and_then(|v| v.as_string())?.to_string();
                let content = r.get("content").and_then(|v| v.as_string())?.to_string();
                holon_api::CalendarDate::parse(&content)
                    .ok()
                    .map(|_| (id, content))
            })
            .collect()
    }

    /// Poll until the journal day-block count reaches `expected` (the rule
    /// fires asynchronously off the CDC path), failing loud on timeout with
    /// the current rows so a divergence is legible.
    async fn wait_for_journal_days(
        comp: &HeadlessFrontendComponent,
        expected: usize,
        timeout: Duration,
    ) -> Vec<(String, String)> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let days = journal_day_children(comp).await;
            if days.len() == expected {
                return days;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "expected {expected} journal day-blocks, still {} after timeout: {days:?}",
                days.len()
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// The FULL journal rule seed: `#+ID: journals` fixes the page id to
    /// `block:journals`; the trigger/action pair lives under a `Journal
    /// Auto-Create` heading (same parent, so `action_discovery` joins them)
    /// and NOT directly under journals, so the day-blocks the action
    /// creates are the only date-content children.
    const JOURNAL_RULE_ORG: &str = "#+ID: journals\n* Journal Auto-Create\n#+BEGIN_SRC holon_sql \
                                    :id journals::trigger::0\nSELECT today as name FROM clock \
                                    WHERE grain = 'day'\n#+END_SRC\n#+BEGIN_SRC holon_rule :id \
                                    journals::action::0\nblock.create(#{parent_id: \
                                    \"block:journals\", content: col(\"name\")})\n#+END_SRC\n";

    #[tokio::test(flavor = "multi_thread")]
    async fn advance_day_fires_one_journal_per_distinct_day_idempotently() {
        use holon_pbt_core::capabilities::SutClockAdvance;

        let boot_ms = noon_millis(2026, 1, 15);
        let clock = Arc::new(holon_api::TestClock::new(boot_ms));
        let boot_date = holon_api::CalendarDate::from_clock(clock.as_ref()).ymd();

        let comp = HeadlessFrontendComponent::new_with_clock(
            &[("Journals.org", JOURNAL_RULE_ORG)],
            Duration::from_millis(500),
            false, // SqlOnly-shaped: no Loro. The rule path is storage-agnostic.
            clock.clone(),
        )
        .await;

        // Boot fired the rule once for the boot day.
        let boot_days = wait_for_journal_days(&comp, 1, Duration::from_secs(10)).await;
        assert_eq!(boot_days[0].1, boot_date, "boot journal is the boot day");
        eprintln!("[advance-day] boot day journal: {boot_days:?}");

        // ORACLE (Bug-3): the created journal block carries the date in `content`
        // — org/render truth, a heading's text IS its content — so it renders as a
        // NON-EMPTY row and org-round-trips to `* <date>`, with NO stray `name`
        // property. `journal_day_children` already read `content`; assert the
        // storage shape directly, then assert the production org render.
        let boot_id = boot_days[0].0.clone();
        let shape = comp
            .engine
            .db_handle()
            .query(
                &format!(
                    "SELECT content, properties FROM block_raw WHERE id = '{}'",
                    boot_id.replace('\'', "''")
                ),
                std::collections::HashMap::new(),
            )
            .await
            .expect("boot journal shape query");
        let shape_row = shape.first().expect("boot journal row present");
        assert_eq!(
            shape_row.get("content").and_then(|v| v.as_string()),
            Some(boot_date.as_str()),
            "journal date must live in `content` (renders as a non-empty row), not empty"
        );
        let has_name_prop = match shape_row.get("properties") {
            Some(Value::Object(m)) => m.contains_key("name"),
            Some(other) => other
                .as_json_value()
                .and_then(|j| j.get("name").cloned())
                .is_some(),
            None => false,
        };
        assert!(
            !has_name_prop,
            "journal block must NOT carry a `name` property — Bug-3: the date belongs in content"
        );

        // ORG round-trip: block:journals renders the day-block as a `* <date>`
        // headline (content in the headline), never a bare `* ` + `:name:` drawer.
        {
            use holon_pbt_core::capabilities::SutOrgRender;
            let pairs = comp.snapshot_org_render_pairs().await;
            let (_, _, rendered) = pairs
                .iter()
                .find(|(path, _, _)| path.ends_with("Journals.org"))
                .expect("Journals.org is a tracked org doc");
            assert!(
                rendered.contains(&format!("* {boot_date}")),
                "journal must org-render as headline `* {boot_date}` (content), got:\n{rendered}"
            );
            assert!(
                !rendered.contains(":name:"),
                "journal block must not org-render a `:name:` property drawer:\n{rendered}"
            );
        }

        // WP2: the boot-day block carries the deterministic id
        // UUIDv5(rule-id, firing-key, slot). Reproduce it from the discovered rule
        // block id + the trigger row `{name: <date>}` and assert it matches — the
        // convergence-by-naming property the whole capstone rests on.
        let rule_id = comp
            .engine
            .db_handle()
            .query(
                "SELECT id FROM block_raw WHERE source_language = 'holon_rule'",
                std::collections::HashMap::new(),
            )
            .await
            .expect("rule-block query")
            .first()
            .and_then(|r| r.get("id").and_then(|v| v.as_string()).map(str::to_string))
            .expect("a holon_rule block exists");
        eprintln!("[advance-day] discovered rule id: {rule_id}");
        // The firing row is the trigger matview row: `{name: <date>, _rowid: 1}`
        // (the clock relation holds a single day-row, so `_rowid` is stably 1).
        let expect_id_for = |date: &str| -> String {
            let mut row = holon_api::StorageEntity::new();
            row.insert("name".into(), Value::String(date.to_string()));
            row.insert("_rowid".into(), Value::Integer(1));
            holon_api::effect_id::deterministic_block_id(
                &holon_api::effect_id::RuleId::new(rule_id.clone()),
                &holon_api::effect_id::FiringKey::from_row(&row),
                &holon_api::effect_id::OutputSlot::first(),
            )
            .as_str()
            .to_string()
        };
        assert_eq!(
            boot_days[0].0,
            expect_id_for(&boot_date),
            "boot journal block carries the WP2 deterministic id"
        );

        // Journal day-blocks are ORDINARY content (WP3): not source/program blocks.
        // The boot seed's display machinery (`journals_page_blocks`: `::src::0` +
        // `::render::0`) legitimately lives as source children of the shell —
        // exclude those fixed ids; anything ELSE source-typed under journals is a
        // rule output gone wrong.
        let program_rows = comp
            .engine
            .db_handle()
            .query(
                &format!(
                    "SELECT id FROM block_raw WHERE parent_id = 'block:journals' AND content_type \
                     = 'source' AND id != '{src}' AND id != '{render}'",
                    src = holon_frontend::JOURNALS_SRC_ID,
                    render = holon_frontend::JOURNALS_RENDER_ID,
                ),
                std::collections::HashMap::new(),
            )
            .await
            .expect("program-child query");
        assert!(
            program_rows.is_empty(),
            "journal day-blocks must be ordinary content, none program/source-marked"
        );

        // A rule-minted id is a deterministic name-based UUIDv5 (the version nibble
        // is `5`), never a random v4 — the property that makes concurrent replicas
        // converge (WP2). `block:XXXXXXXX-XXXX-5XXX-...`.
        let is_uuid_v5 = |id: &str| {
            id.strip_prefix("block:")
                .and_then(|u| u.split('-').nth(2))
                .map(|g| g.starts_with('5'))
                .unwrap_or(false)
        };
        assert!(is_uuid_v5(&boot_days[0].0), "boot journal id is a v5 UUID");

        // Advance one day: exactly one NEW journal appears, with a deterministic id.
        let d1 = comp.advance_clock_days(1).await;
        let after1 = wait_for_journal_days(&comp, 2, Duration::from_secs(10)).await;
        let d1_block = after1
            .iter()
            .find(|(_, n)| n == &d1)
            .expect("day+1 journal created");
        assert!(is_uuid_v5(&d1_block.0), "day+1 journal id is a v5 UUID");

        // Re-tick the SAME day: idempotent — no new block (deterministic-id upsert).
        let d1_again = comp.advance_clock_days(0).await;
        assert_eq!(d1_again, d1, "re-tick stays on the same day");
        // Give any (erroneous) re-fire a chance to land, then assert nothing grew.
        tokio::time::sleep(Duration::from_millis(300)).await;
        let after_retick = journal_day_children(&comp).await;
        assert_eq!(
            after_retick.len(),
            2,
            "re-ticking the same day adds nothing: {after_retick:?}"
        );

        // Advance a second distinct day: a third journal.
        let d2 = comp.advance_clock_days(1).await;
        let after2 = wait_for_journal_days(&comp, 3, Duration::from_secs(10)).await;
        assert!(
            after2.iter().any(|(_, n)| n == &d2),
            "day+2 journal created"
        );

        // Exactly D=3 distinct days ⇒ 3 blocks, one per day.
        let distinct_days: std::collections::BTreeSet<&String> =
            after2.iter().map(|(_, n)| n).collect();
        assert_eq!(distinct_days.len(), 3, "one journal per distinct day");
        eprintln!("[advance-day] final journals: {after2:?} ✓");
    }

    /// C-revised ruling (WP3) loud-guard: a rule's trigger is program machinery
    /// evaluated SOLELY by the action watcher, so it must never reach
    /// display-query evaluation. `block_domain::render_entity` on the
    /// rule-machinery heading (whose only query-source child is the
    /// `holon_sql` trigger) must FAIL LOUD — surfaced as a visible error
    /// node by UiWatcher — rather than compiling the tableless, no-`id`
    /// trigger query into a display matview (the boot-critical
    /// panic the journal rule was once deferred behind). The action watcher
    /// itself still fires (proven by the day-block created on boot), so the
    /// rule stays live while its content is never display-evaluated.
    #[tokio::test(flavor = "multi_thread")]
    async fn rule_trigger_never_reaches_display_evaluation() {
        let boot_ms = noon_millis(2026, 1, 15);
        let clock = Arc::new(holon_api::TestClock::new(boot_ms));

        let comp = HeadlessFrontendComponent::new_with_clock(
            &[("Journals.org", JOURNAL_RULE_ORG)],
            Duration::from_millis(500),
            false,
            clock.clone(),
        )
        .await;

        // The rule fired on boot — the action watcher IS the evaluator (rule live).
        wait_for_journal_days(&comp, 1, Duration::from_secs(10)).await;

        // Discover the trigger (holon_sql source) and its parent = the machinery
        // heading. The heading owns exactly one query-source child (the trigger),
        // so it is the block that would wrongly resolve a display query.
        let trigger_rows = comp
            .engine
            .db_handle()
            .query(
                "SELECT id, parent_id FROM block_raw WHERE source_language = 'holon_sql'",
                std::collections::HashMap::new(),
            )
            .await
            .expect("trigger-block query");
        let heading_id = trigger_rows
            .first()
            .and_then(|r| {
                r.get("parent_id")
                    .and_then(|v| v.as_string())
                    .map(str::to_string)
            })
            .expect("the holon_sql trigger has a parent heading");
        let heading_uri = EntityUri::parse(&heading_id).expect("heading id parses");

        // render_entity on the heading MUST fail loud (the guard), never run
        // `query_and_watch` on the trigger. A silent fallback to a leaf/collection
        // render would mean the trigger leaked onto the display path.
        let result = comp
            .engine
            .blocks()
            .render_entity(&heading_uri, &None)
            .await;
        let err = result.expect_err(
            "render_entity on a rule-machinery heading must fail loud, not resolve the trigger as \
             a display query",
        );
        let msg = format!("{err:#}");
        assert!(
            msg.contains("rule TRIGGER") || msg.contains("program machinery"),
            "guard error must name the program-machinery/rule-trigger cause, got: {msg}"
        );
        eprintln!("[rule-guard] render_entity({heading_id}) failed loud as required: {msg}");
    }

    /// Step 0 make-or-break PROBE (SutHandle decomposition / NavigateFocus):
    /// does driving the production `navigation.focus` op through the
    /// **windowless** `FrontendSession` actually update the `current_focus`
    /// / `focus_roots` matviews — with no GPUI window, no driver, no
    /// geometry pump? If yes, the `NavigateFocus` transition can rebind
    /// onto `SutFocusWrite` realized on this component. If the headless
    /// session has no operation engine, or the matviews need a window/
    /// driver pump, this STOPS the increment (don't fake the read,
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
            "[nav-probe] navigation.focus through the headless session failed — the windowless \
             session has no operation engine (make-or-break): {:?}",
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
    /// (`apply_type_chars` = `send_raw_keystroke`) actually land the typed text
    /// in `block_raw.content` — the projection
    /// `inv-blocks-match-ref/block_raw` reads? The reference commits typed
    /// text into block content on every TypeChars
    /// (`commit_active_editor_if_changed`), so committed-content parity
    /// requires the SUT's MutableText edit to sync through to `block_raw`.
    /// If the headless pipeline never propagates the edit (no automatic
    /// Loro→Turso sync without a window), this STOPS the keystone — don't
    /// fake the commit, surface it.
    #[tokio::test(flavor = "multi_thread")]
    async fn headless_type_chars_commits_to_block_raw() {
        // The wide PBT's working tree: a seed page with three text-leaf children.
        const TREE_ORG: &str = "#+ID: structural-page\n* parent\n:PROPERTIES:\n:ID: \
                                parent\n:END:\n* c1\n:PROPERTIES:\n:ID: c1\n:END:\n* \
                                c2\n:PROPERTIES:\n:ID: c2\n:END:\n";
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

    /// SPIKE (Phase 1b) END-TO-END: with focus on a DISPLAY occurrence
    /// (`focused_occurrence = Some(1)`), a typed char must STILL commit to the
    /// block's CANONICAL `block_raw.content`. Proves
    /// `editable_text(&block_uri)` resolves by the canonical id regardless
    /// of occurrence (write → canonical home) — the runtime companion to
    /// the mirror unit test `occurrence_keyed_cursors_are_independent`,
    /// which covers caret isolation.
    #[tokio::test(flavor = "multi_thread")]
    async fn spike_display_occurrence_write_routes_to_canonical() {
        const TREE_ORG: &str = "#+ID: structural-page\n* parent\n:PROPERTIES:\n:ID: \
                                parent\n:END:\n* c1\n:PROPERTIES:\n:ID: c1\n:END:\n";
        let comp = HeadlessFrontendComponent::new_with_loro(
            &[("structural-page.org", TREE_ORG)],
            Duration::from_millis(300),
            true,
        )
        .await;

        let page = EntityUri::block("structural-page");
        comp.apply_navigate_focus(CapRegion::Main, &page).await;

        let c1 = EntityUri::block("c1");
        let c1_sql = format!(
            "SELECT content FROM block_raw WHERE id = '{}'",
            c1.as_str().replace('\'', "''")
        );

        // 1) Canonical occurrence (None): open the editor and type 'A' → "c1A".
        comp.apply_focus_editable_text(&c1).await;
        assert_eq!(
            comp.reactive.focused_occurrence(),
            None,
            "[spike-1b] focus starts at the canonical occurrence"
        );
        comp.apply_type_chars("A").await;

        // 2) Switch focus to a DISPLAY occurrence and re-open the editor there.
        //    `set_focus_occurrence` is additive — it does NOT touch `focused_block`,
        //    and the production focus path leaves it intact.
        comp.reactive.set_focus_occurrence(Some(1));
        comp.apply_focus_editable_text(&c1).await;
        assert_eq!(
            comp.reactive.focused_block().as_ref(),
            Some(&c1),
            "[spike-1b] block still focused"
        );
        assert_eq!(
            comp.reactive.focused_occurrence(),
            Some(1),
            "[spike-1b] occurrence persists across the production focus path"
        );
        comp.apply_type_chars("B").await;

        // Both writes landed on the CANONICAL block, despite focus being on the
        // display occurrence — editable_text resolves by block id, not occurrence.
        let after = comp
            .sql_query(&c1_sql)
            .await
            .into_iter()
            .next()
            .and_then(|r| HeadlessFrontendComponent::cell(&r, "content"));
        assert_eq!(
            after.as_deref(),
            Some("c1AB"),
            "[spike-1b] typing at display occurrence Some(1) must commit to CANONICAL c1 content \
             (write → canonical home); got {after:?}"
        );
    }

    /// A2 make-or-break PROBE (`SutNavHistoryDrive`): can the **windowless**
    /// `FrontendSession` drive the nav-history ops (`focus_pin`, `go_back`,
    /// `go_forward`) the way `E2ESut` drives them through the GPUI driver's
    /// `synthetic_dispatch` / leader chords? The memory flagged back/forward as
    /// historically "driver-realized only — the headless slice does not drive
    /// these". This asserts: `focus_pin` reaches the matviews (observable
    /// effect), and `go_back`/`go_forward` dispatch headlessly without
    /// error (reachability — their full history *semantics* are a Phase-B
    /// oracle-parity concern). If the session has no operation engine for
    /// these ops, this STOPS A2 (don't fake it).
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
    /// dispatch). Two unknowns gate adding `PinBlock` to the composed nav
    /// alphabet: (a) does the seed contain a pinnable `ContentType::Text`,
    /// non-page block under Main? (b) does headless
    /// `focus_pin(RightSidebar, block)` populate
    /// `focus_roots(right_sidebar)` (which inv-focus-roots reads) with NO
    /// window? Uses an enriched seed (a paragraph under a heading = a Text
    /// child block).
    #[tokio::test(flavor = "multi_thread")]
    async fn headless_pin_block_right_sidebar_probe() {
        let doc0 = "#+ID: ref-doc-0\n* Heading zero\n:PROPERTIES:\n:ID: ref-block-0\n:END:\nFirst \
                    pinnable paragraph\n";
        let comp =
            HeadlessFrontendComponent::new(&[("doc0.org", doc0)], Duration::from_millis(300)).await;

        // Dump block_raw with content_type + parent so we can see what is pinnable.
        let rows = comp
            .engine
            .db_handle()
            .query(
                // Exclude the `sentinel:no_parent` FK-anchor row (CoreSchemaModule
                // seeds it for the parent FK; it is not a real block, and
                // production's `block` matview drops it the same way).
                "SELECT id, parent_id, content_type, content FROM block_raw WHERE id != \
                 'sentinel:no_parent'",
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

        // Pick a Text, non-doc block (content_type text + has a parent that isn't
        // no_parent).
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
        // inv-focus-roots reads this matview, and without a window it might never
        // update.
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
             focus_roots(right_sidebar) — the matview did not update without a window; got \
             {roots:?}"
        );
    }

    /// Read `current_focus(main)`'s block_id from the matview (None when
    /// empty).
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
    /// does headless `go_back`/`go_forward` move the `current_focus(main)`
    /// matview the way the ref's `navigation_history.cursor` moves (so
    /// inv-navigation-focus would stay green), with NO window? Build
    /// history journals(boot)→d0→d1, then `go_back` must return focus to d0
    /// and `go_forward` back to d1. If focus does NOT track the cursor,
    /// NavigateBack/Forward must NOT join the composed alphabet (they stay
    /// E4/windowed) — this probe is the gate.
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

    /// C1 UnpinBlock make-or-break PROBE: (a) what `history_id` does the
    /// headless SUT assign to a right-sidebar pin (the
    /// `navigation_history.id` AUTOINCREMENT), and
    /// (b) does `close(history_id)` actually remove the pin (clear
    /// `focus_roots(right_sidebar)`) headlessly? `UnpinBlock`'s generator draws
    /// the `history_id` from the ref's `open_pins` — so the ref's predicted
    /// id must equal the SUT's real row id (the "risk C" alignment). This
    /// probe establishes the SUT side: pin, read the assigned id, unpin it,
    /// confirm the pin is gone.
    #[tokio::test(flavor = "multi_thread")]
    async fn headless_unpin_block_probe() {
        let doc0 = "#+ID: ref-doc-0\n* Heading zero\n:PROPERTIES:\n:ID: ref-block-0\n:END:\nFirst \
                    pinnable paragraph\n";
        let comp =
            HeadlessFrontendComponent::new(&[("doc0.org", doc0)], Duration::from_millis(300)).await;
        let pin_uri = EntityUri::parse("block:ref-block-0").expect("pin id");

        SutNavHistoryDrive::pin_block(&comp, holon_api::Region::RightSidebar, &pin_uri).await;

        // Dump navigation_history to find the pin row's `id` (the SUT-assigned
        // history_id).
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
    /// already-home guard), so `NavigateHome`×N → N home entries. If the
    /// headless SUT (production `navigation.focus(None)`) instead writes NO
    /// new row when already home (like `navigate_focus`'s same-target
    /// idempotency), then the ref over-counts and `NavigateBack` walks it
    /// through phantom home entries the SUT lacks — a ref-model bug. Drive
    /// `go_home`×3 from the boot (journals) state and count the NULL (home)
    /// rows in `navigation_history`.
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

    /// PROBE (nav-history fold into the wide PBT): two questions, answered
    /// empirically. (1) What `navigation_history.id`s does the WIDE boot
    /// assign (boot focus + the driven `NavigateFocus(page_root)`)? These
    /// set the exact `next_history_id` / `open_pins.history_id` constants
    /// the wide oracle must mirror to fold Pin/Unpin.
    /// (2) Do `FocusEditableText` / `create_document` — already in the wide
    /// alphabet — write SUT `navigation_history` rows the oracle wouldn't
    /// mirror? If so, they silently advance the AUTOINCREMENT and would
    /// desync Pin/Unpin id alignment + Back/Forward stack depth. The counts
    /// after each step decide the fold scope.
    #[tokio::test(flavor = "multi_thread")]
    async fn wide_boot_navigation_history_id_probe() {
        const TREE_ORG: &str = "#+ID: structural-page\n* parent\n:PROPERTIES:\n:ID: \
                                parent\n:END:\n* c1\n:PROPERTIES:\n:ID: c1\n:END:\n* \
                                c2\n:PROPERTIES:\n:ID: c2\n:END:\n";
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

    /// **C2.0 make-or-break PROBE — does the real headless component's
    /// `block_raw` reduce to the spike's fixed `parent/c1/c2` tree after a
    /// production-create seed, so the structural reconcile loop can run
    /// over it?** The spike proves reconcile over a *bare*
    /// `new_sql_engine_with_structural_ops` (no boot bootstrap).
    /// `HeadlessFrontendComponent` runs the FULL production boot — which
    /// may create journals/default pages (the nav slice's `block:journals` came
    /// from here). If boot leaves extra blocks,
    /// `inv-blocks-match-ref/block_raw` (a full-set compare) would
    /// false-RED against `build_started_ref`'s parent/c1/c2. This probe
    /// DUMPS the booted block_raw, seeds the tree via the SAME production
    /// create op the spike uses, dumps again, then runs split→reconcile→catalog
    /// so the seed/oracle alignment is decided EMPIRICALLY before the SUT
    /// is built.
    #[tokio::test(flavor = "multi_thread")]
    async fn headless_structural_seed_and_reconcile_probe() {
        use std::collections::BTreeSet;

        use holon_api::EntityUri;
        use holon_pbt_core::TransitionRef;
        use holon_pbt_core::capabilities::SutBackend;
        use holon_pbt_core::capabilities::SutBlockTreeWrite;

        use crate::pbt::composed::seed_primitives::C1;
        use crate::pbt::composed::seed_primitives::C2;
        use crate::pbt::composed::seed_primitives::PARENT;
        use crate::pbt::composed::seed_primitives::fixed_ids;
        use crate::pbt::composed::subsystem_seed::build_started_ref;
        use crate::pbt::composed::subsystem_seed::run_with_seeded_ref;
        use crate::pbt::is_synthetic_ref_id;
        use crate::pbt::op_write_cap::IdResolver;
        use crate::pbt::op_write_cap::OpDispatchWriter;
        use crate::pbt::sql_slice::SqlProjectionComponent;

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
        use holon_pbt_core::TransitionImpl;

        use crate::pbt::transitions::SplitBlock;
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
            "[struct-probe] non-vacuity (post-split): inv-blocks-match-ref/block_raw must RUN \
             (ran: {:?})",
            report.ran_ids()
        );
        eprintln!(
            "[struct-probe] OK — reconciled structural catalog green over \
             HeadlessFrontendComponent"
        );
    }
}
