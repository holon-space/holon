//! Deterministic regression pin: a search query written in a script where
//! every letter is non-ASCII must not build a predicate whose SQL nesting depth
//! grows with the query.
//!
//! Entry `2026-09-03-search-folding-crashes-the-app-on-cyrillic-and-greek`: case
//! folding was applied to the stored side by wrapping the column in one
//! `replace()` per distinct cased non-ASCII letter, so a 33-character Russian
//! phrase nested 16 deep and overflowed a tokio worker's stack — the process
//! aborted, taking the window and the MCP port with it.
//!
//! The searches run on a thread with a deliberately small stack, so a predicate
//! whose depth grows with the query overflows here rather than only on the
//! device that happens to have the smallest stack.

use std::collections::HashMap;
use std::sync::Arc;

use fluxdi::Module;
use fluxdi::Provider;
use holon_api::EntityName;
use holon_api::OpOrigin;
use holon_api::Value;
use holon_api::query_engine::QueryEngine;
use holon_loro_wiring::EventInfraModule;

/// Small enough that a predicate nesting one level per query letter overflows
/// well before the query returns, large enough for the flat predicate to run
/// with room to spare.
const SMALL_STACK: usize = 256 * 1024;

/// 48 distinct cased letters — the Cyrillic and Greek alphabets, the shape of
/// the phrase that aborted the live app.
const DEEP_QUERY: &str = "абвгдежзийклмнопрстуфхцч αβγδεζηθικλμνξοπρστυφχψω";

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

async fn create_block(engine: &holon::api::BackendEngine, local: &str, content: &str) {
    let block_entity: EntityName = "block".to_string().into();
    let mut p: holon_api::StorageEntity = HashMap::new();
    p.insert("id".into(), Value::String(format!("block:{local}")));
    p.insert("content".into(), Value::String(content.to_string()));
    engine
        .execute_operation(&block_entity, "create", p, OpOrigin::User)
        .await
        .expect("create block");
}

#[test]
fn search_deep_script_query_does_not_overflow() {
    // Default worker stacks: only the probe thread below is starved, so an
    // overflow there is the predicate's depth and never the fixture's setup.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");

    let dir = tempfile::tempdir().expect("tempdir");
    let engine = rt.block_on(async {
        let (engine, _ordering) = fresh_engine(dir.path().join("fresh.db")).await;
        create_block(&engine, "russian", "Программирование на русском языке").await;
        create_block(&engine, "greek", "Επεξεργασία κειμένου").await;
        engine
    });

    // The searches alone run on starved workers: the fixture is already built,
    // so an overflow here is the predicate's depth and nothing else.
    let probe_rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(SMALL_STACK)
        .build()
        .expect("small-stack runtime");
    let (quick_hits, link_hits) = probe_rt.block_on(async {
        // Both call sites: the cmd-K overlay and the `[[` link picker share the
        // predicate builder, so both reached the abort.
        let quick = engine
            .quick_open_search(DEEP_QUERY)
            .await
            .expect("quick_open_search must not error on a Cyrillic/Greek query");
        let links = engine
            .search_link_candidates(DEEP_QUERY)
            .await
            .expect("search_link_candidates must not error on a Cyrillic/Greek query");
        (quick.pages.len() + quick.content.len(), links.len())
    });

    assert_eq!(
        (quick_hits, link_hits),
        (0, 0),
        "no block contains the whole alphabet, so the query must simply find nothing"
    );

    // The folding the depth was paying for still holds: the capital the phrase
    // is actually typed with reaches the stored lower-case letter, both ways.
    rt.block_on(async {
        for (query, expected) in [
            ("ПРОГРАММИРОВАНИЕ", "block:russian"),
            ("программирование", "block:russian"),
            ("ΕΠΕΞΕΡΓΑΣΊΑ", "block:greek"),
            ("επεξεργασία", "block:greek"),
        ] {
            let hits = engine
                .quick_open_search(query)
                .await
                .expect("case-folded query");
            let ids: Vec<String> = hits.content.iter().map(|c| c.id.to_string()).collect();
            assert_eq!(
                ids,
                vec![expected.to_string()],
                "quick_open_search({query:?}) must fold non-ASCII case"
            );
        }
    });
}
