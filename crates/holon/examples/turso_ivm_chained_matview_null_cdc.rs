#![allow(dead_code, clippy::await_holding_lock, clippy::type_complexity)] // standalone repro
//! FU-13 follow-up: do CDC events on a chained matview leak rows that the
//! inner matview's WHERE filter excluded?
//!
//! Holon's `LiveData<FocusRoot>` watcher panicked at
//! `live_data.rs:163: id_fn failed on CDC row: focus_roots row missing
//! 'root_id'` after removing the test-side `WHERE root_id IS NOT NULL`
//! filter, even though:
//!   - the `focus_roots` matview's STATE correctly excludes NULL block_id rows
//!     on `aff40a84` (verified in turso_ivm_focus_roots_null_filter),
//!   - Turso's raw `set_change_callback` on `focus_roots` does NOT fire for
//!     excluded rows (verified there too).
//!
//! Suspected layer: `MatviewManager::watch` creates a CHAINED matview
//! (`CREATE MATERIALIZED VIEW watch_view_<hash> AS SELECT region, root_id
//! FROM focus_roots`) that the LiveData consumer subscribes to. If the
//! chained matview's CDC propagation doesn't honor the inner matview's
//! WHERE-rejected-row exclusion, the consumer sees events for rows that
//! "shouldn't exist."
//!
//! This repro mirrors that shape exactly and observes CDC on BOTH the
//! inner and outer matviews. Hypotheses:
//!
//! - H1: Inner matview correctly suppresses CDC; outer matview also suppresses
//!   (no events on either for NULL block_id rows). Then the bug is in holon's
//!   broadcast translation layer, not Turso. Production GQL via JOIN drops NULL
//!   anyway.
//!
//! - H2: Inner suppresses but outer leaks. Upstream Turso bug, file at
//!   `bigdata/turso/bugs/`.
//!
//! - H3: Outer leaks regardless of inner state (always fires per inner change).
//!   Different upstream bug shape.
//!
//! Run: cargo run --example turso_ivm_chained_matview_null_cdc

use std::sync::Arc;
use std::sync::Mutex;

#[derive(Debug, Clone)]
struct CdcRecord {
    relation: String,
    change_type: &'static str,
    bin_record_len: usize,
    raw: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("=== Chained matview CDC: WHERE-rejected rows ===\n");

    let db_path = "/tmp/turso-chained-cdc-repro.db";
    for ext in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{db_path}{ext}"));
    }
    let db = turso::Builder::new_local(db_path)
        .experimental_materialized_views(true)
        .build()
        .await?;
    let conn = db.connect()?;

    // Base table.
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

    // Inner matview — production focus_roots shape (with the post-aff40a84
    // `block_id IS NOT NULL` filter).
    conn.execute(
        "CREATE MATERIALIZED VIEW focus_roots AS
         SELECT region, block_id AS root_id, timestamp AS added_ts, id AS history_id
         FROM navigation_history
         WHERE closed_at IS NULL AND block_id IS NOT NULL",
        (),
    )
    .await?;

    // Outer matview — production watch_view shape (created by
    // MatviewManager::ensure_view for `SELECT region, root_id FROM focus_roots`).
    // Uses a deterministic name so the events recorder can filter.
    conn.execute(
        "CREATE MATERIALIZED VIEW watch_outer AS
         SELECT region, root_id FROM focus_roots",
        (),
    )
    .await?;

    let events: Arc<Mutex<Vec<CdcRecord>>> = Arc::new(Mutex::new(Vec::new()));
    let recorder = events.clone();

    let columns_seen: Arc<Mutex<Vec<(String, Vec<String>)>>> = Arc::new(Mutex::new(Vec::new()));
    let columns_recorder = columns_seen.clone();
    conn.set_change_callback(move |event| {
        if event.relation_name != "focus_roots" && event.relation_name != "watch_outer" {
            return;
        }
        // Capture event.columns once per relation so we can see what the
        // CDC translation layer would key off.
        {
            let mut cs = columns_recorder.lock().unwrap();
            if !cs.iter().any(|(r, _)| r == &event.relation_name) {
                let cols: Vec<String> = event.columns.iter().map(|c| format!("{:?}", c)).collect();
                cs.push((event.relation_name.clone(), cols));
            }
        }
        for change in &event.changes {
            let _raw = format!("{:?}", change);
            let (change_type, bin_len) = match &change.change {
                turso::DatabaseChangeType::Insert { bin_record } => ("Insert", bin_record.len()),
                turso::DatabaseChangeType::Update { bin_record } => ("Update", bin_record.len()),
                turso::DatabaseChangeType::Delete { bin_record } => ("Delete", bin_record.len()),
            };
            // Try to parse the record using parse_record() — the same path
            // process_cdc_event takes — and see what keys land.
            // An empty list would otherwise read the same whether the record
            // held no values or failed to decode — which this repro exists to
            // tell apart.
            let parsed_keys: Vec<String> = match change.parse_record() {
                Ok(values) => values.iter().map(|v| format!("{:?}", v)).collect(),
                Err(e) => vec![format!("<decode failed: {e}>")],
            };
            recorder.lock().unwrap().push(CdcRecord {
                relation: event.relation_name.clone(),
                change_type,
                bin_record_len: bin_len,
                raw: format!("parsed_values={:?}", parsed_keys),
            });
        }
    })?;

    // ── Probes ────────────────────────────────────────────────────────

    println!("Probe 1: INSERT home row (block_id=NULL) — both matviews should be silent.");
    conn.execute(
        "INSERT INTO navigation_history (region, block_id, timestamp) VALUES ('main', NULL, 1000)",
        (),
    )
    .await?;
    dump_events("after NULL INSERT", &events);

    println!("\nProbe 2: INSERT pinned row (block_id='block:a') — both should fire Insert.");
    conn.execute(
        "INSERT INTO navigation_history (region, block_id, timestamp) VALUES ('main', 'block:a', \
         1001)",
        (),
    )
    .await?;
    dump_events("after pinned INSERT", &events);

    println!("\nProbe 3: UPDATE pinned → NULL — both should fire Delete; no NULL leak.");
    conn.execute(
        "UPDATE navigation_history SET block_id = NULL WHERE region = 'main' AND block_id = \
         'block:a'",
        (),
    )
    .await?;
    dump_events("after UPDATE→NULL", &events);

    println!("\nProbe 4: INSERT another pinned row, then UPDATE NULL→pinned.");
    conn.execute(
        "INSERT INTO navigation_history (region, block_id, timestamp) VALUES ('main', 'block:b', \
         1002)",
        (),
    )
    .await?;
    conn.execute(
        "UPDATE navigation_history SET block_id = 'block:c' WHERE region = 'main' AND block_id IS \
         NULL AND timestamp = 1000",
        (),
    )
    .await?;
    dump_events("after UPDATE NULL→pinned", &events);

    // ── Analysis ──────────────────────────────────────────────────────

    println!("\n{}", "=".repeat(60));
    println!("\nevent.columns seen (per relation):");
    for (rel, cols) in columns_seen.lock().unwrap().iter() {
        println!("  {}:", rel);
        for c in cols {
            println!("    {}", c);
        }
    }

    let recs = events.lock().unwrap();
    let inner_events: Vec<_> = recs
        .iter()
        .filter(|r| r.relation == "focus_roots")
        .collect();
    let outer_events: Vec<_> = recs
        .iter()
        .filter(|r| r.relation == "watch_outer")
        .collect();

    println!(
        "\nfocus_roots (inner) emitted {} events:",
        inner_events.len()
    );
    for r in &inner_events {
        println!("  {} {}", r.change_type, r.raw);
    }
    println!(
        "\nwatch_outer (chained) emitted {} events:",
        outer_events.len()
    );
    for r in &outer_events {
        println!("  {} {}", r.change_type, r.raw);
    }

    // Check both inner and outer state agree.
    let inner_state = count(
        &conn,
        "SELECT COUNT(*) FROM focus_roots WHERE region = 'main'",
    )
    .await?;
    let outer_state = count(
        &conn,
        "SELECT COUNT(*) FROM watch_outer WHERE region = 'main'",
    )
    .await?;
    println!("\nFinal state — inner: {inner_state}, outer: {outer_state} (expected: 2 each)");

    // Hypothesis check.
    println!("\nHypothesis evaluation:");
    if inner_events.len() == outer_events.len() {
        println!("  H1 confirmed: outer event count matches inner. The chained matview");
        println!("                propagates CDC 1:1 from inner. NULL row exclusion is");
        println!("                correct end-to-end. The holon-side panic must originate");
        println!("                in the broadcast/translation layer, NOT Turso.");
    } else if outer_events.len() > inner_events.len() {
        println!("  H2/H3: outer has MORE events than inner. Chained matview is leaking");
        println!("         events. Upstream Turso bug — file at bigdata/turso/bugs/.");
    } else {
        println!("  Unexpected: outer has fewer events than inner. Investigate.");
    }

    Ok(())
}

fn dump_events(stage: &str, events: &Arc<Mutex<Vec<CdcRecord>>>) {
    let recs = events.lock().unwrap();
    println!("  events {} so far: {}", stage, recs.len());
    for (i, r) in recs.iter().enumerate() {
        if i < recs.len().saturating_sub(4) {
            continue;
        }
        println!("    [{}] {} {}", i, r.relation, r.change_type);
    }
}

async fn count(conn: &turso::Connection, sql: &str) -> anyhow::Result<i64> {
    let mut rows = conn.query(sql, ()).await?;
    match rows.next().await? {
        Some(row) => Ok(row.get::<i64>(0)?),
        None => Ok(0),
    }
}
