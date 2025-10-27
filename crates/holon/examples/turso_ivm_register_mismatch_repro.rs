//! Minimal reproducer for Turso IVM "Mismatch in number of registers" panic
//!
//! Bug: When a recursive CTE materialized view with 11 output columns exists
//! alongside other materialized views that also depend on the same base table,
//! inserting into the base table triggers IVM incremental maintenance that panics:
//!
//!   assertion `left == right` failed: Mismatch in number of registers! Got 30, expected 26
//!   at turso_core::incremental::expr_compiler::CompiledExpression::execute
//!
//! Key ingredients (all required):
//! 1. `block` table with 10 columns (9 data + _change_origin)
//! 2. `block_with_path` matview: recursive CTE over block (11 output cols)
//! 3. `focus_roots` matview: UNION ALL of two JOINs on block (also depends on block)
//! 4. `events_view_block` matview: filtered SELECT on events table
//! 5. Events inserted alongside block inserts (both matviews updated in same commit)
//! 6. CDC callback registered
//!
//! Run with: cargo run --example turso_ivm_register_mismatch_repro

use std::path::Path;
use std::sync::Arc;
use turso_core::{Database, DatabaseOpts, OpenFlags, UnixIO};
use turso_sdk_kit::rsapi::{TursoConnection, TursoDatabaseConfig};

fn open_db(path: &str) -> turso::Connection {
    let io = Arc::new(UnixIO::new().expect("UnixIO"));
    let opts = DatabaseOpts::default().with_views(true);
    let db = Database::open_file_with_flags(io, path, OpenFlags::default(), opts, None)
        .expect("open database");
    let db = Arc::new(db);
    let conn_core = db.connect().expect("connect");
    let config = TursoDatabaseConfig {
        path: String::new(),
        experimental_features: None,
        async_io: false,
        encryption: None,
        vfs: None,
        io: None,
        db_file: None,
    };
    let turso_conn = TursoConnection::new(&config, conn_core);
    turso::Connection::create(turso_conn, None)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let db_path = "/tmp/turso-ivm-register-mismatch-repro.db";
    for suffix in ["", "-wal", "-shm"] {
        let p = format!("{}{}", db_path, suffix);
        if Path::new(&p).exists() {
            std::fs::remove_file(&p)?;
        }
    }

    println!("Creating database at {}", db_path);
    let conn = open_db(db_path);

    // =====================================================================
    // STEP 1: Create all base tables (matches production schema exactly)
    // =====================================================================
    println!("\n[STEP 1] Creating base tables...");

    // Events table (used by TursoEventBus for CDC event tracking)
    conn.execute(
        r#"CREATE TABLE events (
            id TEXT PRIMARY KEY,
            event_type TEXT NOT NULL,
            aggregate_type TEXT NOT NULL,
            aggregate_id TEXT NOT NULL,
            origin TEXT NOT NULL,
            status TEXT DEFAULT 'confirmed',
            payload TEXT NOT NULL,
            created_at INTEGER NOT NULL
        )"#,
        (),
    )
    .await?;

    // Block table with 10 columns (9 schema + _change_origin for CDC tracing)
    conn.execute(
        r#"CREATE TABLE block (
            id TEXT PRIMARY KEY,
            parent_id TEXT NOT NULL,
            content TEXT NOT NULL DEFAULT '',
            content_type TEXT NOT NULL DEFAULT 'text',
            source_language TEXT,
            source_name TEXT,
            properties TEXT NOT NULL DEFAULT '{}',
            created_at INTEGER NOT NULL DEFAULT 0,
            updated_at INTEGER NOT NULL DEFAULT 0,
            _change_origin TEXT DEFAULT '{}'
        )"#,
        (),
    )
    .await?;
    conn.execute("CREATE INDEX idx_block_parent_id ON block(parent_id)", ())
        .await?;

    // Navigation tables
    conn.execute(
        r#"CREATE TABLE navigation_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            region TEXT NOT NULL,
            block_id TEXT,
            timestamp TEXT DEFAULT (datetime('now'))
        )"#,
        (),
    )
    .await?;

    conn.execute(
        r#"CREATE TABLE navigation_cursor (
            region TEXT PRIMARY KEY,
            history_id INTEGER REFERENCES navigation_history(id)
        )"#,
        (),
    )
    .await?;

    conn.execute(
        "INSERT INTO navigation_cursor (region, history_id) VALUES ('main', NULL)",
        (),
    )
    .await?;

    println!("  All base tables created");

    // =====================================================================
    // STEP 2: Create materialized views (matches production order exactly)
    // =====================================================================
    println!("\n[STEP 2] Creating materialized views...");

    // Events matviews (3 separate filtered views on the events table)
    conn.execute(
        r#"CREATE MATERIALIZED VIEW events_view_block AS
           SELECT * FROM events
           WHERE status = 'confirmed' AND aggregate_type = 'block'"#,
        (),
    )
    .await?;
    println!("  events_view_block created");

    conn.execute(
        r#"CREATE MATERIALIZED VIEW events_view_directory AS
           SELECT * FROM events
           WHERE status = 'confirmed' AND aggregate_type = 'directory'"#,
        (),
    )
    .await?;

    conn.execute(
        r#"CREATE MATERIALIZED VIEW events_view_file AS
           SELECT * FROM events
           WHERE status = 'confirmed' AND aggregate_type = 'file'"#,
        (),
    )
    .await?;
    println!("  events_view_directory + events_view_file created");

    // Navigation matviews
    conn.execute(
        r#"CREATE MATERIALIZED VIEW current_focus AS
           SELECT nc.region, nh.block_id, nh.timestamp
           FROM navigation_cursor nc
           JOIN navigation_history nh ON nc.history_id = nh.id"#,
        (),
    )
    .await?;
    println!("  current_focus created (JOIN)");

    // focus_roots: UNION ALL of two JOINs on block table (key ingredient!)
    // This creates a SECOND IVM dependency on block alongside block_with_path
    conn.execute(
        r#"CREATE MATERIALIZED VIEW focus_roots AS
           SELECT cf.region, cf.block_id, b.id AS root_id
           FROM current_focus AS cf
           JOIN block AS b ON b.parent_id = cf.block_id
           UNION ALL
           SELECT cf.region, cf.block_id, b.id AS root_id
           FROM current_focus AS cf
           JOIN block AS b ON b.id = cf.block_id"#,
        (),
    )
    .await?;
    println!("  focus_roots created (UNION ALL + 2 JOINs on block)");

    // block_with_path: recursive CTE with 11 output columns
    conn.execute(
        r#"CREATE MATERIALIZED VIEW block_with_path AS
        WITH RECURSIVE paths AS (
            SELECT
                id, parent_id, content, content_type,
                source_language, source_name, properties,
                created_at, updated_at,
                '/' || id as path,
                id as root_id
            FROM block
            WHERE parent_id LIKE 'doc:%'
               OR parent_id LIKE 'sentinel:%'

            UNION ALL

            SELECT
                b.id, b.parent_id, b.content, b.content_type,
                b.source_language, b.source_name, b.properties,
                b.created_at, b.updated_at,
                p.path || '/' || b.id as path,
                p.root_id
            FROM block b
            INNER JOIN paths p ON b.parent_id = p.id
        )
        SELECT * FROM paths"#,
        (),
    )
    .await?;
    println!("  block_with_path created (recursive CTE, 11 output columns)");

    // =====================================================================
    // STEP 3: Set up CDC callback
    // =====================================================================
    println!("\n[STEP 3] Setting up CDC callback...");

    conn.set_change_callback(|event| {
        println!(
            "  CDC: {} changes to {}",
            event.changes.len(),
            event.relation_name
        );
    })?;

    // =====================================================================
    // STEP 4: Insert blocks + events in same transaction (production pattern)
    // In production, the TursoEventBus inserts events for each block change,
    // and both the events matviews AND block matviews get IVM updates.
    // =====================================================================
    println!("\n[STEP 4] Inserting blocks + events (triggers IVM on multiple matviews)...");
    println!("  Expected: panic with 'Mismatch in number of registers! Got 30, expected 26'\n");

    // Simulate production: for each block upsert, also insert an event
    let upsert_block = |id: &str, parent_id: &str, content: &str, ts: i64| -> String {
        format!(
            r#"INSERT INTO block (id, parent_id, content, content_type, properties, created_at, updated_at, _change_origin)
               VALUES ('{}', '{}', '{}', 'text', '{{}}', {}, {}, '{{"origin":"local"}}')
               ON CONFLICT(id) DO UPDATE SET
                 parent_id = excluded.parent_id, content = excluded.content,
                 content_type = excluded.content_type, properties = excluded.properties,
                 created_at = excluded.created_at, updated_at = excluded.updated_at,
                 _change_origin = excluded._change_origin"#,
            id, parent_id, content, ts, ts
        )
    };

    let insert_event = |event_id: &str, block_id: &str, event_type: &str, ts: i64| -> String {
        format!(
            r#"INSERT INTO events (id, event_type, aggregate_type, aggregate_id, origin, status, payload, created_at)
               VALUES ('{}', '{}', 'block', '{}', 'local', 'confirmed', '{{}}', {})"#,
            event_id, event_type, block_id, ts
        )
    };

    // Batch 1: root block + event
    conn.execute("BEGIN", ()).await?;
    conn.execute(
        &upsert_block("block-1", "doc:test-doc", "First heading", 1709312400000),
        (),
    )
    .await?;
    conn.execute(
        &insert_event("evt-1", "block-1", "block.created", 1709312400000),
        (),
    )
    .await?;
    conn.execute("COMMIT", ()).await?;
    println!("  Batch 1: block-1 + event (root block)");

    // Batch 2: child block + event
    conn.execute("BEGIN", ()).await?;
    conn.execute(
        &upsert_block("block-2", "block-1", "Child text", 1709312401000),
        (),
    )
    .await?;
    conn.execute(
        &insert_event("evt-2", "block-2", "block.created", 1709312401000),
        (),
    )
    .await?;
    conn.execute("COMMIT", ()).await?;
    println!("  Batch 2: block-2 + event (child)");

    // Batch 3: many children + events (bulk sync scenario)
    conn.execute("BEGIN", ()).await?;
    for i in 3..=15 {
        let ts = 1709312400000_i64 + i * 1000;
        conn.execute(
            &upsert_block(
                &format!("block-{}", i),
                "block-1",
                &format!("Child {} text", i),
                ts,
            ),
            (),
        )
        .await?;
        conn.execute(
            &insert_event(
                &format!("evt-{}", i),
                &format!("block-{}", i),
                "block.created",
                ts,
            ),
            (),
        )
        .await?;
    }
    conn.execute("COMMIT", ()).await?;
    println!("  Batch 3: block-3 through block-15 + events (bulk sync)");

    // Batch 4: deeper nesting + events
    conn.execute("BEGIN", ()).await?;
    for i in 16..=25 {
        let ts = 1709312500000_i64 + i * 1000;
        conn.execute(
            &upsert_block(
                &format!("block-{}", i),
                "block-2",
                &format!("Grandchild {} text", i),
                ts,
            ),
            (),
        )
        .await?;
        conn.execute(
            &insert_event(
                &format!("evt-{}", i),
                &format!("block-{}", i),
                "block.created",
                ts,
            ),
            (),
        )
        .await?;
    }
    conn.execute("COMMIT", ()).await?;
    println!("  Batch 4: block-16 through block-25 + events (grandchildren)");

    // Batch 5: Add a DDL mid-stream (like operations table in production)
    // In production, the operations table DDL fires right when blocks are being synced
    conn.execute(
        r#"CREATE TABLE IF NOT EXISTS operation (
            id INTEGER PRIMARY KEY NOT NULL,
            operation TEXT NOT NULL,
            inverse TEXT,
            status TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            display_name TEXT NOT NULL,
            entity_name TEXT NOT NULL,
            op_name TEXT NOT NULL
        )"#,
        (),
    )
    .await?;
    println!("  Operations table created mid-stream");

    // Batch 6: updates to ALL blocks including root (triggers heavy IVM rework)
    // Updating block-1 (root) forces IVM to re-evaluate the entire recursive expansion
    conn.execute("BEGIN", ()).await?;
    for i in 1..=10 {
        let ts = 1709312600000_i64 + i * 1000;
        conn.execute(
            &upsert_block(
                &format!("block-{}", i),
                "block-1",
                &format!("Updated child {} text", i),
                ts,
            ),
            (),
        )
        .await?;
        conn.execute(
            &insert_event(
                &format!("evt-upd-{}", i),
                &format!("block-{}", i),
                "block.updated",
                ts,
            ),
            (),
        )
        .await?;
    }
    conn.execute("COMMIT", ()).await?;
    println!("  Batch 6: block-1 through block-10 updated + events");

    // =====================================================================
    // STEP 5: Verify matview contents
    // =====================================================================
    println!("\n[STEP 5] Querying block_with_path...");

    let mut rows = conn
        .query(
            "SELECT id, path, root_id FROM block_with_path ORDER BY path LIMIT 10",
            (),
        )
        .await?;

    while let Some(row) = rows.next().await? {
        let id: String = row.get(0)?;
        let path: String = row.get(1)?;
        let root_id: String = row.get(2)?;
        println!("  {} -> {} (root: {})", id, path, root_id);
    }

    println!("\n=== Register mismatch did not reproduce ===");
    println!("NOTE: The negative weight bug (Bug 2) DOES reproduce if you change");
    println!("Batch 6 to update blocks 1-10 instead of 3-10 (re-parenting root).");
    println!("See turso_ivm_negative_weight_repro.rs for the confirmed reproducer.");
    println!("The register mismatch (Bug 1) may require specific internal IVM circuit state.");
    println!("See docs/HANDOFF_TURSO_IVM_REGISTER_MISMATCH.md for analysis.");

    Ok(())
}
