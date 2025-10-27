//! Characterization: Turso CDC emits events for matviews, NOT for base tables.
//!
//! The handoff at `devlog/HANDOFF_LIVEDATA_REMAINING.md` §Follow-up 6
//! hypothesised that `LiveData<T>` could skip the
//! `CREATE MATERIALIZED VIEW … SELECT * FROM <table>` overhead by subscribing
//! to base-table CDC directly via `MatviewManager::subscribe_cdc("<table>")`,
//! since the demux routes by `BatchMetadata::relation_name` and would
//! presumably accept a base-table name as the routing key.
//!
//! These tests falsify that premise:
//!
//! - Direct `INSERT`/`UPDATE`/`DELETE` against a base table emits **zero**
//!   CDC batches with `relation_name == <table>` on the broadcast channel.
//! - Only the matviews that select from the table emit CDC batches, with
//!   `relation_name == <matview_name>`.
//! - Transactional inserts behave the same way: no base-table emission.
//!
//! Implication: every `LiveData` consumer **must** go through
//! `MatviewManager::watch(sql)` (or equivalent matview creation). The
//! matview is the mechanism by which Turso surfaces row changes. Removing
//! the matview removes the change stream.
//!
//! Run with:
//!   cargo test -p holon --lib cdc_base_vs_matview -- --nocapture

use super::turso::{ChangeData, RowChange, TursoBackend};
use holon_api::streaming::{Batch, BatchMetadata, WithMetadata};
use std::collections::HashMap;

type RawCdcRx = tokio::sync::broadcast::Receiver<WithMetadata<Batch<RowChange>, BatchMetadata>>;

/// Drain the broadcast channel and return all events grouped by relation name,
/// preserving order.
fn drain_cdc(cdc_rx: &mut RawCdcRx) -> Vec<(String, ChangeData)> {
    let mut out = Vec::new();
    while let Ok(batch) = cdc_rx.try_recv() {
        for change in batch.inner.items {
            out.push((change.relation_name, change.change));
        }
    }
    out
}

fn fmt_change(c: &ChangeData) -> String {
    match c {
        ChangeData::Created { data, .. } => {
            let id = data
                .get("id")
                .and_then(|v| v.as_string())
                .unwrap_or("<no-id>");
            format!("Created(id={id})")
        }
        ChangeData::Updated { id, data, .. } => {
            let content = data
                .get("content")
                .and_then(|v| v.as_string())
                .unwrap_or("<no-content>");
            format!("Updated(id={id}, content={content:?})")
        }
        ChangeData::Deleted { id, .. } => format!("Deleted(id={id})"),
        ChangeData::FieldsChanged { entity_id, .. } => {
            format!("FieldsChanged(entity_id={entity_id})")
        }
    }
}

fn dump_events(label: &str, events: &[(String, ChangeData)]) {
    eprintln!("[{label}] {} events:", events.len());
    for (name, change) in events {
        eprintln!("  {name}: {}", fmt_change(change));
    }
}

/// Counts of (Created, Updated, Deleted) for a given relation across the events.
fn shape_for(events: &[(String, ChangeData)], relation: &str) -> (usize, usize, usize) {
    let mut c = 0usize;
    let mut u = 0usize;
    let mut d = 0usize;
    for (name, change) in events.iter().filter(|(n, _)| n == relation) {
        match change {
            ChangeData::Created { .. } => c += 1,
            ChangeData::Updated { .. } => u += 1,
            ChangeData::Deleted { .. } => d += 1,
            ChangeData::FieldsChanged { .. } => {}
        }
        let _ = name;
    }
    (c, u, d)
}

async fn setup() -> (TempBackend, RawCdcRx) {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let db_path = temp_dir.path().join("cdc_base_vs_matview.db");

    let db = TursoBackend::open_database(&db_path).expect("open db");
    let (cdc_tx, cdc_rx) = tokio::sync::broadcast::channel(1024);
    let (_backend, handle) = TursoBackend::new(db, cdc_tx).expect("create backend");

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

    // Mirror matview: SELECT * FROM block. Structurally equivalent to what
    // LiveData currently sets up via MatviewManager::watch.
    handle
        .execute_ddl(
            "CREATE MATERIALIZED VIEW block_mirror AS \
             SELECT id, content, content_type, parent_id, sort_key, properties \
             FROM block",
        )
        .await
        .unwrap();

    (
        TempBackend {
            _temp_dir: temp_dir,
            handle,
        },
        cdc_rx,
    )
}

struct TempBackend {
    _temp_dir: tempfile::TempDir,
    handle: super::turso::DbHandle,
}

/// Direct INSERT/UPDATE/DELETE against the base table emits zero CDC events
/// keyed on the table name; only the mirror matview fires.
#[tokio::test]
async fn base_table_writes_only_emit_via_matview() {
    let (be, mut cdc_rx) = setup().await;

    // Single insert.
    be.handle
        .execute(
            "INSERT INTO block (id, content, parent_id, sort_key) \
             VALUES ('blk-1', 'hello', '', 'A0')",
            vec![],
        )
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let events = drain_cdc(&mut cdc_rx);
    dump_events("insert", &events);
    let block = shape_for(&events, "block");
    let mirror = shape_for(&events, "block_mirror");
    eprintln!(
        "[insert] block (C/U/D)={:?}  block_mirror (C/U/D)={:?}",
        block, mirror
    );
    assert_eq!(
        block,
        (0, 0, 0),
        "base table 'block' must NOT emit CDC events directly; saw {block:?}",
    );
    assert!(
        mirror.0 >= 1,
        "matview 'block_mirror' must emit a Created event; saw {mirror:?}",
    );

    // Update.
    be.handle
        .execute(
            "UPDATE block SET content = 'hello world' WHERE id = 'blk-1'",
            vec![],
        )
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let events = drain_cdc(&mut cdc_rx);
    dump_events("update", &events);
    let block = shape_for(&events, "block");
    let mirror = shape_for(&events, "block_mirror");
    eprintln!(
        "[update] block (C/U/D)={:?}  block_mirror (C/U/D)={:?}",
        block, mirror
    );
    assert_eq!(
        block,
        (0, 0, 0),
        "base table 'block' must NOT emit CDC events directly; saw {block:?}",
    );
    // Sanity: non-recursive matview should fire on UPDATE (split_block_cdc_drop
    // is the recursive-matview UPDATE-drop case; this one is plain).
    assert!(
        mirror.1 >= 1,
        "matview 'block_mirror' must emit an Updated event; saw {mirror:?}",
    );

    // Delete.
    be.handle
        .execute("DELETE FROM block WHERE id = 'blk-1'", vec![])
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let events = drain_cdc(&mut cdc_rx);
    dump_events("delete", &events);
    let block = shape_for(&events, "block");
    let mirror = shape_for(&events, "block_mirror");
    eprintln!(
        "[delete] block (C/U/D)={:?}  block_mirror (C/U/D)={:?}",
        block, mirror
    );
    assert_eq!(
        block,
        (0, 0, 0),
        "base table 'block' must NOT emit CDC events directly; saw {block:?}",
    );
    assert!(
        mirror.2 >= 1,
        "matview 'block_mirror' must emit a Deleted event; saw {mirror:?}",
    );
}

/// Records what the matview-emitted row payload actually looks like for a
/// `SELECT * FROM block` mirror. Useful for reasoning about what `LiveData`
/// gets when it parses the row.
#[tokio::test]
async fn matview_created_event_carries_full_row() {
    let (be, mut cdc_rx) = setup().await;

    be.handle
        .execute(
            "INSERT INTO block (id, content, parent_id, sort_key) \
             VALUES ('blk-2', 'lorem ipsum', 'doc-1', 'B0')",
            vec![],
        )
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let events = drain_cdc(&mut cdc_rx);

    let mirror_data = events
        .iter()
        .find_map(|(name, change)| {
            if name != "block_mirror" {
                return None;
            }
            match change {
                ChangeData::Created { data, .. } => Some(data.clone()),
                _ => None,
            }
        })
        .expect("missing Created on 'block_mirror'");

    let pick = |data: &HashMap<String, holon_api::Value>, key: &str| {
        data.get(key)
            .and_then(|v| v.as_string())
            .unwrap_or("<missing>")
            .to_string()
    };
    eprintln!(
        "[shape] block_mirror: id={:?} content={:?} parent_id={:?} sort_key={:?}",
        pick(&mirror_data, "id"),
        pick(&mirror_data, "content"),
        pick(&mirror_data, "parent_id"),
        pick(&mirror_data, "sort_key"),
    );
    assert_eq!(pick(&mirror_data, "id"), "blk-2");
    assert_eq!(pick(&mirror_data, "content"), "lorem ipsum");
    assert_eq!(pick(&mirror_data, "parent_id"), "doc-1");
    assert_eq!(pick(&mirror_data, "sort_key"), "B0");
}

/// Same conclusion under a multi-statement transaction: matview emits one
/// Created per row, base table emits nothing.
#[tokio::test]
async fn transactional_batch_emits_only_via_matview() {
    let (be, mut cdc_rx) = setup().await;

    be.handle
        .transaction(vec![
            (
                "INSERT INTO block (id, content, parent_id, sort_key) \
                 VALUES ('tx-a', 'A', '', 'A0')"
                    .into(),
                vec![],
            ),
            (
                "INSERT INTO block (id, content, parent_id, sort_key) \
                 VALUES ('tx-b', 'B', '', 'B0')"
                    .into(),
                vec![],
            ),
            (
                "INSERT INTO block (id, content, parent_id, sort_key) \
                 VALUES ('tx-c', 'C', '', 'C0')"
                    .into(),
                vec![],
            ),
        ])
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let events = drain_cdc(&mut cdc_rx);
    dump_events("transaction", &events);

    let block = shape_for(&events, "block");
    let mirror = shape_for(&events, "block_mirror");
    eprintln!(
        "[transaction] block (C/U/D)={:?}  block_mirror (C/U/D)={:?}",
        block, mirror
    );
    assert_eq!(
        block,
        (0, 0, 0),
        "base table 'block' must NOT emit CDC events directly; saw {block:?}",
    );
    assert_eq!(
        mirror.0, 3,
        "matview 'block_mirror' must emit 3 Created events; saw {mirror:?}",
    );
}
