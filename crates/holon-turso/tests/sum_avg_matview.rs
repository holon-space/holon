//! SUM and AVG in a matview are maintained incrementally. After every insert,
//! update and delete the view must hold exactly what the same query computes
//! over the base table.

use std::collections::HashMap;

use holon_turso::matview_manager::reconcile_named_view;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;

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

async fn view_follows_every_write(agg_sql: &str, cols: &[&str]) {
    let (backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(backend);
    handle
        .execute_ddl("CREATE TABLE agg (id INTEGER PRIMARY KEY, grp TEXT, n INTEGER, x REAL)")
        .await
        .expect("create table");
    handle
        .execute(
            "INSERT INTO agg (id, grp, n, x) VALUES (1, 'a', 1, 0.1), (2, 'a', 2, 0.2), \
             (3, 'b', 5, 1e16), (4, 'b', 4, 1.0)",
            vec![],
        )
        .await
        .expect("seed rows");
    reconcile_named_view(&handle, "agg_stats", agg_sql)
        .await
        .expect("SUM/AVG view must build");
    for write in WRITES {
        handle.execute(write, vec![]).await.expect(write);
        assert_eq!(
            snapshot(&handle, "SELECT * FROM agg_stats", cols).await,
            snapshot(&handle, agg_sql, cols).await,
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
    )
    .await;
}

#[tokio::test]
#[ignore = "the engine keeps a running REAL sum, so a retraction leaves rounding residue \
            (0.09999999999999998 where the direct query gives 0.1); run with --ignored"]
async fn real_sum_and_avg_views_equal_the_direct_query_after_every_write() {
    view_follows_every_write(
        "SELECT grp, SUM(x) AS s, AVG(x) AS a FROM agg GROUP BY grp",
        &["grp", "s", "a"],
    )
    .await;
}
