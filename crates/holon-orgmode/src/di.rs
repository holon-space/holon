//! Dependency Injection pieces for the backend-blind OrgMode file-sync core.
//!
//! This module owns the backend-blind half of org-mode sync: the
//! ready/idle signals, [`OrgModeConfig`], vault seeding, and
//! [`register_org_file_sync_core`] — the fluxdi registration whose
//! [`FileSyncStarted`] marker spawns the `FileSyncController` over whatever
//! `BlockReader` / `DocumentManager` / `BlockOrdering` seams the container
//! provides. The Turso-backed seam impls (`CacheBlockReader`,
//! `LiveDocumentManager`) and the Turso container's `OrgModeModule` live at
//! the app composition root (`holon-app`), per ADR 0004.

use std::path::PathBuf;
use std::sync::Arc;

use fluxdi::Injector;
use fluxdi::Provider;
use fluxdi::Shared;
use holon_api::block::Block;
use holon_api::EntityUri;
use holon_core::block_ordering::BlockOrdering;
use holon_filesystem::AliasRegistrar;
use holon_filesystem::BlockDelta;
use holon_filesystem::BlockReader;
use holon_filesystem::DocumentManager;
use holon_filesystem::FileSyncController;

use crate::file_watcher::OrgFileWatcher;
use crate::org_renderer::OrgRenderer;

/// Signal that indicates the FileWatcher is ready to receive file change
/// events.
///
/// Tests can wait on this signal to ensure the file watcher is established
/// before making external file modifications.
#[derive(Clone)]
pub struct FileWatcherReadySignal {
    receiver: tokio::sync::watch::Receiver<Option<Result<(), String>>>,
}

impl FileWatcherReadySignal {
    /// Create a new ready signal (sender/receiver pair)
    pub fn new() -> (FileWatcherReadySender, Self) {
        let (tx, rx) = tokio::sync::watch::channel(None);
        (FileWatcherReadySender { sender: tx }, Self { receiver: rx })
    }

    /// Consume the wrapper and return the inner watch receiver so
    /// downstream consumers (e.g. holon-frontend) can call `borrow()`
    /// without depending on `holon-orgmode`.
    pub fn into_receiver(self) -> tokio::sync::watch::Receiver<Option<Result<(), String>>> {
        self.receiver
    }

    /// Check if startup has completed (either success or failure).
    pub fn is_completed(&self) -> bool {
        self.receiver.borrow().is_some()
    }

    /// Wait until the file watcher signals readiness.
    ///
    /// Returns `Ok(())` on success, `Err` if the FileSyncController startup
    /// failed. Errors are propagated — never swallowed.
    #[tracing::instrument(skip(self), name = "FileWatcherReadySignal.wait_ready")]
    pub async fn wait_ready(&self) -> anyhow::Result<()> {
        let mut receiver = self.receiver.clone();
        // Wait until the value is Some(_)
        let result = receiver.wait_for(|v| v.is_some()).await.map_err(|_| {
            anyhow::anyhow!("FileWatcherReadySignal sender dropped without signaling")
        })?;
        match result.as_ref().unwrap() {
            Ok(()) => Ok(()),
            Err(msg) => Err(anyhow::anyhow!(
                "FileSyncController startup failed: {}",
                msg
            )),
        }
    }
}

/// Sender half of the FileWatcher ready signal
pub struct FileWatcherReadySender {
    sender: tokio::sync::watch::Sender<Option<Result<(), String>>>,
}

impl FileWatcherReadySender {
    /// Signal successful readiness.
    pub fn signal_ready(self) {
        let _ = self.sender.send(Some(Ok(())));
    }

    /// Signal that startup failed. The error message propagates to the waiter.
    pub fn signal_error(self, error: String) {
        let _ = self.sender.send(Some(Err(error)));
    }
}

/// Event-driven idle signal for the FileSyncController loop.
///
/// The controller's background task calls [`mark_progress`] after each
/// iteration where it actually processed an event (file change or block
/// change). Tests use [`wait_quiescent`] to wait until the loop has had no
/// activity for a short window — proving that all org-file writes triggered
/// by recent SQL mutations have already landed on disk.
///
/// This replaces filesystem mtime polling on the hot path (~30 ms per call)
/// with an event signal that completes in ~1 ms when the loop is genuinely
/// idle. Callers that don't have access to the signal (or want extra safety)
/// fall back to mtime polling.
///
/// [`mark_progress`]: OrgSyncIdleSignal::mark_progress
/// [`wait_quiescent`]: OrgSyncIdleSignal::wait_quiescent
#[derive(Debug)]
pub struct OrgSyncIdleSignal {
    /// Monotonic count of completed loop iterations. Bumped after every
    /// processed event (file or block change).
    tick: std::sync::atomic::AtomicU64,
    /// Wakes any task waiting in [`wait_quiescent`] whenever the tick advances.
    notify: tokio::sync::Notify,
    /// Highest fully-processed `FileChange.seq` (ADR 0011). Advanced by the
    /// controller loop after each change — forwarded ones after
    /// `on_file_changed` returns, filtered ones immediately — strictly in
    /// delivery order, so `seq <= watermark` means "that change (and every
    /// earlier one) has been processed".
    change_seq: std::sync::atomic::AtomicU64,
}

impl OrgSyncIdleSignal {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            tick: std::sync::atomic::AtomicU64::new(0),
            notify: tokio::sync::Notify::new(),
            change_seq: std::sync::atomic::AtomicU64::new(0),
        })
    }

    /// Current tick value. Increases monotonically.
    pub fn current_tick(&self) -> u64 {
        self.tick.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Advance the processed-change watermark (monotonic) and wake waiters.
    pub fn advance_change_seq(&self, seq: u64) {
        self.change_seq
            .fetch_max(seq, std::sync::atomic::Ordering::Release);
        self.notify.notify_waiters();
    }

    /// Highest fully-processed `FileChange.seq`.
    pub fn processed_change_seq(&self) -> u64 {
        self.change_seq.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Wait until the processed-change watermark reaches `seq`, or `timeout`
    /// elapses. Returns `true` when the watermark was reached. Deterministic
    /// counterpart to [`wait_quiescent`] for changes whose seq the caller
    /// knows (e.g. an in-memory write it just made).
    ///
    /// [`wait_quiescent`]: Self::wait_quiescent
    pub async fn wait_for_change_seq(&self, seq: u64, timeout: std::time::Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            // Subscribe BEFORE checking to avoid missing a wake.
            let notified = self.notify.notified();
            if self.processed_change_seq() >= seq {
                return true;
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let _ = tokio::time::timeout(remaining, notified).await;
        }
    }

    /// Called by the controller loop after each processed event.
    pub fn mark_progress(&self) {
        self.tick.fetch_add(1, std::sync::atomic::Ordering::Release);
        self.notify.notify_waiters();
    }

    /// Wait until the controller loop has been idle (no [`mark_progress`]
    /// call) for `quiescence`, or `timeout` elapses. Returns `true` if
    /// quiescence was reached, `false` on timeout.
    ///
    /// Cost when already idle: one `tokio::time::timeout` of `quiescence`.
    /// Cost when busy: as long as it takes for the loop to drain, capped by
    /// `timeout`.
    ///
    /// [`mark_progress`]: Self::mark_progress
    pub async fn wait_quiescent(
        &self,
        quiescence: std::time::Duration,
        timeout: std::time::Duration,
    ) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let snapshot = self.current_tick();
            // Subscribe BEFORE re-reading the tick to avoid missing a wake.
            let notified = self.notify.notified();
            if self.current_tick() != snapshot {
                // Activity already happened; loop again.
                if tokio::time::Instant::now() >= deadline {
                    return false;
                }
                continue;
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let wait = quiescence.min(remaining);
            match tokio::time::timeout(wait, notified).await {
                Err(_) => {
                    // No notification within `quiescence` — the loop is idle.
                    if self.current_tick() == snapshot {
                        return true;
                    }
                    // A wake landed between the timeout firing and the
                    // re-check; treat it as activity and
                    // loop.
                }
                Ok(()) => {
                    // Got woken — keep waiting unless we ran out of time.
                    if tokio::time::Instant::now() >= deadline {
                        return false;
                    }
                }
            }
        }
    }
}

/// Scan a directory recursively for .org files.
///
/// Delegates to `file_watcher::scan_directory` — the gitignore-aware walk
/// behind the `FileSystem` port (ADR 0011), filtered to `.org`.
async fn scan_org_files(
    fs: &dyn holon_filesystem::FileSystem,
    dir: &std::path::Path,
) -> std::io::Result<Vec<PathBuf>> {
    Ok(crate::file_watcher::scan_directory(fs, dir).await?.files)
}

/// Configuration for OrgMode integration
#[derive(Clone, Debug)]
pub struct OrgModeConfig {
    /// Root directory containing .org files
    pub root_directory: PathBuf,
    /// Shell command to run after each org file write (e.g. "jj new").
    /// Runs in root_directory with HOLON_FILE env var set to the written path.
    pub post_write_hook: Option<String>,
    /// `(filename, content)` documents seeded through the `FileSystem` port
    /// when the vault contains no .org files (ADR 0011). Filled by the app
    /// wiring from `holon_frontend::DEFAULT_ASSETS`; empty = no seeding.
    pub seed_assets: Vec<(String, String)>,
}

impl OrgModeConfig {
    pub fn new(root_directory: PathBuf) -> Self {
        // Canonicalize to resolve symlinks (e.g., /var -> /private/var on macOS)
        // This ensures path comparisons work correctly when file watcher reports
        // canonicalized paths
        let root_directory = std::fs::canonicalize(&root_directory).unwrap_or(root_directory);
        Self {
            root_directory,
            post_write_hook: None,
            seed_assets: Vec::new(),
        }
    }
}

/// Seed `config.seed_assets` into an empty vault (no .org files) through the
/// `FileSystem` port. Must run before anything scans the vault (the initial
/// `OrgModeSyncProvider::sync` and the controller's initial scan). Panics on
/// write failure — a half-seeded vault on first launch is a startup error.
pub async fn seed_default_org_assets(
    fs: &dyn holon_filesystem::FileSystem,
    config: &OrgModeConfig,
) {
    if config.seed_assets.is_empty() {
        return;
    }
    let root = &config.root_directory;
    let scanned = crate::file_watcher::scan_directory(fs, root)
        .await
        .unwrap_or_else(|e| panic!("Failed to scan org root {}: {e}", root.display()));
    if !scanned.files.is_empty() {
        return;
    }
    fs.create_dir_all(root)
        .await
        .unwrap_or_else(|e| panic!("Failed to create org root {}: {e}", root.display()));
    for (filename, content) in &config.seed_assets {
        fs.write(&root.join(filename), content.as_bytes())
            .await
            .unwrap_or_else(|e| panic!("Failed to write {filename}: {e}"));
    }
}

/// Register the backend-blind file-sync core: FileSystem/FileChangeSource
/// port defaults, the ready/idle signals, OrgRenderer, and the
/// [`FileSyncStarted`] marker whose resolution spawns the controller over
/// whatever seams the container provides. Called by [`OrgModeModule`] (Turso)
/// and by the no-Turso container so both self-start identically.
pub fn register_org_file_sync_core(injector: &Injector) -> std::result::Result<(), fluxdi::Error> {
    use tracing::info;

    // FileSystem port (ADR 0011): default-bind the real-disk adapter.
    // First binding wins in fluxdi, so a test harness that registered an
    // in-memory FileSystem before this module keeps its binding — that
    // case is expected and disclosed below, any other provide error is not.
    match injector.try_provide::<dyn holon_filesystem::FileSystem>(Provider::root(|_| {
        Arc::new(holon_filesystem::RealFileSystem) as Arc<dyn holon_filesystem::FileSystem>
    })) {
        Ok(()) => {}
        // Published fluxdi (dev @ 24b6eebb) models this as a struct
        // `Error { kind, .. }`, not an enum variant.
        Err(e) if matches!(e.kind, fluxdi::ErrorKind::ProviderAlreadyRegistered) => {
            info!(
                "[OrgModeModule] dyn FileSystem already bound — keeping the existing \
                 (test-override) binding"
            );
        }
        Err(e) => return Err(e),
    }

    // FileChangeSource port (ADR 0011): default-bind the notify adapter.
    // Same first-binding-wins override contract as dyn FileSystem above.
    match injector.try_provide::<dyn holon_filesystem::FileChangeSource>(Provider::root(|_| {
        Arc::new(
            holon_filesystem::NotifyWatcher::new_unarmed()
                .expect("notify watcher construction failed"),
        ) as Arc<dyn holon_filesystem::FileChangeSource>
    })) {
        Ok(()) => {}
        Err(e) if matches!(e.kind, fluxdi::ErrorKind::ProviderAlreadyRegistered) => {
            info!(
                "[OrgModeModule] dyn FileChangeSource already bound — keeping the existing \
                 (test-override) binding"
            );
        }
        Err(e) => return Err(e),
    }

    // Create and register FileWatcherReadySignal
    // Tests can wait on this to ensure file watcher is ready before external
    // mutations
    let (ready_sender, ready_signal) = FileWatcherReadySignal::new();
    let ready_signal = std::sync::Arc::new(std::sync::Mutex::new(Some(ready_signal)));
    injector.provide::<FileWatcherReadySignal>(Provider::root(move |_| {
        let signal = ready_signal
            .lock()
            .unwrap()
            .take()
            .expect("FileWatcherReadySignal factory called twice");
        Shared::new(signal)
    }));
    // Store sender in Arc<Mutex> so we can move it into the spawned task later
    let ready_sender = std::sync::Arc::new(std::sync::Mutex::new(Some(ready_sender)));
    let ready_sender_for_factory = ready_sender.clone();

    // Create and register OrgSyncIdleSignal
    // Tests use this to skip mtime polling on the hot path.
    let idle_signal = OrgSyncIdleSignal::new();
    let idle_signal_for_factory = idle_signal.clone();
    injector.provide::<OrgSyncIdleSignal>(Provider::root(move |_| idle_signal_for_factory.clone()));
    let idle_signal_for_loop = idle_signal;

    // Register OrgRenderer
    injector.provide::<OrgRenderer>(Provider::root(|_resolver| Shared::new(OrgRenderer)));

    // FileSyncStarted: resolving this marker builds the backend-blind
    // FileSyncController over the DI-provided seams and spawns its loop.
    // Root scope => built once. Both backends resolve it AFTER seeding
    // (Turso: in the OperationProvider factory below; no-Turso: after layout
    // seeding) — a side-effect-on-resolve start, the analog of the Turso
    // dispatcher pulling the provider set.
    {
        let ready_sender_for_fs = ready_sender_for_factory.clone();
        let idle_signal_for_fs = idle_signal_for_loop.clone();
        injector.provide::<FileSyncStarted>(Provider::root_async(move |resolver| {
            let ready_sender = ready_sender_for_fs.clone();
            let idle_signal = idle_signal_for_fs.clone();
            async move {
                let block_reader = resolver.resolve_async::<dyn BlockReader>().await;
                let doc_manager = resolver.resolve_async::<dyn DocumentManager>().await;
                let ordering = resolver.resolve_async::<dyn BlockOrdering>().await;
                let config = resolver.resolve::<OrgModeConfig>();
                let fs = resolver.resolve::<dyn holon_filesystem::FileSystem>();
                let change_source = resolver.resolve::<dyn holon_filesystem::FileChangeSource>();
                let downstream = resolver
                    .optional_resolve_async::<dyn holon_core::DownstreamProjection>()
                    .await;
                let block_feed = resolver
                    .optional_resolve_async::<holon_api::live_data::BlockFeed>()
                    .await
                    .map(|bf| bf.0.clone());
                let format = resolver
                    .optional_resolve_async::<dyn holon_core::FileFormatAdapter>()
                    .await
                    .unwrap_or_else(|| {
                        Arc::new(crate::file_format::OrgFormatAdapter::new())
                            as Arc<dyn holon_core::FileFormatAdapter>
                    });
                // The alias registrar (doc_id ↔ path) is a Loro-backed seam
                // registered at the composition root — `dyn AliasRegistrar` in
                // both the Turso container (app `wiring.rs`, off `LoroBlockOperations`)
                // and the no-Turso/test container (`LoroAliasRegistrar` directly).
                // Absent in SqlOnly mode; the controller then runs without it.
                let alias_registrar = resolver
                    .optional_resolve_async::<dyn AliasRegistrar>()
                    .await;

                // 3-way text merger for the no-store conflict path (spec 0008
                // §3.1). Present in both containers; the controller consults it
                // only in `Consolidator::Store` (SqlOnly) mode.
                let text_merge = resolver
                    .optional_resolve_async::<dyn holon_filesystem::ThreeWayTextMerge>()
                    .await;

                let idle_signal_weak = std::sync::Arc::downgrade(&idle_signal);

                let mut controller = FileSyncController::with_format(
                    block_reader,
                    doc_manager,
                    config.root_directory.clone(),
                    format,
                    ordering,
                    fs.clone(),
                );
                if let Some(hook_cmd) = config.post_write_hook.clone() {
                    controller = controller.with_post_write_hook(hook_cmd);
                }
                if let Some(downstream) = downstream {
                    controller = controller.with_downstream_projection(downstream);
                }
                if let Some(registrar) = alias_registrar {
                    controller = controller.with_alias_registrar(registrar);
                }
                if let Some(merger) = text_merge {
                    controller = controller.with_text_merge(merger);
                }

                let (rerender_tx, rerender_rx) =
                    tokio::sync::mpsc::unbounded_channel::<OrgRerender>();
                if let Some(feed) = block_feed.clone() {
                    let resolver_feed = feed.clone();
                    let tx = rerender_tx.clone();
                    tokio::spawn(async move {
                        use futures_signals::signal_map::MapDiff;
                        use futures_signals::signal_map::SignalMapExt;
                        resolver_feed
                            .signal_map()
                            .for_each(move |diff| {
                                let tx = tx.clone();
                                let feed = feed.clone();
                                async move {
                                    let msg = match diff {
                                        MapDiff::Insert { value, .. }
                                        | MapDiff::Update { value, .. } => {
                                            match resolve_doc_for_block(&feed, &value) {
                                                Some(doc) => OrgRerender::Block {
                                                    doc,
                                                    delta: BlockDelta::Upsert((*value).clone()),
                                                },
                                                // ALLOW(fallback): doc unresolved (matview lag /
                                                // nested) → full re-render
                                                None => OrgRerender::All,
                                            }
                                        }
                                        // ADR 0025 ROOT ITEM: a removed block is gone from the
                                        // feed, so its owning document can't be resolved HERE —
                                        // but the per-block Remove identity must not be
                                        // discarded. Carry the id; the controller reverse-looks
                                        // up the owning file in `last_projection` and re-renders
                                        // it with the removal SANCTIONED (op-grounded).
                                        MapDiff::Remove { key } => {
                                            match EntityUri::parse(&key) {
                                                Ok(id) => OrgRerender::Remove { id },
                                                Err(e) => {
                                                    // A feed key that is not a valid EntityUri is
                                                    // a defect — surface it, then fall back to the
                                                    // (disclosed) bulk recovery path.
                                                    tracing::error!(
                                                        "[OrgMode] block feed Remove key {key:?} \
                                                         is not a valid EntityUri: {e} — falling \
                                                         back to full re-render"
                                                    );
                                                    OrgRerender::All
                                                }
                                            }
                                        }
                                        // Bulk state resets carry no per-block intent → recovery.
                                        MapDiff::Replace { .. } | MapDiff::Clear {} => {
                                            OrgRerender::All
                                        }
                                    };
                                    let _ = tx.send(msg);
                                }
                            })
                            .await;
                    });
                }
                drop(rerender_tx);

                tokio::spawn(run_file_sync_controller(
                    controller,
                    config.root_directory.clone(),
                    idle_signal_weak,
                    rerender_rx,
                    ready_sender,
                    fs,
                    change_source,
                ));

                Shared::new(FileSyncStarted)
            }
        }));
    }

    Ok(())
}

/// Marker resolved to start the backend-blind `FileSyncController`.
///
/// Registering the seams (`BlockReader` / `DocumentManager` / `BlockOrdering`)
/// and resolving this type spawns the controller over whatever the container
/// provides — no per-backend `spawn_*` call. Root-scoped, so the build+spawn
/// happens exactly once however many times it is resolved.
#[derive(Clone)]
pub struct FileSyncStarted;

/// A re-render request funnelled from the block-feed resolver task into the
/// org controller's single-owner `select!` loop (Phase 5: replaces the EventBus
/// `Consumer::ORG` block-event path).
pub enum OrgRerender {
    /// Re-render exactly this document (resolved to its `Page` root), carrying
    /// the single block change so the controller can update just that block in
    /// its per-doc cache instead of re-reading the whole document.
    Block {
        doc: EntityUri,
        delta: holon_filesystem::BlockDelta,
    },
    /// A block disappeared from the feed (`MapDiff::Remove`). Its owning
    /// document cannot be resolved from the feed (the block is gone), so the
    /// controller reverse-looks it up in its tracked projections and re-renders
    /// exactly that file with the removal SANCTIONED. If no projection contains
    /// the block, the id is accumulated into the sanctioned set the debounced
    /// `re_render_all_tracked` pass consumes (ADR 0025: per-block Remove
    /// identity is preserved end-to-end instead of collapsing to `All`).
    Remove { id: EntityUri },
    /// Document could not be resolved (matview lag, bulk feed reset, etc.) —
    /// reseed via a debounced re-render of every tracked file.
    All,
}

/// Resolve the owning document URI for a feed `Block` by walking `parent_id`
/// up the in-memory block feed to the nearest `Page`-tagged ancestor (the block
/// itself included). Mirrors `SqlOperationProvider::find_document_uri`'s
/// recursive CTE (depth-bounded at 50). Returns `None` when the chain ends
/// without a `Page` — e.g. an ancestor not yet present in the matview-backed
/// feed — and the caller falls back to a full re-render.
fn resolve_doc_for_block(
    feed: &holon_api::live_data::LiveData<Block>,
    block: &Block,
) -> Option<EntityUri> {
    let map = feed.read();
    let mut current = block.clone();
    for _ in 0..50 {
        if current.is_page() {
            return Some(current.id.clone());
        }
        match map.get(current.parent_id.as_str()) {
            Some(parent) => current = (**parent).clone(),
            None => return None,
        }
    }
    None
}

/// Backend-blind FileSyncController driver: initialize, build the file watcher,
/// run the initial scan, signal readiness, arm the watcher, and run the main
/// `select!` loop. Shared by the Turso factory and the no-Turso bootstrap —
/// neither path knows which storage backend the controller's adapters use.
pub async fn run_file_sync_controller(
    mut controller: FileSyncController,
    root_directory: PathBuf,
    idle_signal_weak: std::sync::Weak<OrgSyncIdleSignal>,
    mut rerender_rx: tokio::sync::mpsc::UnboundedReceiver<OrgRerender>,
    ready_sender: std::sync::Arc<std::sync::Mutex<Option<FileWatcherReadySender>>>,
    fs: Arc<dyn holon_filesystem::FileSystem>,
    change_source: Arc<dyn holon_filesystem::FileChangeSource>,
) {
    use tracing::error;
    use tracing::info;
    use tracing::Instrument;

    let init_result = async { controller.initialize().await }
        .instrument(tracing::info_span!("org.startup.controller_initialize"))
        .await;
    if let Err(e) = init_result {
        let msg = format!("FileSyncController initialization failed: {}", e);
        error!("[OrgMode] {}", msg);
        if let Some(sender) = ready_sender.lock().unwrap().take() {
            sender.signal_error(msg);
        }
        return;
    }

    // Build the org filter bridge over the change-source port without arming
    // it yet — the slow recursive watch registration (9+s on macOS for the
    // notify adapter) is deferred until after signal_ready so the factory can
    // return immediately. The bridge subscribes here, so no event is missed.
    let mut file_rx = tracing::info_span!("org.startup.file_watcher_new_unarmed")
        .in_scope(|| OrgFileWatcher::new(change_source.as_ref(), &root_directory))
        .into_receiver();
    info!(
        "[OrgMode] File watcher built (unarmed) for: {}",
        root_directory.display()
    );

    // Initial scan ingests pre-existing files BEFORE
    // signal_ready so prime_seed_count's expected
    // block count can match immediately.
    //
    // Per-file failures are collected and propagated
    // through the ReadySignal — swallowing them at
    // ERROR-log level left downstream consumers
    // (LiveData mirrors, matview cursors) wedged
    // because partial-state writes never reconciled.
    let scan_failures: Vec<(std::path::PathBuf, anyhow::Error)> = async {
        let org_files = match scan_org_files(fs.as_ref(), &root_directory).await {
            Ok(files) => files,
            Err(e) => {
                return vec![(
                    root_directory.clone(),
                    anyhow::Error::from(e)
                        .context(format!("initial scan of {}", root_directory.display())),
                )];
            }
        };
        let fs_warm = fs.clone();
        let preloaded: Vec<(std::path::PathBuf, Option<String>)> =
            futures::future::join_all(org_files.into_iter().map(|p| {
                let fs_warm = fs_warm.clone();
                async move {
                    let content = fs_warm.read_to_string(&p).await.ok(); // ALLOW(ok): best-effort OS page-cache warmup; content is dropped below
                    (p, content)
                }
            }))
            .instrument(tracing::info_span!("org.initial_scan.parallel_read"))
            .await;
        let mut failures = Vec::new();
        // Boot ingest latency (Options 0+1): batch the per-file feed barrier.
        // `begin_initial_scan` makes each `on_file_changed` buffer its feed-catch-up
        // ids instead of paying an up-to-2s round-trip per file; `finish_initial_scan`
        // below does ONE convergence wait over the union before `signal_ready`.
        // `block_raw` is written synchronously per file, so the per-file
        // `get_blocks` count-check + `ordering.children` propagation gate still
        // cover intra-file correctness; only the sidebar-facing `block`-matview
        // feed is deferred. Scoped to the initial scan — runtime edits keep the
        // per-edit barrier.
        let files = preloaded.len();
        let t_scan = std::time::Instant::now();
        controller.begin_initial_scan();
        for (file_path, _content) in preloaded {
            let t_file = std::time::Instant::now();
            let result = controller.on_file_changed(&file_path).await;
            tracing::debug!(
                target: "holon_latency",
                stage = "boot_file",
                ms = t_file.elapsed().as_millis() as u64,
                path = %file_path.display(),
                "holon_latency",
            );
            if let Err(e) = result {
                error!(
                    "[OrgMode] Failed to process existing file {}: {}",
                    file_path.display(),
                    e
                );
                failures.push((file_path, e));
            }
        }
        // ONE end-of-scan convergence wait (30s loud ceiling). A stall becomes a
        // scan failure routed through the existing `signal_error` path below.
        if let Err(e) = controller.finish_initial_scan(30_000).await {
            error!("[OrgMode] initial-scan feed convergence failed: {}", e);
            failures.push((root_directory.clone(), e));
        } else if let Err(e) = controller.materialize_missing_page_files().await {
            error!("[OrgMode] fileless-page materialization failed: {}", e);
            failures.push((root_directory.clone(), e));
        }
        tracing::debug!(
            target: "holon_latency",
            stage = "boot_ingest_total",
            ms = t_scan.elapsed().as_millis() as u64,
            files = files as u64,
            "holon_latency",
        );
        failures
    }
    .instrument(tracing::info_span!("org.initial_scan.ingest"))
    .await;

    // Project rule: fail loud, never fake — but NEVER let one bad file kill
    // sync for every other file. A per-file initial-scan failure is surfaced
    // through the ready signal (`signal_error`), which the frontend turns into
    // a VISIBLE degraded-mode banner (see holon-app wiring `post_ready`), and
    // then we FALL THROUGH to arm() + the watch loop below so the healthy
    // files keep syncing and the user can fix the bad file live.
    //
    // Regression guard (dogfood 2026-07-10 ship-blocker): this used to
    // `return` early on ANY failure, so a single bad vault file left arm()
    // unspawned and every file's runtime sync dead while the window still
    // looked healthy — a silent sync death with no user-visible signal. The
    // detached-worker `panic!` that consumed this error at the wiring layer is
    // likewise replaced by the banner. Do NOT reinstate the early return.
    if !scan_failures.is_empty() {
        let summary = scan_failures
            .iter()
            .map(|(p, e)| format!("{}: {}", p.display(), e))
            .collect::<Vec<_>>()
            .join("; ");
        let msg = format!(
            "OrgMode initial scan failed for {} file(s): {}",
            scan_failures.len(),
            summary
        );
        error!("[OrgMode] {}", msg);
        if let Some(sender) = ready_sender.lock().unwrap().take() {
            sender.signal_error(msg);
        }
        // No early return — arm() + the watch loop run below regardless.
    } else if let Some(sender) = ready_sender.lock().unwrap().take() {
        // Phase 1 fix: signal_ready BEFORE arm(). The
        // 9+ s `notify::watch(Recursive)` on macOS runs
        // detached in the background. Correctness during
        // the unarmed window is preserved by
        // `poll_external_changes`, which now also walks
        // the tree to discover new files via
        // `scan_directory` (see file_sync_controller.rs).
        // Without that Phase A→B extension, this fix
        // breaks `create_document`.
        sender.signal_ready();
    }

    // Spawn arm() on the blocking pool, detached.
    // Holds a strong ref to the change source alive
    // forever via `pending::<()>().await` — dropping
    // it (e.g. the notify adapter's RecommendedWatcher)
    // silently stops event delivery into `file_rx`.
    // AbortOnDrop wraps the JoinHandle so this task
    // terminates when the outer file-watcher loop
    // exits via the Weak<OrgSyncIdleSignal> shutdown.
    let dir_for_arm = root_directory.clone();
    let source_for_arm = change_source.clone();
    let arm_task = tokio::spawn(
        async move {
            let r = tokio::task::spawn_blocking(move || {
                let r = source_for_arm.arm(&dir_for_arm);
                (source_for_arm, r)
            })
            .await;
            match r {
                Ok((source, Ok(()))) => {
                    info!("[OrgMode] watcher armed");
                    let _kept = source;
                    std::future::pending::<()>().await;
                }
                Ok((_, Err(e))) => {
                    error!("[OrgMode] watch_recursive failed: {}", e);
                }
                Err(e) => {
                    error!("[OrgMode] arm spawn_blocking panicked: {}", e);
                }
            }
        }
        .instrument(tracing::info_span!("org.startup.arm_watcher_blocking")),
    );
    struct AbortOnDrop(tokio::task::JoinHandle<()>);
    impl Drop for AbortOnDrop {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let _arm_keepalive = AbortOnDrop(arm_task);

    // Main loop: handle file changes and EventBus block events.
    //
    // Two periodic tickers backstop the notify-driven
    // `file_rx` path:
    //
    // - `poll_tick` (100ms): re-stats every tracked `last_projection` entry. Cheap
    //   — short-circuited by an `(mtime, size)` signature so unchanged files don't
    //   read.
    // - `discovery_tick` (2s): walks the full tree via `scan_directory` to pick up
    //   files created during notify's unarmed window on macOS. Expensive (rebuilds
    //   `ignore::WalkBuilder` gitignore DFAs) so deliberately infrequent.
    //
    // Missed FSEvents now become a 100ms latency blip
    // for modifications and ≤2s for brand-new files.
    let mut poll_tick = tokio::time::interval(tokio::time::Duration::from_millis(100));
    poll_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut discovery_tick = tokio::time::interval(tokio::time::Duration::from_secs(2));
    discovery_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Coalesce mark_processed across bursty events; flushes
    // Coalesce orphan-event full re-renders. Events that
    // lack routing_doc_uri (and whose payload parent_id
    // doesn't resolve via on_block_changed) used to trigger
    // re_render_all_tracked per event — O(events × tracked
    // files) IO + segment-chain lookups during bursty
    // initial scans. The flag is set in the event arm; a
    // 50ms ticker drains it with a single re-render pass.
    let mut pending_full_rerender = false;
    // ADR 0025: per-block `Remove` ids the feed delivered that could not be
    // routed to a single file (deleted page owning its own file, cold
    // projection cache). The debounced `re_render_all_tracked` pass consumes
    // them as sanctioned removals, so even the bulk path grounds these
    // deletions in the ops that authorized them.
    let mut pending_sanctioned_removals: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    let mut rerender_flush_tick = tokio::time::interval(tokio::time::Duration::from_millis(50));
    rerender_flush_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        // Session-alive check: if the strong refs to
        // OrgSyncIdleSignal have all been dropped, the
        // owning FrontendSession is gone — exit.
        let Some(idle_signal_for_task) = idle_signal_weak.upgrade() else {
            info!("[OrgMode] file-watcher loop exiting (session dropped)");
            return;
        };
        tokio::select! {
            Some((maybe_path, change_seq)) = file_rx.recv() => {
                if let Some(file_path) = maybe_path {
                    tracing::debug!("[ORGSYNC_TRACE] file_rx -> on_file_changed({})", file_path.display());
                    if let Err(e) = controller.on_file_changed(&file_path).await {
                        tracing::debug!(
                            "[ORGSYNC_TRACE] on_file_changed ERROR for {}: {}",
                            file_path.display(), e
                        );
                        error!(
                            "[OrgMode] File change error {}: {}",
                            file_path.display(), e
                        );
                    } else {
                        tracing::debug!("[ORGSYNC_TRACE] on_file_changed OK for {}", file_path.display());
                    }
                    idle_signal_for_task.mark_progress();
                }
                // Advance even on error / filtered events: the change was
                // handled (errors are surfaced above); a wedged watermark
                // would turn one logged failure into every later
                // wait_for_change_seq timing out.
                idle_signal_for_task.advance_change_seq(change_seq);
            }
            _ = poll_tick.tick() => {
                match controller.poll_tracked_files().await {
                    Ok(n) if n > 0 => {
                        tracing::debug!("[ORGSYNC_TRACE] poll ingested {} file(s)", n);
                        idle_signal_for_task.mark_progress();
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::debug!("[ORGSYNC_TRACE] poll ERROR: {}", e);
                        error!("[OrgMode] poll_tracked_files error: {}", e);
                    }
                }
            }
            _ = discovery_tick.tick() => {
                match controller.poll_new_files().await {
                    Ok(n) if n > 0 => {
                        tracing::debug!("[ORGSYNC_TRACE] discovery ingested {} new file(s)", n);
                        idle_signal_for_task.mark_progress();
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::debug!("[ORGSYNC_TRACE] discovery ERROR: {}", e);
                        error!("[OrgMode] poll_new_files error: {}", e);
                    }
                }
            }
            Some(rerender) = rerender_rx.recv() => {
                let span = tracing::info_span!("org.on_block_feed");
                async {
                    match rerender {
                        OrgRerender::Block { doc, delta } => {
                            match controller.on_block_changed(&doc, &delta).await {
                                Ok(true) => {}
                                // ALLOW(fallback): doc resolved to no tracked file → full re-render
                                Ok(false) => { pending_full_rerender = true; }
                                Err(e) => {
                                    error!(
                                        "[OrgMode] Block change error for {}: {}",
                                        doc, e
                                    );
                                }
                            }
                        }
                        OrgRerender::Remove { id } => {
                            match controller.on_block_removed(&id).await {
                                Ok(true) => {}
                                // No tracked projection contains the block (deleted page
                                // owning its own file / cold cache) → bulk recovery pass,
                                // CARRYING the id so the removal stays op-grounded.
                                Ok(false) => {
                                    pending_sanctioned_removals.insert(id.as_str().to_string());
                                    pending_full_rerender = true;
                                }
                                Err(e) => {
                                    error!(
                                        "[OrgMode] Block removal error for {}: {}",
                                        id, e
                                    );
                                }
                            }
                        }
                        OrgRerender::All => { pending_full_rerender = true; }
                    }
                }.instrument(span).await;
                idle_signal_for_task.mark_progress();
            }
            _ = rerender_flush_tick.tick(), if pending_full_rerender => {
                pending_full_rerender = false;
                let sanctioned = std::mem::take(&mut pending_sanctioned_removals);
                if let Err(e) = controller.re_render_all_tracked(&sanctioned).await {
                    error!("[OrgMode] re_render_all_tracked (debounced) error: {}", e);
                }
            }
        }
    }
}
