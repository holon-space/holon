//! The one place that opens a Holon Turso database.
//!
//! No open deletes a row. A file this binary cannot use as it is loses only
//! what it computed: its materialized views, rebuilt from the tables they read,
//! and the `block_derived` cache. That happens when the file was built with a
//! different [`scalar_fns`] set, or when the engine could not load one of its
//! views. A file the engine cannot open at all is moved aside, never deleted.

use std::path::Path;
use std::sync::Arc;

use holon_core::replica_state::DurableReplicaState;
use holon_core::storage::types::Result;
use holon_core::storage::types::StorageError;
use turso_core::Connection;
use turso_core::Database;
use turso_core::DatabaseOpts;
use turso_core::IO;
use turso_core::MemoryIO;
use turso_core::OpenFlags;
use turso_core::OpenOptions;
use turso_core::Value;

use crate::durable_state::TursoDurableState;
use crate::scalar_fns;
use crate::scalar_fns::ScalarFn;

/// What an open did to the database file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenOutcome {
    Kept,
    /// The file could not be used as it was: its materialized views and the
    /// `block_derived` cache were dropped. Every table row is kept.
    ViewsDropped {
        reason: String,
        views: Vec<String>,
    },
    /// The engine could not use the file at all. It was renamed to `backup`,
    /// and a fresh database was opened in its place.
    MovedAside {
        reason: String,
        backup: std::path::PathBuf,
    },
}

/// What this process's boot did to its database file. Provided once per
/// session; a boot that opens no database (`LoroMemory`) records `Kept`.
#[derive(Debug, Clone)]
pub struct BootOpenOutcome(pub OpenOutcome);

const FN_SET_TABLE: &str = "holon_db_fn_set";
/// Values computed with the functions; the derived-field reconciler refills it.
const DERIVED_CACHE_TABLE: &str = "block_derived";

/// The opened database, and what this open did to the file.
pub(crate) fn open(db_path: &Path, fns: &[ScalarFn]) -> Result<(Arc<Database>, OpenOutcome)> {
    let path = db_path
        .to_str()
        .ok_or_else(|| StorageError::DatabaseError(format!("path {db_path:?} is not UTF-8")))?;
    if path.starts_with(":memory:") {
        let db = open_with_io(Arc::new(MemoryIO::new()), path, OpenFlags::default(), fns)?;
        return Ok((db, OpenOutcome::Kept));
    }
    let db = match open_file(path, fns) {
        Ok(db) => db,
        Err(e) => {
            return set_aside(
                None,
                db_path,
                path,
                fns,
                &format!("the engine cannot open it: {e}"),
            );
        }
    };
    let Some(reason) = unusable_reason(&db, fns)? else {
        record_signature(&db, fns)?;
        return Ok((db, OpenOutcome::Kept));
    };
    match drop_computed(&db, fns) {
        Ok(views) => {
            tracing::info!(
                "[open_database] {path}: {reason}. Dropped its materialized views [{}] and the \
                 {DERIVED_CACHE_TABLE} cache; every table row is kept, and the views are \
                 rebuilt from the tables",
                views.join(", ")
            );
            Ok((db, OpenOutcome::ViewsDropped { reason, views }))
        }
        Err(e) => set_aside(
            Some(db),
            db_path,
            path,
            fns,
            &format!("{reason}, and dropping its views failed: {e}"),
        ),
    }
}

#[cfg(target_family = "unix")]
fn open_file(path: &str, fns: &[ScalarFn]) -> Result<Arc<Database>> {
    let io = turso_core::UnixIO::new().map_err(|e| StorageError::DatabaseError(e.to_string()))?;
    open_with_io(Arc::new(io), path, OpenFlags::default(), fns)
}

/// wasm32: a file path needs the host IO that the browser worker registers
/// (the OPFS shim) before the engine opens a database.
#[cfg(all(not(target_family = "unix"), target_family = "wasm"))]
fn open_file(path: &str, fns: &[ScalarFn]) -> Result<Arc<Database>> {
    let io = crate::turso::wasm_io::registered().ok_or_else(|| {
        StorageError::DatabaseError(format!(
            "open_database('{path}'): no wasm IO registered — call \
             holon_turso::register_wasm_io (e.g. with the OPFS shim) before opening a \
             file-backed database on wasm32"
        ))
    })?;
    open_with_io(io, path, OpenFlags::Create, fns)
}

#[cfg(all(not(target_family = "unix"), not(target_family = "wasm")))]
fn open_file(_: &str, _: &[ScalarFn]) -> Result<Arc<Database>> {
    Err(StorageError::DatabaseError(
        "File-based storage not yet supported on this platform".to_string(),
    ))
}

/// Rename every file of the database to a timestamped backup beside it and
/// open a fresh one.
#[cfg(target_family = "unix")]
fn set_aside(
    db: Option<Arc<Database>>,
    db_path: &Path,
    path: &str,
    fns: &[ScalarFn],
    reason: &str,
) -> Result<(Arc<Database>, OpenOutcome)> {
    if let Some(db) = db {
        assert_eq!(
            Arc::strong_count(&db),
            1,
            "{path} is unusable ({reason}), but another holder in this process still has it open"
        );
    }
    let backup = std::path::PathBuf::from(format!(
        "{path}.unusable-{}",
        chrono::Utc::now().format("%Y%m%dT%H%M%S%.3fZ")
    ));
    for file in TursoDurableState::new(db_path).durable_paths() {
        if !file.exists() {
            continue;
        }
        let suffix = file
            .to_str()
            .and_then(|f| f.strip_prefix(path))
            .unwrap_or_else(|| panic!("{} is not a file of {path}", file.display()));
        let to = std::path::PathBuf::from(format!("{}{suffix}", backup.display()));
        assert!(!to.exists(), "the backup {} exists already", to.display());
        std::fs::rename(&file, &to).map_err(|e| {
            StorageError::DatabaseError(format!(
                "move the unusable database file {} aside to {}: {e}",
                file.display(),
                to.display()
            ))
        })?;
    }
    tracing::error!(
        "[open_database] {path} is unusable: {reason}. Moved it aside to {} and opened a fresh \
         database; anything that was only in the database is in that file",
        backup.display()
    );
    let db = open_file(path, fns)?;
    if let Some(fresh) = unusable_reason(&db, fns)? {
        return Err(StorageError::DatabaseError(format!(
            "the fresh database at {path} is unusable right after it was created: {fresh}"
        )));
    }
    record_signature(&db, fns)?;
    Ok((
        db,
        OpenOutcome::MovedAside {
            reason: reason.to_string(),
            backup,
        },
    ))
}

/// OPFS files cannot be renamed through the engine's IO.
#[cfg(not(target_family = "unix"))]
fn set_aside(
    _: Option<Arc<Database>>,
    _: &Path,
    path: &str,
    _: &[ScalarFn],
    reason: &str,
) -> Result<(Arc<Database>, OpenOutcome)> {
    Err(StorageError::DatabaseError(format!(
        "the database at {path} is unusable: {reason}. Copy it out of the browser storage and \
         delete it there; the org files rebuild the blocks"
    )))
}

/// In one transaction: drop every materialized view and every view the engine
/// could not load, dependents first; empty the derived cache; record `fns`.
/// Returns the dropped view names.
fn drop_computed(db: &Arc<Database>, fns: &[ScalarFn]) -> Result<Vec<String>> {
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
        other => Err(StorageError::DatabaseError(format!(
            "sqlite_schema view row is {other:?}"
        ))),
    })
    .collect::<Result<Vec<_>>>()?
    .into_iter()
    .filter(|(name, sql)| is_materialized(sql) || unloadable.contains(name))
    .collect();
    let run = |sql: &str| {
        conn.execute(sql)
            .map_err(|e| StorageError::DatabaseError(format!("{sql}: {e}")))
    };
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
            return Err(StorageError::DatabaseError(format!(
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

fn unloadable_views(conn: &Arc<Connection>) -> Result<Vec<String>> {
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
    .map_err(|e| StorageError::DatabaseError(format!("read the schema: {e}")))
}

fn has_table(conn: &Arc<Connection>, table: &str) -> Result<bool> {
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
) -> Result<Arc<Database>> {
    let opts = DatabaseOpts::default().with_views(true);
    // The Tantivy `fts` and sparse-vector `CREATE INDEX .. USING` methods exist
    // only in the native turso_core build.
    #[cfg(target_family = "unix")]
    let opts = opts.with_index_method(true);
    let options = OpenOptions::new(Arc::new(turso_core::SqliteDialect))
        .flags(flags)
        .db_opts(opts);
    Database::open(io, path, scalar_fns::register_on_options(options, fns))
        .map_err(|e| StorageError::DatabaseError(format!("open {path}: {e}")))
}

fn unusable_reason(db: &Arc<Database>, fns: &[ScalarFn]) -> Result<Option<String>> {
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
        .map_err(|e| StorageError::DatabaseError(format!("read the schema: {e}")))?;
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

fn stored_signature(conn: &Arc<Connection>) -> Result<StoredSignature> {
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
fn record_signature(db: &Arc<Database>, fns: &[ScalarFn]) -> Result<()> {
    let conn = connect(db)?;
    let wanted = scalar_fns::signature(fns);
    match stored_signature(&conn)? {
        StoredSignature::Recorded(stored) if stored == wanted => return Ok(()),
        // No table reads as the empty set.
        StoredSignature::Absent if wanted.is_empty() => return Ok(()),
        _ => {}
    }
    let run = |sql: &str| {
        conn.execute(sql)
            .map_err(|e| StorageError::DatabaseError(format!("{sql}: {e}")))
    };
    run("BEGIN IMMEDIATE")?;
    write_signature(&conn, fns)?;
    run("COMMIT")
}

/// Inside the caller's transaction.
fn write_signature(conn: &Arc<Connection>, fns: &[ScalarFn]) -> Result<()> {
    let run = |sql: &str| {
        conn.execute(sql)
            .map_err(|e| StorageError::DatabaseError(format!("{sql}: {e}")))
    };
    run(&format!(
        "CREATE TABLE IF NOT EXISTS {FN_SET_TABLE} (signature TEXT NOT NULL)"
    ))?;
    run(&format!("DELETE FROM {FN_SET_TABLE}"))?;
    #[cfg(test)]
    if CRASH_AFTER_DELETE.load(std::sync::atomic::Ordering::SeqCst) {
        panic!("injected crash between DELETE and INSERT");
    }
    let insert = format!("INSERT INTO {FN_SET_TABLE} (signature) VALUES (?1)");
    let mut stmt = conn
        .prepare(&insert)
        .map_err(|e| StorageError::DatabaseError(format!("{insert}: {e}")))?;
    stmt.bind_at(
        1.try_into().expect("1 is non-zero"),
        Value::build_text(scalar_fns::signature(fns)),
    )
    .map_err(|e| StorageError::DatabaseError(format!("{insert}: bind: {e}")))?;
    stmt.run_ignore_rows()
        .map_err(|e| StorageError::DatabaseError(format!("{insert}: {e}")))
}

fn is_empty(conn: &Arc<Connection>) -> Result<bool> {
    Ok(rows(conn, "SELECT 1 FROM sqlite_schema LIMIT 1")?.is_empty())
}

fn connect(db: &Arc<Database>) -> Result<Arc<Connection>> {
    db.connect()
        .map_err(|e| StorageError::DatabaseError(format!("connect: {e}")))
}

fn rows(conn: &Arc<Connection>, sql: &str) -> Result<Vec<Vec<Value>>> {
    conn.prepare(sql)
        .and_then(|mut stmt| stmt.run_collect_rows())
        .map_err(|e| StorageError::DatabaseError(format!("{sql}: {e}")))
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

    fn exec(db: &Arc<Database>, sql: &str) -> Result<()> {
        connect(db)?
            .execute(sql)
            .map_err(|e| StorageError::DatabaseError(format!("{sql}: {e}")))
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
        let OpenOutcome::MovedAside { backup, .. } = outcome else {
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
                (&backup, garbage),
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
        let OpenOutcome::MovedAside { backup, .. } = outcome else {
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
