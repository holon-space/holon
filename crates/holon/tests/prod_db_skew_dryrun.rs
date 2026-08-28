//! Dry-run against a COPY of a real database that carries matview version
//! skew. Ignored by default and driven by `HOLON_DRYRUN_DB`, because it needs a
//! database no repository can carry.
//!
//! Run with:
//!   HOLON_DRYRUN_DB=/path/to/copy/holon.db \
//!     cargo test -p holon --test prod_db_skew_dryrun -- --ignored --nocapture
//!
//! The copy is written to, so point it at a copy of the `.db` + `.db-wal` pair,
//! never at the original.

use std::collections::HashMap;

use holon_api::Value;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockHierarchySchemaModule;
use holon_turso::schema_modules::BlockMatviewSchemaModule;
use holon_turso::schema_modules::BlockRequirementEdgesSchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;
use holon_turso::schema_modules::CoreSchemaModule;
use holon_turso::schema_modules::NavigationSchemaModule;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;
use tokio::sync::broadcast;

async fn view_definitions(handle: &DbHandle) -> Vec<(String, String)> {
    handle
        .query(
            "SELECT name, sql FROM sqlite_master WHERE type='view' ORDER BY name",
            HashMap::new(),
        )
        .await
        .expect("read sqlite_master")
        .iter()
        .map(|row| {
            let name = match row.get("name") {
                Some(Value::String(s)) => s.clone(),
                other => format!("{other:?}"),
            };
            let sql = match row.get("sql") {
                Some(Value::String(s)) => s.clone(),
                other => format!("{other:?}"),
            };
            (name, sql)
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs HOLON_DRYRUN_DB pointing at a copy of a real database"]
async fn version_skewed_database_boots_and_repairs_itself() {
    let path = std::env::var("HOLON_DRYRUN_DB").expect("set HOLON_DRYRUN_DB");
    let path = std::path::PathBuf::from(path);

    let db = TursoBackend::open_database(&path).expect("open the version-skewed database");
    let (backend, handle) = TursoBackend::new(db, broadcast::channel(64).0).expect("backend");

    // The repair drops and recreates a projection; the source of truth must not
    // move. Counted before and after so a silent loss cannot pass.
    let count_blocks = |handle: DbHandle| async move {
        let rows = handle
            .query("SELECT COUNT(*) AS n FROM block_raw", HashMap::new())
            .await
            .expect("count block_raw");
        match rows.first().and_then(|r| r.get("n")) {
            Some(Value::Integer(n)) => *n,
            other => panic!("unexpected count: {other:?}"),
        }
    };
    let blocks_before = count_blocks(handle.clone()).await;
    println!("block_raw rows before: {blocks_before}");
    assert!(blocks_before > 0, "the copy has no blocks to lose");

    println!("--- views BEFORE the startup schema modules ---");
    let before = view_definitions(&handle).await;
    for (name, sql) in &before {
        println!("{name}: {sql}");
    }

    // The startup order production uses (crates/holon/src/di/schema_providers.rs),
    // flattened: every module below is reachable from the block matview chain.
    let modules: Vec<(&str, &dyn SchemaModule)> = vec![
        ("core", &CoreSchemaModule),
        ("block", &BlockSchemaModule),
        ("block_matview", &BlockMatviewSchemaModule),
        ("block_hierarchy", &BlockHierarchySchemaModule),
        (
            "block_requirement_edges",
            &BlockRequirementEdgesSchemaModule,
        ),
        ("navigation", &NavigationSchemaModule),
    ];
    for (label, module) in modules {
        module
            .ensure_schema(&handle)
            .await
            .unwrap_or_else(|e| panic!("[{label}] ensure_schema failed: {e:#}"));
        println!("[{label}] ensure_schema ok");
    }

    println!("--- views AFTER the startup schema modules ---");
    let after = view_definitions(&handle).await;
    for (name, sql) in &after {
        println!("{name}: {sql}");
    }

    let mut counts: HashMap<&str, usize> = HashMap::new();
    for (name, _) in &after {
        *counts.entry(name.as_str()).or_default() += 1;
    }
    let duplicated: Vec<_> = counts.iter().filter(|(_, n)| **n > 1).collect();
    assert!(
        duplicated.is_empty(),
        "a view name occurs more than once in sqlite_master: {duplicated:?}"
    );

    let block: Vec<_> = after.iter().filter(|(name, _)| name == "block").collect();
    assert_eq!(
        block.len(),
        1,
        "expected exactly one `block` view: {block:?}"
    );
    assert!(
        !block[0].1.contains("depth"),
        "the stale `block` definition survived the startup reconcile: {}",
        block[0].1
    );

    let blocks_after = count_blocks(handle.clone()).await;
    println!("block_raw rows after: {blocks_after}");
    assert_eq!(
        blocks_before, blocks_after,
        "the repair lost rows from the source of truth"
    );

    let projected = handle
        .query("SELECT COUNT(*) AS n FROM block", HashMap::new())
        .await
        .expect("count the repaired block matview");
    let projected = match projected.first().and_then(|r| r.get("n")) {
        Some(Value::Integer(n)) => *n,
        other => panic!("unexpected count: {other:?}"),
    };
    println!("block matview rows after repair: {projected}");
    assert_eq!(
        projected,
        blocks_after - 1,
        "the repaired matview should project every block_raw row except the \
         `sentinel:no_parent` row its WHERE clause excludes"
    );

    handle.shutdown().await.expect("shutdown");
    drop(backend);

    // The repaired database must open cleanly on the next launch.
    let reopened = TursoBackend::open_database(&path).expect("reopen after repair");
    drop(reopened);
    println!("reopen after repair: ok");
}
