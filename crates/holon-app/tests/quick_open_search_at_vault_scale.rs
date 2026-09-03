//! Deterministic regression pin for quick-open search at real-vault scale.
//!
//! Covers the three 2026-09-03 dogfood escapes the keystone's `Search`
//! transition cannot reach: the per-keystroke LATENCY that made every response
//! lose the newest-response race (2257 blocks / 129 pages, the shape of
//! Martin's vault), non-ASCII case folding, and pattern metacharacters.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use fluxdi::Module;
use fluxdi::Provider;
use holon_api::EntityName;
use holon_api::OpOrigin;
use holon_api::Value;
use holon_api::query_engine::QueryEngine;
use holon_loro_wiring::EventInfraModule;

/// Per-keystroke budget. The overlay drops any response a newer keystroke has
/// overtaken, so a search slower than the typing cadence renders permanently
/// empty — the "no matches for every query" escape. A 1–2 character query costs
/// ~1 s from per-row materialisation (OPEN entry
/// `a-broad-search-query-costs-a-second-at-vault-scale`); the JOIN this test
/// pins costs ≥6 s. 3 s is the loud-failure line that still catches that JOIN
/// regression without firing on the known broad-query cost under build load.
const KEYSTROKE_BUDGET: Duration = Duration::from_millis(3000);

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
    Arc<dyn holon_core::block_ordering::BlockOrdering>,
) {
    holon::di::create_backend_engine_with_extras(
        db_path,
        |injector| {
            EventInfraModule
                .configure(injector)
                .map_err(|e| anyhow::anyhow!("configure EventInfraModule: {e}"))?;
            injector.provide_into_set::<dyn holon_core::OperationProvider>(Provider::root(
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
        |injector| async move {
            injector
                .resolve_async::<dyn holon_core::block_ordering::BlockOrdering>()
                .await
        },
    )
    .await
    .expect("fresh-db lazy DI graph must build")
}

async fn create_block(engine: &holon::api::BackendEngine, local: &str, content: &str, page: bool) {
    let block_entity: EntityName = "block".to_string().into();
    let mut p: holon_api::StorageEntity = HashMap::new();
    p.insert("id".into(), Value::String(format!("block:{local}")));
    p.insert("content".into(), Value::String(content.to_string()));
    if page {
        p.insert(
            "tags".into(),
            Value::Array(vec![Value::String("Page".to_string())]),
        );
    }
    engine
        .execute_operation(&block_entity, "create", p, OpOrigin::User)
        .await
        .expect("create block");
}

/// Ids of the hits, so an assertion names what it expected rather than a count.
fn ids(candidates: &[holon_api::LinkCandidate]) -> Vec<String> {
    candidates.iter().map(|c| c.id.to_string()).collect()
}

#[test]
fn quick_open_search_at_vault_scale() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let (engine, _ordering) = fresh_engine(dir.path().join("fresh.db")).await;

        // The named fixtures: a German content block, a page, a block holding
        // both wildcard metacharacters literally.
        create_block(&engine, "linsensuppe", "Übung Linsensuppe kochen", false).await;
        create_block(&engine, "compass", "Compass", true).await;
        create_block(&engine, "meta", "discount 100% for a_b pairs", false).await;

        // Scale the fixture to the shape of the real vault (2257 blocks, 129
        // pages) with synthetic content only.
        let db = engine.db_handle();
        for i in 0..2254u32 {
            db.execute_values(
                &format!(
                    "INSERT INTO block_raw (id, parent_id, content) VALUES ('block:f{i}', \
                     'sentinel:no_parent', 'filler block number {i} lorem ipsum')"
                ),
                vec![],
            )
            .await
            .expect("insert filler block");
        }
        for i in 0..128u32 {
            db.execute_values(
                &format!("INSERT INTO block_tags (block_id, tag) VALUES ('block:f{i}', 'Page')"),
                vec![],
            )
            .await
            .expect("tag filler page");
        }
        db.execute_values(
            "INSERT INTO block_tags (block_id, tag) VALUES ('block:compass', 'Page')",
            vec![],
        )
        .await
        .expect("tag compass page");

        // One untimed search first: the budget below measures steady-state
        // typing, not the first-query cost of planning and a cold page cache.
        engine.quick_open_search("warmup").await.expect("warm-up");

        // Latency, over the keystroke prefixes of a word actually in the vault.
        for q in ["S", "Su", "Sup", "Supp", "Suppe"] {
            let t0 = Instant::now();
            let hits = engine
                .quick_open_search(q)
                .await
                .expect("quick_open_search must not error");
            let elapsed = t0.elapsed();
            println!(
                "keystroke {q:?}: pages={} content={} in {elapsed:?}",
                hits.pages.len(),
                hits.content.len()
            );
            assert!(
                elapsed < KEYSTROKE_BUDGET,
                "quick_open_search({q:?}) took {elapsed:?}, over the {KEYSTROKE_BUDGET:?} \
                 keystroke budget — a search this slow is overtaken by the next keystroke and \
                 the overlay renders 'No matches' forever"
            );
        }

        // Substring match over block content, and over page names.
        let suppe = engine.quick_open_search("Suppe").await.expect("Suppe");
        assert_eq!(
            ids(&suppe.content),
            vec!["block:linsensuppe"],
            "the content section must hold exactly the block containing 'Suppe'"
        );
        let compass = engine.quick_open_search("Compass").await.expect("Compass");
        assert_eq!(
            ids(&compass.pages),
            vec!["block:compass"],
            "the pages section must hold the Compass page"
        );
        assert!(
            compass.content.is_empty(),
            "a Page-tagged block belongs to the pages section only, got {:?}",
            ids(&compass.content)
        );

        // Unicode simple case folding: the capital the German word is actually
        // typed with must reach the stored lower-case letter, and the reverse.
        for q in ["Übung", "übung", "ÜBUNG", "üBuNg"] {
            let hits = engine.quick_open_search(q).await.expect("umlaut query");
            assert_eq!(
                ids(&hits.content),
                vec!["block:linsensuppe"],
                "quick_open_search({q:?}) must fold the umlaut and find 'Übung Linsensuppe kochen'"
            );
        }

        // Pattern metacharacters are literal: they match themselves and nothing
        // else, on a vault where a bare wildcard would match every block.
        for (q, expected) in [
            ("100%", vec!["block:meta"]),
            ("a_b", vec!["block:meta"]),
            ("%", vec!["block:meta"]),
            ("_", vec!["block:meta"]),
        ] {
            let hits = engine.quick_open_search(q).await.expect("metachar query");
            assert_eq!(
                ids(&hits.content),
                expected,
                "quick_open_search({q:?}) must treat the metacharacter as a literal; pages={:?}",
                ids(&hits.pages)
            );
        }
        let nothing = engine.quick_open_search("x%x").await.expect("x%x");
        assert!(
            nothing.pages.is_empty() && nothing.content.is_empty(),
            "a query whose literal spelling is in no block must find nothing, got {:?} / {:?}",
            ids(&nothing.pages),
            ids(&nothing.content)
        );

        // The empty-query rule the overlay promises.
        for q in ["", "   "] {
            let hits = engine.quick_open_search(q).await.expect("empty query");
            assert!(
                hits.pages.is_empty() && hits.content.is_empty(),
                "an empty query promises no matches, got {:?} / {:?}",
                ids(&hits.pages),
                ids(&hits.content)
            );
        }

        // A broken predicate must surface as an `Err` the overlay can render,
        // never as an empty result set that reads as "no matches".
        let broken = engine
            .execute_query(
                "SELECT * FROM no_such_table".to_string(),
                HashMap::new(),
                None,
            )
            .await;
        assert!(
            broken.is_err(),
            "a failing search query must return Err, not an empty row set"
        );
    });
}
