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
        matches!(opened, Ok(holon_turso::db_open::OpenOutcome::Kept)),
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
