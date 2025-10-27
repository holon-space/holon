//! Reproducer: Turso IVM matview WHERE `col != ''` does not filter NULL rows
//!
//! **Bug**: A materialized view defined as `SELECT * FROM t WHERE name != ''`
//! should exclude rows where `name IS NULL` (per SQL three-valued logic,
//! `NULL != ''` evaluates to NULL which is treated as false in WHERE).
//! The equivalent plain `SELECT` correctly returns only non-NULL, non-empty
//! rows. But the matview's incrementally-maintained state includes the NULL
//! rows — they leak through the WHERE filter.
//!
//! **Observed in production**: A sidebar query `SELECT * FROM block WHERE name != ''`
//! returns 4 rows via plain SQL but 50 rows from its materialized view (46 extra
//! rows all have `name IS NULL`). The matview was created at DB startup, rows were
//! inserted incrementally, and the IVM delta computation treated `NULL != ''` as
//! true instead of NULL/false.
//!
//! **Workaround**: Use `WHERE name IS NOT NULL AND name != ''` — the explicit
//! IS NOT NULL is handled correctly by the IVM engine.
//!
//! Run with: cargo run --example turso_ivm_null_where_neq_repro

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("=== Turso IVM: NULL rows leak through WHERE col != '' ===\n");

    let mut all_passed = true;

    all_passed &= test_fresh_inserts().await?;
    all_passed &= test_update_null_to_value().await?;
    all_passed &= test_update_value_to_null().await?;
    all_passed &= test_stale_matview_after_reopen().await?;
    all_passed &= test_production_like_schema().await?;

    println!("\n{}", "=".repeat(60));
    if all_passed {
        println!("ALL TESTS PASSED");
    } else {
        println!("SOME TESTS FAILED — see details above");
        std::process::exit(1);
    }

    Ok(())
}

/// Original test: fresh inserts into a new matview.
async fn test_fresh_inserts() -> anyhow::Result<bool> {
    println!("--- Test 1: Fresh inserts ---");
    let db = fresh_db("turso-ivm-repro-1").await?;
    let conn = db.connect()?;

    conn.execute("CREATE TABLE items (id TEXT PRIMARY KEY, name TEXT)", ())
        .await?;
    conn.execute(
        "CREATE MATERIALIZED VIEW named_items AS SELECT * FROM items WHERE name != ''",
        (),
    )
    .await?;

    // 4 named, 10 NULL, 3 empty-string
    for (id, name) in [("a", "Alice"), ("b", "Bob"), ("c", "Carol"), ("d", "Dave")] {
        conn.execute(
            &format!("INSERT INTO items (id, name) VALUES ('{id}', '{name}')"),
            (),
        )
        .await?;
    }
    for i in 0..10 {
        conn.execute(&format!("INSERT INTO items (id) VALUES ('null_{i}')"), ())
            .await?;
    }
    for i in 0..3 {
        conn.execute(
            &format!("INSERT INTO items (id, name) VALUES ('empty_{i}', '')"),
            (),
        )
        .await?;
    }

    let plain = count(&conn, "SELECT COUNT(*) FROM items WHERE name != ''").await?;
    let matview = count(&conn, "SELECT COUNT(*) FROM named_items").await?;
    let null_leaked = count(&conn, "SELECT COUNT(*) FROM named_items WHERE name IS NULL").await?;

    println!("  Plain SQL: {plain}, Matview: {matview}, NULL leaked: {null_leaked}");
    check(
        plain == 4 && matview == 4 && null_leaked == 0,
        "fresh inserts",
    )
}

/// Test: UPDATE a row's name from NULL to a real value — should appear in matview.
async fn test_update_null_to_value() -> anyhow::Result<bool> {
    println!("--- Test 2: UPDATE name NULL → value ---");
    let db = fresh_db("turso-ivm-repro-2").await?;
    let conn = db.connect()?;

    conn.execute("CREATE TABLE items (id TEXT PRIMARY KEY, name TEXT)", ())
        .await?;
    conn.execute(
        "CREATE MATERIALIZED VIEW named_items AS SELECT * FROM items WHERE name != ''",
        (),
    )
    .await?;

    conn.execute("INSERT INTO items (id) VALUES ('x')", ())
        .await?;

    let before = count(&conn, "SELECT COUNT(*) FROM named_items").await?;
    println!("  Before UPDATE: matview has {before} rows (expected: 0)");

    conn.execute("UPDATE items SET name = 'Xavier' WHERE id = 'x'", ())
        .await?;

    let after = count(&conn, "SELECT COUNT(*) FROM named_items").await?;
    let null_leaked = count(&conn, "SELECT COUNT(*) FROM named_items WHERE name IS NULL").await?;
    println!("  After UPDATE:  matview has {after} rows (expected: 1), NULL leaked: {null_leaked}");

    check(
        before == 0 && after == 1 && null_leaked == 0,
        "NULL→value UPDATE",
    )
}

/// Test: UPDATE a row's name from a value to NULL — should disappear from matview.
async fn test_update_value_to_null() -> anyhow::Result<bool> {
    println!("--- Test 3: UPDATE name value → NULL ---");
    let db = fresh_db("turso-ivm-repro-3").await?;
    let conn = db.connect()?;

    conn.execute("CREATE TABLE items (id TEXT PRIMARY KEY, name TEXT)", ())
        .await?;
    conn.execute(
        "CREATE MATERIALIZED VIEW named_items AS SELECT * FROM items WHERE name != ''",
        (),
    )
    .await?;

    conn.execute("INSERT INTO items (id, name) VALUES ('x', 'Xavier')", ())
        .await?;

    let before = count(&conn, "SELECT COUNT(*) FROM named_items").await?;
    println!("  Before UPDATE: matview has {before} rows (expected: 1)");

    conn.execute("UPDATE items SET name = NULL WHERE id = 'x'", ())
        .await?;

    let after = count(&conn, "SELECT COUNT(*) FROM named_items").await?;
    println!("  After UPDATE:  matview has {after} rows (expected: 0)");

    check(before == 1 && after == 0, "value→NULL UPDATE")
}

/// Test: Close and reopen DB — do stale NULL rows persist in the matview?
/// This simulates the production scenario where the matview was populated
/// before the IVM fix and the app is restarted with the fixed binary.
async fn test_stale_matview_after_reopen() -> anyhow::Result<bool> {
    println!("--- Test 4: Matview correctness after DB close/reopen ---");
    let db_path = "/tmp/turso-ivm-repro-4.db";
    clean_db(db_path);

    // Phase 1: create matview and insert data
    {
        let db = turso::Builder::new_local(db_path)
            .experimental_materialized_views(true)
            .build()
            .await?;
        let conn = db.connect()?;

        conn.execute("CREATE TABLE items (id TEXT PRIMARY KEY, name TEXT)", ())
            .await?;
        conn.execute(
            "CREATE MATERIALIZED VIEW named_items AS SELECT * FROM items WHERE name != ''",
            (),
        )
        .await?;

        for (id, name) in [("a", "Alice"), ("b", "Bob")] {
            conn.execute(
                &format!("INSERT INTO items (id, name) VALUES ('{id}', '{name}')"),
                (),
            )
            .await?;
        }
        for i in 0..5 {
            conn.execute(&format!("INSERT INTO items (id) VALUES ('null_{i}')"), ())
                .await?;
        }

        let matview = count(&conn, "SELECT COUNT(*) FROM named_items").await?;
        println!("  Phase 1 (before close): matview has {matview} rows (expected: 2)");
    }

    // Phase 2: reopen and check — do stale NULLs appear?
    {
        let db = turso::Builder::new_local(db_path)
            .experimental_materialized_views(true)
            .build()
            .await?;
        let conn = db.connect()?;

        let matview = count(&conn, "SELECT COUNT(*) FROM named_items").await?;
        let null_leaked =
            count(&conn, "SELECT COUNT(*) FROM named_items WHERE name IS NULL").await?;
        let plain = count(&conn, "SELECT COUNT(*) FROM items WHERE name != ''").await?;
        println!(
            "  Phase 2 (after reopen): matview has {matview} rows (expected: 2), \
             plain SQL: {plain}, NULL leaked: {null_leaked}"
        );

        // Phase 3: insert more data after reopen — do NULLs leak now?
        for i in 5..10 {
            conn.execute(&format!("INSERT INTO items (id) VALUES ('null_{i}')"), ())
                .await?;
        }
        conn.execute("INSERT INTO items (id, name) VALUES ('c', 'Carol')", ())
            .await?;

        let matview_after = count(&conn, "SELECT COUNT(*) FROM named_items").await?;
        let null_leaked_after =
            count(&conn, "SELECT COUNT(*) FROM named_items WHERE name IS NULL").await?;
        let plain_after = count(&conn, "SELECT COUNT(*) FROM items WHERE name != ''").await?;
        println!(
            "  Phase 3 (post-reopen inserts): matview has {matview_after} rows (expected: 3), \
             plain SQL: {plain_after}, NULL leaked: {null_leaked_after}"
        );

        check(
            matview == 2 && null_leaked == 0 && matview_after == 3 && null_leaked_after == 0,
            "stale matview after reopen",
        )
    }
}

/// Test: Production-like schema matching the `block` table with many columns.
/// The original bug was observed on this schema, so we reproduce it closely.
async fn test_production_like_schema() -> anyhow::Result<bool> {
    println!("--- Test 5: Production-like block table schema ---");
    let db = fresh_db("turso-ivm-repro-5").await?;
    let conn = db.connect()?;

    conn.execute(
        "CREATE TABLE block (
            id TEXT PRIMARY KEY,
            parent_id TEXT,
            depth INTEGER NOT NULL DEFAULT 0,
            sort_key TEXT NOT NULL DEFAULT 'a0',
            content TEXT NOT NULL DEFAULT '',
            content_type TEXT NOT NULL DEFAULT 'text',
            source_language TEXT,
            source_name TEXT,
            name TEXT,
            properties TEXT,
            collapsed INTEGER NOT NULL DEFAULT 0,
            completed INTEGER NOT NULL DEFAULT 0,
            block_type TEXT NOT NULL DEFAULT 'text',
            created_at INTEGER NOT NULL DEFAULT 0,
            updated_at INTEGER NOT NULL DEFAULT 0,
            _change_origin TEXT
        )",
        (),
    )
    .await?;

    conn.execute(
        "CREATE MATERIALIZED VIEW named_blocks AS SELECT * FROM block WHERE name != ''",
        (),
    )
    .await?;

    // Insert named blocks (like documents with names)
    for (id, name) in [
        ("doc1", "Projects"),
        ("doc2", "Holon"),
        ("doc3", "ClaudeCode"),
    ] {
        conn.execute(
            &format!(
                "INSERT INTO block (id, parent_id, content, name, created_at, updated_at) \
                 VALUES ('{id}', 'sentinel:no_parent', '', '{name}', 1000, 1000)"
            ),
            (),
        )
        .await?;
    }

    // Insert blocks WITHOUT name (the majority — like regular content blocks)
    for i in 0..50 {
        conn.execute(
            &format!(
                "INSERT INTO block (id, parent_id, content, content_type, created_at, updated_at) \
                 VALUES ('block_{i}', 'doc1', 'Some content {i}', 'text', 1000, 1000)"
            ),
            (),
        )
        .await?;
    }

    let plain = count(&conn, "SELECT COUNT(*) FROM block WHERE name != ''").await?;
    let matview = count(&conn, "SELECT COUNT(*) FROM named_blocks").await?;
    let null_leaked = count(
        &conn,
        "SELECT COUNT(*) FROM named_blocks WHERE name IS NULL",
    )
    .await?;
    let total = count(&conn, "SELECT COUNT(*) FROM block").await?;

    println!(
        "  Total blocks: {total}, Plain SQL: {plain}, Matview: {matview}, NULL leaked: {null_leaked}"
    );

    check(
        plain == 3 && matview == 3 && null_leaked == 0,
        "production-like schema",
    )
}

// ── Helpers ────────────────────────────────────────────────────────────

async fn fresh_db(name: &str) -> anyhow::Result<turso::Database> {
    let db_path = format!("/tmp/{name}.db");
    clean_db(&db_path);
    Ok(turso::Builder::new_local(&db_path)
        .experimental_materialized_views(true)
        .build()
        .await?)
}

fn clean_db(db_path: &str) {
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{db_path}{suffix}"));
    }
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
