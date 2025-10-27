//! Minimal reproducer for Turso IVM losing rows in a recursive CTE matview
//! when a row's UPDATE causes it to transition between the base case and the
//! recursive case of the WITH RECURSIVE union.
//!
//! ## Hypothesis
//!
//! The matview `blocks_with_paths` defines:
//!
//! ```sql
//! WITH RECURSIVE paths AS (
//!     -- Base: block whose parent is a document
//!     SELECT id, parent_id, '/' || id AS path
//!     FROM block WHERE parent_id LIKE 'doc:%'
//!     UNION ALL
//!     -- Recursive: block joined via parent path
//!     SELECT b.id, b.parent_id, p.path || '/' || b.id AS path
//!     FROM block b INNER JOIN paths p ON b.parent_id = p.id
//! )
//! SELECT * FROM paths
//! ```
//!
//! When a block's `parent_id` is UPDATEd from a `doc:%` value to another
//! block's id (or vice versa), the row needs to move between the base case
//! and the recursive case. We hypothesize Turso IVM doesn't reconcile this
//! transition and ends up dropping the row.
//!
//! ## What this reproduces in production
//!
//! Drag&drop reparents a block (source.parent_id changes from doc to target).
//! After the UPDATE, the moved block disappears from `blocks_with_paths`,
//! breaking any consumer that joins on it (the main panel GQL query, the
//! `descendants` PRQL virtual table, etc.). See
//! `crates/holon-integration-tests/src/pbt/sut.rs::DragDropBlock` and the
//! inv16 invariant for the production-side observation.
//!
//! Run with: `cargo run --example turso_ivm_update_parent_id_repro`

use std::path::Path;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let db_path = "/tmp/turso-ivm-update-parent-id.db";
    if Path::new(db_path).exists() {
        std::fs::remove_file(db_path)?;
    }
    let _ = std::fs::remove_file(format!("{}-wal", db_path));
    let _ = std::fs::remove_file(format!("{}-shm", db_path));

    println!("Creating database at {}", db_path);
    let db = turso::Builder::new_local(db_path)
        .experimental_materialized_views(true)
        .build()
        .await?;
    let conn = db.connect()?;

    println!("\n[STEP 1] Creating schema...");
    conn.execute(
        r#"CREATE TABLE block (
            id TEXT PRIMARY KEY,
            parent_id TEXT,
            content TEXT
        )"#,
        (),
    )
    .await?;

    conn.execute(
        r#"CREATE MATERIALIZED VIEW blocks_with_paths AS
           WITH RECURSIVE paths AS (
               SELECT id, parent_id, content, '/' || id AS path
               FROM block WHERE parent_id LIKE 'doc:%'
               UNION ALL
               SELECT b.id, b.parent_id, b.content, p.path || '/' || b.id AS path
               FROM block b INNER JOIN paths p ON b.parent_id = p.id
           )
           SELECT * FROM paths"#,
        (),
    )
    .await?;

    println!("\n[STEP 2] Inserting 3 sibling blocks under doc:root...");
    for (id, content) in &[
        ("target", "Target"),
        ("source", "Source"),
        ("third", "Third"),
    ] {
        conn.execute(
            "INSERT INTO block (id, parent_id, content) VALUES (?, 'doc:root', ?)",
            turso::params![*id, *content],
        )
        .await?;
    }

    println!("\n[STEP 3] Verify all 3 are in blocks_with_paths (base case)...");
    dump_paths(&conn).await?;

    println!(
        "\n[STEP 4] UPDATE source.parent_id = 'target' (3 sequential UPDATEs, like move_block)..."
    );
    // move_block in holon-core/src/traits.rs:564 does 3 set_field calls:
    //   1. parent_id
    //   2. sort_key  (we don't have that column here, so skip)
    //   3. depth     (we don't have that column here, so skip)
    // To better match production, do 3 sequential UPDATEs on different fields.
    conn.execute(
        "UPDATE block SET parent_id = 'target' WHERE id = 'source'",
        (),
    )
    .await?;
    conn.execute(
        "UPDATE block SET content = 'Source v2' WHERE id = 'source'",
        (),
    )
    .await?;
    conn.execute(
        "UPDATE block SET content = 'Source v3' WHERE id = 'source'",
        (),
    )
    .await?;

    println!("\n[STEP 5] Re-query blocks_with_paths (expecting all 3, source nested):");
    let final_count = dump_paths(&conn).await?;

    if final_count == 3 {
        println!("\n=== PASS ===");
        println!("All 3 rows present. Turso IVM correctly handled the UPDATE.");
        Ok(())
    } else {
        println!("\n=== BUG REPRODUCED ===");
        println!(
            "Expected 3 rows, got {}. Turso IVM dropped the moved row.",
            final_count
        );
        std::process::exit(1);
    }
}

async fn dump_paths(conn: &turso::Connection) -> anyhow::Result<usize> {
    let mut rows = conn
        .query(
            "SELECT id, parent_id, path FROM blocks_with_paths ORDER BY path",
            (),
        )
        .await?;
    let mut count = 0;
    while let Some(row) = rows.next().await? {
        let id: String = row.get(0)?;
        let parent_id: String = row.get(1)?;
        let path: String = row.get(2)?;
        println!("  id={:8} parent={:10} path={}", id, parent_id, path);
        count += 1;
    }
    Ok(count)
}
