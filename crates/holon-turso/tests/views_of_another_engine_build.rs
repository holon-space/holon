//! A vault whose views an older IVM engine build computed: the open drops and
//! discloses them, every table row stays, the boot rebuilds them on this
//! build, and a block insert then costs as many VM steps as on a fresh vault.
//!
//! The fixture is a vault of [`FIXTURE_BLOCKS`] blocks that Holon on the
//! previous engine pin wrote with every schema module, compressed with zstd.
//! Its views were planned by that engine, so a view kept as it is would keep
//! that engine's maintenance plan.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use holon_api::Value;
use holon_turso::db_open::OpenChange;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;
use holon_turso::schema_modules::CoreSchemaModule;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;
use tokio::sync::broadcast;
use turso_core::Connection;
use turso_core::Database;

use crate::block_with_path_upkeep_scaling::FLAT;
use crate::block_with_path_upkeep_scaling::Shape;
use crate::block_with_path_upkeep_scaling::exec;
use crate::block_with_path_upkeep_scaling::open;
use crate::block_with_path_upkeep_scaling::probe_parents;
use crate::block_with_path_upkeep_scaling::seed;
use crate::block_with_path_upkeep_scaling::upkeep_steps;
use crate::warm_reboot_keeps_matviews::ensure_all;

const FIXTURE: &[u8] = include_bytes!("fixtures/views_of_previous_engine_build.db.zst");
const FIXTURE_BLOCKS: usize = 1000;
/// Where `write_the_fixture` puts the compressed database.
const FIXTURE_ENV: &str = "HOLON_ENGINE_BUILD_FIXTURE";
/// Owned rows the fixture holds besides the blocks; none of them is derived.
const OWNED: [(&str, i64); 2] = [("navigation_history", 1), ("block_history", 1)];

async fn count(handle: &DbHandle, sql: &str) -> i64 {
    let rows = handle
        .query(sql, HashMap::new())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
    match rows.first().and_then(|r| r.get("n")) {
        Some(Value::Integer(n)) => *n,
        other => panic!("{sql} gave {other:?}"),
    }
}

async fn block_count(handle: &DbHandle) -> i64 {
    count(
        handle,
        "SELECT count(*) AS n FROM block_raw WHERE id LIKE 'block:b%'",
    )
    .await
}

/// `block_with_path` as the view holds it, and as its SELECT computes it now.
fn held_and_computed(conn: &Arc<Connection>) -> (Vec<String>, Vec<String>) {
    let rows = |sql: &str| {
        let mut rows: Vec<String> = conn
            .prepare(sql)
            .and_then(|mut stmt| stmt.run_collect_rows())
            .unwrap_or_else(|e| panic!("{}: {e}", &sql[..sql.len().min(120)]))
            .into_iter()
            .map(|row| format!("{row:?}"))
            .collect();
        rows.sort();
        rows
    };
    let select = include_str!("../sql/schema/blocks_with_paths.sql");
    (
        rows("SELECT id, parent_id, path, root_id FROM block_with_path"),
        rows(&format!(
            "SELECT id, parent_id, path, root_id FROM ({select})"
        )),
    )
}

/// A vault of [`FIXTURE_BLOCKS`] blocks with every schema module, built on
/// this engine build, as the fixture was on the previous one.
async fn fresh_vault(path: &Path) -> (Arc<Database>, TursoBackend, DbHandle, Arc<Connection>) {
    let (db, backend, handle, conn) = open(path);
    CoreSchemaModule.ensure_schema(&handle).await.expect("core");
    BlockSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("junctions");
    seed(
        &conn,
        &Shape::FANOUT_8.parents(FIXTURE_BLOCKS),
        0..FIXTURE_BLOCKS,
    );
    ensure_all(&handle).await;
    (db, backend, handle, conn)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "writes the fixture; run on the previous engine pin with HOLON_ENGINE_BUILD_FIXTURE set"]
async fn write_the_fixture() {
    let target = std::env::var(FIXTURE_ENV)
        .unwrap_or_else(|_| panic!("{FIXTURE_ENV} must name the file to write"));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("holon.db");
    let (db, backend, handle, conn) = fresh_vault(&path).await;
    exec(
        &conn,
        "INSERT INTO navigation_history (region, block_id) VALUES ('main', 'block:b7')",
    );
    exec(
        &conn,
        "INSERT INTO block_history (entity_name, block_id, op_name, origin, at_millis, day, \
         op_group) VALUES ('block', 'block:b7', 'update', 'user', 0, '2026-01-01', 1)",
    );
    exec(&conn, "PRAGMA wal_checkpoint(TRUNCATE)");
    handle.shutdown().await.expect("shutdown");
    drop((backend, conn, db));
    let bytes = std::fs::read(&path).unwrap();
    let packed = zstd::encode_all(bytes.as_slice(), 19).unwrap();
    std::fs::write(&target, packed).unwrap();
    eprintln!("[fixture] wrote {target} from {} bytes", bytes.len());
}

fn unpack(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("holon.db");
    std::fs::write(&path, zstd::decode_all(FIXTURE).expect("decompress")).unwrap();
    path
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn views_another_engine_build_computed_are_rebuilt_and_every_row_stays() {
    let dir = tempfile::tempdir().unwrap();
    let path = unpack(dir.path());

    let (db, outcome) =
        TursoBackend::open_database_reporting_outcome(&path).expect("the fixture opens");
    let (backend, handle) =
        TursoBackend::new(db.clone(), broadcast::channel(64).0).expect("backend");
    let conn = db.connect(turso_core::ConnectOpts::new()).expect("connect");
    ensure_all(&handle).await;
    assert_eq!(
        block_count(&handle).await,
        FIXTURE_BLOCKS as i64,
        "block_raw keeps every block"
    );
    for (table, rows) in OWNED {
        assert_eq!(
            count(&handle, &format!("SELECT count(*) AS n FROM {table}")).await,
            rows,
            "{table} keeps its rows"
        );
    }
    let (held, computed) = held_and_computed(&conn);
    assert_eq!(
        held.len(),
        FIXTURE_BLOCKS,
        "block_with_path holds every block"
    );
    assert_eq!(
        held, computed,
        "the rebuilt view holds what its SELECT computes"
    );

    let places = probe_parents(&Shape::FANOUT_8.parents(FIXTURE_BLOCKS));
    let upgraded = upkeep_steps(&conn, &places, FIXTURE_BLOCKS);
    let fresh_dir = tempfile::tempdir().unwrap();
    let (fresh_db, fresh_backend, fresh_handle, fresh_conn) =
        fresh_vault(&fresh_dir.path().join("holon.db")).await;
    let fresh = upkeep_steps(&fresh_conn, &places, FIXTURE_BLOCKS);
    fresh_handle.shutdown().await.expect("shutdown");
    drop((fresh_backend, fresh_conn, fresh_db));
    eprintln!("[upkeep] upgraded fixture {upgraded:?} steps, fresh vault {fresh:?} steps");
    for ((name, _), (u, f)) in places.iter().zip(upgraded.into_iter().zip(fresh)) {
        let ratio = u as f64 / f as f64;
        assert!(
            ratio < FLAT,
            "one block insert {name} takes {u} VM steps in the upgraded fixture and {f} in a \
             fresh vault of {FIXTURE_BLOCKS} blocks ({ratio:.2}x, the same plan is below \
             {FLAT}x): a view keeps the previous \
             engine build's maintenance plan"
        );
    }
    let [OpenChange::ViewsDropped { reason, views }] = outcome.0.as_slice() else {
        panic!("the open must drop and disclose the views, got {outcome:?}");
    };
    assert!(
        reason.contains("IVM engine build"),
        "the disclosure must name the engine build: {reason}"
    );
    assert!(
        views.iter().any(|v| v == "block_with_path"),
        "block_with_path must be among the dropped views: {views:?}"
    );

    handle.shutdown().await.expect("shutdown");
    drop((backend, conn, db));
    let (_db, again) =
        TursoBackend::open_database_reporting_outcome(&path).expect("the second open");
    assert!(
        again.0.is_empty(),
        "the second open on this build must keep the file as it is, got {again:?}"
    );
}
