//! `FileChangeSource` port (ADR 0011): "how I learn a file changed".
//!
//! Sits *below* the org-side `OrgFileWatcher`: this port emits raw change
//! events; content-hash gating / extension / gitignore filtering stay with the
//! consumer so prod and tests share the same dedupe and echo-suppression path.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use notify::RecommendedWatcher;
use notify::RecursiveMode;
use notify::Watcher;
use notify::event::ModifyKind;
use notify::event::RenameMode;
use tokio::sync::broadcast;

/// What kind of change a `FileChange` reports.
///
/// `Rename` carries the MOST information: its `FileChange::path` is the NEW
/// (destination) path and `from` is the OLD (source) path — both present in ONE
/// event, so the consumer can re-home a document atomically WITHOUT a
/// delete-then-create window (the window that lets a `mv A.org B.org` be
/// mis-read as "delete A" and cascade-delete the very doc the move re-homed).
/// An unpaired half-rename is unrepresentable at this level: the watcher pairs
/// the two sides BEFORE emitting `Rename`, and when it cannot pair them (a move
/// in/out of the watched root, or a fs backend that never associates the sides)
/// it DISCLOSES and falls back to `Remove` + `Create`.
///
/// Not `Copy`: `Rename` owns a `PathBuf`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileChangeKind {
    Modify,
    Create,
    Remove,
    /// Atomic rename. `FileChange::path` is the destination (new) path; `from`
    /// is the source (old) path.
    Rename { from: PathBuf },
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

/// How long a buffered rename-`From` side waits for its `To` partner before the
/// pairing is abandoned and the `From` is disclosed as a plain `Remove`. macOS
/// FSEvents delivers both sides of a `mv` in the same callback batch (adjacent
/// events), so the partner — when there is one — arrives well within this
/// window; a lone side (move in/out of the watched root) is flushed by the next
/// event or repaired by the controller's poll backstops.
const RENAME_PAIR_WINDOW: Duration = Duration::from_millis(500);

/// Pairs the two sides of a rename that a backend reports separately.
///
/// FSEvents (macOS) explicitly "provides no mechanism to associate the old and
/// new sides of a rename event" — notify emits `Modify(Name(Any))` for EACH
/// side with no tracker. inotify (Linux) emits `From` then `To`. This state
/// machine handles both: it buffers a `From`-side (an `Any` whose path no
/// longer exists, or an explicit `From`) and, when the matching `To`-side
/// arrives within [`RENAME_PAIR_WINDOW`], emits a single atomic
/// `Rename { from }`. `Both` (source+target in one event) needs no buffering.
///
/// The buffer holds AT MOST one pending `From`. Any non-pairing event flushes a
/// stale pending as a disclosed `Remove`, so an event is never held longer than
/// until the next event — and any side that is nonetheless swallowed is
/// repaired by `FileSyncController::poll_tracked_files` / `poll_new_files`.
#[derive(Default)]
struct RenamePairing {
    pending_from: Option<(PathBuf, Instant)>,
}

impl RenamePairing {
    /// Emit a stale pending `From` (its `To` never came) as a plain Remove.
    fn flush_pending(
        pending: &mut Option<(PathBuf, Instant)>,
        out: &mut Vec<(PathBuf, FileChangeKind)>,
    ) {
        if let Some((from, _)) = pending.take() {
            tracing::warn!(
                from = %from.display(),
                "[NotifyWatcher] rename pairing abandoned — no matching `To` side arrived;                  disclosing the orphaned `From` as a Remove (the controller's poll backstop                  reconciles a genuine move-out)."
            );
            out.push((from, FileChangeKind::Remove));
        }
    }

    /// Take the pending `From` iff it is still within the pairing window.
    fn take_fresh_from(pending: &mut Option<(PathBuf, Instant)>) -> Option<PathBuf> {
        match pending.take() {
            Some((from, at)) if at.elapsed() < RENAME_PAIR_WINDOW => Some(from),
            _ => None,
        }
    }

    fn pending_is_stale(pending: &Option<(PathBuf, Instant)>) -> bool {
        pending
            .as_ref()
            .map(|(_, at)| at.elapsed() >= RENAME_PAIR_WINDOW)
            .unwrap_or(false)
    }

    /// Classify one raw notify event into zero or more `(path, kind)` emissions,
    /// updating the pairing buffer. `exists` decides an ambiguous `Any` side:
    /// the source path is gone after a `mv`, the target path is present.
    fn classify(
        &mut self,
        event: &notify::Event,
        exists: &dyn Fn(&Path) -> bool,
    ) -> Vec<(PathBuf, FileChangeKind)> {
        use notify::EventKind as EK;

        let mut out: Vec<(PathBuf, FileChangeKind)> = Vec::new();
        let pending = &mut self.pending_from;

        match &event.kind {
            EK::Modify(ModifyKind::Name(RenameMode::Both)) => {
                // Guaranteed pairing: paths are (from, to) in this exact order.
                Self::flush_pending(pending, &mut out);
                if event.paths.len() == 2 {
                    let from = event.paths[0].clone();
                    let to = event.paths[1].clone();
                    out.push((to, FileChangeKind::Rename { from }));
                } else {
                    tracing::warn!(
                        paths = ?event.paths,
                        "[NotifyWatcher] RenameMode::Both without exactly two paths — falling                          back to per-path Modify."
                    );
                    for p in &event.paths {
                        out.push((p.clone(), FileChangeKind::Modify));
                    }
                }
            }
            EK::Modify(ModifyKind::Name(RenameMode::From)) => {
                Self::flush_pending(pending, &mut out);
                if let Some(p) = event.paths.first() {
                    *pending = Some((p.clone(), Instant::now()));
                }
            }
            EK::Modify(ModifyKind::Name(RenameMode::To)) => {
                if Self::pending_is_stale(pending) {
                    Self::flush_pending(pending, &mut out);
                }
                if let Some(to) = event.paths.first() {
                    match Self::take_fresh_from(pending) {
                        Some(from) => out.push((to.clone(), FileChangeKind::Rename { from })),
                        None => out.push((to.clone(), FileChangeKind::Create)),
                    }
                }
            }
            EK::Modify(ModifyKind::Name(RenameMode::Any)) => {
                // macOS: one `Any` per side, no association. Disambiguate by
                // existence — the source is gone, the target is present.
                if let Some(p) = event.paths.first() {
                    if exists(p) {
                        if Self::pending_is_stale(pending) {
                            Self::flush_pending(pending, &mut out);
                        }
                        match Self::take_fresh_from(pending) {
                            Some(from) => {
                                out.push((p.clone(), FileChangeKind::Rename { from }))
                            }
                            // No partner yet — it is on disk, so a re-ingest
                            // (Modify) is the safe, idempotent classification;
                            // `on_file_changed` creates-or-updates from bytes.
                            None => out.push((p.clone(), FileChangeKind::Modify)),
                        }
                    } else {
                        // Source side. Buffer it, superseding any older pending.
                        Self::flush_pending(pending, &mut out);
                        *pending = Some((p.clone(), Instant::now()));
                    }
                }
            }
            EK::Modify(ModifyKind::Name(RenameMode::Other)) | EK::Modify(_) => {
                Self::flush_pending(pending, &mut out);
                for p in &event.paths {
                    out.push((p.clone(), FileChangeKind::Modify));
                }
            }
            EK::Create(_) => {
                Self::flush_pending(pending, &mut out);
                for p in &event.paths {
                    out.push((p.clone(), FileChangeKind::Create));
                }
            }
            EK::Remove(_) => {
                Self::flush_pending(pending, &mut out);
                for p in &event.paths {
                    out.push((p.clone(), FileChangeKind::Remove));
                }
            }
            _ => {
                Self::flush_pending(pending, &mut out);
            }
        }
        out
    }
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
        let pairing = Mutex::new(RenamePairing::default());
        let watcher = notify::recommended_watcher(
            move |res: Result<notify::Event, notify::Error>| match res {
                Ok(event) => {
                    let emissions = pairing
                        .lock()
                        .expect("NotifyWatcher rename-pairing mutex poisoned")
                        .classify(&event, &|p: &Path| p.exists());
                    for (path, kind) in emissions {
                        let seq =
                            NOTIFY_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
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
    use notify::EventKind;
    use notify::event::ModifyKind;
    use notify::event::RenameMode;

    use super::*;

    fn ev(kind: EventKind, paths: &[&str]) -> notify::Event {
        let mut e = notify::Event::new(kind);
        for p in paths {
            e = e.add_path(PathBuf::from(p));
        }
        e
    }

    #[test]
    fn both_side_pairs_into_one_rename() {
        let mut pairing = RenamePairing::default();
        let out = pairing.classify(
            &ev(
                EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
                &["/vault/a.org", "/vault/b.org"],
            ),
            &|_| false,
        );
        assert_eq!(
            out,
            vec![(
                PathBuf::from("/vault/b.org"),
                FileChangeKind::Rename {
                    from: PathBuf::from("/vault/a.org")
                }
            )]
        );
    }

    #[test]
    fn from_then_to_pairs_into_one_rename() {
        let mut pairing = RenamePairing::default();
        let out1 = pairing.classify(
            &ev(
                EventKind::Modify(ModifyKind::Name(RenameMode::From)),
                &["/vault/a.org"],
            ),
            &|_| false,
        );
        assert!(out1.is_empty(), "From side is buffered, emits nothing yet");
        let out2 = pairing.classify(
            &ev(
                EventKind::Modify(ModifyKind::Name(RenameMode::To)),
                &["/vault/b.org"],
            ),
            &|_| true,
        );
        assert_eq!(
            out2,
            vec![(
                PathBuf::from("/vault/b.org"),
                FileChangeKind::Rename {
                    from: PathBuf::from("/vault/a.org")
                }
            )]
        );
    }

    #[test]
    fn macos_any_pair_by_existence() {
        // FSEvents: gone-side then present-side, both `Any`.
        let mut pairing = RenamePairing::default();
        let gone = "/vault/a.org";
        let present = "/vault/b.org";
        let out1 = pairing.classify(
            &ev(EventKind::Modify(ModifyKind::Name(RenameMode::Any)), &[gone]),
            &|p| p != Path::new(gone),
        );
        assert!(out1.is_empty(), "source (gone) side is buffered");
        let out2 = pairing.classify(
            &ev(
                EventKind::Modify(ModifyKind::Name(RenameMode::Any)),
                &[present],
            ),
            &|p| p == Path::new(present),
        );
        assert_eq!(
            out2,
            vec![(
                PathBuf::from(present),
                FileChangeKind::Rename {
                    from: PathBuf::from(gone)
                }
            )]
        );
    }

    #[test]
    fn lone_from_side_flushes_as_remove_on_next_event() {
        // A move OUT of the watched root: source `Any` (gone), no partner.
        let mut pairing = RenamePairing::default();
        let gone = "/vault/a.org";
        pairing.classify(
            &ev(EventKind::Modify(ModifyKind::Name(RenameMode::Any)), &[gone]),
            &|_| false,
        );
        // An unrelated create supersedes it → the orphan From is disclosed.
        let out = pairing.classify(
            &ev(EventKind::Create(notify::event::CreateKind::File), &["/vault/c.org"]),
            &|_| true,
        );
        assert!(out.contains(&(PathBuf::from(gone), FileChangeKind::Remove)));
        assert!(out.contains(&(PathBuf::from("/vault/c.org"), FileChangeKind::Create)));
    }

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
