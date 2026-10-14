//! The one place that opens a Holon Turso database.
//!
//! No open deletes a row. A file this binary cannot use as it is loses only
//! what it computed: its materialized views, rebuilt from the tables they read,
//! and the `block_derived` cache. That happens when the file was built with a
//! different [`scalar_fns`] set, or when the engine could not load one of its
//! views. A damaged file is moved aside, never deleted: the shared-WAL
//! coordination file or the write-ahead log alone when the error points there,
//! else every file of the database. Every engine error of the open, from the
//! file open to the last read, is classified the same way; any failure other
//! than damage leaves the files where they are.

use std::path::Path;
use std::sync::Arc;

use holon_core::replica_state::DurableReplicaState;
use holon_core::storage::types::DatabaseOpenFailure;
use holon_core::storage::types::Result;
use holon_core::storage::types::StorageError;
use turso_core::CompletionError;
use turso_core::Connection;
use turso_core::Database;
use turso_core::DatabaseOpts;
use turso_core::IO;
use turso_core::LimboError;
use turso_core::MemoryIO;
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
    /// The engine could not use the file at all. It was renamed to `backup`,
    /// and a fresh database was opened in its place.
    MovedAside {
        reason: String,
        backup: std::path::PathBuf,
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
/// Values computed with the functions; the derived-field reconciler refills it.
const DERIVED_CACHE_TABLE: &str = "block_derived";

/// How often an open tries a file another process holds, and how long it
/// waits between tries. Only a unix file lock can be held by another process.
const LOCK_TRIES: u32 = if cfg!(target_family = "unix") { 10 } else { 1 };
const LOCK_RETRY: std::time::Duration = std::time::Duration::from_millis(500);

/// The opened database, and what this open did to the file.
pub(crate) fn open(
    db_path: &Path,
    fns: &[ScalarFn],
) -> std::result::Result<(Arc<Database>, OpenOutcome), OpenFailed> {
    let mut changes = Vec::new();
    match open_recording(db_path, fns, &mut changes) {
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
            if tries < LOCK_TRIES {
                tracing::warn!(
                    "[open_database] {path}: another process holds it; try {tries} of \
                     {LOCK_TRIES}, next in {LOCK_RETRY:?}"
                );
                std::thread::sleep(LOCK_RETRY);
                continue;
            }
        }
        if !error.is_corrupt() {
            return Err(error.classify(path));
        }
        if let Some(db) = db {
            assert_eq!(
                Arc::strong_count(&db),
                1,
                "{path} is damaged ({error}), but another holder in this process still has it open"
            );
        }
        let reason = format!("the engine cannot use it: {error}");
        let stem: &std::path::Path = stem.get_or_insert_with(|| free_backup_stem(db_path, path));
        changes.push(recover_damaged(leg, reason, db_path, path, stem)?);
        if let Some(OpenChange::MovedAside { .. }) = changes.last() {
            return open_fresh(path, fns).map_err(|e| e.classify(path));
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
    Engine { doing: String, error: LimboError },
    Other(StorageError),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Engine { doing, error } => write!(f, "{doing}: {error}"),
            Self::Other(e) => write!(f, "{e}"),
        }
    }
}

impl OpenError {
    fn failure(&self) -> Option<DatabaseOpenFailure> {
        match self {
            Self::Engine { error, .. } => Some(open_failure(error)),
            Self::Other(_) => None,
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
    if let Err(error) = db.connect() {
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

/// Records this binary's function set in `db`, or drops what `db` computed
/// with another one.
fn admit(db: &Arc<Database>, path: &str, fns: &[ScalarFn]) -> Step<Option<OpenChange>> {
    let Some(reason) = unusable_reason(db, fns)? else {
        record_signature(db, fns)?;
        return Ok(None);
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
    for file in TursoDurableState::new(db_path).durable_paths() {
        if file.exists() {
            move_aside(&file, path, stem)?;
        }
    }
    tracing::error!(
        "[open_database] {path} is unusable: {reason}. Moved it aside to {} and opened a fresh \
         database; anything that was only in the database is in that file",
        stem.display()
    );
    Ok(OpenChange::MovedAside {
        reason,
        backup: stem.to_path_buf(),
    })
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
    if let Some(fresh) = unusable_reason(&db, fns)? {
        return Err(invalid(format!(
            "the fresh database at {path} is unusable right after it was created: {fresh}"
        )));
    }
    record_signature(&db, fns)?;
    Ok(db)
}

/// In one transaction: drop every materialized view and every view the engine
/// could not load, dependents first; empty the derived cache; record `fns`.
/// Returns the dropped view names.
fn drop_computed(db: &Arc<Database>, fns: &[ScalarFn]) -> Step<Vec<String>> {
    let conn = connect(db)?;
    let unloadable = unloadable_views(&conn)?;
    let mut views: Vec<(String, String)> = rows(
        &conn,
        "SELECT name, sql FROM sqlite_schema WHERE type = 'view' ORDER BY name",
    )?
    .into_iter()
    .map(|row| match row.as_slice() {
        [Value::Text(name), Value::Text(sql)] => {
            Ok((name.as_str().to_string(), sql.as_str().to_string()))
        }
        other => Err(invalid(format!("sqlite_schema view row is {other:?}"))),
    })
    .collect::<Step<Vec<_>>>()?
    .into_iter()
    .filter(|(name, sql)| is_materialized(sql) || unloadable.contains(name))
    .collect();
    let run = |sql: &str| conn.execute(sql).map_err(engine(sql));
    run("BEGIN IMMEDIATE")?;
    let mut dropped = Vec::new();
    while !views.is_empty() {
        let (free, held): (Vec<_>, Vec<_>) = views.iter().cloned().partition(|(name, _)| {
            !views
                .iter()
                .any(|(other, sql)| other != name && names_word(sql, name))
        });
        if free.is_empty() {
            run("ROLLBACK")?;
            return Err(invalid(format!(
                "the views [{}] read each other, so none can be dropped first",
                held.iter()
                    .map(|(n, _)| n.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        for (name, _) in free {
            run(&format!("DROP VIEW \"{name}\""))?;
            dropped.push(name);
        }
        views = held;
    }
    if has_table(&conn, DERIVED_CACHE_TABLE)? {
        run(&format!("DELETE FROM {DERIVED_CACHE_TABLE}"))?;
    }
    write_signature(&conn, fns)?;
    run("COMMIT")?;
    Ok(dropped)
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

fn unloadable_views(conn: &Arc<Connection>) -> Step<Vec<String>> {
    conn.with_schema_mut(|schema| {
        let mut views: Vec<String> = schema
            .incompatible_views
            .keys()
            .chain(schema.broken_views.iter())
            .cloned()
            .collect();
        views.sort();
        views
    })
    .map_err(engine("read the schema"))
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
    let options = OpenOptions::new(Arc::new(turso_core::SqliteDialect))
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

fn unusable_reason(db: &Arc<Database>, fns: &[ScalarFn]) -> Step<Option<String>> {
    let conn = connect(db)?;
    let wanted = scalar_fns::signature(fns);
    let stored = match stored_signature(&conn)? {
        StoredSignature::Recorded(stored) => stored,
        StoredSignature::Absent if is_empty(&conn)? => return Ok(None),
        // Built before databases recorded their function set: it has none.
        StoredSignature::Absent => String::new(),
        StoredSignature::Unreadable(why) => {
            return Ok(Some(format!(
                "its function-set record is unreadable: {why}"
            )));
        }
    };
    if stored != wanted {
        return Ok(Some(format!(
            "it was built with the scalar functions [{stored}], this binary registers [{wanted}]"
        )));
    }
    let unusable = conn
        .with_schema_mut(|schema| {
            let mut views: Vec<String> = schema
                .incompatible_views
                .iter()
                .map(|(name, reason)| format!("{name} ({reason})"))
                .chain(
                    schema
                        .broken_views
                        .iter()
                        .map(|name| format!("{name} (its SQL does not parse)")),
                )
                .collect();
            views.sort();
            views
        })
        .map_err(engine("read the schema"))?;
    if unusable.is_empty() {
        return Ok(None);
    }
    Ok(Some(format!(
        "the engine cannot load the materialized views {}",
        unusable.join(", ")
    )))
}

enum StoredSignature {
    Absent,
    Recorded(String),
    /// A torn or foreign record: the views are dropped and the record written
    /// again, never refused.
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
    let wanted = scalar_fns::signature(fns);
    match stored_signature(&conn)? {
        StoredSignature::Recorded(stored) if stored == wanted => return Ok(()),
        // No table reads as the empty set.
        StoredSignature::Absent if wanted.is_empty() => return Ok(()),
        _ => {}
    }
    let run = |sql: &str| conn.execute(sql).map_err(engine(sql));
    run("BEGIN IMMEDIATE")?;
    write_signature(&conn, fns)?;
    run("COMMIT")
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
        Value::build_text(scalar_fns::signature(fns)),
    )
    .map_err(engine(format!("{insert}: bind")))?;
    stmt.run_ignore_rows().map_err(engine(&insert))
}

fn is_empty(conn: &Arc<Connection>) -> Step<bool> {
    Ok(rows(conn, "SELECT 1 FROM sqlite_schema LIMIT 1")?.is_empty())
}

fn connect(db: &Arc<Database>) -> Step<Arc<Connection>> {
    db.connect().map_err(engine("connect"))
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
        let (db, _) = open(path, &[DOUBLE]).expect("open with the test function");
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

        let (db, _) = open(&path, &[DOUBLE]).expect("reopen with the same set");
        assert_eq!(count(&db, "SELECT sum(d) FROM v"), 6);
        exec(&db, "INSERT INTO t (n) VALUES (3)").expect("the view is usable");
        assert_eq!(count(&db, "SELECT sum(d) FROM v"), 12);
    }

    #[test]
    fn a_database_without_a_recorded_set_keeps_its_rows_under_the_empty_set() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("holon.db");
        {
            let db = open(&path, &[]).unwrap().0;
            exec(&db, "CREATE TABLE t (id INTEGER PRIMARY KEY)").unwrap();
            exec(&db, "INSERT INTO t (id) VALUES (1)").unwrap();
        }
        let db = open(&path, &[]).unwrap().0;
        assert_eq!(count(&db, "SELECT count(*) FROM t"), 1);
        assert!(
            !has(&db, "table", FN_SET_TABLE),
            "the empty set needs no record"
        );
    }

    #[test]
    fn a_database_opened_by_a_larger_set_keeps_its_tables_and_records_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("holon.db");
        {
            let db = open(&path, &[]).unwrap().0;
            exec(&db, "CREATE TABLE t (id INTEGER PRIMARY KEY)").unwrap();
            exec(&db, "INSERT INTO t (id) VALUES (1)").unwrap();
        }
        let db = open(&path, &[DOUBLE]).unwrap().0;
        assert_eq!(
            count(&db, "SELECT count(*) FROM t"),
            1,
            "a larger function set must keep every table row"
        );
        let conn = connect(&db).unwrap();
        assert!(matches!(
            stored_signature(&conn).unwrap(),
            StoredSignature::Recorded(s) if s == "holon_test_double/1/v1"
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
        let crashed = std::panic::catch_unwind(|| open(&path, &[double_v2]));
        CRASH_AFTER_DELETE.store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(crashed.is_err(), "the injected crash must end that open");
        drop(crashed);
        {
            let (db, _) = open(&path, &[DOUBLE]).expect("open under the old set");
            assert_eq!(
                count(&db, "SELECT sum(d) FROM v"),
                6,
                "the crashed transaction must leave the view in place"
            );
        }

        let (db, _) = open(&path, &[double_v2])
            .unwrap_or_else(|e| panic!("the open after a crash must succeed: {e}"));
        let conn = connect(&db).unwrap();
        assert!(matches!(
            stored_signature(&conn).unwrap(),
            StoredSignature::Recorded(s) if s == "holon_test_double/1/v2"
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
            let db = open(&path, &[]).unwrap().0;
            exec(&db, "CREATE TABLE t (id INTEGER PRIMARY KEY)").unwrap();
            exec(&db, "INSERT INTO t (id) VALUES (1)").unwrap();
        }
        let db = TursoBackend::open_database(&path).expect("open with this binary's set");
        assert_eq!(count(&db, "SELECT count(*) FROM t"), 1);
        let conn = connect(&db).unwrap();
        assert!(matches!(
            stored_signature(&conn).unwrap(),
            StoredSignature::Recorded(s) if s == scalar_fns::signature(scalar_fns::ALL)
                && s.contains("holon_div/2/v2")
        ));
    }

    /// A crash between the DELETE and the INSERT of a non-atomic record leaves
    /// the table with no row; a table holding anything but one text row is as
    /// unreadable. Either way the record says nothing about the tables, so they
    /// keep their rows and the record is written again.
    #[test]
    fn an_unreadable_signature_record_keeps_the_rows_and_is_written_again() {
        for (case, torn) in [
            ("no row", vec![]),
            ("two rows", vec!["'a'", "'b'"]),
            ("a blob", vec!["X'00'"]),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("holon.db");
            {
                let db = open(&path, &[]).unwrap().0;
                exec(&db, "CREATE TABLE t (id INTEGER PRIMARY KEY)").unwrap();
                exec(&db, "INSERT INTO t (id) VALUES (1)").unwrap();
                exec(
                    &db,
                    &format!("CREATE TABLE {FN_SET_TABLE} (signature TEXT NOT NULL)"),
                )
                .unwrap();
                for value in torn {
                    exec(
                        &db,
                        &format!("INSERT INTO {FN_SET_TABLE} (signature) VALUES ({value})"),
                    )
                    .unwrap();
                }
            }
            let (db, _) = open(&path, &[DOUBLE]).unwrap_or_else(|e| {
                panic!("{case}: an unreadable signature record must not be an open error: {e}")
            });
            assert_eq!(
                count(&db, "SELECT count(*) FROM t"),
                1,
                "{case}: the rows must be kept"
            );
            let conn = connect(&db).unwrap();
            assert!(
                matches!(
                    stored_signature(&conn).unwrap(),
                    StoredSignature::Recorded(s) if s == "holon_test_double/1/v1"
                ),
                "{case}: the record must be written again"
            );
        }
    }

    /// `w` reads `v`, which calls `DOUBLE`: both go, the source rows stay.
    #[test]
    fn a_function_set_change_drops_a_chained_view_and_keeps_the_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("holon.db");
        build_with_double(&path);
        {
            let (db, _) = open(&path, &[DOUBLE]).unwrap();
            exec(
                &db,
                "CREATE MATERIALIZED VIEW w AS SELECT id, d FROM v WHERE d > 2",
            )
            .unwrap();
            assert_eq!(count(&db, "SELECT count(*) FROM w"), 1);
        }
        let (db, _) = open(&path, &[]).expect("open without DOUBLE");
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
        let (db, _) = open(&path, &[DOUBLE]).unwrap();
        exec(&db, "CREATE TABLE d (x INTEGER)").unwrap();
        exec(&db, "INSERT INTO d (x) VALUES (1)").unwrap();
        let conn = connect(&db).unwrap();
        let run = |sql: &str| conn.execute(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
        let in_tx = |end: &str| {
            run("BEGIN IMMEDIATE");
            run("DROP VIEW v");
            run("DELETE FROM d");
            run(&format!("UPDATE {FN_SET_TABLE} SET signature = 'changed'"));
            run(end);
        };

        in_tx("ROLLBACK");
        assert!(has(&db, "view", "v"), "ROLLBACK must keep the view");
        assert_eq!(count(&db, "SELECT sum(d) FROM v"), 6);
        assert_eq!(count(&db, "SELECT count(*) FROM d"), 1);
        assert!(matches!(
            stored_signature(&conn).unwrap(),
            StoredSignature::Recorded(s) if s == "holon_test_double/1/v1"
        ));
        exec(&db, "INSERT INTO t (n) VALUES (3)").expect("the kept view still maintains");
        assert_eq!(count(&db, "SELECT sum(d) FROM v"), 12);

        in_tx("COMMIT");
        assert!(!has(&db, "view", "v"));
        assert_eq!(count(&db, "SELECT count(*) FROM d"), 0);
        assert!(matches!(
            stored_signature(&conn).unwrap(),
            StoredSignature::Recorded(s) if s == "changed"
        ));
        drop(conn);
        drop(db);
        let (db, _) = open(&path, &[DOUBLE]).unwrap();
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

        let (db, outcome) = open(&path, &[DOUBLE])
            .unwrap_or_else(|e| panic!("an unopenable file must not stop the open: {e}"));
        let [OpenChange::MovedAside { backup, .. }] = outcome.0.as_slice() else {
            panic!("the open must report the move, got {outcome:?}");
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

        let (db, outcome) =
            open(&path, &[DOUBLE]).expect("an unopenable file must not stop the open");
        let [OpenChange::MovedAside { backup, .. }] = outcome.0.as_slice() else {
            panic!("the open must report the move, got {outcome:?}");
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
}
