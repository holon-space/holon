//! Validation experiment H1 — junction-table gating matview stability under
//! Turso IVM through the six primitive operations that drive a `blocked_by`
//! dependency graph.
//!
//! ## Background
//!
//! Holon is choosing how to store `blocked_by` task dependencies. One option
//! projects `:BLOCKED-BY:` into a `task_blocks(from_id, to_id)` junction table
//! and computes "is task X gated?" via a materialized view. This option is
//! attractive because it (a) supports indexed reverse queries, (b) maps
//! directly onto Petri-net input arcs later, and (c) sidesteps `json_each`
//! over arrays. But none of those benefits matter if the matview itself
//! becomes stale or drops CDC under common operations.
//!
//! This experiment validates the option by exercising the six primitives that
//! a real authoring workflow produces.
//!
//! ## Schema
//!
//! - `task(id PK, status)` — base table.
//! - `task_blocks(from_id, to_id, PK(from_id, to_id), FK CASCADE on both)` — junction.
//!   Cascade deletes mean a deleted task automatically removes its incident edges.
//! - `task_blocking_edges` matview: one row per *active* blocking relationship,
//!   i.e. the join of `task_blocks` with the blocker's task row, filtered by
//!   `blocker_status != 'DONE'`. Pure relational JOIN + WHERE — no GROUP BY,
//!   no recursion.
//!
//! "Is task X unblocked?" is then `NOT EXISTS (SELECT 1 FROM task_blocking_edges
//! WHERE task_id = X)`. Cheap query against an indexed matview.
//!
//! ## Primitives exercised in sequence
//!
//! P1 INSERT task          — base-table inserts, matview untouched (no edges).
//! P2 INSERT edge          — matview gains rows for active blockers.
//! P3 UPDATE blocker→DONE  — matview rows for that blocker should disappear.
//! P4 DELETE edge          — matview row for that edge should disappear.
//! P5 DELETE blocked task  — task gone; if it was a blocker for others, FK
//!                            cascade should remove those edges and matview rows.
//! P6 DELETE blocker task  — same FK-cascade path; verifies cascade also
//!                            propagates correctly when the *target* of an
//!                            edge is deleted.
//!
//! Plus a reopen test: do P1/P2, close DB, reopen, check matview state is
//! correct, then run P3 and verify CDC continues to fire.
//!
//! ## What "PASS" means
//!
//! For each primitive we assert:
//!   - matview row set matches expectation, and
//!   - CDC events on `task_blocking_edges` match expectation in count and kind.
//!
//! For reopen we additionally assert that on second open the matview state
//! equals the state at close, and post-reopen primitives still fire CDC.
//!
//! Run: `cargo run --example turso_ivm_junction_gating_repro`

use std::sync::{Arc, Mutex};
use std::time::Duration;

use turso_sdk_kit::rsapi::DatabaseChangeType;

const MATVIEW: &str = "task_blocking_edges";
const CDC_SETTLE: Duration = Duration::from_millis(250);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("=== H1: Junction-table gating matview under Turso IVM ===\n");

    let mut all_passed = true;
    all_passed &= test_six_primitives().await?;
    all_passed &= test_reopen_preserves_matview().await?;

    println!("\n{}", "=".repeat(60));
    if all_passed {
        println!("H1 RESULT: PASS — junction-table gating is stable under IVM.");
        Ok(())
    } else {
        println!("H1 RESULT: FAIL — see per-primitive output above.");
        std::process::exit(1);
    }
}

// ── Test 1: six primitives in sequence ───────────────────────────────────

async fn test_six_primitives() -> anyhow::Result<bool> {
    println!("--- Test 1: six primitives in sequence ---\n");

    let db = fresh_db("turso-ivm-junction-gating-1").await?;
    let conn = db.connect()?;
    setup_schema(&conn).await?;

    let cdc = install_cdc_observer(&conn)?;

    // Initial state: empty.
    drain(&cdc).await;
    assert_matview(&conn, &[]).await?;

    let mut ok = true;

    // ── P1: INSERT task (3 tasks, no edges) ──────────────────────────────
    println!("[P1] INSERT 3 tasks (A, B, C, all TODO)");
    for id in ["A", "B", "C"] {
        conn.execute(
            &format!("INSERT INTO task (id, status) VALUES ('{id}', 'TODO')"),
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

    // ── P2: INSERT edge (A→B and A→C) ────────────────────────────────────
    println!("[P2] INSERT edges A→B, A→C");
    conn.execute(
        "INSERT INTO task_blocks (from_id, to_id) VALUES ('A', 'B')",
        (),
    )
    .await?;
    conn.execute(
        "INSERT INTO task_blocks (from_id, to_id) VALUES ('A', 'C')",
        (),
    )
    .await?;
    let events = drain(&cdc).await;
    let on_mv = filter_matview(&events);
    println!("  matview CDC events: {} {:?}", on_mv.len(), kinds(&on_mv));
    ok &= check(
        on_mv.len() == 2 && on_mv.iter().all(|(_, k, _)| *k == "Insert"),
        "P2: 2 Inserts on matview for 2 new blocking edges",
    );
    ok &= check_matview_eq(
        &conn,
        &[("A", "B", "TODO"), ("A", "C", "TODO")],
        "P2: matview reflects 2 active blocking edges",
    )
    .await?;

    // ── P3: UPDATE blocker → DONE ────────────────────────────────────────
    println!("[P3] UPDATE task B status=DONE");
    conn.execute("UPDATE task SET status = 'DONE' WHERE id = 'B'", ())
        .await?;
    let events = drain(&cdc).await;
    let on_mv = filter_matview(&events);
    println!("  matview CDC events: {} {:?}", on_mv.len(), kinds(&on_mv));
    // Expected: the (A, B, TODO) row should disappear because the WHERE
    // filter (blocker_status != 'DONE') no longer matches. Either Delete
    // or Update→Delete coalesced.
    let has_delete = on_mv.iter().any(|(_, k, _)| *k == "Delete");
    ok &= check(has_delete, "P3: matview emits Delete when blocker → DONE");
    ok &= check_matview_eq(
        &conn,
        &[("A", "C", "TODO")],
        "P3: only (A, C, TODO) remains; (A, B, *) gone",
    )
    .await?;

    // ── P4: DELETE edge ──────────────────────────────────────────────────
    println!("[P4] DELETE edge A→C");
    conn.execute(
        "DELETE FROM task_blocks WHERE from_id = 'A' AND to_id = 'C'",
        (),
    )
    .await?;
    let events = drain(&cdc).await;
    let on_mv = filter_matview(&events);
    println!("  matview CDC events: {} {:?}", on_mv.len(), kinds(&on_mv));
    ok &= check(
        on_mv.iter().any(|(_, k, _)| *k == "Delete"),
        "P4: matview emits Delete on edge removal",
    );
    ok &= check_matview_eq(&conn, &[], "P4: matview empty after edge delete").await?;

    // ── P5: DELETE blocked task ──────────────────────────────────────────
    // Set up: insert task D (TODO), edge D→A. D is blocked by A.
    // Then delete D (the *blocked* task). FK cascade removes the edge,
    // matview row for (D, A, *) should disappear.
    println!("[P5] setup: INSERT task D, edge D→A; then DELETE task D");
    conn.execute("INSERT INTO task (id, status) VALUES ('D', 'TODO')", ())
        .await?;
    conn.execute(
        "INSERT INTO task_blocks (from_id, to_id) VALUES ('D', 'A')",
        (),
    )
    .await?;
    drain(&cdc).await; // ignore setup events
    ok &= check_matview_eq(
        &conn,
        &[("D", "A", "TODO")],
        "P5 setup: (D, A, TODO) present",
    )
    .await?;

    conn.execute("DELETE FROM task WHERE id = 'D'", ()).await?;
    let events = drain(&cdc).await;
    let on_mv = filter_matview(&events);
    println!("  matview CDC events: {} {:?}", on_mv.len(), kinds(&on_mv));
    ok &= check(
        on_mv.iter().any(|(_, k, _)| *k == "Delete"),
        "P5: matview emits Delete when blocked task is deleted (via FK cascade)",
    );
    ok &= check_matview_eq(&conn, &[], "P5: matview empty after blocked task delete").await?;

    // ── P6: DELETE blocker task ──────────────────────────────────────────
    // Set up: insert task E, F (both TODO), edge E→F. E is blocked by F.
    // Delete F (the *blocker*). FK cascade removes the edge,
    // matview row for (E, F, *) should disappear.
    println!("[P6] setup: INSERT tasks E, F, edge E→F; then DELETE task F");
    conn.execute("INSERT INTO task (id, status) VALUES ('E', 'TODO')", ())
        .await?;
    conn.execute("INSERT INTO task (id, status) VALUES ('F', 'TODO')", ())
        .await?;
    conn.execute(
        "INSERT INTO task_blocks (from_id, to_id) VALUES ('E', 'F')",
        (),
    )
    .await?;
    drain(&cdc).await;
    ok &= check_matview_eq(
        &conn,
        &[("E", "F", "TODO")],
        "P6 setup: (E, F, TODO) present",
    )
    .await?;

    conn.execute("DELETE FROM task WHERE id = 'F'", ()).await?;
    let events = drain(&cdc).await;
    let on_mv = filter_matview(&events);
    println!("  matview CDC events: {} {:?}", on_mv.len(), kinds(&on_mv));
    ok &= check(
        on_mv.iter().any(|(_, k, _)| *k == "Delete"),
        "P6: matview emits Delete when blocker task is deleted (via FK cascade)",
    );
    ok &= check_matview_eq(&conn, &[], "P6: matview empty after blocker task delete").await?;

    println!("\n  Test 1 result: {}\n", if ok { "PASS" } else { "FAIL" });
    Ok(ok)
}

// ── Test 2: matview state survives DB close/reopen ───────────────────────

async fn test_reopen_preserves_matview() -> anyhow::Result<bool> {
    println!("--- Test 2: matview state survives DB close/reopen ---\n");

    let db_path = "/tmp/turso-ivm-junction-gating-2.db";
    clean_db(db_path);

    // Phase 1: open, set up, populate, close.
    {
        let db = open_db(db_path).await?;
        let conn = db.connect()?;
        setup_schema(&conn).await?;

        for id in ["A", "B"] {
            conn.execute(
                &format!("INSERT INTO task (id, status) VALUES ('{id}', 'TODO')"),
                (),
            )
            .await?;
        }
        conn.execute(
            "INSERT INTO task_blocks (from_id, to_id) VALUES ('A', 'B')",
            (),
        )
        .await?;
        // Settle.
        tokio::time::sleep(CDC_SETTLE).await;
        let mv = read_matview(&conn).await?;
        println!("  Phase 1 (pre-close): matview = {mv:?}");
    }

    // Phase 2: reopen, verify.
    let mut ok = true;
    {
        let db = open_db(db_path).await?;
        let conn = db.connect()?;
        let cdc = install_cdc_observer(&conn)?;

        let mv = read_matview(&conn).await?;
        println!("  Phase 2 (post-reopen): matview = {mv:?}");
        ok &= check(
            mv == vec![("A".to_string(), "B".to_string(), "TODO".to_string())],
            "reopen: matview state preserved",
        );

        // Phase 3: do another primitive — make B DONE.
        // Should fire a Delete on the matview, dropping (A,B,TODO).
        println!("  Phase 3: UPDATE task B status=DONE post-reopen");
        conn.execute("UPDATE task SET status = 'DONE' WHERE id = 'B'", ())
            .await?;
        let events = drain(&cdc).await;
        let on_mv = filter_matview(&events);
        println!(
            "    matview CDC events: {} {:?}",
            on_mv.len(),
            kinds(&on_mv)
        );
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
    conn.execute("PRAGMA foreign_keys = ON", ()).await?;
    conn.execute(
        "CREATE TABLE task (
            id TEXT PRIMARY KEY,
            status TEXT NOT NULL
        )",
        (),
    )
    .await?;
    conn.execute(
        "CREATE TABLE task_blocks (
            from_id TEXT NOT NULL,
            to_id   TEXT NOT NULL,
            PRIMARY KEY (from_id, to_id),
            FOREIGN KEY (from_id) REFERENCES task(id) ON DELETE CASCADE,
            FOREIGN KEY (to_id)   REFERENCES task(id) ON DELETE CASCADE
        )",
        (),
    )
    .await?;
    conn.execute(
        "CREATE MATERIALIZED VIEW task_blocking_edges AS
         SELECT
            bb.from_id      AS task_id,
            bb.to_id        AS blocker_id,
            b.status        AS blocker_status
         FROM task_blocks bb
         JOIN task b ON b.id = bb.to_id
         WHERE b.status != 'DONE'",
        (),
    )
    .await?;
    Ok(())
}

// ── CDC observer ─────────────────────────────────────────────────────────

type CdcEvent = (String, &'static str, String); // (relation, kind, id-or-rowid)
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
