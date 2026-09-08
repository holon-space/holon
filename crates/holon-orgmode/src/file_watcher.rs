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
    use std::sync::Arc;

    use holon_filesystem::NotifyWatcher;
    use tempfile::TempDir;
    use tokio::time::Duration;
    use tokio::time::sleep;

    use super::*;

    /// These tests drive the REAL `NotifyWatcher` (fsevents on macOS) end to
    /// end through the vault filter bridge — the dedicated real-watcher
    /// coverage ADR 0011 requires once the PBT harness runs on the in-memory
    /// adapter.
    fn armed_watcher(dir: &Path) -> VaultFileWatcher {
        let source = Arc::new(NotifyWatcher::new_unarmed().unwrap());
        let watcher = VaultFileWatcher::new(
            source.as_ref(),
            dir,
            crate::file_sync_controller::org_only_format_registry(),
        );
        source.arm(dir).unwrap();
        // Leak the source so the notify watcher outlives this helper —
        // dropping it stops event delivery. Test-scoped only.
        std::mem::forget(source);
        watcher
    }

    #[tokio::test]
    async fn test_file_watcher_detects_changes() {
        let temp_dir = TempDir::new().unwrap();
        let test_file = temp_dir.path().join("test.org");

        let watcher = armed_watcher(temp_dir.path());

        sleep(Duration::from_millis(100)).await;

        tokio::fs::write(&test_file, "* Test").await.unwrap();
        sleep(Duration::from_millis(500)).await;

        let mut receiver = watcher.into_receiver();
        let mut saw_org_change = false;
        while let Ok((msg, _seq)) = receiver.try_recv() {
            saw_org_change |= msg.is_some();
        }
        assert!(saw_org_change, "Should receive file change event");
    }

    #[tokio::test]
    async fn test_file_watcher_ignores_git_dir() {
        let temp_dir = TempDir::new().unwrap();
        let git_dir = temp_dir.path().join(".git");
        std::fs::create_dir_all(&git_dir).unwrap();
        let git_file = git_dir.join("test.org");

        let mut watcher = armed_watcher(temp_dir.path());
        sleep(Duration::from_millis(100)).await;

        tokio::fs::write(&git_file, "* Hidden").await.unwrap();
        sleep(Duration::from_millis(500)).await;

        while let Ok((msg, _seq)) = watcher.receiver().try_recv() {
            assert!(
                msg.is_none(),
                "Should NOT receive events from .git/: {msg:?}"
            );
        }
    }

    /// The live vault keeps agent jj-workspaces under `.claude/worktrees/`,
    /// each a full copy of the vault whose `Projects/Holon/Now.org` declares
    /// the same `#+ID:` and headline slugs as the live file. Those workspaces
    /// are written constantly, so every touch used to deliver an event the
    /// watcher forwarded into ingest.
    #[tokio::test]
    async fn a_hidden_nested_vault_copy_is_never_ingested() {
        let temp_dir = TempDir::new().unwrap();
        let copy = temp_dir
            .path()
            .join(".claude/worktrees/agent-x/Projects/Holon");
        std::fs::create_dir_all(&copy).unwrap();

        let mut watcher = armed_watcher(temp_dir.path());
        sleep(Duration::from_millis(100)).await;

        tokio::fs::write(copy.join("Now.org"), "* Stale copy")
            .await
            .unwrap();
        sleep(Duration::from_millis(500)).await;

        while let Ok((msg, _seq)) = watcher.receiver().try_recv() {
            assert!(
                msg.is_none(),
                "a stale vault copy under .claude/worktrees/ reached ingest: {msg:?}"
            );
        }
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

    #[tokio::test]
    async fn test_file_watcher_respects_gitignore() {
        let (_temp, root) = vault_repo();
        let temp_dir = root;

        // Create .gitignore that ignores "vendor/" directory
        tokio::fs::write(temp_dir.join(".gitignore"), "vendor/\n")
            .await
            .unwrap();

        let vendor_dir = temp_dir.join("vendor");
        std::fs::create_dir_all(&vendor_dir).unwrap();

        let mut watcher = armed_watcher(&temp_dir);
        sleep(Duration::from_millis(100)).await;

        // Write to ignored path
        tokio::fs::write(vendor_dir.join("dep.org"), "* Vendor dep")
            .await
            .unwrap();
        sleep(Duration::from_millis(500)).await;

        while let Ok((msg, _seq)) = watcher.receiver().try_recv() {
            assert!(
                msg.is_none(),
                "Should NOT receive events from gitignored paths: {msg:?}"
            );
        }
    }
}
