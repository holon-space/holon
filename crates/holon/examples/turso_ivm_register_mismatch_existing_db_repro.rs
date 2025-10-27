//! Reproducer for Turso IVM "Mismatch in number of registers" panic using existing DB
//!
//! This opens an existing database that already has blocks/matviews and inserts
//! new blocks to trigger the IVM register mismatch. This tests the theory that
//! the bug requires existing data in the recursive CTE matview.
//!
//! Usage:
//!   # Copy production DB first:
//!   cp ~/Library/Application\ Support/space.holon/holon.db /tmp/turso-ivm-repro-prod-copy.db
//!
//!   cargo run --example turso_ivm_register_mismatch_existing_db_repro
//!
//! Known Turso IVM bugs this may trigger:
//! 1. "Mismatch in number of registers! Got 30, expected 26" (expr_compiler.rs:378)
//! 2. "Invalid data in materialized view: expected a positive weight, found -1"

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
    let db_path = "/tmp/turso-ivm-repro-prod-copy.db";
    if !Path::new(db_path).exists() {
        eprintln!("Database not found at {}", db_path);
        eprintln!("Copy production DB first:");
        eprintln!(
            "  cp ~/Library/Application\\ Support/space.holon/holon.db {}",
            db_path
        );
        std::process::exit(1);
    }

    println!("Opening existing database at {}", db_path);
    let conn = open_db(db_path);

    // Check existing state
    let mut rows = conn.query("SELECT COUNT(*) FROM block", ()).await?;
    if let Some(row) = rows.next().await? {
        let count: i64 = row.get(0)?;
        println!("  Existing blocks: {}", count);
    }

    let mut rows = conn
        .query("SELECT COUNT(*) FROM block_with_path", ())
        .await?;
    if let Some(row) = rows.next().await? {
        let count: i64 = row.get(0)?;
        println!("  block_with_path rows: {}", count);
    }

    let mut rows = conn.query("SELECT COUNT(*) FROM events", ()).await?;
    if let Some(row) = rows.next().await? {
        let count: i64 = row.get(0)?;
        println!("  Existing events: {}", count);
    }

    // Set up CDC
    println!("\nSetting up CDC callback...");
    conn.set_change_callback(|event| {
        println!(
            "  CDC: {} changes to {}",
            event.changes.len(),
            event.relation_name
        );
    })?;

    // Get a document URI to use as parent
    let mut rows = conn
        .query(
            "SELECT DISTINCT parent_id FROM block WHERE parent_id LIKE 'doc:%' LIMIT 1",
            (),
        )
        .await?;
    let doc_uri: String = if let Some(row) = rows.next().await? {
        row.get(0)?
    } else {
        "doc:test-doc".to_string()
    };
    println!("Using document URI: {}", doc_uri);

    // Get an existing block to use as parent for child blocks
    let mut rows = conn
        .query(
            "SELECT id FROM block WHERE parent_id LIKE 'doc:%' LIMIT 1",
            (),
        )
        .await?;
    let parent_block: String = if let Some(row) = rows.next().await? {
        row.get(0)?
    } else {
        "nonexistent".to_string()
    };
    println!("Using parent block: {}", parent_block);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;

    println!("\n--- Test 1: Single upsert into existing hierarchy ---");
    let result = conn
        .execute(
            &format!(
                r#"INSERT INTO block (id, parent_id, content, content_type, properties, created_at, updated_at, _change_origin)
                   VALUES ('repro-root-1', '{}', 'Repro root block', 'text', '{{}}', {}, {}, '{{"origin":"test"}}')
                   ON CONFLICT(id) DO UPDATE SET content = excluded.content, updated_at = excluded.updated_at, _change_origin = excluded._change_origin"#,
                doc_uri, now, now
            ),
            (),
        )
        .await;
    match &result {
        Ok(_) => println!("  OK"),
        Err(e) => println!("  ERROR: {}", e),
    }

    println!("\n--- Test 2: Batch upsert with events ---");
    let result = async {
        conn.execute("BEGIN", ()).await?;
        for i in 1..=20 {
            let ts = now + i * 100;
            conn.execute(
                &format!(
                    r#"INSERT INTO block (id, parent_id, content, content_type, properties, created_at, updated_at, _change_origin)
                       VALUES ('repro-child-{}', '{}', 'Repro child {} text', 'text', '{{}}', {}, {}, '{{"origin":"test"}}')
                       ON CONFLICT(id) DO UPDATE SET content = excluded.content, updated_at = excluded.updated_at, _change_origin = excluded._change_origin"#,
                    i, parent_block, i, ts, ts
                ),
                (),
            )
            .await?;
            conn.execute(
                &format!(
                    r#"INSERT INTO events (id, event_type, aggregate_type, aggregate_id, origin, status, payload, created_at)
                       VALUES ('repro-evt-{}', 'block.created', 'block', 'repro-child-{}', 'test', 'confirmed', '{{}}', {})"#,
                    i, i, ts
                ),
                (),
            )
            .await?;
        }
        conn.execute("COMMIT", ()).await
    }
    .await;
    match &result {
        Ok(_) => println!("  OK"),
        Err(e) => println!("  ERROR: {}", e),
    }

    println!("\n--- Test 3: Update existing blocks (triggers DELETE+INSERT in IVM) ---");
    let result = async {
        conn.execute("BEGIN", ()).await?;
        for i in 1..=20 {
            let ts = now + 10000 + i * 100;
            conn.execute(
                &format!(
                    r#"INSERT INTO block (id, parent_id, content, content_type, properties, created_at, updated_at, _change_origin)
                       VALUES ('repro-child-{}', 'repro-root-1', 'Updated repro child {} text', 'text', '{{}}', {}, {}, '{{"origin":"test"}}')
                       ON CONFLICT(id) DO UPDATE SET
                         parent_id = excluded.parent_id, content = excluded.content,
                         updated_at = excluded.updated_at, _change_origin = excluded._change_origin"#,
                    i, i, ts, ts
                ),
                (),
            )
            .await?;
        }
        conn.execute("COMMIT", ()).await
    }
    .await;
    match &result {
        Ok(_) => println!("  OK"),
        Err(e) => println!("  ERROR: {}", e),
    }

    println!("\n--- Test 4: Reparent blocks (moves in tree, heavy IVM) ---");
    let result = async {
        conn.execute("BEGIN", ()).await?;
        // Move some children under each other to create deeper paths
        for i in 2..=10 {
            let ts = now + 20000 + i * 100;
            conn.execute(
                &format!(
                    r#"INSERT INTO block (id, parent_id, content, content_type, properties, created_at, updated_at, _change_origin)
                       VALUES ('repro-child-{}', 'repro-child-{}', 'Reparented child {} text', 'text', '{{}}', {}, {}, '{{"origin":"test"}}')
                       ON CONFLICT(id) DO UPDATE SET
                         parent_id = excluded.parent_id, content = excluded.content,
                         updated_at = excluded.updated_at, _change_origin = excluded._change_origin"#,
                    i, i - 1, i, ts, ts
                ),
                (),
            )
            .await?;
        }
        conn.execute("COMMIT", ()).await
    }
    .await;
    match &result {
        Ok(_) => println!("  OK"),
        Err(e) => println!("  ERROR: {}", e),
    }

    // Cleanup test data
    println!("\n--- Cleanup ---");
    let _ = conn
        .execute("DELETE FROM block WHERE id LIKE 'repro-%'", ())
        .await;
    let _ = conn
        .execute("DELETE FROM events WHERE id LIKE 'repro-%'", ())
        .await;
    println!("  Test data cleaned up");

    println!("\n=== Done ===");
    Ok(())
}
