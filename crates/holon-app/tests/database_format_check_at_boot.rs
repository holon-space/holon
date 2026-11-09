//! Whatever database file a boot finds, Holon boots, its blocks equal a fresh
//! ingest of the same vault, and a row only the database holds survives: in
//! the kept file, or in the backup of a file moved aside. Such a row lives in a
//! table the user created, since sync rebuilds every table it projects. Every
//! move-aside is disclosed with the backup's path, and a kept file raises no
//! such notice.
//!
//! Fixtures, compressed with zstd:
//! - `previous_format_owned_row.db.zst`: written by the engine of the previous
//!   pin, with that pin's format record, a recursive materialized view and one
//!   `advice_suppressed` row.
//! - `old_engine_dropped_new_view.db.zst`: a recursive materialized view
//!   created by this pin's engine, then dropped by the previous pin's engine;
//!   the engine refuses every later open of it.

use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use holon::api::BackendEngine;
use holon_api::Value;
use holon_frontend::FrontendSession;
use holon_frontend::config::HolonConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::config::VaultConfig;
use proptest::prelude::*;
use proptest::test_runner::TestCaseError;
use proptest::test_runner::TestRunner;

const PREVIOUS_FORMAT: &[u8] = include_bytes!("fixtures/previous_format_owned_row.db.zst");
const PREVIOUS_FORMAT_OWNED_TABLE: &str = "advice_suppressed";
const PREVIOUS_FORMAT_OWNED_ROW: (&str, &str) = ("block:a", "block:lesson");
const DROPPED_NEW_VIEW: &[u8] = include_bytes!("fixtures/old_engine_dropped_new_view.db.zst");
const PREVIOUS_VIEW_FORMAT: &str = "view-state/exact-real-sum";
const OWNED_TABLE: &str = "pbt_owned";
const OWNED_ROW: (&str, &str) = ("block:pbt-0", "block:pbt-lesson");

const DB_FILE: &str = "holon.db";
/// The file a later process finds: a copy, because the first session's engine
/// stays alive in this process.
const LATER_DB_FILE: &str = "later.db";

#[derive(Debug, Clone, Copy)]
enum DbState {
    Fresh,
    Current,
    PreviousFormatRecord,
    Unopenable,
    DroppedNewView,
    PreviousFormatFile,
}

#[derive(Debug, Clone)]
struct Case {
    /// (depth, content) per block, in document order.
    blocks: Vec<(usize, String)>,
    state: DbState,
    owned_row: bool,
}

fn vault_org(blocks: &[(usize, String)]) -> String {
    blocks
        .iter()
        .enumerate()
        .map(|(i, (depth, content))| {
            format!(
                "{} {content}\n:PROPERTIES:\n:ID: pbt-{i}\n:END:\n",
                "*".repeat(depth + 1)
            )
        })
        .collect()
}

fn case_strategy() -> impl Strategy<Value = Case> {
    let blocks = prop::collection::vec((0usize..3, "[a-z][a-z ]{0,10}[a-z]"), 1..6).prop_map(
        |mut blocks| {
            let mut previous = 0;
            for (depth, _) in blocks.iter_mut() {
                *depth = (*depth).min(previous + 1);
                previous = *depth;
            }
            blocks[0].0 = 0;
            blocks
        },
    );
    let state = prop_oneof![
        Just(DbState::Fresh),
        Just(DbState::Current),
        Just(DbState::PreviousFormatRecord),
        Just(DbState::Unopenable),
        Just(DbState::DroppedNewView),
        Just(DbState::PreviousFormatFile),
    ];
    (blocks, state, any::<bool>()).prop_map(|(blocks, state, owned_row)| Case {
        blocks,
        state,
        owned_row,
    })
}

struct Booted {
    engine: Arc<BackendEngine>,
    injector: fluxdi::Injector,
    _session: Arc<FrontendSession>,
}

async fn boot(dir: &Path, db_file: &str) -> Booted {
    let config = HolonConfig {
        db_path: Some(dir.join(db_file)),
        vault: VaultConfig {
            root: Some(dir.to_path_buf()),
        },
        ..Default::default()
    };
    let (session, engine, injector) = holon_app::new_from_config_with_di(
        config,
        SessionConfig::new(
            holon_api::UiInfo::permissive(),
            Arc::new(holon_api::SystemClock),
        ),
        dir.to_path_buf(),
        HashSet::new(),
        Arc::new(holon_conditions::ConditionBus::new()),
        |injector| {
            holon::testing::database_stuck_guard::report_database_stuck_in(injector);
            Ok(())
        },
        |injector| injector.clone(),
    )
    .await
    .unwrap_or_else(|e| panic!("boot on {}: {e:#}", dir.join(db_file).display()));
    Booted {
        engine,
        injector,
        _session: session,
    }
}

/// The vault's blocks as the `block` and `block_with_path` views hold them.
async fn vault_blocks(booted: &Booted) -> Vec<Vec<String>> {
    let sql = "SELECT b.id, b.parent_id, b.content, p.path FROM block b \
               JOIN block_with_path p ON p.id = b.id \
               WHERE b.id LIKE 'block:pbt-%' ORDER BY b.id";
    booted
        .engine
        .db_handle()
        .query(sql, Default::default())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
        .iter()
        .map(|row| {
            ["id", "parent_id", "content", "path"]
                .iter()
                .map(|col| match row.get(*col) {
                    Some(Value::String(s)) => s.clone(),
                    Some(Value::Null) | None => "NULL".to_string(),
                    other => panic!("{sql}: {col} is {other:?}"),
                })
                .collect()
        })
        .collect()
}

async fn owned_rows_live(booted: &Booted, table: &str) -> Vec<(String, String)> {
    let sql = format!("SELECT anchor_id, lesson_id FROM {table} ORDER BY anchor_id");
    booted
        .engine
        .db_handle()
        .query(&sql, Default::default())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
        .iter()
        .map(|row| match (row.get("anchor_id"), row.get("lesson_id")) {
            (Some(Value::String(a)), Some(Value::String(l))) => (a.clone(), l.clone()),
            other => panic!("{sql}: {other:?}"),
        })
        .collect()
}

/// Reads a backup with the bare engine: a Holon open would move it aside again.
fn owned_rows_in(backup: &Path, table: &str) -> Vec<(String, String)> {
    let io = Arc::new(turso_core::UnixIO::new().expect("file IO"));
    let options = turso_core::OpenOptions::new(Arc::new(turso_sqlite::SqliteDialect))
        .db_opts(turso_core::DatabaseOpts::default().with_views(true));
    let db = turso_core::Database::open(io, backup.to_str().unwrap(), options)
        .unwrap_or_else(|e| panic!("open the backup {}: {e}", backup.display()));
    let conn = db.connect(turso_core::ConnectOpts::new()).expect("connect");
    let sql = format!("SELECT anchor_id, lesson_id FROM {table} ORDER BY anchor_id");
    conn.prepare(&sql)
        .and_then(|mut stmt| stmt.run_collect_rows())
        .unwrap_or_else(|e| panic!("{sql} in {}: {e}", backup.display()))
        .into_iter()
        .map(|row| match row.as_slice() {
            [turso_core::Value::Text(a), turso_core::Value::Text(l)] => {
                (a.as_str().to_string(), l.as_str().to_string())
            }
            other => panic!("{sql} in {}: {other:?}", backup.display()),
        })
        .collect()
}

fn backups_in(dir: &Path, db_file: &str) -> Vec<PathBuf> {
    let prefix = format!("{db_file}.unusable-");
    std::fs::read_dir(dir)
        .expect("list the vault")
        .map(|e| e.expect("entry").path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(&prefix) && !n.ends_with("-wal"))
        })
        .collect()
}

fn moved_aside_notices(booted: &Booted) -> Vec<String> {
    booted
        .injector
        .resolve::<Arc<holon_conditions::ConditionBus>>()
        .subscribe()
        .current
        .iter()
        .filter(|c| c.condition_key().kind == "database-moved-aside")
        .map(|c| {
            let detail = c.reason.detail(&c.subject);
            format!("{} {}", detail.headline, detail.body.join(" | "))
        })
        .collect()
}

async fn plant_owned_row(booted: &Booted) {
    for sql in [
        format!("CREATE TABLE {OWNED_TABLE} (anchor_id TEXT, lesson_id TEXT)"),
        format!(
            "INSERT INTO {OWNED_TABLE} (anchor_id, lesson_id) VALUES ('{}', '{}')",
            OWNED_ROW.0, OWNED_ROW.1
        ),
    ] {
        booted
            .engine
            .db_handle()
            .execute(&sql, vec![])
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
}

/// Boots once on `DB_FILE`, then copies its files to `LATER_DB_FILE`.
/// Returns the first session's blocks: a fresh ingest of the vault.
async fn a_file_this_build_wrote(dir: &Path, owned_row: bool) -> Vec<Vec<String>> {
    let first = boot(dir, DB_FILE).await;
    let fresh = vault_blocks(&first).await;
    if owned_row {
        plant_owned_row(&first).await;
    }
    holon_app::shutdown_session(&first.injector)
        .await
        .expect("shut the first session down");
    for suffix in ["", "-wal"] {
        let from = dir.join(format!("{DB_FILE}{suffix}"));
        if from.exists() {
            std::fs::copy(&from, dir.join(format!("{LATER_DB_FILE}{suffix}")))
                .expect("copy the first session's database");
        }
    }
    fresh
}

/// The blocks of a fresh ingest of `org`, booted in a directory of its own.
async fn fresh_ingest(org: &str) -> Vec<Vec<String>> {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("pbt.org"), org).expect("write the vault");
    vault_blocks(&boot(dir.path(), DB_FILE).await).await
}

fn record_previous_format(path: &Path) {
    let db = holon::storage::turso::TursoBackend::open_database(path)
        .unwrap_or_else(|e| panic!("open {} between the sessions: {e}", path.display()));
    let conn = db.connect(turso_core::ConnectOpts::new()).expect("connect");
    let sql = format!(
        "UPDATE holon_db_fn_set SET signature = replace(signature, '{}', '{PREVIOUS_VIEW_FORMAT}')",
        holon::storage::db_open::VIEW_FORMAT
    );
    conn.execute(&sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
}

/// The state a case boots on.
struct Prepared {
    db_file: &'static str,
    owned_table: &'static str,
    /// The owned rows the file holds; the table exists only when it holds one.
    owned: Vec<(String, String)>,
    /// A fresh ingest of the vault; `None` when the boot itself is one.
    fresh: Option<Vec<Vec<String>>>,
}

/// Prepares the state in `dir`. A fresh ingest is booted only where no boot
/// of the preparation already is one, so each case costs two boots at most.
async fn prepare(dir: &Path, org: &str, case: &Case) -> Prepared {
    let planted = || match case.owned_row {
        true => vec![(OWNED_ROW.0.to_string(), OWNED_ROW.1.to_string())],
        false => vec![],
    };
    let fixture = |bytes: &[u8]| {
        std::fs::write(
            dir.join(LATER_DB_FILE),
            zstd::decode_all(bytes).expect("decompress fixture"),
        )
        .expect("write the fixture")
    };
    let prepared = |owned, fresh| Prepared {
        db_file: LATER_DB_FILE,
        owned_table: OWNED_TABLE,
        owned,
        fresh,
    };
    match case.state {
        DbState::Fresh => prepared(vec![], None),
        DbState::Current => {
            let fresh = a_file_this_build_wrote(dir, case.owned_row).await;
            prepared(planted(), Some(fresh))
        }
        DbState::PreviousFormatRecord => {
            let fresh = a_file_this_build_wrote(dir, case.owned_row).await;
            record_previous_format(&dir.join(LATER_DB_FILE));
            prepared(planted(), Some(fresh))
        }
        DbState::Unopenable => {
            std::fs::write(
                dir.join(LATER_DB_FILE),
                b"not a database, but the only copy of something".repeat(100),
            )
            .expect("write an unopenable file");
            prepared(vec![], Some(fresh_ingest(org).await))
        }
        DbState::DroppedNewView => {
            fixture(DROPPED_NEW_VIEW);
            prepared(vec![], Some(fresh_ingest(org).await))
        }
        DbState::PreviousFormatFile => {
            fixture(PREVIOUS_FORMAT);
            Prepared {
                owned_table: PREVIOUS_FORMAT_OWNED_TABLE,
                ..prepared(
                    vec![(
                        PREVIOUS_FORMAT_OWNED_ROW.0.to_string(),
                        PREVIOUS_FORMAT_OWNED_ROW.1.to_string(),
                    )],
                    Some(fresh_ingest(org).await),
                )
            }
        }
    }
}

async fn check(case: Case) -> Result<(), TestCaseError> {
    let org = vault_org(&case.blocks);
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("pbt.org"), &org).expect("write the vault");
    let Prepared {
        db_file,
        owned_table,
        owned,
        fresh,
    } = prepare(dir.path(), &org, &case).await;
    let unopenable = std::fs::read(dir.path().join(db_file)).ok();
    let booted = boot(dir.path(), db_file).await;

    let blocks = vault_blocks(&booted).await;
    prop_assert_eq!(
        blocks.len(),
        case.blocks.len(),
        "a boot on {:?} holds {:?} for the vault {:?}",
        case.state,
        blocks,
        org
    );
    if let Some(fresh) = fresh {
        prop_assert_eq!(
            blocks,
            fresh,
            "the blocks after a boot on {:?} differ from a fresh ingest",
            case.state
        );
    }
    let backups = backups_in(dir.path(), db_file);
    let notices = moved_aside_notices(&booted);
    let moved = matches!(
        case.state,
        DbState::PreviousFormatRecord
            | DbState::Unopenable
            | DbState::DroppedNewView
            | DbState::PreviousFormatFile
    );
    match backups.as_slice() {
        [] => {
            prop_assert!(!moved, "{:?}: nothing was moved aside", case.state);
            prop_assert_eq!(
                notices,
                Vec::<String>::new(),
                "a kept file raises no move notice"
            );
            if !owned.is_empty() {
                prop_assert_eq!(
                    owned_rows_live(&booted, owned_table).await,
                    owned,
                    "the kept file lost an owned row"
                );
            }
        }
        [backup] => {
            prop_assert!(
                moved,
                "{:?}: {} was moved aside",
                case.state,
                backup.display()
            );
            prop_assert!(
                notices.len() == 1 && notices[0].contains(&backup.display().to_string()),
                "one notice names {}: {notices:?}",
                backup.display()
            );
            match case.state {
                DbState::Unopenable => prop_assert_eq!(
                    std::fs::read(backup).ok(),
                    unopenable,
                    "the backup holds the unopenable bytes"
                ),
                DbState::DroppedNewView => {}
                _ if owned.is_empty() => {}
                _ => prop_assert_eq!(
                    owned_rows_in(backup, owned_table),
                    owned,
                    "the backup {} lost an owned row",
                    backup.display()
                ),
            }
        }
        more => prop_assert!(false, "more than one backup: {more:?}"),
    }
    Ok(())
}

fn run(case: Case) -> Result<(), TestCaseError> {
    tokio::runtime::Runtime::new()
        .expect("runtime")
        .block_on(check(case))
}

#[test]
fn every_database_state_boots_to_the_vault_and_keeps_owned_rows() {
    let fixed = |state| Case {
        blocks: vec![(0, "a page".to_string()), (1, "a child".to_string())],
        state,
        owned_row: false,
    };
    for case in [
        fixed(DbState::DroppedNewView),
        fixed(DbState::PreviousFormatFile),
        Case {
            blocks: vec![(0, "aa".to_string())],
            state: DbState::Current,
            owned_row: true,
        },
    ] {
        run(case.clone()).unwrap_or_else(|e| panic!("static example {case:?}: {e}"));
    }
    TestRunner::new(ProptestConfig {
        cases: 6,
        failure_persistence: None,
        ..ProptestConfig::default()
    })
    .run(&case_strategy(), run)
    .unwrap_or_else(|e| panic!("{e}"));
}
