//! Minimized reproducer probe (Inc 0 premise-check, Martin's request):
//!
//! Claim under test: the main-panel focus matview (`watch_view_*`, a
//! `CREATE MATERIALIZED VIEW` over a query reading the `block` MATERIALIZED
//! VIEW) emits a phantom RETRACT-ALL + ASSERT-ALL (two same-batch CDC deltas)
//! on a SINGLE-ROW base change — rather than an O(delta) incremental +1.
//!
//! This ladder builds the thinnest chained-matview shape and escalates it one
//! construct at a time toward the real focus query, subscribing CDC on the
//! OUTER matview and applying exactly ONE single-row base insert, capturing the
//! CDC batches verbatim. If any rung emits retract-all+assert-all on a
//! single-row change, that rung's added construct is the trigger. If none do,
//! the prod trace's 11-item all-delete was NOT plain chained-matview
//! maintenance and needs a different explanation.
//!
//! VERDICT (rungs 0–2): the claim is REFUTED. Turso maintains matview-over-
//! matview — including recursive-CTE-over-matview (the exact focus-query shape)
//! — incrementally, O(delta): ONE `Created`, ZERO retractions, on a single-row
//! insert. The prod 11-item all-delete is NOT chained-matview maintenance.
//!
//! REAL MECHANISM (rung 3): the retract-all is an INPUT-DRIVEN, semantically-
//! CORRECT IVM retraction. The main-panel focus query joins
//! `focus_roots fr JOIN navigation_cursor nc ON nc.history_id = fr.history_id`.
//! `focus_roots` is `navigation_history WHERE closed_at IS NULL` (open rows
//! only); forward navigation CLOSES the prior open row (focus_replace). A
//! `NavigateBack` (`UPDATE navigation_cursor SET history_id = <prior>`) moves
//! the cursor onto a CLOSED history row — absent from `focus_roots` — so the
//! join legitimately yields 0 rows and the matview retracts its entire result
//! set (panel blanks). `current_focus` (which joins `navigation_history`
//! WITHOUT the `closed_at` filter) still resolves, so focus "moved" but the
//! panel is empty. Rung 3 reproduces this deterministically. This is a
//! holon-side navigation-model / focus-query-join consistency bug, NOT Turso
//! IVM, NOT chained-matview, NOT a reseed.
//!
//! Harness pattern mirrors `automations_journal_cdc.rs`.

use std::collections::HashMap;
use std::time::Duration;

use holon_api::BatchWithMetadata;
use holon_turso::matview_manager::reconcile_named_view;
use holon_turso::turso::DbHandle;
use holon_turso::turso::RowChange;
use holon_turso::turso::TursoBackend;
use tokio::sync::broadcast::Receiver;

/// One captured CDC batch for a relation, reduced to per-change kind tags.
#[derive(Debug)]
struct CapturedBatch {
    relation: String,
    kinds: Vec<String>,
}

async fn new_db() -> (DbHandle, Receiver<BatchWithMetadata<RowChange>>) {
    let (backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(backend); // keep the actor alive for the test
    let rx = handle.subscribe_row_changes();
    (handle, rx)
}

fn kind(change: &RowChange) -> String {
    use holon_api::streaming::Change;
    match &change.change {
        Change::Created { data, .. } => {
            let id = data.get("id").and_then(|v| v.as_string()).unwrap_or("?");
            format!("C:{id}")
        }
        Change::Updated { id, .. } => format!("U:{id}"),
        Change::Deleted { id, .. } => format!("D:{id}"),
        Change::FieldsChanged { entity_id, .. } => format!("F:{entity_id}"),
    }
}

/// Drain all CDC batches seen so far, one entry per batch (preserving batch
/// boundaries — the whole point is to see whether retract-all and assert-all
/// arrive as ONE batch or TWO), filtered to `relation`.
async fn drain(
    rx: &mut Receiver<BatchWithMetadata<RowChange>>,
    relation: &str,
) -> Vec<CapturedBatch> {
    tokio::time::sleep(Duration::from_millis(250)).await;
    let mut out = Vec::new();
    while let Ok(batch) = rx.try_recv() {
        if batch.metadata.relation_name != relation {
            continue;
        }
        let kinds: Vec<String> = batch.inner.items.iter().map(kind).collect();
        if !kinds.is_empty() {
            out.push(CapturedBatch {
                relation: batch.metadata.relation_name.clone(),
                kinds,
            });
        }
    }
    out
}

/// Seed the base table with three "existing" rows (structural-page + two
/// children), mirroring the prod focus subtree before the create.
async fn seed_base(handle: &DbHandle) {
    handle
        .execute(
            "CREATE TABLE blk_raw (id TEXT PRIMARY KEY, parent_id TEXT, content TEXT, \
             sort_key TEXT)",
            vec![],
        )
        .await
        .expect("create blk_raw");
    for (id, parent, content, sk) in [
        ("page", "root", "structural-page", "a"),
        ("c1", "page", "c1", "b"),
        ("c2", "page", "c2", "c"),
    ] {
        handle
            .execute(
                "INSERT INTO blk_raw (id, parent_id, content, sort_key) VALUES (?, ?, ?, ?)",
                vec![
                    turso::Value::Text(id.into()),
                    turso::Value::Text(parent.into()),
                    turso::Value::Text(content.into()),
                    turso::Value::Text(sk.into()),
                ],
            )
            .await
            .expect("seed row");
    }
}

/// Create the inner `blk` matview (mimics `block` over `block_raw`).
async fn create_inner_matview(handle: &DbHandle) {
    reconcile_named_view(
        handle,
        "blk",
        "SELECT id, parent_id, content, sort_key FROM blk_raw",
    )
    .await
    .expect("inner matview blk");
}

/// Query the outer view's `id` set as a sorted Vec, for matview-vs-recompute.
async fn view_ids(handle: &DbHandle, view: &str) -> Vec<String> {
    let rows = handle
        .query(&format!("SELECT node_id FROM {view}"), HashMap::new())
        .await
        .expect("query view ids");
    let mut ids: Vec<String> = rows
        .iter()
        .map(|r| {
            r.get("node_id")
                .and_then(|v| v.as_string())
                .expect("node_id text")
                .to_string()
        })
        .collect();
    ids.sort();
    ids
}

/// Recompute the SAME defining SELECT against the base matview `blk`, bypassing
/// IVM maintenance — this is the differential oracle the keystone invariant
/// uses.
async fn recompute_ids(handle: &DbHandle, defining_sql: &str) -> Vec<String> {
    let rows = handle
        .query(defining_sql, HashMap::new())
        .await
        .expect("recompute defining select");
    let mut ids: Vec<String> = rows
        .iter()
        .map(|r| {
            r.get("node_id")
                .and_then(|v| v.as_string())
                .expect("node_id text")
                .to_string()
        })
        .collect();
    ids.sort();
    ids
}

/// The exact IVM-compiled `FocusRootDescendants` varlen recursive CTE shape
/// (mirrors `sql_parser.rs` test_extract_recursive_cte_in_subquery / the GQL
/// `MATCH (root)<-[:CHILD_OF*0..N]-(d)` form the main-panel watch registers).
/// Anchored on `blk` node with id = `page`, walking `_fk.parent_id = node_id`.
const FOCUS_DESCENDANTS_SQL: &str = "\
    WITH RECURSIVE _vl1 AS ( \
        SELECT _v0.id AS node_id, 0 AS depth, CAST(_v0.id AS TEXT) AS visited \
        FROM blk AS _v0 WHERE _v0.id = 'page' \
        UNION ALL \
        SELECT _fk.id, _vl1.depth + 1, _vl1.visited || ',' || CAST(_fk.id AS TEXT) \
        FROM _vl1 JOIN blk _fk ON _fk.parent_id = _vl1.node_id \
        WHERE _vl1.depth < 20 \
          AND ',' || _vl1.visited || ',' NOT LIKE '%,' || CAST(_fk.id AS TEXT) || ',%' \
    ) \
    SELECT node_id FROM _vl1";

/// Insert ONE new child under `page` — the single-row change under test.
async fn insert_one_child(handle: &DbHandle) {
    handle
        .execute(
            "INSERT INTO blk_raw (id, parent_id, content, sort_key) VALUES (?, ?, ?, ?)",
            vec![
                turso::Value::Text("new1".into()),
                turso::Value::Text("page".into()),
                turso::Value::Text("a".into()),
                turso::Value::Text("d".into()),
            ],
        )
        .await
        .expect("insert one child");
}

fn report(rung: &str, base_insert: &[CapturedBatch]) {
    eprintln!("\n===== RUNG {rung} — CDC batches on OUTER view for ONE single-row insert =====");
    if base_insert.is_empty() {
        eprintln!("  (no CDC batches captured for the outer view)");
    }
    for (i, b) in base_insert.iter().enumerate() {
        eprintln!(
            "  batch[{i}] relation={} items={} kinds={:?}",
            b.relation,
            b.kinds.len(),
            b.kinds
        );
    }
    let total_deletes: usize = base_insert
        .iter()
        .flat_map(|b| &b.kinds)
        .filter(|k| k.starts_with("D:"))
        .count();
    let total_creates: usize = base_insert
        .iter()
        .flat_map(|b| &b.kinds)
        .filter(|k| k.starts_with("C:"))
        .count();
    eprintln!(
        "  SUMMARY {rung}: batches={} deletes={total_deletes} creates={total_creates} \
         => {}",
        base_insert.len(),
        if total_deletes > 0 {
            "RETRACT-ALL PRESENT (phantom deletes on single-row insert)"
        } else {
            "incremental (no retractions) — claim NOT reproduced at this rung"
        }
    );
}

/// RUNG 0 — trivial passthrough matview over the inner matview.
#[tokio::test]
async fn rung0_trivial_matview_over_matview() {
    let (handle, mut rx) = new_db().await;
    seed_base(&handle).await;
    create_inner_matview(&handle).await;
    let outer =
        reconcile_named_view(&handle, "outer0", "SELECT id, parent_id, content FROM blk").await;
    outer.expect("outer0 matview");
    let _ = drain(&mut rx, "outer0").await; // discard seed/backfill
    insert_one_child(&handle).await;
    let batches = drain(&mut rx, "outer0").await;
    report("0/trivial passthrough over matview", &batches);
}

/// RUNG 1 — self-join (parent↔child) over the inner matview.
#[tokio::test]
async fn rung1_self_join_over_matview() {
    let (handle, mut rx) = new_db().await;
    seed_base(&handle).await;
    create_inner_matview(&handle).await;
    reconcile_named_view(
        &handle,
        "outer1",
        "SELECT c.id AS id, c.parent_id AS parent_id, c.content AS content \
         FROM blk c JOIN blk p ON c.parent_id = p.id",
    )
    .await
    .expect("outer1 matview");
    let _ = drain(&mut rx, "outer1").await;
    insert_one_child(&handle).await;
    let batches = drain(&mut rx, "outer1").await;
    report("1/self-join over matview", &batches);
}

/// RUNG 2 — recursive CTE (focus_descendants shape) over the inner matview,
/// anchored on a `focus_roots`-like base table.
#[tokio::test]
async fn rung2_recursive_cte_over_matview() {
    let (handle, mut rx) = new_db().await;
    seed_base(&handle).await;
    handle
        .execute(
            "CREATE TABLE focus_roots (region TEXT, root_id TEXT)",
            vec![],
        )
        .await
        .expect("focus_roots");
    handle
        .execute(
            "INSERT INTO focus_roots (region, root_id) VALUES ('main', 'page')",
            vec![],
        )
        .await
        .expect("seed focus_roots");
    create_inner_matview(&handle).await;
    // The real main-panel SQL (assets/default/index.org
    // default-main-panel::src::0), trimmed to the reproducer's tables (no
    // navigation_cursor / block_tags gate).
    let sql = "WITH RECURSIVE focus_descendants AS ( \
        SELECT b.id AS node_id, b.id AS source_id, 0 AS depth, CAST(b.id AS TEXT) AS visited \
        FROM blk b JOIN focus_roots fr ON b.id = fr.root_id \
        UNION ALL \
        SELECT child.id, focus_descendants.source_id, focus_descendants.depth + 1, \
               focus_descendants.visited || ',' || CAST(child.id AS TEXT) \
        FROM focus_descendants \
        JOIN blk child ON child.parent_id = focus_descendants.node_id \
        WHERE focus_descendants.depth < 20 \
          AND ',' || focus_descendants.visited || ',' NOT LIKE \
              '%,' || CAST(child.id AS TEXT) || ',%' \
    ) \
    SELECT d.id AS id, d.parent_id AS parent_id, d.content AS content \
    FROM focus_roots fr \
    JOIN blk root ON root.id = fr.root_id \
    JOIN focus_descendants ON focus_descendants.source_id = root.id \
    JOIN blk d ON d.id = focus_descendants.node_id \
    WHERE fr.region = 'main'";
    reconcile_named_view(&handle, "outer2", sql)
        .await
        .expect("outer2 recursive matview");
    let _ = drain(&mut rx, "outer2").await;
    insert_one_child(&handle).await;
    let batches = drain(&mut rx, "outer2").await;
    report("2/recursive CTE + focus_roots join over matview", &batches);
}

/// RUNG 3 — the REAL mechanism, deterministic. Model the prod navigation
/// tables + the `focus_roots`/`navigation_cursor` join exactly as
/// `matview_focus_roots.sql` / `navigation.sql` / the main-panel query define
/// them, then perform a `NavigateBack` (`UPDATE navigation_cursor SET
/// history_id = <closed prior row>`) and assert the focus matview RETRACTS its
/// entire result set — because the cursor now points at a CLOSED
/// `navigation_history` row that `focus_roots` (open-rows-only) excludes, so
/// the `nc.history_id = fr.history_id` join yields 0 rows.
#[tokio::test]
async fn rung3_navigate_back_onto_closed_row_retracts_focus_panel() {
    let (handle, mut rx) = new_db().await;
    seed_base(&handle).await;
    create_inner_matview(&handle).await;

    // Navigation tables (verbatim shapes from
    // crates/holon-turso/sql/schema/navigation.sql).
    handle
        .execute(
            "CREATE TABLE navigation_history (id INTEGER PRIMARY KEY AUTOINCREMENT, \
             region TEXT NOT NULL, block_id TEXT, closed_at TEXT NULL)",
            vec![],
        )
        .await
        .expect("navigation_history");
    handle
        .execute(
            "CREATE TABLE navigation_cursor (region TEXT PRIMARY KEY, history_id INTEGER)",
            vec![],
        )
        .await
        .expect("navigation_cursor");
    // H1 = prior focus (CLOSED by a later focus_replace — the NavigateBack target).
    handle
        .execute(
            "INSERT INTO navigation_history (id, region, block_id, closed_at) \
             VALUES (1, 'main', 'page', '2026-07-24T00:00:00')",
            vec![],
        )
        .await
        .expect("H1 closed");
    // H2 = current focus (OPEN). Same block; focus didn't logically leave `page`.
    handle
        .execute(
            "INSERT INTO navigation_history (id, region, block_id, closed_at) \
             VALUES (2, 'main', 'page', NULL)",
            vec![],
        )
        .await
        .expect("H2 open");
    handle
        .execute(
            "INSERT INTO navigation_cursor (region, history_id) VALUES ('main', 2)",
            vec![],
        )
        .await
        .expect("cursor at H2");

    // focus_roots matview — verbatim from matview_focus_roots.sql (open rows only).
    reconcile_named_view(
        &handle,
        "focus_roots",
        "SELECT region, block_id AS root_id, id AS history_id \
         FROM navigation_history WHERE closed_at IS NULL AND block_id IS NOT NULL",
    )
    .await
    .expect("focus_roots matview");

    // The main-panel focus query (recursive-CTE form), joining focus_roots AND
    // navigation_cursor on history_id — the exact coupling from
    // assets/default/index.org default-main-panel::src::0.
    let sql = "WITH RECURSIVE focus_descendants AS ( \
        SELECT b.id AS node_id, b.id AS source_id, 0 AS depth, CAST(b.id AS TEXT) AS visited \
        FROM blk b JOIN focus_roots fr ON b.id = fr.root_id \
        UNION ALL \
        SELECT child.id, focus_descendants.source_id, focus_descendants.depth + 1, \
               focus_descendants.visited || ',' || CAST(child.id AS TEXT) \
        FROM focus_descendants \
        JOIN blk child ON child.parent_id = focus_descendants.node_id \
        WHERE focus_descendants.depth < 20 \
          AND ',' || focus_descendants.visited || ',' NOT LIKE \
              '%,' || CAST(child.id AS TEXT) || ',%' \
    ) \
    SELECT d.id AS id, d.parent_id AS parent_id, d.content AS content \
    FROM focus_roots fr \
    JOIN blk root ON root.id = fr.root_id \
    JOIN focus_descendants ON focus_descendants.source_id = root.id \
    JOIN blk d ON d.id = focus_descendants.node_id \
    JOIN navigation_cursor nc ON nc.region = fr.region AND nc.history_id = fr.history_id \
    WHERE fr.region = 'main'";
    reconcile_named_view(&handle, "main_panel", sql)
        .await
        .expect("main_panel matview");

    // Baseline: the panel resolves the page subtree (page + c1 + c2 = 3 rows).
    let rows_before = handle
        .query("SELECT id FROM main_panel", HashMap::new())
        .await
        .expect("query main_panel");
    assert_eq!(
        rows_before.len(),
        3,
        "baseline: main panel must show the page subtree (page,c1,c2); got {rows_before:?}"
    );
    let _ = drain(&mut rx, "main_panel").await; // discard baseline CDC

    // NavigateBack: move the cursor onto the CLOSED prior row H1 (verbatim
    // update_cursor.sql shape). No focus_roots change accompanies it.
    handle
        .execute(
            "UPDATE navigation_cursor SET history_id = 1 WHERE region = 'main'",
            vec![],
        )
        .await
        .expect("navigate back");

    let batches = drain(&mut rx, "main_panel").await;
    report(
        "3/NavigateBack onto CLOSED history row (join break)",
        &batches,
    );

    let rows_after = handle
        .query("SELECT id FROM main_panel", HashMap::new())
        .await
        .expect("query main_panel after back");
    let deletes: usize = batches
        .iter()
        .flat_map(|b| &b.kinds)
        .filter(|k| k.starts_with("D:"))
        .count();
    assert_eq!(
        rows_after.len(),
        0,
        "REAL MECHANISM: NavigateBack onto a CLOSED history row must blank the focus panel \
         (cursor.history_id has no matching open focus_roots row); got {rows_after:?}"
    );
    assert!(
        deletes >= 3,
        "the blanking must arrive as an IVM retract-all CDC delta (>=3 deletes); saw {batches:?}"
    );
}

/// Seed a deeper anchored chain for the re-parent rungs:
///   page (anchor) → A (depth 1) → B (depth 2)   plus an OUTSIDE node not under
/// page. Returns after the inner `blk` matview is built and settled.
async fn seed_chain(handle: &DbHandle) {
    handle
        .execute(
            "CREATE TABLE blk_raw (id TEXT PRIMARY KEY, parent_id TEXT, content TEXT, \
             sort_key TEXT)",
            vec![],
        )
        .await
        .expect("create blk_raw");
    for (id, parent, content, sk) in [
        ("page", "root", "anchor-page", "a"),
        ("A", "page", "A", "b"),
        ("B", "A", "B", "c"),
        ("outside", "root", "outside", "z"),
    ] {
        handle
            .execute(
                "INSERT INTO blk_raw (id, parent_id, content, sort_key) VALUES (?, ?, ?, ?)",
                vec![
                    turso::Value::Text(id.into()),
                    turso::Value::Text(parent.into()),
                    turso::Value::Text(content.into()),
                    turso::Value::Text(sk.into()),
                ],
            )
            .await
            .expect("seed chain row");
    }
    create_inner_matview(handle).await;
}

/// RUNG 4 — re-parent a LEAF (`B`) OUT of the anchored subtree via
/// `UPDATE blk_raw SET parent_id`. Recompute drops B; assert the IVM recursive
/// matview also drops it (single-edge membership retraction).
#[tokio::test]
async fn rung4_reparent_leaf_out_retracts_from_recursive_matview() {
    let (handle, mut rx) = new_db().await;
    seed_chain(&handle).await;
    reconcile_named_view(&handle, "fdesc4", FOCUS_DESCENDANTS_SQL)
        .await
        .expect("fdesc4 recursive matview");

    let before = view_ids(&handle, "fdesc4").await;
    assert_eq!(
        before,
        vec!["A".to_string(), "B".to_string(), "page".to_string()],
        "baseline: anchored subtree is page→A→B"
    );
    let _ = drain(&mut rx, "fdesc4").await;

    // Re-parent B onto `outside` (still in DB, but disconnected from `page`).
    handle
        .execute(
            "UPDATE blk_raw SET parent_id = 'outside' WHERE id = 'B'",
            vec![],
        )
        .await
        .expect("reparent leaf B");

    let matview_ids = view_ids(&handle, "fdesc4").await;
    let recompute = recompute_ids(&handle, FOCUS_DESCENDANTS_SQL).await;
    eprintln!(
        "\n===== RUNG 4 leaf re-parent: matview={matview_ids:?} recompute={recompute:?} ====="
    );
    assert_eq!(
        recompute,
        vec!["A".to_string(), "page".to_string()],
        "recompute (defining SELECT) must drop the re-parented leaf B"
    );
    assert_eq!(
        matview_ids, recompute,
        "MATVIEW-VS-RECOMPUTE DRIFT: IVM recursive matview must retract leaf B on re-parent-out"
    );
}

/// RUNG 5 — re-parent an INTERMEDIATE node (`A`) OUT of the anchored subtree.
/// `B` is now a TRANSITIVE descendant that must cascade-retract even though B's
/// OWN row never changed. This is the exact evidence shape: fe-target (depth 1)
/// retained under fe-parent after a bulk re-parent moved the intermediate out.
#[tokio::test]
async fn rung5_reparent_intermediate_out_cascade_retracts_transitive_descendant() {
    let (handle, mut rx) = new_db().await;
    seed_chain(&handle).await;
    reconcile_named_view(&handle, "fdesc5", FOCUS_DESCENDANTS_SQL)
        .await
        .expect("fdesc5 recursive matview");

    let before = view_ids(&handle, "fdesc5").await;
    assert_eq!(
        before,
        vec!["A".to_string(), "B".to_string(), "page".to_string()],
        "baseline: anchored subtree is page→A→B"
    );
    let _ = drain(&mut rx, "fdesc5").await;

    // Re-parent the INTERMEDIATE A onto `outside`. B's row is untouched; its
    // ancestry (A) leaves the anchored subtree, so B must cascade out.
    handle
        .execute(
            "UPDATE blk_raw SET parent_id = 'outside' WHERE id = 'A'",
            vec![],
        )
        .await
        .expect("reparent intermediate A");

    let matview_ids = view_ids(&handle, "fdesc5").await;
    let recompute = recompute_ids(&handle, FOCUS_DESCENDANTS_SQL).await;
    eprintln!(
        "\n===== RUNG 5 intermediate re-parent: matview={matview_ids:?} recompute={recompute:?} ====="
    );
    assert_eq!(
        recompute,
        vec!["page".to_string()],
        "recompute (defining SELECT) must drop BOTH A and its transitive descendant B"
    );
    assert_eq!(
        matview_ids, recompute,
        "MATVIEW-VS-RECOMPUTE DRIFT: IVM recursive matview retains transitive descendant B \
         after its intermediate ancestor A was re-parented out — the keystone drift shape"
    );
}

/// The outer watch shape actually registered by the main panel: the recursive
/// `_vl1` membership is JOINed back to `blk` to project the block columns
/// (`node_id, depth, content, parent_id`). This is the layer the keystone
/// `watch_view_*` really materializes, so the re-parent rungs below assert
/// against THIS (join-back) form, not the bare `SELECT node_id FROM _vl1`.
const FOCUS_DESCENDANTS_JOINED_SQL: &str = "\
    WITH RECURSIVE _vl1 AS ( \
        SELECT _v0.id AS node_id, 0 AS depth, CAST(_v0.id AS TEXT) AS visited \
        FROM blk AS _v0 WHERE _v0.id = 'page' \
        UNION ALL \
        SELECT _fk.id, _vl1.depth + 1, _vl1.visited || ',' || CAST(_fk.id AS TEXT) \
        FROM _vl1 JOIN blk _fk ON _fk.parent_id = _vl1.node_id \
        WHERE _vl1.depth < 20 \
          AND ',' || _vl1.visited || ',' NOT LIKE '%,' || CAST(_fk.id AS TEXT) || ',%' \
    ) \
    SELECT d.id AS node_id, _vl1.depth AS depth, d.parent_id AS parent_id, d.content AS content \
    FROM _vl1 JOIN blk d ON d.id = _vl1.node_id";

/// RUNG 6 — re-parent a node WITHIN the anchored subtree so its DEPTH changes
/// (`B` moves from under `A` (depth 2) to directly under `page` (depth 1)).
/// The recursive `depth`/`visited` are part of the projected row VALUE, so this
/// requires the IVM to retract the OLD-depth derivation and assert the new one.
/// A retract-miss here leaves a stale old-depth row → matview has MORE rows,
/// exactly the keystone "matview 7 / recompute 6" shape.
#[tokio::test]
async fn rung6_reparent_changes_depth_within_subtree_no_stale_old_depth_row() {
    let (handle, _rx) = new_db().await;
    seed_chain(&handle).await; // page→A→B, plus `outside`
    reconcile_named_view(&handle, "fdesc6", FOCUS_DESCENDANTS_JOINED_SQL)
        .await
        .expect("fdesc6 joined recursive matview");

    // Move B from under A (depth 2) to directly under page (depth 1).
    handle
        .execute(
            "UPDATE blk_raw SET parent_id = 'page' WHERE id = 'B'",
            vec![],
        )
        .await
        .expect("reparent B up to page");

    let matview = canon_rows(
        handle
            .query("SELECT node_id, depth FROM fdesc6", HashMap::new())
            .await
            .expect("query fdesc6 matview"),
    );
    let recompute = canon_rows(
        handle
            .query(FOCUS_DESCENDANTS_JOINED_SQL, HashMap::new())
            .await
            .expect("recompute fdesc6"),
    );
    eprintln!(
        "\n===== RUNG 6 depth-change re-parent: matview={matview:?} recompute={recompute:?} ====="
    );
    assert_eq!(
        matview, recompute,
        "MATVIEW-VS-RECOMPUTE DRIFT: depth-changing re-parent left a stale old-depth row in \
         the IVM recursive matview (matview has MORE rows than recompute — the keystone shape)"
    );
}

/// RUNG 7 — BULK/BATCH re-parent: multiple descendants re-parented in ONE
/// transaction, mirroring the evidence ("stale rows include re-parented BULK
/// blocks", "bulk-0-4 re-parented under bulk-0-2"). Seeds a wider tree and, in
/// a single `transaction`, moves several nodes around (some out, some to a new
/// depth). Asserts the recursive matview equals its recompute afterwards.
#[tokio::test]
async fn rung7_bulk_batch_reparent_no_drift() {
    let (handle, _rx) = new_db().await;
    // page (anchor) → b0 → b1 → b2 → b3 → b4  (a deep chain, the bulk-0-* shape)
    handle
        .execute(
            "CREATE TABLE blk_raw (id TEXT PRIMARY KEY, parent_id TEXT, content TEXT, \
             sort_key TEXT)",
            vec![],
        )
        .await
        .expect("create blk_raw");
    for (id, parent, sk) in [
        ("page", "root", "a"),
        ("b0", "page", "b"),
        ("b1", "b0", "c"),
        ("b2", "b1", "d"),
        ("b3", "b2", "e"),
        ("b4", "b3", "f"),
        ("outside", "root", "z"),
    ] {
        handle
            .execute(
                "INSERT INTO blk_raw (id, parent_id, content, sort_key) VALUES (?, ?, ?, ?)",
                vec![
                    turso::Value::Text(id.into()),
                    turso::Value::Text(parent.into()),
                    turso::Value::Text(id.into()),
                    turso::Value::Text(sk.into()),
                ],
            )
            .await
            .expect("seed bulk row");
    }
    create_inner_matview(&handle).await;
    reconcile_named_view(&handle, "fdesc7", FOCUS_DESCENDANTS_JOINED_SQL)
        .await
        .expect("fdesc7 joined recursive matview");

    // ONE transaction (single CDC batch): bulk re-parents — b4 under b2 (evidence
    // shape), b3 up under page (depth change), b1 out to `outside` (cascades b0's
    // remaining subtree). All three deltas land in the SAME maintenance batch.
    handle
        .transaction(vec![
            (
                "UPDATE blk_raw SET parent_id = 'b2' WHERE id = 'b4'".to_string(),
                vec![],
            ),
            (
                "UPDATE blk_raw SET parent_id = 'page' WHERE id = 'b3'".to_string(),
                vec![],
            ),
            (
                "UPDATE blk_raw SET parent_id = 'outside' WHERE id = 'b1'".to_string(),
                vec![],
            ),
        ])
        .await
        .expect("commit bulk reparent txn");

    let matview = canon_rows(
        handle
            .query("SELECT node_id, depth FROM fdesc7", HashMap::new())
            .await
            .expect("query fdesc7 matview"),
    );
    let recompute = canon_rows(
        handle
            .query(FOCUS_DESCENDANTS_JOINED_SQL, HashMap::new())
            .await
            .expect("recompute fdesc7"),
    );
    eprintln!(
        "\n===== RUNG 7 bulk batch re-parent: matview={matview:?} recompute={recompute:?} ====="
    );
    assert_eq!(
        matview, recompute,
        "MATVIEW-VS-RECOMPUTE DRIFT: bulk/batch re-parent in one txn desynced the IVM recursive \
         matview from its recompute — the keystone bulk-reparent drift shape"
    );
}

/// Canonicalize a `(node_id, depth)` result set to a sorted `Vec<String>`, so a
/// stale row (same id, different depth — or an extra id) is visible as a
/// multiset difference between matview and recompute.
fn canon_rows(rows: Vec<holon_core::storage::StorageEntity>) -> Vec<String> {
    let mut out: Vec<String> = rows
        .iter()
        .map(|r| {
            let id = r
                .get("node_id")
                .and_then(|v| v.as_string())
                .expect("node_id text")
                .to_string();
            let depth = r
                .get("depth")
                .and_then(|v| v.as_i64())
                .expect("depth integer");
            format!("{id}@d{depth}")
        })
        .collect();
    out.sort();
    out
}

/// The main-panel watch shape with a **dynamic anchor**: the recursive CTE's
/// base case is `JOIN focus_roots fr ON b.id = fr.root_id`, so the anchor SET
/// itself is a matview that changes when the user navigates. Rungs 4–7 hold the
/// anchor literal (`WHERE _v0.id = 'page'`) fixed and only move blocks; this
/// shape is what `assets/default/index.org default-main-panel::src::0` really
/// registers.
const FOCUS_SWITCH_SQL: &str = "\
    WITH RECURSIVE focus_descendants AS ( \
        SELECT b.id AS node_id, b.id AS source_id, 0 AS depth, CAST(b.id AS TEXT) AS visited \
        FROM blk b JOIN focus_roots fr ON b.id = fr.root_id \
        UNION ALL \
        SELECT child.id, focus_descendants.source_id, focus_descendants.depth + 1, \
               focus_descendants.visited || ',' || CAST(child.id AS TEXT) \
        FROM focus_descendants \
        JOIN blk child ON child.parent_id = focus_descendants.node_id \
        WHERE focus_descendants.depth < 20 \
          AND ',' || focus_descendants.visited || ',' NOT LIKE \
              '%,' || CAST(child.id AS TEXT) || ',%' \
    ) \
    SELECT d.id AS node_id, focus_descendants.depth AS depth \
    FROM focus_roots fr \
    JOIN blk root ON root.id = fr.root_id \
    JOIN focus_descendants ON focus_descendants.source_id = root.id \
    JOIN blk d ON d.id = focus_descendants.node_id \
    JOIN navigation_cursor nc ON nc.region = fr.region AND nc.history_id = fr.history_id \
    WHERE fr.region = 'main'";

/// Seed the navigation tables + `focus_roots` matview with ONE open row (H1)
/// focusing `page`, cursor on it. Mirrors `navigation.sql` /
/// `matview_focus_roots.sql` verbatim (same shapes rung 3 uses).
async fn seed_navigation_on_page(handle: &DbHandle) {
    handle
        .execute(
            "CREATE TABLE navigation_history (id INTEGER PRIMARY KEY AUTOINCREMENT, \
             region TEXT NOT NULL, block_id TEXT, closed_at TEXT NULL)",
            vec![],
        )
        .await
        .expect("navigation_history");
    handle
        .execute(
            "CREATE TABLE navigation_cursor (region TEXT PRIMARY KEY, history_id INTEGER)",
            vec![],
        )
        .await
        .expect("navigation_cursor");
    handle
        .execute(
            "INSERT INTO navigation_history (id, region, block_id, closed_at) \
             VALUES (1, 'main', 'page', NULL)",
            vec![],
        )
        .await
        .expect("H1 open on page");
    handle
        .execute(
            "INSERT INTO navigation_cursor (region, history_id) VALUES ('main', 1)",
            vec![],
        )
        .await
        .expect("cursor at H1");
    reconcile_named_view(
        handle,
        "focus_roots",
        "SELECT region, block_id AS root_id, id AS history_id \
         FROM navigation_history WHERE closed_at IS NULL AND block_id IS NOT NULL",
    )
    .await
    .expect("focus_roots matview");
}

/// `focus_replace` + cursor move, as ONE transaction — the exact DML
/// `NavigateFocus` runs: close the open row for the region, open a new one on
/// the target, point the cursor at it.
async fn navigate_focus_to(handle: &DbHandle, new_history_id: i64, target: &str) {
    handle
        .transaction(vec![
            (
                "UPDATE navigation_history SET closed_at = '2026-07-25T00:00:00' \
                 WHERE region = 'main' AND closed_at IS NULL"
                    .to_string(),
                vec![],
            ),
            (
                "INSERT INTO navigation_history (id, region, block_id, closed_at) \
                 VALUES (?, 'main', ?, NULL)"
                    .to_string(),
                vec![
                    turso::Value::Integer(new_history_id),
                    turso::Value::Text(target.into()),
                ],
            ),
            (
                "UPDATE navigation_cursor SET history_id = ? WHERE region = 'main'".to_string(),
                vec![turso::Value::Integer(new_history_id)],
            ),
        ])
        .await
        .expect("navigate focus txn");
}

/// Seed a deep chain under `page` plus a SEPARATE navigable root `page2`:
///   page → c1 → n0 → n1 → n2      (the split/outdent working set)
///   page2 → d1                    (the NavigateFocus target subtree)
async fn seed_two_roots(handle: &DbHandle) {
    handle
        .execute(
            "CREATE TABLE blk_raw (id TEXT PRIMARY KEY, parent_id TEXT, content TEXT, \
             sort_key TEXT)",
            vec![],
        )
        .await
        .expect("create blk_raw");
    for (id, parent, sk) in [
        ("page", "root", "a"),
        ("c1", "page", "b"),
        ("n0", "c1", "c"),
        ("n1", "n0", "d"),
        ("n2", "n1", "e"),
        ("page2", "root", "y"),
        ("d1", "page2", "z"),
    ] {
        handle
            .execute(
                "INSERT INTO blk_raw (id, parent_id, content, sort_key) VALUES (?, ?, ?, ?)",
                vec![
                    turso::Value::Text(id.into()),
                    turso::Value::Text(parent.into()),
                    turso::Value::Text(id.into()),
                    turso::Value::Text(sk.into()),
                ],
            )
            .await
            .expect("seed two-root row");
    }
    create_inner_matview(handle).await;
}

/// RUNG 8 — the ANCHOR SWITCH the earlier rungs never exercise: `NavigateFocus`
/// retracts the whole old anchor's recursive closure and asserts a new one, in
/// ONE batch. Rungs 4–7 move blocks under a LITERAL anchor; rung 3 collapses
/// the anchor to empty. Neither covers "anchor A out, anchor B in".
#[tokio::test]
async fn rung8_focus_root_switch_retracts_old_subtree() {
    let (handle, _rx) = new_db().await;
    seed_two_roots(&handle).await;
    seed_navigation_on_page(&handle).await;
    reconcile_named_view(&handle, "fdesc8", FOCUS_SWITCH_SQL)
        .await
        .expect("fdesc8 dynamic-anchor recursive matview");

    let before = canon_rows(
        handle
            .query("SELECT node_id, depth FROM fdesc8", HashMap::new())
            .await
            .expect("query fdesc8 baseline"),
    );
    assert_eq!(
        before,
        vec!["c1@d1", "n0@d2", "n1@d3", "n2@d4", "page@d0"],
        "baseline: the panel shows the whole `page` subtree"
    );

    navigate_focus_to(&handle, 2, "page2").await;

    let matview = canon_rows(
        handle
            .query("SELECT node_id, depth FROM fdesc8", HashMap::new())
            .await
            .expect("query fdesc8 matview"),
    );
    let recompute = canon_rows(
        handle
            .query(FOCUS_SWITCH_SQL, HashMap::new())
            .await
            .expect("recompute fdesc8"),
    );
    eprintln!(
        "\n===== RUNG 8 focus-root switch: matview={matview:?} recompute={recompute:?} ====="
    );
    assert_eq!(
        matview, recompute,
        "MATVIEW-VS-RECOMPUTE DRIFT: switching the focus root left the previous root's rows in \
         the dynamic-anchor recursive matview — the main-panel stale-row shape"
    );
}

/// RUNG 9 — the OBSERVED sweep shape: blocks are CREATED under the old focus
/// root (SplitBlock) and RE-PARENTED (Outdent) shortly before the focus root
/// switches away. The recursive closure of the old anchor is therefore built
/// from deltas that arrived AFTER the matview was created, which is the only
/// difference from rung 8.
#[tokio::test]
async fn rung9_split_then_outdent_then_focus_switch_retracts_old_subtree() {
    let (handle, _rx) = new_db().await;
    seed_two_roots(&handle).await;
    seed_navigation_on_page(&handle).await;
    reconcile_named_view(&handle, "fdesc9", FOCUS_SWITCH_SQL)
        .await
        .expect("fdesc9 dynamic-anchor recursive matview");

    // SplitBlock ×2: two NEW blocks born under the currently-focused subtree.
    handle
        .execute(
            "INSERT INTO blk_raw (id, parent_id, content, sort_key) \
             VALUES ('s1', 'n2', 's1', 'f')",
            vec![],
        )
        .await
        .expect("split 1");
    handle
        .execute(
            "INSERT INTO blk_raw (id, parent_id, content, sort_key) \
             VALUES ('s2', 's1', 's2', 'g')",
            vec![],
        )
        .await
        .expect("split 2");
    // Outdent: re-parent s1 (carrying s2) one level up.
    handle
        .execute(
            "UPDATE blk_raw SET parent_id = 'n1' WHERE id = 's1'",
            vec![],
        )
        .await
        .expect("outdent s1");

    navigate_focus_to(&handle, 2, "page2").await;

    let matview = canon_rows(
        handle
            .query("SELECT node_id, depth FROM fdesc9", HashMap::new())
            .await
            .expect("query fdesc9 matview"),
    );
    let recompute = canon_rows(
        handle
            .query(FOCUS_SWITCH_SQL, HashMap::new())
            .await
            .expect("recompute fdesc9"),
    );
    eprintln!(
        "\n===== RUNG 9 split+outdent then focus switch: matview={matview:?} recompute={recompute:?} ====="
    );
    assert_eq!(
        matview, recompute,
        "MATVIEW-VS-RECOMPUTE DRIFT: rows created/re-parented under the OLD focus root survived \
         the focus-root switch in the dynamic-anchor recursive matview — the observed sweep's \
         stale-row shape (stale ids were freshly split blocks under a departed root)"
    );
}

/// Same dynamic-anchor shape as [`FOCUS_SWITCH_SQL`], but projecting `id` (as
/// the prod `SELECT d.*` does) so the CDC change payload carries an identity
/// the delta-consuming frontend can key on.
const FOCUS_SWITCH_SQL_WITH_ID: &str = "\
    WITH RECURSIVE focus_descendants AS ( \
        SELECT b.id AS node_id, b.id AS source_id, 0 AS depth, CAST(b.id AS TEXT) AS visited \
        FROM blk b JOIN focus_roots fr ON b.id = fr.root_id \
        UNION ALL \
        SELECT child.id, focus_descendants.source_id, focus_descendants.depth + 1, \
               focus_descendants.visited || ',' || CAST(child.id AS TEXT) \
        FROM focus_descendants \
        JOIN blk child ON child.parent_id = focus_descendants.node_id \
        WHERE focus_descendants.depth < 20 \
          AND ',' || focus_descendants.visited || ',' NOT LIKE \
              '%,' || CAST(child.id AS TEXT) || ',%' \
    ) \
    SELECT d.id AS id, d.parent_id AS parent_id, d.content AS content, \
           focus_descendants.depth AS depth \
    FROM focus_roots fr \
    JOIN blk root ON root.id = fr.root_id \
    JOIN focus_descendants ON focus_descendants.source_id = root.id \
    JOIN blk d ON d.id = focus_descendants.node_id \
    JOIN navigation_cursor nc ON nc.region = fr.region AND nc.history_id = fr.history_id \
    WHERE fr.region = 'main'";

/// RUNG 10 — CDC DELTA BALANCE (not content). A delta-driven mirror (the
/// frontend's keyed row provider → `MutableTree`) never re-reads the matview;
/// it only applies the CDC stream. So the matview can be content-correct while
/// its emitted deltas are unbalanced — a row asserted twice and retracted once
/// stays in every downstream mirror forever. This rung replays the observed
/// sweep (create under the focused subtree, re-parent, then switch the focus
/// root) and asserts the NET per-id delta equals the final matview membership.
#[tokio::test]
async fn rung10_cdc_delta_balance_across_focus_switch() {
    let (handle, mut rx) = new_db().await;
    seed_two_roots(&handle).await;
    seed_navigation_on_page(&handle).await;
    reconcile_named_view(&handle, "fdesc10", FOCUS_SWITCH_SQL_WITH_ID)
        .await
        .expect("fdesc10 dynamic-anchor recursive matview");

    // Seed the mirror with the matview's INITIAL population: a watch registers
    // by reading the view once, then follows the CDC stream. Only the stream is
    // under test here.
    let mut net: HashMap<String, i64> = HashMap::new();
    for r in handle
        .query("SELECT id FROM fdesc10", HashMap::new())
        .await
        .expect("initial fdesc10 population")
    {
        let id = r.get("id").and_then(|v| v.as_string()).expect("id text");
        *net.entry(id.to_string()).or_default() += 1;
    }

    let mut absorb = |batches: Vec<CapturedBatch>| {
        for b in &batches {
            for k in &b.kinds {
                let (tag, id) = k.split_once(':').expect("kind tag");
                match tag {
                    "C" => *net.entry(id.to_string()).or_default() += 1,
                    "D" => *net.entry(id.to_string()).or_default() -= 1,
                    "U" | "F" => {}
                    other => panic!("unexpected CDC kind tag {other:?} in {k:?}"),
                }
            }
        }
        eprintln!("  batches: {batches:?}");
    };

    absorb(drain(&mut rx, "fdesc10").await);

    handle
        .execute(
            "INSERT INTO blk_raw (id, parent_id, content, sort_key) \
             VALUES ('s1', 'n2', 's1', 'f')",
            vec![],
        )
        .await
        .expect("split 1");
    absorb(drain(&mut rx, "fdesc10").await);
    handle
        .execute(
            "INSERT INTO blk_raw (id, parent_id, content, sort_key) \
             VALUES ('s2', 's1', 's2', 'g')",
            vec![],
        )
        .await
        .expect("split 2");
    absorb(drain(&mut rx, "fdesc10").await);
    handle
        .execute(
            "UPDATE blk_raw SET parent_id = 'n1' WHERE id = 's1'",
            vec![],
        )
        .await
        .expect("outdent s1");
    absorb(drain(&mut rx, "fdesc10").await);

    navigate_focus_to(&handle, 2, "page2").await;
    absorb(drain(&mut rx, "fdesc10").await);

    let mut delta_membership: Vec<String> = net
        .iter()
        .filter(|(_, n)| **n != 0)
        .map(|(id, n)| format!("{id}x{n}"))
        .collect();
    delta_membership.sort();
    let rows = handle
        .query("SELECT id FROM fdesc10", HashMap::new())
        .await
        .expect("query fdesc10");
    let mut actual: Vec<String> = rows
        .iter()
        .map(|r| {
            format!(
                "{}x1",
                r.get("id").and_then(|v| v.as_string()).expect("id text")
            )
        })
        .collect();
    actual.sort();
    eprintln!(
        "\n===== RUNG 10 CDC delta balance: net={delta_membership:?} matview={actual:?} ====="
    );
    assert_eq!(
        delta_membership, actual,
        "CDC DELTA IMBALANCE: the net of emitted Created/Deleted CDC events does not equal the \
         matview's actual membership. A delta-driven mirror (the frontend row provider → \
         MutableTree) can never converge — this is the main-panel stale-row shape."
    );
}

/// RUNG 11 — DELETE (not re-parent) of nodes inside the anchored subtree.
/// Rungs 4–7 only ever `UPDATE ... SET parent_id`; nothing here covers a base
/// row being REMOVED, which is what an undo / external-rewrite / block-delete
/// does. A retract-miss on delete leaves a row the recompute no longer has —
/// the "matview has MORE rows than recompute" drift shape.
#[tokio::test]
async fn rung11_delete_inside_subtree_retracts_from_recursive_matview() {
    let (handle, _rx) = new_db().await;
    seed_two_roots(&handle).await;
    seed_navigation_on_page(&handle).await;
    reconcile_named_view(&handle, "fdesc11", FOCUS_SWITCH_SQL)
        .await
        .expect("fdesc11 dynamic-anchor recursive matview");

    // Delete a LEAF, then an INTERMEDIATE node (whose descendant must cascade
    // out of the closure), in separate batches.
    handle
        .execute("DELETE FROM blk_raw WHERE id = 'n2'", vec![])
        .await
        .expect("delete leaf n2");
    let after_leaf = canon_rows(
        handle
            .query("SELECT node_id, depth FROM fdesc11", HashMap::new())
            .await
            .expect("query fdesc11 after leaf delete"),
    );
    let recompute_leaf = canon_rows(
        handle
            .query(FOCUS_SWITCH_SQL, HashMap::new())
            .await
            .expect("recompute after leaf delete"),
    );
    eprintln!(
        "\n===== RUNG 11a leaf delete: matview={after_leaf:?} recompute={recompute_leaf:?} ====="
    );
    assert_eq!(
        after_leaf, recompute_leaf,
        "MATVIEW-VS-RECOMPUTE DRIFT: deleting a LEAF inside the anchored subtree left a stale \
         row in the IVM recursive matview"
    );

    handle
        .execute("DELETE FROM blk_raw WHERE id = 'n0'", vec![])
        .await
        .expect("delete intermediate n0");
    let after_mid = canon_rows(
        handle
            .query("SELECT node_id, depth FROM fdesc11", HashMap::new())
            .await
            .expect("query fdesc11 after intermediate delete"),
    );
    let recompute_mid = canon_rows(
        handle
            .query(FOCUS_SWITCH_SQL, HashMap::new())
            .await
            .expect("recompute after intermediate delete"),
    );
    eprintln!(
        "\n===== RUNG 11b intermediate delete: matview={after_mid:?} recompute={recompute_mid:?} \
         ====="
    );
    assert_eq!(
        after_mid, recompute_mid,
        "MATVIEW-VS-RECOMPUTE DRIFT: deleting an INTERMEDIATE node left its transitive \
         descendant (n1) in the IVM recursive matview"
    );
}

/// RUNG 12 — re-parent DEEPER (the `bulk-3-*` sweep shape): a node moves from
/// the subtree root to a node that is itself a descendant, so BOTH its own
/// depth and its whole subtree's depths change, inside ONE batch alongside a
/// sibling delete. This is the closest SQL-level model of the observed
/// `inv-matview-consistent-with-recompute` drift (matview 13 / recompute 12,
/// the extra row being a re-parented `bulk-3-5` at a stale depth).
#[tokio::test]
async fn rung12_reparent_deeper_with_concurrent_delete_no_drift() {
    let (handle, _rx) = new_db().await;
    seed_two_roots(&handle).await;
    seed_navigation_on_page(&handle).await;
    reconcile_named_view(&handle, "fdesc12", FOCUS_SWITCH_SQL)
        .await
        .expect("fdesc12 dynamic-anchor recursive matview");

    handle
        .execute(
            "INSERT INTO blk_raw (id, parent_id, content, sort_key) \
             VALUES ('m1', 'c1', 'm1', 'f')",
            vec![],
        )
        .await
        .expect("insert m1");
    handle
        .execute(
            "INSERT INTO blk_raw (id, parent_id, content, sort_key) \
             VALUES ('m2', 'm1', 'm2', 'g')",
            vec![],
        )
        .await
        .expect("insert m2");

    // ONE batch: move m1 (carrying m2) deeper, under n1; delete n2 alongside.
    handle
        .transaction(vec![
            (
                "UPDATE blk_raw SET parent_id = 'n1' WHERE id = 'm1'".to_string(),
                vec![],
            ),
            ("DELETE FROM blk_raw WHERE id = 'n2'".to_string(), vec![]),
        ])
        .await
        .expect("commit deeper-reparent + delete txn");

    let matview = canon_rows(
        handle
            .query("SELECT node_id, depth FROM fdesc12", HashMap::new())
            .await
            .expect("query fdesc12"),
    );
    let recompute = canon_rows(
        handle
            .query(FOCUS_SWITCH_SQL, HashMap::new())
            .await
            .expect("recompute fdesc12"),
    );
    eprintln!(
        "\n===== RUNG 12 deeper re-parent + delete: matview={matview:?} recompute={recompute:?} \
         ====="
    );
    assert_eq!(
        matview, recompute,
        "MATVIEW-VS-RECOMPUTE DRIFT: a depth-increasing re-parent batched with a sibling delete \
         left a stale-depth row in the IVM recursive matview — the observed sweep's \
         inv-matview-consistent-with-recompute shape"
    );
}

/// RUNG 13 — the OBSERVED keystone drift shape, minimized. Outdent's
/// sibling-adoption rule makes a node's parent move AND that node get re-homed
/// onto its own moved parent inside ONE batch:
///
///   doc → b0 → b4 → b5
///   Outdent(b5)  ⇒ b5's parent becomes b0 (it trails b4 as a sibling)
///   Outdent(b4)  ⇒ b4's parent becomes doc, and b5 — b4's following sibling —
///                  is re-adopted BY b4. Both UPDATEs land in the same batch,
///                  and b5's new parent is the very node that moved.
///
/// Every earlier rung moves nodes whose new parent is STATIONARY in that batch.
/// The sweep's `inv-matview-consistent-with-recompute` red is exactly this:
/// the matview retained `bulk-3-5 @ parent bulk-3-0` (the intermediate state)
/// while the recompute no longer had that row at all.
#[tokio::test]
async fn rung13_outdent_chain_child_readopted_by_its_own_moved_parent() {
    let (handle, _rx) = new_db().await;
    handle
        .execute(
            "CREATE TABLE blk_raw (id TEXT PRIMARY KEY, parent_id TEXT, content TEXT, \
             sort_key TEXT)",
            vec![],
        )
        .await
        .expect("create blk_raw");
    for (id, parent, sk) in [
        ("page", "root", "a"),
        ("b0", "page", "b"),
        ("b4", "b0", "c"),
        ("b5", "b4", "d"),
    ] {
        handle
            .execute(
                "INSERT INTO blk_raw (id, parent_id, content, sort_key) VALUES (?, ?, ?, ?)",
                vec![
                    turso::Value::Text(id.into()),
                    turso::Value::Text(parent.into()),
                    turso::Value::Text(id.into()),
                    turso::Value::Text(sk.into()),
                ],
            )
            .await
            .expect("seed rung13 row");
    }
    create_inner_matview(&handle).await;
    seed_navigation_on_page(&handle).await;
    reconcile_named_view(&handle, "fdesc13", FOCUS_SWITCH_SQL)
        .await
        .expect("fdesc13 dynamic-anchor recursive matview");

    // Outdent(b5): b5 rises to sit beside b4 under b0.
    handle
        .execute(
            "UPDATE blk_raw SET parent_id = 'b0' WHERE id = 'b5'",
            vec![],
        )
        .await
        .expect("outdent b5");

    // Outdent(b4): b4 rises under `page`, and its following sibling b5 is
    // re-adopted by b4 — ONE batch, and b5's new parent is the moved node.
    handle
        .transaction(vec![
            (
                "UPDATE blk_raw SET parent_id = 'page' WHERE id = 'b4'".to_string(),
                vec![],
            ),
            (
                "UPDATE blk_raw SET parent_id = 'b4' WHERE id = 'b5'".to_string(),
                vec![],
            ),
        ])
        .await
        .expect("commit outdent-b4 + readopt-b5 txn");

    let matview = canon_rows(
        handle
            .query("SELECT node_id, depth FROM fdesc13", HashMap::new())
            .await
            .expect("query fdesc13"),
    );
    let recompute = canon_rows(
        handle
            .query(FOCUS_SWITCH_SQL, HashMap::new())
            .await
            .expect("recompute fdesc13"),
    );
    eprintln!(
        "\n===== RUNG 13 outdent-chain re-adoption: matview={matview:?} recompute={recompute:?} \
         ====="
    );
    assert_eq!(
        matview, recompute,
        "MATVIEW-VS-RECOMPUTE DRIFT: a child re-adopted by its OWN moved parent in the same \
         batch left the intermediate-state row in the IVM recursive matview — the keystone \
         inv-matview-consistent-with-recompute / main-panel stale-row shape"
    );
}

/// `id`+`depth`-keyed canonicalizer for the `*_WITH_ID` projection (rung 14's
/// view B), which carries `id` rather than `node_id`.
fn canon_id_rows(rows: Vec<holon_core::storage::StorageEntity>) -> Vec<String> {
    let mut out: Vec<String> = rows
        .iter()
        .map(|r| {
            let id = r
                .get("id")
                .and_then(|v| v.as_string())
                .expect("id text")
                .to_string();
            let depth = r
                .get("depth")
                .and_then(|v| v.as_i64())
                .expect("depth integer");
            format!("{id}@d{depth}")
        })
        .collect();
    out.sort();
    out
}

/// A THIRD recursive descendants shape over the SAME `blk` base rows, but with
/// a LITERAL anchor (`WHERE b.id = 'page'`) instead of the dynamic
/// `focus_roots` join — so it overlaps rungs A/B on every base row yet is a
/// distinct IVM circuit maintained in the same reparent commit. Projects
/// `node_id, depth` (reuses [`canon_rows`]).
const FOCUS_DESCENDANTS_ND_SQL: &str = "\
    WITH RECURSIVE _d AS ( \
        SELECT b.id AS node_id, 0 AS depth, CAST(b.id AS TEXT) AS visited \
        FROM blk b WHERE b.id = 'page' \
        UNION ALL \
        SELECT child.id, _d.depth + 1, _d.visited || ',' || CAST(child.id AS TEXT) \
        FROM _d JOIN blk child ON child.parent_id = _d.node_id \
        WHERE _d.depth < 20 \
          AND ',' || _d.visited || ',' NOT LIKE '%,' || CAST(child.id AS TEXT) || ',%' \
    ) \
    SELECT node_id, depth FROM _d";

/// Non-blocking drain of the CDC broadcast — keeps a LIVE subscriber consuming
/// so the channel stays un-lagged during the reparent commits (mirrors the
/// composed keystone's live watch subscriber), without the 250 ms settle that
/// [`drain`] uses. Content is discarded; the oracle is matview-vs-recompute.
fn drain_live(rx: &mut Receiver<BatchWithMetadata<RowChange>>) -> usize {
    let mut n = 0;
    while rx.try_recv().is_ok() {
        n += 1;
    }
    n
}

/// RUNG 14 — the MISSING INGREDIENT the isolated rungs never add: **N
/// overlapping recursive matviews maintained in the SAME reparent commit** over
/// shared base rows, plus a live CDC subscriber, looping rung 13's exact
/// re-adoption delta to hit the ~10% probabilistic retract-miss.
///
/// rung 13 replays the identical `{b4→page, b5→b4}` re-adoption batch and is
/// GREEN 14/14 — because it maintains ONE outer recursive matview (`fdesc13`)
/// with no live subscriber. The composed holon keystone REDS ~10% on exactly
/// this delta because it maintains **three `watch_view_*` recursive matviews
/// plus a chained stack** in the one commit. The unmodeled variable is
/// TOPOLOGICAL (many overlapping recursive matviews per transaction), NOT
/// concurrency: holon serializes every write and all matview maintenance
/// through a single actor connection (investigation §3). This rung reproduces
/// that topology at the storage layer:
///
///   A = `fdesc14a` (FOCUS_SWITCH_SQL, dynamic `focus_roots` anchor)
///   B = `fdesc14b` (FOCUS_SWITCH_SQL_WITH_ID, same anchor, `id` projection)
///   C = `fdesc14c` (literal-anchor recursive descendants over `blk`)
///   D = `fdesc14d` (CHAINED matview: `SELECT node_id, depth FROM fdesc14a`)
///
/// All four close over `b4`/`b5`, so the atomic re-adoption batch maintains all
/// four in one commit. Each iteration asserts `matview == recompute` for A/B/C
/// (independent recompute of each defining SELECT off the base matviews) and
/// `D == A` (chained-view self-consistency).
///
/// OUTCOME (2026-07-27, Turso rev `de14a597`): this rung stays **GREEN** — the
/// `reconcile_named_view` storage-layer topology does NOT reproduce the miss,
/// across three shape variations (bounded-reset 4-node, the exact `bulk-3`
/// sibling fan-out, and accumulate-growing) at 200–500 iterations each. The
/// holon-side keystone still reds ~10% on the identical delta, so the unmodeled
/// factor is NOT matview count/topology alone: it is the **watch-registration
/// path** — the composed keystone's matviews are `watch_view_*` compiled by
/// holon's `watch_query` (carrying `rowid` tracking, `SELECT *, rowid AS _rowid
/// FROM watch_view_*`), a different IVM circuit than `reconcile_named_view`
/// produces — plus the full `blocks_with_paths`/`root_layout` chained stack.
/// rung 14 therefore stands as the topology BOUNDARY (rung 13 = 1 matview
/// green; rung 14 = 4 overlapping matviews still green) that narrows the Turso
/// search to the watch-view circuit. It is a red-first gate for the eventual
/// Turso fix if the watch path is later added here; keep the assert as-is.
///
/// If it ever reds, the signature is a `watch_view`-shaped view retaining the
/// intermediate `b5 @ parent b0` derivation (`ivm-race-det-run4.log:549`,
/// `matview 13 / recompute 12`).
#[tokio::test]
async fn rung14_multi_overlapping_recursive_matviews_per_reparent_commit() {
    let (handle, mut rx) = new_db().await;

    // Seed the EXACT `bulk-3` fan-out from the red log (page plays `ref-doc-0`):
    //   page → b0 → b4 → b5   (the outdent working chain)
    //   page → b1 → {b6, b7}
    //   page → b2 → b3
    // rung 13 minimized this to the bare 4-node chain and stays green; the
    // sibling branches share base rows with the recursive closure and may be
    // load-bearing for the multi-matview consolidation miss.
    handle
        .execute(
            "CREATE TABLE blk_raw (id TEXT PRIMARY KEY, parent_id TEXT, content TEXT, \
             sort_key TEXT)",
            vec![],
        )
        .await
        .expect("create blk_raw");
    for (id, parent, sk) in [
        ("page", "root", "a"),
        ("b0", "page", "b"),
        ("b4", "b0", "c"),
        ("b5", "b4", "d"),
        ("b1", "page", "e"),
        ("b6", "b1", "f"),
        ("b7", "b1", "g"),
        ("b2", "page", "h"),
        ("b3", "b2", "i"),
    ] {
        handle
            .execute(
                "INSERT INTO blk_raw (id, parent_id, content, sort_key) VALUES (?, ?, ?, ?)",
                vec![
                    turso::Value::Text(id.into()),
                    turso::Value::Text(parent.into()),
                    turso::Value::Text(id.into()),
                    turso::Value::Text(sk.into()),
                ],
            )
            .await
            .expect("seed rung14 row");
    }
    create_inner_matview(&handle).await;
    seed_navigation_on_page(&handle).await;

    // The overlapping topology: three recursive matviews over the shared base
    // rows + one chained matview reading the first.
    reconcile_named_view(&handle, "fdesc14a", FOCUS_SWITCH_SQL)
        .await
        .expect("fdesc14a dynamic-anchor recursive matview");
    reconcile_named_view(&handle, "fdesc14b", FOCUS_SWITCH_SQL_WITH_ID)
        .await
        .expect("fdesc14b dynamic-anchor recursive matview (id projection)");
    reconcile_named_view(&handle, "fdesc14c", FOCUS_DESCENDANTS_ND_SQL)
        .await
        .expect("fdesc14c literal-anchor recursive matview");
    reconcile_named_view(&handle, "fdesc14d", "SELECT node_id, depth FROM fdesc14a")
        .await
        .expect("fdesc14d chained matview over fdesc14a");

    let iterations: usize = std::env::var("RUNG14_ITERATIONS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(200);

    for iter in 0..iterations {
        // Outdent(b5): b5 rises to sit beside b4 under b0.
        handle
            .execute(
                "UPDATE blk_raw SET parent_id = 'b0' WHERE id = 'b5'",
                vec![],
            )
            .await
            .expect("outdent b5");

        // Outdent(b4) + re-adopt(b5): b4 rises under `page`, and b5 — b4's
        // following sibling — is re-adopted BY b4, in ONE batch. b5's new parent
        // is the very node that also moved. This is rung 13's exact delta, now
        // maintaining four overlapping recursive/chained matviews per commit.
        handle
            .transaction(vec![
                (
                    "UPDATE blk_raw SET parent_id = 'page' WHERE id = 'b4'".to_string(),
                    vec![],
                ),
                (
                    "UPDATE blk_raw SET parent_id = 'b4' WHERE id = 'b5'".to_string(),
                    vec![],
                ),
            ])
            .await
            .expect("commit outdent-b4 + readopt-b5 txn");

        // Live subscriber keeps consuming the CDC stream produced by the commit.
        drain_live(&mut rx);

        // Per-view differential oracle: each IVM matview vs. a fresh recompute
        // of its own defining SELECT off the (quiescent) base matviews.
        let a_mv = canon_rows(
            handle
                .query("SELECT node_id, depth FROM fdesc14a", HashMap::new())
                .await
                .expect("query fdesc14a"),
        );
        let a_re = canon_rows(
            handle
                .query(FOCUS_SWITCH_SQL, HashMap::new())
                .await
                .expect("recompute fdesc14a"),
        );
        assert_eq!(
            a_mv, a_re,
            "iter {iter}: MATVIEW-VS-RECOMPUTE DRIFT on view A (fdesc14a): the re-adoption batch \
             left a stale intermediate row in one of N overlapping recursive matviews — the \
             keystone inv-matview-consistent-with-recompute / main-panel stale-row shape"
        );

        let b_mv = canon_id_rows(
            handle
                .query("SELECT id, depth FROM fdesc14b", HashMap::new())
                .await
                .expect("query fdesc14b"),
        );
        let b_re = canon_id_rows(
            handle
                .query(FOCUS_SWITCH_SQL_WITH_ID, HashMap::new())
                .await
                .expect("recompute fdesc14b"),
        );
        assert_eq!(
            b_mv, b_re,
            "iter {iter}: MATVIEW-VS-RECOMPUTE DRIFT on view B (fdesc14b, id projection): stale \
             intermediate row retained under multi-matview per-transaction maintenance"
        );

        let c_mv = canon_rows(
            handle
                .query("SELECT node_id, depth FROM fdesc14c", HashMap::new())
                .await
                .expect("query fdesc14c"),
        );
        let c_re = canon_rows(
            handle
                .query(FOCUS_DESCENDANTS_ND_SQL, HashMap::new())
                .await
                .expect("recompute fdesc14c"),
        );
        assert_eq!(
            c_mv, c_re,
            "iter {iter}: MATVIEW-VS-RECOMPUTE DRIFT on view C (fdesc14c, literal anchor): stale \
             intermediate row retained under multi-matview per-transaction maintenance"
        );

        let d_mv = canon_rows(
            handle
                .query("SELECT node_id, depth FROM fdesc14d", HashMap::new())
                .await
                .expect("query fdesc14d"),
        );
        assert_eq!(
            d_mv, a_mv,
            "iter {iter}: CHAINED-MATVIEW DRIFT: view D (fdesc14d, matview over fdesc14a) diverged \
             from its own source matview A — chained per-transaction maintenance dropped a delta"
        );

        // Reset to the pre-state (page → b0 → b4 → b5) for the next iteration:
        // b5 is already under b4; only b4 must return under b0.
        handle
            .execute(
                "UPDATE blk_raw SET parent_id = 'b0' WHERE id = 'b4'",
                vec![],
            )
            .await
            .expect("reset b4 under b0");
        drain_live(&mut rx);
    }

    eprintln!(
        "\n===== RUNG 14 — {iterations} re-adoption iterations over 4 overlapping matviews: \
         all views stayed consistent with recompute (no retract-miss observed this run) ====="
    );
}
