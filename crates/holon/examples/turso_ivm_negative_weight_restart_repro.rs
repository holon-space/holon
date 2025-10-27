//! Reproducer for Turso IVM "expected a positive weight, found -1" on app restart.
//!
//! This simulates the Holon app restart scenario:
//!   1. First "session": create table + matview, insert rows → works fine
//!   2. Close connection (drop)
//!   3. Second "session": reopen same DB, matview already exists (IF NOT EXISTS),
//!      INSERT OR REPLACE all rows (simulates org sync re-importing same blocks)
//!   4. Query matview → "expected a positive weight, found -1"
//!
//! This differs from the original reproducer which did everything in one session.
//! The bug may specifically involve IVM state persisted to disk between sessions.
//!
//! Run with: cargo run --example turso_ivm_negative_weight_restart_repro

use std::sync::Arc;
use turso_core::{Database, DatabaseOpts, OpenFlags, UnixIO};

fn open_db(path: &str) -> (Arc<Database>, turso::Connection) {
    let io = Arc::new(UnixIO::new().expect("UnixIO"));
    let opts = DatabaseOpts::default().with_views(true);
    let db = Database::open_file_with_flags(io, path, OpenFlags::default(), opts, None)
        .expect("open database");
    let conn_core = db.connect().expect("connect");
    let config = turso_sdk_kit::rsapi::TursoDatabaseConfig {
        path: String::new(),
        experimental_features: None,
        async_io: false,
        encryption: None,
        vfs: None,
        io: None,
        db_file: None,
    };
    let turso_conn = turso_sdk_kit::rsapi::TursoConnection::new(&config, conn_core);
    let conn = turso::Connection::create(turso_conn, None);
    (db, conn)
}

const NUM_ROOTS: usize = 8;
const CHAIN_DEPTH: usize = 20;

async fn insert_all_rows(conn: &turso::Connection, version: &str) -> anyhow::Result<()> {
    for i in 0..NUM_ROOTS {
        let root = format!("r{}", i);
        conn.execute(
            &format!(
                "INSERT OR REPLACE INTO t VALUES ('{}', 'doc:d1', 'H{} {}')",
                root, i, version
            ),
            (),
        )
        .await?;
        for j in 0..CHAIN_DEPTH {
            let parent = if j == 0 {
                root.clone()
            } else {
                format!("c{}-{}", i, j - 1)
            };
            conn.execute(
                &format!(
                    "INSERT OR REPLACE INTO t VALUES ('c{}-{}', '{}', 'C {}')",
                    i, j, parent, version
                ),
                (),
            )
            .await?;
        }
    }
    Ok(())
}

async fn query_matview(conn: &turso::Connection, label: &str) -> anyhow::Result<bool> {
    match conn.query("SELECT COUNT(*) FROM mv", ()).await {
        Ok(mut rows) => match rows.next().await {
            Ok(Some(row)) => {
                let count: i64 = row.get(0)?;
                let expected = (NUM_ROOTS * (1 + CHAIN_DEPTH)) as i64;
                if count == expected {
                    println!("    [{}] OK — {} rows", label, count);
                    Ok(true)
                } else {
                    println!(
                        "    [{}] WRONG COUNT: {} (expected {})",
                        label, count, expected
                    );
                    Ok(false)
                }
            }
            Ok(None) => {
                println!("    [{}] No rows returned", label);
                Ok(false)
            }
            Err(e) => {
                println!("    [{}] BUG: {}", label, e);
                Ok(false)
            }
        },
        Err(e) => {
            println!("    [{}] BUG: {}", label, e);
            Ok(false)
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let db_path = "/tmp/turso-ivm-restart-repro.db";

    // Clean up from previous runs
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{}", db_path, suffix));
    }

    println!("=== Turso IVM Negative Weight — App Restart Reproducer ===\n");

    // === SESSION 1: First app run ===
    println!("--- Session 1 (first app run) ---");
    {
        let (_db, conn) = open_db(db_path);

        conn.execute(
            "CREATE TABLE t (id TEXT PRIMARY KEY, pid TEXT, val TEXT)",
            (),
        )
        .await?;

        conn.execute(
            "CREATE MATERIALIZED VIEW IF NOT EXISTS mv AS
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

        println!("[1] Inserting {} rows...", NUM_ROOTS * (1 + CHAIN_DEPTH));
        insert_all_rows(&conn, "v1").await?;
        query_matview(&conn, "session1").await?;

        println!("[1] Closing connection (simulating app exit)\n");
        // conn and _db are dropped here
    }

    // === SESSION 2: Second app run (reopen same DB) ===
    println!("--- Session 2 (app restart, same DB) ---");
    {
        let (_db, conn) = open_db(db_path);

        // This is a no-op since the matview already exists
        conn.execute(
            "CREATE MATERIALIZED VIEW IF NOT EXISTS mv AS
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

        // Verify matview is readable before any mutations
        println!("[2] Querying matview before mutations...");
        query_matview(&conn, "before-sync").await?;

        // Re-insert all rows (simulates org sync on app startup)
        println!("[2] Re-inserting all rows via INSERT OR REPLACE (simulates org sync)...");
        insert_all_rows(&conn, "v2").await?;

        println!("[2] Querying matview after sync...");
        let ok = query_matview(&conn, "after-sync").await?;

        if !ok {
            println!("\n=== BUG REPRODUCED ===");
            println!("IVM weights corrupted after INSERT OR REPLACE on restart.");
            println!("The matview existed from the previous session, and re-syncing");
            println!("the same rows corrupted the IVM internal state.");
        } else {
            println!("\n=== NO BUG (fixed in this Turso build) ===");
        }
    }

    // === SESSION 3: Third run (if bug is cumulative) ===
    println!("\n--- Session 3 (third restart) ---");
    {
        let (_db, conn) = open_db(db_path);

        conn.execute(
            "CREATE MATERIALIZED VIEW IF NOT EXISTS mv AS
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

        println!("[3] Querying matview before mutations...");
        query_matview(&conn, "before-sync-3").await?;

        println!("[3] Re-inserting all rows via INSERT OR REPLACE (third sync)...");
        insert_all_rows(&conn, "v3").await?;

        println!("[3] Querying matview after sync...");
        let ok = query_matview(&conn, "after-sync-3").await?;

        if !ok {
            println!("\n=== BUG REPRODUCED on 3rd restart ===");
        } else {
            println!("\n=== All 3 sessions OK ===");
        }
    }

    Ok(())
}
