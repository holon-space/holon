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
