//! Minimal reproducer for Turso IVM "expected a positive weight, found -1"
//!
//! **Bug**: Any row modification (UPDATE, INSERT OR REPLACE, ON CONFLICT DO UPDATE)
//! to a table with a recursive CTE materialized view corrupts IVM internal weights.
//!
//! **Minimal trigger**: A self-referencing table with a recursive CTE matview,
//! 8 root nodes with chains of 20 children each, then INSERT OR REPLACE all rows.
//! No CDC, no other matviews, no OR conditions needed.
//!
//! **Confirmed** via:
//! - This Rust example (100% repro)
//! - Pure SQL via holon-live MCP (100% repro, even with 2 rows when other matviews exist)
//! - Holon production app (100% repro on every startup)
//!
//! Run with: cargo run --example turso_ivm_negative_weight_repro

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
    let db_path = "/tmp/turso-ivm-negative-weight.db";
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{}", db_path, suffix));
    }

    println!("=== Turso IVM Negative Weight — Minimal Reproducer ===\n");

    let conn = open_db(db_path);

    // Step 1: Self-referencing table
    conn.execute(
        "CREATE TABLE t (id TEXT PRIMARY KEY, pid TEXT, val TEXT)",
        (),
    )
    .await?;

    // Step 2: Recursive CTE materialized view
    conn.execute(
        "CREATE MATERIALIZED VIEW mv AS
        WITH RECURSIVE tree AS (
            SELECT id, pid, val, '/' || id as path
            FROM t WHERE pid LIKE 'doc:%'
            UNION ALL
            SELECT c.id, c.pid, c.val, p.path || '/' || c.id
            FROM t c INNER JOIN tree p ON c.pid = p.id
        ) SELECT * FROM tree",
        (),
    )
    .await?;

    // Step 3: Insert 8 chains of depth 20 (168 rows total)
    println!("[1] Inserting 168 rows (8 roots × 21 depth)...");
    for i in 0..8 {
        let root = format!("r{}", i);
        conn.execute(
            &format!("INSERT INTO t VALUES ('{}', 'doc:d1', 'H{}')", root, i),
            (),
        )
        .await?;
        for j in 0..20 {
            let parent = if j == 0 {
                root.clone()
            } else {
                format!("c{}-{}", i, j - 1)
            };
            conn.execute(
                &format!("INSERT INTO t VALUES ('c{}-{}', '{}', 'C')", i, j, parent),
                (),
            )
            .await?;
        }
    }

    // Verify
    let mut rows = conn.query("SELECT COUNT(*) FROM mv", ()).await?;
    let count: i64 = rows.next().await?.unwrap().get(0)?;
    println!("    mv has {} rows (correct: 168)", count);
    assert_eq!(count, 168);

    // Step 4: INSERT OR REPLACE all rows (simulates re-sync / upsert pattern)
    println!("[2] Re-inserting all 168 rows via INSERT OR REPLACE...");
    for i in 0..8 {
        let root = format!("r{}", i);
        conn.execute(
            &format!(
                "INSERT OR REPLACE INTO t VALUES ('{}', 'doc:d1', 'H{} v2')",
                root, i
            ),
            (),
        )
        .await?;
        for j in 0..20 {
            let parent = if j == 0 {
                root.clone()
            } else {
                format!("c{}-{}", i, j - 1)
            };
            conn.execute(
                &format!(
                    "INSERT OR REPLACE INTO t VALUES ('c{}-{}', '{}', 'C v2')",
                    i, j, parent
                ),
                (),
            )
            .await?;
        }
    }

    // Step 5: Query — triggers the negative weight error
    println!("[3] Querying mv...\n");
    match conn.query("SELECT COUNT(*) FROM mv", ()).await {
        Ok(mut rows) => match rows.next().await {
            Ok(Some(row)) => {
                let count: i64 = row.get(0)?;
                if count == 168 {
                    println!("    OK — {} rows (no bug in this Turso build)", count);
                } else {
                    println!(
                        "    WRONG COUNT: {} (expected 168, duplicate accumulation)",
                        count
                    );
                }
            }
            Ok(None) => println!("    No rows returned"),
            Err(e) => {
                println!("=== BUG REPRODUCED ===");
                println!("{}\n", e);
                println!("IVM internal weights went negative after INSERT OR REPLACE");
                println!("on rows in a recursive CTE materialized view.");
            }
        },
        Err(e) => {
            println!("=== BUG REPRODUCED ===");
            println!("{}\n", e);
        }
    }

    Ok(())
}
