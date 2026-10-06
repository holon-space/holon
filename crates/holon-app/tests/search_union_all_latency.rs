//! Keystroke latency of quick-open search, at the real vault's shape and at
//! 20k / 100k entities spread over five searchable types.
//!
//! Prints p50 / p95 per keystroke prefix; asserts nothing. Run with
//! `--release -- --ignored --nocapture`.

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use fluxdi::Module;
use holon_api::query_engine::QueryEngine;
use holon_loro_wiring::EventInfraModule;

const TYPES: [&str; 4] = ["gen_a", "gen_b", "gen_c", "gen_d"];
const RUNS: usize = 20;
const WORDS: [&str; 12] = [
    "lorem",
    "ipsum",
    "Linsensuppe",
    "kochen",
    "Compass",
    "review",
    "planung",
    "garden",
    "notes",
    "meeting",
    "Übung",
    "budget",
];

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create runtime"),
    )
}

async fn fresh_engine(
    db_path: std::path::PathBuf,
) -> (
    Arc<holon::api::BackendEngine>,
    Arc<holon_profiles::TypeRegistry>,
) {
    let (engine, types) = holon::di::create_backend_engine_with_extras(
        db_path,
        |injector| {
            EventInfraModule
                .configure(injector)
                .map_err(|e| anyhow::anyhow!("configure EventInfraModule: {e}"))?;
            injector.provide_into_set::<dyn holon_core::OperationProvider>(fluxdi::Provider::root(
                |resolver| {
                    let db = resolver
                        .resolve::<dyn holon::di::DbHandleProvider>()
                        .handle();
                    Arc::new(holon::core::SqlOperationProvider::new(
                        db,
                        holon::storage::BLOCK_WRITE_TABLE.to_string(),
                        "block".to_string(),
                        "block".to_string(),
                    )) as Arc<dyn holon_core::OperationProvider>
                },
            ));
            Ok(())
        },
        |injector| async move { injector.resolve::<holon_profiles::TypeRegistry>() },
    )
    .await
    .expect("fresh-db lazy DI graph must build");
    (engine, types)
}

/// A searchable, soft-deleting type over `{name}_raw`.
fn searchable_type(name: &str) -> holon_api::TypeDefinition {
    let field = |f: &str| holon_api::computation::FieldIdent::try_from(f.to_string()).unwrap();
    let mut type_def = holon_api::TypeDefinition::new(
        name,
        vec![
            holon_api::FieldSchema::new("id", "TEXT").primary_key(),
            holon_api::FieldSchema::new("name", "TEXT"),
            holon_api::FieldSchema::new("note", "TEXT").nullable(),
            holon_api::FieldSchema::new("deleted_at", "TEXT").nullable(),
        ],
    );
    type_def.soft_delete = Some(holon_api::entity::SoftDelete {
        tombstone_field: "deleted_at".to_string(),
        retention_days: 30,
    });
    type_def.services = Some(holon_api::TypeServices {
        title: field("name"),
        searchable: vec![field("name"), field("note")],
        linkable: true,
        embeddable: false,
        dense_view: None,
        hierarchy: None,
        rich_text: Vec::new(),
    });
    type_def
}

fn text(i: usize) -> String {
    let w = |k: usize| WORDS[(i * 7 + k * 13) % WORDS.len()];
    format!(
        "{} {} {} item {i}\n{} {} line two",
        w(0),
        w(1),
        w(2),
        w(3),
        w(4)
    )
}

async fn seed(
    db: &holon::storage::turso::DbHandle,
    types: &holon_profiles::TypeRegistry,
    blocks: usize,
    pages: usize,
    typed: usize,
) {
    for chunk in (0..blocks).collect::<Vec<_>>().chunks(500) {
        let values: Vec<String> = chunk
            .iter()
            .map(|i| format!("('block:f{i}', 'sentinel:no_parent', '{}')", text(*i)))
            .collect();
        db.execute_values(
            &format!(
                "INSERT INTO block_raw (id, parent_id, content) VALUES {}",
                values.join(", ")
            ),
            vec![],
        )
        .await
        .expect("insert blocks");
    }
    for chunk in (0..pages).collect::<Vec<_>>().chunks(500) {
        let values: Vec<String> = chunk
            .iter()
            .map(|i| format!("('block:f{i}', 'Page')"))
            .collect();
        db.execute_values(
            &format!(
                "INSERT INTO block_tags (block_id, tag) VALUES {}",
                values.join(", ")
            ),
            vec![],
        )
        .await
        .expect("tag pages");
    }
    for name in TYPES {
        types
            .register(searchable_type(name))
            .expect("register searchable type");
        let table = format!("{name}_raw");
        db.execute_ddl(&format!(
            "CREATE TABLE {table} (id TEXT PRIMARY KEY, name TEXT NOT NULL, note TEXT, deleted_at \
             TEXT)"
        ))
        .await
        .expect("create typed table");
        for chunk in (0..typed).collect::<Vec<_>>().chunks(500) {
            let values: Vec<String> = chunk
                .iter()
                .map(|i| format!("('{table}{i}', '{}', '{}', NULL)", text(i + 3), text(i + 5)))
                .collect();
            db.execute_values(
                &format!(
                    "INSERT INTO {table} (id, name, note, deleted_at) VALUES {}",
                    values.join(", ")
                ),
                vec![],
            )
            .await
            .expect("insert typed rows");
        }
    }
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

async fn measure(label: &str, blocks: usize, pages: usize, typed: usize) {
    let dir = tempfile::tempdir().expect("tempdir");
    let (engine, types) = fresh_engine(dir.path().join("probe.db")).await;
    seed(engine.db_handle(), &types, blocks, pages, typed).await;
    let entities = blocks + typed * TYPES.len();
    engine.quick_open_search("warmup").await.expect("warm-up");
    let mut all = Vec::new();
    for q in ["S", "Su", "Sup", "Supp", "Suppe", "zzz"] {
        let mut times = Vec::with_capacity(RUNS);
        let mut rows = 0;
        for _ in 0..RUNS {
            let t0 = Instant::now();
            let hits = engine
                .quick_open_search(q)
                .await
                .expect("quick-open search");
            times.push(t0.elapsed());
            rows = hits.pages.len() + hits.content.len();
        }
        times.sort();
        println!(
            "SEARCH-PROBE {label} entities={entities} q={q:?} rows={rows} p50={:?} p95={:?}",
            percentile(&times, 0.5),
            percentile(&times, 0.95)
        );
        all.extend(times);
    }
    all.sort();
    println!(
        "SEARCH-PROBE {label} entities={entities} ALL p50={:?} p95={:?} max={:?}",
        percentile(&all, 0.5),
        percentile(&all, 0.95),
        all.last().unwrap()
    );
}

#[test]
#[ignore = "release-only wall-clock measurement; run with --ignored"]
fn union_all_search_latency() {
    let rt = runtime();
    rt.block_on(async {
        measure("vault", 2257, 129, 0).await;
        measure("20k", 4000, 200, 4000).await;
        measure("100k", 20000, 1000, 20000).await;
    });
}
