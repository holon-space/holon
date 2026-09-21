//! D173.a: the hot point reads on the hydrated `block` relation must be index
//! SEARCHes, not full table scans.
//!
//! `block` and `block_with_path` are materialized views — ordinary rowid
//! btrees in the schema, but until the fork learned `CREATE INDEX` on a view
//! every `WHERE id = ?` against them read the whole view. At 12,800 rows that
//! is 19–28 ms per point read against ~50 µs on `block_raw`, and the read path
//! at `holon/src/api/block_domain.rs:209` and the `nearest_page_ancestor` walk
//! at `holon-filesystem/src/sync_ports.rs:1079` issue one per block.
//!
//! The oracle is the query plan, not a wall clock: a timing assertion on a
//! shared machine is noise, while `SCAN block` is the defect itself. The four
//! statements are the ones production issues — the last is `ui_watcher.rs:54`'s
//! structural predicate.

use std::collections::HashMap;
use std::time::Instant;

use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockHierarchySchemaModule;
use holon_turso::schema_modules::BlockMatviewSchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;
use holon_turso::schema_modules::CoreSchemaModule;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;

const ROOT_PARENT: &str = "sentinel:no_parent";

async fn boot() -> DbHandle {
    let (backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    // The backend actor must outlive the handle.
    std::mem::forget(backend);
    schema(&handle).await;
    handle
}

async fn schema(handle: &DbHandle) {
    CoreSchemaModule.ensure_schema(handle).await.expect("core");
    BlockSchemaModule
        .ensure_schema(handle)
        .await
        .expect("block junctions");
    BlockMatviewSchemaModule
        .ensure_schema(handle)
        .await
        .expect("block matview");
    BlockHierarchySchemaModule
        .ensure_schema(handle)
        .await
        .expect("block_with_path");
}

async fn seed(handle: &DbHandle, rows: usize) {
    for i in 0..rows {
        let parent = if i == 0 {
            ROOT_PARENT.to_string()
        } else {
            format!("block:b{}", i / 8)
        };
        handle
            .execute(
                "INSERT INTO block_raw (id, parent_id, content) VALUES (?, ?, ?)",
                vec![
                    turso::Value::Text(format!("block:b{i}")),
                    turso::Value::Text(parent),
                    turso::Value::Text(format!("row {i}")),
                ],
            )
            .await
            .expect("insert block_raw");
    }
}

/// The plan as one flat string — every `detail` column of every step.
async fn plan(handle: &DbHandle, sql: &str) -> String {
    let rows = handle
        .query(&format!("EXPLAIN QUERY PLAN {sql}"), HashMap::new())
        .await
        .unwrap_or_else(|e| panic!("EXPLAIN QUERY PLAN failed for {sql:?}: {e}"));
    assert!(
        !rows.is_empty(),
        "EXPLAIN QUERY PLAN returned no rows for {sql:?} — the plan oracle would pass vacuously"
    );
    rows.iter()
        .flat_map(|row| row.values())
        .map(|v| match v {
            holon_api::Value::String(s) => s.clone(),
            other => format!("{other:?}"),
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

/// The plan must reach the row through one of this view's indexes.
///
/// Two spellings count: a single-column predicate plans as `SEARCH`, while
/// `id = ? OR parent_id = ?` plans as `MULTI-INDEX OR` over both indexes. Both
/// are checked by the index NAME rather than the verb, so a plan that happens
/// to search some other relation cannot satisfy this.
fn assert_searches(plan: &str, relation: &str, sql: &str) {
    assert!(
        !plan.contains(&format!("SCAN {relation}")),
        "point read {sql:?} still SCANs {relation} — the whole view is read per lookup.\nplan: \
         {plan}"
    );
    assert!(
        plan.contains(&format!("idx_{relation}_")),
        "point read {sql:?} reaches {relation} without naming one of its indexes.\nplan: {plan}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn hydrated_block_point_reads_search_an_index() {
    let handle = boot().await;
    seed(&handle, 200).await;

    for sql in [
        "SELECT * FROM block WHERE id = 'block:b7'",
        "SELECT * FROM block WHERE parent_id = 'block:b7'",
        "SELECT * FROM block WHERE id = 'block:b7' OR parent_id = 'block:b7'",
    ] {
        let plan = plan(&handle, sql).await;
        assert_searches(&plan, "block", sql);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn block_with_path_point_read_searches_an_index() {
    let handle = boot().await;
    seed(&handle, 200).await;

    let sql = "SELECT * FROM block_with_path WHERE id = 'block:b7'";
    let plan = plan(&handle, sql).await;
    assert_searches(&plan, "block_with_path", sql);
}

/// An index that speeds up the plan but hands back different rows is worse
/// than the scan. The forced-scan arm is the oracle.
#[tokio::test(flavor = "multi_thread")]
async fn the_indexed_point_read_returns_the_same_rows_as_a_scan() {
    let handle = boot().await;
    seed(&handle, 200).await;
    assert_indexed_reads_match_a_scan(&handle).await;
}

// ---------------------------------------------------------------------------
// Upgrade path and measurement. Both are `#[ignore]`d: the first needs a
// database file written by a DIFFERENT build, the second is a timing
// instrument whose numbers are only meaningful on an idle release build.
// ---------------------------------------------------------------------------

/// Path of the cross-build fixture database, set by
/// `lane-logs/build-old-pin-fixture.sh` and read back by the upgrade test.
const FIXTURE_ENV: &str = "HOLON_MVINDEX_FIXTURE";

async fn open_file_db(path: &std::path::Path) -> DbHandle {
    let db = TursoBackend::open_database(path).expect("open fixture db");
    let (cdc_tx, _rx) = tokio::sync::broadcast::channel(1024);
    let (backend, handle) = TursoBackend::new(db, cdc_tx).expect("backend over fixture db");
    std::mem::forget(backend);
    handle
}

fn fixture_path() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var(FIXTURE_ENV).unwrap_or_else(|_| {
        panic!("{FIXTURE_ENV} must name the database file this test boots over")
    }))
}

/// Write a database with the production schema and `rows` blocks, then leave
/// it on disk. Run by the OLD pin so the upgrade test has something to open.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "writes the cross-build fixture database; driven by lane-logs/build-old-pin-fixture.sh"]
async fn build_fixture_database() {
    let path = fixture_path();
    let handle = open_file_db(&path).await;
    schema(&handle).await;
    seed(&handle, 400).await;
    let count = row_count(&handle, "block").await;
    assert_eq!(
        count, 400,
        "the fixture's block matview must hold every seeded row"
    );
    eprintln!("[fixture] wrote {path:?} with {count} blocks");
}

/// Boot over a database written by the old pin: the indexes must be created on
/// the existing views, backfilled, and the reads must stay correct.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "boots over a database written by a different build; driven by lane-logs/upgrade-over-old-db.sh"]
async fn boots_over_a_database_written_without_the_indexes() {
    let path = fixture_path();
    let handle = open_file_db(&path).await;

    let before = row_count(&handle, "block").await;
    assert_eq!(
        before, 400,
        "the fixture was not written by the old pin's schema — nothing is being upgraded"
    );

    // The production boot path, second time over the same file.
    schema(&handle).await;

    for sql in [
        "SELECT * FROM block WHERE id = 'block:b7'",
        "SELECT * FROM block WHERE parent_id = 'block:b7'",
        "SELECT * FROM block WHERE id = 'block:b7' OR parent_id = 'block:b7'",
    ] {
        assert_searches(&plan(&handle, sql).await, "block", sql);
    }
    let bwp = "SELECT * FROM block_with_path WHERE id = 'block:b7'";
    assert_searches(&plan(&handle, bwp).await, "block_with_path", bwp);

    // The backfill is the claim: the index must index the rows that were
    // already there, not only rows written after it existed.
    assert_eq!(
        row_count(&handle, "block").await,
        before,
        "the upgrade boot changed the block matview's row count"
    );
    assert_indexed_reads_match_a_scan(&handle).await;

    // A write after the upgrade maintains both the view and its indexes.
    handle
        .execute(
            "INSERT INTO block_raw (id, parent_id, content) VALUES (?, ?, ?)",
            vec![
                turso::Value::Text("block:after-upgrade".into()),
                turso::Value::Text("block:b7".into()),
                turso::Value::Text("written after the index existed".into()),
            ],
        )
        .await
        .expect("insert after upgrade");
    assert_eq!(row_count(&handle, "block").await, before + 1);
    assert_indexed_reads_match_a_scan(&handle).await;
    assert_matview_agrees_with_recompute(&handle).await;
}

async fn row_count(handle: &DbHandle, relation: &str) -> i64 {
    let rows = handle
        .query(
            &format!("SELECT COUNT(*) AS n FROM {relation}"),
            HashMap::new(),
        )
        .await
        .unwrap_or_else(|e| panic!("count on {relation} failed: {e}"));
    match rows.first().and_then(|r| r.get("n")) {
        Some(holon_api::Value::Integer(n)) => *n,
        other => panic!("COUNT(*) on {relation} returned {other:?}"),
    }
}

/// Every `id` the statement returns, sorted.
async fn ids(handle: &DbHandle, sql: &str) -> Vec<String> {
    let rows = handle
        .query(sql, HashMap::new())
        .await
        .unwrap_or_else(|e| panic!("{sql} failed: {e}"));
    let mut v: Vec<String> = rows
        .iter()
        .map(|r| match r.get("id") {
            Some(holon_api::Value::String(s)) => s.clone(),
            other => panic!("id column is not text: {other:?}"),
        })
        .collect();
    v.sort();
    v
}

/// `block` is a projection of `block_raw` minus the root sentinel, which the
/// matview SELECT excludes by `b.id != 'sentinel:no_parent'`. The two must
/// otherwise name the same ids.
async fn assert_matview_agrees_with_recompute(handle: &DbHandle) {
    let view = ids(handle, "SELECT id FROM block").await;
    let raw = ids(
        handle,
        "SELECT id FROM block_raw WHERE id != 'sentinel:no_parent'",
    )
    .await;
    assert!(
        !raw.is_empty(),
        "nothing to compare — the oracle would pass vacuously"
    );
    assert_eq!(
        view, raw,
        "the block matview and block_raw disagree on which blocks exist"
    );
}

async fn assert_indexed_reads_match_a_scan(handle: &DbHandle) {
    let indexed = ids(
        handle,
        "SELECT id FROM block WHERE id = 'block:b7' OR parent_id = 'block:b7'",
    )
    .await;
    let scanned = ids(
        handle,
        "SELECT id FROM block WHERE (id || '') = 'block:b7' OR (parent_id || '') = 'block:b7'",
    )
    .await;
    assert!(!indexed.is_empty(), "the point read found nothing");
    assert_eq!(
        indexed, scanned,
        "the indexed read and a forced scan disagree"
    );
}

/// The D173.a measurement instrument: per-statement cost of the four hot point
/// reads, and the write cost of maintaining whatever indexes exist, across
/// vault sizes. Release profile and an idle machine or the numbers are noise.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "timing instrument; run explicitly on an idle release build"]
async fn measure_point_read_cost() {
    const SAMPLES: usize = 6;
    const REPS: usize = 200;
    const READS: [&str; 4] = [
        "SELECT * FROM block WHERE id = 'block:b7'",
        "SELECT * FROM block WHERE parent_id = 'block:b7'",
        "SELECT * FROM block WHERE id = 'block:b7' OR parent_id = 'block:b7'",
        "SELECT * FROM block_with_path WHERE id = 'block:b7'",
    ];

    for n in [204usize, 1600, 3200] {
        let handle = boot().await;
        seed(&handle, n).await;

        // Harness floor: the same loop over a statement that touches nothing.
        let floor = time_us(&handle, "SELECT 1", SAMPLES, REPS).await;
        eprintln!("[bench] N={n} floor={:.1}–{:.1} us/stmt", floor.0, floor.1);

        for sql in READS {
            let (lo, hi) = time_us(&handle, sql, SAMPLES, REPS).await;
            eprintln!(
                "[bench] N={n} read {lo:.1}-{hi:.1} us/stmt  plan={}  sql={sql}",
                plan(&handle, sql).await
            );
        }

        let (lo, hi) = time_write_us(&handle, SAMPLES, REPS).await;
        eprintln!("[bench] N={n} write {lo:.1}-{hi:.1} us/stmt");
    }
}

/// Min and max over `samples` batches of `reps` executions, µs per statement.
async fn time_us(handle: &DbHandle, sql: &str, samples: usize, reps: usize) -> (f64, f64) {
    let mut per_stmt = Vec::with_capacity(samples);
    for _ in 0..samples {
        let start = Instant::now();
        for _ in 0..reps {
            handle
                .query(sql, HashMap::new())
                .await
                .unwrap_or_else(|e| panic!("{sql} failed: {e}"));
        }
        per_stmt.push(start.elapsed().as_secs_f64() * 1e6 / reps as f64);
    }
    per_stmt.sort_by(f64::total_cmp);
    (per_stmt[0], per_stmt[samples - 1])
}

async fn time_write_us(handle: &DbHandle, samples: usize, reps: usize) -> (f64, f64) {
    let mut per_stmt = Vec::with_capacity(samples);
    let mut round = 0u64;
    for _ in 0..samples {
        let start = Instant::now();
        for _ in 0..reps {
            round += 1;
            handle
                .execute(
                    "UPDATE block_raw SET content = ? WHERE id = 'block:b7'",
                    vec![turso::Value::Text(format!("churn {round}"))],
                )
                .await
                .expect("update block_raw");
        }
        per_stmt.push(start.elapsed().as_secs_f64() * 1e6 / reps as f64);
    }
    per_stmt.sort_by(f64::total_cmp);
    (per_stmt[0], per_stmt[samples - 1])
}
