//! Does a REPLACE that overwrites an existing row with an UNCHANGED value
//! corrupt the matview maintained over that table?
//!
//! The fork's IVM captures the old row TWICE on the REPLACE path —
//! `Insn::Delete` captures it for view maintenance and the following
//! `Insn::Insert` (REQUIRE_SEEK branch) captures it again — so one replace
//! emits two retractions against one insertion and the row's weight falls to
//! -1.
//!
//! Holon writes REPLACE into two tables that are matview bases:
//! `navigation_cursor` (base of `current_focus`) and `block_links` (base of
//! `backlinks`); the MCP vtable writeback does the same into sidecar cache
//! tables. `navigation_history` is the one rowid-alias matview base in the
//! tree — and nothing in production REPLACEs into it, which is the absence the
//! DbHandle guard now enforces. This file measures the engine at the seam,
//! against the PRODUCTION DDL and the PRODUCTION write statements, for each of
//! the view shapes holon actually ships: JOIN, projection+filter, and
//! aggregate.
//!
//! The oracle is always the same and is the only one that can see this class of
//! corruption: the matview must equal a RECOMPUTE of its own defining SELECT
//! against the base tables. Reading the matview alone cannot detect a weight
//! that has gone negative until a later delta makes a row vanish.

use std::collections::HashMap;

use holon_api::Value;
use holon_turso::matview_manager::reconcile_named_view;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;

/// The production navigation schema, verbatim from the shipped DDL.
const NAVIGATION_SCHEMA: &str = include_str!("../sql/schema/navigation.sql");
/// The production `current_focus` defining SELECT, verbatim.
const CURRENT_FOCUS_SELECT: &str = include_str!("../sql/schema/matview_current_focus.sql");
/// The production `block_links` junction DDL, verbatim.
const BLOCK_LINKS_SCHEMA: &str = include_str!("../sql/schema/block_links.sql");

/// A production SQL statement owned by the `holon` crate, read from disk.
///
/// These live in a different crate than this test, so they cannot be
/// `include_str!`d. Reading them at runtime is deliberate and is what makes the
/// measurement about PRODUCTION rather than about a transcription of it: fix
/// the statement in `crates/holon/sql/` and this test measures the fixed
/// statement on the next run.
fn holon_crate_sql(relative: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../holon")
        .join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read production SQL {}: {e}", path.display()))
}

async fn setup() -> DbHandle {
    let (backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(backend); // keep the actor alive for the test
    handle
}

/// Split a schema file into statements the same way the schema module does —
/// via the production splitter, which is comment-aware.
async fn apply_ddl(handle: &DbHandle, ddl: &str) {
    for stmt in holon_turso::sql_utils::sql_statements(ddl) {
        handle
            .execute_ddl(stmt)
            .await
            .expect("apply production DDL");
    }
}

fn text(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Integer(i)) => i.to_string(),
        Some(Value::Null) | None => "<null>".to_string(),
        other => panic!("unexpected column value {other:?}"),
    }
}

/// Every row of a result set as a sorted vector of stringified columns, so a
/// matview read and a recompute of its SELECT can be compared directly.
async fn rows_of(handle: &DbHandle, sql: &str, columns: &[&str]) -> Vec<Vec<String>> {
    let rows = handle
        .query(sql, HashMap::new())
        .await
        .unwrap_or_else(|e| panic!("query failed: {e}\nSQL: {sql}"));
    let mut out: Vec<Vec<String>> = rows
        .iter()
        .map(|r| columns.iter().map(|c| text(r.get(*c))).collect())
        .collect();
    out.sort();
    out
}

/// The assertion this whole file exists for: the incrementally maintained
/// matview must equal a fresh recompute of its defining SELECT.
async fn assert_matview_equals_recompute(
    handle: &DbHandle,
    view: &str,
    select_sql: &str,
    columns: &[&str],
    after: &str,
) {
    let projection = columns.join(", ");
    let maintained = rows_of(handle, &format!("SELECT {projection} FROM {view}"), columns).await;
    let recomputed = rows_of(
        handle,
        &format!(
            "SELECT {projection} FROM ({}) ",
            select_sql.trim().trim_end_matches(';')
        ),
        columns,
    )
    .await;
    assert_eq!(
        maintained, recomputed,
        "matview `{view}` diverged from a recompute of its own SELECT after {after}\n  \
         maintained (IVM): {maintained:?}\n  recomputed (truth): {recomputed:?}"
    );
}

// ---------------------------------------------------------------------------
// (a) navigation_cursor -> current_focus (JOIN), via the PRODUCTION statements
// ---------------------------------------------------------------------------

/// Re-focusing the region that is ALREADY focused runs the production cursor
/// upsert with a history_id it already holds. That is a REPLACE of an existing
/// row with an unchanged value, against a table the `current_focus` matview
/// joins.
///
/// GREEN, and the reason is the measurement this file exists for:
/// `navigation_cursor` is `region TEXT PRIMARY KEY`, so it is not a rowid-alias
/// table and the REPLACE is safe. The statement is read off disk, so if
/// `upsert_cursor.sql` or the key shape ever changes, this test follows.
#[tokio::test]
async fn production_cursor_upsert_with_unchanged_value_keeps_current_focus_correct() {
    let handle = setup().await;
    apply_ddl(&handle, NAVIGATION_SCHEMA).await;
    reconcile_named_view(&handle, "current_focus", CURRENT_FOCUS_SELECT)
        .await
        .expect("create current_focus matview");

    handle
        .execute(
            "INSERT INTO navigation_history (id, region, block_id) VALUES (1, 'main', \
             'block:alpha')",
            vec![],
        )
        .await
        .expect("seed history");

    let upsert = holon_crate_sql("sql/navigation/upsert_cursor.sql");
    let params = |id: i64| {
        HashMap::from([
            ("region".to_string(), Value::from("main")),
            ("new_id".to_string(), Value::Integer(id)),
        ])
    };

    // First focus: the cursor row exists already (init_default_region seeds one
    // per region with a NULL history_id), so even this is a replace — but of a
    // CHANGED value, which the peer measured as safe.
    handle
        .execute(
            "INSERT OR IGNORE INTO navigation_cursor (region, history_id) VALUES ('main', NULL)",
            vec![],
        )
        .await
        .expect("seed default region cursor");
    handle.query(&upsert, params(1)).await.expect("first focus");
    assert_matview_equals_recompute(
        &handle,
        "current_focus",
        CURRENT_FOCUS_SELECT,
        &["region", "block_id"],
        "the first focus",
    )
    .await;

    // Re-focus the SAME target: same region, same history_id. One replace with
    // an unchanged value is enough to drive the row's weight to -1.
    handle
        .query(&upsert, params(1))
        .await
        .expect("re-focus the same target");
    assert_matview_equals_recompute(
        &handle,
        "current_focus",
        CURRENT_FOCUS_SELECT,
        &["region", "block_id"],
        "re-focusing the already-focused target (one REPLACE with an unchanged value)",
    )
    .await;
}

// ---------------------------------------------------------------------------
// The three view shapes holon ships, on a minimal base table
// ---------------------------------------------------------------------------

/// Base table plus one matview of the given shape; returns the defining SELECT.
async fn shape_setup(handle: &DbHandle, view: &str, select_sql: &str) {
    handle
        .execute_ddl(
            "CREATE TABLE cache_row (id TEXT PRIMARY KEY, grp TEXT NOT NULL, val TEXT NOT NULL)",
        )
        .await
        .expect("create base table");
    handle
        .execute(
            "INSERT INTO cache_row (id, grp, val) VALUES ('r1', 'g1', 'v1')",
            vec![],
        )
        .await
        .expect("seed row");
    reconcile_named_view(handle, view, select_sql)
        .await
        .expect("create matview");
}

/// Projection+filter — the `gcal_upcoming_flagged` / `upcoming` shape.
#[tokio::test]
async fn replace_with_unchanged_value_keeps_a_filter_matview_correct() {
    let handle = setup().await;
    let select = "SELECT id, grp, val FROM cache_row WHERE grp = 'g1'";
    shape_setup(&handle, "cache_filtered", select).await;

    handle
        .execute(
            "INSERT OR REPLACE INTO cache_row (id, grp, val) VALUES ('r1', 'g1', 'v1')",
            vec![],
        )
        .await
        .expect("replace with unchanged value");

    assert_matview_equals_recompute(
        &handle,
        "cache_filtered",
        select,
        &["id", "grp", "val"],
        "one REPLACE with an unchanged value",
    )
    .await;
}

/// Aggregate — the `gmail_unread_by_thread` / `session_last_message` shape.
#[tokio::test]
async fn replace_with_unchanged_value_keeps_an_aggregate_matview_correct() {
    let handle = setup().await;
    let select = "SELECT grp, count(*) AS n FROM cache_row GROUP BY grp";
    shape_setup(&handle, "cache_counts", select).await;

    handle
        .execute(
            "INSERT OR REPLACE INTO cache_row (id, grp, val) VALUES ('r1', 'g1', 'v1')",
            vec![],
        )
        .await
        .expect("replace with unchanged value");

    assert_matview_equals_recompute(
        &handle,
        "cache_counts",
        select,
        &["grp", "n"],
        "one REPLACE with an unchanged value",
    )
    .await;
}

/// The control the peer reported as safe: replacing with a DIFFERENT value.
/// If THIS goes red too, the defect is wider than "unchanged value" and the
/// fix must be chosen against the wider defect.
#[tokio::test]
async fn replace_with_a_changed_value_keeps_a_filter_matview_correct() {
    let handle = setup().await;
    let select = "SELECT id, grp, val FROM cache_row WHERE grp = 'g1'";
    shape_setup(&handle, "cache_filtered", select).await;

    handle
        .execute(
            "INSERT OR REPLACE INTO cache_row (id, grp, val) VALUES ('r1', 'g1', 'v2')",
            vec![],
        )
        .await
        .expect("replace with changed value");

    assert_matview_equals_recompute(
        &handle,
        "cache_filtered",
        select,
        &["id", "grp", "val"],
        "one REPLACE with a CHANGED value (expected safe)",
    )
    .await;
}

/// A weight driven to -1 does not have to show up in the NEXT read: the row can
/// still project correctly until a later delta over the same key resolves the
/// arithmetic. So replace, then keep mutating the table, and check the oracle
/// after every step — this is the strongest form of the measurement.
#[tokio::test]
async fn replace_with_unchanged_value_survives_the_following_deltas() {
    let handle = setup().await;
    let select = "SELECT id, grp, val FROM cache_row WHERE grp = 'g1'";
    shape_setup(&handle, "cache_filtered", select).await;
    let cols = ["id", "grp", "val"];

    handle
        .execute(
            "INSERT OR REPLACE INTO cache_row (id, grp, val) VALUES ('r1', 'g1', 'v1')",
            vec![],
        )
        .await
        .expect("replace with unchanged value");
    assert_matview_equals_recompute(&handle, "cache_filtered", select, &cols, "the replace").await;

    // A second unchanged replace: two replaces, four retractions, two insertions.
    handle
        .execute(
            "INSERT OR REPLACE INTO cache_row (id, grp, val) VALUES ('r1', 'g1', 'v1')",
            vec![],
        )
        .await
        .expect("second replace with unchanged value");
    assert_matview_equals_recompute(&handle, "cache_filtered", select, &cols, "a second replace")
        .await;

    // An unrelated insert forces a delta through the view.
    handle
        .execute(
            "INSERT INTO cache_row (id, grp, val) VALUES ('r2', 'g1', 'v9')",
            vec![],
        )
        .await
        .expect("insert a sibling row");
    assert_matview_equals_recompute(
        &handle,
        "cache_filtered",
        select,
        &cols,
        "an insert following the replaces",
    )
    .await;

    // Updating the replaced row is the delta most likely to resolve a bad weight.
    handle
        .execute("UPDATE cache_row SET val = 'v2' WHERE id = 'r1'", vec![])
        .await
        .expect("update the replaced row");
    assert_matview_equals_recompute(
        &handle,
        "cache_filtered",
        select,
        &cols,
        "an update of the replaced row",
    )
    .await;

    // Deleting it must retract exactly once.
    handle
        .execute("DELETE FROM cache_row WHERE id = 'r1'", vec![])
        .await
        .expect("delete the replaced row");
    assert_matview_equals_recompute(
        &handle,
        "cache_filtered",
        select,
        &cols,
        "the delete of the replaced row",
    )
    .await;
}

/// The bare `REPLACE INTO` keyword form, which is a different parse than
/// `INSERT OR REPLACE` even though SQLite treats them alike.
#[tokio::test]
async fn bare_replace_into_with_unchanged_value_keeps_a_filter_matview_correct() {
    let handle = setup().await;
    let select = "SELECT id, grp, val FROM cache_row WHERE grp = 'g1'";
    shape_setup(&handle, "cache_filtered", select).await;

    handle
        .execute(
            "REPLACE INTO cache_row (id, grp, val) VALUES ('r1', 'g1', 'v1')",
            vec![],
        )
        .await
        .expect("bare REPLACE INTO with unchanged value");

    assert_matview_equals_recompute(
        &handle,
        "cache_filtered",
        select,
        &["id", "grp", "val"],
        "a bare REPLACE INTO with an unchanged value",
    )
    .await;
}

/// An INTEGER PRIMARY KEY table is a rowid table, and the seek the REPLACE path
/// performs differs from the TEXT-key case.
///
/// THE ONE RED IN THIS FILE, and the only shape that reproduces the fork's
/// double-capture defect: the row's weight falls to -1 and it VANISHES from the
/// matview. Every other combination measured here — TEXT or composite-TEXT key,
/// any view shape, and a rowid table replaced with a CHANGED value — is green.
///
/// No holon production table matches this shape today (see the sibling tests
/// exercising the production navigation and block_links statements), so this is
/// a tripwire for the day one does, not a reproduction of a live corruption.
/// Ignored because it is a KNOWN engine defect: un-ignore it to check whether
/// the turso fork has been fixed.
#[tokio::test]
async fn replace_with_unchanged_value_keeps_a_rowid_table_matview_correct() {
    let handle = setup().await;
    handle
        .execute_ddl("CREATE TABLE rowid_row (id INTEGER PRIMARY KEY, grp TEXT NOT NULL)")
        .await
        .expect("create rowid base table");
    handle
        .execute("INSERT INTO rowid_row (id, grp) VALUES (1, 'g1')", vec![])
        .await
        .expect("seed row");
    let select = "SELECT id, grp FROM rowid_row WHERE grp = 'g1'";
    reconcile_named_view(&handle, "rowid_filtered", select)
        .await
        .expect("create matview");

    handle
        .execute_unguarded(
            "INSERT OR REPLACE INTO rowid_row (id, grp) VALUES (1, 'g1')",
            vec![],
        )
        .await
        .expect("replace with unchanged value");

    assert_matview_equals_recompute(
        &handle,
        "rowid_filtered",
        select,
        &["id", "grp"],
        "a REPLACE with an unchanged value on a rowid table",
    )
    .await;
}

/// The JOIN shape on synthetic tables, isolating it from the production DDL so
/// a failure here cannot be blamed on the navigation schema.
#[tokio::test]
async fn replace_with_unchanged_value_keeps_a_join_matview_correct() {
    let handle = setup().await;
    handle
        .execute_ddl("CREATE TABLE j_left (k TEXT PRIMARY KEY, fk INTEGER)")
        .await
        .expect("create left");
    handle
        .execute_ddl("CREATE TABLE j_right (id INTEGER PRIMARY KEY, label TEXT)")
        .await
        .expect("create right");
    handle
        .execute(
            "INSERT INTO j_right (id, label) VALUES (1, 'alpha')",
            vec![],
        )
        .await
        .expect("seed right");
    handle
        .execute("INSERT INTO j_left (k, fk) VALUES ('main', 1)", vec![])
        .await
        .expect("seed left");
    let select = "SELECT l.k, r.label FROM j_left l JOIN j_right r ON l.fk = r.id";
    reconcile_named_view(&handle, "j_view", select)
        .await
        .expect("create join matview");

    handle
        .execute(
            "INSERT OR REPLACE INTO j_left (k, fk) VALUES ('main', 1)",
            vec![],
        )
        .await
        .expect("replace left with unchanged value");

    assert_matview_equals_recompute(
        &handle,
        "j_view",
        select,
        &["k", "label"],
        "a REPLACE with an unchanged value on the JOIN's left side",
    )
    .await;
}

/// A rowid-alias base, PROJECTION view, replaced with CHANGED values: green.
///
/// Named for exactly what it measures. It does NOT license the wider claim that
/// "a changed value is always safe on a rowid table" — an AGGREGATE view over
/// the same base diverges once a value REVISITS a previously-seen group, which
/// `replace_with_changed_values_revisiting_a_group_keeps_an_aggregate_correct`
/// pins as a separate, still-failing witness.
#[tokio::test]
async fn replace_with_changed_values_keeps_a_rowid_projection_matview_correct() {
    let handle = setup().await;
    handle
        .execute_ddl("CREATE TABLE rowid_row (id INTEGER PRIMARY KEY, grp TEXT NOT NULL)")
        .await
        .expect("create rowid base table");
    handle
        .execute("INSERT INTO rowid_row (id, grp) VALUES (1, 'g1')", vec![])
        .await
        .expect("seed row");
    let select = "SELECT id, grp FROM rowid_row";
    reconcile_named_view(&handle, "rowid_all", select)
        .await
        .expect("create matview");

    handle
        .execute_unguarded(
            "INSERT OR REPLACE INTO rowid_row (id, grp) VALUES (1, 'g2')",
            vec![],
        )
        .await
        .expect("replace with changed value");

    assert_matview_equals_recompute(
        &handle,
        "rowid_all",
        select,
        &["id", "grp"],
        "a REPLACE with a CHANGED value on a rowid table",
    )
    .await;
}

// ---------------------------------------------------------------------------
// (c) block_links -> backlinks, via the PRODUCTION DDL and write sequence
// ---------------------------------------------------------------------------

/// `link_statements` (crates/holon/src/core/sql_operation_provider.rs) emits a
/// `DELETE FROM block_links WHERE source_block_id = ...` and THEN one
/// `INSERT OR REPLACE` per derived link. Two things make this the interesting
/// case: the delete means the replace usually has nothing to conflict with, and
/// when a block carries the SAME link twice the second replace DOES conflict —
/// with a row inserted moments earlier in the same batch.
///
/// `block_links`' primary key is the composite `(source_block_id, target,
/// kind)`, all TEXT, so it is not a rowid-alias table.
#[tokio::test]
async fn production_link_write_sequence_keeps_a_backlinks_matview_correct() {
    let handle = setup().await;
    apply_ddl(&handle, BLOCK_LINKS_SCHEMA).await;
    // The backlinks matview joins block_links to the block table; the join's
    // block side is not what a REPLACE touches, so a minimal stand-in for
    // `block_raw` keeps the shape without dragging in the block schema.
    handle
        .execute_ddl("CREATE TABLE block_raw (id TEXT PRIMARY KEY, content TEXT)")
        .await
        .expect("create block stand-in");
    handle
        .execute(
            "INSERT INTO block_raw (id, content) VALUES ('block:src', 'Source'), ('block:tgt', \
             'Target')",
            vec![],
        )
        .await
        .expect("seed blocks");
    let select = "SELECT bl.resolved_id AS target_id, b.id AS id, b.content AS content FROM \
                  block_links bl JOIN block_raw b ON b.id = bl.source_block_id WHERE \
                  bl.resolved_id IS NOT NULL";
    reconcile_named_view(&handle, "backlinks", select)
        .await
        .expect("create backlinks matview");
    let cols = ["target_id", "id", "content"];

    // First save of the block: delete-then-insert, nothing to conflict with.
    handle
        .transaction(vec![
            (
                "DELETE FROM block_links WHERE source_block_id = 'block:src'".to_string(),
                vec![],
            ),
            (
                "INSERT OR REPLACE INTO block_links (source_block_id, target, kind, resolved_id) \
                 VALUES ('block:src', 'Target', 'page', 'block:tgt')"
                    .to_string(),
                vec![],
            ),
        ])
        .await
        .expect("first link write");
    assert_matview_equals_recompute(&handle, "backlinks", select, &cols, "the first link write")
        .await;

    // Re-saving the block with the link UNCHANGED: the production sequence
    // again, writing an identical row.
    handle
        .transaction(vec![
            (
                "DELETE FROM block_links WHERE source_block_id = 'block:src'".to_string(),
                vec![],
            ),
            (
                "INSERT OR REPLACE INTO block_links (source_block_id, target, kind, resolved_id) \
                 VALUES ('block:src', 'Target', 'page', 'block:tgt')"
                    .to_string(),
                vec![],
            ),
        ])
        .await
        .expect("re-save with an unchanged link");
    assert_matview_equals_recompute(
        &handle,
        "backlinks",
        select,
        &cols,
        "re-saving the block with the link unchanged",
    )
    .await;

    // The one sequence where the REPLACE genuinely conflicts: a block whose
    // content names the same target twice yields two identical junction rows,
    // so the second statement replaces the first.
    handle
        .transaction(vec![
            (
                "DELETE FROM block_links WHERE source_block_id = 'block:src'".to_string(),
                vec![],
            ),
            (
                "INSERT OR REPLACE INTO block_links (source_block_id, target, kind, resolved_id) \
                 VALUES ('block:src', 'Target', 'page', 'block:tgt')"
                    .to_string(),
                vec![],
            ),
            (
                "INSERT OR REPLACE INTO block_links (source_block_id, target, kind, resolved_id) \
                 VALUES ('block:src', 'Target', 'page', 'block:tgt')"
                    .to_string(),
                vec![],
            ),
        ])
        .await
        .expect("write a duplicated link");
    assert_matview_equals_recompute(
        &handle,
        "backlinks",
        select,
        &cols,
        "a block naming the same target twice (a REPLACE that really conflicts)",
    )
    .await;
}

// ---------------------------------------------------------------------------
// P3: the candidate matrix. These pick the fix.
// ---------------------------------------------------------------------------

/// Candidate (i): `INSERT ... ON CONFLICT DO UPDATE`, unchanged value.
#[tokio::test]
async fn on_conflict_do_update_with_unchanged_value_keeps_a_filter_matview_correct() {
    let handle = setup().await;
    let select = "SELECT id, grp, val FROM cache_row WHERE grp = 'g1'";
    shape_setup(&handle, "cache_filtered", select).await;

    handle
        .execute(
            "INSERT INTO cache_row (id, grp, val) VALUES ('r1', 'g1', 'v1') ON CONFLICT(id) DO \
             UPDATE SET grp = excluded.grp, val = excluded.val",
            vec![],
        )
        .await
        .expect("on-conflict upsert, unchanged value");

    assert_matview_equals_recompute(
        &handle,
        "cache_filtered",
        select,
        &["id", "grp", "val"],
        "ON CONFLICT DO UPDATE with an unchanged value",
    )
    .await;
}

/// Candidate (i): `INSERT ... ON CONFLICT DO UPDATE`, changed value.
#[tokio::test]
async fn on_conflict_do_update_with_a_changed_value_keeps_a_filter_matview_correct() {
    let handle = setup().await;
    let select = "SELECT id, grp, val FROM cache_row WHERE grp = 'g1'";
    shape_setup(&handle, "cache_filtered", select).await;

    handle
        .execute(
            "INSERT INTO cache_row (id, grp, val) VALUES ('r1', 'g1', 'v2') ON CONFLICT(id) DO \
             UPDATE SET grp = excluded.grp, val = excluded.val",
            vec![],
        )
        .await
        .expect("on-conflict upsert, changed value");

    assert_matview_equals_recompute(
        &handle,
        "cache_filtered",
        select,
        &["id", "grp", "val"],
        "ON CONFLICT DO UPDATE with a changed value",
    )
    .await;
}

/// Candidate (i) against the aggregate shape, unchanged value.
#[tokio::test]
async fn on_conflict_do_update_with_unchanged_value_keeps_an_aggregate_matview_correct() {
    let handle = setup().await;
    let select = "SELECT grp, count(*) AS n FROM cache_row GROUP BY grp";
    shape_setup(&handle, "cache_counts", select).await;

    handle
        .execute(
            "INSERT INTO cache_row (id, grp, val) VALUES ('r1', 'g1', 'v1') ON CONFLICT(id) DO \
             UPDATE SET grp = excluded.grp, val = excluded.val",
            vec![],
        )
        .await
        .expect("on-conflict upsert, unchanged value");

    assert_matview_equals_recompute(
        &handle,
        "cache_counts",
        select,
        &["grp", "n"],
        "ON CONFLICT DO UPDATE with an unchanged value",
    )
    .await;
}

/// Candidate (ii): DELETE + INSERT inside ONE transaction, unchanged value.
#[tokio::test]
async fn delete_then_insert_in_one_transaction_keeps_a_filter_matview_correct() {
    let handle = setup().await;
    let select = "SELECT id, grp, val FROM cache_row WHERE grp = 'g1'";
    shape_setup(&handle, "cache_filtered", select).await;

    handle
        .transaction(vec![
            ("DELETE FROM cache_row WHERE id = 'r1'".to_string(), vec![]),
            (
                "INSERT INTO cache_row (id, grp, val) VALUES ('r1', 'g1', 'v1')".to_string(),
                vec![],
            ),
        ])
        .await
        .expect("delete+insert in one transaction");

    assert_matview_equals_recompute(
        &handle,
        "cache_filtered",
        select,
        &["id", "grp", "val"],
        "DELETE + INSERT in one transaction with an unchanged value",
    )
    .await;
}

// ---------------------------------------------------------------------------
// The turso-1d peer's minimal repros, and the cells its matrix did not cover.
//
// Every one of these uses `INTEGER PRIMARY KEY`, which the measurements above
// identify as half the trigger. They are the A/B witness for the engine fix
// (fork bookmark `ivm-replace-double-old-row-capture`): RED on the current pin,
// expected GREEN after the bump.
// ---------------------------------------------------------------------------

/// Seed a rowid table with one row and a matview of the given shape, then
/// REPLACE that row with an UNCHANGED value and return what the matview reads
/// back — `Err` when the read itself fails, which is the signature the peer
/// reported (`expected a positive weight, found -1`).
async fn rowid_replace_probe(
    view: &str,
    select_sql: &str,
    read_sql: &str,
) -> Result<String, String> {
    let handle = setup().await;
    handle
        .execute_ddl("CREATE TABLE t7 (id INTEGER PRIMARY KEY, val TEXT)")
        .await
        .expect("create rowid table");
    reconcile_named_view(&handle, view, select_sql)
        .await
        .map_err(|e| format!("CREATE MATERIALIZED VIEW failed: {e}"))?;
    handle
        .execute("INSERT INTO t7 (id, val) VALUES (1, 'a')", vec![])
        .await
        .expect("seed row");
    handle
        .execute_unguarded(
            "INSERT OR REPLACE INTO t7 (id, val) VALUES (1, 'a')",
            vec![],
        )
        .await
        .map_err(|e| format!("the REPLACE itself failed: {e}"))?;
    match handle.query(read_sql, HashMap::new()).await {
        Ok(rows) => Ok(format!("{} row(s)", rows.len())),
        Err(e) => Err(format!("reading the matview failed: {e}")),
    }
}

/// The peer's projection/filter repro, verbatim in shape.
#[tokio::test]
async fn peer_repro_projection_filter_over_a_rowid_table() {
    let outcome = rowid_replace_probe(
        "v7",
        "SELECT id, val FROM t7 WHERE val = 'a'",
        "SELECT id, val FROM v7 ORDER BY id",
    )
    .await;
    assert_eq!(
        outcome,
        Ok("1 row(s)".to_string()),
        "projection/filter matview over a rowid table after one same-value REPLACE"
    );
}

/// The peer's aggregate twin.
#[tokio::test]
async fn peer_repro_aggregate_over_a_rowid_table() {
    let outcome = rowid_replace_probe(
        "m7",
        "SELECT val, COUNT(*) AS c FROM t7 GROUP BY val",
        "SELECT val, c FROM m7",
    )
    .await;
    assert_eq!(
        outcome,
        Ok("1 row(s)".to_string()),
        "aggregate matview over a rowid table after one same-value REPLACE"
    );
}

/// THE CELL THE PEER HAS NOT VERIFIED: an INNER JOIN whose replaced side is a
/// rowid table. `current_focus` and `backlinks` are both inner joins, so
/// whether the defect reaches joins at all decides how much of holon could ever
/// be affected — independent of today's key types.
#[tokio::test]
async fn peer_gap_inner_join_over_a_rowid_table() {
    let handle = setup().await;
    handle
        .execute_ddl("CREATE TABLE t7 (id INTEGER PRIMARY KEY, val TEXT)")
        .await
        .expect("create rowid table");
    handle
        .execute_ddl("CREATE TABLE side (val TEXT PRIMARY KEY, label TEXT)")
        .await
        .expect("create side table");
    handle
        .execute(
            "INSERT INTO side (val, label) VALUES ('a', 'alpha')",
            vec![],
        )
        .await
        .expect("seed side");
    handle
        .execute("INSERT INTO t7 (id, val) VALUES (1, 'a')", vec![])
        .await
        .expect("seed rowid row");
    let select = "SELECT t.id, s.label FROM t7 t JOIN side s ON t.val = s.val";
    reconcile_named_view(&handle, "j7", select)
        .await
        .expect("create join matview");

    handle
        .execute_unguarded(
            "INSERT OR REPLACE INTO t7 (id, val) VALUES (1, 'a')",
            vec![],
        )
        .await
        .expect("same-value replace on the join's rowid side");

    assert_matview_equals_recompute(
        &handle,
        "j7",
        select,
        &["id", "label"],
        "a same-value REPLACE into the INNER JOIN's rowid side",
    )
    .await;
}

/// LEFT JOIN over a rowid table — the other shape neither the peer nor holon
/// has measured. The `block` matview is holon's only LEFT JOIN matview; its
/// base `block_raw` is TEXT-keyed (crates/holon-turso/sql/schema/blocks.sql:6),
/// so this probes the engine dimension rather than a live holon exposure.
#[tokio::test]
async fn peer_gap_left_join_over_a_rowid_table() {
    let handle = setup().await;
    handle
        .execute_ddl("CREATE TABLE t7 (id INTEGER PRIMARY KEY, val TEXT)")
        .await
        .expect("create rowid table");
    handle
        .execute_ddl("CREATE TABLE side (val TEXT PRIMARY KEY, label TEXT)")
        .await
        .expect("create side table");
    handle
        .execute("INSERT INTO t7 (id, val) VALUES (1, 'a')", vec![])
        .await
        .expect("seed rowid row");
    let select = "SELECT t.id, s.label FROM t7 t LEFT JOIN side s ON t.val = s.val";
    reconcile_named_view(&handle, "lj7", select)
        .await
        .expect("create left-join matview");

    handle
        .execute_unguarded(
            "INSERT OR REPLACE INTO t7 (id, val) VALUES (1, 'a')",
            vec![],
        )
        .await
        .expect("same-value replace on the left join's rowid side");

    assert_matview_equals_recompute(
        &handle,
        "lj7",
        select,
        &["id", "label"],
        "a same-value REPLACE into the LEFT JOIN's rowid side",
    )
    .await;
}

/// The LEFT JOIN control main asked for, on the PRODUCTION key type: the
/// `block` matview's shape over a TEXT-keyed `block_raw`.
#[tokio::test]
async fn same_value_replace_into_a_text_keyed_left_join_base_is_correct() {
    let handle = setup().await;
    handle
        .execute_ddl("CREATE TABLE block_raw (id TEXT PRIMARY KEY, parent_id TEXT, content TEXT)")
        .await
        .expect("create block_raw stand-in");
    handle
        .execute_ddl(
            "CREATE TABLE block_tags (block_id TEXT, tag TEXT, PRIMARY KEY (block_id, tag))",
        )
        .await
        .expect("create edge table");
    handle
        .execute(
            "INSERT INTO block_raw (id, parent_id, content) VALUES ('block:a', NULL, 'A')",
            vec![],
        )
        .await
        .expect("seed block");
    let select = "SELECT b.id, b.content, t.tag FROM block_raw b LEFT JOIN block_tags t ON t.block_id = b.id";
    reconcile_named_view(&handle, "block_view", select)
        .await
        .expect("create left-join matview");

    handle
        .execute(
            "INSERT OR REPLACE INTO block_raw (id, parent_id, content) VALUES ('block:a', NULL, \
             'A')",
            vec![],
        )
        .await
        .expect("same-value replace into the LEFT JOIN base");

    assert_matview_equals_recompute(
        &handle,
        "block_view",
        select,
        &["id", "content", "tag"],
        "a same-value REPLACE into a TEXT-keyed LEFT JOIN base",
    )
    .await;
}

/// The cell that decides whether the cache SYNC path is exposed.
///
/// `QueryableCache` upserts with `INSERT ... ON CONFLICT DO UPDATE`
/// (crates/holon/src/core/queryable_cache.rs:243,789), not with REPLACE — and
/// the shipped `jsonplaceholder` sidecar declares `id INTEGER PRIMARY KEY`, so
/// its cache table IS a rowid-alias table. ON CONFLICT is measured green on
/// TEXT keys above; this pins it on the shape that actually breaks REPLACE.
#[tokio::test]
async fn on_conflict_do_update_with_unchanged_value_keeps_a_rowid_table_matview_correct() {
    let handle = setup().await;
    handle
        .execute_ddl("CREATE TABLE t7 (id INTEGER PRIMARY KEY, val TEXT)")
        .await
        .expect("create rowid table");
    handle
        .execute("INSERT INTO t7 (id, val) VALUES (1, 'a')", vec![])
        .await
        .expect("seed row");
    let select = "SELECT id, val FROM t7 WHERE val = 'a'";
    reconcile_named_view(&handle, "oc7", select)
        .await
        .expect("create matview");

    handle
        .execute(
            "INSERT INTO t7 (id, val) VALUES (1, 'a') ON CONFLICT(id) DO UPDATE SET val = \
             excluded.val",
            vec![],
        )
        .await
        .expect("on-conflict upsert, unchanged value, rowid table");

    assert_matview_equals_recompute(
        &handle,
        "oc7",
        select,
        &["id", "val"],
        "ON CONFLICT DO UPDATE with an unchanged value on a ROWID table",
    )
    .await;
}

// ---------------------------------------------------------------------------
// The DbHandle guard: the conjunction is refused at BOTH write entry points.
// ---------------------------------------------------------------------------

/// Base table + matview, ready for a REPLACE attempt. `columns` is the FULL
/// column list so a caller can spell the key either way — a table constraint
/// must come after every column, so it cannot be appended to a key fragment.
async fn guarded_setup(columns: &str) -> DbHandle {
    let handle = setup().await;
    handle
        .execute_ddl(&format!("CREATE TABLE guarded ({columns})"))
        .await
        .expect("create base table");
    reconcile_named_view(&handle, "guarded_view", "SELECT id, val FROM guarded")
        .await
        .expect("create matview");
    handle
}

#[tokio::test]
async fn a_replace_into_a_rowid_alias_matview_base_is_rejected_at_every_entry_point() {
    let handle = guarded_setup("id INTEGER PRIMARY KEY, val TEXT").await;

    let via_execute = handle
        .execute(
            "INSERT OR REPLACE INTO guarded (id, val) VALUES (1, 'a')",
            vec![],
        )
        .await
        .expect_err("execute must refuse a REPLACE into a rowid-alias matview base");
    let via_transaction = handle
        .transaction(vec![(
            "REPLACE INTO guarded (id, val) VALUES (1, 'a')".to_string(),
            vec![],
        )])
        .await
        .expect_err("transaction must refuse it too — the batch writers must not route around it");
    // `query` is write-capable, and EVERY production REPLACE in the tree is
    // written through it (the four navigation call sites). A guard that misses
    // this entry point is inert against exactly what it exists to stop.
    let via_query = handle
        .query(
            "INSERT OR REPLACE INTO guarded (id, val) VALUES (1, 'a')",
            HashMap::new(),
        )
        .await
        .expect_err("query() must refuse it — this is the path production uses");
    let via_query_positional = handle
        .query_positional(
            "INSERT OR REPLACE INTO guarded (id, val) VALUES (1, 'a')",
            vec![],
        )
        .await
        .expect_err("query_positional() must refuse it too");

    for (label, err) in [
        ("execute", via_execute.to_string()),
        ("transaction", via_transaction.to_string()),
        ("query", via_query.to_string()),
        ("query_positional", via_query_positional.to_string()),
    ] {
        assert!(
            err.contains("guarded") && err.contains("guarded_view"),
            "[{label}] the error must name the table AND the dependent view; got: {err}"
        );
        assert!(
            err.contains("rowid-alias") && err.contains("UNCHANGED"),
            "[{label}] the error must state the conjunction that triggers it; got: {err}"
        );
        assert!(
            err.contains("PR #8463"),
            "[{label}] the error must point at the engine fix; got: {err}"
        );
        assert!(
            err.contains("ON CONFLICT"),
            "[{label}] the error must name the safe alternative; got: {err}"
        );
    }
}

/// The guard must not become a false-positive machine. Each of these differs
/// from the rejected case in exactly ONE way, and each must be allowed —
/// together they pin every conjunct as load-bearing.
#[tokio::test]
async fn the_guard_spares_everything_that_is_not_the_conjunction() {
    // (1) Same REPLACE, but the key is TEXT — the shape holon actually ships.
    let handle = guarded_setup("id TEXT PRIMARY KEY, val TEXT").await;
    handle
        .execute(
            "INSERT OR REPLACE INTO guarded (id, val) VALUES ('a', 'a')",
            vec![],
        )
        .await
        .expect("a REPLACE into a TEXT-keyed matview base is measurably safe and must be allowed");

    // (2) Rowid-alias table with NO matview over it: nothing to corrupt.
    let handle = setup().await;
    handle
        .execute_ddl("CREATE TABLE unviewed (id INTEGER PRIMARY KEY, val TEXT)")
        .await
        .expect("create base table");
    handle
        .execute(
            "INSERT OR REPLACE INTO unviewed (id, val) VALUES (1, 'a')",
            vec![],
        )
        .await
        .expect("a REPLACE into a rowid table with no matview must be allowed");

    // (3) The rowid-alias matview base, written with the SAFE upsert form.
    let handle = guarded_setup("id INTEGER PRIMARY KEY, val TEXT").await;
    handle
        .execute(
            "INSERT INTO guarded (id, val) VALUES (1, 'a') ON CONFLICT(id) DO UPDATE SET val = \
             excluded.val",
            vec![],
        )
        .await
        .expect("ON CONFLICT DO UPDATE is the sanctioned form and must pass the guard");
    handle
        .execute(
            "INSERT OR IGNORE INTO guarded (id, val) VALUES (2, 'b')",
            vec![],
        )
        .await
        .expect("INSERT OR IGNORE never deletes the old row and must pass the guard");
    handle
        .execute("UPDATE guarded SET val = 'c' WHERE id = 1", vec![])
        .await
        .expect("a plain UPDATE must pass the guard");
}

/// The forms the first version of this guard silently let through. Each drove a
/// real matview to silent corruption; each must now be refused.
#[tokio::test]
async fn the_guard_is_not_fooled_by_comments_qualifiers_or_the_table_constraint_key() {
    // A leading comment block: `sql_tokens` used to read the first comment word
    // as the verb. A PRODUCTION file opens with five such lines.
    let handle = guarded_setup("id INTEGER PRIMARY KEY, val TEXT").await;
    handle
        .query(
            "-- point the cursor at the row\n-- (second comment line)\nINSERT OR REPLACE INTO \
             guarded (id, val) VALUES (1, 'a')",
            HashMap::new(),
        )
        .await
        .expect_err("a leading comment block must not hide the REPLACE");

    // Schema-qualified target: the token after INTO used to be read as the
    // table, yielding the SCHEMA name, which matches nothing.
    let handle = guarded_setup("id INTEGER PRIMARY KEY, val TEXT").await;
    handle
        .query(
            "INSERT OR REPLACE INTO main.guarded (id, val) VALUES (1, 'a')",
            HashMap::new(),
        )
        .await
        .expect_err("a schema-qualified target must not hide the table");

    // Single-column TABLE-CONSTRAINT key. This IS a rowid alias, and reading the
    // DDL string for the phrase "INTEGER PRIMARY KEY" misses it entirely —
    // under-approximation, the unsafe direction.
    let handle = guarded_setup("id INTEGER, val TEXT, PRIMARY KEY (id)").await;
    handle
        .query(
            "INSERT OR REPLACE INTO guarded (id, val) VALUES (1, 'a')",
            HashMap::new(),
        )
        .await
        .expect_err("the table-constraint spelling of a rowid alias must be caught");
}

/// The table-constraint spelling really is a rowid alias on this engine — the
/// premise the guard's new detection rests on. Without this, the test above
/// could be asserting a rejection the engine never needed.
#[tokio::test]
async fn table_constraint_rowid_alias_corrupts_like_the_column_constraint_form() {
    let handle = setup().await;
    handle
        .execute_ddl("CREATE TABLE tc (id INTEGER, val TEXT, PRIMARY KEY (id))")
        .await
        .expect("create table-constraint rowid table");
    handle
        .execute("INSERT INTO tc (id, val) VALUES (1, 'a')", vec![])
        .await
        .expect("seed row");
    let select = "SELECT id, val FROM tc WHERE val = 'a'";
    reconcile_named_view(&handle, "tc_view", select)
        .await
        .expect("create matview");

    handle
        .execute_unguarded(
            "INSERT OR REPLACE INTO tc (id, val) VALUES (1, 'a')",
            vec![],
        )
        .await
        .expect("same-value replace");

    assert_matview_equals_recompute(
        &handle,
        "tc_view",
        select,
        &["id", "val"],
        "a same-value REPLACE into a table-constraint rowid alias",
    )
    .await;
}

/// Tripwire #6 — the cell that shows the unchanged-value condition is NOT
/// necessary.
///
/// On a rowid-alias base under an AGGREGATE view, CHANGED values corrupt the
/// view as soon as one of them revisits a group that existed before: the
/// sequence a→b→a→b→a is correct through the first change and diverges from the
/// second onward. Controls locating the trigger, all green and all measured:
/// the same sequence on a TEXT key, all-distinct values on a rowid key, a
/// projection view on a rowid key, and both ON CONFLICT and plain UPDATE on a
/// rowid key.
///
/// This exists so the other five witnesses cannot all go green while this
/// survives — which is exactly what would happen if the engine fix addressed
/// only the unchanged-value path.
#[tokio::test]
async fn replace_with_changed_values_revisiting_a_group_keeps_an_aggregate_correct() {
    let handle = setup().await;
    handle
        .execute_ddl("CREATE TABLE grp (id INTEGER PRIMARY KEY, val TEXT)")
        .await
        .expect("create rowid base");
    handle
        .execute("INSERT INTO grp (id, val) VALUES (1, 'a')", vec![])
        .await
        .expect("seed row");
    let select = "SELECT val, COUNT(*) AS c FROM grp GROUP BY val";
    reconcile_named_view(&handle, "grp_counts", select)
        .await
        .expect("create aggregate matview");

    // Every value here DIFFERS from the row's current value, so the
    // unchanged-value condition never holds — yet the view still diverges.
    for next in ["b", "a", "b", "a"] {
        handle
            .execute_unguarded(
                &format!("INSERT OR REPLACE INTO grp (id, val) VALUES (1, '{next}')"),
                vec![],
            )
            .await
            .expect("replace with a changed value");
        assert_matview_equals_recompute(
            &handle,
            "grp_counts",
            select,
            &["val", "c"],
            &format!("a CHANGED-value REPLACE moving the row to group '{next}'"),
        )
        .await;
    }
}

/// The controls that locate tripwire #6's trigger. All green — so the trigger
/// is the revisited group on a rowid-alias key under an aggregate, and NOT
/// simply "changed values corrupt things".
#[tokio::test]
async fn changed_value_controls_locate_the_revisited_group_trigger() {
    // TEXT key, same revisiting sequence: green. Key shape still decides, so
    // production exposure is not widened by tripwire #6.
    let handle = setup().await;
    handle
        .execute_ddl("CREATE TABLE gt (id TEXT PRIMARY KEY, val TEXT)")
        .await
        .expect("create text-key base");
    handle
        .execute("INSERT INTO gt (id, val) VALUES ('k', 'a')", vec![])
        .await
        .expect("seed");
    let select_t = "SELECT val, COUNT(*) AS c FROM gt GROUP BY val";
    reconcile_named_view(&handle, "gt_counts", select_t)
        .await
        .expect("matview");
    for next in ["b", "a", "b", "a"] {
        handle
            .execute(
                &format!("INSERT OR REPLACE INTO gt (id, val) VALUES ('k', '{next}')"),
                vec![],
            )
            .await
            .expect("replace on a TEXT key");
        assert_matview_equals_recompute(
            &handle,
            "gt_counts",
            select_t,
            &["val", "c"],
            "a revisiting CHANGED-value REPLACE on a TEXT key",
        )
        .await;
    }

    // Rowid key, ALL-DISTINCT values: green — so it is the revisit, not the
    // change, that trips it.
    let handle = setup().await;
    handle
        .execute_ddl("CREATE TABLE gd (id INTEGER PRIMARY KEY, val TEXT)")
        .await
        .expect("create rowid base");
    handle
        .execute("INSERT INTO gd (id, val) VALUES (1, 'a')", vec![])
        .await
        .expect("seed");
    let select_d = "SELECT val, COUNT(*) AS c FROM gd GROUP BY val";
    reconcile_named_view(&handle, "gd_counts", select_d)
        .await
        .expect("matview");
    for next in ["b", "c", "d"] {
        handle
            .execute_unguarded(
                &format!("INSERT OR REPLACE INTO gd (id, val) VALUES (1, '{next}')"),
                vec![],
            )
            .await
            .expect("replace to a never-seen group");
        assert_matview_equals_recompute(
            &handle,
            "gd_counts",
            select_d,
            &["val", "c"],
            "a CHANGED-value REPLACE into a never-before-seen group",
        )
        .await;
    }

    // Rowid key, ON CONFLICT DO UPDATE, same revisiting sequence: green — the
    // sanctioned upsert form survives the cell that breaks REPLACE.
    let handle = setup().await;
    handle
        .execute_ddl("CREATE TABLE gc (id INTEGER PRIMARY KEY, val TEXT)")
        .await
        .expect("create rowid base");
    handle
        .execute("INSERT INTO gc (id, val) VALUES (1, 'a')", vec![])
        .await
        .expect("seed");
    let select_c = "SELECT val, COUNT(*) AS c FROM gc GROUP BY val";
    reconcile_named_view(&handle, "gc_counts", select_c)
        .await
        .expect("matview");
    for next in ["b", "a", "b", "a"] {
        handle
            .execute(
                &format!(
                    "INSERT INTO gc (id, val) VALUES (1, '{next}') ON CONFLICT(id) DO UPDATE SET \
                     val = excluded.val"
                ),
                vec![],
            )
            .await
            .expect("on-conflict upsert");
        assert_matview_equals_recompute(
            &handle,
            "gc_counts",
            select_c,
            &["val", "c"],
            "a revisiting ON CONFLICT DO UPDATE on a rowid key",
        )
        .await;
    }
}

// ---------------------------------------------------------------------------
// R2: REPLACE semantics WITHOUT the word "replace" in the write.
//
// `CREATE TABLE t (id INTEGER PRIMARY KEY ON CONFLICT REPLACE, …)` makes a
// PLAIN `INSERT` behave as a REPLACE. The DML carries no "replace" substring at
// all, so no amount of statement inspection can catch it — the hazard is
// DECLARED IN THE DDL, and that is the only place it can be refused. This is
// why the statement fast path can never be the guard's boundary.
// ---------------------------------------------------------------------------

/// Hook 1: the DDL that arms the trap is refused.
#[tokio::test]
async fn ddl_declaring_on_conflict_replace_is_refused() {
    for (label, ddl) in [
        (
            "column constraint",
            "CREATE TABLE r2 (id INTEGER PRIMARY KEY ON CONFLICT REPLACE, val TEXT)",
        ),
        (
            "table-level UNIQUE",
            "CREATE TABLE r2 (id INTEGER, val TEXT, UNIQUE(val) ON CONFLICT REPLACE)",
        ),
        (
            "lowercase, extra spacing",
            "create table r2 (id integer primary key on  conflict  replace)",
        ),
    ] {
        let handle = setup().await;
        let err = handle
            .execute_ddl(ddl)
            .await
            .expect_err(&format!("[{label}] this DDL must be refused: {ddl}"))
            .to_string();
        assert!(
            err.contains("ON CONFLICT REPLACE"),
            "[{label}] the error must name the clause; got: {err}"
        );
        assert!(
            err.contains("PR #8463"),
            "[{label}] the error must point at the engine fix; got: {err}"
        );
    }
}

/// The guard must not reject ordinary DDL — including the other conflict
/// actions, and the phrase appearing only inside a string default.
#[tokio::test]
async fn ordinary_ddl_still_passes_the_conflict_screen() {
    let handle = setup().await;
    for ddl in [
        "CREATE TABLE ok1 (id INTEGER PRIMARY KEY, val TEXT)",
        "CREATE TABLE ok2 (id INTEGER PRIMARY KEY ON CONFLICT IGNORE, val TEXT)",
        "CREATE TABLE ok3 (id TEXT PRIMARY KEY, val TEXT UNIQUE ON CONFLICT ABORT)",
        // The phrase inside a LITERAL is data, not a conflict clause.
        "CREATE TABLE ok4 (id TEXT PRIMARY KEY, note TEXT DEFAULT 'on conflict replace')",
    ] {
        handle
            .execute_ddl(ddl)
            .await
            .unwrap_or_else(|e| panic!("ordinary DDL must pass: {ddl}\n{e}"));
    }
}

/// Hook 2: a table that ALREADY carries the clause — created before this guard
/// existed, or outside DbHandle entirely — must not become a matview base.
#[tokio::test]
async fn a_matview_over_an_on_conflict_replace_base_is_refused_at_registration() {
    let handle = setup().await;
    handle
        .execute_ddl_unguarded(
            "CREATE TABLE r2 (id INTEGER PRIMARY KEY ON CONFLICT REPLACE, val TEXT)",
        )
        .await
        .expect("the unguarded door exists so this pre-existing-table case can be built at all");

    let err = reconcile_named_view(&handle, "r2_view", "SELECT id, val FROM r2")
        .await
        .expect_err("a matview over an ON CONFLICT REPLACE base must be refused")
        .to_string();
    assert!(
        err.contains("ON CONFLICT REPLACE") && err.contains("r2"),
        "the error must name the clause and the table; got: {err}"
    );
}

/// The corruption itself, so the two refusals are not guarding a hypothetical:
/// a plain INSERT with no "replace" anywhere diverges the matview exactly as an
/// explicit REPLACE does.
#[tokio::test]
async fn a_plain_insert_into_an_on_conflict_replace_table_corrupts_the_matview() {
    let handle = setup().await;
    handle
        .execute_ddl_unguarded(
            "CREATE TABLE r2 (id INTEGER PRIMARY KEY ON CONFLICT REPLACE, val TEXT)",
        )
        .await
        .expect("create the trap table");
    handle
        .execute("INSERT INTO r2 (id, val) VALUES (1, 'a')", vec![])
        .await
        .expect("seed row");
    let select = "SELECT id, val FROM r2 WHERE val = 'a'";
    handle
        .execute_ddl_unguarded(&format!("CREATE MATERIALIZED VIEW r2_view AS {select}"))
        .await
        .expect("create matview over the trap table");

    // No "replace" anywhere in this statement: the fast path returns None, the
    // statement screen never runs, and the engine performs a REPLACE anyway.
    handle
        .execute("INSERT INTO r2 (id, val) VALUES (1, 'a')", vec![])
        .await
        .expect("plain insert carrying REPLACE semantics");

    assert_matview_equals_recompute(
        &handle,
        "r2_view",
        select,
        &["id", "val"],
        "a PLAIN INSERT into an ON CONFLICT REPLACE table",
    )
    .await;
}
