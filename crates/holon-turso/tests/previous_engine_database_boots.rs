//! A database the previous IVM engine wrote opens on this one: every stored
//! materialized view is dropped and reported, every table row stays in the
//! same file, and the schema modules build the views again.
//!
//! The fixture is a vault database that Holon on the previous engine pin wrote
//! at boot (every schema-module view, the integration type views and the
//! watch views of the default layout), compressed with zstd.

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;

use holon_api::Value;
use holon_turso::db_open::OpenChange;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;
use tokio::sync::broadcast;

use crate::warm_reboot_keeps_matviews::ensure_all;

const FIXTURE: &[u8] = include_bytes!("fixtures/previous_engine_vault.db.zst");
const FIXTURE_BLOCK_ROWS: i64 = 34;

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_previous_engine_database_keeps_its_rows_and_rebuilds_its_views() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("holon.db");
    std::fs::write(
        &path,
        zstd::decode_all(FIXTURE).expect("decompress fixture"),
    )
    .unwrap();
    let inode = std::fs::metadata(&path).unwrap().ino();

    let (db, outcome) =
        TursoBackend::open_database_reporting_outcome(&path).expect("the old database opens");
    let [OpenChange::ViewsDropped { reason, views }] = outcome.0.as_slice() else {
        panic!("the open must report the dropped views, got {outcome:?}");
    };
    assert!(
        views.iter().any(|v| v == "block") && views.iter().any(|v| v == "block_with_path"),
        "the schema-module views are reported: {views:?}"
    );
    assert!(
        reason.contains("block"),
        "the reason names a view: {reason}"
    );

    let (_backend, handle) = TursoBackend::new(db, broadcast::channel(64).0).expect("backend");
    assert_eq!(
        names(
            &handle,
            "SELECT name FROM sqlite_schema WHERE type = 'view'"
        )
        .await,
        BTreeSet::new(),
        "no stored view survives the open"
    );
    let rows = handle
        .query("SELECT count(*) AS n FROM block_raw", HashMap::new())
        .await
        .expect("count block_raw");
    assert_eq!(rows[0].get("n"), Some(&Value::Integer(FIXTURE_BLOCK_ROWS)));

    ensure_all(&handle).await;
    let rebuilt = names(
        &handle,
        "SELECT name FROM sqlite_schema WHERE type = 'view'",
    )
    .await;
    for view in ["block", "block_with_path", "backlinks", "journal_feed"] {
        assert!(rebuilt.contains(view), "{view} is built again: {rebuilt:?}");
    }
    let rows = handle
        .query(
            "SELECT (SELECT count(*) FROM block) AS view, \
             (SELECT count(*) FROM block_raw WHERE id != 'sentinel:no_parent') AS raw",
            HashMap::new(),
        )
        .await
        .expect("read the rebuilt block view");
    assert_eq!(
        rows[0].get("view"),
        rows[0].get("raw"),
        "the rebuilt block view holds every block row"
    );

    assert_eq!(
        std::fs::metadata(&path).unwrap().ino(),
        inode,
        "the database file is the same file, not a replacement"
    );
    let siblings: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains("unusable"))
        .collect();
    assert_eq!(siblings, Vec::<String>::new(), "nothing is moved aside");
}
