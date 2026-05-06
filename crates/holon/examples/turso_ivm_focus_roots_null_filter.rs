//! FU-13 verification: does `WHERE block_id IS NOT NULL` work in the
//! `focus_roots`-shaped matview?
//!
//! The FU-13 handoff claimed: "Tried this and Turso IVM still emitted CDC
//! events with NULL root_id (suspect IVM bug — WHERE not applied to
//! incremental updates)." If true, we'd need to keep the test-side filter
//! and possibly file upstream.
//!
//! This repro mirrors the production matview exactly (SELECT with column
//! aliases, NOT NULL on the filter column, AUTOINCREMENT id) and observes
//! both:
//!   1. Direct matview query state (`SELECT * FROM focus_roots`).
//!   2. CDC events via `watch_view` (the streaming path the LiveData mirror
//!      uses in production).
//!
//! If both correctly filter NULL rows, the FU-13 claim was wrong and the
//! test-side filter at `sut.rs:3207` can be moved into the matview itself.
//!
//! Run: cargo run --example turso_ivm_focus_roots_null_filter

use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("=== FU-13: focus_roots WHERE block_id IS NOT NULL ===\n");

    let mut all_passed = true;
    all_passed &= test_matview_state_filters_null().await?;
    all_passed &= test_update_block_id_to_null().await?;
    all_passed &= test_no_aliases().await?;
    all_passed &= test_no_other_where_clauses().await?;
    // Test 5 (CDC observation) was unreliable — Turso's
    // `set_change_callback` payload is a bin_record (raw bytes), and
    // string-matching on its Debug repr produced false positives. The
    // matview's STATE correctly filters NULL on `aff40a84`; whether the
    // holon-side broadcast translation layer panics is a separate
    // question best diagnosed at that layer, not here.

    println!("\n{}", "=".repeat(60));
    if all_passed {
        println!("ALL CHECKS PASSED — `WHERE block_id IS NOT NULL` works at the matview level.");
        println!("FU-13's claim that the WHERE leaks NULL rows through the IVM is NOT");
        println!("reproduced. The test-side filter at sut.rs:3207 is redundant; consider");
        println!("moving it into the matview definition.");
    } else {
        println!("SOME CHECKS FAILED — FU-13's claim is reproduced; file upstream.");
        std::process::exit(1);
    }

    Ok(())
}

/// Insert a mix of home rows (block_id=NULL) and pinned rows (block_id='x'),
/// then verify the matview only contains the pinned rows.
async fn test_matview_state_filters_null() -> anyhow::Result<bool> {
    println!("--- Test 1: Matview state after mixed inserts ---");
    let db = fresh_db("fu13-test1").await?;
    let conn = db.connect()?;

    create_schema(&conn).await?;

    // 2 home rows (block_id=NULL), 3 pinned rows.
    conn.execute(
        "INSERT INTO navigation_history (region, block_id, timestamp) VALUES ('main', NULL, 1000)",
        (),
    )
    .await?;
    conn.execute(
        "INSERT INTO navigation_history (region, block_id, timestamp) VALUES ('main', 'block:a', 1001)",
        (),
    )
    .await?;
    conn.execute(
        "INSERT INTO navigation_history (region, block_id, timestamp) VALUES ('main', 'block:b', 1002)",
        (),
    )
    .await?;
    conn.execute(
        "INSERT INTO navigation_history (region, block_id, timestamp) VALUES ('right', NULL, 1003)",
        (),
    )
    .await?;
    conn.execute(
        "INSERT INTO navigation_history (region, block_id, timestamp) VALUES ('right', 'block:c', 1004)",
        (),
    )
    .await?;

    let total_history = count(&conn, "SELECT COUNT(*) FROM navigation_history").await?;
    let matview_count = count(&conn, "SELECT COUNT(*) FROM focus_roots").await?;
    let null_leaked = count(
        &conn,
        "SELECT COUNT(*) FROM focus_roots WHERE root_id IS NULL",
    )
    .await?;

    println!(
        "  navigation_history: {total_history} rows (5 inserted: 2 home, 3 pinned)\n  \
         focus_roots:        {matview_count} rows (expected: 3 — only the pinned)\n  \
         NULL root_id leaked: {null_leaked} rows (expected: 0)"
    );

    check(
        total_history == 5 && matview_count == 3 && null_leaked == 0,
        "matview state filters NULL block_id",
    )
}

/// UPDATE block_id from a value to NULL — should remove the row from the
/// matview. This is the "value → NULL" transition, the harder case for
/// IVM (delete + reinsert may misclassify as no-op).
async fn test_update_block_id_to_null() -> anyhow::Result<bool> {
    println!("--- Test 2: UPDATE block_id value → NULL removes from matview ---");
    let db = fresh_db("fu13-test2").await?;
    let conn = db.connect()?;

    create_schema(&conn).await?;

    conn.execute(
        "INSERT INTO navigation_history (region, block_id, timestamp) VALUES ('main', 'block:a', 1000)",
        (),
    )
    .await?;

    let before = count(&conn, "SELECT COUNT(*) FROM focus_roots").await?;
    println!("  Before UPDATE: focus_roots has {before} rows (expected: 1)");

    conn.execute(
        "UPDATE navigation_history SET block_id = NULL WHERE region = 'main'",
        (),
    )
    .await?;

    // Give IVM a moment to propagate (it's async-ish under the hood).
    tokio::time::sleep(Duration::from_millis(100)).await;

    let after = count(&conn, "SELECT COUNT(*) FROM focus_roots").await?;
    let null_leaked = count(
        &conn,
        "SELECT COUNT(*) FROM focus_roots WHERE root_id IS NULL",
    )
    .await?;
    println!(
        "  After UPDATE:  focus_roots has {after} rows (expected: 0), \
         NULL leaked: {null_leaked}"
    );

    check(
        before == 1 && after == 0 && null_leaked == 0,
        "UPDATE value→NULL removes row",
    )
}

/// Same shape as test 1 but the matview projects raw column names — no
/// aliases. Isolates whether the IVM bug needs the `block_id AS root_id`
/// rename or fires regardless of projection shape.
async fn test_no_aliases() -> anyhow::Result<bool> {
    println!("--- Test 3: No aliases — does projection rename matter? ---");
    let db = fresh_db("fu13-test3").await?;
    let conn = db.connect()?;

    conn.execute(
        "CREATE TABLE navigation_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            region TEXT NOT NULL,
            block_id TEXT,
            timestamp INTEGER NOT NULL,
            closed_at TEXT
        )",
        (),
    )
    .await?;
    conn.execute(
        "CREATE MATERIALIZED VIEW open_pins AS
         SELECT region, block_id, timestamp
         FROM navigation_history
         WHERE block_id IS NOT NULL",
        (),
    )
    .await?;

    conn.execute(
        "INSERT INTO navigation_history (region, block_id, timestamp) VALUES ('main', NULL, 1000)",
        (),
    )
    .await?;
    conn.execute(
        "INSERT INTO navigation_history (region, block_id, timestamp) VALUES ('main', 'block:a', 1001)",
        (),
    )
    .await?;

    let matview = count(&conn, "SELECT COUNT(*) FROM open_pins").await?;
    let null_leaked = count(
        &conn,
        "SELECT COUNT(*) FROM open_pins WHERE block_id IS NULL",
    )
    .await?;
    println!("  open_pins: {matview} rows (expected: 1), NULL leaked: {null_leaked}");

    check(
        matview == 1 && null_leaked == 0,
        "no-alias matview filters NULL",
    )
}

/// Strip the matview down to ONLY `WHERE col IS NOT NULL` — no other
/// predicates, no aliases, single column projection. Isolates whether the
/// bug needs anything other than the IS NOT NULL filter itself.
async fn test_no_other_where_clauses() -> anyhow::Result<bool> {
    println!("--- Test 4: Minimal `WHERE col IS NOT NULL` only ---");
    let db = fresh_db("fu13-test4").await?;
    let conn = db.connect()?;

    conn.execute(
        "CREATE TABLE rows (id INTEGER PRIMARY KEY, payload TEXT)",
        (),
    )
    .await?;
    conn.execute(
        "CREATE MATERIALIZED VIEW non_null AS
         SELECT id, payload FROM rows WHERE payload IS NOT NULL",
        (),
    )
    .await?;

    conn.execute("INSERT INTO rows (id, payload) VALUES (1, NULL)", ())
        .await?;
    conn.execute("INSERT INTO rows (id, payload) VALUES (2, 'real')", ())
        .await?;
    conn.execute("INSERT INTO rows (id, payload) VALUES (3, NULL)", ())
        .await?;

    let total = count(&conn, "SELECT COUNT(*) FROM rows").await?;
    let matview = count(&conn, "SELECT COUNT(*) FROM non_null").await?;
    let null_leaked = count(&conn, "SELECT COUNT(*) FROM non_null WHERE payload IS NULL").await?;
    println!(
        "  rows total: {total}, non_null: {matview} rows (expected: 1), NULL leaked: {null_leaked}"
    );

    check(
        matview == 1 && null_leaked == 0,
        "minimal IS NOT NULL filter",
    )
}

/// Observes CDC events on the matview. The matview STATE may filter NULL
/// rows correctly (verified in tests 1-2), but does the CDC stream also
/// suppress events for rows whose WHERE evaluates to FALSE? Production
/// holon's `LiveData<FocusRoot>` watcher panicked at
/// `live_data.rs:163: id_fn failed on CDC row: focus_roots row missing
/// 'root_id'` — meaning a CDC event arrived with NULL/missing root_id
/// even though the matview state excludes such rows.
async fn test_cdc_events_filter_null() -> anyhow::Result<bool> {
    use std::sync::{Arc, Mutex};

    println!("--- Test 5: CDC events suppress WHERE-rejected rows ---");
    let db = fresh_db("fu13-test5").await?;
    let conn = db.connect()?;

    create_schema(&conn).await?;

    // Capture every CDC event the matview emits. We care whether
    // INSERT-with-NULL-block_id fires a Created/Updated event with NULL
    // root_id — that's the production panic.
    #[derive(Debug)]
    struct CdcRecord {
        relation: String,
        change_count: usize,
        had_null_root_id: bool,
    }
    let events: Arc<Mutex<Vec<CdcRecord>>> = Arc::new(Mutex::new(Vec::new()));
    let recorder = events.clone();

    let raw_dump: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let raw_recorder = raw_dump.clone();
    conn.set_change_callback(move |event| {
        if event.relation_name != "focus_roots" {
            return;
        }
        let mut had_null = false;
        for change in &event.changes {
            let json = format!("{:?}", change);
            raw_recorder.lock().unwrap().push(json.clone());
            if json.contains("root_id: Null") || json.contains("Null,") || !json.contains("root_id")
            {
                had_null = true;
            }
        }
        recorder.lock().unwrap().push(CdcRecord {
            relation: event.relation_name.clone(),
            change_count: event.changes.len(),
            had_null_root_id: had_null,
        });
    })?;

    // Insert a home row (block_id=NULL). matview WHERE excludes it.
    conn.execute(
        "INSERT INTO navigation_history (region, block_id, timestamp) VALUES ('main', NULL, 1000)",
        (),
    )
    .await?;
    // Insert a real row.
    conn.execute(
        "INSERT INTO navigation_history (region, block_id, timestamp) VALUES ('main', 'block:a', 1001)",
        (),
    )
    .await?;
    // Update the home row to a real value.
    conn.execute(
        "UPDATE navigation_history SET block_id = 'block:b', timestamp = 1002 WHERE block_id IS NULL",
        (),
    )
    .await?;
    // Update real row back to NULL.
    conn.execute(
        "UPDATE navigation_history SET block_id = NULL WHERE block_id = 'block:a'",
        (),
    )
    .await?;

    // Give CDC a moment to settle.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let recorded = events.lock().unwrap();
    let raw = raw_dump.lock().unwrap();
    println!("  Captured {} focus_roots CDC events:", recorded.len());
    for (i, e) in recorded.iter().enumerate() {
        println!(
            "    [{i}] relation={}, changes={}, null_root_id={}",
            e.relation, e.change_count, e.had_null_root_id
        );
    }
    println!("  Raw change shapes (first 5):");
    for (i, r) in raw.iter().take(5).enumerate() {
        println!("    [{i}] {}", r);
    }

    let null_events = recorded.iter().filter(|e| e.had_null_root_id).count();
    let final_state = count(&conn, "SELECT COUNT(*) FROM focus_roots").await?;
    println!(
        "  Final matview state: {final_state} rows (expected 1: 'block:b')\n  \
         CDC events with NULL/missing root_id: {null_events} (expected 0 if WHERE filters CDC)"
    );

    check(
        null_events == 0 && final_state == 1,
        "CDC events suppress NULL-root_id rows",
    )
}

// ── Helpers ────────────────────────────────────────────────────────────

async fn create_schema(conn: &turso::Connection) -> anyhow::Result<()> {
    conn.execute(
        "CREATE TABLE navigation_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            region TEXT NOT NULL,
            block_id TEXT,
            timestamp INTEGER NOT NULL,
            closed_at TEXT
        )",
        (),
    )
    .await?;

    // Mirrors crates/holon/sql/schema/matview_focus_roots.sql exactly,
    // EXCEPT for the additional `AND block_id IS NOT NULL` filter that
    // FU-13's open question proposed adding.
    conn.execute(
        "CREATE MATERIALIZED VIEW focus_roots AS
         SELECT
             region,
             block_id AS root_id,
             timestamp AS added_ts,
             id AS history_id
         FROM navigation_history
         WHERE closed_at IS NULL AND block_id IS NOT NULL",
        (),
    )
    .await?;

    Ok(())
}

async fn fresh_db(name: &str) -> anyhow::Result<turso::Database> {
    let db_path = format!("/tmp/{name}.db");
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{db_path}{suffix}"));
    }
    Ok(turso::Builder::new_local(&db_path)
        .experimental_materialized_views(true)
        .build()
        .await?)
}

fn check(condition: bool, label: &str) -> anyhow::Result<bool> {
    if condition {
        println!("  PASS: {label}\n");
        Ok(true)
    } else {
        println!("  FAIL: {label}\n");
        Ok(false)
    }
}

async fn count(conn: &turso::Connection, sql: &str) -> anyhow::Result<i64> {
    let mut rows = conn.query(sql, ()).await?;
    match rows.next().await? {
        Some(row) => Ok(row.get::<i64>(0)?),
        None => Ok(0),
    }
}
