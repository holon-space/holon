//! Phase 4b oracle: `LinkEventSubscriber` maintains the `block_link` table
//! from the convergent `LiveData<Block>` feed (NOT the EventBus).
//!
//! This is the previously-missing oracle for the link sink — it pins the
//! contract the Phase-4b rewire must preserve: a block inserted/updated in the
//! feed re-extracts its `[[...]]` links into `block_link`; a removal drops
//! them.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use holon::storage::turso::DbHandle;
use holon::storage::turso::TursoBackend;
use holon::sync::link_event_subscriber::LinkEventSubscriber;
use holon::sync::live_data::LiveData;
use holon_api::Change;
use holon_api::EntityUri;
use holon_api::block::Block;

async fn setup_block_link(handle: &DbHandle) {
    handle
        .execute_ddl(
            "CREATE TABLE block_link (
                source_block_id TEXT NOT NULL,
                target_raw TEXT NOT NULL,
                target_id TEXT,
                display_text TEXT,
                position INTEGER NOT NULL,
                PRIMARY KEY (source_block_id, position)
            )",
        )
        .await
        .expect("create block_link");
}

async fn link_targets(handle: &DbHandle, source: &str) -> Vec<String> {
    let sql = format!(
        "SELECT target_raw FROM block_link WHERE source_block_id = '{}' ORDER BY position",
        source.replace('\'', "''")
    );
    let rows = handle
        .query(&sql, HashMap::new())
        .await
        .expect("query block_link");
    rows.iter()
        .filter_map(|r| {
            r.get("target_raw")
                .and_then(|v| v.as_string())
                .map(String::from)
        })
        .collect()
}

/// Poll `block_link` until `source`'s targets equal `expected`
/// (order-insensitive), or panic after `timeout`. The subscriber processes the
/// feed on a spawned task, so the write is eventually-consistent.
async fn await_targets(
    handle: &DbHandle,
    source: &str,
    mut expected: Vec<String>,
    timeout: Duration,
) {
    expected.sort();
    let start = Instant::now();
    loop {
        let mut got = link_targets(handle, source).await;
        got.sort();
        if got == expected {
            return;
        }
        if start.elapsed() > timeout {
            panic!("[link oracle] timed out for {source}: expected {expected:?}, got {got:?}");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn block_with(id: &str, content: &str) -> Block {
    Block::new_text(
        EntityUri::block(id),
        EntityUri::no_parent(),
        content.to_string(),
    )
}

#[tokio::test]
async fn block_link_maintained_from_live_data_feed() {
    let (_backend, handle) = TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso");
    setup_block_link(&handle).await;

    let live: Arc<LiveData<Block>> = LiveData::new(
        vec![],
        |row| {
            row.get("id")
                .and_then(|v| v.as_string())
                .map(|s| s.to_string())
                .ok_or_else(|| anyhow::anyhow!("row missing id"))
        },
        |row| Block::try_from(row.clone()),
    );

    let indexer = Arc::new(holon::storage::TursoBlockLinkIndexer::new(handle.clone()));
    let subscriber = LinkEventSubscriber::new(indexer);
    subscriber.start_from_live_data(live.clone());

    let timeout = Duration::from_secs(5);

    // Create: a block with one link → one block_link row.
    live.insert(
        "block:a".to_string(),
        Arc::new(block_with("a", "see [[target-page]] for details")),
    );
    await_targets(&handle, "block:a", vec!["target-page".to_string()], timeout).await;

    // Update: replace content → links re-extracted (old gone, new present).
    live.insert(
        "block:a".to_string(),
        Arc::new(block_with("a", "now [[other-page]] and [[third]]")),
    );
    await_targets(
        &handle,
        "block:a",
        vec!["other-page".to_string(), "third".to_string()],
        timeout,
    )
    .await;

    // Delete: removing the block from the feed drops its links.
    live.apply_changes(vec![Change::Deleted {
        id: "block:a".to_string(),
        origin: holon_api::ChangeOrigin::Local {
            operation_id: None,
            trace_id: None,
        },
    }]);
    await_targets(&handle, "block:a", vec![], timeout).await;
}
