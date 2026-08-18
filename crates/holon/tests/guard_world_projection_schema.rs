//! ADR 0031 Increment 3a — schema honesty for the guard gate's evaluation
//! substrate.
//!
//! `CurrentSchema` (the ADR 0024 spike's shape) does NOT describe the
//! production projection: the real block relation has no `name` column and the
//! real `clock` holds one row per grain. `ProjectionSchema` is the prod
//! implementor; these tests run its compiled SQL against the REAL schema
//! modules, so a projection change that breaks the gate reds here rather than
//! silently fail-opening at dispatch.

use std::sync::Arc;

use holon::api::guard_world::GuardQuery;
use holon::api::guard_world::GuardWorld;
use holon::api::guard_world::SqlGuardWorld;
use holon::storage::schema_module::SchemaModule;
use holon::storage::turso::DbHandle;
use holon::storage::turso::TursoBackend;
use holon_api::pattern::Guard;
use holon_turso::schema_modules::BlockMatviewSchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;
use holon_turso::schema_modules::CoreSchemaModule;
use tempfile::TempDir;
use tokio::sync::broadcast;

const ROOT_PARENT: &str = "sentinel:no_parent";

async fn boot(dir: &TempDir) -> DbHandle {
    let db = TursoBackend::open_database(&dir.path().join("guard.db")).expect("open db");
    let (cdc_tx, _rx) = broadcast::channel(1024);
    let (backend, handle) = TursoBackend::new(db, cdc_tx).expect("backend");
    std::mem::forget(backend);
    handle
        .execute_ddl("PRAGMA foreign_keys = ON")
        .await
        .unwrap();
    CoreSchemaModule.ensure_schema(&handle).await.unwrap();
    BlockSchemaModule.ensure_schema(&handle).await.unwrap();
    BlockMatviewSchemaModule
        .ensure_schema(&handle)
        .await
        .unwrap();
    handle
}

async fn block(handle: &DbHandle, id: &str, parent: &str, content: &str) {
    handle
        .execute(
            "INSERT INTO block_raw (id, parent_id, content) VALUES (?, ?, ?)",
            vec![
                turso::Value::Text(id.into()),
                turso::Value::Text(parent.into()),
                turso::Value::Text(content.into()),
            ],
        )
        .await
        .expect("insert block");
}

async fn tag(handle: &DbHandle, id: &str, tag: &str) {
    handle
        .execute(
            "INSERT INTO block_tags (block_id, tag) VALUES (?, ?)",
            vec![
                turso::Value::Text(id.into()),
                turso::Value::Text(tag.into()),
            ],
        )
        .await
        .expect("insert tag");
}

async fn today(handle: &DbHandle) -> String {
    let rows = handle
        .query(
            "SELECT today FROM clock WHERE grain = 'day'",
            std::collections::HashMap::new(),
        )
        .await
        .expect("clock query");
    rows[0]
        .get("today")
        .and_then(|v| v.as_string())
        .expect("day grain row seeded by CoreSchemaModule")
        .to_string()
}

async fn holds(world: &Arc<dyn GuardWorld>, src: &str, subject: Option<&str>) -> bool {
    let guard = Guard::parse(src).unwrap_or_else(|e| panic!("guard {src:?} must parse: {e}"));
    let query = GuardQuery::bind(&guard, subject).expect("bindable");
    world.guard_holds(&query).await.expect("evaluates")
}

/// A block-driven guard reads the real `block` matview and its `block_tags`
/// junction, and binds the SUBJECT ONLY — a satisfied sibling does not pass it.
#[tokio::test(flavor = "multi_thread")]
async fn block_guard_binds_the_subject_against_the_real_projection() {
    let dir = TempDir::new().unwrap();
    let handle = boot(&dir).await;
    block(&handle, "b:page", ROOT_PARENT, "Journals").await;
    tag(&handle, "b:page", "Page").await;
    block(&handle, "b:plain", "b:page", "milk").await;
    let world: Arc<dyn GuardWorld> = Arc::new(SqlGuardWorld::new(handle));

    assert!(holds(&world, "has_tag(\"Page\")", Some("b:page")).await);
    assert!(!holds(&world, "has_tag(\"Page\")", Some("b:plain")).await);
    assert!(!holds(&world, "has_tag(\"Page\")", Some("b:absent")).await);
    assert!(holds(&world, "parent(has_tag(\"Page\"))", Some("b:plain")).await);
    assert!(!holds(&world, "parent(has_tag(\"Page\"))", Some("b:page")).await);
}

/// A path pattern matches on the block's NAME, which in production is the
/// `content` column. Reds if `ProjectionSchema::name_column` regresses to the
/// spike's `name`.
#[tokio::test(flavor = "multi_thread")]
async fn path_patterns_match_the_content_column() {
    let dir = TempDir::new().unwrap();
    let handle = boot(&dir).await;
    let day = today(&handle).await;
    block(&handle, "b:journals", ROOT_PARENT, "Journals").await;
    let world: Arc<dyn GuardWorld> = Arc::new(SqlGuardWorld::new(handle.clone()));

    assert!(!holds(&world, "block_exists(\"Journals/{today}\")", None).await);
    block(&handle, "b:day", "b:journals", &day).await;
    assert!(holds(&world, "block_exists(\"Journals/{today}\")", None).await);
}

/// R4 — the gate's per-dispatch cost, REPORT-ONLY.
///
/// The budget is ~2ms per declared dispatch (SLO p95 interaction→visible
/// < 200ms). It is NOT met against the `block` matview: measured ~20ms per
/// dispatch over 1001 blocks. No production op declares a guard yet, so
/// nothing pays this today; admitting the first op needs the substrate fixed
/// first. `probe_guard_cost_attribution` locates the cost.
#[tokio::test(flavor = "multi_thread")]
async fn measure_bound_guard_cost_per_dispatch() {
    let dir = TempDir::new().unwrap();
    let handle = boot(&dir).await;
    block(&handle, "b:page", ROOT_PARENT, "Journals").await;
    tag(&handle, "b:page", "Page").await;
    for i in 0..1000 {
        block(&handle, &format!("b:{i}"), "b:page", &format!("row {i}")).await;
    }
    tag(&handle, "b:500", "Page").await;
    let world: Arc<dyn GuardWorld> = Arc::new(SqlGuardWorld::new(handle));

    let guard = Guard::parse("has_tag(\"Page\")").expect("parses");
    let started = std::time::Instant::now();
    const RUNS: u32 = 200;
    for i in 0..RUNS {
        let subject = format!("b:{}", i % 1000);
        let query = GuardQuery::bind(&guard, Some(&subject)).expect("bindable");
        world.guard_holds(&query).await.expect("evaluates");
    }
    let per_dispatch = started.elapsed() / RUNS;
    println!("guard evaluation: {per_dispatch:?} per declared dispatch (1001 blocks)");
}

/// Diagnostic probe (R4), report-only: attributes the bound guard's cost.
///
/// The finding it pins: `block_raw` point lookups are ~0.1ms (PRIMARY KEY),
/// the `block` matview is ~7ms for the SAME lookup, and Turso does not flatten
/// a derived table — so wrapping [`Guard::to_sql`] cannot recover the index.
/// A gate on the interaction path needs the subject predicate compiled INTO
/// the WHERE over an indexed relation.
#[tokio::test(flavor = "multi_thread")]
async fn probe_guard_cost_attribution() {
    let dir = TempDir::new().unwrap();
    let handle = boot(&dir).await;
    block(&handle, "b:page", ROOT_PARENT, "Journals").await;
    tag(&handle, "b:page", "Page").await;
    for i in 0..1000 {
        block(&handle, &format!("b:{i}"), "b:page", &format!("row {i}")).await;
    }
    const RUNS: u32 = 100;
    let guard = Guard::parse("has_tag(\"Page\")").expect("parses");
    let wrapped = guard
        .to_sql_bound(&holon::api::guard_world::ProjectionSchema)
        .expect("a block/clock guard compiles");
    let flat = "SELECT b.id AS binding FROM block b WHERE b.id = ?1 AND EXISTS (SELECT 1 FROM \
                block_tags bt WHERE bt.block_id = b.id AND bt.tag = 'Page') LIMIT 1";
    for (label, sql) in [
        ("trivial", "SELECT 1 AS binding WHERE ?1 IS NOT NULL"),
        (
            "matview_id_only",
            "SELECT b.id AS binding FROM block b WHERE b.id = ?1 LIMIT 1",
        ),
        (
            "raw_id_only",
            "SELECT b.id AS binding FROM block_raw b WHERE b.id = ?1 LIMIT 1",
        ),
        (
            "raw_with_tag",
            "SELECT b.id AS binding FROM block_raw b WHERE b.id = ?1 AND EXISTS (SELECT 1 FROM \
             block_tags bt WHERE bt.block_id = b.id AND bt.tag = 'Page') LIMIT 1",
        ),
        (
            "raw_no_sentinel_subquery",
            "SELECT b.id AS binding FROM (SELECT * FROM block_raw WHERE id != \
             'sentinel:no_parent') b WHERE b.id = ?1 AND EXISTS (SELECT 1 FROM block_tags bt \
             WHERE bt.block_id = b.id AND bt.tag = 'Page') LIMIT 1",
        ),
        ("flat", flat),
        ("wrapped", wrapped.as_str()),
    ] {
        let started = std::time::Instant::now();
        for i in 0..RUNS {
            handle
                .query_positional(sql, vec![turso::Value::Text(format!("b:{}", i % 1000))])
                .await
                .unwrap_or_else(|e| panic!("{label} failed:\n{sql}\n{e}"));
        }
        println!("{label}: {:?} per query", started.elapsed() / RUNS);
    }
}

/// `clock` is one row PER GRAIN in production. `{today}` must read the day
/// row: an hour-grain label (`YYYY-MM-DDThh`) names no journal page, so a
/// grain-blind read would make the inhibitor spuriously fire.
#[tokio::test(flavor = "multi_thread")]
async fn today_reads_the_day_grain_only() {
    let dir = TempDir::new().unwrap();
    let handle = boot(&dir).await;
    let day = today(&handle).await;
    handle
        .execute(
            "INSERT INTO clock (grain, today, epoch_day, updated_at) VALUES ('hour', ?, 1, '')",
            vec![turso::Value::Text(format!("{day}T09"))],
        )
        .await
        .expect("seed hour grain");
    block(&handle, "b:journals", ROOT_PARENT, "Journals").await;
    block(&handle, "b:day", "b:journals", &day).await;
    let world: Arc<dyn GuardWorld> = Arc::new(SqlGuardWorld::new(handle));

    assert!(holds(&world, "block_exists(\"Journals/{today}\")", None).await);
    assert!(!holds(&world, "not block_exists(\"Journals/{today}\")", None).await);
}
