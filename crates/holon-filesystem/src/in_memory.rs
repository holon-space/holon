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
//! (e.g. `/holon-virtual/<test>`) so `holon::sync::CanonicalPath::new` — which
//! consults the real fs and falls back to the input path — degrades to the
//! same lexical identity everywhere.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use std::time::UNIX_EPOCH;

use async_trait::async_trait;
use tokio::sync::broadcast;

use crate::change_source::FileChange;
use crate::change_source::FileChangeKind;
use crate::change_source::FileChangeSource;
use crate::fs_port::FileMeta;
use crate::fs_port::FileSystem;
use crate::fs_port::ScannedEntries;

struct FileEntry {
    bytes: Vec<u8>,
    mtime_tick: u64,
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
}

pub struct InMemoryFileSystem {
    state: Mutex<State>,
    tx: broadcast::Sender<FileChange>,
}

impl Default for InMemoryFileSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryFileSystem {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(4096);
        Self {
            state: Mutex::new(State {
                files: BTreeMap::new(),
                dirs: BTreeSet::new(),
                clock: 0,
                write_targets: Vec::new(),
            }),
            tx,
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

    /// Synchronous `create_dir_all` for non-async construction contexts
    /// (the trait method delegates here).
    pub fn mkdir_all(&self, path: &Path) {
        let path = normalize(path);
        let mut st = self.lock();
        st.write_targets.push(path.clone());
        let mut cur = PathBuf::new();
        for comp in path.components() {
            cur.push(comp.as_os_str());
            st.dirs.insert(cur.clone());
        }
    }

    /// Remove a file, emitting a `Remove` change. Errors if absent.
    /// Synchronous core the trait's `remove` delegates to; pre-existing
    /// callers simulating external deletion use it directly.
    pub fn remove_file(&self, path: &Path) -> std::io::Result<()> {
        let path = normalize(path);
        let seq = {
            let mut st = self.lock();
            if st.files.remove(&path).is_none() {
                return Err(not_found(&path));
            }
            st.clock += 1;
            st.clock
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
        let from = normalize(from);
        let to = normalize(to);
        let seq = {
            let mut st = self.lock();
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
            let Some(entry) = st.files.remove(&from) else {
                return Err(not_found(&from));
            };
            st.clock += 1;
            let tick = st.clock;
            st.files.insert(
                to.clone(),
                FileEntry {
                    bytes: entry.bytes,
                    mtime_tick: tick,
                },
            );
            tick
        };
        let _ = self.tx.send(FileChange {
            path: to,
            kind: FileChangeKind::Rename { from },
            seq,
        });
        Ok(())
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
        let path = normalize(path);
        let st = self.lock();
        st.files
            .get(&path)
            .map(|f| f.bytes.clone())
            .ok_or_else(|| not_found(&path))
    }

    async fn write(&self, path: &Path, contents: &[u8]) -> std::io::Result<()> {
        let path = normalize(path);
        let (kind, tick) = {
            let mut st = self.lock();
            st.write_targets.push(path.clone());
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
            st.clock += 1;
            let tick = st.clock;
            let kind = if st.files.contains_key(&path) {
                FileChangeKind::Modify
            } else {
                FileChangeKind::Create
            };
            st.files.insert(
                path.clone(),
                FileEntry {
                    bytes: contents.to_vec(),
                    mtime_tick: tick,
                },
            );
            (kind, tick)
        };
        // The "close" hook: the full content is committed before anyone is
        // notified. send only errors when there are no subscribers — fine.
        let _ = self.tx.send(FileChange {
            path,
            kind,
            seq: tick,
        });
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
        let root = normalize(root);
        let st = self.lock();
        if !st.dirs.contains(&root) {
            return Ok(ScannedEntries::default());
        }
        Ok(ScannedEntries {
            files: st
                .files
                .keys()
                .filter(|f| f.starts_with(&root))
                .cloned()
                .collect(),
        })
    }

    async fn metadata(&self, path: &Path) -> std::io::Result<FileMeta> {
        let path = normalize(path);
        let st = self.lock();
        let entry = st.files.get(&path).ok_or_else(|| not_found(&path))?;
        Ok(FileMeta {
            modified: UNIX_EPOCH + Duration::from_nanos(entry.mtime_tick),
            len: entry.bytes.len() as u64,
        })
    }

    fn exists(&self, path: &Path) -> bool {
        let path = normalize(path);
        let st = self.lock();
        st.files.contains_key(&path) || st.dirs.contains(&path)
    }

    fn canonicalize(&self, path: &Path) -> std::io::Result<PathBuf> {
        let path = normalize(path);
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
        // Always armed: writes notify synchronously.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(rx.try_recv().unwrap().kind, FileChangeKind::Modify);
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
}
