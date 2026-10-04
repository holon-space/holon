//! The one place that opens a Holon Turso database.
//!
//! The database is derived data: the org files rebuild it. So a file that this
//! binary cannot use is deleted and opened fresh, never migrated. A file is
//! unusable when it was built with a different [`scalar_fns`] set, or when the
//! engine could not load one of its materialized views (such a view refuses
//! every write to the tables that feed it).

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
use crate::table_classes;
use crate::table_classes::CacheRows;
use crate::table_classes::Class;
use crate::table_classes::LostRows;
use crate::table_classes::Rebuild;

const FN_SET_TABLE: &str = "holon_db_fn_set";

/// The opened database, and the rebuild this open did, if any.
pub(crate) fn open(db_path: &Path, fns: &[ScalarFn]) -> Result<(Arc<Database>, Option<Rebuild>)> {
    let path = db_path
        .to_str()
        .ok_or_else(|| StorageError::DatabaseError(format!("path {db_path:?} is not UTF-8")))?;
    if path.starts_with(":memory:") {
        let db = open_with_io(Arc::new(MemoryIO::new()), path, OpenFlags::default(), fns)?;
        return Ok((db, None));
    }
    let db = open_file(path, fns)?;
    let Some(reason) = unusable_reason(&db, fns)? else {
        record_signature(&db, fns)?;
        return Ok((db, None));
    };
    #[cfg(target_family = "unix")]
    return rebuild(db, db_path, path, fns, &reason);
    // OPFS files cannot be deleted through the engine's IO.
    #[cfg(not(target_family = "unix"))]
    return Err(StorageError::DatabaseError(format!(
        "the database at {path} is unusable: {reason}. Delete it from the browser storage; \
         the org files rebuild it"
    )));
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

#[cfg(target_family = "unix")]
fn rebuild(
    db: Arc<Database>,
    db_path: &Path,
    path: &str,
    fns: &[ScalarFn],
    reason: &str,
) -> Result<(Arc<Database>, Option<Rebuild>)> {
    let (lost, caches) = rows_not_rebuilt(&db)?;
    tracing::info!(
        "[open_database] deleting {path}: {reason}. The org files and the Loro store rebuild \
         the blocks and every view. Integration caches cleared, each re-synced from its source \
         (a row the source no longer holds is gone): {}. Lost for good: {}",
        describe_caches(&caches),
        describe(&lost)
    );
    assert_eq!(
        Arc::strong_count(&db),
        1,
        "{path} is unusable ({reason}), but another holder in this process still has it open"
    );
    drop(db);
    for file in TursoDurableState::new(db_path).durable_paths() {
        match std::fs::remove_file(&file) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(StorageError::DatabaseError(format!(
                    "delete unusable database file {}: {e}",
                    file.display()
                )));
            }
        }
    }
    let db = open_file(path, fns)?;
    if let Some(reason) = unusable_reason(&db, fns)? {
        return Err(StorageError::DatabaseError(format!(
            "the fresh database at {path} is unusable right after it was created: {reason}"
        )));
    }
    record_signature(&db, fns)?;
    Ok((
        db,
        Some(Rebuild {
            reason: reason.to_string(),
            lost,
            caches,
        }),
    ))
}

/// Every table the file holds that the org files and the Loro store do not
/// rebuild, with its row count: the lost ones, and the integration caches.
fn rows_not_rebuilt(db: &Arc<Database>) -> Result<(Vec<LostRows>, Vec<CacheRows>)> {
    let conn = connect(db)?;
    let caches: std::collections::HashMap<String, String> =
        if has_table(&conn, "integration_cache")? {
            rows(&conn, "SELECT table_name, provider FROM integration_cache")?
                .into_iter()
                .map(|row| match row.as_slice() {
                    [Value::Text(table), Value::Text(provider)] => {
                        Ok((table.as_str().to_string(), provider.as_str().to_string()))
                    }
                    other => Err(StorageError::DatabaseError(format!(
                        "integration_cache row is {other:?}"
                    ))),
                })
                .collect::<Result<_>>()?
        } else {
            Default::default()
        };
    let mut lost = Vec::new();
    let mut cleared = Vec::new();
    for row in rows(
        &conn,
        "SELECT name FROM sqlite_schema WHERE type = 'table' AND substr(name, 1, 7) NOT IN \
         ('sqlite_', '__turso') ORDER BY name",
    )? {
        let table = match row.as_slice() {
            [Value::Text(name)] => name.as_str().to_string(),
            other => {
                return Err(StorageError::DatabaseError(format!(
                    "sqlite_schema name is {other:?}"
                )));
            }
        };
        match table_classes::class_of(&table, &caches) {
            Some(Class::Rebuilt) => {}
            Some(Class::IntegrationCache(provider)) => {
                let rows = count(&conn, &table)?;
                cleared.push(CacheRows {
                    table,
                    provider,
                    rows,
                });
            }
            Some(Class::Lost(what)) => {
                let rows = count(&conn, &table)?;
                lost.push(LostRows { table, what, rows });
            }
            None => {
                let rows = count(&conn, &table)?;
                lost.push(LostRows {
                    table,
                    what: table_classes::UNCLASSIFIED,
                    rows,
                });
            }
        }
    }
    Ok((lost, cleared))
}

fn has_table(conn: &Arc<Connection>, table: &str) -> Result<bool> {
    Ok(!rows(
        conn,
        &format!("SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = '{table}'"),
    )?
    .is_empty())
}

fn count(conn: &Arc<Connection>, table: &str) -> Result<u64> {
    let count = rows(conn, &format!("SELECT count(*) FROM \"{table}\""))?;
    match count.as_slice() {
        [row] => match row.as_slice() {
            [Value::Numeric(turso_core::Numeric::Integer(n))] => u64::try_from(*n)
                .map_err(|e| StorageError::DatabaseError(format!("count of {table}: {e}"))),
            other => Err(StorageError::DatabaseError(format!(
                "count of {table} is {other:?}"
            ))),
        },
        other => Err(StorageError::DatabaseError(format!(
            "count of {table} returned {} rows",
            other.len()
        ))),
    }
}

fn describe(lost: &[LostRows]) -> String {
    if lost.is_empty() {
        return "nothing".to_string();
    }
    lost.iter()
        .map(|l| format!("{} ({} rows: {})", l.table, l.rows, l.what))
        .collect::<Vec<_>>()
        .join(", ")
}

fn describe_caches(caches: &[CacheRows]) -> String {
    if caches.is_empty() {
        return "none".to_string();
    }
    caches
        .iter()
        .map(|c| format!("{} ({} rows, {})", c.table, c.rows, c.provider))
        .collect::<Vec<_>>()
        .join(", ")
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
    /// A torn or foreign record: the file is rebuilt, never refused.
    Unreadable(String),
}

fn stored_signature(conn: &Arc<Connection>) -> Result<StoredSignature> {
    let has_table = rows(
        conn,
        &format!("SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = '{FN_SET_TABLE}'"),
    )?;
    if has_table.is_empty() {
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
    run(&format!(
        "CREATE TABLE IF NOT EXISTS {FN_SET_TABLE} (signature TEXT NOT NULL)"
    ))?;
    run(&format!("DELETE FROM {FN_SET_TABLE}"))?;
    #[cfg(test)]
    if CRASH_AFTER_DELETE.load(std::sync::atomic::Ordering::SeqCst) {
        return Err(StorageError::DatabaseError(
            "injected crash between DELETE and INSERT".to_string(),
        ));
    }
    let insert = format!("INSERT INTO {FN_SET_TABLE} (signature) VALUES (?1)");
    let mut stmt = conn
        .prepare(&insert)
        .map_err(|e| StorageError::DatabaseError(format!("{insert}: {e}")))?;
    stmt.bind_at(
        1.try_into().expect("1 is non-zero"),
        Value::build_text(wanted),
    )
    .map_err(|e| StorageError::DatabaseError(format!("{insert}: bind: {e}")))?;
    stmt.run_ignore_rows()
        .map_err(|e| StorageError::DatabaseError(format!("{insert}: {e}")))?;
    run("COMMIT")
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

    #[test]
    fn a_database_built_with_another_function_set_is_rebuilt_at_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("holon.db");
        build_with_double(&path);

        let db = TursoBackend::open_database(&path).expect("open with this binary's set");
        let write = exec(
            &db,
            "CREATE TABLE IF NOT EXISTS t (id INTEGER PRIMARY KEY, n INTEGER)",
        )
        .and_then(|()| exec(&db, "INSERT INTO t (n) VALUES (3)"));
        assert!(
            write.is_ok(),
            "a database this binary cannot use must be rebuilt at open, but its writes are \
             refused: {write:?}"
        );
        assert_eq!(
            count(&db, "SELECT count(*) FROM t"),
            1,
            "the rebuilt database must start empty"
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
        assert_eq!(
            count(
                &db,
                &format!("SELECT count(*) FROM sqlite_schema WHERE name = '{FN_SET_TABLE}'")
            ),
            0,
            "the empty set needs no record"
        );
    }

    #[test]
    fn a_database_opened_by_a_larger_set_is_rebuilt_and_records_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("holon.db");
        {
            let db = open(&path, &[]).unwrap().0;
            exec(&db, "CREATE TABLE t (id INTEGER PRIMARY KEY)").unwrap();
        }
        let db = open(&path, &[DOUBLE]).unwrap().0;
        assert_eq!(
            count(&db, "SELECT count(*) FROM sqlite_schema WHERE name = 't'"),
            0,
            "a database built without the function must be rebuilt"
        );
        let conn = connect(&db).unwrap();
        assert!(matches!(
            stored_signature(&conn).unwrap(),
            StoredSignature::Recorded(s) if s == "holon_test_double/1/v1"
        ));
    }
    /// A crash inside the record write leaves the database openable: the
    /// transaction rolls back, and the next open records the set again.
    #[test]
    fn a_crash_while_recording_the_set_leaves_the_database_openable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("holon.db");
        build_with_double(&path);
        let double_v2 = ScalarFn {
            version: 2,
            ..DOUBLE
        };

        CRASH_AFTER_DELETE.store(true, std::sync::atomic::Ordering::SeqCst);
        let crashed = open(&path, &[double_v2]);
        CRASH_AFTER_DELETE.store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(crashed.is_err(), "the injected crash must fail that open");
        drop(crashed);

        let (db, _) = open(&path, &[double_v2])
            .unwrap_or_else(|e| panic!("the open after a crash must succeed: {e}"));
        let conn = connect(&db).unwrap();
        assert!(matches!(
            stored_signature(&conn).unwrap(),
            StoredSignature::Recorded(s) if s == "holon_test_double/1/v2"
        ));
    }

    /// A file an older binary built under the empty set cannot load a view that
    /// calls this binary's functions, so the first open of this binary rebuilds
    /// it.
    #[test]
    fn a_database_built_without_this_binarys_functions_is_rebuilt_at_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("holon.db");
        {
            let db = open(&path, &[]).unwrap().0;
            exec(&db, "CREATE TABLE t (id INTEGER PRIMARY KEY)").unwrap();
            exec(&db, "INSERT INTO t (id) VALUES (1)").unwrap();
        }
        let db = TursoBackend::open_database(&path).expect("open with this binary's set");
        assert_eq!(
            count(&db, "SELECT count(*) FROM sqlite_schema WHERE name = 't'"),
            0,
            "a database built without [{}] must be rebuilt",
            scalar_fns::signature(scalar_fns::ALL)
        );
        let conn = connect(&db).unwrap();
        assert!(matches!(
            stored_signature(&conn).unwrap(),
            StoredSignature::Recorded(s) if s == scalar_fns::signature(scalar_fns::ALL)
                && s.contains("holon_div/2/v2")
        ));
    }

    /// A crash between the DELETE and the INSERT of a non-atomic record leaves
    /// the table with no row; a table holding anything but one text row is as
    /// unreadable. Either way the file is derived data, so it is rebuilt.
    #[test]
    fn an_unreadable_signature_record_is_rebuilt_not_an_open_error() {
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
            for fns in [&[][..], &[DOUBLE][..]] {
                let (db, _) = open(&path, fns).unwrap_or_else(|e| {
                    panic!(
                        "{case}: an unreadable signature record must lead to a rebuild, not to \
                         an open error: {e}"
                    )
                });
                assert_eq!(
                    count(&db, "SELECT count(*) FROM sqlite_schema WHERE name = 't'"),
                    0,
                    "{case}: the database must be rebuilt"
                );
            }
        }
    }
}
