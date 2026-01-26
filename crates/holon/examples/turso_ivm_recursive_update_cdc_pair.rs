//! Reproducer: UPDATEs through a `WITH RECURSIVE` matview emit a same-key
//! Created+Deleted PAIR (in INSERT-then-DELETE order) instead of a single
//! Updated event.
//!
//! Run with: `cargo run --example turso-ivm-recursive-update-cdc-pair`
//!
//! ## Setup
//!
//! - `block(id PK, content, parent_id, ...)` base table
//! - `descendants_view` — materialized view over `WITH RECURSIVE` descending
//!   from a fixed root via `parent_id`
//! - `set_view_change_callback` registered so we observe every matview delta
//!
//! ## Repro
//!
//! 1. Seed: insert `doc`, `child_1`, `child_2`
//! 2. Drain CDC
//! 3. `UPDATE block SET content = 'one' WHERE id = 'child_1'`
//!
//! ## Observed
//!
//! ```text
//! [matview CDC] descendants_view changes=2
//!   [0] Insert id='child_1' (carries new content)
//!   [1] Delete id='child_1' (carries old content)
//! ```
//!
//! ## Expected — either of:
//!
//! - **(A) ideal** — one `Update` event:
//!   ```text
//!   [matview CDC] descendants_view changes=1
//!     [0] Update id='child_1'
//!   ```
//! - **(B) acceptable** — same pair, but in `Delete`-then-`Insert` order
//!   (matches Debezium / Postgres logical replication convention, lets
//!   downstream coalescers fold into `Update`).
//!
//! ## Why current emission is problematic
//!
//! Downstream coalescers that treat INSERT-then-DELETE on the same key as a
//! no-op — a legitimate semantics for a transient base-table row that was
//! inserted and removed within a transaction — lose the real matview update
//! entirely. DBSP/Z-set semantics make the `+1` (insert) / `-1` (delete) pair
//! commute within a tick, so emission order is an implementation choice
//! rather than a semantic constraint. Holding to the `Delete`-first
//! convention (or, better, folding the pair upstream into a single `Update`)
//! has zero semantic cost and keeps every existing CDC consumer working.
//!
//! ## Cross-references
//!
//! - In-tree regression test:
//!   `crates/holon/src/storage/turso_ivm_split_block_cdc_drop_repro.rs`
//! - Holon-side handoff:
//!   `HANDOFF_TURSO_RECURSIVE_CTE_UPDATE_CDC.md`

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use turso_sdk_kit::rsapi::DatabaseChangeType;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let db_path = "/tmp/turso-ivm-recursive-update-cdc-pair.db";
    for ext in ["", "-wal", "-shm"] {
        let p = format!("{db_path}{ext}");
        if Path::new(&p).exists() {
            std::fs::remove_file(&p)?;
        }
    }

    let db = turso::Builder::new_local(db_path)
        .experimental_materialized_views(true)
        .build()
        .await?;
    let conn = db.connect()?;

    conn.execute(
        "CREATE TABLE block (
            id TEXT PRIMARY KEY,
            content TEXT DEFAULT '',
            content_type TEXT DEFAULT 'text',
            parent_id TEXT DEFAULT '',
            sort_key TEXT DEFAULT ''
        )",
        (),
    )
    .await?;

    conn.execute(
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
         WHERE _vl.source_id = 'doc'
           AND b.content_type != 'source'
           AND _vl.depth >= 0
           AND _vl.depth <= 20",
        (),
    )
    .await?;

    // Collect every change event with relation, type, and id (when parseable).
    let captured: Arc<Mutex<Vec<(String, &'static str, String)>>> =
        Arc::new(Mutex::new(Vec::new()));
    let captured_for_cb = captured.clone();
    conn.set_change_callback(move |event| {
        let mut buf = captured_for_cb.lock().unwrap();
        for change in event.changes.iter() {
            let tag = match &change.change {
                DatabaseChangeType::Insert { .. } => "Insert",
                DatabaseChangeType::Update { .. } => "Update",
                DatabaseChangeType::Delete { .. } => "Delete",
            };
            // Try to recover the row's `id` column for clarity in the printout.
            let id = change
                .parse_record()
                .and_then(|values| {
                    event
                        .columns
                        .iter()
                        .position(|c| c == "id")
                        .and_then(|i| values.get(i).cloned())
                })
                .map(|v| format!("{v:?}"))
                .unwrap_or_else(|| change.id.to_string());
            buf.push((event.relation_name.clone(), tag, id));
        }
    })?;

    // Seed.
    conn.execute(
        "INSERT INTO block (id, content, parent_id, sort_key)
         VALUES ('doc', 'Doc', '', 'A0')",
        (),
    )
    .await?;
    conn.execute(
        "INSERT INTO block (id, content, parent_id, sort_key)
         VALUES ('child_1', 'one two three', 'doc', 'B0')",
        (),
    )
    .await?;
    conn.execute(
        "INSERT INTO block (id, content, parent_id, sort_key)
         VALUES ('child_2', 'tail', 'doc', 'C0')",
        (),
    )
    .await?;

    tokio::time::sleep(Duration::from_millis(200)).await;
    captured.lock().unwrap().clear();

    // ─── The interesting bit: a single UPDATE on a row whose projection
    // flows through the recursive matview.
    println!("UPDATE block SET content = 'one' WHERE id = 'child_1' …");
    conn.execute("UPDATE block SET content = 'one' WHERE id = 'child_1'", ())
        .await?;
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Report.
    let events = captured.lock().unwrap().clone();
    let on_view: Vec<_> = events
        .iter()
        .filter(|(rel, _, _)| rel == "descendants_view")
        .collect();

    println!();
    println!("descendants_view CDC events ({}):", on_view.len());
    for (i, (_rel, tag, id)) in on_view.iter().enumerate() {
        println!("  [{i}] {tag} id={id}");
    }
    println!();

    let kinds: Vec<&str> = on_view.iter().map(|(_, t, _)| *t).collect();
    match kinds.as_slice() {
        ["Update"] => println!("OK: matview emitted a single Update — ideal."),
        ["Delete", "Insert"] => println!(
            "OK-ish: matview emitted Delete-then-Insert pair (Debezium-style).\n\
             Downstream coalescers can fold this to Update."
        ),
        ["Insert", "Delete"] => println!(
            "BUG: matview emitted Insert-then-Delete pair on the same key.\n\
             Order-aware coalescers will treat this as a transient no-op\n\
             and DROP the user-visible content change."
        ),
        [] => println!(
            "BUG (different): matview emitted no events at all\n\
             — UPDATE never reached CDC."
        ),
        other => println!("UNEXPECTED shape: {other:?}"),
    }

    Ok(())
}
