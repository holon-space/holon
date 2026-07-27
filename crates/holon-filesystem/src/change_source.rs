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
/// A backend-agnostic raw filesystem signal — ONE per path. `notify::Event`s
/// map to a sequence of these ([`notify_event_to_signals`]); the pairing state
/// machine ([`RenamePairing::classify`]) consumes them. Decoupling from `notify`
/// lets the pairing be exercised deterministically (no fsevents) AND across
/// crates (the org-side sync tests drive the fallback through the controller).
///
/// macOS `Any` (which side is unknown) is resolved to `RenameFrom`/`RenameTo`
/// at mapping time via on-disk existence, so `classify` never needs the fs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawFsSignal {
    Create(PathBuf),
    Modify(PathBuf),
    Remove(PathBuf),
    /// Rename source side (the path is now gone).
    RenameFrom(PathBuf),
    /// Rename target side (the path is present).
    RenameTo(PathBuf),
    /// Both sides in one event, in `(from, to)` order.
    RenameBoth { from: PathBuf, to: PathBuf },
}

/// Map one raw `notify::Event` into zero or more [`RawFsSignal`]s, resolving a
/// macOS ambiguous `Any` side by on-disk existence (`exists`): the source is
/// gone after a `mv`, the target is present.
fn notify_event_to_signals(
    event: &notify::Event,
    exists: &dyn Fn(&Path) -> bool,
) -> Vec<RawFsSignal> {
    use notify::EventKind as EK;
    match &event.kind {
        EK::Modify(ModifyKind::Name(RenameMode::Both)) => {
            if event.paths.len() == 2 {
                vec![RawFsSignal::RenameBoth {
                    from: event.paths[0].clone(),
                    to: event.paths[1].clone(),
                }]
            } else {
                tracing::warn!(
                    paths = ?event.paths,
                    "[NotifyWatcher] RenameMode::Both without exactly two paths — treating each                      as a plain Modify."
                );
                event.paths.iter().cloned().map(RawFsSignal::Modify).collect()
            }
        }
        EK::Modify(ModifyKind::Name(RenameMode::From)) => {
            event.paths.iter().cloned().map(RawFsSignal::RenameFrom).collect()
        }
        EK::Modify(ModifyKind::Name(RenameMode::To)) => {
            event.paths.iter().cloned().map(RawFsSignal::RenameTo).collect()
        }
        EK::Modify(ModifyKind::Name(RenameMode::Any)) => event
            .paths
            .iter()
            .cloned()
            .map(|p| {
                if exists(&p) {
                    RawFsSignal::RenameTo(p)
                } else {
                    RawFsSignal::RenameFrom(p)
                }
            })
            .collect(),
        EK::Modify(_) => event.paths.iter().cloned().map(RawFsSignal::Modify).collect(),
        EK::Create(_) => event.paths.iter().cloned().map(RawFsSignal::Create).collect(),
        EK::Remove(_) => event.paths.iter().cloned().map(RawFsSignal::Remove).collect(),
        _ => Vec::new(),
    }
}

/// Pairs the two sides of a rename that a backend reports separately.
///
/// FSEvents (macOS) explicitly "provides no mechanism to associate the old and
/// new sides of a rename event" — `notify` emits `Modify(Name(Any))` for EACH
/// side with no tracker; inotify (Linux) emits `From` then `To`. This state
/// machine handles both: it buffers a `From` side and, when the matching `To`
/// side arrives, emits a single atomic `Rename { from }`. `Both` needs no
/// buffering.
///
/// ## Refutation fix (2026-07-27): a pending `From` is DURABLE
///
/// The pairing runs UPSTREAM of org-relevance filtering (that lives in the
/// `OrgFileWatcher` bridge), so ANY interposing event — an editor lock file, a
/// Dropbox/iCloud/Syncthing daemon write — can land between the two rename
/// halves. Such an interposer must NEVER flush the pending `From`: a flushed
/// `Remove` routes to `on_file_deleted`, whose title-based D3 guard cannot fire
/// before the title has followed, so a LIVE document gets cascade-deleted.
/// Therefore:
///   * **Relevance-gated** — only `.org`-relevant signals touch the buffer; an
///     irrelevant signal passes through WITHOUT disturbing a pending `From`.
///   * **Timeout-only flush** — a pending `From` is flushed as a disclosed
///     `Remove` ONLY once it is older than [`RENAME_PAIR_WINDOW`] (a genuine
///     move-out whose `To` never came). A relevant event interposing WITHIN the
///     window leaves the pending intact — two unrelated org events can
///     legitimately interleave with a rename pair on fsevents.
///   * **Pair at any age** — a `To` always pairs with a pending `From`
///     regardless of age; the window only bounds the *unpaired* case.
/// The flushed `Remove` is additionally safety-netted at the controller
/// (`on_file_deleted` id-based reunification), and any swallowed side is
/// repaired by `poll_tracked_files` / `poll_new_files`.
#[derive(Debug, Default)]
pub struct RenamePairing {
    pending_from: Option<(PathBuf, Instant)>,
}

impl RenamePairing {
    pub fn new() -> Self {
        Self::default()
    }

    /// Emit a pending `From` as a disclosed `Remove` (its `To` never came).
    fn flush_pending(
        pending: &mut Option<(PathBuf, Instant)>,
        out: &mut Vec<(PathBuf, FileChangeKind)>,
    ) {
        if let Some((from, _)) = pending.take() {
            tracing::warn!(
                from = %from.display(),
                "[NotifyWatcher] rename pairing timed out — no matching `To` side arrived within                  the window; disclosing the orphaned `From` as a Remove (the controller's                  id-based reunification + poll backstop reconcile a genuine move-out)."
            );
            out.push((from, FileChangeKind::Remove));
        }
    }

    /// Flush the pending `From` ONLY if it has aged past the pairing window.
    /// Called from plain (non-rename) relevant signals — never from irrelevant
    /// ones, and never for a still-fresh pending.
    fn maybe_timeout_flush(
        pending: &mut Option<(PathBuf, Instant)>,
        now: Instant,
        out: &mut Vec<(PathBuf, FileChangeKind)>,
    ) {
        let stale = pending
            .as_ref()
            .map(|(_, at)| now.duration_since(*at) >= RENAME_PAIR_WINDOW)
            .unwrap_or(false);
        if stale {
            Self::flush_pending(pending, out);
        }
    }

    /// Classify one [`RawFsSignal`] into zero or more `(path, kind)` emissions,
    /// updating the pairing buffer. `now` is the signal's arrival instant
    /// (injected so the timeout is deterministically testable); `is_relevant`
    /// decides whether a path is one the org side tracks (`.org`, not ignored) —
    /// irrelevant signals never disturb a pending rename `From`.
    pub fn classify(
        &mut self,
        signal: &RawFsSignal,
        now: Instant,
        is_relevant: &dyn Fn(&Path) -> bool,
    ) -> Vec<(PathBuf, FileChangeKind)> {
        let mut out: Vec<(PathBuf, FileChangeKind)> = Vec::new();
        let pending = &mut self.pending_from;
        match signal {
            RawFsSignal::RenameBoth { from, to } => {
                // Self-contained pair; a concurrent pending is unrelated — leave
                // it intact (it will pair with its own `To` or time out).
                out.push((to.clone(), FileChangeKind::Rename { from: from.clone() }));
            }
            RawFsSignal::RenameFrom(p) => {
                if is_relevant(p) {
                    // A new source side. Supersede any existing pending, flushing
                    // it as a Remove (a distinct file that moved and never paired
                    // — safety-netted at the controller). Rare: one `mv` yields
                    // exactly one source side.
                    Self::flush_pending(pending, &mut out);
                    *pending = Some((p.clone(), now));
                }
                // Irrelevant source side: ignore, do NOT touch pending.
            }
            RawFsSignal::RenameTo(p) => {
                if is_relevant(p) {
                    match pending.take() {
                        // Pair at ANY age — the window bounds only the unpaired
                        // case, never a real pair whose `To` arrives late.
                        Some((from, _)) => {
                            out.push((p.clone(), FileChangeKind::Rename { from }))
                        }
                        None => out.push((p.clone(), FileChangeKind::Create)),
                    }
                }
                // Irrelevant target side: ignore, do NOT touch pending.
            }
            RawFsSignal::Create(p) => {
                if is_relevant(p) {
                    Self::maybe_timeout_flush(pending, now, &mut out);
                }
                out.push((p.clone(), FileChangeKind::Create));
            }
            RawFsSignal::Modify(p) => {
                if is_relevant(p) {
                    Self::maybe_timeout_flush(pending, now, &mut out);
                }
                out.push((p.clone(), FileChangeKind::Modify));
            }
            RawFsSignal::Remove(p) => {
                if is_relevant(p) {
                    Self::maybe_timeout_flush(pending, now, &mut out);
                }
                out.push((p.clone(), FileChangeKind::Remove));
            }
        }
        out
    }
}

/// Cheap org-relevance predicate for the pairing buffer: a `.org` extension.
/// The full gitignore / VCS-internal filter lives downstream in the
/// `OrgFileWatcher` bridge; here we only need to keep NON-`.org` interposers
/// (editor lock files, byte-syncer temp files) from disturbing a pending
/// rename `From`.
fn is_org_ext(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "org")
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
                    let signals = notify_event_to_signals(&event, &|p: &Path| p.exists());
                    let now = Instant::now();
                    let mut pairing = pairing
                        .lock()
                        .expect("NotifyWatcher rename-pairing mutex poisoned");
                    for signal in &signals {
                        // Cheap `.org`-extension relevance here; the full
                        // gitignore/VCS filter stays in the `OrgFileWatcher`
                        // bridge. This is enough to keep a non-`.org` interposer
                        // (lock file, byte-syncer temp) from disturbing a pending
                        // rename `From`.
                        let emissions = pairing.classify(signal, now, &is_org_ext);
                        for (path, kind) in emissions {
                            let seq = NOTIFY_SEQ
                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                                + 1;
                            // send only errors with no subscribers — fine.
                            let _ = event_tx.send(FileChange { path, kind, seq });
                        }
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
    use std::time::Duration;

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

    fn rel(p: &Path) -> bool {
        p.extension().is_some_and(|e| e == "org")
    }

    #[test]
    fn both_side_pairs_into_one_rename() {
        let mut pairing = RenamePairing::new();
        let out = pairing.classify(
            &RawFsSignal::RenameBoth {
                from: PathBuf::from("/vault/a.org"),
                to: PathBuf::from("/vault/b.org"),
            },
            Instant::now(),
            &rel,
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
        let mut pairing = RenamePairing::new();
        let now = Instant::now();
        let out1 = pairing.classify(&RawFsSignal::RenameFrom("/vault/a.org".into()), now, &rel);
        assert!(out1.is_empty(), "From side is buffered, emits nothing yet");
        let out2 = pairing.classify(&RawFsSignal::RenameTo("/vault/b.org".into()), now, &rel);
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
    fn notify_any_maps_by_existence_then_pairs() {
        // FSEvents: gone-side then present-side, both `Any`. The mapping resolves
        // each `Any` to From/To via existence; the pairing then unifies them.
        let gone = "/vault/a.org";
        let present = "/vault/b.org";
        let s0 = notify_event_to_signals(
            &ev(EventKind::Modify(ModifyKind::Name(RenameMode::Any)), &[gone]),
            &|p| p != Path::new(gone),
        );
        assert_eq!(s0, vec![RawFsSignal::RenameFrom(PathBuf::from(gone))]);
        let s1 = notify_event_to_signals(
            &ev(
                EventKind::Modify(ModifyKind::Name(RenameMode::Any)),
                &[present],
            ),
            &|p| p == Path::new(present),
        );
        assert_eq!(s1, vec![RawFsSignal::RenameTo(PathBuf::from(present))]);

        let mut pairing = RenamePairing::new();
        let now = Instant::now();
        assert!(pairing.classify(&s0[0], now, &rel).is_empty());
        assert_eq!(
            pairing.classify(&s1[0], now, &rel),
            vec![(
                PathBuf::from(present),
                FileChangeKind::Rename {
                    from: PathBuf::from(gone)
                }
            )]
        );
    }

    #[test]
    fn irrelevant_interposer_must_not_flush_pending() {
        // REFUTATION RED->GREEN (verifier 2026-07-27): a byte-syncer / lock-file
        // write between the two rename halves must NOT flush the pending `From`
        // as a Remove — a flushed Remove cascade-deletes a live doc.
        let mut pairing = RenamePairing::new();
        let now = Instant::now();
        let gone = "/vault/a.org";
        let moved = "/vault/b.org";
        let tmp = "/vault/.syncthing.tmp";
        assert!(
            pairing
                .classify(&RawFsSignal::RenameFrom(gone.into()), now, &rel)
                .is_empty()
        );
        let interposed = pairing.classify(&RawFsSignal::Create(tmp.into()), now, &rel);
        assert!(
            !interposed
                .iter()
                .any(|(p, k)| p == Path::new(gone) && *k == FileChangeKind::Remove),
            "an interposing (byte-syncer) event must NOT flush the pending rename `From`"
        );
        assert_eq!(
            pairing.classify(&RawFsSignal::RenameTo(moved.into()), now, &rel),
            vec![(
                PathBuf::from(moved),
                FileChangeKind::Rename {
                    from: PathBuf::from(gone)
                }
            )],
            "the pair must still complete after the interposer"
        );
    }

    #[test]
    fn relevant_interposer_within_window_does_not_flush() {
        // Two unrelated org events can legitimately interleave with a rename
        // pair on fsevents — a FRESH pending survives a relevant interposer.
        let mut pairing = RenamePairing::new();
        let now = Instant::now();
        pairing.classify(&RawFsSignal::RenameFrom("/vault/a.org".into()), now, &rel);
        let interposed =
            pairing.classify(&RawFsSignal::Create("/vault/other.org".into()), now, &rel);
        assert!(
            !interposed
                .iter()
                .any(|(p, k)| p == Path::new("/vault/a.org") && *k == FileChangeKind::Remove),
            "a relevant interposer WITHIN the window must not flush a fresh pending From"
        );
        assert_eq!(
            pairing.classify(&RawFsSignal::RenameTo("/vault/b.org".into()), now, &rel),
            vec![(
                PathBuf::from("/vault/b.org"),
                FileChangeKind::Rename {
                    from: PathBuf::from("/vault/a.org")
                }
            )]
        );
    }

    #[test]
    fn lone_from_side_flushes_as_remove_on_timeout() {
        // A move OUT of the watched root: source side, no partner. It is flushed
        // as a disclosed Remove ONLY once it ages past the window — triggered by
        // a later RELEVANT event.
        let mut pairing = RenamePairing::new();
        let t0 = Instant::now();
        let gone = "/vault/a.org";
        pairing.classify(&RawFsSignal::RenameFrom(gone.into()), t0, &rel);
        // A relevant event AFTER the window elapses triggers the timeout flush.
        let later = t0 + RENAME_PAIR_WINDOW + Duration::from_millis(1);
        let out = pairing.classify(&RawFsSignal::Create("/vault/c.org".into()), later, &rel);
        assert!(
            out.contains(&(PathBuf::from(gone), FileChangeKind::Remove)),
            "a timed-out pending From is disclosed as a Remove"
        );
        assert!(out.contains(&(PathBuf::from("/vault/c.org"), FileChangeKind::Create)));
    }

    #[test]
    fn irrelevant_event_never_triggers_timeout_flush() {
        // Even a STALE pending must not be flushed by an IRRELEVANT event —
        // relevance-gating protects the buffer end-to-end.
        let mut pairing = RenamePairing::new();
        let t0 = Instant::now();
        pairing.classify(&RawFsSignal::RenameFrom("/vault/a.org".into()), t0, &rel);
        let later = t0 + RENAME_PAIR_WINDOW + Duration::from_millis(1);
        let out = pairing.classify(&RawFsSignal::Create("/vault/.tmp".into()), later, &rel);
        assert!(
            !out.iter().any(|(_, k)| *k == FileChangeKind::Remove),
            "an irrelevant event must never flush the pending From, even when stale"
        );
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
