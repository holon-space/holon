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
//!
//! @pbt kind harness
//! @pbt covers loro-unseeded-split — Loro-over-unseeded-vault split failure
//! class

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon_api::Value;
use holon_integration_tests::TestEnvironment;
use holon_loro::LoroBackend;
use holon_loro::loro_document::LoroDocument;

/// Live tree-node count read from a SETTLED snapshot.
///
/// `snapshot_blocks_from_doc_settled` reports `settled = false` when a live
/// node is transiently skipped because its meta/`STABLE_ID` is mid-commit (an
/// in-flight create/move commits the node and its meta in separate doc-state
/// steps, so a concurrent reader can momentarily see the node without its
/// meta). The public `LoroBackend::snapshot_blocks` discards that bool, so a
/// bare `.len()` under-reports at such an instant. Retry until the read is
/// settled so the count is the tree's true, stable size.
async fn settled_node_count(doc: &LoroDocument) -> usize {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let (blocks, settled) = doc
            .with_read(|d| Ok(holon_loro::snapshot_blocks_from_doc_settled(d)))
            .expect("read settled Loro snapshot");
        if settled {
            return blocks.len();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Loro snapshot never settled (last unsettled count {})",
            blocks.len()
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn is_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                *c == b'-'
            } else {
                c.is_ascii_digit()
            }
        })
}

/// Drain the ONE async boot mutation that races this test: the
/// `daily_journal` auto-create rule (`when: not
/// block_exists("Journals/{date}")`, `crates/holon-frontend/src/lib.rs`) fires
/// off the clock scheduler AFTER boot and mints today's journal page as a new
/// Loro tree node — an independent write, unrelated to the split under test. If
/// `nodes_before` is snapshotted before it lands and `nodes_after` after, the
/// count grows by one (`20 != 21`). The journal page is the only node whose
/// content is a bare `YYYY-MM-DD` date, so awaiting an ISO-date node
/// deterministically waits for that rule to fire without coupling to the
/// clock's exact value or timezone.
async fn wait_for_journal_seeded(doc: &LoroDocument) {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let (blocks, settled) = doc
            .with_read(|d| Ok(holon_loro::snapshot_blocks_from_doc_settled(d)))
            .expect("read settled Loro snapshot");
        if settled && blocks.values().any(|b| is_iso_date(&b.block.content)) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "daily_journal auto-create never seeded a journal node into the Loro tree"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

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
    let env = TestEnvironment::new(runtime).expect("TestEnvironment::new");
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

    // Drain the async Loro projection so the tree has reached its final size
    // before we snapshot it. Mirrors the quiescence idiom the other Loro
    // integration tests use to defeat the seed-in-flight race.
    env.wait_for_loro_quiescence(Duration::from_secs(10)).await;

    let doc_root = env
        .resolve_page_uri_by_name("vault.org")
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
            "INSERT INTO block_raw (id, parent_id, sort_key, content) VALUES ($id, \
             $parent, 'Zz', 'stranded content here')",
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
    let backend = LoroBackend::from_document(global_doc.clone());
    assert!(
        backend.resolve_to_tree_id(stranded_id).await.is_none(),
        "precondition violated: stranded block unexpectedly has a Loro node"
    );
    // Let the async daily-journal auto-create land BEFORE we baseline the tree
    // size, so its node is counted in both `nodes_before` and `nodes_after`
    // and cannot masquerade as split-induced growth.
    wait_for_journal_seeded(&global_doc).await;
    let nodes_before = settled_node_count(&global_doc).await;

    // The live repro: Cmd+Enter → block.split_block through the real engine.
    let mut split_params = HashMap::new();
    split_params.insert("id".to_string(), Value::String(stranded_id.to_string()));
    split_params.insert("position".to_string(), Value::Integer(8));
    env.execute_operation("block", "split_block", split_params)
        .await
        .expect(
            "split_block on a SQL-only block must succeed via the disclosed SQL route (pre-guard: \
             'Block not found: <after>' from update_block_position)",
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
    let nodes_after = settled_node_count(&global_doc).await;
    assert_eq!(
        nodes_before, nodes_after,
        "split on a SQL-only block must not mutate the Loro tree"
    );
}
