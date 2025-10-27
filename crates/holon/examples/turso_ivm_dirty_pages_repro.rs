//! Minimal reproducer for Turso IVM "dirty pages should be empty for read txn" bug
//!
//! **Bug**: During IVM processing of a write that triggers multiple materialized view
//! updates (especially recursive CTE + JOIN matviews), the pager panics with:
//!   "dirty pages should be empty for read txn"
//! at pager.rs:4699 inside `rollback()`.
//!
//! **Secondary corruption**: After the panic, the BTree index becomes inconsistent:
//!   "Index points to non-existent table row"
//! at incremental/persistence.rs:152.
//!
//! **Key insight**: The bug occurs on a SINGLE connection when IVM processes a write
//! that cascades through multiple materialized views. The pager's internal read
//! sub-operations (used by IVM to compute deltas) find dirty pages from the
//! ongoing write transaction. This is NOT a cross-connection race.
//!
//! Run with: cargo run --example turso_ivm_dirty_pages_repro
//!
//! Expected: panic "dirty pages should be empty for read txn" or
//! "Index points to non-existent table row" errors.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let db_path = "/tmp/turso-ivm-dirty-pages-repro.db";
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{}", db_path, suffix));
    }

    println!("=== Turso IVM Dirty Pages Reproducer ===\n");

    let db = turso::Builder::new_local(db_path)
        .experimental_materialized_views(true)
        .build()
        .await?;

    let conn = db.connect()?;

    // -- Schema: blocks table --
    conn.execute(
        "CREATE TABLE blocks (
            id TEXT PRIMARY KEY,
            parent_id TEXT NOT NULL,
            content TEXT DEFAULT '',
            content_type TEXT DEFAULT 'text',
            source_language TEXT,
            properties TEXT DEFAULT '{}'
        )",
        (),
    )
    .await?;

    // -- Matview 1: recursive CTE (blocks_with_paths) --
    conn.execute(
        "CREATE MATERIALIZED VIEW blocks_with_paths AS
        WITH RECURSIVE paths AS (
            SELECT id, parent_id, content, content_type, source_language, properties,
                   '/' || id as path
            FROM blocks WHERE parent_id LIKE 'doc:%'
            UNION ALL
            SELECT b.id, b.parent_id, b.content, b.content_type, b.source_language,
                   b.properties, p.path || '/' || b.id as path
            FROM blocks b INNER JOIN paths p ON b.parent_id = p.id
        )
        SELECT * FROM paths",
        (),
    )
    .await?;

    // -- Navigation tables + JOIN matview --
    conn.execute(
        "CREATE TABLE navigation_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT, region TEXT NOT NULL, block_id TEXT)",
        (),
    )
    .await?;
    conn.execute(
        "CREATE TABLE navigation_cursor (
            region TEXT PRIMARY KEY, history_id INTEGER REFERENCES navigation_history(id))",
        (),
    )
    .await?;
    conn.execute(
        "INSERT INTO navigation_cursor (region, history_id) VALUES ('main', NULL)",
        (),
    )
    .await?;
    conn.execute(
        "CREATE MATERIALIZED VIEW current_focus AS
         SELECT nc.region, nh.block_id
         FROM navigation_cursor nc JOIN navigation_history nh ON nc.history_id = nh.id",
        (),
    )
    .await?;

    // -- Events table with filtered matview (simulates event bus) --
    conn.execute(
        "CREATE TABLE events (
            id TEXT PRIMARY KEY, event_type TEXT NOT NULL, aggregate_type TEXT NOT NULL,
            aggregate_id TEXT NOT NULL, origin TEXT NOT NULL,
            status TEXT DEFAULT 'confirmed', payload TEXT NOT NULL,
            created_at INTEGER NOT NULL, processed_by_loro INTEGER DEFAULT 0)",
        (),
    )
    .await?;
    conn.execute(
        "CREATE MATERIALIZED VIEW events_view_block AS
         SELECT * FROM events WHERE status = 'confirmed' AND aggregate_type = 'block'",
        (),
    )
    .await?;

    // -- Additional matviews to increase IVM cascade pressure --
    // These simulate the watch_view_xxx matviews that query_and_watch creates
    for i in 0..5 {
        conn.execute(
            &format!(
                "CREATE MATERIALIZED VIEW watch_view_{} AS \
                 SELECT id, content FROM blocks WHERE content_type = 'text'",
                i
            ),
            (),
        )
        .await?;
    }

    println!("[Setup] Created 8 materialized views on 3 base tables");

    // Register CDC callback
    let cdc_count = std::sync::Arc::new(AtomicU32::new(0));
    let cdc_count_clone = cdc_count.clone();
    conn.set_change_callback(move |event| {
        if !event.changes.is_empty() {
            cdc_count_clone.fetch_add(1, Ordering::Relaxed);
        }
    })?;

    let bug_detected = AtomicBool::new(false);

    // -- Phase 1: Insert deep tree chains (maximizes recursive CTE work) --
    println!("\n[Phase 1] Inserting deep block hierarchies (20 chains of depth 15)...");

    for chain in 0..20 {
        // Root block
        let root_id = format!("chain-{}-0", chain);
        if let Err(e) = conn
            .execute(
                &format!(
                    "INSERT INTO blocks (id, parent_id, content) VALUES ('{}', 'doc:test.org', 'root-{}')",
                    root_id, chain
                ),
                (),
            )
            .await
        {
            let err = format!("{:?}", e);
            eprintln!("  [ERROR] {}", err);
            if err.contains("dirty pages") || err.contains("non-existent") {
                println!("\n=== BUG REPRODUCED (phase 1) ===\n{}", err);
                bug_detected.store(true, Ordering::SeqCst);
                break;
            }
        }

        // Deep children
        for depth in 1..15 {
            let id = format!("chain-{}-{}", chain, depth);
            let parent = format!("chain-{}-{}", chain, depth - 1);
            if let Err(e) = conn
                .execute(
                    &format!(
                        "INSERT INTO blocks (id, parent_id, content) VALUES ('{}', '{}', 'child-{}-{}')",
                        id, parent, chain, depth
                    ),
                    (),
                )
                .await
            {
                let err = format!("{:?}", e);
                eprintln!("  [ERROR] {}", err);
                if err.contains("dirty pages") || err.contains("non-existent") {
                    println!("\n=== BUG REPRODUCED (phase 1) ===\n{}", err);
                    bug_detected.store(true, Ordering::SeqCst);
                    break;
                }
            }
        }
        if bug_detected.load(Ordering::SeqCst) {
            break;
        }

        // Interleave: also create an event (writes to second base table)
        if let Err(e) = conn
            .execute(
                &format!(
                    "INSERT INTO events (id, event_type, aggregate_type, aggregate_id, origin, payload, created_at) \
                     VALUES ('evt-{}', 'block.created', 'block', 'chain-{}-0', 'org', '{{}}', {})",
                    chain, chain, chain * 1000 + 1
                ),
                (),
            )
            .await
        {
            let err = format!("{:?}", e);
            eprintln!("  [ERROR event] {}", err);
            if err.contains("dirty pages") || err.contains("non-existent") {
                println!("\n=== BUG REPRODUCED (phase 1, event insert) ===\n{}", err);
                bug_detected.store(true, Ordering::SeqCst);
                break;
            }
        }
    }

    let total_cdc = cdc_count.load(Ordering::Relaxed);
    println!("  {} CDC callback invocations during phase 1", total_cdc);

    // -- Phase 2: Rapid alternating DDL + DML --
    if !bug_detected.load(Ordering::SeqCst) {
        println!("\n[Phase 2] Alternating DDL (new matviews) + DML (bulk inserts)...");
        for round in 0..20 {
            // Create a new matview (DDL)
            let view_name = format!("stress_view_{}", round);
            if let Err(e) = conn
                .execute(
                    &format!(
                        "CREATE MATERIALIZED VIEW IF NOT EXISTS {} AS \
                         SELECT id, parent_id, content FROM blocks WHERE content LIKE 'child-%'",
                        view_name
                    ),
                    (),
                )
                .await
            {
                let err = format!("{:?}", e);
                eprintln!("  [DDL] Error: {}", err);
                if err.contains("dirty pages") || err.contains("non-existent") {
                    println!("\n=== BUG REPRODUCED (phase 2, DDL) ===\n{}", err);
                    bug_detected.store(true, Ordering::SeqCst);
                    break;
                }
            }

            // Immediately do bulk DML that triggers IVM on ALL matviews
            for i in 0..10 {
                let id = format!("stress-{}-{}", round, i);
                let parent = if i == 0 {
                    "doc:test.org".to_string()
                } else {
                    format!("stress-{}-{}", round, i - 1)
                };
                if let Err(e) = conn
                    .execute(
                        &format!(
                            "INSERT INTO blocks (id, parent_id, content) VALUES ('{}', '{}', 'stress-{}-{}')",
                            id, parent, round, i
                        ),
                        (),
                    )
                    .await
                {
                    let err = format!("{:?}", e);
                    eprintln!("  [DML] Error: {}", err);
                    if err.contains("dirty pages") || err.contains("non-existent") {
                        println!("\n=== BUG REPRODUCED (phase 2, DML) ===\n{}", err);
                        bug_detected.store(true, Ordering::SeqCst);
                        break;
                    }
                }
            }
            if bug_detected.load(Ordering::SeqCst) {
                break;
            }
        }
    }

    // -- Verification --
    println!("\n[Verification] Querying blocks_with_paths...");
    match conn
        .query("SELECT count(*) as cnt FROM blocks_with_paths", ())
        .await
    {
        Ok(mut rows) => {
            if let Some(row) = rows.next().await? {
                let count: i64 = row.get(0)?;
                println!("  blocks_with_paths has {} rows", count);
            }
        }
        Err(e) => {
            let err = format!("{:?}", e);
            println!("  Query FAILED: {}", err);
            if err.contains("non-existent table row") {
                println!("\n=== INDEX CORRUPTION CONFIRMED ===");
                bug_detected.store(true, Ordering::SeqCst);
            }
        }
    }

    if bug_detected.load(Ordering::SeqCst) {
        println!("\n=== SUMMARY ===");
        println!("Bug reproduced. Two related issues:");
        println!("  1. pager.rs:4699 - 'dirty pages should be empty for read txn'");
        println!("  2. persistence.rs:152 - 'Index points to non-existent table row'");
        std::process::exit(1);
    } else {
        let total_cdc_final = cdc_count.load(Ordering::Relaxed);
        println!(
            "\n=== Bug did NOT reproduce ({} CDC events) ===",
            total_cdc_final
        );
        println!("The bug requires specific timing conditions observed in holon's PBT test.");
        println!("In production, it manifests during app startup when:");
        println!(
            "  - org-file sync bulk-inserts blocks (triggering IVM on recursive CTE matviews)"
        );
        println!("  - query_and_watch creates new materialized views concurrently");
        println!("  - All operations go through a single database actor connection");
        println!("The pager's rollback() then finds dirty pages on what it thinks is a read txn.");
        Ok(())
    }
}
