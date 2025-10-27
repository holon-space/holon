//! Reproducer + characterization: Turso IVM drops `Updated` events for
//! recursive-CTE materialized views. Production symptom is the
//! "split_block" UI staleness: Enter splits the block in SQL correctly,
//! but the editable_text on screen keeps the pre-split content because
//! the matview never emits an `Updated` row event for the truncated
//! original block.
//!
//! What the variations in this file establish:
//!
//! - `split_block_emits_both_insert_and_update_events` — the canonical
//!   regression. INSERT + UPDATE on different rows; only the Created
//!   event fires.
//! - `split_block_update_then_insert` — order doesn't matter.
//! - `split_block_insert_sleep_update` — even with 300ms between writes
//!   so the IVM has its own tick, the second UPDATE produces 0 events.
//! - `recursive_matview_handles_update_alone` — *just* an UPDATE on a
//!   recursive matview, no INSERT in sight: still 0 events. So the bug
//!   is **UPDATE through a recursive CTE matview** in general, not the
//!   INSERT + UPDATE interaction. The split_block scenario is just the
//!   most user-visible incarnation.
//! - `recursive_matview_delete_then_insert_workaround` — DELETE +
//!   INSERT for the same id **does** fire two CDC events (Deleted then
//!   Created). After `coalesce_row_changes`, downstream sees an Updated.
//!   Viable application-level workaround.
//! - `split_block_nonrecursive_matview` — control: a plain
//!   `WHERE parent_id = 'doc'` matview emits both events correctly. So
//!   the bug is specific to `WITH RECURSIVE`, not matviews in general.
//!
//! Run with:
//!   cargo test -p holon --lib turso_ivm_split_block -- --nocapture

use std::collections::HashMap;

use super::turso::{ChangeData, RowChange, TursoBackend};
use holon_api::streaming::{Batch, BatchMetadata, WithMetadata};
use tempfile::TempDir;

/// Drain the CDC channel and return all events grouped by relation name,
/// preserving order. Each entry is `(relation_name, ChangeData)`.
fn drain_cdc(
    cdc_rx: &mut tokio::sync::broadcast::Receiver<WithMetadata<Batch<RowChange>, BatchMetadata>>,
) -> Vec<(String, ChangeData)> {
    let mut out = Vec::new();
    while let Ok(batch) = cdc_rx.try_recv() {
        for change in batch.inner.items {
            out.push((change.relation_name, change.change));
        }
    }
    out
}

fn count_for_view<'a>(
    events: &'a [(String, ChangeData)],
    view: &str,
) -> impl Iterator<Item = &'a ChangeData> + 'a {
    let view = view.to_string();
    events
        .iter()
        .filter(move |(name, _)| name == &view)
        .map(|(_, c)| c)
}

#[tokio::test]
async fn split_block_emits_both_insert_and_update_events() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("split_block_cdc.db");

    let db = TursoBackend::open_database(&db_path).expect("open db");
    let (cdc_tx, mut cdc_rx) = tokio::sync::broadcast::channel(1024);
    let (_backend, handle) = TursoBackend::new(db, cdc_tx).expect("create backend");

    // Production-shape block schema (subset of the real schema, enough to
    // drive the recursive-CTE descendants matview).
    handle
        .execute_ddl(
            "CREATE TABLE block (
                id TEXT PRIMARY KEY,
                content TEXT DEFAULT '',
                content_type TEXT DEFAULT 'text',
                parent_id TEXT DEFAULT '',
                sort_key TEXT DEFAULT '',
                properties TEXT DEFAULT '{}'
            )",
        )
        .await
        .unwrap();

    // Recursive descendants matview, structurally equivalent to production's
    // `watch_view_*`. Pinned to a single root via WHERE so the test stays
    // deterministic; production's variant joins through `focus_roots` to pick
    // the root.
    handle
        .execute_ddl(
            "CREATE MATERIALIZED VIEW descendants_view AS
             WITH RECURSIVE _vl AS (
                 SELECT id AS node_id, id AS source_id, 0 AS depth
                 FROM block
                 UNION ALL
                 SELECT b.id, _vl.source_id, _vl.depth + 1
                 FROM _vl JOIN block b ON b.parent_id = _vl.node_id
                 WHERE _vl.depth < 20
             )
             SELECT b.*
             FROM _vl
             JOIN block b ON b.id = _vl.node_id
             WHERE _vl.source_id = 'doc' AND b.content_type != 'source'
               AND _vl.depth >= 0 AND _vl.depth <= 20",
        )
        .await
        .unwrap();

    // Seed: doc with two children (`child_1`, `child_2`).
    handle
        .execute(
            "INSERT INTO block (id, content, parent_id, sort_key) VALUES ('doc', 'Doc', '', 'A0')",
            vec![],
        )
        .await
        .unwrap();
    handle.execute("INSERT INTO block (id, content, parent_id, sort_key) VALUES ('child_1', 'one two three', 'doc', 'B0')", vec![]).await.unwrap();
    handle.execute("INSERT INTO block (id, content, parent_id, sort_key) VALUES ('child_2', 'tail', 'doc', 'C0')", vec![]).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let _ = drain_cdc(&mut cdc_rx);

    // ─── The split: INSERT a new sibling between child_1 and child_2,
    // and UPDATE child_1's content (truncated). Mirrors what production
    // `split_block` does inside one operation.
    handle.execute(
        "INSERT INTO block (id, content, parent_id, sort_key) VALUES ('child_1_split', 'two three', 'doc', 'B5')",
        vec![],
    ).await.unwrap();
    handle
        .execute(
            "UPDATE block SET content = 'one' WHERE id = 'child_1'",
            vec![],
        )
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let events = drain_cdc(&mut cdc_rx);
    eprintln!("[split_repro] all CDC events ({}):", events.len());
    for (name, change) in &events {
        let tag = match change {
            ChangeData::Created { .. } => "Created",
            ChangeData::Updated { .. } => "Updated",
            ChangeData::Deleted { .. } => "Deleted",
            ChangeData::FieldsChanged { .. } => "FieldsChanged",
        };
        let id = match change {
            ChangeData::Created { data, .. } => data
                .get("id")
                .and_then(|v| v.as_string())
                .map(|s| s.to_string())
                .unwrap_or_default(),
            ChangeData::Updated { id, .. } => id.clone(),
            ChangeData::Deleted { id, .. } => id.clone(),
            ChangeData::FieldsChanged { entity_id, .. } => entity_id.clone(),
        };
        eprintln!("  {name}: {tag} id={id}");
    }

    // Verify the matview reflects the post-split state at SQL level.
    let rows = handle
        .query(
            "SELECT id, content FROM descendants_view WHERE id IN ('child_1', 'child_1_split') ORDER BY id",
            HashMap::new(),
        )
        .await
        .unwrap();
    eprintln!("[split_repro] post-split SQL state of matview:");
    for r in &rows {
        eprintln!(
            "  id={:?} content={:?}",
            r.get("id").and_then(|v| v.as_string()),
            r.get("content").and_then(|v| v.as_string())
        );
    }

    // Both events should reach the matview's CDC stream:
    //   1. Created/Inserted: child_1_split (new sibling)
    //   2. Updated:           child_1     (truncated content)
    let view_events: Vec<&ChangeData> = count_for_view(&events, "descendants_view").collect();
    let saw_insert_new = view_events.iter().any(|c| matches!(c, ChangeData::Created { data, .. } if data.get("id").and_then(|v| v.as_string()).as_deref() == Some("child_1_split")));
    let saw_update_orig = view_events.iter().any(|c| match c {
        ChangeData::Updated { id, .. } => id == "child_1",
        // After coalesce, a same-batch DELETE+INSERT for child_1 with new
        // content also surfaces as Updated. Either is acceptable here.
        _ => false,
    });

    assert!(
        saw_insert_new,
        "Expected a Created/Updated event for new sibling 'child_1_split' on descendants_view"
    );
    assert!(
        saw_update_orig,
        "BUG: Expected an Updated event on descendants_view for 'child_1' (content truncated), \
         but only saw {} events for the view: {:?}",
        view_events.len(),
        view_events
            .iter()
            .map(|c| match c {
                ChangeData::Created { data, .. } => format!(
                    "Created({})",
                    data.get("id")
                        .and_then(|v| v.as_string())
                        .unwrap_or_default()
                ),
                ChangeData::Updated { id, .. } => format!("Updated({id})"),
                ChangeData::Deleted { id, .. } => format!("Deleted({id})"),
                ChangeData::FieldsChanged { entity_id, .. } =>
                    format!("FieldsChanged({entity_id})"),
            })
            .collect::<Vec<_>>()
    );
}

fn short_change(c: &ChangeData) -> String {
    match c {
        ChangeData::Created { data, .. } => format!(
            "Created({})",
            data.get("id")
                .and_then(|v| v.as_string())
                .unwrap_or_default()
        ),
        ChangeData::Updated { id, .. } => format!("Updated({id})"),
        ChangeData::Deleted { id, .. } => format!("Deleted({id})"),
        ChangeData::FieldsChanged { entity_id, .. } => format!("FieldsChanged({entity_id})"),
    }
}

/// Variation: do the UPDATE before the INSERT. Tests whether order matters.
#[tokio::test]
async fn split_block_update_then_insert() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("split_update_first.db");
    let db = TursoBackend::open_database(&db_path).expect("open db");
    let (cdc_tx, mut cdc_rx) = tokio::sync::broadcast::channel(1024);
    let (_backend, handle) = TursoBackend::new(db, cdc_tx).expect("create backend");

    handle.execute_ddl("CREATE TABLE block (id TEXT PRIMARY KEY, content TEXT DEFAULT '', content_type TEXT DEFAULT 'text', parent_id TEXT DEFAULT '', sort_key TEXT DEFAULT '', properties TEXT DEFAULT '{}')").await.unwrap();
    handle
        .execute_ddl(
            "CREATE MATERIALIZED VIEW descendants_view AS
         WITH RECURSIVE _vl AS (
             SELECT id AS node_id, id AS source_id, 0 AS depth FROM block
             UNION ALL
             SELECT b.id, _vl.source_id, _vl.depth + 1
             FROM _vl JOIN block b ON b.parent_id = _vl.node_id
             WHERE _vl.depth < 20
         )
         SELECT b.* FROM _vl JOIN block b ON b.id = _vl.node_id
         WHERE _vl.source_id = 'doc' AND b.content_type != 'source'
           AND _vl.depth >= 0 AND _vl.depth <= 20",
        )
        .await
        .unwrap();

    handle
        .execute(
            "INSERT INTO block (id, content, parent_id, sort_key) VALUES ('doc', 'Doc', '', 'A0')",
            vec![],
        )
        .await
        .unwrap();
    handle.execute("INSERT INTO block (id, content, parent_id, sort_key) VALUES ('child_1', 'one two three', 'doc', 'B0')", vec![]).await.unwrap();
    handle.execute("INSERT INTO block (id, content, parent_id, sort_key) VALUES ('child_2', 'tail', 'doc', 'C0')", vec![]).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let _ = drain_cdc(&mut cdc_rx);

    handle
        .execute(
            "UPDATE block SET content = 'one' WHERE id = 'child_1'",
            vec![],
        )
        .await
        .unwrap();
    handle.execute("INSERT INTO block (id, content, parent_id, sort_key) VALUES ('child_1_split', 'two three', 'doc', 'B5')", vec![]).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let events = drain_cdc(&mut cdc_rx);
    eprintln!("[update_then_insert] events ({}):", events.len());
    for (n, c) in &events {
        eprintln!("  {n}: {}", short_change(c));
    }
    let view_events: Vec<&ChangeData> = count_for_view(&events, "descendants_view").collect();
    let saw_update = view_events
        .iter()
        .any(|c| matches!(c, ChangeData::Updated { id, .. } if id == "child_1"));
    let saw_insert = view_events.iter().any(|c| matches!(c, ChangeData::Created { data, .. } if data.get("id").and_then(|v| v.as_string()).as_deref() == Some("child_1_split")));
    eprintln!("[update_then_insert] saw_update_orig={saw_update} saw_insert_new={saw_insert}");
}

/// Variation: insert sibling, sleep so matview re-evaluates, then update.
/// Tests whether spreading writes across IVM ticks rescues the Updated event.
#[tokio::test]
async fn split_block_insert_sleep_update() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("split_sleep.db");
    let db = TursoBackend::open_database(&db_path).expect("open db");
    let (cdc_tx, mut cdc_rx) = tokio::sync::broadcast::channel(1024);
    let (_backend, handle) = TursoBackend::new(db, cdc_tx).expect("create backend");

    handle.execute_ddl("CREATE TABLE block (id TEXT PRIMARY KEY, content TEXT DEFAULT '', content_type TEXT DEFAULT 'text', parent_id TEXT DEFAULT '', sort_key TEXT DEFAULT '', properties TEXT DEFAULT '{}')").await.unwrap();
    handle
        .execute_ddl(
            "CREATE MATERIALIZED VIEW descendants_view AS
         WITH RECURSIVE _vl AS (
             SELECT id AS node_id, id AS source_id, 0 AS depth FROM block
             UNION ALL
             SELECT b.id, _vl.source_id, _vl.depth + 1
             FROM _vl JOIN block b ON b.parent_id = _vl.node_id
             WHERE _vl.depth < 20
         )
         SELECT b.* FROM _vl JOIN block b ON b.id = _vl.node_id
         WHERE _vl.source_id = 'doc' AND b.content_type != 'source'
           AND _vl.depth >= 0 AND _vl.depth <= 20",
        )
        .await
        .unwrap();

    handle
        .execute(
            "INSERT INTO block (id, content, parent_id, sort_key) VALUES ('doc', 'Doc', '', 'A0')",
            vec![],
        )
        .await
        .unwrap();
    handle.execute("INSERT INTO block (id, content, parent_id, sort_key) VALUES ('child_1', 'one two three', 'doc', 'B0')", vec![]).await.unwrap();
    handle.execute("INSERT INTO block (id, content, parent_id, sort_key) VALUES ('child_2', 'tail', 'doc', 'C0')", vec![]).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let _ = drain_cdc(&mut cdc_rx);

    handle.execute("INSERT INTO block (id, content, parent_id, sort_key) VALUES ('child_1_split', 'two three', 'doc', 'B5')", vec![]).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let after_insert = drain_cdc(&mut cdc_rx);
    eprintln!("[sleep_between] after-insert ({}):", after_insert.len());
    for (n, c) in &after_insert {
        eprintln!("  {n}: {}", short_change(c));
    }

    handle
        .execute(
            "UPDATE block SET content = 'one' WHERE id = 'child_1'",
            vec![],
        )
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let after_update = drain_cdc(&mut cdc_rx);
    eprintln!("[sleep_between] after-update ({}):", after_update.len());
    for (n, c) in &after_update {
        eprintln!("  {n}: {}", short_change(c));
    }
}

/// Variation: UPDATE alone (no INSERT) through the recursive matview.
/// Tests whether recursive matviews handle plain Updated events at all.
#[tokio::test]
async fn recursive_matview_handles_update_alone() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("update_only.db");
    let db = TursoBackend::open_database(&db_path).expect("open db");
    let (cdc_tx, mut cdc_rx) = tokio::sync::broadcast::channel(1024);
    let (_backend, handle) = TursoBackend::new(db, cdc_tx).expect("create backend");

    handle.execute_ddl("CREATE TABLE block (id TEXT PRIMARY KEY, content TEXT DEFAULT '', content_type TEXT DEFAULT 'text', parent_id TEXT DEFAULT '', sort_key TEXT DEFAULT '', properties TEXT DEFAULT '{}')").await.unwrap();
    handle
        .execute_ddl(
            "CREATE MATERIALIZED VIEW descendants_view AS
         WITH RECURSIVE _vl AS (
             SELECT id AS node_id, id AS source_id, 0 AS depth FROM block
             UNION ALL
             SELECT b.id, _vl.source_id, _vl.depth + 1
             FROM _vl JOIN block b ON b.parent_id = _vl.node_id
             WHERE _vl.depth < 20
         )
         SELECT b.* FROM _vl JOIN block b ON b.id = _vl.node_id
         WHERE _vl.source_id = 'doc' AND b.content_type != 'source'
           AND _vl.depth >= 0 AND _vl.depth <= 20",
        )
        .await
        .unwrap();

    handle
        .execute(
            "INSERT INTO block (id, content, parent_id, sort_key) VALUES ('doc', 'Doc', '', 'A0')",
            vec![],
        )
        .await
        .unwrap();
    handle.execute("INSERT INTO block (id, content, parent_id, sort_key) VALUES ('child_1', 'one two three', 'doc', 'B0')", vec![]).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let _ = drain_cdc(&mut cdc_rx);

    handle
        .execute(
            "UPDATE block SET content = 'one' WHERE id = 'child_1'",
            vec![],
        )
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let events = drain_cdc(&mut cdc_rx);
    eprintln!("[update_alone] events ({}):", events.len());
    for (n, c) in &events {
        eprintln!("  {n}: {}", short_change(c));
    }
    let view_events: Vec<&ChangeData> = count_for_view(&events, "descendants_view").collect();
    let saw_update = view_events
        .iter()
        .any(|c| matches!(c, ChangeData::Updated { id, .. } if id == "child_1"));
    eprintln!("[update_alone] saw_update_orig={saw_update}");
}

/// Workaround attempt: replace UPDATE with DELETE + INSERT (with the new
/// content). If this fires CDC where UPDATE did not, we have a viable
/// application-level workaround: rewrite `set_field` (or at least the
/// content field on split_block) to delete-and-reinsert.
#[tokio::test]
async fn recursive_matview_delete_then_insert_workaround() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("delete_insert.db");
    let db = TursoBackend::open_database(&db_path).expect("open db");
    let (cdc_tx, mut cdc_rx) = tokio::sync::broadcast::channel(1024);
    let (_backend, handle) = TursoBackend::new(db, cdc_tx).expect("create backend");

    handle.execute_ddl("CREATE TABLE block (id TEXT PRIMARY KEY, content TEXT DEFAULT '', content_type TEXT DEFAULT 'text', parent_id TEXT DEFAULT '', sort_key TEXT DEFAULT '', properties TEXT DEFAULT '{}')").await.unwrap();
    handle
        .execute_ddl(
            "CREATE MATERIALIZED VIEW descendants_view AS
         WITH RECURSIVE _vl AS (
             SELECT id AS node_id, id AS source_id, 0 AS depth FROM block
             UNION ALL
             SELECT b.id, _vl.source_id, _vl.depth + 1
             FROM _vl JOIN block b ON b.parent_id = _vl.node_id
             WHERE _vl.depth < 20
         )
         SELECT b.* FROM _vl JOIN block b ON b.id = _vl.node_id
         WHERE _vl.source_id = 'doc' AND b.content_type != 'source'
           AND _vl.depth >= 0 AND _vl.depth <= 20",
        )
        .await
        .unwrap();

    handle
        .execute(
            "INSERT INTO block (id, content, parent_id, sort_key) VALUES ('doc', 'Doc', '', 'A0')",
            vec![],
        )
        .await
        .unwrap();
    handle.execute("INSERT INTO block (id, content, parent_id, sort_key) VALUES ('child_1', 'one two three', 'doc', 'B0')", vec![]).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let _ = drain_cdc(&mut cdc_rx);

    // DELETE + INSERT instead of UPDATE. Coalesce should fold them into Updated.
    handle
        .execute("DELETE FROM block WHERE id = 'child_1'", vec![])
        .await
        .unwrap();
    handle.execute("INSERT INTO block (id, content, parent_id, sort_key) VALUES ('child_1', 'one', 'doc', 'B0')", vec![]).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let events = drain_cdc(&mut cdc_rx);
    eprintln!("[delete_insert] events ({}):", events.len());
    for (n, c) in &events {
        eprintln!("  {n}: {}", short_change(c));
    }
    let view_events: Vec<&ChangeData> = count_for_view(&events, "descendants_view").collect();
    let saw_any_for_child_1 = view_events.iter().any(|c| match c {
        ChangeData::Updated { id, .. } => id == "child_1",
        ChangeData::Created { data, .. } => {
            data.get("id").and_then(|v| v.as_string()).as_deref() == Some("child_1")
        }
        ChangeData::Deleted { id, .. } => id == "child_1",
        _ => false,
    });
    eprintln!("[delete_insert] saw_any_for_child_1={saw_any_for_child_1}");
}

/// Variation: same INSERT + UPDATE but the matview is **non-recursive**
/// (a plain SELECT). Tests whether the recursive CTE specifically is the
/// IVM's blind spot.
#[tokio::test]
async fn split_block_nonrecursive_matview() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("split_nonrec.db");
    let db = TursoBackend::open_database(&db_path).expect("open db");
    let (cdc_tx, mut cdc_rx) = tokio::sync::broadcast::channel(1024);
    let (_backend, handle) = TursoBackend::new(db, cdc_tx).expect("create backend");

    handle.execute_ddl("CREATE TABLE block (id TEXT PRIMARY KEY, content TEXT DEFAULT '', content_type TEXT DEFAULT 'text', parent_id TEXT DEFAULT '', sort_key TEXT DEFAULT '', properties TEXT DEFAULT '{}')").await.unwrap();
    handle
        .execute_ddl(
            "CREATE MATERIALIZED VIEW children_view AS
         SELECT id, content, parent_id, sort_key FROM block WHERE parent_id = 'doc'",
        )
        .await
        .unwrap();

    handle
        .execute(
            "INSERT INTO block (id, content, parent_id, sort_key) VALUES ('doc', 'Doc', '', 'A0')",
            vec![],
        )
        .await
        .unwrap();
    handle.execute("INSERT INTO block (id, content, parent_id, sort_key) VALUES ('child_1', 'one two three', 'doc', 'B0')", vec![]).await.unwrap();
    handle.execute("INSERT INTO block (id, content, parent_id, sort_key) VALUES ('child_2', 'tail', 'doc', 'C0')", vec![]).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let _ = drain_cdc(&mut cdc_rx);

    handle.execute("INSERT INTO block (id, content, parent_id, sort_key) VALUES ('child_1_split', 'two three', 'doc', 'B5')", vec![]).await.unwrap();
    handle
        .execute(
            "UPDATE block SET content = 'one' WHERE id = 'child_1'",
            vec![],
        )
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let events = drain_cdc(&mut cdc_rx);
    eprintln!("[nonrecursive] events ({}):", events.len());
    for (n, c) in &events {
        eprintln!("  {n}: {}", short_change(c));
    }
    let view_events: Vec<&ChangeData> = count_for_view(&events, "children_view").collect();
    let saw_update = view_events
        .iter()
        .any(|c| matches!(c, ChangeData::Updated { id, .. } if id == "child_1"));
    let saw_insert = view_events.iter().any(|c| matches!(c, ChangeData::Created { data, .. } if data.get("id").and_then(|v| v.as_string()).as_deref() == Some("child_1_split")));
    eprintln!("[nonrecursive] saw_update_orig={saw_update} saw_insert_new={saw_insert}");
}
