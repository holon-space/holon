//! Selection tests of the combined SQL+Loro slice. The slice's job is to wire
//! **both** `SutSqlProjection` (SQL) and `SutLoroTaskState` (Loro) so the shared
//! catalog's `inv-task-state-storage-coherence` selects and runs over two real
//! stores. The invariant body, its `Needs`, and the fixture-driven catch triad
//! all live in `composed/` (reused, not re-authored here); these tests only
//! prove the *real* combined SUT lights it up — a positive (stores agree) and a
//! catch (stores diverge). No bespoke generator: the engine builder is reused
//! from `sql_slice::builders`, and the mutation-driven coverage of the catalog
//! lives in the per-store slices.

use std::collections::HashMap;

use holon_api::Value;
use holon_api::repository::{CoreOperations, Lifecycle};
use holon_loro::LoroBackend;

use crate::pbt::composed::fixtures::*;
use crate::pbt::loro_slice::components::LoroBackendComponent;
use crate::pbt::sql_loro_slice::builders::sql_loro_wide;
use crate::pbt::sql_slice::builders::new_sql_engine;
use crate::pbt::sql_slice::components::SqlProjectionComponent;

fn task_state_props(state: &str) -> HashMap<String, Value> {
    let mut props = HashMap::new();
    props.insert("task_state".to_string(), Value::String(state.to_string()));
    props
}

/// Seed the SAME block (same explicit id) into both stores, optionally with a
/// `task_state` property — SQL via `set_field`, Loro via `update_block_properties`.
async fn seed_block(
    sql: &SqlProjectionComponent,
    loro: &LoroBackend,
    id: &EntityUri,
    content: &str,
    task_state: Option<&str>,
) {
    sql.create_block(id, &EntityUri::no_parent(), content).await;
    loro.create_block(
        EntityUri::no_parent(),
        BlockContent::text(content),
        Some(id.clone()),
    )
    .await
    .expect("loro create_block");
    if let Some(ts) = task_state {
        sql.set_task_state(id, ts).await;
        loro.update_block_properties(id.as_str(), &task_state_props(ts))
            .await
            .expect("loro update_block_properties");
    }
}

async fn new_loro() -> LoroBackend {
    LoroBackend::create_new("sql-loro-slice".to_string())
        .await
        .expect("create_new LoroBackend")
}

/// Positive: two real stores seeded with the same blocks ⇒
/// `inv-task-state-storage-coherence` is selected (both `SutSqlProjection` and
/// `SutLoroTaskState` are wired) and passes.
#[tokio::test(flavor = "multi_thread")]
async fn sql_loro_slice_task_state_coherent_across_stores() {
    let engine = new_sql_engine().await;
    let loro = new_loro().await;
    let sql = SqlProjectionComponent::new(engine);

    seed_block(&sql, &loro, &uri("block:a"), "alpha", Some("TODO")).await;
    seed_block(&sql, &loro, &uri("block:b"), "beta", Some("DONE")).await;
    // A block with no task_state — both projections must agree it's `None`.
    seed_block(&sql, &loro, &uri("block:c"), "gamma", None).await;

    let sut = sql_loro_wide(sql, LoroBackendComponent::new(loro));
    let report = run_selected(&composed_invariant_catalog(), &sut, &CapMap::new()).await;

    assert!(
        report
            .ran_ids()
            .contains(&"inv-task-state-storage-coherence"),
        "the combined slice wires both SutSqlProjection and SutLoroTaskState, so \
         the coherence invariant must be selected; ran={:?} deselected={:?}",
        report.ran_ids(),
        report.deselected,
    );
    assert!(
        report.failures().is_empty(),
        "both stores hold the same task_state, so coherence holds: {:?}",
        report.failures(),
    );
}

/// Catch over real stores: the SAME block carries `TODO` in SQL but `DONE` in
/// Loro — a desync the synced pair can't produce, hand-built here by writing the
/// two stores independently. The coherence invariant fires.
#[tokio::test(flavor = "multi_thread")]
async fn sql_loro_slice_catches_task_state_divergence() {
    let engine = new_sql_engine().await;
    let loro = new_loro().await;
    let sql = SqlProjectionComponent::new(engine);

    let a = uri("block:a");
    sql.create_block(&a, &EntityUri::no_parent(), "alpha").await;
    loro.create_block(
        EntityUri::no_parent(),
        BlockContent::text("alpha"),
        Some(a.clone()),
    )
    .await
    .expect("loro create_block");
    sql.set_task_state(&a, "TODO").await;
    loro.update_block_properties(a.as_str(), &task_state_props("DONE"))
        .await
        .expect("loro update_block_properties");

    let sut = sql_loro_wide(sql, LoroBackendComponent::new(loro));
    let report = run_selected(&composed_invariant_catalog(), &sut, &CapMap::new()).await;

    let failures = report.failures();
    assert!(
        failures
            .iter()
            .any(|(id, _)| *id == "inv-task-state-storage-coherence"),
        "a real SQL↔Loro task_state divergence must be caught; failures={failures:?}",
    );
}
