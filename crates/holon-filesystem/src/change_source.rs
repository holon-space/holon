//! `FileChangeSource` port (ADR 0011): "how I learn a file changed".
//!
//! Sits *below* the org-side `OrgFileWatcher`: this port emits raw change
//! events; content-hash gating / extension / gitignore filtering stay with the
//! consumer so prod and tests share the same dedupe and echo-suppression path.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::broadcast;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChangeKind {
    Modify,
    Create,
    Remove,
}

#[derive(Debug, Clone)]
pub struct FileChange {
    pub path: PathBuf,
    pub kind: FileChangeKind,
    /// Monotonic per-source sequence number. Consumers can record the highest
    /// fully-processed seq as a watermark; a producer that knows the seq of
    /// its own write (`InMemoryFileSystem::last_change_seq`) can then await
    /// "my change has been processed" deterministically.
    pub seq: u64,
}

/// The file-change notification seam.
///
/// `arm(root)` starts delivery of events under `root`. It may block for
/// seconds (`notify::watch(_, Recursive)` takes 9+ s on macOS for populated
/// trees) — call it from `spawn_blocking` after signalling readiness.
/// Subscribers created before `arm` receive all events delivered after it.
pub trait FileChangeSource: Send + Sync {
    fn subscribe(&self) -> broadcast::Receiver<FileChange>;
    fn arm(&self, root: &Path) -> std::io::Result<()>;
}

/// Production adapter: wraps `notify::RecommendedWatcher` (fsevents on macOS).
pub struct NotifyWatcher {
    watcher: Mutex<RecommendedWatcher>,
    tx: broadcast::Sender<FileChange>,
}

static NOTIFY_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl NotifyWatcher {
    /// Build the watcher (channels + callback) without registering the
    /// recursive watch — see [`FileChangeSource::arm`].
    pub fn new_unarmed() -> std::io::Result<Self> {
        let (tx, _) = broadcast::channel(4096);
        let event_tx = tx.clone();
        let watcher = notify::recommended_watcher(
            move |res: Result<notify::Event, notify::Error>| match res {
                Ok(event) => {
                    let kind = match event.kind {
                        notify::EventKind::Modify(_) => FileChangeKind::Modify,
                        notify::EventKind::Create(_) => FileChangeKind::Create,
                        notify::EventKind::Remove(_) => FileChangeKind::Remove,
                        _ => return,
                    };
                    for path in event.paths {
                        let seq = NOTIFY_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                        // send only errors when there are no subscribers — fine.
                        let _ = event_tx.send(FileChange { path, kind, seq });
                    }
                }
                Err(e) => {
                    tracing::error!("[NotifyWatcher] watcher error: {e}");
                }
            },
        )
        .map_err(std::io::Error::other)?;
        Ok(Self {
            watcher: Mutex::new(watcher),
            tx,
        })
    }
}

impl FileChangeSource for NotifyWatcher {
    fn subscribe(&self) -> broadcast::Receiver<FileChange> {
        self.tx.subscribe()
    }

    fn arm(&self, root: &Path) -> std::io::Result<()> {
        self.watcher
            .lock()
            .expect("NotifyWatcher mutex poisoned")
            .watch(root, RecursiveMode::Recursive)
            .map_err(std::io::Error::other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn notify_watcher_delivers_events_after_arm() {
        let dir = tempfile::tempdir().unwrap();
        let watcher = NotifyWatcher::new_unarmed().unwrap();
        let mut rx = watcher.subscribe();
        watcher.arm(dir.path()).unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        tokio::fs::write(dir.path().join("a.org"), "* x")
            .await
            .unwrap();

        let change = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for fs event")
            .expect("channel closed");
        assert!(change.path.ends_with("a.org"));
    }
}
