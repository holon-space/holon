//! Whatever database file a boot finds, Holon boots, its blocks equal a fresh
//! ingest of the same vault, and every row a rebuild keeps survives: in the
//! kept file, and for a file moved aside both in its backup and in the rebuilt
//! copy, wherever the engine opens the file. Such rows live in every table
//! nothing else restores (the `Lost` class) and in a table the user created.
//! Derived rows of a moved file do not survive. Every move-aside is disclosed
//! with the backup's path together with one notice that says whether the
//! owned state was kept or may be lost; a kept file raises no such notice, and
//! a second boot on the result moves nothing.
//!
//! Fixtures, compressed with zstd:
//! - `previous_format_owned_row.db.zst`: written by the engine of the previous
//!   pin, with that pin's format record, a recursive materialized view and one
//!   `advice_suppressed` row.
//! - `old_engine_dropped_new_view.db.zst`: a recursive materialized view
//!   created by this pin's engine, then dropped by the previous pin's engine;
//!   the engine refuses every later open of it.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use holon::api::BackendEngine;
use holon::storage::table_classes;
use holon::storage::table_classes::Class;
use holon_api::Value;
use holon_frontend::FrontendSession;
use holon_frontend::config::HolonConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::config::VaultConfig;
use proptest::prelude::*;
use proptest::test_runner::TestCaseError;
use proptest::test_runner::TestRunner;

const PREVIOUS_FORMAT: &[u8] = include_bytes!("fixtures/previous_format_owned_row.db.zst");
const DROPPED_NEW_VIEW: &[u8] = include_bytes!("fixtures/old_engine_dropped_new_view.db.zst");
const PREVIOUS_VIEW_FORMAT: &str = "view-state/exact-real-sum";
const OWNED_TABLE: &str = "pbt_owned";
const OWNED_ROW: (&str, &str) = ("block:pbt-0", "block:pbt-lesson");

const DB_FILE: &str = "holon.db";
/// The file a later process finds: a copy, because the first session's engine
/// stays alive in this process.
const LATER_DB_FILE: &str = "later.db";
/// A copy of the booted file, for the second boot.
const AGAIN_DB_FILE: &str = "again.db";
/// The text and integer every planted row holds.
const MARK: &str = "pbt-mark";
/// A derived block no vault file backs; planted only in moved files.
const STALE_BLOCK: &str = "block:pbt-stale";
const MARK_INT: i64 = 7_000_000_000_001;

#[derive(Debug, Clone, Copy)]
enum DbState {
    Fresh,
    Current,
    PreviousFormatRecord,
    Unopenable,
    DroppedNewView,
    PreviousFormatFile,
}

const ALL_STATES: [DbState; 6] = [
    DbState::Fresh,
    DbState::Current,
    DbState::PreviousFormatRecord,
    DbState::Unopenable,
    DbState::DroppedNewView,
    DbState::PreviousFormatFile,
];

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
    (blocks, prop::sample::select(&ALL_STATES[..]), any::<bool>()).prop_map(
        |(blocks, state, owned_row)| Case {
            blocks,
            state,
            owned_row,
        },
    )
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

fn bare_open(path: &Path) -> Arc<turso_core::Connection> {
    let io = Arc::new(turso_core::UnixIO::new().expect("file IO"));
    let options = turso_core::OpenOptions::new(Arc::new(turso_sqlite::SqliteDialect))
        .db_opts(turso_core::DatabaseOpts::default().with_views(true));
    let db = turso_core::Database::open(io, path.to_str().unwrap(), options)
        .unwrap_or_else(|e| panic!("open {}: {e}", path.display()));
    db.connect(turso_core::ConnectOpts::new()).expect("connect")
}

fn bare_rows(conn: &Arc<turso_core::Connection>, sql: &str) -> Vec<Vec<turso_core::Value>> {
    conn.prepare(sql)
        .and_then(|mut stmt| stmt.run_collect_rows())
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
}

fn bare_texts(conn: &Arc<turso_core::Connection>, sql: &str) -> Vec<String> {
    bare_rows(conn, sql)
        .into_iter()
        .map(|row| match row.as_slice() {
            [turso_core::Value::Text(t), ..] => t.as_str().to_string(),
            other => panic!("{sql}: {other:?}"),
        })
        .collect()
}

/// A table of the file with what a rebuild does with it, and its columns as
/// (name, declared type).
struct Table {
    name: String,
    class: Option<Class>,
    /// A rebuild keeps it, by the rule written here, not by the code under
    /// test.
    kept: bool,
    columns: Vec<(String, String)>,
}

impl Table {
    /// Every row as one text of its values quoted by SQL.
    fn rows_sql(&self) -> String {
        let quoted: Vec<String> = self
            .columns
            .iter()
            .map(|(column, _)| format!("quote(\"{column}\")"))
            .collect();
        format!(
            "SELECT {} AS r FROM \"{}\" ORDER BY 1",
            quoted.join(" || '|' || "),
            self.name
        )
    }
}

fn tables_of(conn: &Arc<turso_core::Connection>) -> Vec<Table> {
    let names = bare_texts(
        conn,
        "SELECT name FROM sqlite_schema WHERE type = 'table' ORDER BY name",
    );
    let caches: HashMap<String, String> = match names.iter().any(|n| n == "integration_cache") {
        true => bare_rows(conn, "SELECT table_name, provider FROM integration_cache")
            .into_iter()
            .map(|row| match row.as_slice() {
                [turso_core::Value::Text(t), turso_core::Value::Text(p)] => {
                    (t.as_str().to_string(), p.as_str().to_string())
                }
                other => panic!("integration_cache row {other:?}"),
            })
            .collect(),
        false => HashMap::new(),
    };
    names
        .into_iter()
        .filter(|name| !name.starts_with("sqlite_") && !name.starts_with("__turso_internal_"))
        .map(|name| {
            let columns = bare_rows(conn, &format!("PRAGMA table_info(\"{name}\")"))
                .into_iter()
                .map(|row| match &row[1..3] {
                    [
                        turso_core::Value::Text(column),
                        turso_core::Value::Text(kind),
                    ] => (
                        column.as_str().to_string(),
                        kind.as_str().to_ascii_uppercase(),
                    ),
                    other => panic!("table_info of {name}: {other:?}"),
                })
                .collect();
            let class = table_classes::class_of(&name, &caches);
            Table {
                // Owned and unclassified tables stay; sync tokens go with the caches.
                kept: match &class {
                    Some(Class::Lost(_)) => name != "sync_states",
                    None => true,
                    Some(Class::Rebuilt) | Some(Class::IntegrationCache(_)) => false,
                },
                class,
                name,
                columns,
            }
        })
        .collect()
}

/// Each table a rebuild keeps: the SQL that reads its rows, and the rows.
type KeptRows = BTreeMap<String, (String, Vec<String>)>;

/// Reads a file with the bare engine: a Holon open would move it aside again.
fn kept_rows_in(path: &Path) -> KeptRows {
    let conn = bare_open(path);
    tables_of(&conn)
        .into_iter()
        .filter(|table| table.kept)
        .map(|table| {
            let sql = table.rows_sql();
            let rows = bare_texts(&conn, &sql);
            (table.name, (sql, rows))
        })
        .collect()
}

async fn live_texts(booted: &Booted, sql: &str) -> Vec<String> {
    booted
        .engine
        .db_handle()
        .query(sql, Default::default())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
        .iter()
        .map(|row| match row.values().next() {
            Some(Value::String(t)) => t.clone(),
            other => panic!("{sql}: {other:?}"),
        })
        .collect()
}

async fn live_tables(booted: &Booted) -> Vec<String> {
    live_texts(
        booted,
        "SELECT name FROM sqlite_schema WHERE type = 'table' ORDER BY name",
    )
    .await
}

/// The rows of `expected` the live database does not hold, per table,
/// counting duplicates.
async fn missing_live(booted: &Booted, expected: &KeptRows) -> BTreeMap<String, Vec<String>> {
    let mut missing = BTreeMap::new();
    for (table, (sql, rows)) in expected {
        let mut live = live_texts(booted, sql).await;
        let gone: Vec<String> = rows
            .iter()
            .filter(|row| match live.iter().position(|l| l == *row) {
                Some(at) => {
                    live.swap_remove(at);
                    false
                }
                None => true,
            })
            .cloned()
            .collect();
        if !gone.is_empty() {
            missing.insert(table.clone(), gone);
        }
    }
    missing
}

/// A planted derived row of a moved file: its table, the SQL that reads its
/// rows, and the row.
type StaleRow = (String, String, String);

/// Plants, with the bare engine, one marked row in every `Lost` table of the
/// file this build wrote, and two in `advice_suppressed`: one anchored at the
/// stale block (which exists with `stale`) and one whose anchor is gone. With
/// `stale`, also derived rows the rebuild must drop: a `block_raw` row no vault
/// block backs, a `file` row no vault file backs, and a cache table of an
/// integration with a row. Returns the derived rows.
fn plant_rows(path: &Path, stale: bool) -> Vec<StaleRow> {
    let conn = bare_open(path);
    if stale {
        let sql = "CREATE TABLE gcal_pbt (id TEXT PRIMARY KEY)";
        conn.execute(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    let insert = |table: &Table, overrides: &[(&str, &str)]| {
        let values: Vec<String> = table
            .columns
            .iter()
            .map(
                |(column, kind)| match overrides.iter().find(|(name, _)| name == column) {
                    Some((_, value)) => format!("'{value}'"),
                    None if kind.contains("INT") => MARK_INT.to_string(),
                    None if kind.contains("REAL") || kind.contains("FLOA") => "0.5".to_string(),
                    None => format!("'{MARK}'"),
                },
            )
            .collect();
        let sql = format!(
            "INSERT INTO \"{}\" VALUES ({})",
            table.name,
            values.join(", ")
        );
        conn.execute(&sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
        // `quote()` renders these literals as written.
        values.join("|")
    };
    // Older files still hold Lost tables this build no longer creates.
    let existing = bare_texts(&conn, "SELECT name FROM sqlite_schema WHERE type = 'table'");
    for name in table_classes::lost_tables().filter(|name| !existing.iter().any(|n| n == name)) {
        let sql = format!("CREATE TABLE \"{name}\" (mark TEXT)");
        conn.execute(&sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    let mut stale_rows = Vec::new();
    for table in tables_of(&conn) {
        match (&table.class, table.name.as_str()) {
            (Some(Class::Lost(_)), "advice_suppressed") => {
                // The app parses `lesson_id` as a block URI. A vault block's
                // suppressions follow its org drawer, so no anchor is a vault block.
                let lesson = ("lesson_id", "block:pbt-mark");
                insert(&table, &[("anchor_id", STALE_BLOCK), lesson]);
                insert(&table, &[("anchor_id", "block:pbt-gone"), lesson]);
            }
            (Some(Class::Lost(_)), _) => {
                let row = insert(&table, &[]);
                if stale && !table.kept {
                    stale_rows.push((table.name.clone(), table.rows_sql(), row));
                }
            }
            (Some(Class::Rebuilt), "block_raw") if stale => {
                // Views over `block_raw` parse these columns as JSON.
                let json = [
                    ("properties", "{}"),
                    ("property_kinds", "{}"),
                    ("marks", "[]"),
                ];
                let row = insert(&table, &[&[("id", STALE_BLOCK)][..], &json].concat());
                stale_rows.push((table.name.clone(), table.rows_sql(), row));
            }
            (Some(Class::Rebuilt), "file") if stale => {
                let json = [
                    ("properties", "{}"),
                    ("property_kinds", "{}"),
                    ("read_only_blocks", "[]"),
                ];
                let row = insert(
                    &table,
                    &[&[("id", "file:pbt-stale.org")][..], &json].concat(),
                );
                stale_rows.push((table.name.clone(), table.rows_sql(), row));
            }
            (Some(Class::IntegrationCache(_)), "gcal_pbt") => {
                let row = insert(&table, &[]);
                stale_rows.push((table.name.clone(), table.rows_sql(), row));
            }
            _ => {}
        }
    }
    stale_rows
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

const MOVED_ASIDE: &str = "database-moved-aside";
const OWNED_STATE_KEPT: &str = "database-owned-state-kept";
const OWNED_STATE_LOST: &str = "database-owned-state-lost";

/// The rendered notices of the boot's database-open kinds, as (kind, text).
fn open_notices(booted: &Booted) -> Vec<(String, String)> {
    booted
        .injector
        .resolve::<Arc<holon_conditions::ConditionBus>>()
        .subscribe()
        .current
        .iter()
        .filter(|c| {
            [MOVED_ASIDE, OWNED_STATE_KEPT, OWNED_STATE_LOST].contains(&c.condition_key().kind)
        })
        .map(|c| {
            let detail = c.reason.detail(&c.subject);
            (
                c.condition_key().kind.to_string(),
                format!("{} {}", detail.headline, detail.body.join(" | ")),
            )
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
    /// The rows a rebuild keeps, read before the boot from a file the engine
    /// opens; `None` for no file or one the engine cannot open.
    kept: Option<KeptRows>,
    /// Planted derived rows the boot must not keep.
    stale: Vec<StaleRow>,
    /// A fresh ingest of the vault; `None` when the boot itself is one.
    fresh: Option<Vec<Vec<String>>>,
}

/// Prepares the state in `dir`. A fresh ingest is booted only where no boot
/// of the preparation already is one, so each case costs two boots at most
/// before the boot under test.
async fn prepare(dir: &Path, org: &str, case: &Case) -> Prepared {
    let later = dir.join(LATER_DB_FILE);
    let fixture = |bytes: &[u8]| {
        std::fs::write(&later, zstd::decode_all(bytes).expect("decompress fixture"))
            .expect("write the fixture")
    };
    let prepared = |kept, stale, fresh| Prepared {
        db_file: LATER_DB_FILE,
        kept,
        stale,
        fresh,
    };
    match case.state {
        DbState::Fresh => prepared(None, vec![], None),
        DbState::Current => {
            let fresh = a_file_this_build_wrote(dir, case.owned_row).await;
            plant_rows(&later, false);
            prepared(Some(kept_rows_in(&later)), vec![], Some(fresh))
        }
        DbState::PreviousFormatRecord => {
            let fresh = a_file_this_build_wrote(dir, case.owned_row).await;
            let stale = plant_rows(&later, true);
            record_previous_format(&later);
            prepared(Some(kept_rows_in(&later)), stale, Some(fresh))
        }
        DbState::Unopenable => {
            std::fs::write(
                &later,
                b"not a database, but the only copy of something".repeat(100),
            )
            .expect("write an unopenable file");
            prepared(None, vec![], Some(fresh_ingest(org).await))
        }
        DbState::DroppedNewView => {
            fixture(DROPPED_NEW_VIEW);
            prepared(None, vec![], Some(fresh_ingest(org).await))
        }
        DbState::PreviousFormatFile => {
            fixture(PREVIOUS_FORMAT);
            prepared(
                Some(kept_rows_in(&later)),
                vec![],
                Some(fresh_ingest(org).await),
            )
        }
    }
}

async fn check(case: Case) -> Result<(), TestCaseError> {
    let org = vault_org(&case.blocks);
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("pbt.org"), &org).expect("write the vault");
    let Prepared {
        db_file,
        kept,
        stale,
        fresh,
    } = prepare(dir.path(), &org, &case).await;
    let before = std::fs::read(dir.path().join(db_file)).ok();
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
    if let Some(kept) = &kept {
        prop_assert_eq!(
            missing_live(&booted, kept).await,
            BTreeMap::new(),
            "{:?}: rows a rebuild keeps are missing from the live database",
            case.state
        );
    }
    let tables = live_tables(&booted).await;
    for (table, sql, row) in &stale {
        if tables.contains(table) {
            prop_assert!(
                !live_texts(&booted, sql).await.contains(row),
                "{:?}: the derived row {} of {} survived the move-aside",
                case.state,
                row,
                table
            );
        }
    }
    prop_assert_eq!(
        holon_turso::dbsp_state::left_behind_in(booted.engine.db_handle()).await,
        Vec::<String>::new(),
        "{:?}: DBSP state of a dropped view is left behind",
        case.state
    );
    let backups = backups_in(dir.path(), db_file);
    let notices = open_notices(&booted);
    let kinds: Vec<&str> = notices.iter().map(|(kind, _)| kind.as_str()).collect();
    let owned_kind = match case.state {
        DbState::Fresh | DbState::Current => None,
        DbState::PreviousFormatRecord | DbState::PreviousFormatFile => Some(OWNED_STATE_KEPT),
        DbState::Unopenable | DbState::DroppedNewView => Some(OWNED_STATE_LOST),
    };
    match (backups.as_slice(), owned_kind) {
        ([], None) => prop_assert_eq!(
            kinds,
            Vec::<&str>::new(),
            "a kept file raises no move notice"
        ),
        ([backup], Some(owned_kind)) => {
            let path = backup.display().to_string();
            prop_assert_eq!(
                kinds,
                vec![MOVED_ASIDE, owned_kind],
                "{:?}: one move notice and one owned-state notice",
                case.state
            );
            prop_assert!(
                notices.iter().all(|(_, text)| text.contains(&path)),
                "every notice names {}: {:?}",
                path,
                notices
            );
            prop_assert_eq!(
                std::fs::read(backup).ok(),
                before,
                "the backup {} holds the bytes the boot found",
                path
            );
            if let Some(kept) = &kept {
                prop_assert_eq!(
                    &kept_rows_in(backup),
                    kept,
                    "the backup {} lost a row",
                    path
                );
            }
            holon_app::shutdown_session(&booted.injector)
                .await
                .expect("shut the session down");
            for suffix in ["", "-wal"] {
                let from = dir.path().join(format!("{db_file}{suffix}"));
                if from.exists() {
                    std::fs::copy(&from, dir.path().join(format!("{AGAIN_DB_FILE}{suffix}")))
                        .expect("copy the booted database");
                }
            }
            let again = boot(dir.path(), AGAIN_DB_FILE).await;
            prop_assert_eq!(
                (backups_in(dir.path(), AGAIN_DB_FILE), open_notices(&again)),
                (vec![], vec![]),
                "{:?}: a second boot on the result moves nothing",
                case.state
            );
        }
        (backups, _) => prop_assert!(
            false,
            "{:?}: backups {:?}, notices {:?}",
            case.state,
            backups,
            notices
        ),
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
    // The random cases are few, so every state and owned-row value runs here first.
    let every_state = ALL_STATES.into_iter().flat_map(|state| {
        [false, true].map(|owned_row| Case {
            blocks: vec![(0, "a page".to_string()), (1, "a child".to_string())],
            state,
            owned_row,
        })
    });
    let single_block = Case {
        blocks: vec![(0, "aa".to_string())],
        state: DbState::Current,
        owned_row: true,
    };
    for case in every_state.chain([single_block]) {
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
