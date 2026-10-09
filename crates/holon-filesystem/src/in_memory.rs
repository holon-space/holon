//! In-memory test adapter implementing BOTH ports (ADR 0011).
//!
//! `write` commits the full buffer to the map and then synchronously sends
//! the `FileChange` — the trait's whole-buffer `write` makes the end of the
//! call the close boundary, so a partial-write window is unrepresentable and
//! no debounce / mtime polling is needed.
//!
//! Path handling: purely lexical (`.` and `..` resolved, no symlinks).
//! `canonicalize` errors on non-existent paths for parity with
//! `std::fs::canonicalize`. Use a root that does not exist on the real disk
//! (e.g. `/holon-virtual/<test>`) so `holon_loro::CanonicalPath::new` — which
//! consults the real fs and falls back to the input path — degrades to the
//! same lexical identity everywhere.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use std::time::UNIX_EPOCH;

use async_trait::async_trait;
use tokio::sync::broadcast;
use tokio::sync::watch;

use crate::change_source::FileChange;
use crate::change_source::FileChangeKind;
use crate::change_source::FileChangeSource;
use crate::fs_port::FileMeta;
use crate::fs_port::FileStamp;
use crate::fs_port::FileSystem;
use crate::fs_port::ScannedEntries;
use crate::fs_port::StampedRead;
use crate::fs_port::WriteBack;

struct FileEntry {
    bytes: Vec<u8>,
    mtime_tick: u64,
}

impl FileEntry {
    /// Every write and rename takes a fresh tick, so the tick alone tells two
    /// versions of a path apart; there is no inode.
    fn stamp(&self) -> FileStamp {
        FileStamp::present(
            None,
            self.bytes.len() as u64,
            UNIX_EPOCH + Duration::from_nanos(self.mtime_tick),
        )
    }
}

struct State {
    files: BTreeMap<PathBuf, FileEntry>,
    dirs: BTreeSet<PathBuf>,
    clock: u64,
    /// Append-only log of every path this adapter was ASKED to create or
    /// write, normalized. Distinct from `files`/`dirs`, which hold only what
    /// currently exists: a containment check must see the target of a write
    /// that was later removed or overwritten.
    write_targets: Vec<PathBuf>,
    /// Armed by [`InMemoryFileSystem::fail_next_write_commit`].
    fail_next_write_commit: bool,
    /// Armed by [`InMemoryFileSystem::arm_write_churn`].
    churning: BTreeSet<PathBuf>,
    case: PathCase,
    /// [`PathCase::Insensitive`] only: every file and directory, keyed by its
    /// parent's stored spelling and the [`holon_api::caseless_fold`] of its
    /// name, so [`State::spelled`] folds the path it is given, not the store.
    folded: HashMap<(PathBuf, String), PathBuf>,
}

/// Whether two spellings of a path that differ only in letter case name one
/// entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathCase {
    /// Linux file systems.
    Sensitive,
    /// APFS, the macOS default: an entry keeps the spelling it was created
    /// with, and every spelling with its [`holon_api::caseless_fold`] (case
    /// and Unicode normalization variants) reaches it.
    Insensitive,
}

impl State {
    /// The stored spelling of `path` (already normalized): each component
    /// takes the spelling of the existing entry it names, if any.
    fn spelled(&self, path: &Path) -> PathBuf {
        if self.case == PathCase::Sensitive {
            return path.to_path_buf();
        }
        let mut out = PathBuf::new();
        for comp in path.components() {
            let key = (out.clone(), fold_name(comp.as_os_str()));
            out = match self.folded.get(&key) {
                Some(existing) => existing.clone(),
                None => out.join(comp.as_os_str()),
            };
        }
        out
    }

    fn folded_key(&self, path: &Path) -> Option<(PathBuf, String)> {
        if self.case == PathCase::Sensitive {
            return None;
        }
        Some((path.parent()?.to_path_buf(), fold_name(path.file_name()?)))
    }

    fn insert_dir(&mut self, path: PathBuf) {
        if let Some(key) = self.folded_key(&path) {
            self.folded.insert(key, path.clone());
        }
        self.dirs.insert(path);
    }

    fn insert_file(&mut self, path: PathBuf, entry: FileEntry) {
        if let Some(key) = self.folded_key(&path) {
            self.folded.insert(key, path.clone());
        }
        self.files.insert(path, entry);
    }

    fn remove_file_entry(&mut self, path: &Path) -> Option<FileEntry> {
        let entry = self.files.remove(path)?;
        if let Some(key) = self.folded_key(path) {
            if self.folded.get(&key).is_some_and(|stored| stored == path)
                && !self.dirs.contains(path)
            {
                self.folded.remove(&key);
            }
        }
        Some(entry)
    }
}

fn fold_name(name: &std::ffi::OsStr) -> String {
    holon_api::caseless_fold(&name.to_string_lossy())
}

pub struct InMemoryFileSystem {
    state: Mutex<State>,
    tx: broadcast::Sender<FileChange>,
    /// `true` while [`InMemoryFileSystem::hold_scans`] parks every scan.
    scans_held: watch::Sender<bool>,
    /// Extensions whose reads [`InMemoryFileSystem::hold_reads_with_extension`]
    /// parks.
    reads_held: watch::Sender<BTreeSet<String>>,
}

impl Default for InMemoryFileSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryFileSystem {
    pub fn new() -> Self {
        Self::with_path_case(PathCase::Sensitive)
    }

    pub fn with_path_case(case: PathCase) -> Self {
        let (tx, _) = broadcast::channel(4096);
        Self {
            state: Mutex::new(State {
                files: BTreeMap::new(),
                dirs: BTreeSet::new(),
                clock: 0,
                write_targets: Vec::new(),
                fail_next_write_commit: false,
                churning: BTreeSet::new(),
                case,
                folded: HashMap::new(),
            }),
            tx,
            scans_held: watch::Sender::new(false),
            reads_held: watch::Sender::new(BTreeSet::new()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .expect("InMemoryFileSystem mutex poisoned")
    }

    /// Highest change seq emitted so far. Pair with a consumer-side processed
    /// watermark to await "everything I wrote has been processed"
    /// deterministically.
    pub fn last_change_seq(&self) -> u64 {
        self.lock().clock
    }

    /// Every path this adapter was asked to write or create, normalized and in
    /// call order. Feeds the containment invariant: a write ATTEMPT that
    /// escaped the vault root is a defect even when the write itself failed.
    pub fn write_targets(&self) -> Vec<PathBuf> {
        self.lock().write_targets.clone()
    }

    /// Fail the NEXT `write` after its temp copy exists but before that copy
    /// replaces the target — the crash window an in-place write would tear in.
    /// The target must come out of it holding its complete previous bytes.
    pub fn fail_next_write_commit(&self) {
        self.lock().fail_next_write_commit = true;
    }

    /// Park every `scan_directory` until [`Self::release_scans`], so a test
    /// can hold a boot inside its initial vault scan.
    pub fn hold_scans(&self) {
        self.scans_held.send_replace(true);
    }

    pub fn release_scans(&self) {
        self.scans_held.send_replace(false);
    }

    /// Until [`Self::disarm_write_churn`], some process rewrites `path` with
    /// its own bytes just before every conditional write to it, so each
    /// `write_if_unchanged` sees a new stamp over unchanged bytes. The arm
    /// follows the file through a rename and ends with its removal.
    pub fn arm_write_churn(&self, path: &Path) {
        let mut st = self.lock();
        let path = st.spelled(&normalize(path));
        assert!(
            st.files.contains_key(&path),
            "arm_write_churn: no file {}",
            path.display()
        );
        assert!(
            st.churning.insert(path.clone()),
            "arm_write_churn: {} is already armed",
            path.display()
        );
    }

    pub fn disarm_write_churn(&self, path: &Path) {
        let mut st = self.lock();
        let path = st.spelled(&normalize(path));
        assert!(
            st.churning.remove(&path),
            "disarm_write_churn: {} is not armed",
            path.display()
        );
    }

    /// Give `path` a new stamp over the same bytes, as a no-op save does.
    pub fn touch_file(&self, path: &Path) -> std::io::Result<()> {
        let (path, seq) = {
            let mut st = self.lock();
            let path = st.spelled(&normalize(path));
            let seq = Self::retick(&mut st, &path)?;
            (path, seq)
        };
        let _ = self.tx.send(FileChange {
            path,
            kind: FileChangeKind::Create,
            seq,
        });
        Ok(())
    }

    fn retick(st: &mut State, path: &Path) -> std::io::Result<u64> {
        st.clock += 1;
        let tick = st.clock;
        st.files
            .get_mut(path)
            .ok_or_else(|| not_found(path))?
            .mtime_tick = tick;
        Ok(tick)
    }

    /// Park every read of a file named `*.{ext}` until
    /// [`Self::release_reads_with_extension`], so a test can hold one format's
    /// files back while the rest of the vault is read.
    pub fn hold_reads_with_extension(&self, ext: &str) {
        self.reads_held.send_modify(|held| {
            held.insert(ext.to_string());
        });
    }

    pub fn release_reads_with_extension(&self, ext: &str) {
        self.reads_held.send_modify(|held| {
            held.remove(ext);
        });
    }

    async fn wait_until_readable(&self, path: &Path) {
        let Some(ext) = path.extension().map(|e| e.to_string_lossy().into_owned()) else {
            return;
        };
        self.reads_held
            .subscribe()
            .wait_for(|held| !held.contains(&ext))
            .await
            .expect("the file system owns the read-hold sender");
    }

    /// Synchronous `create_dir_all` for non-async construction contexts
    /// (the trait method delegates here).
    pub fn mkdir_all(&self, path: &Path) {
        let path = normalize(path);
        let mut st = self.lock();
        st.write_targets.push(path.clone());
        let mut cur = PathBuf::new();
        for comp in path.components() {
            cur = st.spelled(&cur.join(comp.as_os_str()));
            st.insert_dir(cur.clone());
        }
    }

    /// Remove a file, emitting a `Remove` change. Errors if absent.
    /// Synchronous core the trait's `remove` delegates to; pre-existing
    /// callers simulating external deletion use it directly.
    pub fn remove_file(&self, path: &Path) -> std::io::Result<()> {
        let (path, seq) = {
            let mut st = self.lock();
            let path = st.spelled(&normalize(path));
            if st.remove_file_entry(&path).is_none() {
                return Err(not_found(&path));
            }
            st.churning.remove(&path);
            st.clock += 1;
            (path, st.clock)
        };
        let _ = self.tx.send(FileChange {
            path,
            kind: FileChangeKind::Remove,
            seq,
        });
        Ok(())
    }

    /// Atomically move `from` to `to`, emitting ONE `Rename { from }` change on
    /// `to` — the in-memory analog of the paired atomic rename the
    /// `NotifyWatcher` reconstructs on real disk. Errors if `from` is absent or
    /// `to`'s parent directory does not exist (parity with `std::fs::rename`).
    /// The two paths are the ONLY event this move produces: no `Remove(from)` +
    /// `Create(to)` pair, so `FileSyncController::on_file_renamed` re-homes the
    /// document without the delete-then-create window a `mv` used to open.
    pub fn rename_file(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        let (from, to, seq) = {
            let mut st = self.lock();
            let from = st.spelled(&normalize(from));
            let to = normalize(to);
            // Onto another entry, the target keeps that entry's spelling; a
            // rename of an entry onto a case variant of itself respells it.
            let to = match st.spelled(&to) {
                existing if existing == from => to.parent().map_or(to.clone(), |parent| {
                    st.spelled(parent)
                        .join(to.file_name().expect("a renamed file has a name"))
                }),
                existing => existing,
            };
            match to.parent() {
                Some(parent) if st.dirs.contains(parent) => {}
                Some(parent) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!(
                            "Parent directory does not exist (in-memory): {}",
                            parent.display()
                        ),
                    ));
                }
                None => return Err(not_found(&to)),
            }
            let Some(entry) = st.remove_file_entry(&from) else {
                return Err(not_found(&from));
            };
            if st.churning.remove(&from) {
                st.churning.insert(to.clone());
            }
            st.clock += 1;
            let tick = st.clock;
            st.insert_file(
                to.clone(),
                FileEntry {
                    bytes: entry.bytes,
                    mtime_tick: tick,
                },
            );
            (from, to, tick)
        };
        let _ = self.tx.send(FileChange {
            path: to,
            kind: FileChangeKind::Rename { from },
            seq,
        });
        Ok(())
    }

    /// The core of `write` and `write_if_unchanged`. The stamp check and the
    /// commit hold one lock, so here the check has no window at all.
    fn replace(
        &self,
        path: &Path,
        contents: &[u8],
        expected: Option<&FileStamp>,
    ) -> std::io::Result<WriteBack> {
        let (path, kind, tick) = {
            let mut st = self.lock();
            let given = normalize(path);
            st.write_targets.push(given.clone());
            let path = st.spelled(&given);
            let temp = crate::fs_port::atomic_temp_path(&path)?;
            match path.parent() {
                Some(parent) if st.dirs.contains(parent) => {}
                Some(parent) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!(
                            "Parent directory does not exist (in-memory): {}",
                            parent.display()
                        ),
                    ));
                }
                None => return Err(not_found(&path)),
            }
            if expected.is_some() && st.churning.contains(&path) {
                let seq = Self::retick(&mut st, &path)?;
                drop(st);
                let _ = self.tx.send(FileChange {
                    path,
                    kind: FileChangeKind::Create,
                    seq,
                });
                return Ok(WriteBack::Changed);
            }
            if let Some(expected) = expected {
                let current = st
                    .files
                    .get(&path)
                    .map_or_else(FileStamp::absent, FileEntry::stamp);
                if current != *expected {
                    return Ok(WriteBack::Changed);
                }
            }
            st.clock += 1;
            let tick = st.clock;
            // The temp side of the real adapter's temp+rename, so a test can
            // fail the replacement at the commit boundary and see the target
            // still hold its complete previous bytes (ADR 0030 D3.1).
            st.insert_file(
                temp.clone(),
                FileEntry {
                    bytes: contents.to_vec(),
                    mtime_tick: tick,
                },
            );
            if st.fail_next_write_commit {
                st.fail_next_write_commit = false;
                st.remove_file_entry(&temp);
                return Err(std::io::Error::other(format!(
                    "injected failure between temp write and rename (in-memory): {}",
                    path.display()
                )));
            }
            let entry = st
                .remove_file_entry(&temp)
                .expect("temp entry just inserted");
            st.insert_file(path.clone(), entry);
            // `Create`, not `Modify`, whether or not the target existed: an
            // atomic replacement reaches the real watcher as the `To` half of a
            // rename, which `RenamePairing` classifies as a Create. A double
            // that emits a shape the production adapter never produces cannot
            // be trusted to prove anything about the watcher.
            (path, FileChangeKind::Create, tick)
        };
        // The "close" hook: the full content is committed before anyone is
        // notified. send only errors when there are no subscribers — fine.
        let _ = self.tx.send(FileChange {
            path,
            kind,
            seq: tick,
        });
        Ok(WriteBack::Written)
    }
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn not_found(path: &Path) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("No such file or directory (in-memory): {}", path.display()),
    )
}

#[async_trait]
impl FileSystem for InMemoryFileSystem {
    async fn read_to_string(&self, path: &Path) -> std::io::Result<String> {
        let bytes = self.read(path).await?;
        String::from_utf8(bytes)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    async fn read(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        self.wait_until_readable(path).await;
        let st = self.lock();
        let path = st.spelled(&normalize(path));
        st.files
            .get(&path)
            .map(|f| f.bytes.clone())
            .ok_or_else(|| not_found(&path))
    }

    async fn read_stamped(&self, path: &Path) -> std::io::Result<StampedRead> {
        self.wait_until_readable(path).await;
        let st = self.lock();
        let path = st.spelled(&normalize(path));
        let Some(entry) = st.files.get(&path) else {
            return Ok(StampedRead {
                content: None,
                stamp: FileStamp::absent(),
            });
        };
        let content = String::from_utf8(entry.bytes.clone())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        Ok(StampedRead {
            content: Some(content),
            stamp: entry.stamp(),
        })
    }

    async fn write_if_unchanged(
        &self,
        path: &Path,
        expected: &FileStamp,
        contents: &[u8],
    ) -> std::io::Result<WriteBack> {
        self.replace(path, contents, Some(expected))
    }

    async fn write(&self, path: &Path, contents: &[u8]) -> std::io::Result<()> {
        let written = self.replace(path, contents, None)?;
        assert_eq!(written, WriteBack::Written, "an unconditional write writes");
        Ok(())
    }

    async fn remove(&self, path: &Path) -> std::io::Result<()> {
        // Emits `FileChangeKind::Remove` on the same broadcast channel as
        // `write` — the in-memory analog of the `notify` deletion event, so
        // the org watcher's `on_file_changed` runs for the removed path.
        self.remove_file(path)
    }

    async fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        self.rename_file(from, to)
    }

    async fn create_dir_all(&self, path: &Path) -> std::io::Result<()> {
        self.mkdir_all(path);
        Ok(())
    }

    async fn scan_directory(&self, root: &Path) -> std::io::Result<ScannedEntries> {
        self.scans_held
            .subscribe()
            .wait_for(|held| !held)
            .await
            .expect("the file system owns the scan-hold sender");
        let st = self.lock();
        let root = st.spelled(&normalize(root));
        if !st.dirs.contains(&root) {
            return Ok(ScannedEntries::default());
        }
        // The harness answers vault membership through the SAME filter the live
        // watcher asks and the real walk implements. Returning what the walk
        // drops would give the harness a vault production never sees, which is
        // how a stale copy under `.claude/worktrees/` stayed invisible to the
        // fleet.
        let filter = crate::vault_filter::VaultFilter::for_root(&root);
        Ok(ScannedEntries {
            files: st
                .files
                .keys()
                .filter(|f| f.starts_with(&root) && filter.admits(f))
                .cloned()
                .collect(),
        })
    }

    async fn metadata(&self, path: &Path) -> std::io::Result<FileMeta> {
        let st = self.lock();
        let path = st.spelled(&normalize(path));
        let entry = st.files.get(&path).ok_or_else(|| not_found(&path))?;
        Ok(FileMeta {
            modified: UNIX_EPOCH + Duration::from_nanos(entry.mtime_tick),
            len: entry.bytes.len() as u64,
        })
    }

    fn exists(&self, path: &Path) -> bool {
        let st = self.lock();
        let path = st.spelled(&normalize(path));
        st.files.contains_key(&path) || st.dirs.contains(&path)
    }

    fn canonicalize(&self, path: &Path) -> std::io::Result<PathBuf> {
        let path = self.lock().spelled(&normalize(path));
        if self.exists(&path) {
            Ok(path)
        } else {
            Err(not_found(&path))
        }
    }
}

impl FileChangeSource for InMemoryFileSystem {
    fn subscribe(&self) -> broadcast::Receiver<FileChange> {
        self.tx.subscribe()
    }

    fn arm(&self, _: &Path) -> std::io::Result<()> {
        #[cfg(feature = "crash-injection")]
        if crate::crash_injection::fires("arm_watcher") {
            return Err(std::io::Error::other(
                "[crash-injection] arming the watcher fails",
            ));
        }
        // Always armed: writes notify synchronously.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_held_extension_parks_its_reads_only() {
        let fs = std::sync::Arc::new(InMemoryFileSystem::new());
        fs.create_dir_all(Path::new("/v")).await.unwrap();
        fs.write(Path::new("/v/a.cook"), b"recipe").await.unwrap();
        fs.write(Path::new("/v/b.org"), b"page").await.unwrap();
        fs.hold_reads_with_extension("cook");

        assert_eq!(fs.read(Path::new("/v/b.org")).await.unwrap(), b"page");
        let held = tokio::spawn({
            let fs = fs.clone();
            async move { fs.read_to_string(Path::new("/v/a.cook")).await }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!held.is_finished(), "a read of a held extension returned");

        fs.release_reads_with_extension("cook");
        assert_eq!(held.await.unwrap().unwrap(), "recipe");
    }

    #[tokio::test]
    async fn write_fires_change_synchronously_and_reads_back() {
        let fs = InMemoryFileSystem::new();
        let mut rx = fs.subscribe();
        fs.create_dir_all(Path::new("/holon-virtual/vault"))
            .await
            .unwrap();
        fs.write(Path::new("/holon-virtual/vault/a.org"), b"* A")
            .await
            .unwrap();

        let change = rx
            .try_recv()
            .expect("change must be available synchronously");
        assert_eq!(change.kind, FileChangeKind::Create);
        assert_eq!(change.path, PathBuf::from("/holon-virtual/vault/a.org"));
        assert_eq!(
            fs.read_to_string(Path::new("/holon-virtual/vault/a.org"))
                .await
                .unwrap(),
            "* A"
        );

        fs.write(Path::new("/holon-virtual/vault/a.org"), b"* B")
            .await
            .unwrap();
        // Replacing an existing file announces a Create, matching the shape the
        // production watcher derives from the rename that replacement performs.
        assert_eq!(rx.try_recv().unwrap().kind, FileChangeKind::Create);
    }

    /// ADR 0030 D3.1 as the double must uphold it: a replacement that dies at
    /// the commit boundary leaves the complete previous bytes visible, emits
    /// no change, and leaves no temp behind.
    #[tokio::test]
    async fn a_failed_write_commit_leaves_the_previous_bytes_visible() {
        let fs = InMemoryFileSystem::new();
        let page = Path::new("/holon-virtual/vault/page.org");
        fs.create_dir_all(Path::new("/holon-virtual/vault"))
            .await
            .unwrap();
        fs.write(page, b"* Old complete").await.unwrap();
        let mut rx = fs.subscribe();

        fs.fail_next_write_commit();
        let err = fs.write(page, b"* New complete").await.unwrap_err();
        assert!(err.to_string().contains("temp write and rename"), "{err}");

        assert_eq!(fs.read_to_string(page).await.unwrap(), "* Old complete");
        assert!(rx.try_recv().is_err(), "a failed write announced a change");
        let scanned = fs
            .scan_directory(Path::new("/holon-virtual/vault"))
            .await
            .unwrap();
        assert_eq!(scanned.files, vec![page.to_path_buf()]);

        // Only the NEXT write fails; the double is not left permanently armed.
        fs.write(page, b"* New complete").await.unwrap();
        assert_eq!(fs.read_to_string(page).await.unwrap(), "* New complete");
    }

    /// As on APFS: a case variant of a path reaches the stored entry, which
    /// keeps its first spelling through writes and renames onto it; only a
    /// rename of an entry onto a variant of itself respells it.
    #[tokio::test]
    async fn a_case_insensitive_store_keeps_one_entry_per_folded_path() {
        let fs = InMemoryFileSystem::with_path_case(PathCase::Insensitive);
        fs.create_dir_all(Path::new("/v/Sub")).await.unwrap();
        fs.write(Path::new("/v/My Notes.org"), b"first")
            .await
            .unwrap();
        fs.write(Path::new("/v/my notes.org"), b"second")
            .await
            .unwrap();
        assert_eq!(
            fs.read(Path::new("/v/MY NOTES.org")).await.unwrap(),
            b"second"
        );
        fs.write(Path::new("/v/sub/a.org"), b"a").await.unwrap();
        let scanned = fs.scan_directory(Path::new("/V")).await.unwrap();
        assert_eq!(
            scanned.files,
            vec![
                PathBuf::from("/v/My Notes.org"),
                PathBuf::from("/v/Sub/a.org")
            ]
        );
        assert_eq!(
            fs.canonicalize(Path::new("/v/my notes.org")).unwrap(),
            PathBuf::from("/v/My Notes.org")
        );

        fs.write(Path::new("/v/b.org"), b"b").await.unwrap();
        fs.rename_file(Path::new("/v/b.org"), Path::new("/v/MY NOTES.org"))
            .unwrap();
        fs.rename_file(Path::new("/v/sub/a.org"), Path::new("/v/Sub/A.org"))
            .unwrap();
        let scanned = fs.scan_directory(Path::new("/v")).await.unwrap();
        assert_eq!(
            scanned.files,
            vec![
                PathBuf::from("/v/My Notes.org"),
                PathBuf::from("/v/Sub/A.org")
            ]
        );
        assert_eq!(fs.read(Path::new("/v/my notes.org")).await.unwrap(), b"b");

        let sensitive = InMemoryFileSystem::new();
        sensitive.create_dir_all(Path::new("/v")).await.unwrap();
        sensitive.write(Path::new("/v/A.org"), b"1").await.unwrap();
        sensitive.write(Path::new("/v/a.org"), b"2").await.unwrap();
        assert_eq!(
            sensitive
                .scan_directory(Path::new("/v"))
                .await
                .unwrap()
                .files
                .len(),
            2
        );
    }

    /// The Unicode spellings APFS names one file by: writing the first then
    /// the second leaves the files the host leaves, under the first
    /// spelling, holding the second write. On macOS the host is asked too.
    #[tokio::test]
    async fn a_case_insensitive_store_folds_like_apfs() {
        let pairs: &[(&str, &str, usize)] = &[
            ("caf\u{e9}", "cafe\u{301}", 1),
            ("\u{e4}", "a\u{308}", 1),
            ("Strasse", "Stra\u{df}e", 1),
            ("\u{3c3}", "\u{3c2}", 1),
            ("\u{130}stanbul", "i\u{307}stanbul", 1),
            ("\u{c4}pfel", "\u{e4}pfel", 1),
            ("My Notes", "My  Notes", 2),
        ];
        for (first, second, files) in pairs {
            let fs = InMemoryFileSystem::with_path_case(PathCase::Insensitive);
            fs.create_dir_all(Path::new("/v")).await.unwrap();
            let first_path = PathBuf::from(format!("/v/{first}.org"));
            fs.write(&first_path, b"first").await.unwrap();
            fs.write(Path::new(&format!("/v/{second}.org")), b"second")
                .await
                .unwrap();
            let scanned = fs.scan_directory(Path::new("/v")).await.unwrap().files;
            assert_eq!(scanned.len(), *files, "{first:?} / {second:?}: {scanned:?}");
            assert!(
                scanned.contains(&first_path),
                "{first:?} / {second:?}: {scanned:?}"
            );
            if *files == 1 {
                assert_eq!(fs.read(&first_path).await.unwrap(), b"second");
            }

            if cfg!(target_os = "macos") {
                let tmp = tempfile::tempdir().unwrap();
                std::fs::write(tmp.path().join(format!("{first}.org")), b"first").unwrap();
                std::fs::write(tmp.path().join(format!("{second}.org")), b"second").unwrap();
                let host = std::fs::read_dir(tmp.path()).unwrap().count();
                assert_eq!(host, *files, "{first:?} / {second:?}: the host disagrees");
            }
        }
    }

    #[tokio::test]
    async fn parity_errors_and_scan() {
        let fs = InMemoryFileSystem::new();
        // write without parent dir fails like the real fs
        assert!(fs.write(Path::new("/nope/x.org"), b"x").await.is_err());
        // canonicalize errors on missing paths like std::fs::canonicalize
        assert!(fs.canonicalize(Path::new("/nope")).is_err());

        fs.create_dir_all(Path::new("/r/sub")).await.unwrap();
        fs.write(Path::new("/r/a.org"), b"a").await.unwrap();
        fs.write(Path::new("/r/sub/b.org"), b"b").await.unwrap();
        fs.write(Path::new("/r/sub/c.txt"), b"c").await.unwrap();

        let scanned = fs.scan_directory(Path::new("/r")).await.unwrap();
        assert_eq!(scanned.files.len(), 3);
        assert!(scanned.files.contains(&PathBuf::from("/r/sub/b.org")));

        let meta_a = fs.metadata(Path::new("/r/a.org")).await.unwrap();
        let meta_b = fs.metadata(Path::new("/r/sub/b.org")).await.unwrap();
        assert!(meta_b.modified > meta_a.modified, "mtimes are monotonic");

        fs.remove_file(Path::new("/r/a.org")).unwrap();
        assert!(!fs.exists(Path::new("/r/a.org")));
    }

    /// The real adapter's walk never yields a hidden entry, so a harness that
    /// yields them tests a vault production cannot have — and the stale vault
    /// copies under `.claude/worktrees/` are exactly that shape.
    #[tokio::test]
    async fn scan_skips_hidden_entries_like_the_real_walk() {
        let fs = InMemoryFileSystem::new();
        fs.create_dir_all(Path::new("/r/.claude/worktrees/x"))
            .await
            .unwrap();
        fs.write(Path::new("/r/a.org"), b"a").await.unwrap();
        fs.write(Path::new("/r/.claude/worktrees/x/a.org"), b"stale")
            .await
            .unwrap();

        let scanned = fs.scan_directory(Path::new("/r")).await.unwrap();
        assert_eq!(scanned.files, vec![PathBuf::from("/r/a.org")]);
    }

    #[tokio::test]
    async fn a_write_back_lands_only_on_the_file_it_read() {
        let fs = InMemoryFileSystem::new();
        let page = Path::new("/holon-virtual/vault/a.org");
        fs.mkdir_all(page.parent().unwrap());
        fs.write(page, b"* A").await.unwrap();

        let read = fs.read_stamped(page).await.unwrap();
        fs.write(page, b"* B").await.unwrap();
        let outcome = fs.write_if_unchanged(page, &read.stamp, b"* Holon").await;
        assert_eq!(outcome.unwrap(), WriteBack::Changed);
        assert_eq!(fs.read(page).await.unwrap(), b"* B");

        let read = fs.read_stamped(page).await.unwrap();
        fs.remove_file(page).unwrap();
        let outcome = fs.write_if_unchanged(page, &read.stamp, b"* Holon").await;
        assert_eq!(outcome.unwrap(), WriteBack::Changed);
        assert!(!fs.exists(page));

        let read = fs.read_stamped(page).await.unwrap();
        assert_eq!(read.content, None);
        let outcome = fs.write_if_unchanged(page, &read.stamp, b"* Holon").await;
        assert_eq!(outcome.unwrap(), WriteBack::Written);
        assert_eq!(fs.read(page).await.unwrap(), b"* Holon");
    }
}
