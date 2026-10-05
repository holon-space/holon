//! `FileSystem` port (ADR 0011): all org/markdown SerDe disk access goes
//! through this trait so tests can substitute an in-memory adapter.
//!
//! Canonicalization is the adapter's responsibility — call sites pass paths
//! as-is and adapters normalise (real fs: `std::fs::canonicalize`, incl.
//! macOS `/var → /private/var` symlink resolution).

use std::path::Path;
use std::path::PathBuf;
use std::time::SystemTime;

use async_trait::async_trait;

/// Filesystem entries found by scanning a directory.
///
/// `files` contains *all* files (not filtered by extension) — format-specific
/// filters (`.org`, `.md`) belong to the caller.
#[derive(Debug, Default, Clone)]
pub struct ScannedEntries {
    pub files: Vec<PathBuf>,
}

/// Subset of `std::fs::Metadata` the sync path needs (dirty-check signatures).
#[derive(Debug, Clone, Copy)]
pub struct FileMeta {
    pub modified: SystemTime,
    pub len: u64,
}

/// What a file was when Holon read it: the basis a write-back over it is
/// checked against. Only an adapter mints one, from the read itself, so a
/// write-back cannot name a basis it never read.
///
/// The file is identified by inode (where the platform has one), size and
/// mtime: an atomic replacement changes the inode, an in-place edit the mtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStamp(Basis);

#[derive(Debug, Clone, PartialEq, Eq)]
enum Basis {
    Absent,
    Present {
        inode: Option<u64>,
        len: u64,
        modified: SystemTime,
    },
}

impl FileStamp {
    pub(crate) fn absent() -> Self {
        Self(Basis::Absent)
    }

    pub(crate) fn present(inode: Option<u64>, len: u64, modified: SystemTime) -> Self {
        Self(Basis::Present {
            inode,
            len,
            modified,
        })
    }

    fn of_metadata(meta: &std::fs::Metadata) -> std::io::Result<Self> {
        #[cfg(unix)]
        let inode = Some(std::os::unix::fs::MetadataExt::ino(meta));
        #[cfg(not(unix))]
        let inode = None;
        Ok(Self::present(inode, meta.len(), meta.modified()?))
    }

    /// The stamp of whatever `path` is now.
    fn current_blocking(path: &Path) -> std::io::Result<Self> {
        match std::fs::metadata(path) {
            Ok(meta) => Self::of_metadata(&meta),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::absent()),
            Err(e) => Err(e),
        }
    }
}

/// A file's text and the stamp of the file it came from. `content` is `None`
/// exactly when the file was absent.
#[derive(Debug, Clone)]
pub struct StampedRead {
    pub content: Option<String>,
    pub stamp: FileStamp,
}

/// Outcome of [`FileSystem::write_if_unchanged`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum WriteBack {
    Written,
    /// The file no longer matched the stamp; nothing was written.
    Changed,
}

/// "Where the bytes live" — see ADR 0011.
///
/// `write` is whole-buffer by design: there is no streaming / file-handle
/// API, so the end of a `write` call is the close boundary an in-memory
/// adapter can hook change notifications onto (no partial-write window).
#[async_trait]
pub trait FileSystem: Send + Sync {
    async fn read_to_string(&self, path: &Path) -> std::io::Result<String>;
    async fn read(&self, path: &Path) -> std::io::Result<Vec<u8>>;
    /// Read `path` as text together with the [`FileStamp`] of the file the
    /// text came from. An absent file is not an error.
    async fn read_stamped(&self, path: &Path) -> std::io::Result<StampedRead>;
    /// [`FileSystem::write`] that writes only while `path` still matches
    /// `expected`: the target is re-checked after the new bytes are staged,
    /// immediately before they replace it. A user edit or delete that lands
    /// between the read and the write is therefore never overwritten or
    /// recreated, except inside the window between that check and the rename.
    ///
    /// Every write-back of a file Holon read goes through here.
    async fn write_if_unchanged(
        &self,
        path: &Path,
        expected: &FileStamp,
        contents: &[u8],
    ) -> std::io::Result<WriteBack>;
    /// Replace `path`'s contents ATOMICALLY: a reader (our own ingest
    /// included) sees either the complete previous file or the complete new
    /// one, never an interior — ADR 0030 D3.1, whose motivation is that a torn
    /// org mirror is re-ingestable as authority corruption. Adapters implement
    /// this by writing a sibling temp and renaming over the target
    /// ([`write_atomic_blocking`]); an in-place write is a contract violation.
    async fn write(&self, path: &Path, contents: &[u8]) -> std::io::Result<()>;
    /// Remove a file. Errors if absent (like `std::fs::remove_file`). Adapters
    /// with a change-notification surface emit a `Remove` event, mirroring what
    /// the real `notify` watcher delivers for an on-disk deletion.
    async fn remove(&self, path: &Path) -> std::io::Result<()>;
    /// Atomically move `from` to `to`. Adapters with a change-notification
    /// surface emit ONE `Rename { from }` event on `to` — the atomic port that
    /// lets a consumer re-home a document without a delete-then-create window.
    ///
    /// The default is a non-atomic read→write→remove (kept so
    /// minimal test doubles need not implement it); the real and in-memory
    /// adapters override it with a genuine atomic move + paired event.
    async fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        let bytes = self.read(from).await?;
        self.write(to, &bytes).await?;
        self.remove(from).await
    }
    async fn create_dir_all(&self, path: &Path) -> std::io::Result<()>;
    /// Recursive walk respecting `.gitignore`, skipping hidden entries
    /// (`.git`, `.jj`, …). A missing `root` yields empty entries, not an
    /// error (matches the historical `scan_directory` contract).
    async fn scan_directory(&self, root: &Path) -> std::io::Result<ScannedEntries>;
    async fn metadata(&self, path: &Path) -> std::io::Result<FileMeta>;
    fn exists(&self, path: &Path) -> bool;
    /// Resolve symlinks / normalise. Errors on non-existent paths exactly
    /// like `std::fs::canonicalize` — callers rely on that error to resolve
    /// not-yet-created files via their nearest existing parent.
    fn canonicalize(&self, path: &Path) -> std::io::Result<PathBuf>;
}

/// Production adapter: thin passthrough to `tokio::fs` / `std::fs`.
pub struct RealFileSystem;

/// Name the path in an io error.
///
/// A bare `No such file or directory (os error 2)` travelling up through an
/// operation dispatch says nothing about WHICH file was missing, and the
/// operation error that wraps it can only add the operation and the entity —
/// it never knew the path. Naming it here, at the syscall, is the only place
/// the information exists. Also says whether the parent directory is there,
/// which separates "the vault went away" from "this document's folder was
/// never created".
fn at_path(op: &str, path: &Path, e: std::io::Error) -> std::io::Error {
    let parent = path.parent();
    std::io::Error::new(
        e.kind(),
        format!(
            "{e} while {op} {} (parent {}: {})",
            path.display(),
            parent.map_or_else(|| "<none>".to_string(), |p| p.display().to_string()),
            match parent {
                Some(p) if p.is_dir() => "exists",
                Some(_) => "MISSING",
                None => "n/a",
            }
        ),
    )
}

#[async_trait]
impl FileSystem for RealFileSystem {
    async fn read_to_string(&self, path: &Path) -> std::io::Result<String> {
        tokio::fs::read_to_string(path)
            .await
            .map_err(|e| at_path("reading", path, e))
    }

    async fn read(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        tokio::fs::read(path)
            .await
            .map_err(|e| at_path("reading", path, e))
    }

    async fn read_stamped(&self, path: &Path) -> std::io::Result<StampedRead> {
        let owned = path.to_path_buf();
        tokio::task::spawn_blocking(move || read_stamped_blocking(&owned))
            .await
            .map_err(|e| std::io::Error::other(format!("read join error: {e}")))?
            .map_err(|e| at_path("reading", path, e))
    }

    async fn write_if_unchanged(
        &self,
        path: &Path,
        expected: &FileStamp,
        contents: &[u8],
    ) -> std::io::Result<WriteBack> {
        let path = path.to_path_buf();
        let expected = expected.clone();
        let contents = contents.to_vec();
        tokio::task::spawn_blocking(move || {
            replace_via_temp(&path, &contents, Barrier::None, Some(&expected))
        })
        .await
        .map_err(|e| std::io::Error::other(format!("write join error: {e}")))?
    }

    async fn write(&self, path: &Path, contents: &[u8]) -> std::io::Result<()> {
        let path = path.to_path_buf();
        let contents = contents.to_vec();
        tokio::task::spawn_blocking(move || write_atomic_blocking(&path, &contents))
            .await
            .map_err(|e| std::io::Error::other(format!("write join error: {e}")))?
    }

    async fn remove(&self, path: &Path) -> std::io::Result<()> {
        tokio::fs::remove_file(path)
            .await
            .map_err(|e| std::io::Error::new(e.kind(), format!("remove {}: {e}", path.display())))
    }

    async fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        tokio::fs::rename(from, to).await.map_err(|e| {
            std::io::Error::new(
                e.kind(),
                format!("rename {} -> {}: {e}", from.display(), to.display()),
            )
        })
    }

    async fn create_dir_all(&self, path: &Path) -> std::io::Result<()> {
        tokio::fs::create_dir_all(path).await
    }

    async fn scan_directory(&self, root: &Path) -> std::io::Result<ScannedEntries> {
        let root = root.to_path_buf();
        // `ignore::WalkBuilder` is blocking (gitignore regex DFAs + readdir).
        tokio::task::spawn_blocking(move || walk_directory(&root))
            .await
            .map_err(|e| std::io::Error::other(format!("scan_directory join error: {e}")))
    }

    async fn metadata(&self, path: &Path) -> std::io::Result<FileMeta> {
        let meta = tokio::fs::metadata(path).await?;
        Ok(FileMeta {
            modified: meta.modified()?,
            len: meta.len(),
        })
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn canonicalize(&self, path: &Path) -> std::io::Result<PathBuf> {
        std::fs::canonicalize(path)
    }
}

/// Marks a name as the temp half of an in-flight atomic replacement. The ONE
/// place the convention is written down: [`atomic_temp_path`] builds with it,
/// [`atomic_temp_target`] reads it back.
const ATOMIC_TEMP_INFIX: &str = ".holon-tmp-";

/// Sibling temp path for an atomic replacement of `path`.
///
/// Invisible to ingest on two independent counts: it is dot-prefixed (the
/// gitignore-aware walk runs with `hidden(true)`, so [`walk_directory`] never
/// yields it) and its extension is not `.org` (every org-relevance filter —
/// `file_watcher::scan_directory`, `is_org_relevant`, the rename pairing's
/// `is_org_ext` — tests the extension). Same directory as the target so the
/// rename stays within one filesystem.
pub fn atomic_temp_path(path: &Path) -> std::io::Result<PathBuf> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let name = path.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("atomic write target has no file name: {}", path.display()),
        )
    })?;
    let unique = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut temp = std::ffi::OsString::from(".");
    temp.push(name);
    temp.push(format!(
        "{ATOMIC_TEMP_INFIX}{}-{unique}",
        std::process::id()
    ));
    Ok(path.with_file_name(temp))
}

/// The target `temp` is on its way to, or `None` when `temp` is not the temp
/// half of one of our atomic replacements.
///
/// The filesystem watcher needs this: our replacement reaches it as a rename,
/// and without recognizing the source half it reads the write as a document
/// being re-homed (see `RenamePairing`).
pub fn atomic_temp_target(temp: &Path) -> Option<PathBuf> {
    let name = temp.file_name()?.to_str()?;
    let stem = name.strip_prefix('.')?;
    let (target, unique) = stem.rsplit_once(ATOMIC_TEMP_INFIX)?;
    let (pid, counter) = unique.split_once('-')?;
    let minted = !target.is_empty()
        && !pid.is_empty()
        && !counter.is_empty()
        && pid.bytes().all(|b| b.is_ascii_digit())
        && counter.bytes().all(|b| b.is_ascii_digit());
    minted.then(|| temp.with_file_name(target))
}

/// A file-replacement step that bears on durability, as seen by
/// [`DurabilityRecorder`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DurabilityEvent {
    /// The file's bytes reached the device (`sync_all`).
    FileSynced(PathBuf),
    /// A temp file was renamed over this path.
    Replaced(PathBuf),
    /// The directory entry changes in this directory reached the device.
    DirSynced(PathBuf),
}

thread_local! {
    static RECORDED: std::cell::RefCell<Option<Vec<DurabilityEvent>>> =
        const { std::cell::RefCell::new(None) };
}

fn record_durability_event(event: DurabilityEvent) {
    RECORDED.with(|recorded| {
        if let Some(events) = recorded.borrow_mut().as_mut() {
            events.push(event);
        }
    });
}

/// Records the durability steps this thread's file writes take until
/// [`DurabilityRecorder::finish`].
pub struct DurabilityRecorder(());

impl DurabilityRecorder {
    pub fn start() -> Self {
        RECORDED.with(|recorded| {
            let previous = recorded.borrow_mut().replace(Vec::new());
            assert!(
                previous.is_none(),
                "a DurabilityRecorder is already recording on this thread"
            );
        });
        Self(())
    }

    pub fn finish(self) -> Vec<DurabilityEvent> {
        RECORDED
            .with(|recorded| recorded.borrow_mut().take())
            .expect("the recorder started on this thread")
    }
}

/// Write `contents` to a sibling temp and rename it over `path` — the atomic
/// replacement [`FileSystem::write`] promises (ADR 0030 D3.1).
///
/// No `fsync` before the rename: on macOS a durable barrier means
/// `F_FULLFSYNC` (a full device flush, tens of ms) on every write-back, and
/// what it would buy — the newest bytes surviving a power loss — is not owed
/// for a mirror that is re-derivable from the authority. Rename ordering alone
/// delivers what IS owed: no reader ever sees an interior. A file that IS the
/// authority uses [`write_durable_blocking`].
///
/// A symlink at `path` is REPLACED, not followed, so the bytes land exactly
/// where the caller's containment proof says they do.
pub fn write_atomic_blocking(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let written = replace_via_temp(path, contents, Barrier::None, None)?;
    assert_eq!(written, WriteBack::Written, "an unconditional write writes");
    Ok(())
}

/// [`write_atomic_blocking`] that also survives a power loss once it returns:
/// the temp's bytes reach the device before the rename, and the rename
/// reaches it before this returns.
pub fn write_durable_blocking(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let written = replace_via_temp(path, contents, Barrier::Durable, None)?;
    assert_eq!(written, WriteBack::Written, "an unconditional write writes");
    Ok(())
}

/// The stamp comes from the open handle, so it describes the file the text
/// was read from even when the path is replaced during the read.
fn read_stamped_blocking(path: &Path) -> std::io::Result<StampedRead> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(StampedRead {
                content: None,
                stamp: FileStamp::absent(),
            });
        }
        Err(e) => return Err(e),
    };
    let stamp = FileStamp::of_metadata(&file.metadata()?)?;
    let mut content = String::new();
    std::io::Read::read_to_string(&mut file, &mut content)?;
    Ok(StampedRead {
        content: Some(content),
        stamp,
    })
}

/// Make `file`'s data, open at `path`, survive a power loss.
pub fn sync_data_blocking(file: &std::fs::File, path: &Path) -> std::io::Result<()> {
    file.sync_data()?;
    record_durability_event(DurabilityEvent::FileSynced(path.to_path_buf()));
    Ok(())
}

/// Make the directory entries created or renamed in `dir` survive a power
/// loss.
pub fn sync_dir_blocking(dir: &Path) -> std::io::Result<()> {
    std::fs::File::open(dir)?.sync_all()?;
    record_durability_event(DurabilityEvent::DirSynced(dir.to_path_buf()));
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Barrier {
    None,
    Durable,
}

fn replace_via_temp(
    path: &Path,
    contents: &[u8],
    barrier: Barrier,
    expected: Option<&FileStamp>,
) -> std::io::Result<WriteBack> {
    let temp = atomic_temp_path(path)?;
    let replace = || -> std::io::Result<WriteBack> {
        let mut file = std::fs::File::create(&temp)?;
        std::io::Write::write_all(&mut file, contents)?;
        // A replacement must not silently reset a file's mode; the target's
        // permissions are the user's, not ours.
        if let Ok(meta) = std::fs::metadata(path) {
            std::fs::set_permissions(&temp, meta.permissions())?;
        }
        if barrier == Barrier::Durable {
            file.sync_all()?;
            record_durability_event(DurabilityEvent::FileSynced(temp.clone()));
        }
        drop(file);
        if let Some(expected) = expected {
            if FileStamp::current_blocking(path)? != *expected {
                std::fs::remove_file(&temp)?;
                return Ok(WriteBack::Changed);
            }
        }
        std::fs::rename(&temp, path)?;
        record_durability_event(DurabilityEvent::Replaced(path.to_path_buf()));
        if barrier == Barrier::Durable {
            let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
            sync_dir_blocking(parent.unwrap_or(Path::new(".")))?;
        }
        Ok(WriteBack::Written)
    };
    replace()
        .inspect_err(|_| {
            // The write's own error is what the caller acts on; a leftover temp
            // would be dead weight in the vault, so drop it best-effort.
            let _ = std::fs::remove_file(&temp);
        })
        // A bare `No such file or directory (os error 2)` names neither the
        // file nor the missing parent, so a write-back failure reaches the
        // caller — and the operation error a user or a gate sees — with no way
        // to tell WHICH path was absent. Say which, and whether the parent
        // exists: that distinguishes "the vault directory went away" from "the
        // document's folder was never created".
        .map_err(|e| at_path("atomically writing", path, e))
}

/// Synchronous gitignore-aware recursive walk — the single source of truth
/// for directory walking (moved here from `holon-orgmode::file_watcher`).
#[tracing::instrument(name = "scan_directory", fields(root = %root.display()))]
pub fn walk_directory(root: &Path) -> ScannedEntries {
    let mut files = Vec::new();

    if !root.exists() {
        return ScannedEntries { files };
    }

    for entry in ignore::WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .build()
        .flatten()
    {
        let path = entry.into_path();
        if path == root {
            continue;
        }
        if !path.is_dir() {
            files.push(path);
        }
    }

    ScannedEntries { files }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::MetadataExt;

    use super::*;

    /// ADR 0030 D3.1: a file mirror must never be observable between two
    /// authority commits. An in-place write mutates the SAME inode — every
    /// reader (including our own ingest) sees the interior of the write.
    /// Replacement makes the interior unreachable: the visible path holds the
    /// complete old bytes until the instant it holds the complete new ones.
    #[tokio::test]
    async fn write_replaces_the_target_instead_of_mutating_it_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let fs = RealFileSystem;
        let file = dir.path().join("page.org");
        fs.write(&file, b"* Old complete\n").await.unwrap();

        let before = std::fs::metadata(&file).unwrap().ino();
        // A reader that opened the file before the write is the observable
        // stand-in for "a crash left the interior visible": it keeps reading
        // the inode it opened.
        let held = std::fs::File::open(&file).unwrap();

        fs.write(&file, b"* New complete content\n").await.unwrap();

        let after = std::fs::metadata(&file).unwrap().ino();
        assert_ne!(
            before, after,
            "write mutated the visible inode in place — the tearing window ADR 0030 D3.1 forbids"
        );
        let mut old = String::new();
        {
            use std::io::Read;
            let mut held = held;
            held.read_to_string(&mut old).unwrap();
        }
        assert_eq!(
            old, "* Old complete\n",
            "an open reader saw the write's interior instead of the complete old file"
        );
        assert_eq!(
            fs.read_to_string(&file).await.unwrap(),
            "* New complete content\n"
        );
    }

    /// The same clause stated the way ingest experiences it: a reader polling
    /// the path while write-back runs only ever sees one whole version — never
    /// the truncate-and-refill interior of an in-place write.
    #[tokio::test]
    async fn concurrent_reader_never_observes_a_partial_file() {
        const LEN: usize = 512 * 1024;
        const ROUNDS: usize = 200;
        let dir = tempfile::tempdir().unwrap();
        let fs = RealFileSystem;
        let file = dir.path().join("big.org");
        let versions = [vec![b'a'; LEN], vec![b'b'; LEN]];
        fs.write(&file, &versions[0]).await.unwrap();

        let probe = file.clone();
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reader_done = done.clone();
        let reader = tokio::task::spawn_blocking(move || {
            let mut torn: Option<(usize, u8)> = None;
            let mut reads = 0usize;
            while !reader_done.load(std::sync::atomic::Ordering::Relaxed) && torn.is_none() {
                let Ok(bytes) = std::fs::read(&probe) else {
                    continue;
                };
                reads += 1;
                let whole = bytes.len() == LEN
                    && (bytes.iter().all(|b| *b == b'a') || bytes.iter().all(|b| *b == b'b'));
                if !whole {
                    torn = Some((bytes.len(), bytes.first().copied().unwrap_or(0)));
                }
            }
            (torn, reads)
        });

        for round in 0..ROUNDS {
            fs.write(&file, &versions[round % 2]).await.unwrap();
        }
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        let (torn, reads) = reader.await.unwrap();
        assert!(reads > 0, "probe never read the file — test is vacuous");
        assert!(
            torn.is_none(),
            "a concurrent reader observed a partial file (len, first byte = {:?}) — write is not \
             atomic",
            torn.unwrap()
        );
    }

    /// The temp side of the replacement must be invisible to ingest. Both
    /// independent reasons are asserted: hidden (the walk skips it) and
    /// non-`.org` (every relevance filter tests the extension).
    #[test]
    fn atomic_temp_name_is_invisible_to_the_ingest_walk() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("Some Page.org");
        let temp = atomic_temp_path(&target).unwrap();

        assert_eq!(temp.parent(), target.parent());
        let name = temp.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with('.'), "temp name is not hidden: {name}");
        assert_ne!(
            temp.extension().and_then(|e| e.to_str()),
            Some("org"),
            "temp name still looks like an org file: {name}"
        );

        std::fs::write(&temp, b"* partial").unwrap();
        std::fs::write(&target, b"* whole").unwrap();
        let scanned = walk_directory(dir.path());
        assert_eq!(scanned.files, vec![target]);
    }

    /// The watcher recognizes our in-flight replacements by name, so the two
    /// halves of the convention must agree — the pairing reads back exactly
    /// what the writer minted.
    #[test]
    fn an_atomic_temp_name_reads_back_as_its_target() {
        for target in ["page.org", "Some Page.org", "a.b.c.org", "image.png"] {
            let target = Path::new("/vault/sub").join(target);
            let temp = atomic_temp_path(&target).unwrap();
            assert_eq!(atomic_temp_target(&temp).as_deref(), Some(target.as_path()));
        }
    }

    #[test]
    fn a_foreign_name_is_not_mistaken_for_an_atomic_temp() {
        for foreign in [
            "page.org",
            ".hidden.org",
            ".page.org.holon-tmp-",
            ".page.org.holon-tmp-abc-1",
            ".page.org.holon-tmp-12",
            ".holon-tmp-1-2",
        ] {
            let path = Path::new("/vault").join(foreign);
            assert_eq!(atomic_temp_target(&path), None, "{foreign}");
        }
    }

    /// The replacement lands where the caller's containment proof says it does:
    /// the link is replaced, and whatever it pointed at is untouched.
    #[test]
    fn a_symlink_target_is_replaced_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let far = dir.path().join("far.org");
        std::fs::write(&far, b"* far original\n").unwrap();
        let link = dir.path().join("link.org");
        std::os::unix::fs::symlink(&far, &link).unwrap();

        write_atomic_blocking(&link, b"* new\n").unwrap();

        assert!(!link.is_symlink(), "the link survived as a link");
        assert_eq!(std::fs::read(&link).unwrap(), b"* new\n");
        assert_eq!(std::fs::read(&far).unwrap(), b"* far original\n");
    }

    #[test]
    fn a_replacement_keeps_the_targets_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("private.org");
        std::fs::write(&file, b"* old\n").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();

        write_atomic_blocking(&file, b"* new\n").unwrap();

        let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the replacement reset the file's mode");
    }

    #[test]
    fn atomic_temp_names_are_unique_per_call() {
        let target = Path::new("/vault/page.org");
        assert_ne!(
            atomic_temp_path(target).unwrap(),
            atomic_temp_path(target).unwrap()
        );
    }

    /// A failed replacement leaves the target on its previous complete bytes
    /// and drops the temp — the vault gains no debris from a failed write.
    #[test]
    fn a_failed_rename_leaves_the_target_and_no_debris() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("page.org");
        std::fs::write(&target, b"* Old complete\n").unwrap();
        // A non-empty directory at the target is a rename the kernel refuses
        // AFTER the temp is on disk.
        let blocked = dir.path().join("blocked.org");
        std::fs::create_dir(&blocked).unwrap();
        std::fs::write(blocked.join("inner"), b"x").unwrap();

        let err = write_atomic_blocking(&blocked, b"* New\n").unwrap_err();
        assert!(blocked.is_dir(), "target replaced despite the error: {err}");
        assert_eq!(std::fs::read(&target).unwrap(), b"* Old complete\n");
        assert_eq!(walk_directory(dir.path()).files.len(), 2);
    }

    #[tokio::test]
    async fn real_fs_roundtrip_and_scan() {
        let dir = tempfile::tempdir().unwrap();
        let fs = RealFileSystem;
        let file = dir.path().join("a.org");
        fs.write(&file, b"* Hello").await.unwrap();
        assert_eq!(fs.read_to_string(&file).await.unwrap(), "* Hello");
        assert!(fs.exists(&file));
        let meta = fs.metadata(&file).await.unwrap();
        assert_eq!(meta.len, 7);

        fs.write(&dir.path().join("b.txt"), b"x").await.unwrap();
        let scanned = fs.scan_directory(dir.path()).await.unwrap();
        assert_eq!(scanned.files.len(), 2);

        let missing = fs.scan_directory(&dir.path().join("nope")).await.unwrap();
        assert!(missing.files.is_empty());
    }

    /// What the user does between Holon's read and its write-back.
    fn interfere(path: &Path, how: &str) {
        match how {
            "untouched" => {}
            "edited in place" => std::fs::write(path, b"* User edit, longer\n").unwrap(),
            "replaced" => write_atomic_blocking(path, b"* Same len\n").unwrap(),
            "deleted" => std::fs::remove_file(path).unwrap(),
            "created" => std::fs::write(path, b"* User file\n").unwrap(),
            other => unreachable!("{other}"),
        }
    }

    #[tokio::test]
    async fn a_write_back_lands_only_on_the_file_it_read() {
        let fs = RealFileSystem;
        for (start, how, expected) in [
            (Some("* Same len\n"), "untouched", WriteBack::Written),
            (Some("* Same len\n"), "edited in place", WriteBack::Changed),
            (Some("* Same len\n"), "replaced", WriteBack::Changed),
            (Some("* Same len\n"), "deleted", WriteBack::Changed),
            (None, "untouched", WriteBack::Written),
            (None, "created", WriteBack::Changed),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join("page.org");
            if let Some(start) = start {
                std::fs::write(&file, start).unwrap();
            }
            let read = fs.read_stamped(&file).await.unwrap();
            assert_eq!(read.content.as_deref(), start);
            interfere(&file, how);
            let before = std::fs::read(&file).ok(); // ALLOW(ok): a deleted file is None

            let outcome = fs
                .write_if_unchanged(&file, &read.stamp, b"* Holon\n")
                .await
                .unwrap();

            assert_eq!(outcome, expected, "file {how} after the read");
            let after = std::fs::read(&file).ok(); // ALLOW(ok): a deleted file is None
            match outcome {
                WriteBack::Written => assert_eq!(after.as_deref(), Some(&b"* Holon\n"[..])),
                WriteBack::Changed => assert_eq!(after, before, "a dropped write touched {how}"),
            }
            let debris: Vec<_> = std::fs::read_dir(dir.path())
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .filter(|n| n != "page.org")
                .collect();
            assert!(debris.is_empty(), "temp left behind: {debris:?}");
        }
    }
}
