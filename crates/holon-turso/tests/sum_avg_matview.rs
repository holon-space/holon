//! SUM and AVG in a matview are maintained incrementally. After every insert,
//! update and delete the view must hold exactly what the same query computes
//! over the base table.

use std::collections::HashMap;
use std::sync::Arc;

use holon_turso::db_open::OpenChange;
use holon_turso::matview_manager::reconcile_named_view;
use holon_turso::scalar_fns;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;
use proptest::prelude::*;
use tokio::sync::broadcast;

const SEED: &str = "INSERT INTO agg (id, grp, n, x) VALUES (1, 'a', 1, 0.1), (2, 'a', 2, 0.2), \
     (3, 'b', 5, 1e16), (4, 'b', 4, 1.0)";

const WRITES: &[&str] = &[
    "INSERT INTO agg (id, grp, n, x) VALUES (5, 'a', 3, 0.3)",
    "INSERT INTO agg (id, grp, n, x) VALUES (6, 'b', -5, -1e16)",
    "UPDATE agg SET n = 7, x = 0.7 WHERE id = 2",
    "UPDATE agg SET grp = 'b' WHERE id = 1",
    "UPDATE agg SET n = NULL, x = NULL WHERE id = 4",
    "DELETE FROM agg WHERE id = 3",
    "DELETE FROM agg WHERE id = 5",
    "INSERT INTO agg (id, grp, n, x) VALUES (7, 'c', 9223372036854775807, 2.5)",
    "DELETE FROM agg WHERE grp = 'b'",
];

async fn snapshot(handle: &DbHandle, sql: &str, cols: &[&str]) -> Vec<String> {
    let mut rows: Vec<String> = handle
        .query(sql, HashMap::new())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
        .iter()
        .map(|r| {
            cols.iter()
                .map(|c| format!("{:?}", r.get(*c)))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    rows.sort();
    rows
}

async fn view_follows_every_write<S: AsRef<str>>(
    agg_sql: &str,
    cols: &[&str],
    seed: &str,
    writes: &[S],
    compared_groups: &str,
) {
    let (backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(backend);
    handle
        .execute_ddl("CREATE TABLE agg (id INTEGER PRIMARY KEY, grp TEXT, n INTEGER, x REAL)")
        .await
        .expect("create table");
    handle.execute(seed, vec![]).await.expect("seed rows");
    reconcile_named_view(&handle, "agg_stats", agg_sql)
        .await
        .expect("SUM/AVG view must build");
    for write in writes.iter().map(AsRef::as_ref) {
        handle.execute(write, vec![]).await.expect(write);
        assert_eq!(
            snapshot(
                &handle,
                &format!("SELECT * FROM agg_stats WHERE {compared_groups}"),
                cols
            )
            .await,
            snapshot(
                &handle,
                &format!("SELECT * FROM ({agg_sql}) WHERE {compared_groups}"),
                cols
            )
            .await,
            "after `{write}` the view must equal the direct query"
        );
    }
}

#[tokio::test]
async fn integer_sum_and_avg_views_equal_the_direct_query_after_every_write() {
    view_follows_every_write(
        "SELECT grp, SUM(n) AS s, AVG(n) AS a, SUM(CASE WHEN n > 2 THEN 1 ELSE 0 END) AS s_case, \
         COUNT(*) AS c FROM agg GROUP BY grp",
        &["grp", "s", "a", "s_case", "c"],
        SEED,
        WRITES,
        "TRUE",
    )
    .await;
}

#[tokio::test]
async fn real_sum_and_avg_views_equal_the_direct_query_after_every_write() {
    view_follows_every_write(
        "SELECT grp, SUM(x) AS s, AVG(x) AS a FROM agg GROUP BY grp",
        &["grp", "s", "a"],
        SEED,
        WRITES,
        "TRUE",
    )
    .await;
}

/// Cents, and multiples of 1e15 whose retraction must leave the cents behind.
fn real_value() -> impl Strategy<Value = f64> {
    prop_oneof![
        4 => (-100_000_000i64..100_000_000).prop_map(|cents| cents as f64 / 100.0),
        1 => (-10i64..=10).prop_map(|k| k as f64 * 1e15),
    ]
}

/// While a group holds a multiple of 1e15 next to cents, its exact sum and
/// SQLite's KBN sum may round apart in the last bit by design.
const GROUPS_WITHOUT_A_LARGE_VALUE: &str = "grp NOT IN (SELECT grp FROM agg WHERE abs(x) >= 1e15)";

fn write() -> impl Strategy<Value = (u8, i64, Option<f64>, &'static str)> {
    (
        0u8..4,
        1i64..36,
        prop::option::weighted(0.9, real_value()),
        prop_oneof![Just("a"), Just("b"), Just("c")],
    )
}

fn as_sql(ops: &[(u8, i64, Option<f64>, &str)]) -> Vec<String> {
    let lit = |x: &Option<f64>| x.map_or("NULL".to_string(), |v| format!("{v:?}"));
    ops.iter()
        .enumerate()
        .map(|(i, (kind, id, x, grp))| match kind {
            0 => format!(
                "INSERT INTO agg (id, grp, n, x) VALUES ({}, '{grp}', {id}, {})",
                12 + i,
                lit(x)
            ),
            1 => format!("UPDATE agg SET x = {} WHERE id = {id}", lit(x)),
            2 => format!("UPDATE agg SET grp = '{grp}' WHERE id = {id}"),
            _ => format!("DELETE FROM agg WHERE id = {id}"),
        })
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, ..ProptestConfig::default() })]

    #[test]
    fn real_sum_and_avg_views_equal_the_direct_query_after_random_writes(
        ops in prop::collection::vec(write(), 1..24)
    ) {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(view_follows_every_write(
                "SELECT grp, SUM(x) AS s, TOTAL(x) AS t, AVG(x) AS a, COUNT(x) AS c FROM agg \
                 GROUP BY grp",
                &["grp", "s", "t", "a", "c"],
                "INSERT INTO agg (id, grp, n, x) VALUES (1, 'a', 1, 0.1), (2, 'a', 2, 0.2), \
                 (3, 'b', 3, 0.3), (4, 'b', 4, 1.0), (5, 'c', 5, -0.7)",
                &as_sql(&ops),
                GROUPS_WITHOUT_A_LARGE_VALUE,
            ));
    }
}

/// A database whose SUM/TOTAL/AVG view state an engine before the exact REAL
/// sum wrote: turso's own compatibility fixture, compressed with zstd.
const BEFORE_EXACT_SUM: &[u8] = include_bytes!("fixtures/sum_state_before_exact_sum.db.zst");
const FIXTURE_VIEW: &str =
    "SELECT g, SUM(x) AS sx, TOTAL(x) AS tx, AVG(y) AS ay, COUNT(*) AS c FROM t GROUP BY g";

/// Writes the function-set record that Holon on that engine kept: the record
/// without the view-state layout.
fn record_as_the_previous_engine_did(path: &std::path::Path) {
    let io = Arc::new(turso_core::UnixIO::new().expect("file IO"));
    let options = turso_core::OpenOptions::new(Arc::new(turso_core::SqliteDialect))
        .db_opts(turso_core::DatabaseOpts::default().with_views(true));
    let db = turso_core::Database::open(io, path.to_str().unwrap(), options).expect("raw open");
    let conn = db.connect(turso_core::ConnectOpts::new()).expect("connect");
    for sql in [
        "CREATE TABLE holon_db_fn_set (signature TEXT NOT NULL)".to_string(),
        format!(
            "INSERT INTO holon_db_fn_set (signature) VALUES ('{}')",
            scalar_fns::signature(scalar_fns::ALL)
        ),
    ] {
        conn.execute(&sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sum_view_from_before_the_exact_sum_is_rebuilt_and_follows_a_real_write() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("holon.db");
    std::fs::write(
        &path,
        zstd::decode_all(BEFORE_EXACT_SUM).expect("decompress fixture"),
    )
    .unwrap();
    record_as_the_previous_engine_did(&path);

    let (db, outcome) =
        TursoBackend::open_database_reporting_outcome(&path).expect("the old database opens");
    let (_backend, handle) = TursoBackend::new(db, broadcast::channel(64).0).expect("backend");
    reconcile_named_view(&handle, "v", FIXTURE_VIEW)
        .await
        .expect("the schema modules build the view");
    let write = "INSERT INTO t VALUES (1, 0.1, 1.0), (4, 0.2, 3.5)";
    handle
        .execute(write, vec![])
        .await
        .unwrap_or_else(|e| panic!("{write}: {e}"));
    // Groups 2 and 3 sum to an infinity, which Holon's read path refuses.
    let cols = ["g", "sx", "tx", "ay", "c"];
    assert_eq!(
        snapshot(&handle, "SELECT * FROM v WHERE g IN (1, 4)", &cols).await,
        snapshot(
            &handle,
            "SELECT g, SUM(x) AS sx, TOTAL(x) AS tx, AVG(y) AS ay, COUNT(*) AS c FROM t \
             WHERE g IN (1, 4) GROUP BY g",
            &cols
        )
        .await,
        "after `{write}` the view must equal the direct query"
    );
    let [OpenChange::ViewsDropped { reason, views }] = outcome.0.as_slice() else {
        panic!("the open must report the dropped view, got {outcome:?}");
    };
    assert_eq!(views, &["v".to_string()]);
    assert!(reason.contains("engine state"), "{reason}");
}
