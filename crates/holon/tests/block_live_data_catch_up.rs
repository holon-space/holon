//! Phase 5 Step-0 oracle: `LiveData<Block>::wait_until` is a sound *positional*
//! catch-up primitive — the replacement for the `event_acks`-watermark wait
//! (`wait_for_cache_caught_up`).
//!
//! The contract this pins (and that the Phase-5 cutover must preserve):
//!   1. `wait_until` returns `true` as soon as the predicate holds over the feed.
//!   2. It wakes promptly when the awaited rows arrive concurrently — via the CDC
//!      `apply_changes` path *and* via the optimistic `insert` path — not on a
//!      poll timer.
//!   3. It returns `false` on timeout when the rows never arrive — and crucially
//!      it does NOT return early just because the feed is *quiet* (no traffic).
//!      That is the load-bearing difference from `wait_for_quiescent`: a
//!      quiescence-only wait could return before the projection producing the
//!      rows has even started, reintroducing the stale-cache re-render race (R1).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use holon::sync::live_data::LiveData;
use holon_api::block::Block;
use holon_api::{Change, EntityUri, Value};
use holon_core::storage::types::StorageEntity;

fn live_blocks() -> Arc<LiveData<Block>> {
    LiveData::new(
        vec![],
        |row| {
            row.get("id")
                .and_then(|v| v.as_string())
                .map(|s| s.to_string())
                .ok_or_else(|| anyhow::anyhow!("row missing id"))
        },
        |row| Block::try_from(row.clone()),
    )
}

fn block(id: &str, content: &str) -> Block {
    Block::new_text(
        EntityUri::block(id),
        EntityUri::no_parent(),
        content.to_string(),
    )
}

/// A `StorageEntity` row as the `block` matview's CDC stream delivers it —
/// the faithful input to `LiveData::apply_changes`.
fn block_row(id: &str, content: &str) -> holon_api::StorageEntity {
    let mut row: holon_api::StorageEntity = holon_api::StorageEntity::new();
    row.insert(
        "id".into(),
        Value::String(EntityUri::block(id).as_str().to_string()),
    );
    row.insert(
        "parent_id".into(),
        Value::String(EntityUri::no_parent().as_str().to_string()),
    );
    row.insert("content".into(), Value::String(content.to_string()));
    // The `block` matview always projects these; Block's strict row parser
    // requires them.
    row.insert("content_type".into(), Value::String("text".to_string()));
    row.insert("created_at".into(), Value::Integer(0));
    row.insert("updated_at".into(), Value::Integer(0));
    row.insert("tags".into(), Value::Json("[]".to_string()));
    row.insert("requires".into(), Value::Json("[]".to_string()));
    row.insert("advice_suppressed".into(), Value::Json("[]".to_string()));
    row
}

fn created(id: &str, content: &str) -> Change<StorageEntity> {
    Change::Created {
        data: block_row(id, content),
        origin: holon_api::ChangeOrigin::Local {
            operation_id: None,
            trace_id: None,
        },
    }
}

fn has_count(n: usize) -> impl FnMut(&BTreeMap<String, Arc<Block>>) -> bool {
    move |map| map.len() >= n
}

/// Predicate already satisfied → returns immediately, no waiting.
#[tokio::test]
async fn returns_true_when_predicate_already_holds() {
    let live = live_blocks();
    live.insert("block:a".to_string(), Arc::new(block("a", "hello")));

    let caught_up = live.wait_until(has_count(1), Duration::from_secs(5)).await;
    assert!(
        caught_up,
        "predicate already held; wait_until must return true"
    );
}

/// Rows arrive concurrently through the CDC `apply_changes` path → the wait
/// wakes promptly (well under the timeout), proving it is notify-driven and
/// observes the awaited count.
#[tokio::test]
async fn wakes_when_blocks_arrive_via_cdc() {
    let live = live_blocks();
    let writer = live.clone();

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        writer.apply_changes(vec![created("a", "first"), created("b", "second")]);
    });

    let start = std::time::Instant::now();
    let caught_up = live.wait_until(has_count(2), Duration::from_secs(5)).await;
    let elapsed = start.elapsed();

    assert!(caught_up, "two blocks arrived; wait_until must return true");
    assert!(
        elapsed < Duration::from_secs(2),
        "wait_until should wake on CDC arrival, not poll-spin; took {elapsed:?}"
    );
    assert_eq!(live.read().len(), 2);
}

/// Rows arrive concurrently through the optimistic `insert` bypass → the wait
/// also wakes (proving `wait_until` is decoupled from the seq mechanism).
#[tokio::test]
async fn wakes_when_blocks_arrive_via_insert() {
    let live = live_blocks();
    let writer = live.clone();

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        writer.insert("block:a".to_string(), Arc::new(block("a", "via insert")));
    });

    let caught_up = live
        .wait_until(|m| m.contains_key("block:a"), Duration::from_secs(5))
        .await;
    assert!(
        caught_up,
        "block arrived via insert; wait_until must return true"
    );
}

/// The load-bearing R1 guarantee: a *quiet* feed (no traffic at all) that never
/// produces the awaited rows must time out to `false` — NOT return early like a
/// quiescence wait would.
#[tokio::test]
async fn times_out_false_on_quiet_feed_without_rows() {
    let live = live_blocks();

    let start = std::time::Instant::now();
    let caught_up = live
        .wait_until(has_count(1), Duration::from_millis(300))
        .await;
    let elapsed = start.elapsed();

    assert!(
        !caught_up,
        "no blocks ever arrived; wait_until must time out to false, \
         not false-return on quiescence"
    );
    assert!(
        elapsed >= Duration::from_millis(300),
        "wait_until must wait the full timeout on a quiet, unsatisfied feed; \
         took {elapsed:?}"
    );
}

/// Partial arrival that never reaches the threshold still times out to `false`
/// (the count predicate is genuinely positional, not "any change seen").
#[tokio::test]
async fn times_out_false_when_rows_insufficient() {
    let live = live_blocks();
    let writer = live.clone();

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        writer.apply_changes(vec![created("a", "only one")]);
    });

    let caught_up = live
        .wait_until(has_count(2), Duration::from_millis(300))
        .await;
    assert!(
        !caught_up,
        "only one of two expected blocks arrived; must time out to false"
    );
    assert_eq!(live.read().len(), 1);
}
