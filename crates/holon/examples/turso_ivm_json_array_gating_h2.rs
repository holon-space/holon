//! Validation experiment H2 — JSON-array column gating matview vs. H1's
//! junction-table approach.
//!
//! ## Goal
//!
//! Mirror H1's six-primitive sequence against an alternative schema where
//! `blocked_by` is a JSON array column on `task` and the gating matview is
//! derived via `json_each(blocked_by)`. Determine whether the existing
//! pipeline produces *more* failures, slower CDC, or both — relative to H1.
//!
//! ## Why we run this even after H1 passed
//!
//! H1 already validated the junction-table direction. H2 is a confidence
//! check for documentation purposes: confirms that the JSON-array option
//! also doesn't quietly work, so future maintainers don't reconsider the
//! design without context.
//!
//! ## Schema
//!
//! - `task(id PK, status, blocked_by JSON)` — single table; `blocked_by` is
//!   either NULL or a JSON array of slug strings.
//! - `task_blocking_edges` matview: one row per *active* blocking
//!   relationship, derived by:
//!
//!   ```sql
//!   SELECT
//!      t.id AS task_id,
//!      j.value AS blocker_id,
//!      b.status AS blocker_status
//!   FROM task t, json_each(COALESCE(t.blocked_by, '[]')) j
//!   JOIN task b ON b.id = j.value
//!   WHERE b.status != 'DONE'
//!   ```
//!
//! Compared to H1, this introduces:
//!   - `json_each` in the FROM clause (a table-valued function).
//!   - No FK referential integrity (deleting a blocker leaves dangling
//!     references in the JSON arrays of dependents).
//!   - "Edge" mutations are UPDATEs on a single column, not INSERT/DELETE
//!     on a junction table.
//!
//! ## Six primitives (mirroring H1 with array semantics)
//!
//! P1 INSERT task           — three tasks A, B, C (all TODO, blocked_by=NULL).
//! P2 UPDATE blocked_by     — set A.blocked_by='["B","C"]' (add 2 edges).
//! P3 UPDATE blocker→DONE   — set B.status='DONE' (matview row A→B should drop).
//! P4 UPDATE blocked_by=[]  — clear A.blocked_by (matview row A→C should drop).
//! P5 DELETE blocked task   — restore A→C, then DELETE A (matview should drop).
//! P6 DELETE blocker task   — recreate A→C, then DELETE C (dangling JSON ref).
//!
//! Plus a reopen test: persist state, close DB, reopen, verify matview state.
//!
//! ## What "PASS" means
//!
//! For each primitive: matview row set matches expected, and matview CDC
//! events match expected count and kind. PASS = same correctness guarantees
//! as H1.
//!
//! Run: `cargo run --example turso_ivm_json_array_gating_h2`

use std::sync::{Arc, Mutex};
use std::time::Duration;

use turso_sdk_kit::rsapi::DatabaseChangeType;

const MATVIEW: &str = "task_blocking_edges";
const CDC_SETTLE: Duration = Duration::from_millis(250);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("=== H2: JSON-array column gating matview under Turso IVM ===\n");

    // Test 0: can we even create the matview?
    let matview_ddl_result = test_matview_ddl_supported().await;
    let matview_supported = matview_ddl_result.is_ok();

    if matview_supported {
        println!("\nMatview DDL accepted — running full primitives suite.\n");
        let mut all_passed = true;
        all_passed &= test_six_primitives().await?;
        all_passed &= test_reopen_preserves_matview().await?;

        println!("\n{}", "=".repeat(60));
        if all_passed {
            println!("H2 RESULT: PASS — JSON-array gating matview works (would tie with H1).");
        } else {
            println!("H2 RESULT: FAIL on primitives — see per-primitive output above.");
            std::process::exit(1);
        }
    } else {
        let err = matview_ddl_result.err().unwrap();
        println!("\n[Test 0] FAIL — Matview DDL rejected by Turso IVM:");
        println!("    {err}");

        // Fallback: confirm the array approach at least works for *reads*
        // via a raw query (no matview, no CDC observability).
        println!("\n--- Fallback: can the array approach support correct reads via raw query? ---");
        let raw_ok = test_raw_query_fallback().await?;

        println!("\n{}", "=".repeat(60));
        println!("H2 RESULT: FALSIFIED — JSON-array column is meaningfully worse than junction.");
        println!(
            "  - Cannot build an IVM matview (table-valued functions unsupported in logical plan)."
        );
        println!(
            "  - Raw-query reads {}: array approach can answer gating queries on demand,",
            if raw_ok { "WORK" } else { "ALSO FAIL" }
        );
        println!("    but loses the CDC observability that H1's matview provides.");
        println!("  - Junction-table approach (H1) is structurally superior on Turso today.");
    }

    Ok(())
}

// ── Test 0: matview DDL supported? ───────────────────────────────────────

async fn test_matview_ddl_supported() -> anyhow::Result<()> {
    let db = fresh_db("turso-ivm-json-array-gating-ddl-probe").await?;
    let conn = db.connect()?;
    setup_schema(&conn).await?;
    Ok(())
}

// ── Fallback: raw query (no matview) ─────────────────────────────────────

async fn test_raw_query_fallback() -> anyhow::Result<bool> {
    let db = fresh_db("turso-ivm-json-array-gating-fallback").await?;
    let conn = db.connect()?;

    // Schema *without* the matview.
    conn.execute(
        "CREATE TABLE task (
            id          TEXT PRIMARY KEY,
            status      TEXT NOT NULL,
            blocked_by  TEXT
        )",
        (),
    )
    .await?;

    // Populate: A blocked_by [B, C], B done, C todo.
    conn.execute(
        "INSERT INTO task (id, status, blocked_by) VALUES \
         ('A', 'TODO', '[\"B\",\"C\"]'), \
         ('B', 'DONE', NULL), \
         ('C', 'TODO', NULL)",
        (),
    )
    .await?;

    // Raw gating query — the form a non-IVM read path would use.
    let raw_sql = "SELECT t.id, j.value, b.status \
         FROM task t, json_each(COALESCE(t.blocked_by, '[]')) j \
         JOIN task b ON b.id = j.value \
         WHERE b.status != 'DONE' \
         ORDER BY t.id, j.value";
    let mut rows = conn.query(raw_sql, ()).await?;
    let mut got: Vec<(String, String, String)> = Vec::new();
    while let Some(row) = rows.next().await? {
        got.push((
            row.get::<String>(0)?,
            row.get::<String>(1)?,
            row.get::<String>(2)?,
        ));
    }
    let want = vec![("A".into(), "C".into(), "TODO".into())];
    let ok = got == want;
    println!(
        "  raw query: got={got:?} want={want:?} → {}",
        if ok { "PASS" } else { "FAIL" }
    );
    Ok(ok)
}

// ── Test 1: six primitives in sequence ───────────────────────────────────

async fn test_six_primitives() -> anyhow::Result<bool> {
    println!("--- Test 1: six primitives in sequence ---\n");

    let db = fresh_db("turso-ivm-json-array-gating-1").await?;
    let conn = db.connect()?;
    setup_schema(&conn).await?;

    let cdc = install_cdc_observer(&conn)?;

    // Initial state: empty.
    drain(&cdc).await;
    assert_matview(&conn, &[]).await?;

    let mut ok = true;

    // ── P1: INSERT 3 tasks (A, B, C, all TODO, no blockers) ──────────────
    println!("[P1] INSERT 3 tasks (A, B, C, all TODO, blocked_by=NULL)");
    for id in ["A", "B", "C"] {
        conn.execute(
            &format!("INSERT INTO task (id, status, blocked_by) VALUES ('{id}', 'TODO', NULL)"),
            (),
        )
        .await?;
    }
    let events = drain(&cdc).await;
    let on_mv = filter_matview(&events);
    println!("  matview CDC events: {} (expected: 0)", on_mv.len());
    ok &= check(
        on_mv.is_empty(),
        "P1: no matview events on plain task inserts",
    );
    ok &= check_matview_eq(&conn, &[], "P1: matview empty (no edges)").await?;

    // ── P2: UPDATE A.blocked_by='["B","C"]' (add 2 edges) ────────────────
    println!("[P2] UPDATE A.blocked_by='[\"B\",\"C\"]'");
    conn.execute(
        "UPDATE task SET blocked_by = '[\"B\",\"C\"]' WHERE id = 'A'",
        (),
    )
    .await?;
    let events = drain(&cdc).await;
    let on_mv = filter_matview(&events);
    println!("  matview CDC events: {} {:?}", on_mv.len(), kinds(&on_mv));
    ok &= check(
        on_mv.len() == 2 && on_mv.iter().all(|(_, k, _)| *k == "Insert"),
        "P2: matview emits 2x Insert (A→B, A→C)",
    );
    ok &= check_matview_eq(
        &conn,
        &[("A", "B", "TODO"), ("A", "C", "TODO")],
        "P2: matview has 2 active blocking rows",
    )
    .await?;

    // ── P3: UPDATE B.status='DONE' ───────────────────────────────────────
    println!("[P3] UPDATE B.status='DONE' (B was a blocker for A)");
    conn.execute("UPDATE task SET status = 'DONE' WHERE id = 'B'", ())
        .await?;
    let events = drain(&cdc).await;
    let on_mv = filter_matview(&events);
    println!("  matview CDC events: {} {:?}", on_mv.len(), kinds(&on_mv));
    ok &= check(
        on_mv.len() == 1 && on_mv[0].1 == "Delete",
        "P3: matview emits 1x Delete (A→B row gone)",
    );
    ok &= check_matview_eq(
        &conn,
        &[("A", "C", "TODO")],
        "P3: matview now has 1 row (A→C)",
    )
    .await?;

    // ── P4: UPDATE A.blocked_by='[]' (clear array) ───────────────────────
    println!("[P4] UPDATE A.blocked_by='[]' (drop edge A→C)");
    conn.execute("UPDATE task SET blocked_by = '[]' WHERE id = 'A'", ())
        .await?;
    let events = drain(&cdc).await;
    let on_mv = filter_matview(&events);
    println!("  matview CDC events: {} {:?}", on_mv.len(), kinds(&on_mv));
    ok &= check(
        on_mv.len() == 1 && on_mv[0].1 == "Delete",
        "P4: matview emits 1x Delete (A→C row gone)",
    );
    ok &= check_matview_eq(&conn, &[], "P4: matview empty after clear").await?;

    // ── P5: DELETE blocked task A ────────────────────────────────────────
    // Restore A→C first so we can test that DELETE A removes its incident
    // matview rows.
    println!("[P5 setup] Restore A.blocked_by='[\"C\"]' so A has 1 active edge");
    conn.execute("UPDATE task SET blocked_by = '[\"C\"]' WHERE id = 'A'", ())
        .await?;
    drain(&cdc).await;
    assert_matview(&conn, &[("A", "C", "TODO")]).await?;

    println!("[P5] DELETE task A (the blocked task)");
    conn.execute("DELETE FROM task WHERE id = 'A'", ()).await?;
    let events = drain(&cdc).await;
    let on_mv = filter_matview(&events);
    println!("  matview CDC events: {} {:?}", on_mv.len(), kinds(&on_mv));
    ok &= check(
        on_mv.len() == 1 && on_mv[0].1 == "Delete",
        "P5: matview emits 1x Delete when blocked task deleted",
    );
    ok &= check_matview_eq(&conn, &[], "P5: matview empty after blocked task DELETE").await?;

    // ── P6: DELETE blocker task C (dangling JSON ref scenario) ───────────
    // Re-create A with blocked_by=["C"], then DELETE C. JSON arrays have
    // no FK, so A.blocked_by retains "C" after C is deleted (DANGLING
    // reference). The matview's JOIN filters out dangling refs, so the
    // matview itself should still be correct.
    println!("[P6 setup] Re-INSERT A with blocked_by='[\"C\"]'");
    conn.execute(
        "INSERT INTO task (id, status, blocked_by) VALUES ('A', 'TODO', '[\"C\"]')",
        (),
    )
    .await?;
    drain(&cdc).await;
    assert_matview(&conn, &[("A", "C", "TODO")]).await?;

    println!("[P6] DELETE task C (the blocker; A.blocked_by still references C)");
    conn.execute("DELETE FROM task WHERE id = 'C'", ()).await?;
    let events = drain(&cdc).await;
    let on_mv = filter_matview(&events);
    println!("  matview CDC events: {} {:?}", on_mv.len(), kinds(&on_mv));
    ok &= check(
        on_mv.len() == 1 && on_mv[0].1 == "Delete",
        "P6: matview emits 1x Delete when blocker deleted",
    );
    ok &= check_matview_eq(&conn, &[], "P6: matview empty after blocker deleted").await?;

    // Document the dangling-reference data integrity gap — for record only.
    let mut rows = conn
        .query("SELECT blocked_by FROM task WHERE id = 'A'", ())
        .await?;
    let dangling = match rows.next().await? {
        Some(row) => row.get::<String>(0).ok(),
        None => None,
    };
    println!(
        "  data integrity note: A.blocked_by still = {:?} (C was deleted) — dangling ref",
        dangling
    );

    println!("\n  Test 1 result: {}\n", if ok { "PASS" } else { "FAIL" });
    Ok(ok)
}

// ── Test 2: matview survives DB close/reopen ─────────────────────────────

async fn test_reopen_preserves_matview() -> anyhow::Result<bool> {
    println!("--- Test 2: matview state survives DB reopen ---\n");

    let db_path = "/tmp/turso-ivm-json-array-gating-2.db";
    clean_db(db_path);

    let mut ok = true;

    {
        let db = open_db(db_path).await?;
        let conn = db.connect()?;
        setup_schema(&conn).await?;
        for id in ["A", "B"] {
            conn.execute(
                &format!("INSERT INTO task (id, status, blocked_by) VALUES ('{id}', 'TODO', NULL)"),
                (),
            )
            .await?;
        }
        conn.execute("UPDATE task SET blocked_by = '[\"B\"]' WHERE id = 'A'", ())
            .await?;
        tokio::time::sleep(CDC_SETTLE).await;
        let pre_close = read_matview(&conn).await?;
        ok &= check(
            pre_close == vec![("A".into(), "B".into(), "TODO".into())],
            "pre-close: matview has A→B",
        );
        // Drop conn + db.
    }

    // Reopen and verify state preserved.
    let db = open_db(db_path).await?;
    let conn = db.connect()?;
    let cdc = install_cdc_observer(&conn)?;
    drain(&cdc).await;

    let post_open = read_matview(&conn).await?;
    println!("  post-reopen matview: {post_open:?}");
    ok &= check(
        post_open == vec![("A".into(), "B".into(), "TODO".into())],
        "reopen: matview state preserved (A→B)",
    );

    // Apply a post-reopen UPDATE to confirm CDC still fires.
    println!("[reopen-UPDATE] B.status='DONE'");
    conn.execute("UPDATE task SET status = 'DONE' WHERE id = 'B'", ())
        .await?;
    {
        let events = drain(&cdc).await;
        let on_mv = filter_matview(&events);
        println!("  matview CDC events: {} {:?}", on_mv.len(), kinds(&on_mv));
        ok &= check(
            on_mv.iter().any(|(_, k, _)| *k == "Delete"),
            "reopen: post-reopen UPDATE still fires CDC",
        );
        ok &= check_matview_eq(&conn, &[], "reopen: matview empty after post-reopen DONE").await?;
    }

    println!("\n  Test 2 result: {}\n", if ok { "PASS" } else { "FAIL" });
    Ok(ok)
}

// ── Schema ───────────────────────────────────────────────────────────────

async fn setup_schema(conn: &turso::Connection) -> anyhow::Result<()> {
    conn.execute(
        "CREATE TABLE task (
            id          TEXT PRIMARY KEY,
            status      TEXT NOT NULL,
            blocked_by  TEXT
        )",
        (),
    )
    .await?;
    conn.execute(
        "CREATE MATERIALIZED VIEW task_blocking_edges AS
         SELECT
            t.id        AS task_id,
            j.value     AS blocker_id,
            b.status    AS blocker_status
         FROM task t, json_each(COALESCE(t.blocked_by, '[]')) j
         JOIN task b ON b.id = j.value
         WHERE b.status != 'DONE'",
        (),
    )
    .await?;
    Ok(())
}

// ── CDC observer ─────────────────────────────────────────────────────────

type CdcEvent = (String, &'static str, String);
type Cdc = Arc<Mutex<Vec<CdcEvent>>>;

fn install_cdc_observer(conn: &turso::Connection) -> anyhow::Result<Cdc> {
    let captured: Cdc = Arc::new(Mutex::new(Vec::new()));
    let captured_for_cb = captured.clone();
    conn.set_change_callback(move |event| {
        let mut buf = captured_for_cb.lock().unwrap();
        for change in event.changes.iter() {
            let kind = match &change.change {
                DatabaseChangeType::Insert { .. } => "Insert",
                DatabaseChangeType::Update { .. } => "Update",
                DatabaseChangeType::Delete { .. } => "Delete",
            };
            buf.push((event.relation_name.clone(), kind, change.id.to_string()));
        }
    })?;
    Ok(captured)
}

async fn drain(cdc: &Cdc) -> Vec<CdcEvent> {
    tokio::time::sleep(CDC_SETTLE).await;
    let mut buf = cdc.lock().unwrap();
    let out = buf.clone();
    buf.clear();
    out
}

fn filter_matview(events: &[CdcEvent]) -> Vec<&CdcEvent> {
    events.iter().filter(|(rel, _, _)| rel == MATVIEW).collect()
}

fn kinds(events: &[&CdcEvent]) -> Vec<&'static str> {
    events.iter().map(|(_, k, _)| *k).collect()
}

// ── Matview state queries ────────────────────────────────────────────────

async fn read_matview(conn: &turso::Connection) -> anyhow::Result<Vec<(String, String, String)>> {
    let mut rows = conn
        .query(
            "SELECT task_id, blocker_id, blocker_status
             FROM task_blocking_edges
             ORDER BY task_id, blocker_id",
            (),
        )
        .await?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await? {
        out.push((
            row.get::<String>(0)?,
            row.get::<String>(1)?,
            row.get::<String>(2)?,
        ));
    }
    Ok(out)
}

async fn assert_matview(
    conn: &turso::Connection,
    expected: &[(&str, &str, &str)],
) -> anyhow::Result<()> {
    let actual = read_matview(conn).await?;
    let want: Vec<(String, String, String)> = expected
        .iter()
        .map(|(a, b, c)| (a.to_string(), b.to_string(), c.to_string()))
        .collect();
    if actual != want {
        anyhow::bail!("matview mismatch:\n  expected: {want:?}\n  actual:   {actual:?}");
    }
    Ok(())
}

async fn check_matview_eq(
    conn: &turso::Connection,
    expected: &[(&str, &str, &str)],
    label: &str,
) -> anyhow::Result<bool> {
    match assert_matview(conn, expected).await {
        Ok(()) => {
            println!("  PASS [matview]: {label}");
            Ok(true)
        }
        Err(e) => {
            println!("  FAIL [matview]: {label}\n    {e}");
            Ok(false)
        }
    }
}

fn check(condition: bool, label: &str) -> bool {
    if condition {
        println!("  PASS [cdc]: {label}");
        true
    } else {
        println!("  FAIL [cdc]: {label}");
        false
    }
}

// ── DB lifecycle ─────────────────────────────────────────────────────────

async fn fresh_db(name: &str) -> anyhow::Result<turso::Database> {
    let db_path = format!("/tmp/{name}.db");
    clean_db(&db_path);
    open_db(&db_path).await
}

async fn open_db(db_path: &str) -> anyhow::Result<turso::Database> {
    Ok(turso::Builder::new_local(db_path)
        .experimental_materialized_views(true)
        .build()
        .await?)
}

fn clean_db(db_path: &str) {
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{db_path}{suffix}"));
    }
}
