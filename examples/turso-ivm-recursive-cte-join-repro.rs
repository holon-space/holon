//! Minimal reproducer for Turso IVM "Invalid join commit state" error
//!
//! This bug occurs when:
//! 1. A materialized view uses a recursive CTE with INNER JOIN
//! 2. CDC callbacks are active via set_view_change_callback()
//! 3. Data is inserted into the base table, triggering IVM update
//!
//! The error manifests as "Invalid join commit state" during the IVM commit phase.
//!
//! Run with: cargo run --example turso-ivm-recursive-cte-join-repro
//!
//! Expected: Panic with "Invalid join commit state" or similar JoinOperator error

use std::path::Path;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Use a fresh database each time
    let db_path = "/tmp/turso-ivm-recursive-cte-test.db";
    if Path::new(db_path).exists() {
        std::fs::remove_file(db_path)?;
    }
    let _ = std::fs::remove_file(format!("{}-wal", db_path));
    let _ = std::fs::remove_file(format!("{}-shm", db_path));

    println!("Creating database at {}", db_path);
    let db = turso::Builder::new_local(db_path)
        .with_views(true) // Enable experimental materialized views
        .build()
        .await?;
    let conn = db.connect()?;

    // Setup blocks table (same as holon)
    println!("Creating blocks table...");
    conn.execute(
        r#"CREATE TABLE blocks (
            id TEXT PRIMARY KEY,
            parent_id TEXT NOT NULL,
            content TEXT DEFAULT '',
            content_type TEXT DEFAULT 'text',
            source_language TEXT,
            source_name TEXT,
            properties TEXT DEFAULT '{}',
            created_at TEXT DEFAULT (datetime('now')),
            updated_at TEXT DEFAULT (datetime('now'))
        )"#,
        (),
    )
    .await?;

    println!("Creating blocks_with_paths materialized view with recursive CTE...");
    // Create the blocks_with_paths materialized view with recursive CTE
    // This uses INNER JOIN in the recursive case - triggers the bug
    conn.execute(
        r#"CREATE MATERIALIZED VIEW blocks_with_paths AS
        WITH RECURSIVE paths AS (
            -- Base case: root blocks (parent is a document, not another block)
            SELECT
                id,
                parent_id,
                content,
                content_type,
                source_language,
                source_name,
                properties,
                created_at,
                updated_at,
                '/' || id as path
            FROM blocks
            WHERE parent_id LIKE 'holon-doc://%'
               OR parent_id = '__no_parent__'

            UNION ALL

            -- Recursive case: build path from parent (uses INNER JOIN)
            SELECT
                b.id,
                b.parent_id,
                b.content,
                b.content_type,
                b.source_language,
                b.source_name,
                b.properties,
                b.created_at,
                b.updated_at,
                p.path || '/' || b.id as path
            FROM blocks b
            INNER JOIN paths p ON b.parent_id = p.id
        )
        SELECT * FROM paths"#,
        (),
    )
    .await?;

    println!("Materialized view created, setting up CDC callback...");

    // Set up CDC callback - this triggers the bug
    conn.set_view_change_callback(|event| {
        println!(
            "CDC callback: {} changes to {}",
            event.changes.len(),
            event.relation_name
        );
    })?;

    println!("CDC callback registered, now inserting data...");

    // Insert a root block first
    println!("Inserting root block...");
    conn.execute(
        r#"INSERT INTO blocks (id, parent_id, content, content_type)
           VALUES ('root-block', 'holon-doc://test.org', 'Root content', 'text')"#,
        (),
    )
    .await?;

    println!("Root block inserted, now inserting child block...");

    // Insert a child block - this triggers the recursive CTE update
    // and is more likely to hit the "Invalid join commit state" error
    conn.execute(
        r#"INSERT INTO blocks (id, parent_id, content, content_type)
           VALUES ('child-block', 'root-block', 'Child content', 'text')"#,
        (),
    )
    .await?;

    println!("Child block inserted, now inserting grandchild block...");

    // Insert more nested blocks to stress the recursive CTE
    conn.execute(
        r#"INSERT INTO blocks (id, parent_id, content, content_type)
           VALUES ('grandchild-block', 'child-block', 'Grandchild content', 'text')"#,
        (),
    )
    .await?;

    println!("Grandchild block inserted");

    // Query the view to verify
    println!("\nQuerying blocks_with_paths...");
    let mut rows = conn
        .query("SELECT id, path FROM blocks_with_paths ORDER BY path", ())
        .await?;

    println!("blocks_with_paths contents:");
    while let Some(row) = rows.next().await? {
        let id: String = row.get(0)?;
        let path: String = row.get(1)?;
        println!("  {} -> {}", id, path);
    }

    println!("\nSUCCESS - no panic (bug may be fixed or not triggered in this scenario)");
    println!("\nNote: The 'Invalid join commit state' error typically occurs under");
    println!("more complex conditions with concurrent operations or nested matviews.");
    Ok(())
}
