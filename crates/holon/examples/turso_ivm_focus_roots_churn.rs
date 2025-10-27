//! Reproducer for Turso IVM over-eager CDC: matviews chained on
//! `current_focus → focus_roots` fire `set_change_callback` for upstream
//! transactions that don't actually change the matview's output.
//!
//! Production symptom: `gpui_ui_pbt` `[inv16] CDC not quiescent — spurious
//! events: [("region:main", 9..11)]` (`crates/holon-integration-tests/src/
//! test_environment.rs:1333`). The Holon test framework's
//! `setup_region_watch` subscribes to a `watch_view` shaped like
//!   `SELECT fr.root_id AS id, b.content, b.parent_id
//!    FROM focus_roots fr JOIN block b ON b.id = fr.root_id
//!    WHERE fr.region = 'main'`
//! and expects CDC to settle within 50 ms after each transition. The
//! production navigation flow (`crates/holon/src/navigation/provider.rs`
//! `focus()`) runs three transactions back-to-back — `DELETE FROM
//! navigation_history`, `INSERT INTO navigation_history`, `INSERT OR
//! REPLACE INTO navigation_cursor` — and only the third one actually
//! changes the matview's output. The other two should be no-ops as far
//! as `focus_roots` is concerned.
//!
//! Expected: each matview emits exactly one CDC batch per *output-changing*
//! navigation. Two navigations × 2 batches/nav (remove old + add new,
//! possibly merged) ⇒ ≤ 4 batches per matview.
//!
//! Actual: 5 batches per matview. The extra batches carry `items=0`
//! payloads that fire on the intermediate transactions (the `INSERT INTO
//! block` and `DELETE FROM navigation_history`) which leave the matview
//! output unchanged. They cascade through the entire chain in lockstep —
//! `focus_roots`, `region_main_view`, and the recursive-CTE
//! `main_panel_view` all fire at the same wall-clock microsecond — so a
//! consumer of any one of them sees the redundant traffic.
//!
//! Run with:
//!   cargo run --example turso_ivm_focus_roots_churn -p holon

use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
struct CdcEvent {
    relation_name: String,
    items: usize,
    elapsed_ms: u128,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let db_path = "/tmp/turso-ivm-focus-roots-churn.db";
    for ext in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{db_path}{ext}"));
    }

    println!("=== Turso IVM: focus_roots churn reproducer ===\n");

    let db = turso::Builder::new_local(db_path)
        .experimental_materialized_views(true)
        .build()
        .await?;
    let conn = db.connect()?;

    // -- Schema --------------------------------------------------------
    conn.execute(
        "CREATE TABLE block (
            id TEXT PRIMARY KEY,
            parent_id TEXT NOT NULL,
            content TEXT DEFAULT '',
            content_type TEXT DEFAULT 'text'
        )",
        (),
    )
    .await?;

    conn.execute(
        "CREATE TABLE navigation_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            region TEXT NOT NULL,
            block_id TEXT
        )",
        (),
    )
    .await?;

    conn.execute(
        "CREATE TABLE navigation_cursor (
            region TEXT PRIMARY KEY,
            history_id INTEGER REFERENCES navigation_history(id)
        )",
        (),
    )
    .await?;

    conn.execute(
        "INSERT INTO navigation_cursor (region, history_id) VALUES ('main', NULL)",
        (),
    )
    .await?;

    // The production click handler (`frontends/gpui/src/render/builders/
    // render_entity.rs:54`) hard-codes `region: "main"` when dispatching
    // `editor_focus`, so even a click in the LeftSidebar writes a row
    // here for region='main'. Phase 3 below tests this scenario.
    conn.execute(
        "CREATE TABLE editor_cursor (
            region TEXT PRIMARY KEY,
            block_id TEXT,
            cursor_offset INTEGER
        )",
        (),
    )
    .await?;

    // -- Test data: two docs with identical fan-out so the row count is
    //    constant across the navigation we're measuring. -----------------
    for i in 1..=4 {
        conn.execute(
            &format!(
                "INSERT INTO block (id, parent_id, content) \
                 VALUES ('a-{i}', 'doc:aaa', 'Doc A block {i}')"
            ),
            (),
        )
        .await?;
        conn.execute(
            &format!(
                "INSERT INTO block (id, parent_id, content) \
                 VALUES ('b-{i}', 'doc:bbb', 'Doc B block {i}')"
            ),
            (),
        )
        .await?;
    }
    conn.execute(
        "INSERT INTO block (id, parent_id, content) \
         VALUES ('doc:aaa', 'root', 'Doc A')",
        (),
    )
    .await?;
    conn.execute(
        "INSERT INTO block (id, parent_id, content) \
         VALUES ('doc:bbb', 'root', 'Doc B')",
        (),
    )
    .await?;

    // -- Matview chain mirroring assets/default/index.org --------------
    conn.execute(
        "CREATE MATERIALIZED VIEW current_focus AS
         SELECT nc.region, nh.block_id
         FROM navigation_cursor nc
         JOIN navigation_history nh ON nc.history_id = nh.id",
        (),
    )
    .await?;

    conn.execute(
        "CREATE MATERIALIZED VIEW focus_roots AS
         SELECT cf.region, cf.block_id, b.id AS root_id
         FROM current_focus cf
         JOIN block b ON b.parent_id = cf.block_id
         UNION ALL
         SELECT cf.region, cf.block_id, b.id AS root_id
         FROM current_focus cf
         JOIN block b ON b.id = cf.block_id",
        (),
    )
    .await?;

    // The same shape Holon's `setup_region_watch` subscribes to.
    conn.execute(
        "CREATE MATERIALIZED VIEW region_main_view AS
         SELECT fr.root_id AS id, b.content, b.parent_id
         FROM focus_roots fr
         JOIN block b ON b.id = fr.root_id
         WHERE fr.region = 'main'",
        (),
    )
    .await?;

    // Phase-2 fixture: a sibling watch for region='left_sidebar'. The PBT
    // calls `setup_region_watch` once per region, so production has
    // multiple matviews of this shape simultaneously. We need both
    // present to test that a navigation in *one* region doesn't leak
    // CDC into the other.
    conn.execute(
        "CREATE MATERIALIZED VIEW region_left_sidebar_view AS
         SELECT fr.root_id AS id, b.content, b.parent_id
         FROM focus_roots fr
         JOIN block b ON b.id = fr.root_id
         WHERE fr.region = 'left_sidebar'",
        (),
    )
    .await?;

    // Two regions, mirroring the production PBT setup.
    conn.execute(
        "INSERT INTO navigation_cursor (region, history_id) VALUES ('left_sidebar', NULL)",
        (),
    )
    .await?;

    // Production's `default-main-panel` block compiles to a recursive-CTE
    // matview rooted on `focus_roots`. Adding it puts IVM under the same
    // multi-downstream pressure as the live app — every focus change has
    // to ripple through both downstream matviews.
    conn.execute(
        "CREATE MATERIALIZED VIEW main_panel_view AS
         WITH RECURSIVE _vl AS (
             SELECT _v1.id AS node_id, _v1.id AS source_id, 0 AS depth,
                    CAST(_v1.id AS TEXT) AS visited
             FROM block AS _v1
             UNION ALL
             SELECT _fk.id, _vl.source_id, _vl.depth + 1,
                    _vl.visited || ',' || CAST(_fk.id AS TEXT)
             FROM _vl JOIN block _fk ON _fk.parent_id = _vl.node_id
             WHERE _vl.depth < 20
               AND ',' || _vl.visited || ',' NOT LIKE '%,' || CAST(_fk.id AS TEXT) || ',%'
         )
         SELECT _v3.id, _v3.content, _v3.parent_id
         FROM focus_roots AS _v0
         JOIN block AS _v1 ON _v1.id = _v0.root_id
         JOIN _vl ON _vl.source_id = _v1.id
         JOIN block AS _v3 ON _v3.id = _vl.node_id
         WHERE _v0.region = 'main'
           AND _vl.depth >= 0 AND _vl.depth <= 20",
        (),
    )
    .await?;

    // -- CDC instrumentation ------------------------------------------
    let events: Arc<Mutex<Vec<CdcEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let recorder = events.clone();
    let started = Instant::now();
    conn.set_change_callback(move |event| {
        let name = event.relation_name.clone();
        if matches!(
            name.as_str(),
            "focus_roots"
                | "region_main_view"
                | "region_left_sidebar_view"
                | "main_panel_view"
                | "current_focus"
        ) {
            recorder.lock().unwrap().push(CdcEvent {
                relation_name: name,
                items: event.changes.len(),
                elapsed_ms: started.elapsed().as_millis(),
            });
        }
    })?;

    // -- Pre-roll: navigate to doc:aaa using the production 3-statement
    //    sequence (`crates/holon/sql/navigation/*.sql`) so we exercise the
    //    same per-transaction CDC pattern. -------------------------------
    conn.execute(
        "DELETE FROM navigation_history WHERE region = 'main' AND id > 0",
        (),
    )
    .await?;
    conn.execute(
        "INSERT INTO navigation_history (region, block_id) VALUES ('main', 'doc:aaa')",
        (),
    )
    .await?;
    conn.execute(
        "INSERT OR REPLACE INTO navigation_cursor (region, history_id) VALUES ('main', 1)",
        (),
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Drop preroll events — we measure only the second navigation.
    let preroll: Vec<_> = events.lock().unwrap().drain(..).collect();
    println!(
        "[preroll] navigate→doc:aaa produced {} CDC events",
        preroll.len()
    );
    for ev in &preroll {
        println!(
            "  +{:>4}ms  {:<18}  items={}",
            ev.elapsed_ms, ev.relation_name, ev.items
        );
    }
    println!();

    // -- The navigations under test -----------------------------------
    //
    // Production PBT issues several navigation transitions back-to-back
    // (e.g. `ClickBlock(LeftSidebar, doc-A)` → `ClickBlock(LeftSidebar,
    // doc-B)` → `ClickBlock(LeftSidebar, journals)`) and afterwards calls
    // `assert_cdc_quiescent` with a 50 ms grace window. The reported
    // "spurious events" are CDC items stamped with `seq > target_seq`
    // captured at the start of the assertion — i.e. CDC that arrives
    // *after* the last navigation should have produced its single batch.
    //
    // Reproduce the same shape: two navigations, second of them is the
    // "transition we just applied" and the assertion window starts after
    // its 3 SQL statements have committed.
    println!("[test] back-to-back navigation: doc:bbb then journals");
    let start_navigate = Instant::now();

    // Navigation 1: doc:bbb
    conn.execute(
        "DELETE FROM navigation_history WHERE region = 'main' AND id > 1",
        (),
    )
    .await?;
    conn.execute(
        "INSERT INTO navigation_history (region, block_id) VALUES ('main', 'doc:bbb')",
        (),
    )
    .await?;
    conn.execute(
        "INSERT OR REPLACE INTO navigation_cursor (region, history_id) VALUES ('main', 2)",
        (),
    )
    .await?;

    // Navigation 2: journals (no children — to mirror the seed=7 trace
    // where ClickBlock(journals) produced inv16 panic)
    conn.execute(
        "INSERT INTO block (id, parent_id, content) VALUES ('journals', 'root', 'Journals')",
        (),
    )
    .await?;
    conn.execute(
        "DELETE FROM navigation_history WHERE region = 'main' AND id > 2",
        (),
    )
    .await?;
    conn.execute(
        "INSERT INTO navigation_history (region, block_id) VALUES ('main', 'journals')",
        (),
    )
    .await?;
    conn.execute(
        "INSERT OR REPLACE INTO navigation_cursor (region, history_id) VALUES ('main', 3)",
        (),
    )
    .await?;

    // Wait long enough that any post-settle re-emission has a chance to
    // fire — we want to catch churn that arrives 100s of ms after the
    // initial response, exactly the pattern seen in PBT logs.
    tokio::time::sleep(Duration::from_secs(4)).await;

    let recorded: Vec<CdcEvent> = events.lock().unwrap().clone();
    let nav_elapsed = start_navigate.elapsed();
    println!(
        "[test] navigate→doc:bbb produced {} CDC events over {:.2}s",
        recorded.len(),
        nav_elapsed.as_secs_f64()
    );
    for ev in &recorded {
        println!(
            "  +{:>4}ms  {:<18}  items={}",
            ev.elapsed_ms, ev.relation_name, ev.items
        );
    }

    // -- Assertions ---------------------------------------------------
    let by_relation = |relation: &str| {
        recorded
            .iter()
            .filter(|e| e.relation_name == relation)
            .cloned()
            .collect::<Vec<_>>()
    };
    let fr = by_relation("focus_roots");
    let rmv = by_relation("region_main_view");
    let mpv = by_relation("main_panel_view");

    let summarise = |label: &str, evs: &[CdcEvent]| {
        let nonempty = evs.iter().filter(|e| e.items > 0).count();
        let items: usize = evs.iter().map(|e| e.items).sum();
        println!(
            "{:<20}  {} batches ({} non-empty), {} total items",
            label,
            evs.len(),
            nonempty,
            items
        );
    };
    println!();
    summarise("focus_roots:", &fr);
    summarise("region_main_view:", &rmv);
    summarise("main_panel_view:", &mpv);

    // Two navigations should yield at most 4 batches per matview
    // (each navigation: remove old + add new — IVM is allowed to merge
    // each pair into 1, giving 2 total). Anything more is churn.
    let max_expected_batches = 4;
    let mut bug_present = false;

    let check = |label: &str, evs: &[CdcEvent]| -> bool {
        if evs.len() > max_expected_batches {
            println!(
                "!!! BUG: {label} emitted {} batches for two navigations \
                 (expected ≤ {max_expected_batches}) — IVM is firing CDC \
                 callbacks on transactions that don't change the matview's \
                 output",
                evs.len(),
            );
            true
        } else {
            false
        }
    };
    bug_present |= check("focus_roots", &fr);
    bug_present |= check("region_main_view", &rmv);
    bug_present |= check("main_panel_view", &mpv);

    // Cross-view alignment: if both views fire the same number of batches
    // microseconds apart, they share an upstream — i.e. focus_roots churn
    // cascades to every downstream consumer.
    if fr.len() == rmv.len() && fr.len() > 1 {
        let pairs: Vec<_> = fr.iter().zip(rmv.iter()).collect();
        let max_skew_ms = pairs
            .iter()
            .map(|(a, b)| (a.elapsed_ms as i128 - b.elapsed_ms as i128).abs())
            .max()
            .unwrap_or(0);
        println!(
            "\n[diagnostic] focus_roots and region_main_view fire in lockstep \
             ({} matched batches, max skew {} ms) — confirms shared upstream churn",
            pairs.len(),
            max_skew_ms,
        );
    }

    // Empty batches (items=0) are the smoking gun for IVM over-eagerness.
    // They mean: "an upstream table was written to, but the matview's
    // output didn't actually change — yet IVM still fired the CDC
    // callback". Each empty batch is a no-op that downstream code has to
    // process anyway; under a 50 ms `assert_cdc_quiescent` window in the
    // PBT, a chain of these can land *after* the watermark snapshot is
    // taken and trip the assertion as a false positive.
    let empty_batches = recorded.iter().filter(|e| e.items == 0).count();
    println!(
        "\n[diagnostic] {} of {} CDC batches contain zero items — IVM is \
         notifying for upstream writes that don't change matview output",
        empty_batches,
        recorded.len(),
    );

    if bug_present {
        println!("\nPhase 1: FAIL — empty/excessive CDC batches detected (see above).");
    } else {
        println!("\nPhase 1: PASS — each matview emitted ≤ {max_expected_batches} batches.");
    }

    // ====================================================================
    // Phase 2: cross-region IVM filter leakage
    //
    // gpui_ui_pbt observation (after the phase-1 fix): a ClickBlock in the
    // LeftSidebar — which only writes to navigation_history/cursor with
    // region='left_sidebar' — causes the test's region:main watch
    // (`WHERE fr.region = 'main'`) to emit a 4-item batch:
    //
    //   [apply] ClickBlock: region=LeftSidebar block=block:ref-doc-1
    //   [inv16] CDC not quiescent — spurious events: [("region:main", 4)]
    //
    // Whatever changed in `current_focus`/`focus_roots` for region=
    // 'left_sidebar' is propagating through region_main_view's WHERE
    // clause. The IVM filter should suppress every delta whose region
    // doesn't match. Items > 0 here means it doesn't.
    // ====================================================================
    println!("\n=== Phase 2: cross-region IVM filter leakage ===");

    // Pre-roll: navigate left_sidebar to doc:aaa, mirroring phase-1.
    conn.execute(
        "INSERT INTO navigation_history (region, block_id) VALUES ('left_sidebar', 'doc:aaa')",
        (),
    )
    .await?;
    let preroll_history_id = max_history_id_for(&conn, "left_sidebar").await?;
    conn.execute(
        &format!(
            "INSERT OR REPLACE INTO navigation_cursor (region, history_id) \
             VALUES ('left_sidebar', {preroll_history_id})"
        ),
        (),
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Drain everything from phase-1 + left_sidebar pre-roll.
    events.lock().unwrap().clear();
    let phase2_start = Instant::now();

    // Issue the same 3-statement focus sequence the production
    // navigation provider runs (`crates/holon/src/navigation/provider.rs`
    // `focus()`), but only for region='left_sidebar'. Crucially: every
    // statement targets region='left_sidebar' — no write touches a
    // 'main' row, so region_main_view's WHERE filter should suppress
    // every delta.
    println!("[test] navigate region='left_sidebar' to doc:bbb (no main-region writes)");
    conn.execute(
        &format!(
            "DELETE FROM navigation_history \
             WHERE region = 'left_sidebar' AND id > {preroll_history_id}"
        ),
        (),
    )
    .await?;
    conn.execute(
        "INSERT INTO navigation_history (region, block_id) VALUES ('left_sidebar', 'doc:bbb')",
        (),
    )
    .await?;
    let new_history_id = max_history_id_for(&conn, "left_sidebar").await?;
    conn.execute(
        &format!(
            "INSERT OR REPLACE INTO navigation_cursor (region, history_id) \
             VALUES ('left_sidebar', {new_history_id})"
        ),
        (),
    )
    .await?;

    tokio::time::sleep(Duration::from_secs(4)).await;

    let phase2_events: Vec<CdcEvent> = events.lock().unwrap().clone();
    println!(
        "[test] left_sidebar navigation produced {} CDC events over {:.2}s",
        phase2_events.len(),
        phase2_start.elapsed().as_secs_f64()
    );
    for ev in &phase2_events {
        println!(
            "  +{:>4}ms  {:<26}  items={}",
            ev.elapsed_ms, ev.relation_name, ev.items
        );
    }

    let main_view_items: usize = phase2_events
        .iter()
        .filter(|e| e.relation_name == "region_main_view")
        .map(|e| e.items)
        .sum();
    let left_view_items: usize = phase2_events
        .iter()
        .filter(|e| e.relation_name == "region_left_sidebar_view")
        .map(|e| e.items)
        .sum();

    println!();
    println!("region_main_view leaked items:        {main_view_items} (expected 0)");
    println!("region_left_sidebar_view real items:  {left_view_items} (expected > 0)");

    let mut phase2_bug = false;
    if main_view_items > 0 {
        println!(
            "\n!!! BUG: region_main_view received {main_view_items} item(s) for a \
             left_sidebar-only navigation — IVM is not applying the WHERE \
             fr.region = 'main' filter to the propagated delta"
        );
        phase2_bug = true;
    }
    if left_view_items == 0 {
        println!(
            "\n!!! WARN: region_left_sidebar_view saw zero items — the test \
             navigation may not have actually fired (sanity check failed)"
        );
        phase2_bug = true;
    }

    if !phase2_bug {
        println!("\nPhase 2: PASS — region_main_view saw zero cross-region leakage.");
    } else {
        println!("\nPhase 2: FAIL — see above.");
    }

    // ====================================================================
    // Phase 3: editor_cursor write triggers spurious CDC on region_main_view
    //
    // gpui_ui_pbt symptom: ClickBlock(LeftSidebar, ...) fires inv16 on
    // `region:main` with 4 items. The click handler at
    // `frontends/gpui/src/render/builders/render_entity.rs:54` dispatches
    // `editor_focus(region="main", block_id=...)` (region is hard-coded
    // to "main" — see TODO in that file). `editor_focus` performs a
    //
    //   INSERT OR REPLACE INTO editor_cursor
    //     (region, block_id, cursor_offset) VALUES (...)
    //
    // (`crates/holon/src/navigation/provider.rs:258`). `editor_cursor`
    // does NOT participate in `focus_roots` / `region_main_view`'s
    // schema, so a write here MUST NOT fire CDC on either.
    // ====================================================================
    println!("\n=== Phase 3: editor_cursor write should not touch region_main_view ===");

    events.lock().unwrap().clear();
    let phase3_start = Instant::now();

    println!(
        "[test] INSERT OR REPLACE INTO editor_cursor (region='main', block_id='b-1', cursor_offset=0)"
    );
    conn.execute(
        "INSERT OR REPLACE INTO editor_cursor (region, block_id, cursor_offset) \
         VALUES ('main', 'b-1', 0)",
        (),
    )
    .await?;
    tokio::time::sleep(Duration::from_secs(2)).await;

    // A subsequent click in production sets a *different* cursor_offset,
    // so reproduce that too — INSERT OR REPLACE rewrites the same row.
    println!(
        "[test] INSERT OR REPLACE INTO editor_cursor (region='main', block_id='b-2', cursor_offset=0)"
    );
    conn.execute(
        "INSERT OR REPLACE INTO editor_cursor (region, block_id, cursor_offset) \
         VALUES ('main', 'b-2', 0)",
        (),
    )
    .await?;
    tokio::time::sleep(Duration::from_secs(2)).await;

    let phase3_events: Vec<CdcEvent> = events.lock().unwrap().clone();
    println!(
        "[test] editor_cursor writes produced {} CDC events on watched matviews over {:.2}s",
        phase3_events.len(),
        phase3_start.elapsed().as_secs_f64()
    );
    for ev in &phase3_events {
        println!(
            "  +{:>4}ms  {:<26}  items={}",
            ev.elapsed_ms, ev.relation_name, ev.items
        );
    }

    let main_view_items_p3: usize = phase3_events
        .iter()
        .filter(|e| e.relation_name == "region_main_view")
        .map(|e| e.items)
        .sum();
    let left_view_items_p3: usize = phase3_events
        .iter()
        .filter(|e| e.relation_name == "region_left_sidebar_view")
        .map(|e| e.items)
        .sum();
    let main_panel_items_p3: usize = phase3_events
        .iter()
        .filter(|e| e.relation_name == "main_panel_view")
        .map(|e| e.items)
        .sum();

    println!();
    println!("region_main_view items:           {main_view_items_p3} (expected 0)");
    println!("region_left_sidebar_view items:   {left_view_items_p3} (expected 0)");
    println!("main_panel_view items:            {main_panel_items_p3} (expected 0)");

    let mut phase3_bug = false;
    if main_view_items_p3 > 0 || left_view_items_p3 > 0 || main_panel_items_p3 > 0 {
        println!(
            "\n!!! BUG: editor_cursor writes triggered {} item(s) on matviews \
             that don't depend on editor_cursor — IVM is firing CDC \
             callbacks on relations whose schema doesn't include the \
             written table",
            main_view_items_p3 + left_view_items_p3 + main_panel_items_p3
        );
        phase3_bug = true;
    }

    if bug_present || phase2_bug || phase3_bug {
        anyhow::bail!("CDC churn detected — see output above");
    }

    println!(
        "\nPASS: phase 1 emitted ≤ {max_expected_batches} batches per matview, \
         phase 2 saw zero cross-region leakage, phase 3 saw zero unrelated-table churn."
    );
    Ok(())
}

/// Read `max(id) FROM navigation_history WHERE region = $region`.
async fn max_history_id_for(conn: &turso::Connection, region: &str) -> anyhow::Result<i64> {
    let sql = format!("SELECT max(id) FROM navigation_history WHERE region = '{region}'");
    let mut rows = conn.query(&sql, ()).await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| anyhow::anyhow!("no rows for max(id) in region={region}"))?;
    let id: i64 = row.get(0)?;
    Ok(id)
}
