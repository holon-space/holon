//! A database the previous IVM engine wrote is moved aside whole on this one:
//! the backup is the file byte for byte, its rows stay readable there, and the
//! schema modules build every view in the fresh file.
//!
//! The fixture is a vault database that Holon on the previous engine pin wrote
//! at boot (every schema-module view, the integration type views and the
//! watch views of the default layout), compressed with zstd.

use std::collections::BTreeSet;
use std::collections::HashMap;

use holon_api::Value;
use holon_turso::db_open::OpenChange;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;
use tokio::sync::broadcast;

use crate::warm_reboot_keeps_matviews::ensure_all;

const FIXTURE: &[u8] = include_bytes!("fixtures/previous_engine_vault.db.zst");
const FIXTURE_BLOCK_ROWS: i64 = 34;
const FIXTURE_KEPT_TABLES: [&str; 14] = [
    "_holon_quarantine",
    "advice_suppressed",
    "block_history",
    "block_redirects",
    "ingredient_use_raw",
    "local_ui_state",
    "navigation_cursor",
    "navigation_history",
    "operation",
    "pantry_item_raw",
    "person_raw",
    "recipe_raw",
    "shopping_item_raw",
    "undo_log",
];

async fn names(handle: &DbHandle, sql: &str) -> BTreeSet<String> {
    handle
        .query(sql, HashMap::new())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
        .into_iter()
        .map(|row| match row.get("name") {
            Some(Value::String(n)) => n.clone(),
            other => panic!("row without a name: {other:?}"),
        })
        .collect()
}

fn block_raw_rows_in(backup: &std::path::Path) -> i64 {
    let io = std::sync::Arc::new(turso_core::UnixIO::new().expect("file IO"));
    let options = turso_core::OpenOptions::new(std::sync::Arc::new(turso_sqlite::SqliteDialect));
    let db = turso_core::Database::open(io, backup.to_str().unwrap(), options).expect("raw open");
    let conn = db.connect(turso_core::ConnectOpts::new()).expect("connect");
    let sql = "SELECT count(*) FROM block_raw";
    match conn
        .prepare(sql)
        .and_then(|mut stmt| stmt.run_collect_rows())
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
        .as_slice()
    {
        [row] => match row.as_slice() {
            [turso_core::Value::Numeric(turso_core::Numeric::Integer(n))] => *n,
            other => panic!("{sql} gave {other:?}"),
        },
        other => panic!("{sql} gave {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_previous_engine_database_is_moved_aside_and_its_views_are_built_fresh() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("holon.db");
    let bytes = zstd::decode_all(FIXTURE).expect("decompress fixture");
    std::fs::write(&path, &bytes).unwrap();

    let (db, outcome) =
        TursoBackend::open_database_reporting_outcome(&path).expect("the old database opens");
    let [
        OpenChange::MovedAside { reason, backup },
        OpenChange::OwnedStateKept { kept, .. },
    ] = outcome.0.as_slice()
    else {
        panic!("the open must move the old file aside and rebuild a copy, got {outcome:?}");
    };
    let kept: BTreeSet<&str> = kept.iter().map(|(table, _)| table.as_str()).collect();
    assert_eq!(
        kept,
        FIXTURE_KEPT_TABLES.into_iter().collect(),
        "the rebuilt copy keeps exactly the tables nothing else restores"
    );
    assert!(
        reason.contains(holon_turso::db_open::VIEW_FORMAT),
        "the reason names the format this engine maintains: {reason}"
    );
    assert_eq!(
        std::fs::read(backup).unwrap(),
        bytes,
        "the backup at {} is the old file byte for byte",
        backup.display()
    );
    assert_eq!(block_raw_rows_in(backup), FIXTURE_BLOCK_ROWS);

    let (_backend, handle) = TursoBackend::new(db, broadcast::channel(64).0).expect("backend");
    assert_eq!(
        names(
            &handle,
            "SELECT name FROM sqlite_schema WHERE type = 'view'"
        )
        .await,
        BTreeSet::new(),
        "the rebuilt copy holds no view yet"
    );
    ensure_all(&handle).await;
    let rebuilt = names(
        &handle,
        "SELECT name FROM sqlite_schema WHERE type = 'view'",
    )
    .await;
    for view in ["block", "block_with_path", "backlinks", "journal_feed"] {
        assert!(rebuilt.contains(view), "{view} is built: {rebuilt:?}");
    }
}
