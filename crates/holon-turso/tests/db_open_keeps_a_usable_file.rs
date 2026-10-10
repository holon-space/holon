//! An open that fails for a reason other than a damaged file leaves the file
//! where it is: only corruption moves a database aside.
//!
//! The lock case needs a second process, because the engine's file lock is a
//! per-process `fcntl` lock and shares one `Database` per file in a process.
//! The test re-runs its own binary as that process.

use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use holon_core::storage::types::DatabaseOpenFailure;
use holon_core::storage::types::StorageError;
use holon_turso::db_open::OpenOutcome;
use holon_turso::turso::TursoBackend;
use turso_core::Database;

const CHILD_DB_PATH: &str = "HOLON_DB_OPEN_TEST_CHILD_DB_PATH";

fn exec(db: &Arc<Database>, sql: &str) {
    db.connect()
        .unwrap_or_else(|e| panic!("connect: {e}"))
        .execute(sql)
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
}

fn backups(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.contains(".unusable-"))
        })
        .collect()
}

fn build(path: &Path) {
    let (db, _) = TursoBackend::open_database_reporting_outcome(path).expect("create");
    exec(&db, "CREATE TABLE t (id INTEGER PRIMARY KEY)");
    exec(&db, "INSERT INTO t (id) VALUES (1)");
}

/// Process B: the production open of a file process A holds.
fn run_as_child(path: &str) -> ! {
    match TursoBackend::open_database_reporting_outcome(path) {
        Ok((_, outcome)) => println!("CHILD-OPENED {outcome:?}"),
        Err(StorageError::DatabaseUnopenable { failure, .. }) => {
            println!("CHILD-UNOPENABLE {failure:?}")
        }
        Err(e) => println!("CHILD-ERROR {e:?}"),
    }
    std::process::exit(0);
}

#[test]
fn a_second_process_does_not_move_a_live_database_aside() {
    if let Ok(path) = std::env::var(CHILD_DB_PATH) {
        run_as_child(&path);
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("holon.db");
    let (a, _) = TursoBackend::open_database_reporting_outcome(&path).expect("process A opens");
    exec(&a, "CREATE TABLE t (id INTEGER PRIMARY KEY)");
    exec(&a, "INSERT INTO t (id) VALUES (1)");
    let inode = std::fs::metadata(&path).unwrap().ino();

    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "a_second_process_does_not_move_a_live_database_aside",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_DB_PATH, &path)
        .output()
        .expect("spawn process B");
    let stdout = String::from_utf8_lossy(&child.stdout);
    let report = stdout
        .lines()
        .find_map(|l| l.find("CHILD-").map(|at| &l[at..]))
        .unwrap_or_else(|| {
            panic!(
                "process B printed no result; stdout:\n{stdout}\nstderr:\n{}",
                String::from_utf8_lossy(&child.stderr)
            )
        });
    println!("process B: {report}");

    exec(&a, "INSERT INTO t (id) VALUES (2)");
    assert_eq!(
        (std::fs::metadata(&path).unwrap().ino(), backups(dir.path())),
        (inode, vec![]),
        "process A's live file must stay at its path, with no backup beside it; B: {report}"
    );
    assert!(
        report == "CHILD-UNOPENABLE Locked",
        "process B must get a Locked open error, got: {report}"
    );
}

#[test]
fn a_read_only_database_file_is_not_moved_aside() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("holon.db");
    build(&path);
    let bytes = std::fs::read(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();

    let opened = TursoBackend::open_database_reporting_outcome(&path).map(|(_, o)| o);
    let left = (
        std::fs::read(&path).ok() == Some(bytes),
        backups(dir.path()),
    );
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        left,
        (true, vec![]),
        "a file without write permission must stay at its path, unchanged; open gave {opened:?}"
    );
    assert!(
        matches!(
            opened,
            Err(StorageError::DatabaseUnopenable {
                failure: DatabaseOpenFailure::Io(std::io::ErrorKind::PermissionDenied),
                ..
            })
        ),
        "the open must name a permission error, got: {opened:?}"
    );
}

#[test]
fn a_database_in_a_read_only_directory_opens_where_it_is() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("holon.db");
    build(&path);
    let before: Vec<_> = {
        let mut v: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        v.sort();
        v
    };
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o555)).unwrap();

    let opened = TursoBackend::open_database_reporting_outcome(&path).map(|(_, o)| o);
    let mut after: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    after.sort();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        after, before,
        "no file may be renamed; open gave {opened:?}"
    );
    assert!(
        matches!(opened, Ok(OpenOutcome::Kept)),
        "a database whose files are writable opens in a read-only directory, got {opened:?}"
    );
}

#[test]
fn a_database_in_a_missing_directory_is_an_io_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gone").join("holon.db");

    let opened = TursoBackend::open_database_reporting_outcome(&path).map(|(_, o)| o);
    assert!(
        matches!(
            opened,
            Err(StorageError::DatabaseUnopenable {
                failure: DatabaseOpenFailure::Io(std::io::ErrorKind::NotFound),
                ..
            })
        ),
        "the open must name a missing directory, got: {opened:?}"
    );
}

/// The open reports `MovedAside`, and the backup beside the database holds
/// `files` byte for byte, keyed by suffix (`""`, `"-wal"`).
fn assert_moved_aside(path: &Path, files: &[(&str, Vec<u8>)], case: &str) {
    let opened = TursoBackend::open_database_reporting_outcome(path);
    let (db, outcome) = opened.unwrap_or_else(|e| panic!("{case}: the open must recover: {e:?}"));
    let OpenOutcome::MovedAside { backup, .. } = outcome else {
        panic!("{case}: a damaged file must be moved aside, got {outcome:?}");
    };
    for (suffix, bytes) in files {
        let moved = PathBuf::from(format!("{}{suffix}", backup.display()));
        assert!(
            std::fs::read(&moved).ok().as_ref() == Some(bytes),
            "{case}: {} must hold the original bytes",
            moved.display()
        );
    }
    exec(&db, "CREATE TABLE fresh (id INTEGER PRIMARY KEY)");
}

#[test]
fn a_file_that_is_not_a_database_is_moved_aside() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("holon.db");
    let garbage = b"not a database".repeat(400);
    std::fs::write(&path, &garbage).unwrap();
    assert_moved_aside(&path, &[("", garbage)], "garbage");
}

#[test]
fn a_truncated_database_is_moved_aside() {
    for keep in [200, 1024] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("holon.db");
        build(&path);
        let bytes = std::fs::read(&path).unwrap();
        assert!(
            bytes.len() > keep,
            "the built file has {} bytes",
            bytes.len()
        );
        std::fs::write(&path, &bytes[..keep]).unwrap();
        assert_moved_aside(
            &path,
            &[("", bytes[..keep].to_vec())],
            &format!("truncated to {keep} bytes"),
        );
    }
}

/// `build`, then fold the log into the main file, so the main file alone
/// holds the row.
fn build_checkpointed(path: &Path) {
    build(path);
    let (db, _) = TursoBackend::open_database_reporting_outcome(path).expect("reopen");
    exec(&db, "PRAGMA wal_checkpoint(TRUNCATE)");
}

fn rows_in_t(db: &Arc<Database>) -> Vec<Vec<turso_core::Value>> {
    db.connect()
        .unwrap()
        .prepare("SELECT id FROM t")
        .and_then(|mut stmt| stmt.run_collect_rows())
        .unwrap_or_else(|e| panic!("SELECT id FROM t: {e}"))
}

fn single_row() -> Vec<Vec<turso_core::Value>> {
    vec![vec![turso_core::Value::Numeric(
        turso_core::Numeric::Integer(1),
    )]]
}

/// A damaged log costs only the commits it held: the intact main file stays
/// where it is, and the open says the log was moved aside.
#[test]
fn a_damaged_write_ahead_log_is_moved_aside_alone() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("holon.db");
    build_checkpointed(&path);
    let inode = std::fs::metadata(&path).unwrap().ino();
    let wal_bytes = b"garbage wal".repeat(50);
    std::fs::write(dir.path().join("holon.db-wal"), &wal_bytes).unwrap();

    let (db, outcome) = TursoBackend::open_database_reporting_outcome(&path)
        .unwrap_or_else(|e| panic!("a damaged log must not stop the open: {e:?}"));
    let OpenOutcome::LogMovedAside { backup, .. } = outcome else {
        panic!("the open must report the moved log, got {outcome:?}");
    };
    assert!(
        std::fs::read(&backup).ok() == Some(wal_bytes),
        "{} must hold the damaged log",
        backup.display()
    );
    assert_eq!(
        (std::fs::metadata(&path).unwrap().ino(), rows_in_t(&db)),
        (inode, single_row()),
        "the intact main file must stay at its path with its checkpointed row"
    );
}

/// The shared-WAL coordination file holds no rows. A damaged one is moved
/// aside alone; the engine does not need it to open a database that no
/// multi-process writer holds, and does not create it again.
#[test]
fn a_damaged_coordination_file_is_moved_aside_alone() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("holon.db");
    build_checkpointed(&path);
    let inode = std::fs::metadata(&path).unwrap().ino();
    let tshm = dir.path().join("holon.db-tshm");
    assert!(!tshm.exists(), "a Holon open creates no coordination file");
    let garbage = b"garbage tshm".repeat(33);
    std::fs::write(&tshm, &garbage).unwrap();

    let (db, outcome) = TursoBackend::open_database_reporting_outcome(&path)
        .unwrap_or_else(|e| panic!("a damaged coordination file must not stop the open: {e:?}"));
    assert_eq!(outcome, OpenOutcome::Kept);
    assert_eq!(
        (std::fs::metadata(&path).unwrap().ino(), rows_in_t(&db)),
        (inode, single_row()),
        "the database must stay at its path with its row"
    );
    let moved: Vec<_> = backups(dir.path());
    match moved.as_slice() {
        [only] if only.to_string_lossy().ends_with("-tshm") => assert!(
            std::fs::read(only).ok() == Some(garbage),
            "{} must hold the damaged coordination file",
            only.display()
        ),
        other => panic!("only the coordination file may be moved aside, found {other:?}"),
    }
    assert!(
        !tshm.exists(),
        "the engine must not create the coordination file again"
    );
}

/// A valid SQLite file in an encoding the engine does not support is not
/// damaged: it stays, and the open names the cause.
#[test]
fn a_database_in_another_text_encoding_is_kept_and_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("holon.db");
    build_checkpointed(&path);
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[56..60].copy_from_slice(&2u32.to_be_bytes());
    std::fs::write(&path, &bytes).unwrap();

    let opened = TursoBackend::open_database_reporting_outcome(&path).map(|(_, o)| o);
    assert_eq!(
        (
            std::fs::read(&path).ok() == Some(bytes),
            backups(dir.path())
        ),
        (true, vec![]),
        "a UTF-16 database must stay at its path, unchanged; open gave {opened:?}"
    );
    match opened {
        Err(StorageError::DatabaseUnopenable {
            failure: DatabaseOpenFailure::Unclassified,
            cause,
            ..
        }) => assert!(
            cause.contains("UTF-16"),
            "the cause must name the encoding: {cause}"
        ),
        other => panic!("the open must refuse with an unclassified error, got {other:?}"),
    }
}

/// A log cut short by a crash drops the commits it held, and the open says
/// nothing about it.
#[test]
#[ignore = "holon-work #155 truncated WAL: a truncated or empty WAL silently loses committed rows and the open reports Kept; tracked as its own task"]
fn a_truncated_write_ahead_log_does_not_lose_rows_silently() {
    for keep in [0usize, 40] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("holon.db");
        build(&path);
        let wal = dir.path().join("holon.db-wal");
        let bytes = std::fs::read(&wal).unwrap();
        assert!(bytes.len() > keep, "the log holds {} bytes", bytes.len());
        std::fs::write(&wal, &bytes[..keep]).unwrap();

        let (db, outcome) = TursoBackend::open_database_reporting_outcome(&path)
            .unwrap_or_else(|e| panic!("log cut to {keep} bytes: {e:?}"));
        let rows = db
            .connect()
            .unwrap()
            .prepare("SELECT id FROM t")
            .and_then(|mut stmt| stmt.run_collect_rows());
        assert!(
            outcome != OpenOutcome::Kept || rows.as_ref().is_ok_and(|r| r.len() == 1),
            "log cut to {keep} bytes: the open reported Kept, but the committed row is gone \
             ({rows:?})"
        );
    }
}
