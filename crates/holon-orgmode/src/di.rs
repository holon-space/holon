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
use holon_api::EntityUri;
use holon_api::block::Block;
use holon_core::block_ordering::BlockOrdering;
use holon_filesystem::AliasRegistrar;
use holon_filesystem::BlockDelta;
use holon_filesystem::BlockReader;
use holon_filesystem::DocumentManager;
use holon_filesystem::FileSyncController;

use crate::file_watcher::FileEvent;
use crate::file_watcher::VaultFileWatcher;
use crate::home_authority::DocHome;
use crate::org_renderer::OrgRenderer;

/// Documents written back from the write-back holder in this process.
///
/// Liveness evidence, not a metric. The holder is the ONLY write-back path
/// now, so a suite that asserts on-disk org content while this stays at zero
/// proved nothing about it — the content it checked came from the ingest
/// direction or from the authoritative bulk pass. Suites that mean to exercise
/// write-back gate on this being non-zero.
static DOCS_WRITTEN_FROM_HOLDER: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

fn note_doc_written_from_holder() {
    DOCS_WRITTEN_FROM_HOLDER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// How many documents this process wrote back from the holder.
pub fn docs_written_from_holder() -> u64 {
    DOCS_WRITTEN_FROM_HOLDER.load(std::sync::atomic::Ordering::Relaxed)
}

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

/// Scan a directory recursively for vault files of any registered format.
///
/// Delegates to `file_watcher::scan_directory` — the gitignore-aware walk
/// behind the `FileSystem` port (ADR 0011), filtered to the registry's union.
async fn scan_vault_files(
    fs: &dyn holon_filesystem::FileSystem,
    dir: &std::path::Path,
    formats: &holon_core::FormatRegistry,
) -> std::io::Result<Vec<PathBuf>> {
    Ok(crate::file_watcher::scan_directory(fs, dir, formats)
        .await?
        .files)
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
    // Seeding is an ORG concern — it ships org assets — so emptiness is judged
    // over org files alone. A vault holding only foreign-format documents has
    // no org page yet and still wants the shipped layout.
    let scanned = crate::file_watcher::scan_directory(
        fs,
        root,
        &crate::file_sync_controller::org_only_format_registry(),
    )
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

/// The container's registered file formats.
///
/// A container that binds a [`FormatRegistry`] (the app wiring does, with org
/// + cook) gets it verbatim. Otherwise the vault is single-format: a container
/// may still override the ONE adapter, and failing that it is org.
async fn resolve_format_registry(resolver: &Injector) -> Arc<holon_core::FormatRegistry> {
    if let Some(registry) = resolver
        .optional_resolve_async::<holon_core::FormatRegistry>()
        .await
    {
        return registry;
    }

    let classifier = resolver
        .optional_resolve_async::<holon_api::link_parser::LinkTargetClassifier>()
        .await
        .map(|c| (*c).clone());
    let adapter = resolver
        .optional_resolve_async::<dyn holon_core::FileFormatAdapter>()
        .await
        .unwrap_or_else(|| {
            // The container binds a registry-backed classifier; without one the
            // adapter falls back to the built-in entity schemes.
            let classifier = classifier.unwrap_or_default();
            Arc::new(crate::file_format::OrgFormatAdapter::with_classifier(
                classifier,
            )) as Arc<dyn holon_core::FileFormatAdapter>
        });
    Arc::new(
        holon_core::FormatRegistry::new(vec![adapter])
            .expect("one adapter cannot contest its own extensions"),
    )
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
    // The rename-pairing buffer inside the adapter gates on the vault's
    // extensions, so it is built from the registry rather than from `.org`:
    // otherwise a `.cook` rename reads as a foreign interposer.
    match injector.try_provide::<dyn holon_filesystem::FileChangeSource>(Provider::root(
        |resolver| {
            // SYNC resolve, deliberately: this port is also resolved from sync
            // contexts, and an async provider makes every one of those fail
            // with `AsyncFactoryRequiresAsyncResolve`. The composition root
            // binds the registry with a sync provider; a container binding
            // none watches org alone.
            let extensions: Vec<String> = match resolver.try_resolve::<holon_core::FormatRegistry>()
            {
                Ok(formats) => formats.extensions().map(|e| e.to_string()).collect(),
                Err(_) => vec!["org".to_string()],
            };
            Arc::new(
                holon_filesystem::NotifyWatcher::new_unarmed_for_extensions(extensions)
                    .expect("notify watcher construction failed"),
            ) as Arc<dyn holon_filesystem::FileChangeSource>
        },
    )) {
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

    // The write-back render, resolvable on its own so inspection callers (the
    // `render_org` MCP tool) answer from the same code path — and the same
    // seams — the FileSyncController writes through. Stateless, so this
    // instance and the controller's are interchangeable.
    injector.provide::<holon_filesystem::WritebackRenderer>(Provider::root_async(
        |resolver| async move {
            Shared::new(holon_filesystem::WritebackRenderer::new(
                resolver.resolve_async::<dyn BlockReader>().await,
                resolver.resolve_async::<dyn DocumentManager>().await,
                resolve_format_registry(&resolver).await,
            ))
        },
    ));

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
                let formats = resolve_format_registry(&resolver).await;
                let formats_for_loop = formats.clone();
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

                // Disclosure seam for shared-subtree write-back gaps (Inc 1).
                // Present in the app container (forwards to `DegradedSignalBus`);
                // absent in test/no-Turso containers — then shared-subtree edits
                // that fail to materialize log at WARN instead of a banner.
                let share_disclosure = resolver
                    .optional_resolve_async::<dyn holon_filesystem::ShareWritebackDisclosure>()
                    .await;

                // Disclosure seam for the write-back supervisor giving up.
                // Absent in test/no-Turso containers → a spent restart budget is
                // audible in the log only.
                let writeback_disclosure = resolver
                    .optional_resolve_async::<dyn holon_filesystem::WritebackDisclosure>()
                    .await;

                // Authoritative mount registry (Inc 3): lets the ingest guard
                // skip a shared-subtree projection file only when its page id is
                // a real mount node (not a user-authored `share-role` drawer).
                // Absent in test/SqlOnly containers → the guard never skips.
                let mount_registry = resolver
                    .optional_resolve_async::<dyn holon_filesystem::MountRegistry>()
                    .await;

                // Writer for the declared-type rows a format derives from a
                // file. Absent in the no-Turso / org-standalone containers,
                // where no registered format emits any — a format that does
                // then refuses to ingest rather than dropping them.
                let typed_row_sink = resolver
                    .optional_resolve_async::<dyn holon_core::file_format::TypedRowSink>()
                    .await;

                let idle_signal_weak = std::sync::Arc::downgrade(&idle_signal);

                // Keep an authoritative-read handle for the feed resolver BEFORE
                // the reader is moved into the controller: block routing must
                // read the `Page` tag from the write authority (`block_raw`), not
                // the lagging matview-backed feed (see `resolve_doc_for_block`).
                let feed_block_reader = block_reader.clone();
                // The write-back holder's order authority. The same handle the
                // controller writes through, so `prev` and the rendered sibling
                // order can never come from two different stores.
                let home_ordering = ordering.clone();

                let mut controller = FileSyncController::with_formats(
                    block_reader,
                    doc_manager,
                    config.root_directory.clone(),
                    formats,
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
                if let Some(registry) = mount_registry {
                    controller = controller.with_mount_registry(registry);
                }
                if let Some(sink) = typed_row_sink {
                    controller = controller.with_typed_row_sink(sink);
                }
                // Shared-subtree materialization gaps are disclosed from the
                // WRITE-BACK path inside the controller — it is the only layer
                // that sees a real write attempt and knows the file the shared
                // content landed in.
                if let Some(disclosure) = share_disclosure {
                    controller = controller.with_share_disclosure(disclosure);
                }
                // The same seam the supervisor's give-up uses. The controller
                // raises it for a different persistent failure with the same
                // consequence: one document's fold stops converging, so its
                // edits stop reaching disk while everything else keeps working.
                if let Some(disclosure) = writeback_disclosure.clone() {
                    controller = controller.with_writeback_disclosure(disclosure);
                }

                // Image bytes for `[[file:…]]` blocks, so an image synced from a
                // peer materializes on disk. Absent in every container today →
                // `materialize_images` / `ingest_images` stay no-ops.
                if let Some(images) = resolver
                    .optional_resolve_async::<dyn holon_filesystem::ImageDataProvider>()
                    .await
                {
                    controller = controller.with_image_data(images);
                }

                // R3b: wire the C2b history store so the org-ingest doc-page
                // create records provenance (absent in org-standalone/no-Turso
                // wirings). Plus the injected clock, when a test provides one, so
                // the ingest history `at_millis` is deterministic under replay.
                if let Some(history) = resolver
                    .optional_resolve_async::<dyn holon_api::HistoryStore>()
                    .await
                {
                    controller = controller.with_history_store(history);
                }
                if let Some(injected) = resolver
                    .optional_resolve_async::<holon_api::InjectedClock>()
                    .await
                {
                    controller = controller.with_clock(injected.0.clone());
                }

                let (rerender_tx, rerender_rx) =
                    tokio::sync::mpsc::unbounded_channel::<RerenderMsg>();
                if let Some(feed) = block_feed.clone() {
                    let tx = rerender_tx.clone();
                    // The authority's reads happen while the combinator is
                    // still folding, so they PEEK: the write-back pass below
                    // is the one that takes the provenance.
                    let feed_for_provenance = feed.clone();
                    let authority = Arc::new(
                        crate::home_authority::BlockHomeAuthority::new(
                            feed_block_reader.clone(),
                            home_ordering,
                        )
                        .with_provenance(Arc::new(move |id: &str| {
                            feed_for_provenance.provenance_for(id)
                        })),
                    );
                    let degraded = writeback_disclosure.clone();
                    let feed_for_stream = feed.clone();

                    // The write-back document mirror IS the `home_by`
                    // combinator's output, under let-it-die supervision: the
                    // supervisor holds the stream FACTORY, so a dead
                    // incarnation is replaced by a fresh subscription that
                    // re-seeds itself from the feed's `MapDiff::Replace`. Boot
                    // and recovery are structurally the same path — `Reset`
                    // precedes every incarnation, including the first.
                    let mut supervised = holon_api::live_data::supervision::spawn_supervised(
                        "org-writeback",
                        move || feed_for_stream.home_by(authority.clone()),
                        move |component, restarts, err| {
                            // A spent restart budget means edits stop reaching
                            // disk for the rest of the process — exactly the
                            // persistent-failure case let-it-die exists to
                            // disclose, so it must be audible outside the log.
                            match degraded.as_deref() {
                                Some(d) => d.writeback_degraded(&format!(
                                    "{component} gave up after {restarts} restarts: {err:#}"
                                )),
                                None => tracing::error!(
                                    "[{component}] gave up after {restarts} restarts ({err:#}) — \
                                     org write-back is DOWN for the rest of this process and no \
                                     disclosure seam is wired in this container"
                                ),
                            }
                        },
                    );

                    tokio::spawn(async move {
                        use holon_api::live_data::home_by::HomedDiff;
                        use holon_api::live_data::supervision::Supervised;

                        // The initial snapshot fans out one `Upsert` per block.
                        // Rendering each of them would be N per-block boot
                        // renders, which destabilises the cold-boot matview (the
                        // frontend's creation slot then resolves NO parent). So
                        // snapshot blocks SEED the holder and one debounced bulk
                        // pass renders them together; the set drains as the
                        // snapshot is consumed, after which everything routes
                        // per block.
                        let mut snapshot_pending: std::collections::HashSet<String> =
                            std::collections::HashSet::new();

                        while let Some(item) = supervised.recv().await {
                            // The interaction that wrote this block, taken from
                            // the feed rather than from the ambient span: this
                            // task is spawned at container construction, so its
                            // span is the process boot.
                            let origins = match &item {
                                Supervised::Diff(HomedDiff::Upsert { key, .. })
                                | Supervised::Diff(HomedDiff::Remove { key, .. }) => {
                                    feed.take_provenance(key)
                                }
                                Supervised::Reset => Vec::new(),
                            };
                            // `None` routes nothing at all — see
                            // [`route_homed_block`].
                            let msg: Option<OrgRerender> = match item {
                                Supervised::Reset => {
                                    snapshot_pending = feed.read().keys().cloned().collect();
                                    // Drop the dead incarnation's derived state,
                                    // then cover the incoming seed with one bulk
                                    // render off the authority.
                                    let _ = tx.send(RerenderMsg::unattributed(OrgRerender::Reset));
                                    Some(OrgRerender::All)
                                }
                                Supervised::Diff(HomedDiff::Upsert {
                                    doc,
                                    key,
                                    prev,
                                    value,
                                }) => {
                                    let seeding = snapshot_pending.remove(&key);
                                    route_upsert(&doc, &value, prev.as_deref(), seeding)
                                }
                                Supervised::Diff(HomedDiff::Remove { doc, key }) => {
                                    // The DEPARTURE. A retraction always lands
                                    // before the matching `Upsert` at the new
                                    // document, so the source document
                                    // re-renders WITHOUT the block whether it
                                    // was deleted or merely re-homed — one
                                    // uniform path, and no authoritative
                                    // presence-check that would short-circuit a
                                    // move.
                                    snapshot_pending.remove(&key);
                                    // A departure carries no block value, so the
                                    // proposal predicates have nothing to read:
                                    // retracting a proposal still arms the bulk
                                    // pass. Unchanged here deliberately.
                                    match (doc, EntityUri::parse(&key)) {
                                        (DocHome::Resolved(doc), Ok(id)) => {
                                            Some(OrgRerender::Block {
                                                doc,
                                                delta: Box::new(BlockDelta::Remove(id)),
                                            })
                                        }
                                        (DocHome::Unresolved, _) => Some(OrgRerender::All),
                                        (DocHome::Resolved(_), Err(e)) => {
                                            tracing::error!(
                                                "[OrgMode] block feed Remove key {key:?} is not a \
                                                 valid EntityUri: {e} — falling back to full \
                                                 re-render"
                                            );
                                            Some(OrgRerender::All)
                                        }
                                    }
                                }
                            };
                            if let Some(rerender) = msg {
                                let _ = tx.send(RerenderMsg { rerender, origins });
                            }
                        }
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
                    formats_for_loop,
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
    /// Fold this homed diff into the document's holder entry, then write the
    /// document back.
    Block {
        doc: EntityUri,
        delta: Box<holon_filesystem::BlockDelta>,
    },
    /// Fold into the holder WITHOUT writing: this diff belongs to a fresh
    /// subscription's initial snapshot, and the `All` that accompanies the
    /// snapshot renders every one of them in a single bulk pass. Per-block boot
    /// renders destabilise the cold-boot matview.
    Seed {
        doc: EntityUri,
        delta: Box<holon_filesystem::BlockDelta>,
    },
    /// The supervised stream restarted (or booted): drop all derived state. A
    /// complete re-seed follows on the next incarnation.
    Reset,
    /// No document could be resolved for a change, or a re-seed just landed —
    /// converge via a debounced re-render of every tracked file, read from the
    /// authority rather than the holder.
    All,
}

/// A re-render request plus the interactions that caused it.
///
/// The block feed publishes its diffs through a `futures-signals` map, which
/// carries values only, and both ends of this channel are tasks spawned at
/// container construction — so the writing context has to travel ON the
/// message or not at all.
pub struct RerenderMsg {
    pub rerender: OrgRerender,
    /// The writing contexts of the CDC batch behind this request: the parent
    /// first, consolidated co-writers after (ruling D3.a). Empty for boot
    /// seeding and for stream restarts, which no interaction asked for.
    pub origins: Vec<holon_api::BatchTraceContext>,
}

impl RerenderMsg {
    /// A request no interaction can claim.
    pub fn unattributed(rerender: OrgRerender) -> Self {
        Self {
            rerender,
            origins: Vec::new(),
        }
    }
}

/// The projection span for one coalesced write-back pass, tied to the
/// interactions the pass serves.
///
/// Attribution has to REPLACE the ambient parent — this runs on the controller
/// task, spawned at container construction, so that parent is the process boot,
/// the orphan this exists to remove. With nothing to bill (boot seeding, a
/// stream restart) the pass keeps that ambient parent instead: a boring parent
/// beats no parent.
pub fn block_feed_pass_span(drained: &[RerenderMsg]) -> tracing::Span {
    let origins: Vec<holon_api::BatchTraceContext> = drained
        .iter()
        .flat_map(|m| m.origins.iter().cloned())
        .collect();
    let contexts = holon_api::BatchTraceContext::resolve_all(&origins);
    if contexts.is_empty() {
        return tracing::info_span!("org.on_block_feed", messages = drained.len());
    }
    let span = tracing::info_span!(
        parent: None,
        "org.on_block_feed",
        messages = drained.len(),
    );
    holon_api::BatchTraceContext::attribute(&span, &contexts, "org.on_block_feed");
    span
}

/// Parse the combinator's document-relative previous-sibling id.
///
/// `home_by` carries ids as strings; the holder keys on [`EntityUri`]. An
/// unparseable id is a genuine defect in whatever minted it, so it surfaces as
/// an `Err` the caller escalates to a full re-render — never silently as
/// "first in its sibling group", which would reorder the document.
fn parse_prev(prev: Option<&str>) -> anyhow::Result<Option<EntityUri>> {
    match prev {
        None => Ok(None),
        Some(p) => Ok(Some(EntityUri::parse(p)?)),
    }
}

/// Where one block the feed homed is routed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockRoute {
    /// Render into this document.
    Document(EntityUri),
    /// Route nothing at all: the block is not vault content.
    Drop,
    /// No document owns the block — converge via the authoritative bulk pass.
    Recover,
}

/// The routing decision for one homed block, as a pure function of its home
/// and its own fields.
///
/// `Recover` is the designed recovery for vault content whose document cannot
/// be resolved, and it costs a re-render of EVERY tracked file. Proposal blocks
/// would take it on every write: the trust gate mints them under a parentless,
/// non-page place, so they home `Unresolved` by construction rather than by
/// fault. They are not vault content and route nowhere; every other block keeps
/// the recovery behaviour unchanged.
pub fn route_homed_block(
    home: &DocHome,
    id: &EntityUri,
    parent_id: &EntityUri,
    properties: &std::collections::HashMap<String, holon_api::Value>,
) -> BlockRoute {
    if holon_api::is_proposal_block(properties) || holon_api::is_proposals_place(id, parent_id) {
        return BlockRoute::Drop;
    }
    match home {
        DocHome::Resolved(doc) => BlockRoute::Document(doc.clone()),
        DocHome::Unresolved => BlockRoute::Recover,
    }
}

/// The re-render request one homed `Upsert` becomes, or `None` when it routes
/// nowhere.
///
/// This is the production call site of [`route_homed_block`]: the feed loop
/// does no routing of its own, so a test driving this drives the real decision
/// AND the message the loop acts on, rather than a transcription of either.
pub fn route_upsert(
    home: &DocHome,
    block: &Block,
    prev: Option<&str>,
    seeding: bool,
) -> Option<OrgRerender> {
    let route = route_homed_block(home, &block.id, &block.parent_id, &block.properties);
    match (route, parse_prev(prev)) {
        (BlockRoute::Document(doc), Ok(prev)) => {
            let delta = Box::new(BlockDelta::Upsert {
                block: block.clone(),
                prev,
            });
            Some(if seeding {
                OrgRerender::Seed { doc, delta }
            } else {
                OrgRerender::Block { doc, delta }
            })
        }
        // Not vault content: no document renders and no recovery pass arms.
        (BlockRoute::Drop, _) => None,
        // No `Page` ancestor, or the authority faulted: the block belongs to no
        // document we can write, so recover through the authoritative bulk pass.
        (BlockRoute::Recover, _) => Some(OrgRerender::All),
        (BlockRoute::Document(doc), Err(e)) => {
            tracing::error!(
                "[OrgMode] home_by homed block {} to {doc} with an unparseable \
                 previous sibling: {e} — falling back to full re-render",
                block.id
            );
            Some(OrgRerender::All)
        }
    }
}

/// Backend-blind FileSyncController driver: initialize, build the file watcher,
/// run the initial scan, signal readiness, arm the watcher, and run the main
/// `select!` loop. Shared by the Turso factory and the no-Turso bootstrap —
/// neither path knows which storage backend the controller's adapters use.
pub async fn run_file_sync_controller(
    mut controller: FileSyncController,
    root_directory: PathBuf,
    idle_signal_weak: std::sync::Weak<OrgSyncIdleSignal>,
    mut rerender_rx: tokio::sync::mpsc::UnboundedReceiver<RerenderMsg>,
    ready_sender: std::sync::Arc<std::sync::Mutex<Option<FileWatcherReadySender>>>,
    fs: Arc<dyn holon_filesystem::FileSystem>,
    change_source: Arc<dyn holon_filesystem::FileChangeSource>,
    formats: Arc<holon_core::FormatRegistry>,
) {
    use tracing::Instrument;
    use tracing::error;
    use tracing::info;

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
    let mut file_rx = tracing::info_span!("vault.startup.file_watcher_new_unarmed")
        .in_scope(|| {
            VaultFileWatcher::new(change_source.as_ref(), &root_directory, formats.clone())
        })
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
        let org_files = match scan_vault_files(fs.as_ref(), &root_directory, &formats).await {
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
            tracing::info!(
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
        // Store-health sweep (BugFunnel row 295): repair title-less
        // (empty-content) `Page` doc-roots left by a broken convert/delete.
        // UNCONDITIONAL, after the scan — the ingest byte-identity fast-path skips
        // unchanged degraded files, so their heal cannot live in ingest; it is a
        // separate store-health concern owned by this one sweep (which reaches the
        // same heal implementation the file-watch path uses). Idempotent; a
        // healthy vault writes nothing.
        if let Err(e) = controller.heal_title_less_doc_roots().await {
            error!(
                "[OrgMode] title-less doc-root store-health sweep failed: {}",
                e
            );
            failures.push((root_directory.clone(), e));
        }
        // Boot seed/re-seed phase is over. From here a runtime user edit to a
        // copy-on-write seed doc (e.g. `block:__default__`) materializes its
        // vault file (copy-on-write); every boot re-seed write stayed virtual.
        controller.finish_boot_seeding();
        tracing::info!(
            target: "holon_latency",
            stage = "boot_ingest_total",
            ms = t_scan.elapsed().as_millis() as u64,
            files = files as u64,
            "holon_latency",
        );
        tracing::info!(
            "[OrgMode] initial scan complete: {} file(s) in {}ms, {} failure(s)",
            files,
            t_scan.elapsed().as_millis(),
            failures.len(),
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
            Some((maybe_evt, change_seq)) = file_rx.recv() => {
                match maybe_evt {
                    Some(FileEvent::Changed(file_path)) => {
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
                    Some(FileEvent::Renamed { from, to }) => {
                        tracing::debug!("[ORGSYNC_TRACE] file_rx -> on_file_renamed({} -> {})", from.display(), to.display());
                        if let Err(e) = controller.on_file_renamed(&from, &to).await {
                            tracing::debug!(
                                "[ORGSYNC_TRACE] on_file_renamed ERROR for {} -> {}: {}",
                                from.display(), to.display(), e
                            );
                            error!(
                                "[OrgMode] File rename error {} -> {}: {}",
                                from.display(), to.display(), e
                            );
                        } else {
                            tracing::debug!("[ORGSYNC_TRACE] on_file_renamed OK for {} -> {}", from.display(), to.display());
                        }
                        idle_signal_for_task.mark_progress();
                    }
                    None => {}
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
            Some(first) = rerender_rx.recv() => {
                // Drain everything the channel already holds before doing any
                // I/O. The feed fans one message per member, so a single page
                // toggle or re-home arrives as a burst that is fully queued by
                // the time the first message wakes this loop. Rendering per
                // message made that burst cost one render per member; draining
                // first lets `on_block_changed_coalesced` spend one render per
                // DOCUMENT. Pure latency win — nothing waits for a timer, and a
                // lone message drains to a batch of one.
                let mut drained = vec![first];
                while let Ok(next) = rerender_rx.try_recv() {
                    drained.push(next);
                }
                let span = block_feed_pass_span(&drained);
                async {
                    // Order is preserved and `Reset` is a barrier: it means the
                    // stream that produced everything before it is gone, so any
                    // fold accumulated earlier in this batch must be discarded
                    // with the holder rather than rendered after the reset.
                    let mut pending_blocks: Vec<(EntityUri, BlockDelta)> = Vec::new();
                    for msg in drained {
                        match msg.rerender {
                            OrgRerender::Block { doc, delta } => {
                                pending_blocks.push((doc, *delta));
                            }
                            OrgRerender::Seed { doc, delta } => {
                                controller.apply_block_delta(&doc, &delta);
                            }
                            OrgRerender::Reset => {
                                pending_blocks.clear();
                                controller.reset_holder();
                            }
                            OrgRerender::All => { pending_full_rerender = true; }
                        }
                    }

                    if !pending_blocks.is_empty() {
                        for (doc, verdict) in
                            controller.on_block_changed_coalesced(&pending_blocks).await
                        {
                            match verdict {
                                Ok(true) => { note_doc_written_from_holder(); }
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
                    }
                }.instrument(span).await;
                idle_signal_for_task.mark_progress();
            }
            _ = rerender_flush_tick.tick(), if pending_full_rerender => {
                pending_full_rerender = false;
                // Recovery reseeds carry no per-block Remove intent — departures and
                // deletions route as `OrgRerender::Block { Remove }` and ground per
                // file via `on_block_changed`, so the bulk pass never needs a
                // sanctioned set.
                if let Err(e) = controller
                    .re_render_all_tracked(&std::collections::HashSet::new())
                    .await
                {
                    error!("[OrgMode] re_render_all_tracked (debounced) error: {}", e);
                }
            }
        }
    }
}
