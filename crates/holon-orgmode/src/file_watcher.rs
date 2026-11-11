//! File watcher for vault files
//!
//! Bridges the injected [`FileChangeSource`] port (ADR 0011) to the vault sync
//! loop: subscribes to raw file-change events, filters to files of a
//! REGISTERED format that are not gitignored (including always skipping
//! `.git/` and `.jj/`), and forwards the paths on an unbounded mpsc channel.
//!
//! The admitted extension set is the union of the [`FormatRegistry`]'s
//! adapters, so a vault holding `.org` pages and `.cook` recipes watches both.
//!
//! Echo suppression lives in `FileSyncController::last_projection`, not here.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use holon_core::CanonicalPath;
use holon_core::FormatRegistry;
use holon_filesystem::FileChange;
use holon_filesystem::FileChangeKind;
use holon_filesystem::FileChangeSource;
use holon_filesystem::FileSystem;
pub use holon_filesystem::ScannedEntries;
use holon_filesystem::vault_filter::VaultFilter;
use tokio::sync::mpsc;
use tracing::debug;
use tracing::warn;

/// Scan a directory for vault files through the `FileSystem` port (ADR 0011).
///
/// The gitignore-aware recursive walk lives in the port
/// (`holon_filesystem::fs_port::walk_directory` for the real adapter); this
/// wrapper keeps only the files a registered format claims.
pub async fn scan_directory(
    fs: &dyn FileSystem,
    root: &Path,
    formats: &FormatRegistry,
) -> std::io::Result<ScannedEntries> {
    let mut scanned = fs.scan_directory(root).await?;
    scanned.files.retain(|p| formats.handles(p));
    Ok(scanned)
}

fn is_ignored(path: &Path, filter: &VaultFilter) -> bool {
    let Some(refusal) = filter.refusal(path) else {
        return false;
    };
    debug!(
        path = %path.display(),
        reason = %refusal,
        "[VaultFileWatcher] outside the vault — not ingested, matching the boot walk"
    );
    true
}

/// An org-relevant file event the sync loop must act on.
///
/// `Changed` is the Modify/Create/Remove path (`on_file_changed`, which stats
/// the path and routes create-vs-delete). `Renamed` carries BOTH sides of an
/// atomic `mv` so the loop can re-home the document via `on_file_renamed`
/// WITHOUT the delete-then-create window that lets a rename be mis-read as a
/// delete (and cascade-delete the re-homed doc).
#[derive(Debug, Clone)]
pub enum FileEvent {
    /// A single-path change: the sync loop calls `on_file_changed`.
    Changed(PathBuf),
    /// An atomic rename: the sync loop calls `on_file_renamed(from, to)`.
    Renamed { from: PathBuf, to: PathBuf },
}

/// Whether `path` is a vault file the sync loop should track (an extension a
/// registered format claims, not gitignored / VCS-internal).
fn is_vault_relevant(path: &Path, formats: &FormatRegistry, filter: &VaultFilter) -> bool {
    formats.handles(path) && !is_ignored(path, filter)
}

/// Map one raw [`FileChange`] to the org-relevant [`FileEvent`] the sync loop
/// acts on, or `None` when it is filtered (non-`.org`, gitignored). This is the
/// single source of truth for the bridge's kind→event routing — exposed so a
/// test can drive SYNTHETIC notify-shaped changes through the SAME routing the
/// production bridge uses (the ENVIRONMENT-parity rung for pairing's degraded
/// path, see docs/Testing/BugFunnel.md 2026-07-27).
///
/// `is_relevant` decides whether a path is one the vault side tracks; the
/// bridge passes a [`VaultFilter`]-backed predicate, a focused test may pass an
/// extension check.
pub fn classify_change_to_event(
    change: FileChange,
    is_relevant: &dyn Fn(&Path) -> bool,
) -> Option<FileEvent> {
    match change.kind {
        FileChangeKind::Rename { from } => {
            let to = change.path;
            if is_relevant(&to) {
                debug!(
                    "File rename detected: {} -> {}",
                    from.display(),
                    to.display()
                );
                Some(FileEvent::Renamed { from, to })
            } else if is_relevant(&from) {
                // Renamed OUT of vault-space (`.org` -> `.txt`): the org side sees
                // only the departure, so treat it as a change to the vanished
                // `from` (stats NotFound -> delete).
                debug!(
                    "Org file renamed out of org-space: {} -> {}",
                    from.display(),
                    to.display()
                );
                Some(FileEvent::Changed(from))
            } else {
                None
            }
        }
        _ => {
            let path = change.path;
            if is_relevant(&path) {
                debug!("File change detected: {}", path.display());
                Some(FileEvent::Changed(path))
            } else {
                None
            }
        }
    }
}

/// File watcher for Org files: the org-side consumer of a [`FileChangeSource`].
///
/// The channel carries `(Option<FileEvent>, seq)`: `Some(event)` for
/// org-relevant changes the sync loop must ingest, `None` for filtered ones
/// (non-`.org`, gitignored). Filtered events still flow through the SAME
/// channel so the consumer can advance its processed-seq watermark strictly in
/// delivery order — advancing for a filtered event from the bridge directly
/// could overtake an unprocessed earlier forwarded event.
pub struct VaultFileWatcher {
    change_rx: mpsc::UnboundedReceiver<(Option<FileEvent>, u64)>,
}

impl VaultFileWatcher {
    /// Subscribe to `source` and spawn the filter bridge. Subscribing happens
    /// here — before the caller arms the source — so no event is missed.
    ///
    /// The gitignore root is canonicalized so it matches the canonical paths
    /// fs event backends report (macOS: `/var` → `/private/var`).
    pub fn new(
        source: &dyn FileChangeSource,
        watch_dir: &Path,
        formats: Arc<FormatRegistry>,
    ) -> Self {
        // The fs event backends report canonical paths (macOS: `/var` →
        // `/private/var`), so both the gitignore root and the hidden-segment
        // root must be canonical or neither matches what arrives.
        let root = CanonicalPath::new(watch_dir).into_path_buf();
        let filter = tracing::info_span!("VaultFileWatcher.build_vault_filter")
            .in_scope(|| VaultFilter::for_root(&root));
        let (change_tx, change_rx) = mpsc::unbounded_channel();
        let mut source_rx = source.subscribe();

        tokio::spawn(async move {
            loop {
                match source_rx.recv().await {
                    Ok(change) => {
                        let seq = change.seq;
                        let msg = classify_change_to_event(change, &|p| {
                            is_vault_relevant(p, &formats, &filter)
                        });
                        if change_tx.send((msg, seq)).is_err() {
                            // Receiver dropped — sync loop is gone.
                            return;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        // Dropped raw events are repaired by the controller's
                        // poll backstops (poll_tracked_files / poll_new_files).
                        warn!("[VaultFileWatcher] lagged behind change source by {n} events");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                }
            }
        });

        Self { change_rx }
    }

    /// Get a receiver for file change events
    pub fn receiver(&mut self) -> &mut mpsc::UnboundedReceiver<(Option<FileEvent>, u64)> {
        &mut self.change_rx
    }

    /// Consume the watcher and return the filtered-path receiver.
    pub fn into_receiver(self) -> mpsc::UnboundedReceiver<(Option<FileEvent>, u64)> {
        self.change_rx
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::future::Future;

    use holon_filesystem::NotifyWatcher;
    use proptest::prelude::*;
    use tempfile::TempDir;
    use tokio::sync::broadcast;
    use tokio::sync::mpsc;
    use tokio::time::Duration;
    use tokio::time::Instant;

    use super::*;

    /// How long a live test waits for FSEvents; a loaded machine delays
    /// delivery by seconds.
    const LIVE_BUDGET: Duration = Duration::from_secs(30);

    /// A quiet spell after which [`collect_until`] rewrites its nudge file.
    const QUIET: Duration = Duration::from_millis(500);

    /// The REAL `NotifyWatcher` (fsevents on macOS), with every change handed
    /// on `delay` late and in order: the shape of a loaded `fseventsd`.
    struct DelayedRelay {
        watcher: NotifyWatcher,
        tx: broadcast::Sender<FileChange>,
    }

    impl DelayedRelay {
        fn new(delay: Duration) -> Self {
            let watcher = NotifyWatcher::new_unarmed().unwrap();
            let mut raw = watcher.subscribe();
            let (tx, _) = broadcast::channel(4096);
            let (due_tx, mut due_rx) = mpsc::unbounded_channel();
            // Stamps each change on arrival, so a burst is not delayed once per
            // change.
            tokio::spawn(async move {
                loop {
                    match raw.recv().await {
                        Ok(change) => {
                            if due_tx.send((Instant::now() + delay, change)).is_err() {
                                return;
                            }
                        }
                        Err(broadcast::error::RecvError::Closed) => return,
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            panic!("the delayed relay lagged {n} changes behind the watcher")
                        }
                    }
                }
            });
            let relay_tx = tx.clone();
            tokio::spawn(async move {
                while let Some((due, change)) = due_rx.recv().await {
                    tokio::time::sleep_until(due).await;
                    let _ = relay_tx.send(change);
                }
            });
            Self { watcher, tx }
        }
    }

    impl FileChangeSource for DelayedRelay {
        fn subscribe(&self) -> broadcast::Receiver<FileChange> {
            self.tx.subscribe()
        }

        fn arm(&self, root: &Path) -> std::io::Result<()> {
            self.watcher.arm(root)
        }
    }

    /// The paths of the forwarded (`Some`) events, in arrival order.
    type Seen = Vec<PathBuf>;

    /// Collects forwarded paths until `done` holds or [`LIVE_BUDGET`] passes,
    /// rewriting `nudge` after each [`QUIET`] spell. Panics when no nudge came
    /// back, so a stalled `fseventsd` is not reported as a lost event.
    async fn collect_until(
        watcher: &mut VaultFileWatcher,
        nudge: &Path,
        done: impl Fn(&Seen) -> bool,
    ) -> Seen {
        let deadline = Instant::now() + LIVE_BUDGET;
        let mut seen = Seen::new();
        while !done(&seen) && Instant::now() < deadline {
            match tokio::time::timeout(QUIET, watcher.receiver().recv()).await {
                Ok(msg) => match msg.expect("watcher channel closed").0 {
                    Some(FileEvent::Changed(path)) | Some(FileEvent::Renamed { to: path, .. }) => {
                        seen.push(path)
                    }
                    None => {}
                },
                Err(_) => std::fs::write(nudge, b"* nudge").unwrap(),
            }
        }
        let nudge_name = nudge.file_name().unwrap().to_string_lossy();
        assert!(
            done(&seen) || seen.iter().any(|p| p == nudge),
            "the watch stalled: no event for {nudge_name} within {LIVE_BUDGET:?}: {seen:?}"
        );
        seen
    }

    /// Arms a delayed real watcher on `root` (canonical) and returns once the
    /// watch delivers: FSEvents drops what happens before the stream starts.
    /// The relay must outlive the test: dropping it stops delivery.
    async fn armed_watcher(root: &Path, delay: Duration) -> (DelayedRelay, VaultFileWatcher) {
        let source = DelayedRelay::new(delay);
        let mut watcher = VaultFileWatcher::new(
            &source,
            root,
            crate::file_sync_controller::org_only_format_registry(),
        );
        source.arm(root).unwrap();
        let live = root.join("live.org");
        collect_until(&mut watcher, &live, |s| s.contains(&live)).await;
        (source, watcher)
    }

    fn canonical_tempdir() -> (TempDir, PathBuf) {
        let temp = TempDir::new().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        (temp, root)
    }

    /// Runs `case` on a runtime of its own for the static example delay, then
    /// for generated delays. Each case drives the real `fseventsd`, so there
    /// are few.
    fn for_every_delay<F: Future<Output = ()>>(case: impl Fn(Duration) -> F) {
        let run = |delay: Duration| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(case(delay))
        };
        run(Duration::from_secs(1));
        let config = ProptestConfig {
            cases: 3,
            max_shrink_iters: 0,
            failure_persistence: None,
            ..ProptestConfig::default()
        };
        proptest!(config, |(delay_ms in 0u64..=3000)| run(Duration::from_millis(delay_ms)));
    }

    /// Writes `forbidden`, then a vault file `control.org`, then a marker.
    /// FSEvents delivers in order, so once the marker arrived any event for
    /// `forbidden` has arrived too, and `control.org` shows that the check sees
    /// a late vault event.
    async fn assert_never_forwarded(
        watcher: &mut VaultFileWatcher,
        root: &Path,
        forbidden: &Path,
        message: &str,
    ) {
        std::fs::write(forbidden, "* Hidden").unwrap();
        let control = root.join("control.org");
        std::fs::write(&control, "* Control").unwrap();
        let marker = root.join("marker.org");
        std::fs::write(&marker, "* Marker").unwrap();
        let seen = collect_until(watcher, &marker, |s| s.contains(&marker)).await;
        assert!(
            seen.contains(&control),
            "control.org did not arrive before the marker, so the check cannot see a late event: {seen:?}"
        );
        assert!(
            !seen.contains(&forbidden.to_path_buf()),
            "{message}: {seen:?}"
        );
    }

    #[test]
    fn test_file_watcher_detects_changes() {
        for_every_delay(|delay| async move {
            let (_temp, root) = canonical_tempdir();
            let (_source, mut watcher) = armed_watcher(&root, delay).await;

            let page = root.join("test.org");
            std::fs::write(&page, "* Test").unwrap();

            let seen =
                collect_until(&mut watcher, &root.join("nudge.org"), |s| s.contains(&page)).await;
            assert!(
                seen.contains(&page),
                "the watch delivered nudges but no event for test.org: {seen:?}"
            );
        });
    }

    #[test]
    fn test_file_watcher_ignores_git_dir() {
        for_every_delay(|delay| async move {
            let (_temp, root) = canonical_tempdir();
            std::fs::create_dir_all(root.join(".git")).unwrap();
            let (_source, mut watcher) = armed_watcher(&root, delay).await;

            assert_never_forwarded(
                &mut watcher,
                &root,
                &root.join(".git/test.org"),
                "Should NOT receive events from .git/",
            )
            .await;
        });
    }

    /// The live vault keeps agent jj-workspaces under `.claude/worktrees/`,
    /// each a full copy of the vault whose `Projects/Holon/Now.org` declares
    /// the same `#+ID:` and headline slugs as the live file. Those workspaces
    /// are written constantly.
    #[test]
    fn a_hidden_nested_vault_copy_is_never_ingested() {
        for_every_delay(|delay| async move {
            let (_temp, root) = canonical_tempdir();
            let copy = root.join(".claude/worktrees/agent-x/Projects/Holon");
            std::fs::create_dir_all(&copy).unwrap();
            let (_source, mut watcher) = armed_watcher(&root, delay).await;

            assert_never_forwarded(
                &mut watcher,
                &root,
                &copy.join("Now.org"),
                "a stale vault copy under .claude/worktrees/ reached ingest",
            )
            .await;
        });
    }

    /// The boot scan and the live watcher answer the same question — "may this
    /// path be ingested?" — through different code, so nothing but this test
    /// keeps them from drifting. They drifted: the scan delegated to `ignore`'s
    /// `hidden(true)` while the watcher matched `.git`/`.jj` by hand, and the
    /// live vault's 12 stale copies of itself under `.claude/worktrees/` were
    /// invisible at boot yet ingested on every touch (bugfunnel
    /// `2026-09-09-hidden-dir-vault-copy-invisible-at-boot-ingested-by-the-watcher`).
    /// What each leg answers for `corpus`, in the order walk / watcher /
    /// harness scan. Disk setup is the caller's — the harness leg populates its
    /// own in-memory vault from the same corpus.
    async fn every_legs_verdict(root: &Path, corpus: &[&str]) -> [BTreeSet<PathBuf>; 3] {
        let formats = crate::file_sync_controller::org_only_format_registry();
        let filter = VaultFilter::for_root(root);

        let walked = holon_filesystem::fs_port::walk_directory(root)
            .files
            .into_iter()
            .filter(|p| formats.handles(p))
            .collect();

        let watched = corpus
            .iter()
            .map(|rel| root.join(rel))
            .filter(|p| is_vault_relevant(p, &formats, &filter))
            .collect();

        let memory = holon_filesystem::InMemoryFileSystem::new();
        memory.mkdir_all(root);
        for rel in corpus {
            let path = root.join(rel);
            memory.mkdir_all(path.parent().unwrap());
            holon_filesystem::FileSystem::write(&memory, &path, b"* H")
                .await
                .unwrap();
        }
        let scanned = scan_directory(&memory, root, &formats)
            .await
            .unwrap()
            .files
            .into_iter()
            .collect();

        [walked, watched, scanned]
    }

    fn assert_legs_agree(legs: [BTreeSet<PathBuf>; 3]) {
        let [walked, watched, scanned] = legs;
        assert_eq!(
            watched,
            walked,
            "the watcher forwards paths the boot walk skips — those files are \
             ingested only once they change, so the store holds documents no \
             boot would ever have loaded; watcher-only: {:?}",
            watched.difference(&walked).collect::<Vec<_>>()
        );
        assert_eq!(
            scanned,
            walked,
            "the harness's vault holds files production's walk never sees, so no \
             test can reproduce what production ingests; harness-only: {:?}",
            scanned.difference(&walked).collect::<Vec<_>>()
        );
    }

    fn seed(root: &Path, relative_paths: &[&str], contents: &str) {
        for rel in relative_paths {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, contents).unwrap();
        }
    }

    /// A vault root that is a git repository — gitignore rules only bind inside
    /// one (`ignore`'s `require_git` default), so a fixture without `.git`
    /// measures the wrong walk.
    fn vault_repo() -> (TempDir, PathBuf) {
        let temp = TempDir::new().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        (temp, root)
    }

    #[tokio::test]
    async fn every_leg_agrees_on_every_dot_directory() {
        let (_temp, root) = vault_repo();
        let corpus = [
            "A.org",
            "Projects/Holon/Now.org",
            ".claude/worktrees/agent-x/Projects/Holon/Now.org",
            ".obsidian/A.org",
            ".logseq/A.org",
            ".git/A.org",
            ".jj/A.org",
        ];
        seed(&root, &corpus, "* H");

        assert_legs_agree(every_legs_verdict(&root, &corpus).await);
    }

    /// The same drift one axis over: the walk reads every `.gitignore` on the
    /// way down, the watcher read only the root's, so a file a NESTED
    /// `.gitignore` excludes was invisible at boot and ingested on every touch.
    #[tokio::test]
    async fn every_leg_agrees_on_a_nested_gitignore() {
        let (_temp, root) = vault_repo();
        seed(&root, &[".gitignore"], "vendor/\n");
        seed(&root, &["sub/.gitignore"], "scratch.org\n");
        let corpus = [
            "A.org",
            "sub/Keep.org",
            "sub/scratch.org",
            "vendor/dep.org",
            "sub/deeper/scratch.org",
        ];
        seed(&root, &corpus, "* H");

        let legs = every_legs_verdict(&root, &corpus).await;
        assert!(
            !legs[0].contains(&root.join("sub/scratch.org")),
            "fixture is wrong: the boot walk must skip the nested-gitignored file"
        );
        assert_legs_agree(legs);
    }

    /// The walk does not follow links. Everything BENEATH a symlinked directory
    /// is therefore outside the vault — including a link that points into a
    /// hidden directory, the shape an agent workspace mirror takes. A link to a
    /// plain sibling file is a different case: the walk yields it as a file, so
    /// the vault contains it.
    #[tokio::test]
    async fn every_leg_agrees_across_symlinks() {
        let (_temp, root) = vault_repo();
        seed(
            &root,
            &["A.org", ".claude/worktrees/agent-x/Mirrored.org"],
            "* H",
        );
        std::os::unix::fs::symlink(root.join(".claude/worktrees/agent-x"), root.join("mirror"))
            .unwrap();
        std::os::unix::fs::symlink(root.join("A.org"), root.join("Alias.org")).unwrap();

        let corpus = ["A.org", "Alias.org", "mirror/Mirrored.org"];
        let legs = every_legs_verdict(&root, &corpus).await;
        assert!(
            legs[0].contains(&root.join("Alias.org")),
            "fixture is wrong: the boot walk yields a symlink to a plain file"
        );
        assert!(
            !legs[0].contains(&root.join("mirror/Mirrored.org")),
            "fixture is wrong: the boot walk must not descend through a symlink"
        );
        assert_legs_agree(legs);
    }

    #[test]
    fn test_file_watcher_respects_gitignore() {
        for_every_delay(|delay| async move {
            let (_temp, root) = vault_repo();
            std::fs::write(root.join(".gitignore"), "vendor/\n").unwrap();
            std::fs::create_dir_all(root.join("vendor")).unwrap();
            let (_source, mut watcher) = armed_watcher(&root, delay).await;

            assert_never_forwarded(
                &mut watcher,
                &root,
                &root.join("vendor/dep.org"),
                "Should NOT receive events from gitignored paths",
            )
            .await;
        });
    }
}
