//! Regression slice for the "Loro enabled over an unseeded vault" failure
//! class (live bug, 2026-06-10).
//!
//! Production state it models: `[loro] enabled = true` over a pre-existing
//! vault whose SQL DB is fully populated while the Loro tree is missing
//! blocks (`.loro/holon_tree.loro.sync` was ~11 bytes vs 1013 SQL rows).
//! Splitting any such block routed the new-block create through
//! `BlockCellRegistry::create_entity` → `LoroBackend::update_block_position`,
//! which failed "Block not found: <after>" — and, worse, only AFTER minting a
//! placeholder parent + empty-text node, so later splits read "" content from
//! the poisoned tree ("Split position N exceeds content length 0").
//!
//! The test reproduces the state directly: a Loro-enabled prod-DI session
//! plus one block inserted SQL-only (no Loro node) — byte-for-byte the
//! stranded-vault condition — then dispatches `block.split_block` through the
//! real engine. Pre-guard this errored; post-guard the create falls through
//! to the SQL path (disclosed) and the tree stays untouched.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon_api::Value;
use holon_integration_tests::TestEnvironment;
use holon_loro::LoroBackend;

#[test]
fn split_block_on_sql_only_block_succeeds_without_poisoning_loro_tree() {
    // The SUT owns its own runtime (mirrors the phased runner); a plain
    // `#[test]` + `block_on` keeps the runtime's Drop on the main thread.
    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap(),
    );
    runtime.clone().block_on(run_test(runtime.clone()));
}

async fn run_test(runtime: Arc<tokio::runtime::Runtime>) {
    let mut env = TestEnvironment::new(runtime).expect("TestEnvironment::new");
    assert!(env.loro_enabled(), "this slice needs the Loro wiring");
    env.write_org_file("vault.org", "* vault\n- first block\n- second block\n")
        .await
        .expect("write vault.org");
    env.start_app(true).await.expect("start_app");

    // Wait until the org scan landed the vault in SQL (and Loro).
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let rows = env
            .query_sql("SELECT id FROM block_raw")
            .await
            .expect("query block_raw");
        if rows.len() >= 3 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "org scan never populated SQL (have {} rows)",
            rows.len()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let doc_root = env
        .resolve_doc_uri_by_name("vault.org")
        .await
        .expect("resolve vault.org root");

    // The stranded block: present in SQL, absent from the Loro tree — the
    // state a pre-Loro vault is in after `[loro] enabled = true` flips on
    // without a seed pass.
    let stranded_id = "block:11111111-2222-3333-4444-555555555555";
    let mut params = HashMap::new();
    params.insert("id".to_string(), Value::String(stranded_id.to_string()));
    params.insert("parent".to_string(), Value::String(doc_root.to_string()));
    env.engine()
        .db_handle()
        .query(
            "INSERT INTO block_raw (id, parent_id, depth, sort_key, content) \
             VALUES ($id, $parent, 1, 'Zz', 'stranded content here')",
            params,
        )
        .await
        .expect("insert stranded SQL-only block");

    // Pin the precondition: the stranded block has NO Loro node, and record
    // the tree size so poisoning (placeholder roots / stray nodes) is
    // detectable after the split.
    let store = env
        .loro_doc_store()
        .expect("loro_doc_store present in Loro wiring")
        .clone();
    let global_doc = store
        .read()
        .await
        .get_global_doc()
        .await
        .expect("global Loro doc");
    let backend = LoroBackend::from_document(global_doc);
    assert!(
        backend.resolve_to_tree_id(stranded_id).await.is_none(),
        "precondition violated: stranded block unexpectedly has a Loro node"
    );
    let nodes_before = backend.snapshot_blocks().await.len();

    // The live repro: Cmd+Enter → block.split_block through the real engine.
    let mut split_params = HashMap::new();
    split_params.insert("id".to_string(), Value::String(stranded_id.to_string()));
    split_params.insert("position".to_string(), Value::Integer(8));
    env.execute_operation("block", "split_block", split_params)
        .await
        .expect(
            "split_block on a SQL-only block must succeed via the disclosed SQL route \
             (pre-guard: 'Block not found: <after>' from update_block_position)",
        );

    // Both halves live in SQL under the same parent.
    let rows = env
        .query_sql(&format!(
            "SELECT id, content FROM block_raw WHERE parent_id = '{}' ORDER BY sort_key",
            doc_root
        ))
        .await
        .expect("query halves");
    let contents: Vec<String> = rows
        .iter()
        .filter_map(|r| {
            r.get("content")
                .and_then(|v| v.as_string())
                .map(String::from)
        })
        .collect();
    assert!(
        contents.iter().any(|c| c == "stranded"),
        "original block must hold the trimmed prefix; got {contents:?}"
    );
    assert!(
        contents.iter().any(|c| c == "content here"),
        "new block must hold the trimmed suffix; got {contents:?}"
    );

    // The Loro tree must be untouched: no node for either half, no
    // placeholder roots minted on the failed-anchor path.
    assert!(
        backend.resolve_to_tree_id(stranded_id).await.is_none(),
        "split must not have minted a Loro node for the stranded block"
    );
    let nodes_after = backend.snapshot_blocks().await.len();
    assert_eq!(
        nodes_before, nodes_after,
        "split on a SQL-only block must not mutate the Loro tree"
    );
}
