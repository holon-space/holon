//! File watcher for Org files
//!
//! Bridges the injected [`FileChangeSource`] port (ADR 0011) to the org sync
//! loop: subscribes to raw file-change events, filters to `.org` files that
//! are not gitignored (including always skipping `.git/` and `.jj/`), and
//! forwards the paths on an unbounded mpsc channel.
//!
//! Echo suppression lives in `FileSyncController::last_projection`, not here.

use std::path::Path;
use std::path::PathBuf;

use holon_core::CanonicalPath;
use holon_filesystem::FileChange;
use holon_filesystem::FileChangeKind;
use holon_filesystem::FileChangeSource;
use holon_filesystem::FileSystem;
pub use holon_filesystem::ScannedEntries;
use ignore::gitignore::Gitignore;
use tokio::sync::mpsc;
use tracing::debug;
use tracing::warn;

/// Scan a directory for `.org` files through the `FileSystem` port (ADR 0011).
///
/// The gitignore-aware recursive walk lives in the port
/// (`holon_filesystem::fs_port::walk_directory` for the real adapter); this
/// wrapper applies the org-format extension filter.
pub async fn scan_directory(fs: &dyn FileSystem, root: &Path) -> std::io::Result<ScannedEntries> {
    let mut scanned = fs.scan_directory(root).await?;
    scanned
        .files
        .retain(|p| p.extension().is_some_and(|e| e == "org"));
    Ok(scanned)
}

#[tracing::instrument(name = "build_gitignore", fields(root = %root.display()))]
fn build_gitignore(root: &Path) -> Gitignore {
    let (gitignore, errors) = Gitignore::new(root.join(".gitignore"));
    if let Some(err) = errors {
        warn!("Error parsing .gitignore: {}", err);
    }
    gitignore
}

fn is_ignored(path: &Path, gitignore: &Gitignore) -> bool {
    // Always skip VCS internals
    for component in path.components() {
        let s = component.as_os_str().to_str().unwrap_or("");
        if s == ".git" || s == ".jj" {
            return true;
        }
    }
    let is_dir = path.is_dir();
    // `matched` alone never consults parent dirs, so a `vendor/` pattern
    // would not ignore `vendor/dep.org`.
    gitignore
        .matched_path_or_any_parents(path, is_dir)
        .is_ignore()
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

/// Whether `path` is an org file the sync loop should track (`.org` extension,
/// not gitignored / VCS-internal).
fn is_org_relevant(path: &Path, gitignore: &Gitignore) -> bool {
    path.extension().map(|e| e == "org").unwrap_or(false) && !is_ignored(path, gitignore)
}

/// Map one raw [`FileChange`] to the org-relevant [`FileEvent`] the sync loop
/// acts on, or `None` when it is filtered (non-`.org`, gitignored). This is the
/// single source of truth for the bridge's kind→event routing — exposed so a
/// test can drive SYNTHETIC notify-shaped changes through the SAME routing the
/// production bridge uses (the ENVIRONMENT-parity rung for the pairing fallback,
/// see docs/Testing/BugFunnel.md 2026-07-27).
///
/// `is_relevant` decides whether a path is one the org side tracks; the bridge
/// passes a gitignore-aware predicate, a focused test may pass an extension
/// check.
pub fn classify_change_to_event(
    change: FileChange,
    is_relevant: &dyn Fn(&Path) -> bool,
) -> Option<FileEvent> {
    match change.kind {
        FileChangeKind::Rename { from } => {
            let to = change.path;
            if is_relevant(&to) {
                debug!("File rename detected: {} -> {}", from.display(), to.display());
                Some(FileEvent::Renamed { from, to })
            } else if is_relevant(&from) {
                // Renamed OUT of org-space (`.org` -> `.txt`): the org side sees
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
/// (non-`.org`, gitignored). Filtered events still flow through the SAME channel
/// so the consumer can advance its processed-seq watermark strictly in delivery
/// order — advancing for a filtered event from the bridge directly could
/// overtake an unprocessed earlier forwarded event.
pub struct OrgFileWatcher {
    change_rx: mpsc::UnboundedReceiver<(Option<FileEvent>, u64)>,
}

impl OrgFileWatcher {
    /// Subscribe to `source` and spawn the filter bridge. Subscribing happens
    /// here — before the caller arms the source — so no event is missed.
    ///
    /// The gitignore root is canonicalized so it matches the canonical paths
    /// fs event backends report (macOS: `/var` → `/private/var`).
    pub fn new(source: &dyn FileChangeSource, watch_dir: &Path) -> Self {
        let gitignore = tracing::info_span!("OrgFileWatcher.build_gitignore")
            .in_scope(|| build_gitignore(&CanonicalPath::new(watch_dir).into_path_buf()));
        let (change_tx, change_rx) = mpsc::unbounded_channel();
        let mut source_rx = source.subscribe();

        tokio::spawn(async move {
            loop {
                match source_rx.recv().await {
                    Ok(change) => {
                        let seq = change.seq;
                        let msg = classify_change_to_event(change, &|p| {
                            is_org_relevant(p, &gitignore)
                        });
                        if change_tx.send((msg, seq)).is_err() {
                            // Receiver dropped — sync loop is gone.
                            return;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        // Dropped raw events are repaired by the controller's
                        // poll backstops (poll_tracked_files / poll_new_files).
                        warn!("[OrgFileWatcher] lagged behind change source by {n} events");
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
    use std::sync::Arc;

    use holon_filesystem::NotifyWatcher;
    use tempfile::TempDir;
    use tokio::time::Duration;
    use tokio::time::sleep;

    use super::*;

    /// These tests drive the REAL `NotifyWatcher` (fsevents on macOS) end to
    /// end through the org filter bridge — the dedicated real-watcher coverage
    /// ADR 0011 requires once the PBT harness runs on the in-memory adapter.
    fn armed_watcher(dir: &Path) -> OrgFileWatcher {
        let source = Arc::new(NotifyWatcher::new_unarmed().unwrap());
        let watcher = OrgFileWatcher::new(source.as_ref(), dir);
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

    #[tokio::test]
    async fn test_file_watcher_respects_gitignore() {
        let temp_dir = TempDir::new().unwrap();

        // Create .gitignore that ignores "vendor/" directory
        tokio::fs::write(temp_dir.path().join(".gitignore"), "vendor/\n")
            .await
            .unwrap();

        let vendor_dir = temp_dir.path().join("vendor");
        std::fs::create_dir_all(&vendor_dir).unwrap();

        let mut watcher = armed_watcher(temp_dir.path());
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
