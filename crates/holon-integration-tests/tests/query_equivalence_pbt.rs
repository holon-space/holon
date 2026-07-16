//! Query-equivalence PBT — the foundational de-risk for making `TestQuery` the
//! single source of truth for LAYOUT queries.
//!
//! Generates a random block forest + a random [`QuerySource`], then asserts the
//! block-id set returned by the **real backend** for that source — compiled to
//! each applicable language (PRQL / SQL / GQL) — equals
//! [`TestQuery::rendered_block_ids`] (the reference model's `evaluate`). If all
//! languages agree with the reference, they agree with each other, so the
//! reference's rendered set is a faithful stand-in for what the SUT renders.
//!
//! This validates the `evaluate` semantics BEFORE they are wired into the
//! state-machine PBT to replace the `is_descendant_of_any(focus_roots)` proxy.
//!
//! Run: `cargo test -p holon-integration-tests --features pbt --test
//! query_equivalence_pbt`
//!
//! @pbt kind harness
//! @pbt covers testquery-equivalence — TestQuery vs production query equivalence for LAYOUT queries
//! @pbt overlaps general_e2e_composed_pbt — kept: distinct oracle, isolation

#![cfg(feature = "pbt")]

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;

use holon::api::backend_engine::BackendEngine;
use holon::di::test_helpers::create_test_engine;
use holon::storage::BLOCK_WRITE_TABLE;
use holon_api::EntityUri;
use holon_api::QueryContext;
use holon_api::QueryLanguage;
use holon_api::block::Block;
use holon_integration_tests::pbt::query::QuerySource;
use holon_integration_tests::pbt::query::TestQuery;
use proptest::prelude::*;
use tokio::runtime::Runtime;
use tokio_stream::StreamExt;

/// A generated block: its parent (an earlier index, or `None` for a root) and
/// whether it is a `source`-typed block (filtered out by `children`/traversal
/// virtual tables).
#[derive(Debug, Clone)]
struct GenBlock {
    parent: Option<usize>,
    is_source: bool,
}

/// Strategy: 2..=8 blocks forming a forest. Block `i`'s parent selector maps to
/// some earlier index `j < i` (a child) or `i` itself (rendered as a root).
fn forest_strategy() -> impl Strategy<Value = Vec<GenBlock>> {
    proptest::collection::vec((any::<u8>(), proptest::bool::weighted(0.25)), 2..=8).prop_map(
        |raw| {
            raw.into_iter()
                .enumerate()
                .map(|(i, (sel, is_source))| {
                    let p = (sel as usize) % (i + 1);
                    GenBlock {
                        parent: if p == i { None } else { Some(p) },
                        is_source,
                    }
                })
                .collect()
        },
    )
}

fn block_uri(i: usize) -> EntityUri {
    EntityUri::block(&format!("b{i}"))
}

/// Build the reference block map (what `TestQuery::evaluate` reads).
fn build_reference(forest: &[GenBlock]) -> BTreeMap<EntityUri, Block> {
    let mut blocks = BTreeMap::new();
    for (i, gb) in forest.iter().enumerate() {
        let uri = block_uri(i);
        let parent = match gb.parent {
            Some(j) => block_uri(j),
            None => EntityUri::no_parent(),
        };
        let block = if gb.is_source {
            Block::new_source(uri.clone(), parent, "holon_sql", "SELECT 1")
        } else {
            Block::new_text(uri.clone(), parent, &format!("Block {i}"))
        };
        blocks.insert(uri, block);
    }
    blocks
}

/// Insert the forest into `block_raw` so the `block` matview hydrates it.
async fn seed_backend(engine: &BackendEngine, forest: &[GenBlock]) {
    let mut values = Vec::new();
    for (i, gb) in forest.iter().enumerate() {
        let id = format!("block:b{i}");
        let parent = match gb.parent {
            Some(j) => format!("'block:b{j}'"),
            None => "NULL".to_string(),
        };
        let ct = if gb.is_source { "source" } else { "text" };
        values.push(format!("('{id}', {parent}, 'Block {i}', '{ct}')"));
    }
    let sql = format!(
        "INSERT INTO {table} (id, parent_id, content, content_type) VALUES {vals}",
        table = BLOCK_WRITE_TABLE,
        vals = values.join(", "),
    );
    engine
        .db_handle()
        .execute(&sql, vec![])
        .await
        .expect("seed block_raw");
}

/// Run `source` compiled to `lang` against the backend; return the id set of
/// the first (initial) batch.
async fn backend_ids(
    engine: &BackendEngine,
    query: &TestQuery,
    lang: QueryLanguage,
    context: Option<QueryContext>,
) -> BTreeSet<String> {
    let (source, _) = query.compile_for(lang);
    let sql = engine
        .compile_to_sql(&source, lang)
        .unwrap_or_else(|e| panic!("compile {lang:?} `{source}`: {e}"));
    let stream = engine
        .query_and_watch(sql, HashMap::new(), context)
        .await
        .unwrap_or_else(|e| panic!("query_and_watch `{source}`: {e}"));
    tokio::pin!(stream);
    let initial = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
        .await
        .expect("initial batch within 5s")
        .expect("stream should not close");
    initial
        .inner
        .items
        .iter()
        .filter_map(|c| match &c.change {
            holon_api::streaming::Change::Created { data, .. } => {
                data.get("id").and_then(|v| v.as_string()).map(String::from)
            }
            _ => None,
        })
        .collect()
}

fn reference_ids(query: &TestQuery, blocks: &BTreeMap<EntityUri, Block>) -> BTreeSet<String> {
    query
        .rendered_block_ids(blocks, &BTreeMap::new())
        .into_iter()
        .map(|u| u.to_string())
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, ..ProptestConfig::default() })]

    #[test]
    fn all_blocks_matches_across_languages(forest in forest_strategy()) {
        let rt = Runtime::new().unwrap();
        let result: Result<(), TestCaseError> = rt.block_on(async {
            let engine = create_test_engine().await.map_err(|e| TestCaseError::fail(format!("engine: {e}")))?;
            seed_backend(&engine, &forest).await;
            let blocks = build_reference(&forest);
            let query = TestQuery::layout(QuerySource::AllBlocks);
            let expected = reference_ids(&query, &blocks);

            for lang in [QueryLanguage::HolonPrql, QueryLanguage::HolonSql, QueryLanguage::HolonGql] {
                let got = backend_ids(&engine, &query, lang, None).await;
                prop_assert_eq!(&got, &expected, "AllBlocks via {:?} diverged from reference", lang);
            }
            Ok(())
        });
        result?;
    }

    #[test]
    fn direct_children_matches_across_languages((forest, ctx_idx) in forest_strategy().prop_flat_map(|f| {
        let n = f.len();
        (Just(f), 0..n)
    })) {
        let rt = Runtime::new().unwrap();
        let result: Result<(), TestCaseError> = rt.block_on(async {
            let engine = create_test_engine().await.map_err(|e| TestCaseError::fail(format!("engine: {e}")))?;
            seed_backend(&engine, &forest).await;
            let blocks = build_reference(&forest);
            let context_uri = block_uri(ctx_idx);
            let query = TestQuery::layout(QuerySource::DirectChildren { context: context_uri.clone() });
            let expected = reference_ids(&query, &blocks);

            // PRQL `from children` binds the context at runtime; SQL/GQL embed
            // the literal, so the context arg is harmless for them.
            let context = Some(QueryContext::for_block(&context_uri, None));
            for lang in [QueryLanguage::HolonPrql, QueryLanguage::HolonSql, QueryLanguage::HolonGql] {
                let got = backend_ids(&engine, &query, lang, context.clone()).await;
                prop_assert_eq!(&got, &expected, "DirectChildren({}) via {:?} diverged from reference", context_uri, lang);
            }
            Ok(())
        });
        result?;
    }

    #[test]
    fn descendants_of_any_matches_gql(forest in forest_strategy()) {
        let rt = Runtime::new().unwrap();
        let result: Result<(), TestCaseError> = rt.block_on(async {
            let engine = create_test_engine().await.map_err(|e| TestCaseError::fail(format!("engine: {e}")))?;
            seed_backend(&engine, &forest).await;
            let blocks = build_reference(&forest);
            let query = TestQuery::layout(QuerySource::DescendantsOfAny { min_depth: 1, max_depth: 3 });
            let expected = reference_ids(&query, &blocks);

            let got = backend_ids(&engine, &query, QueryLanguage::HolonGql, None).await;
            prop_assert_eq!(&got, &expected, "DescendantsOfAny via GQL diverged from reference");
            Ok(())
        });
        result?;
    }
}
