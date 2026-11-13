//! The one place that opens a Holon Turso database.
//!
//! No open deletes a row. Before any write, the open reads the engine format
//! the file records. A file in another or an unknown format is moved aside
//! whole, unchanged, because an engine must not drop views whose stored state
//! it does not know. The open then rebuilds a copy of the moved file in its
//! place: it drops what the vault, the Loro store and the integrations restore
//! and keeps every other table with its rows. A copy that cannot be rebuilt
//! gives way to a fresh database.
//! A file in this engine's format that this binary cannot use as it is loses
//! only what it computed: its materialized views, rebuilt from the tables they
//! read, and the `block_derived` cache. That happens when the file was built
//! with a different [`scalar_fns`] set, or when one of its views no longer
//! loads. A damaged file is moved aside, never deleted: the shared-WAL
//! coordination file or the write-ahead log alone when the error points there,
//! else every file of the database. Every engine error of the open, from the
//! file open to the last read, is classified the same way; any failure other
//! than damage or a foreign format leaves the files where they are.

use std::path::Path;
use std::sync::Arc;

use holon_core::replica_state::DurableReplicaState;
use holon_core::storage::types::DatabaseOpenFailure;
use holon_core::storage::types::Result;
use holon_core::storage::types::StorageError;
use turso_core::CompletionError;
use turso_core::ConnectOpts;
use turso_core::Connection;
use turso_core::Database;
use turso_core::DatabaseOpts;
use turso_core::IO;
use turso_core::LimboError;
use turso_core::MemoryIO;
use turso_core::NamespaceId;
use turso_core::OpenFlags;
use turso_core::OpenOptions;
use turso_core::Value;

use crate::durable_state::TursoDurableState;
use crate::scalar_fns;
use crate::scalar_fns::ScalarFn;

/// What an open did to the database files, in the order it did it. Empty when
/// it kept them as they were.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenOutcome(pub Vec<OpenChange>);

impl OpenOutcome {
    pub const KEPT: Self = Self(Vec::new());
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenChange {
    /// The file could not be used as it was: its materialized views and the
    /// `block_derived` cache were dropped. Every table row is kept.
    ViewsDropped { reason: String, views: Vec<String> },
    /// The shared-WAL coordination file was damaged. It holds no rows; it was
    /// renamed to `backup`.
    CoordinationFileMovedAside {
        reason: String,
        backup: std::path::PathBuf,
    },
    /// The write-ahead log was damaged and the main file intact. The log was
    /// renamed to `backup`; the commits it held and the main file did not are
    /// in that file only.
    LogMovedAside {
        reason: String,
        backup: std::path::PathBuf,
    },
    /// The engine could not use the file at all, or the file is in an engine
    /// format this binary does not maintain. It was renamed to `backup`; the
    /// next change says whether a copy of it or a fresh database took its
    /// place.
    MovedAside {
        reason: String,
        backup: std::path::PathBuf,
    },
    /// A copy of the moved file `backup` took its place, rebuilt: every table
    /// the vault, the Loro store or an integration restores was dropped, and
    /// `kept` lists every other table with its row count. `cleared` names the
    /// state dropped although only the database held it.
    OwnedStateKept {
        backup: std::path::PathBuf,
        kept: Vec<(String, u64)>,
        cleared: Vec<&'static str>,
    },
    /// The copy of the moved file `backup` could not be rebuilt (`why`), so a
    /// fresh database took its place: every kind of state in `kinds` may be
    /// lost here, and is in `backup` if the file held it.
    OwnedStateLost {
        backup: std::path::PathBuf,
        kinds: Vec<&'static str>,
        why: String,
    },
    /// Another process held the file's lock. The open waited `waited_ms` for
    /// it and then got the file; nothing was changed.
    LockWaited { waited_ms: u64 },
}

impl std::fmt::Display for OpenChange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ViewsDropped { reason, views } => write!(
                f,
                "dropped the materialized views [{}] ({reason})",
                views.join(", ")
            ),
            Self::CoordinationFileMovedAside { reason, backup } => write!(
                f,
                "moved the coordination file aside to {} ({reason})",
                backup.display()
            ),
            Self::LogMovedAside { reason, backup } => write!(
                f,
                "moved the write-ahead log aside to {} ({reason})",
                backup.display()
            ),
            Self::MovedAside { reason, backup } => write!(
                f,
                "moved the database aside to {} ({reason})",
                backup.display()
            ),
            Self::OwnedStateKept {
                backup,
                kept,
                cleared,
            } => write!(
                f,
                "rebuilt a copy of {}, keeping the tables [{}] and clearing [{}]",
                backup.display(),
                kept.iter()
                    .map(|(table, rows)| format!("{table}: {rows} rows"))
                    .collect::<Vec<_>>()
                    .join(", "),
                cleared.join("; ")
            ),
            Self::OwnedStateLost { backup, kinds, why } => write!(
                f,
                "could not rebuild a copy of {} ({why}) and opened a fresh database; it may have \
                 held [{}]",
                backup.display(),
                kinds.join("; ")
            ),
            Self::LockWaited { waited_ms } => write!(
                f,
                "waited {:.1} s for the lock another process held",
                *waited_ms as f64 / 1000.0
            ),
        }
    }
}

/// An open that failed, and what it did to the files before it failed.
#[derive(Debug)]
pub struct OpenFailed {
    pub changed: OpenOutcome,
    pub error: StorageError,
}

impl std::fmt::Display for OpenFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.error)?;
        for change in &self.changed.0 {
            write!(f, "; before it failed, the open {change}")?;
        }
        Ok(())
    }
}

impl std::error::Error for OpenFailed {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

/// Keeps the class of the error when the open changed nothing; otherwise the
/// error text names every change.
impl From<OpenFailed> for StorageError {
    fn from(failed: OpenFailed) -> Self {
        match failed.changed.0.is_empty() {
            true => failed.error,
            false => StorageError::DatabaseError(failed.to_string()),
        }
    }
}

/// What this process's boot did to its database file. Provided once per
/// session; a boot that opens no database (`LoroMemory`) records
/// [`OpenOutcome::KEPT`].
#[derive(Debug, Clone)]
pub struct BootOpenOutcome(pub OpenOutcome);

const FN_SET_TABLE: &str = "holon_db_fn_set";
/// The engine format of the stored view state, recorded after the function set
/// in [`FN_SET_TABLE`]. A file recording another format is moved aside whole.
pub const VIEW_FORMAT: &str = "view-state/dbsp-3-probe-key";
/// Values computed with the functions; the derived-field reconciler refills it.
const DERIVED_CACHE_TABLE: &str = "block_derived";

/// How often an open tries a file another process holds, and how long it
/// waits between tries. Only a unix file lock can be held by another process.
const LOCK_TRIES: u32 = if cfg!(target_family = "unix") { 10 } else { 1 };
const LOCK_RETRY: std::time::Duration = std::time::Duration::from_millis(500);

/// What an open does when another process holds the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhenHeld {
    /// Try again for a bounded time, then fail.
    Wait,
    /// Fail at once with [`DatabaseOpenFailure::Locked`].
    Fail,
}

/// The opened database, and what this open did to the file.
pub(crate) fn open(
    db_path: &Path,
    fns: &[ScalarFn],
    when_held: WhenHeld,
) -> std::result::Result<(Arc<Database>, OpenOutcome), OpenFailed> {
    let lock_tries = match when_held {
        WhenHeld::Wait => LOCK_TRIES,
        WhenHeld::Fail => 1,
    };
    let mut changes = Vec::new();
    match open_recording(db_path, fns, lock_tries, &mut changes) {
        Ok(db) => Ok((db, OpenOutcome(changes))),
        Err(error) => Err(OpenFailed {
            changed: OpenOutcome(changes),
            error,
        }),
    }
}

/// Opens the database, pushing each change to the files onto `changes` as it
/// makes it, so a later failure still reports the earlier changes.
fn open_recording(
    db_path: &Path,
    fns: &[ScalarFn],
    lock_tries: u32,
    changes: &mut Vec<OpenChange>,
) -> Result<Arc<Database>> {
    let path = db_path
        .to_str()
        .ok_or_else(|| StorageError::DatabaseError(format!("path {db_path:?} is not UTF-8")))?;
    if path.starts_with(":memory:") {
        return open_with_io(Arc::new(MemoryIO::new()), path, OpenFlags::default(), fns)
            .map_err(|e| e.classify(path));
    }
    let mut stem = None;
    let mut tries = 0;
    // Each failed pass moves one more existing file aside or uses one of the
    // bounded lock tries, so the loop ends.
    loop {
        let Failed { leg, error, db } = match attempt(path, fns) {
            Ok((db, change)) => {
                if tries > 0 {
                    changes.push(OpenChange::LockWaited {
                        waited_ms: (LOCK_RETRY * tries).as_millis() as u64,
                    });
                }
                changes.extend(change);
                return Ok(db);
            }
            Err(failed) => failed,
        };
        if error.failure() == Some(DatabaseOpenFailure::Locked) {
            tries += 1;
            if tries < lock_tries {
                tracing::warn!(
                    "[open_database] {path}: another process holds it; try {tries} of \
                     {lock_tries}, next in {LOCK_RETRY:?}"
                );
                std::thread::sleep(LOCK_RETRY);
                continue;
            }
        }
        let foreign_format = matches!(error, OpenError::Format(_));
        if !foreign_format && !error.is_corrupt() {
            return Err(error.classify(path));
        }
        if let Some(db) = db {
            assert_eq!(
                Arc::strong_count(&db),
                1,
                "{path} is unusable ({error}), but another holder in this process still has it open"
            );
        }
        let stem: &std::path::Path = stem.get_or_insert_with(|| free_backup_stem(db_path, path));
        changes.push(match foreign_format {
            true => move_database_aside(error.to_string(), db_path, path, stem)?,
            false => recover_damaged(
                leg,
                format!("the engine cannot use it: {error}"),
                db_path,
                path,
                stem,
            )?,
        });
        if let Some(OpenChange::MovedAside { backup, .. }) = changes.last() {
            let backup = backup.clone();
            // A log moved aside earlier in this open is the damaged one.
            let with_log = !changes
                .iter()
                .any(|change| matches!(change, OpenChange::LogMovedAside { .. }));
            return match rebuild_on_copy(&backup, db_path, path, fns, with_log)? {
                Ok(Rebuilt { db, kept, cleared }) => {
                    tracing::warn!(
                        "[open_database] {path}: rebuilt a copy of {}; kept {kept:?}, cleared \
                         {cleared:?}",
                        backup.display()
                    );
                    changes.push(OpenChange::OwnedStateKept {
                        backup,
                        kept,
                        cleared,
                    });
                    Ok(db)
                }
                Err(why) => {
                    tracing::error!(
                        "[open_database] {path}: {why}. Opened a fresh database; the state only \
                         the database held is in {}",
                        backup.display()
                    );
                    changes.push(OpenChange::OwnedStateLost {
                        backup,
                        kinds: crate::table_classes::lost_kinds(),
                        why,
                    });
                    open_fresh(path, fns).map_err(|e| e.classify(path))
                }
            };
        }
    }
}

/// Whether another process holds the file at `db_path`. Takes the file's lock
/// and releases it, and changes nothing in the file.
pub fn is_held(db_path: &Path) -> Result<bool> {
    let path = db_path
        .to_str()
        .ok_or_else(|| StorageError::DatabaseError(format!("path {db_path:?} is not UTF-8")))?;
    match open_file(path, scalar_fns::ALL) {
        Ok(_) => Ok(false),
        Err(error) if error.failure() == Some(DatabaseOpenFailure::Locked) => Ok(true),
        Err(error) => Err(error.classify(path)),
    }
}

/// Lets a test make a database file read as held by another process, so every
/// open of it — the boot's and [`is_held`]'s — meets the engine's own lock
/// error. A process can never contend with itself for the real lock, so this
/// is the only way one process reaches the held path.
#[cfg(all(feature = "held-lock-seam", target_family = "unix"))]
pub mod held_lock_seam {
    use std::collections::BTreeSet;
    use std::path::Path;
    use std::path::PathBuf;
    use std::sync::Mutex;

    static HELD: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

    /// What asking for a file's lock answers.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum LockAnswer {
        HeldByAnotherProcess,
        /// The engine asks the file itself.
        Acquired,
    }

    /// Another process holds `db_path` until this is dropped.
    #[must_use = "the hold ends when this is dropped"]
    pub struct AnotherProcessHolds {
        path: PathBuf,
    }

    pub fn hold(db_path: &Path) -> AnotherProcessHolds {
        let path = db_path.to_path_buf();
        let fresh = HELD
            .lock()
            .expect("held-lock registry")
            .insert(path.clone());
        assert!(fresh, "{} is already held", path.display());
        AnotherProcessHolds { path }
    }

    impl Drop for AnotherProcessHolds {
        fn drop(&mut self) {
            HELD.lock().expect("held-lock registry").remove(&self.path);
        }
    }

    pub(super) fn answer(path: &str) -> LockAnswer {
        if HELD
            .lock()
            .expect("held-lock registry")
            .contains(Path::new(path))
        {
            LockAnswer::HeldByAnotherProcess
        } else {
            LockAnswer::Acquired
        }
    }
}

/// Where an engine error arose during an open.
#[derive(Clone, Copy)]
enum Leg {
    Open,
    /// The first connect, which reads the write-ahead log.
    Connect,
    /// A read or write of the opened database.
    Use,
}

struct Failed {
    leg: Leg,
    error: OpenError,
    db: Option<Arc<Database>>,
}

/// An error inside an open. An engine error keeps its `LimboError`, so `open`
/// classifies the errors of every leg in one place.
#[derive(Debug)]
enum OpenError {
    Engine {
        doing: String,
        error: LimboError,
    },
    /// The file is in an engine format this binary does not maintain.
    Format(String),
    Other(StorageError),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Engine { doing, error } => write!(f, "{doing}: {error}"),
            Self::Format(reason) => write!(f, "{reason}"),
            Self::Other(e) => write!(f, "{e}"),
        }
    }
}

impl OpenError {
    fn failure(&self) -> Option<DatabaseOpenFailure> {
        match self {
            Self::Engine { error, .. } => Some(open_failure(error)),
            Self::Format(_) | Self::Other(_) => None,
        }
    }

    fn is_corrupt(&self) -> bool {
        self.failure() == Some(DatabaseOpenFailure::Corrupt)
    }

    fn classify(self, path: &str) -> StorageError {
        match self {
            Self::Engine { ref error, .. } => StorageError::DatabaseUnopenable {
                path: path.to_string(),
                failure: open_failure(error),
                cause: self.to_string(),
            },
            Self::Format(reason) => {
                StorageError::DatabaseError(format!("the database at {path}: {reason}"))
            }
            Self::Other(e) => e,
        }
    }
}

type Step<T> = std::result::Result<T, OpenError>;

fn engine(doing: impl std::fmt::Display) -> impl FnOnce(LimboError) -> OpenError {
    move |error| OpenError::Engine {
        doing: doing.to_string(),
        error,
    }
}

fn invalid(message: String) -> OpenError {
    OpenError::Other(StorageError::DatabaseError(message))
}

/// One try to open `path` and make it usable by this binary.
fn attempt(
    path: &str,
    fns: &[ScalarFn],
) -> std::result::Result<(Arc<Database>, Option<OpenChange>), Failed> {
    let db = open_file(path, fns).map_err(|error| Failed {
        leg: Leg::Open,
        error,
        db: None,
    })?;
    // The engine reads the write-ahead log at the first connect, so a damaged
    // log surfaces there.
    if let Err(error) = log_header_is_intact(path) {
        return Err(Failed {
            leg: Leg::Connect,
            error,
            db: Some(db),
        });
    }
    if let Err(error) = db.connect(ConnectOpts::new()) {
        return Err(Failed {
            leg: Leg::Connect,
            error: engine("connect")(error),
            db: Some(db),
        });
    }
    match admit(&db, path, fns) {
        Ok(change) => Ok((db, change)),
        Err(error) => Err(Failed {
            leg: Leg::Use,
            error,
            db: Some(db),
        }),
    }
}

/// Records this binary's function set in `db`, drops what `db` computed with
/// another one, or refuses a file in another engine format.
fn admit(db: &Arc<Database>, path: &str, fns: &[ScalarFn]) -> Step<Option<OpenChange>> {
    let reason = match verdict(db, fns)? {
        Verdict::Usable => {
            record_signature(db, fns)?;
            return Ok(None);
        }
        Verdict::MoveAside(reason) => return Err(OpenError::Format(reason)),
        Verdict::Recompute(reason) => reason,
    };
    let views = drop_computed(db, fns)?;
    tracing::info!(
        "[open_database] {path}: {reason}. Dropped its materialized views [{}] and the \
         {DERIVED_CACHE_TABLE} cache; every table row is kept, and the views are rebuilt from \
         the tables",
        views.join(", ")
    );
    Ok(Some(OpenChange::ViewsDropped { reason, views }))
}

#[cfg(target_family = "unix")]
fn open_file(path: &str, fns: &[ScalarFn]) -> Step<Arc<Database>> {
    #[cfg(feature = "held-lock-seam")]
    if held_lock_seam::answer(path) == held_lock_seam::LockAnswer::HeldByAnotherProcess {
        return Err(engine("open")(LimboError::LockingError(format!(
            "Failed locking file '{path}'. File is locked by another process"
        ))));
    }
    let io = turso_core::UnixIO::new().map_err(engine("create the file IO"))?;
    open_with_io(Arc::new(io), path, OpenFlags::default(), fns)
}

/// wasm32: a file path needs the host IO that the browser worker registers
/// (the OPFS shim) before the engine opens a database.
#[cfg(all(not(target_family = "unix"), target_family = "wasm"))]
fn open_file(path: &str, fns: &[ScalarFn]) -> Step<Arc<Database>> {
    let io = crate::turso::wasm_io::registered().ok_or_else(|| {
        invalid(format!(
            "open_database('{path}'): no wasm IO registered — call \
             holon_turso::register_wasm_io (e.g. with the OPFS shim) before opening a \
             file-backed database on wasm32"
        ))
    })?;
    open_with_io(io, path, OpenFlags::Create, fns)
}

#[cfg(all(not(target_family = "unix"), not(target_family = "wasm")))]
fn open_file(_: &str, _: &[ScalarFn]) -> Step<Arc<Database>> {
    Err(invalid(
        "File-based storage not yet supported on this platform".to_string(),
    ))
}

/// Moves aside the files a `Corrupt` error can point at. The engine does not
/// say which file is damaged, so the smallest move that can explain the error
/// goes first: the coordination file, which holds no rows; then the log, if
/// the first connect raised the error; then every file of the database.
fn recover_damaged(
    leg: Leg,
    reason: String,
    db_path: &Path,
    path: &str,
    stem: &Path,
) -> Result<OpenChange> {
    let coordination = sidecar(path, COORDINATION_SUFFIX);
    if coordination.exists() {
        let backup = move_aside(&coordination, path, stem)?;
        tracing::warn!(
            "[open_database] {path}: {reason}. Moved its shared-WAL coordination file aside to \
             {}; it holds no rows",
            backup.display()
        );
        return Ok(OpenChange::CoordinationFileMovedAside { reason, backup });
    }
    let log = sidecar(path, LOG_SUFFIX);
    if matches!(leg, Leg::Connect) && log.exists() {
        let backup = move_aside(&log, path, stem)?;
        tracing::error!(
            "[open_database] {path}: {reason}. Moved its write-ahead log aside to {}; commits \
             that were only in the log are in that file",
            backup.display()
        );
        return Ok(OpenChange::LogMovedAside { reason, backup });
    }
    move_database_aside(reason, db_path, path, stem)
}

/// Moves every file of the database aside.
fn move_database_aside(
    reason: String,
    db_path: &Path,
    path: &str,
    stem: &Path,
) -> Result<OpenChange> {
    for file in TursoDurableState::new(db_path).durable_paths() {
        if file.exists() {
            move_aside(&file, path, stem)?;
        }
    }
    tracing::error!(
        "[open_database] {path} is unusable: {reason}. Moved it aside to {}; the moved file \
         keeps everything the database held",
        stem.display()
    );
    Ok(OpenChange::MovedAside {
        reason,
        backup: stem.to_path_buf(),
    })
}

/// The engine reads a log whose header fails its checksum as empty and
/// overwrites it at the next write, so the open refuses such a log first.
#[cfg(target_family = "unix")]
fn log_header_is_intact(path: &str) -> Step<()> {
    use std::io::Read;

    use turso_core::storage::sqlite3_ondisk::WAL_HEADER_SIZE;
    use turso_core::storage::sqlite3_ondisk::WAL_MAGIC_BE;
    use turso_core::storage::sqlite3_ondisk::WAL_MAGIC_LE;
    use turso_core::storage::sqlite3_ondisk::WalHeader;
    use turso_core::storage::sqlite3_ondisk::checksum_wal;

    let log = sidecar(path, LOG_SUFFIX);
    let unreadable = |e: std::io::Error| invalid(format!("read the log {}: {e}", log.display()));
    let mut file = match std::fs::File::open(&log) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(unreadable(e)),
    };
    if file.metadata().map_err(unreadable)?.len() < WAL_HEADER_SIZE as u64 {
        return Ok(());
    }
    let mut bytes = [0u8; WAL_HEADER_SIZE];
    file.read_exact(&mut bytes).map_err(unreadable)?;
    let word = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
    let header = WalHeader {
        magic: word(0),
        file_format: word(4),
        page_size: word(8),
        checkpoint_seq: word(12),
        salt_1: word(16),
        salt_2: word(20),
        checksum_1: word(24),
        checksum_2: word(28),
    };
    let native = cfg!(target_endian = "big") == (header.magic & 1 != 0);
    if matches!(header.magic, WAL_MAGIC_LE | WAL_MAGIC_BE)
        && checksum_wal(&bytes[..24], &header, (0, 0), native)
            == (header.checksum_1, header.checksum_2)
    {
        return Ok(());
    }
    Err(engine("read the log header")(LimboError::Corrupt(format!(
        "{} does not start with a valid write-ahead log header",
        log.display()
    ))))
}

#[cfg(not(target_family = "unix"))]
fn log_header_is_intact(_: &str) -> Step<()> {
    Ok(())
}

const LOG_SUFFIX: &str = "-wal";
const COORDINATION_SUFFIX: &str = "-tshm";

fn sidecar(path: &str, suffix: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("{path}{suffix}"))
}

/// The suffix `file`, a file of the database at `path`, adds to `path`.
fn suffix_of<'a>(file: &'a Path, path: &str) -> &'a str {
    file.to_str()
        .and_then(|f| f.strip_prefix(path))
        .unwrap_or_else(|| panic!("{} is not a file of {path}", file.display()))
}

/// The base name the files of one recovery move to: `{path}.unusable-{now}`,
/// with `.2`, `.3`, … appended while a file of the database under that name
/// exists already.
fn free_backup_stem(db_path: &Path, path: &str) -> std::path::PathBuf {
    let base = format!(
        "{path}.unusable-{}",
        chrono::Utc::now().format("%Y%m%dT%H%M%S%.3fZ")
    );
    let files = TursoDurableState::new(db_path).durable_paths();
    (1..)
        .map(|n| match n {
            1 => base.clone(),
            n => format!("{base}.{n}"),
        })
        .find(|stem| {
            files
                .iter()
                .all(|file| !Path::new(&format!("{stem}{}", suffix_of(file, path))).exists())
        })
        .map(std::path::PathBuf::from)
        .expect("the range of names is unbounded")
}

/// Renames `file`, a file of the database at `path`, to `stem` plus its
/// suffix. Returns the new path.
#[cfg(target_family = "unix")]
fn move_aside(file: &Path, path: &str, stem: &Path) -> Result<std::path::PathBuf> {
    let to = std::path::PathBuf::from(format!("{}{}", stem.display(), suffix_of(file, path)));
    if to.exists() {
        return Err(StorageError::DatabaseError(format!(
            "move the damaged database file {} aside: {} appeared after the name was chosen; \
             nothing was overwritten",
            file.display(),
            to.display()
        )));
    }
    std::fs::rename(file, &to).map_err(|e| {
        StorageError::DatabaseError(format!(
            "move the damaged database file {} aside to {}: {e}",
            file.display(),
            to.display()
        ))
    })?;
    Ok(to)
}

/// OPFS files cannot be renamed through the engine's IO.
#[cfg(not(target_family = "unix"))]
fn move_aside(file: &Path, path: &str, _: &Path) -> Result<std::path::PathBuf> {
    Err(StorageError::DatabaseError(format!(
        "the database at {path} is damaged ({}). Copy it out of the browser storage and delete \
         it there; the org files rebuild the blocks",
        file.display()
    )))
}

/// A database created where a damaged one was moved aside.
fn open_fresh(path: &str, fns: &[ScalarFn]) -> Step<Arc<Database>> {
    let db = open_file(path, fns)?;
    match verdict(&db, fns)? {
        Verdict::Usable => {}
        Verdict::Recompute(fresh) | Verdict::MoveAside(fresh) => {
            return Err(invalid(format!(
                "the fresh database at {path} is unusable right after it was created: {fresh}"
            )));
        }
    }
    record_signature(&db, fns)?;
    Ok(db)
}

/// In one transaction: drop every materialized view and every view the engine
/// could not load, dependents first; empty the derived cache; record `fns`.
/// Returns the dropped view names.
fn drop_computed(db: &Arc<Database>, fns: &[ScalarFn]) -> Step<Vec<String>> {
    let conn = connect(db)?;
    let views = computed_views(&conn)?;
    in_write_tx(&conn, || {
        let dropped = drop_views(&conn, views)?;
        if has_table(&conn, DERIVED_CACHE_TABLE)? {
            let sql = format!("DELETE FROM {DERIVED_CACHE_TABLE}");
            conn.execute(&sql).map_err(engine(&sql))?;
        }
        write_signature(&conn, fns)?;
        Ok(dropped)
    })
}

/// Every materialized view and every view the engine could not load.
fn computed_views(conn: &Arc<Connection>) -> Step<Vec<(String, String)>> {
    let unloadable: Vec<String> = unloadable_views(conn)?
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    Ok(view_rows(conn)?
        .into_iter()
        .filter(|(name, sql)| is_materialized(sql) || unloadable.contains(name))
        .collect())
}

/// Drops `views`, dependents first, inside the caller's transaction. Returns
/// the dropped names.
fn drop_views(conn: &Arc<Connection>, mut views: Vec<(String, String)>) -> Step<Vec<String>> {
    let mut dropped = Vec::new();
    while !views.is_empty() {
        let (free, held): (Vec<_>, Vec<_>) = views.iter().cloned().partition(|(name, _)| {
            !views
                .iter()
                .any(|(other, sql)| other != name && names_word(sql, name))
        });
        if free.is_empty() {
            return Err(invalid(format!(
                "the views [{}] read each other, so none can be dropped first",
                held.iter()
                    .map(|(n, _)| n.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        for (name, _) in free {
            let sql = format!("DROP VIEW \"{name}\"");
            conn.execute(&sql).map_err(engine(&sql))?;
            dropped.push(name);
        }
        views = held;
    }
    Ok(dropped)
}

/// The rebuilt copy of a moved-aside database.
struct Rebuilt {
    db: Arc<Database>,
    kept: Vec<(String, u64)>,
    cleared: Vec<&'static str>,
}

/// Copies the moved-aside files at `stem` (the log only `with_log`) to
/// `path`, then rebuilds the copy: see [`rebuild`]. The backup is only read.
/// `Ok(Err(why))`: the copy was not rebuilt and every file of it is removed,
/// so a fresh database can take its place. `Err`: a file of the copy could not
/// be removed.
fn rebuild_on_copy(
    stem: &Path,
    db_path: &Path,
    path: &str,
    fns: &[ScalarFn],
    with_log: bool,
) -> Result<std::result::Result<Rebuilt, String>> {
    let failed = match copy_backup(stem, path, with_log).and_then(|()| rebuild(path, fns)) {
        Ok(rebuilt) => return Ok(Ok(rebuilt)),
        Err(error) => error,
    };
    for file in TursoDurableState::new(db_path).durable_paths() {
        if file.exists() {
            std::fs::remove_file(&file).map_err(|e| {
                StorageError::DatabaseError(format!(
                    "rebuild a copy of {} at {path}: {failed}; then removing the copy's file {} \
                     failed: {e}",
                    stem.display(),
                    file.display()
                ))
            })?;
        }
    }
    Ok(Err(format!(
        "rebuild a copy of {} at {path}: {failed}",
        stem.display()
    )))
}

fn copy_backup(stem: &Path, path: &str, with_log: bool) -> Step<()> {
    let suffixes: &[&str] = match with_log {
        true => &["", LOG_SUFFIX],
        false => &[""],
    };
    for suffix in suffixes {
        let from = std::path::PathBuf::from(format!("{}{suffix}", stem.display()));
        if from.exists() {
            let to = sidecar(path, suffix);
            std::fs::copy(&from, &to).map_err(|e| {
                invalid(format!("copy {} to {}: {e}", from.display(), to.display()))
            })?;
        }
    }
    #[cfg(test)]
    rebuild_fault::at(rebuild_fault::Cut::Copied)?;
    Ok(())
}

/// In one transaction on the copy at `path`: drops its computed views, then
/// every table [`rebuild_action`](crate::table_classes::rebuild_action) drops,
/// and records `fns`. The copy must then be usable as it is.
fn rebuild(path: &str, fns: &[ScalarFn]) -> Step<Rebuilt> {
    use crate::table_classes::Class;
    use crate::table_classes::RebuildAction;

    let db = open_file(path, fns)?;
    let conn = connect(&db)?;
    // `advice_suppressed` cascades from `block_raw`, which the rebuild drops.
    conn.execute("PRAGMA foreign_keys = OFF")
        .map_err(engine("PRAGMA foreign_keys = OFF"))?;
    #[cfg(test)]
    rebuild_fault::at(rebuild_fault::Cut::Opened)?;
    let views = computed_views(&conn)?;
    let (kept, cleared) = in_write_tx(&conn, || {
        let caches: std::collections::HashMap<String, String> =
            match has_table(&conn, "integration_cache")? {
                true => text_pairs(&conn, "SELECT table_name, provider FROM integration_cache")?
                    .into_iter()
                    .collect(),
                false => Default::default(),
            };
        drop_views(&conn, views)?;
        let mut kept = Vec::new();
        let mut cleared = Vec::new();
        for table in user_tables(&conn)? {
            match crate::table_classes::rebuild_action(&table, &caches) {
                RebuildAction::Keep => kept.push(table),
                RebuildAction::Drop => {
                    if let Some(Class::Lost(what)) = crate::table_classes::class_of(&table, &caches)
                    {
                        cleared.push(what);
                    }
                    let sql = format!("DROP TABLE \"{table}\"");
                    conn.execute(&sql).map_err(engine(&sql))?;
                }
            }
        }
        #[cfg(test)]
        rebuild_fault::at(rebuild_fault::Cut::Dropped)?;
        write_signature(&conn, fns)?;
        Ok((kept, cleared))
    })?;
    #[cfg(test)]
    rebuild_fault::at(rebuild_fault::Cut::Committed)?;
    match verdict(&db, fns)? {
        Verdict::Usable => {}
        Verdict::Recompute(why) | Verdict::MoveAside(why) => {
            return Err(invalid(format!(
                "the rebuilt copy is still unusable: {why}"
            )));
        }
    }
    let kept = kept
        .into_iter()
        .map(|table| {
            let sql = format!("SELECT count(*) FROM \"{table}\"");
            match rows(&conn, &sql)?.as_slice() {
                [row] => match row.as_slice() {
                    [Value::Numeric(turso_core::Numeric::Integer(n))] => Ok((table, *n as u64)),
                    other => Err(invalid(format!("{sql} gave {other:?}"))),
                },
                other => Err(invalid(format!("{sql} gave {other:?}"))),
            }
        })
        .collect::<Step<Vec<_>>>()?;
    drop(conn);
    Ok(Rebuilt { db, kept, cleared })
}

/// The tables of the schema, without the engine's own.
fn user_tables(conn: &Arc<Connection>) -> Step<Vec<String>> {
    Ok(text_pairs(
        conn,
        "SELECT name, name FROM sqlite_schema WHERE type = 'table' ORDER BY name",
    )?
    .into_iter()
    .map(|(name, _)| name)
    .filter(|name| !name.starts_with("sqlite_") && !name.starts_with("__turso_internal_"))
    .collect())
}

fn text_pairs(conn: &Arc<Connection>, sql: &str) -> Step<Vec<(String, String)>> {
    rows(conn, sql)?
        .into_iter()
        .map(|row| match row.as_slice() {
            [Value::Text(a), Value::Text(b)] => {
                Ok((a.as_str().to_string(), b.as_str().to_string()))
            }
            other => Err(invalid(format!("{sql}: a row is {other:?}"))),
        })
        .collect()
}

/// Lets a test fail or crash a rebuild at one point.
#[cfg(test)]
mod rebuild_fault {
    use std::sync::Mutex;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Cut {
        Copied,
        Opened,
        /// Inside the transaction, before the signature is written.
        Dropped,
        Committed,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Fault {
        Error,
        Crash,
    }

    pub static PLANTED: Mutex<Option<(Cut, Fault)>> = Mutex::new(None);

    pub fn at(cut: Cut) -> super::Step<()> {
        let planted = *PLANTED.lock().expect("rebuild fault");
        match planted {
            Some((at, Fault::Error)) if at == cut => {
                Err(super::invalid(format!("injected failure after {cut:?}")))
            }
            Some((at, Fault::Crash)) if at == cut => panic!("injected crash after {cut:?}"),
            _ => Ok(()),
        }
    }
}

/// Runs `body` in one write transaction. The engine keeps a transaction open
/// after a failed statement, so a failed body or COMMIT is rolled back here.
fn in_write_tx<T>(conn: &Arc<Connection>, body: impl FnOnce() -> Step<T>) -> Step<T> {
    conn.execute("BEGIN IMMEDIATE")
        .map_err(engine("BEGIN IMMEDIATE"))?;
    let result = body().and_then(|value| {
        conn.execute("COMMIT").map_err(engine("COMMIT"))?;
        Ok(value)
    });
    match result {
        Err(error) if !conn.get_auto_commit() => match conn.execute("ROLLBACK") {
            Ok(()) => Err(error),
            Err(rollback) => Err(invalid(format!(
                "{error}; the ROLLBACK after it failed too: {rollback}"
            ))),
        },
        result => result,
    }
}

fn is_materialized(sql: &str) -> bool {
    sql.split_whitespace()
        .take(3)
        .map(str::to_ascii_uppercase)
        .eq(["CREATE", "MATERIALIZED", "VIEW"])
}

/// Whether `sql` names `name` as a whole identifier.
fn names_word(sql: &str, name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    sql.to_ascii_lowercase()
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .any(|word| word == name)
}

fn view_rows(conn: &Arc<Connection>) -> Step<Vec<(String, String)>> {
    rows(
        conn,
        "SELECT name, sql FROM sqlite_schema WHERE type = 'view' ORDER BY name",
    )?
    .into_iter()
    .map(|row| match row.as_slice() {
        [Value::Text(name), Value::Text(sql)] => {
            Ok((name.as_str().to_string(), sql.as_str().to_string()))
        }
        other => Err(invalid(format!("sqlite_schema view row is {other:?}"))),
    })
    .collect()
}

/// The views whose stored state is in a version this engine does not maintain.
fn incompatible_views(conn: &Arc<Connection>) -> Step<Vec<String>> {
    let views = view_rows(conn)?;
    conn.with_schema_mut(|schema| {
        views
            .into_iter()
            .map(|(name, _)| name)
            .filter(|name| schema.is_incompatible_view(NamespaceId::ROOT, name))
            .collect()
    })
    .map_err(engine("read the schema"))
}

/// The views the engine could not load, each with why. The engine loads a
/// materialized view whose SELECT names a column its table no longer has, and
/// then refuses every write to that table, so such a view counts as unloadable.
fn unloadable_views(conn: &Arc<Connection>) -> Step<Vec<(String, String)>> {
    let views = view_rows(conn)?;
    let flagged: Vec<(String, String, Option<&'static str>)> = conn
        .with_schema_mut(|schema| {
            views
                .into_iter()
                .map(|(name, sql)| {
                    let why = schema
                        .is_broken_view(NamespaceId::ROOT, &name)
                        .then_some("its SQL does not parse");
                    (name, sql, why)
                })
                .collect()
        })
        .map_err(engine("read the schema"))?;
    Ok(flagged
        .into_iter()
        .filter_map(|(name, sql, why)| match why {
            Some(why) => Some((name, why.to_string())),
            None if is_materialized(&sql) => match view_select(&sql) {
                Some(select) => conn
                    .prepare(select)
                    .err()
                    .map(|e| (name, format!("its SELECT no longer compiles: {e}"))),
                None => Some((name, "its SQL has no `AS <select>`".to_string())),
            },
            None => None,
        })
        .collect())
}

/// The SELECT of a stored `CREATE MATERIALIZED VIEW <name> AS <select>`.
fn view_select(sql: &str) -> Option<&str> {
    let b = sql.as_bytes();
    (1..b.len().saturating_sub(2))
        .find(|&i| {
            b[i - 1].is_ascii_whitespace()
                && b[i..i + 2].eq_ignore_ascii_case(b"AS")
                && b[i + 2].is_ascii_whitespace()
        })
        .map(|i| &sql[i + 3..])
}

fn has_table(conn: &Arc<Connection>, table: &str) -> Step<bool> {
    Ok(!rows(
        conn,
        &format!("SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = '{table}'"),
    )?
    .is_empty())
}

fn open_with_io(
    io: Arc<dyn IO>,
    path: &str,
    flags: OpenFlags,
    fns: &[ScalarFn],
) -> Step<Arc<Database>> {
    let opts = DatabaseOpts::default().with_views(true);
    // The Tantivy `fts` and sparse-vector `CREATE INDEX .. USING` methods exist
    // only in the native turso_core build.
    #[cfg(target_family = "unix")]
    let opts = opts.with_index_method(true);
    let options = OpenOptions::new(Arc::new(turso_sqlite::SqliteDialect))
        .flags(flags)
        .db_opts(opts);
    Database::open(io, path, scalar_fns::register_on_options(options, fns)).map_err(engine("open"))
}

fn open_failure(e: &LimboError) -> DatabaseOpenFailure {
    match e {
        LimboError::Corrupt(_) | LimboError::NotADB => DatabaseOpenFailure::Corrupt,
        LimboError::CompletionError(
            CompletionError::ShortRead { .. }
            | CompletionError::ShortReadWalFrame { .. }
            | CompletionError::WalFramePageMismatch { .. }
            | CompletionError::ChecksumMismatch { .. },
        ) => DatabaseOpenFailure::Corrupt,
        // The engine names contention only in its text: a lock another process
        // holds (`core/io/unix.rs`), or a database another process has open
        // in the other WAL mode (`core/database.rs`).
        LimboError::LockingError(message)
            if message.ends_with("locked by another process")
                || message.ends_with("in another process") =>
        {
            DatabaseOpenFailure::Locked
        }
        LimboError::CompletionError(CompletionError::IOError(kind, _)) => {
            DatabaseOpenFailure::Io(*kind)
        }
        #[cfg(target_family = "unix")]
        LimboError::CompletionError(CompletionError::RustixIOError(errno)) => {
            DatabaseOpenFailure::Io(std::io::Error::from(*errno).kind())
        }
        LimboError::DatabaseFull => DatabaseOpenFailure::Io(std::io::ErrorKind::StorageFull),
        _ => DatabaseOpenFailure::Unclassified,
    }
}

/// What an open does with a file it opened.
enum Verdict {
    Usable,
    /// The file is in this engine's format, but its views were computed with
    /// other functions or no longer load: they are dropped and rebuilt.
    Recompute(String),
    /// The file is in another or an unknown engine format.
    MoveAside(String),
}

/// Reads the recorded engine format first, so a file in another format is
/// refused before any statement writes to it.
fn verdict(db: &Arc<Database>, fns: &[ScalarFn]) -> Step<Verdict> {
    let conn = connect(db)?;
    let stored = match stored_signature(&conn)? {
        StoredSignature::Recorded(stored) => stored,
        StoredSignature::Absent if is_empty(&conn)? => return Ok(Verdict::Usable),
        StoredSignature::Absent => {
            return Ok(Verdict::MoveAside(format!(
                "it holds tables but no {FN_SET_TABLE} record, so its engine format is \
                 unknown; this engine maintains only [{VIEW_FORMAT}]"
            )));
        }
        StoredSignature::Unreadable(why) => {
            return Ok(Verdict::MoveAside(format!(
                "its engine format record is unreadable ({why}); this engine maintains only \
                 [{VIEW_FORMAT}]"
            )));
        }
    };
    let Some((stored_fns, format)) = stored.rsplit_once(';') else {
        return Ok(Verdict::MoveAside(format!(
            "its record [{stored}] names no engine format; this engine maintains only \
             [{VIEW_FORMAT}]"
        )));
    };
    if format != VIEW_FORMAT {
        return Ok(Verdict::MoveAside(format!(
            "its views are stored in the engine format [{format}]; this engine maintains only \
             [{VIEW_FORMAT}]"
        )));
    }
    let incompatible = incompatible_views(&conn)?;
    if !incompatible.is_empty() {
        return Ok(Verdict::MoveAside(format!(
            "it records the engine format [{format}], but the engine holds the views [{}] in a \
             storage version it does not maintain",
            incompatible.join(", ")
        )));
    }
    let wanted = scalar_fns::signature(fns);
    if stored_fns != wanted {
        return Ok(Verdict::Recompute(format!(
            "it was built with the scalar functions [{stored_fns}], this binary registers [{wanted}]"
        )));
    }
    let unusable: Vec<String> = unloadable_views(&conn)?
        .into_iter()
        .map(|(name, why)| format!("{name} ({why})"))
        .collect();
    if !unusable.is_empty() {
        return Ok(Verdict::Recompute(format!(
            "the engine cannot load the materialized views {}",
            unusable.join(", ")
        )));
    }
    Ok(Verdict::Usable)
}

enum StoredSignature {
    Absent,
    Recorded(String),
    /// A torn or foreign record: the file's engine format is unknown.
    Unreadable(String),
}

fn stored_signature(conn: &Arc<Connection>) -> Step<StoredSignature> {
    if !has_table(conn, FN_SET_TABLE)? {
        return Ok(StoredSignature::Absent);
    }
    let stored = rows(conn, &format!("SELECT signature FROM {FN_SET_TABLE}"))?;
    Ok(match stored.as_slice() {
        [row] => match row.as_slice() {
            [Value::Text(signature)] => StoredSignature::Recorded(signature.as_str().to_string()),
            other => StoredSignature::Unreadable(format!("{FN_SET_TABLE} holds {other:?}")),
        },
        other => StoredSignature::Unreadable(format!("{FN_SET_TABLE} holds {} rows", other.len())),
    })
}

#[cfg(test)]
static CRASH_AFTER_DELETE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// One transaction: a crash inside it leaves the previous record.
fn record_signature(db: &Arc<Database>, fns: &[ScalarFn]) -> Step<()> {
    let conn = connect(db)?;
    if matches!(stored_signature(&conn)?, StoredSignature::Recorded(stored) if stored == record(fns))
    {
        return Ok(());
    }
    in_write_tx(&conn, || write_signature(&conn, fns))
}

/// What a file built by `fns` on this engine holds in [`FN_SET_TABLE`].
fn record(fns: &[ScalarFn]) -> String {
    format!("{};{VIEW_FORMAT}", scalar_fns::signature(fns))
}

/// Inside the caller's transaction.
fn write_signature(conn: &Arc<Connection>, fns: &[ScalarFn]) -> Step<()> {
    let run = |sql: &str| conn.execute(sql).map_err(engine(sql));
    run(&format!(
        "CREATE TABLE IF NOT EXISTS {FN_SET_TABLE} (signature TEXT NOT NULL)"
    ))?;
    run(&format!("DELETE FROM {FN_SET_TABLE}"))?;
    #[cfg(test)]
    if CRASH_AFTER_DELETE.load(std::sync::atomic::Ordering::SeqCst) {
        panic!("injected crash between DELETE and INSERT");
    }
    let insert = format!("INSERT INTO {FN_SET_TABLE} (signature) VALUES (?1)");
    let mut stmt = conn.prepare(&insert).map_err(engine(&insert))?;
    stmt.bind_at(
        1.try_into().expect("1 is non-zero"),
        Value::build_text(record(fns)),
    )
    .map_err(engine(format!("{insert}: bind")))?;
    stmt.run_ignore_rows().map_err(engine(&insert))
}

fn is_empty(conn: &Arc<Connection>) -> Step<bool> {
    Ok(rows(conn, "SELECT 1 FROM sqlite_schema LIMIT 1")?.is_empty())
}

fn connect(db: &Arc<Database>) -> Step<Arc<Connection>> {
    db.connect(ConnectOpts::new()).map_err(engine("connect"))
}

fn rows(conn: &Arc<Connection>, sql: &str) -> Step<Vec<Vec<Value>>> {
    conn.prepare(sql)
        .and_then(|mut stmt| stmt.run_collect_rows())
        .map_err(engine(sql))
}

#[cfg(test)]
mod tests {
    use turso_core::Numeric;

    use super::*;
    use crate::turso::TursoBackend;

    fn double(args: &[Value]) -> std::result::Result<Value, String> {
        match args {
            [Value::Numeric(Numeric::Integer(n))] => Ok(Value::Numeric(Numeric::Integer(n * 2))),
            other => Err(format!(
                "holon_test_double takes one integer, got {other:?}"
            )),
        }
    }

    const DOUBLE: ScalarFn = ScalarFn {
        name: "holon_test_double",
        arg_count: 1,
        version: 1,
        func: double,
    };

    fn exec(db: &Arc<Database>, sql: &str) -> Step<()> {
        connect(db)?.execute(sql).map_err(engine(sql))
    }

    fn count(db: &Arc<Database>, sql: &str) -> i64 {
        let conn = connect(db).expect("connect");
        match rows(&conn, sql).expect("count").as_slice() {
            [row] => match row.as_slice() {
                [Value::Numeric(Numeric::Integer(n))] => *n,
                other => panic!("{sql} gave {other:?}"),
            },
            other => panic!("{sql} gave {other:?}"),
        }
    }

    /// A table `t` with two rows and a view `v` that calls `DOUBLE`.
    fn build_with_double(path: &Path) {
        let (db, _) = open(path, &[DOUBLE], WhenHeld::Wait).expect("open with the test function");
        exec(&db, "CREATE TABLE t (id INTEGER PRIMARY KEY, n INTEGER)").unwrap();
        exec(&db, "INSERT INTO t (n) VALUES (1), (2)").unwrap();
        exec(
            &db,
            "CREATE MATERIALIZED VIEW v AS SELECT id, holon_test_double(n) AS d FROM t",
        )
        .unwrap();
        assert_eq!(count(&db, "SELECT sum(d) FROM v"), 6);
    }

    fn has(db: &Arc<Database>, kind: &str, name: &str) -> bool {
        count(
            db,
            &format!(
                "SELECT count(*) FROM sqlite_schema WHERE type = '{kind}' AND name = '{name}'"
            ),
        ) == 1
    }

    #[test]
    fn a_database_built_with_another_function_set_keeps_its_rows_and_drops_its_views() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("holon.db");
        build_with_double(&path);

        let db = TursoBackend::open_database(&path).expect("open with this binary's set");
        assert!(
            !has(&db, "view", "v"),
            "a view built over a function this binary does not register must be dropped"
        );
        exec(&db, "INSERT INTO t (n) VALUES (3)")
            .expect("with the view gone, writes to its source table must be accepted");
        assert_eq!(
            count(&db, "SELECT count(*) FROM t"),
            3,
            "a function-set change must keep every table row"
        );
    }

    #[test]
    fn a_database_built_with_the_same_function_set_keeps_its_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("holon.db");
        build_with_double(&path);

        let (db, _) = open(&path, &[DOUBLE], WhenHeld::Wait).expect("reopen with the same set");
        assert_eq!(count(&db, "SELECT sum(d) FROM v"), 6);
        exec(&db, "INSERT INTO t (n) VALUES (3)").expect("the view is usable");
        assert_eq!(count(&db, "SELECT sum(d) FROM v"), 12);
    }

    #[test]
    fn a_database_without_a_recorded_set_keeps_its_rows_under_the_empty_set() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("holon.db");
        {
            let db = open(&path, &[], WhenHeld::Wait).unwrap().0;
            exec(&db, "CREATE TABLE t (id INTEGER PRIMARY KEY)").unwrap();
            exec(&db, "INSERT INTO t (id) VALUES (1)").unwrap();
        }
        let db = open(&path, &[], WhenHeld::Wait).unwrap().0;
        assert_eq!(count(&db, "SELECT count(*) FROM t"), 1);
        let conn = connect(&db).unwrap();
        assert!(matches!(
            stored_signature(&conn).unwrap(),
            StoredSignature::Recorded(s) if s == record(&[])
        ));
    }

    #[test]
    fn a_database_opened_by_a_larger_set_keeps_its_tables_and_records_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("holon.db");
        {
            let db = open(&path, &[], WhenHeld::Wait).unwrap().0;
            exec(&db, "CREATE TABLE t (id INTEGER PRIMARY KEY)").unwrap();
            exec(&db, "INSERT INTO t (id) VALUES (1)").unwrap();
        }
        let db = open(&path, &[DOUBLE], WhenHeld::Wait).unwrap().0;
        assert_eq!(
            count(&db, "SELECT count(*) FROM t"),
            1,
            "a larger function set must keep every table row"
        );
        let conn = connect(&db).unwrap();
        assert!(matches!(
            stored_signature(&conn).unwrap(),
            StoredSignature::Recorded(s) if s == record(&[DOUBLE])
        ));
    }

    /// A crash after the views are dropped but before the record is written
    /// rolls the whole transaction back; the next open drops them again.
    #[test]
    fn a_crash_while_dropping_the_views_leaves_them_for_the_next_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("holon.db");
        build_with_double(&path);
        let double_v2 = ScalarFn {
            version: 2,
            ..DOUBLE
        };

        CRASH_AFTER_DELETE.store(true, std::sync::atomic::Ordering::SeqCst);
        let crashed = std::panic::catch_unwind(|| open(&path, &[double_v2], WhenHeld::Wait));
        CRASH_AFTER_DELETE.store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(crashed.is_err(), "the injected crash must end that open");
        drop(crashed);
        {
            let (db, _) = open(&path, &[DOUBLE], WhenHeld::Wait).expect("open under the old set");
            assert_eq!(
                count(&db, "SELECT sum(d) FROM v"),
                6,
                "the crashed transaction must leave the view in place"
            );
        }

        let (db, _) = open(&path, &[double_v2], WhenHeld::Wait)
            .unwrap_or_else(|e| panic!("the open after a crash must succeed: {e}"));
        let conn = connect(&db).unwrap();
        assert!(matches!(
            stored_signature(&conn).unwrap(),
            StoredSignature::Recorded(s) if s == record(&[double_v2])
        ));
        assert!(!has(&db, "view", "v"), "the next open must drop the view");
        assert_eq!(count(&db, "SELECT count(*) FROM t"), 2);
    }

    /// A file an older binary built under the empty set keeps its rows when
    /// this binary, which registers functions, first opens it.
    #[test]
    fn a_database_built_without_this_binarys_functions_keeps_its_rows_at_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("holon.db");
        {
            let db = open(&path, &[], WhenHeld::Wait).unwrap().0;
            exec(&db, "CREATE TABLE t (id INTEGER PRIMARY KEY)").unwrap();
            exec(&db, "INSERT INTO t (id) VALUES (1)").unwrap();
        }
        let db = TursoBackend::open_database(&path).expect("open with this binary's set");
        assert_eq!(count(&db, "SELECT count(*) FROM t"), 1);
        let conn = connect(&db).unwrap();
        assert!(matches!(
            stored_signature(&conn).unwrap(),
            StoredSignature::Recorded(s) if s == record(scalar_fns::ALL)
                && s.contains("holon_div/2/v2")
        ));
    }

    /// The files of the database at `path`, with their bytes.
    fn files_of(path: &Path) -> Vec<(String, Vec<u8>)> {
        TursoDurableState::new(path)
            .durable_paths()
            .into_iter()
            .filter(|file| file.exists())
            .map(|file| {
                let suffix = suffix_of(&file, path.to_str().unwrap()).to_string();
                (suffix, std::fs::read(&file).unwrap())
            })
            .collect()
    }

    /// A record that names no engine format, another one, or that is torn or
    /// foreign leaves the format unknown: the file moves aside whole, with its
    /// rows and views, and a rebuilt copy takes its place: the view is gone,
    /// the table nothing classifies keeps its rows, and the record is this
    /// binary's.
    #[test]
    fn a_file_in_another_or_an_unknown_engine_format_is_moved_aside_whole() {
        for (case, record_rows) in [
            ("no row", vec![]),
            ("two rows", vec!["'a'".to_string(), "'b'".to_string()]),
            ("a blob", vec!["X'00'".to_string()]),
            (
                "no format",
                vec![format!("'{}'", scalar_fns::signature(&[DOUBLE]))],
            ),
            (
                "the previous format",
                vec![format!(
                    "'{};view-state/exact-real-sum'",
                    scalar_fns::signature(&[DOUBLE])
                )],
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("holon.db");
            build_with_double(&path);
            {
                let db = open(&path, &[DOUBLE], WhenHeld::Wait).unwrap().0;
                exec(&db, &format!("DELETE FROM {FN_SET_TABLE}")).unwrap();
                for value in &record_rows {
                    exec(
                        &db,
                        &format!("INSERT INTO {FN_SET_TABLE} (signature) VALUES ({value})"),
                    )
                    .unwrap();
                }
            }
            let before = files_of(&path);
            let (db, outcome) = open(&path, &[DOUBLE], WhenHeld::Wait)
                .unwrap_or_else(|e| panic!("{case}: the file must not stop the open: {e}"));
            let [
                OpenChange::MovedAside { backup, reason },
                OpenChange::OwnedStateKept {
                    backup: copied,
                    kept,
                    cleared,
                },
            ] = outcome.0.as_slice()
            else {
                panic!("{case}: the open must report the move and the rebuild, got {outcome:?}");
            };
            assert!(
                reason.contains(VIEW_FORMAT),
                "{case}: the reason must name the format this engine maintains: {reason}"
            );
            assert_eq!(copied, backup, "{case}");
            assert_eq!(
                (kept.as_slice(), cleared.as_slice()),
                (&[("t".to_string(), 2)][..], &[][..]),
                "{case}"
            );
            assert!(!has(&db, "view", "v"), "{case}: the rebuild drops the view");
            assert_eq!(
                count(&db, "SELECT sum(n) FROM t"),
                3,
                "{case}: t keeps its rows"
            );
            let conn = connect(&db).unwrap();
            assert!(matches!(
                stored_signature(&conn).unwrap(),
                StoredSignature::Recorded(s) if s == record(&[DOUBLE])
            ));
            assert_eq!(
                files_of(backup),
                before,
                "{case}: the backup holds the original files byte for byte"
            );
            let moved = open_file(backup.to_str().unwrap(), &[DOUBLE]).unwrap();
            assert_eq!(
                (
                    count(&moved, "SELECT count(*) FROM t"),
                    count(&moved, "SELECT sum(d) FROM v")
                ),
                (2, 6),
                "{case}: the moved file must keep its rows and its view"
            );
        }
    }

    /// A file of the previous format with one table of each action.
    fn build_previous_format(path: &Path) {
        let db = open(path, &[DOUBLE], WhenHeld::Wait).unwrap().0;
        for sql in [
            "CREATE TABLE block_raw (id TEXT PRIMARY KEY)",
            // ALLOW(sole_block_writer): test fixture of a previous-format file.
            "INSERT INTO block_raw (id) VALUES ('block:a')",
            "CREATE TABLE advice_suppressed (anchor_id TEXT NOT NULL, lesson_id TEXT NOT NULL, \
             PRIMARY KEY (anchor_id, lesson_id), \
             FOREIGN KEY (anchor_id) REFERENCES block_raw(id) ON DELETE CASCADE)",
            "INSERT INTO advice_suppressed VALUES ('block:a', 'block:lesson')",
            "CREATE TABLE sync_states (provider_name TEXT PRIMARY KEY, sync_token TEXT)",
            "INSERT INTO sync_states VALUES ('gcal', 'token')",
            "CREATE TABLE gcal_event (id TEXT)",
            "INSERT INTO gcal_event VALUES ('e')",
            "CREATE MATERIALIZED VIEW v AS SELECT id, holon_test_double(1) AS d FROM block_raw",
        ] {
            exec(&db, sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
        }
        exec(
            &db,
            &format!(
                "UPDATE {FN_SET_TABLE} SET signature = '{};view-state/exact-real-sum'",
                scalar_fns::signature(&[DOUBLE])
            ),
        )
        .unwrap();
    }

    fn tables(db: &Arc<Database>) -> Vec<String> {
        user_tables(&connect(db).unwrap()).unwrap()
    }

    /// The rebuild keeps the owned row whose anchor table it drops, clears the
    /// sync tokens with the caches, and drops the derived tables.
    #[test]
    fn a_rebuild_keeps_owned_rows_and_clears_the_caches_with_their_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("holon.db");
        build_previous_format(&path);
        let (db, outcome) = open(&path, &[DOUBLE], WhenHeld::Wait).unwrap();
        let [
            OpenChange::MovedAside { .. },
            OpenChange::OwnedStateKept { kept, cleared, .. },
        ] = outcome.0.as_slice()
        else {
            panic!("expected a move and a rebuild, got {outcome:?}");
        };
        assert_eq!(kept, &[("advice_suppressed".to_string(), 1)]);
        assert_eq!(
            cleared,
            &[crate::table_classes::lost_kinds()
                .into_iter()
                .find(|what| what.starts_with("sync tokens"))
                .unwrap()]
        );
        assert_eq!(
            tables(&db),
            ["advice_suppressed".to_string(), FN_SET_TABLE.to_string()]
        );
    }

    /// Every cut point of a rebuild, as an error and as a crash. An error
    /// leaves no file of the copy and opens a fresh database with a notice
    /// naming the backup; a crash leaves what it leaves, and the next open
    /// ends as one clean rebuild. The backups always hold the original bytes.
    /// Example-based: the input space is these eight named points.
    #[test]
    fn a_failed_or_crashed_rebuild_falls_back_and_the_next_open_converges() {
        use rebuild_fault::Cut;
        use rebuild_fault::Fault;
        for cut in [Cut::Copied, Cut::Opened, Cut::Dropped, Cut::Committed] {
            for fault in [Fault::Error, Fault::Crash] {
                let case = format!("{fault:?} after {cut:?}");
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("holon.db");
                build_previous_format(&path);
                let original = files_of(&path);
                *rebuild_fault::PLANTED.lock().unwrap() = Some((cut, fault));
                let opened = std::panic::catch_unwind(|| open(&path, &[DOUBLE], WhenHeld::Wait));
                *rebuild_fault::PLANTED.lock().unwrap() = None;
                match (fault, opened) {
                    (Fault::Error, Ok(Ok((db, outcome)))) => {
                        let [
                            OpenChange::MovedAside { backup, .. },
                            OpenChange::OwnedStateLost {
                                backup: lost_in,
                                kinds,
                                why,
                            },
                        ] = outcome.0.as_slice()
                        else {
                            panic!("{case}: expected a move and a lost notice, got {outcome:?}");
                        };
                        assert_eq!(lost_in, backup, "{case}");
                        assert_eq!(kinds, &crate::table_classes::lost_kinds(), "{case}");
                        assert!(why.contains("injected failure"), "{case}: {why}");
                        assert_eq!(tables(&db), [FN_SET_TABLE.to_string()], "{case}: fresh");
                        assert_eq!(files_of(backup), original, "{case}: backup bytes");
                    }
                    (Fault::Crash, Err(_)) => {
                        let (db, outcome) = open(&path, &[DOUBLE], WhenHeld::Wait)
                            .unwrap_or_else(|e| panic!("{case}: the open after it: {e}"));
                        assert_eq!(
                            tables(&db),
                            ["advice_suppressed".to_string(), FN_SET_TABLE.to_string()],
                            "{case}: the next open ends as one clean rebuild ({outcome:?})"
                        );
                        assert_eq!(count(&db, "SELECT count(*) FROM advice_suppressed"), 1);
                        let mut backups: Vec<std::path::PathBuf> = std::fs::read_dir(dir.path())
                            .unwrap()
                            .map(|e| e.unwrap().path())
                            .filter(|p| {
                                p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                                    n.starts_with("holon.db.unusable-")
                                        && !["-wal", "-shm", "-tshm"].iter().any(|s| n.ends_with(s))
                                })
                            })
                            .collect();
                        backups.sort();
                        assert!(!backups.is_empty(), "{case}: a backup exists");
                        assert_eq!(files_of(&backups[0]), original, "{case}: first backup");
                    }
                    (_, Ok(Err(e))) => panic!("{case}: the open failed: {e}"),
                    (Fault::Error, Err(_)) => panic!("{case}: the open crashed"),
                    (Fault::Crash, Ok(Ok(_))) => panic!("{case}: the open did not crash"),
                }
            }
        }
    }

    /// The engine opens a file whose view calls a function nobody registered,
    /// and only refuses the next write that has to maintain that view. The
    /// open must drop the view instead, and say so.
    #[test]
    fn a_function_dropped_from_the_registered_set_drops_its_views_at_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("holon.db");
        {
            let (db, _) = open(&path, scalar_fns::ALL, WhenHeld::Wait).unwrap();
            exec(&db, "CREATE TABLE t (id INTEGER PRIMARY KEY, n INTEGER)").unwrap();
            exec(&db, "INSERT INTO t (n) VALUES (1), (2)").unwrap();
            exec(
                &db,
                "CREATE MATERIALIZED VIEW v AS SELECT id, holon_add(n, 1) AS m FROM t",
            )
            .unwrap();
        }
        let without_add: Vec<ScalarFn> = scalar_fns::ALL
            .iter()
            .filter(|f| f.name != "holon_add")
            .copied()
            .collect();
        assert_eq!(without_add.len() + 1, scalar_fns::ALL.len());

        let (db, outcome) = open(&path, &without_add, WhenHeld::Wait)
            .unwrap_or_else(|e| panic!("open without holon_add: {e}"));
        assert!(
            matches!(
                outcome.0.as_slice(),
                [OpenChange::ViewsDropped { views, .. }] if views == &["v".to_string()]
            ),
            "the open must disclose the dropped view, got {:?}",
            outcome.0
        );
        assert!(!has(&db, "view", "v"));
        exec(&db, "INSERT INTO t (n) VALUES (3)")
            .expect("with the view gone, writes to its source table must be accepted");
        assert_eq!(count(&db, "SELECT count(*) FROM t"), 3);
    }

    /// `w` reads `v`, which calls `DOUBLE`: both go, the source rows stay.
    #[test]
    fn a_function_set_change_drops_a_chained_view_and_keeps_the_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("holon.db");
        build_with_double(&path);
        {
            let (db, _) = open(&path, &[DOUBLE], WhenHeld::Wait).unwrap();
            exec(
                &db,
                "CREATE MATERIALIZED VIEW w AS SELECT id, d FROM v WHERE d > 2",
            )
            .unwrap();
            assert_eq!(count(&db, "SELECT count(*) FROM w"), 1);
        }
        let (db, _) = open(&path, &[], WhenHeld::Wait).expect("open without DOUBLE");
        assert!(!has(&db, "view", "v") && !has(&db, "view", "w"));
        assert_eq!(count(&db, "SELECT count(*) FROM t"), 2);
        exec(&db, "INSERT INTO t (n) VALUES (3)").expect("t accepts writes");
    }

    /// The engine treats DROP of a materialized view, a DELETE and the record
    /// write as one transaction: a ROLLBACK keeps all three, a COMMIT applies
    /// all three.
    #[test]
    fn a_view_drop_rolls_back_and_commits_with_the_writes_beside_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("holon.db");
        build_with_double(&path);
        let (db, _) = open(&path, &[DOUBLE], WhenHeld::Wait).unwrap();
        exec(&db, "CREATE TABLE d (x INTEGER)").unwrap();
        exec(&db, "INSERT INTO d (x) VALUES (1)").unwrap();
        let conn = connect(&db).unwrap();
        let run = |sql: &str| conn.execute(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
        let changed = format!("changed;{VIEW_FORMAT}");
        let in_tx = |end: &str| {
            run("BEGIN IMMEDIATE");
            run("DROP VIEW v");
            run("DELETE FROM d");
            run(&format!(
                "UPDATE {FN_SET_TABLE} SET signature = '{changed}'"
            ));
            run(end);
        };

        in_tx("ROLLBACK");
        assert!(has(&db, "view", "v"), "ROLLBACK must keep the view");
        assert_eq!(count(&db, "SELECT sum(d) FROM v"), 6);
        assert_eq!(count(&db, "SELECT count(*) FROM d"), 1);
        assert!(matches!(
            stored_signature(&conn).unwrap(),
            StoredSignature::Recorded(s) if s == record(&[DOUBLE])
        ));
        exec(&db, "INSERT INTO t (n) VALUES (3)").expect("the kept view still maintains");
        assert_eq!(count(&db, "SELECT sum(d) FROM v"), 12);

        in_tx("COMMIT");
        assert!(!has(&db, "view", "v"));
        assert_eq!(count(&db, "SELECT count(*) FROM d"), 0);
        assert!(matches!(
            stored_signature(&conn).unwrap(),
            StoredSignature::Recorded(s) if s == changed
        ));
        drop(conn);
        drop(db);
        let (db, _) = open(&path, &[DOUBLE], WhenHeld::Wait).unwrap();
        assert!(
            !has(&db, "view", "v"),
            "the committed drop must survive a reopen"
        );
        assert_eq!(count(&db, "SELECT count(*) FROM t"), 3);
    }

    #[test]
    fn engine_open_errors_map_to_their_class() {
        use std::io::ErrorKind;
        let short_read = CompletionError::ShortRead {
            page_idx: 1,
            expected: 4096,
            actual: 200,
        };
        for (error, class) in [
            (LimboError::NotADB, DatabaseOpenFailure::Corrupt),
            (
                LimboError::Corrupt("invalid page size in WAL".into()),
                DatabaseOpenFailure::Corrupt,
            ),
            (
                LimboError::CompletionError(short_read),
                DatabaseOpenFailure::Corrupt,
            ),
            (
                LimboError::LockingError(
                    "Failed opening database 'x'. Database is already open without \
                     experimental multiprocess WAL in another process"
                        .into(),
                ),
                DatabaseOpenFailure::Locked,
            ),
            (
                LimboError::LockingError(
                    "Failed locking file 'x'. File is locked by another process".into(),
                ),
                DatabaseOpenFailure::Locked,
            ),
            (
                LimboError::LockingError("Failed locking file 'x', permission denied".into()),
                DatabaseOpenFailure::Unclassified,
            ),
            (
                LimboError::LockingError(
                    "mmap shared WAL coordination file failed: no memory".into(),
                ),
                DatabaseOpenFailure::Unclassified,
            ),
            (
                LimboError::UnsupportedEncoding("UTF-16le".into()),
                DatabaseOpenFailure::Unclassified,
            ),
            (
                LimboError::DatabaseFull,
                DatabaseOpenFailure::Io(ErrorKind::StorageFull),
            ),
            (
                LimboError::CompletionError(CompletionError::IOError(
                    ErrorKind::PermissionDenied,
                    "open",
                )),
                DatabaseOpenFailure::Io(ErrorKind::PermissionDenied),
            ),
            (
                LimboError::InternalError("x".into()),
                DatabaseOpenFailure::Unclassified,
            ),
        ] {
            assert_eq!(open_failure(&error), class, "{error}");
        }
    }

    #[test]
    fn an_unclassified_open_error_names_its_cause() {
        let message = engine("open")(LimboError::InternalError("page 7 vanished".into()))
            .classify("/db")
            .to_string();
        assert!(
            message.contains("does not classify") && message.contains("page 7 vanished"),
            "{message}"
        );
    }

    /// Bytes the engine cannot open as a database are kept under a new name,
    /// and the open goes on with a fresh file.
    #[test]
    fn a_file_the_engine_cannot_open_is_moved_aside_not_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("holon.db");
        let garbage = b"not a database, but somebody's only copy".repeat(200);
        std::fs::write(&path, &garbage).unwrap();

        let (db, outcome) = open(&path, &[DOUBLE], WhenHeld::Wait)
            .unwrap_or_else(|e| panic!("an unopenable file must not stop the open: {e}"));
        let [
            OpenChange::MovedAside { backup, .. },
            OpenChange::OwnedStateLost { .. },
        ] = outcome.0.as_slice()
        else {
            panic!("the open must report the move and the lost state, got {outcome:?}");
        };
        exec(&db, "CREATE TABLE t (id INTEGER PRIMARY KEY)").expect("the fresh file is usable");
        let backups: Vec<std::path::PathBuf> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("holon.db.unusable-") && !n.ends_with("-wal"))
            })
            .collect();
        match backups.as_slice() {
            [found] => assert_eq!(
                (found, std::fs::read(found).unwrap()),
                (backup, garbage),
                "the backup must be the reported path and hold the file byte for byte"
            ),
            other => panic!("expected one backup next to the database, found {other:?}"),
        }
    }

    /// The write-ahead log and the shared-memory file belong to the database
    /// file; they move with it, under the same timestamp.
    #[test]
    fn the_sidecars_of_an_unopenable_file_are_moved_aside_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("holon.db");
        let contents = [
            ("", b"main file, not a database".repeat(200)),
            ("-wal", b"write-ahead log bytes".repeat(150)),
            ("-shm", b"shared memory bytes".repeat(100)),
        ];
        for (suffix, bytes) in &contents {
            std::fs::write(format!("{}{suffix}", path.display()), bytes).unwrap();
        }

        let (db, outcome) = open(&path, &[DOUBLE], WhenHeld::Wait)
            .expect("an unopenable file must not stop the open");
        let [
            OpenChange::MovedAside { backup, .. },
            OpenChange::OwnedStateLost { .. },
        ] = outcome.0.as_slice()
        else {
            panic!("the open must report the move and the lost state, got {outcome:?}");
        };
        exec(&db, "CREATE TABLE t (id INTEGER PRIMARY KEY)").expect("the fresh file is usable");
        for (suffix, bytes) in &contents {
            let moved = format!("{}{suffix}", backup.display());
            assert_eq!(
                std::fs::read(&moved).unwrap_or_else(|e| panic!("{moved}: {e}")),
                *bytes,
                "{moved} must hold the original bytes"
            );
        }
    }

    fn ids(conn: &Arc<Connection>) -> Vec<i64> {
        rows(conn, "SELECT id FROM k ORDER BY id")
            .expect("read k")
            .into_iter()
            .map(|row| match row.as_slice() {
                [Value::Numeric(Numeric::Integer(n))] => *n,
                other => panic!("k row is {other:?}"),
            })
            .collect()
    }

    proptest::proptest! {
        /// A write transaction whose statements may fail on a duplicate key:
        /// a failed one leaves the connection out of the transaction and the
        /// table as it was before, and another connection can write next.
        #[test]
        fn a_failed_write_transaction_is_rolled_back_and_releases_the_file(
            seeded in proptest::collection::btree_set(0i64..6, 0..4),
            inserts in proptest::collection::vec(0i64..6, 0..6),
        ) {
            let dir = tempfile::tempdir().unwrap();
            let (db, _) = open(&dir.path().join("holon.db"), &[], WhenHeld::Wait).unwrap();
            exec(&db, "CREATE TABLE k (id INTEGER PRIMARY KEY)").unwrap();
            for id in &seeded {
                exec(&db, &format!("INSERT INTO k (id) VALUES ({id})")).unwrap();
            }
            let conn = connect(&db).unwrap();
            let result = in_write_tx(&conn, || {
                for id in &inserts {
                    let sql = format!("INSERT INTO k (id) VALUES ({id})");
                    conn.execute(&sql).map_err(engine(&sql))?;
                }
                Ok(())
            });

            let mut taken = seeded.clone();
            let collides = !inserts.iter().all(|id| taken.insert(*id));
            proptest::prop_assert_eq!(result.is_err(), collides);
            proptest::prop_assert!(conn.get_auto_commit(), "the connection is still in a transaction");
            let expected: Vec<i64> = match collides {
                true => seeded.iter().copied().collect(),
                false => taken.iter().copied().collect(),
            };
            proptest::prop_assert_eq!(ids(&conn), expected);
            exec(&db, "INSERT INTO k (id) VALUES (100)")
                .map_err(|e| proptest::test_runner::TestCaseError::fail(format!("next writer: {e}")))?;
        }
    }
}
