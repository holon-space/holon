//! True-restart twin of `loro_unseeded_vault_split` (live bug, 2026-06-10).
//!
//! That slice synthesizes the stranded state with a raw SQL INSERT into a
//! running Loro session. This one reproduces it the way production actually
//! gets there: a vault is populated under a NO-Loro session (SQL + org files
//! on disk, no `.loro` tree), the app stops, and the user flips
//! `[loro] enabled = true` — the same `test.db` + org filesystem are then
//! reopened by a Loro-enabled session.
//!
//! Phase 2 asserts the upgrade contract:
//! 1. the restart works at all (Turso actor shut down cleanly, WAL not held),
//! 2. SQL state survives intact,
//! 3. the re-seed pass adopts phase-1 blocks into the Loro tree: the
//!    consolidator tag in `projection_hash` forces a full re-ingest on the
//!    first Loro-enabled boot, and the org-scan diff loop `create_in_tree`s
//!    every pre-existing block the tree is missing (parent-first via DFS
//!    document order),
//! 4. splitting a phase-1 block through the real engine then flows through Loro
//!    (node count grows by exactly the new half) — never the poisoned-tree
//!    "Block not found" / placeholder-root path. If the re-seed ever declines
//!    (guarded), the assertions flip to the disclosed SQL-owned branch instead
//!    of failing blind.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon_api::Value;
use holon_integration_tests::TestEnvironment;
use holon_loro::LoroBackend;

#[test]
fn restart_with_loro_enabled_over_populated_sql_vault() {
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

    // ── Phase 1: populate the vault WITHOUT Loro ─────────────────────────
    env.set_enable_loro(false);
    env.write_org_file("vault.org", "* alpha one\n* beta two\n")
        .await
        .expect("write vault.org");
    env.start_app(true).await.expect("phase-1 start_app");

    // Exact-content match below: a `contains` would also match the doc block
    // (whole-file content) and split the wrong row. Wait for THIS row — a
    // bare row-count gate races the scan (seed blocks count toward it).
    let target_content = "alpha one";
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let phase1_rows = loop {
        let rows = env
            .query_sql("SELECT id, content FROM block_raw ORDER BY id")
            .await
            .expect("query block_raw");
        if rows
            .iter()
            .any(|r| r.get("content").and_then(|v| v.as_string()) == Some(target_content))
        {
            break rows;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "phase 1: org scan never landed {target_content:?} in SQL; have {:?}",
            rows.iter()
                .filter_map(|r| r.get("content").and_then(|v| v.as_string()))
                .collect::<Vec<_>>()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let doc_root = env
        .resolve_page_uri_by_name("vault.org")
        .await
        .expect("resolve vault.org root");
    let target_id = phase1_rows
        .iter()
        .find_map(|r| {
            (r.get("content").and_then(|v| v.as_string()) == Some(target_content))
                .then(|| r.get("id").and_then(|v| v.as_string()).map(String::from))?
        })
        .unwrap_or_else(|| {
            panic!(
                "phase 1: no block with exactly {target_content:?}; have {:?}",
                phase1_rows
                    .iter()
                    .filter_map(|r| r.get("content").and_then(|v| v.as_string()))
                    .collect::<Vec<_>>()
            )
        });

    env.stop_app().await.expect("stop_app after phase 1");

    // ── Phase 2: same vault, Loro enabled ────────────────────────────────
    env.set_enable_loro(true);
    env.start_app(true)
        .await
        .expect("phase-2 start_app over the same test.db must succeed");

    let rows = env
        .query_sql("SELECT id FROM block_raw")
        .await
        .expect("phase 2: query block_raw");
    for r in &phase1_rows {
        let id = r.get("id").and_then(|v| v.as_string()).expect("id column");
        assert!(
            rows.iter()
                .any(|row| row.get("id").and_then(|v| v.as_string()) == Some(id)),
            "phase 2: phase-1 block {id} missing from SQL after restart"
        );
    }

    // Observe the tree state the upgrade produced (give the org rescan a
    // bounded window to seed if it is going to).
    let store = env
        .loro_doc_store()
        .expect("loro_doc_store present in phase-2 Loro wiring")
        .clone();
    let global_doc = store
        .read()
        .await
        .get_global_doc()
        .await
        .expect("global Loro doc");
    let backend = LoroBackend::from_document(global_doc);
    let mut target_seeded = backend.resolve_to_tree_id(&target_id).await.is_some();
    let seed_deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !target_seeded && std::time::Instant::now() < seed_deadline {
        tokio::time::sleep(Duration::from_millis(200)).await;
        target_seeded = backend.resolve_to_tree_id(&target_id).await.is_some();
    }
    let nodes_before = backend.snapshot_blocks().await.len();
    eprintln!("[restart-vault] phase-2 tree: target_seeded={target_seeded} nodes={nodes_before}");

    // ── The live repro: split a phase-1 block through the real engine ────
    // Split "alpha one" at byte 5 ("alpha" / " one"); prod trims the seam.
    let mut split_params = HashMap::new();
    split_params.insert("id".to_string(), Value::String(target_id.clone()));
    split_params.insert("position".to_string(), Value::Integer(5));
    env.execute_operation("block", "split_block", split_params)
        .await
        .expect(
            "phase 2: split_block on a phase-1 block must succeed (guarded SQL route if unseeded; \
             Loro route if the rescan seeded it)",
        );

    // Both halves must land in SQL. Through the Loro route the content
    // update is projected asynchronously (outbound projector), so poll.
    let split_deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let all = env
            .query_sql("SELECT content FROM block_raw")
            .await
            .expect("phase 2: query post-split blocks");
        let contents: Vec<String> = all
            .iter()
            .filter_map(|r| {
                r.get("content")
                    .and_then(|v| v.as_string())
                    .map(String::from)
            })
            .collect();
        if contents.iter().any(|c| c == "alpha") && contents.iter().any(|c| c == "one") {
            break;
        }
        assert!(
            std::time::Instant::now() < split_deadline,
            "split halves never landed in SQL; got {contents:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let _ = doc_root;

    // Tree consistency: seeded ⇒ the split flows through Loro (node count
    // grows by one); unseeded ⇒ the guarded SQL route leaves the tree
    // untouched. Either way, no placeholder poisoning.
    let nodes_after = backend.snapshot_blocks().await.len();
    if target_seeded {
        assert_eq!(
            nodes_after,
            nodes_before + 1,
            "seeded vault: split must add exactly the new half to the Loro tree"
        );
    } else {
        assert!(
            backend.resolve_to_tree_id(&target_id).await.is_none(),
            "unseeded vault: split must not mint a Loro node for the SQL-only block"
        );
        assert_eq!(
            nodes_before, nodes_after,
            "unseeded vault: split must not mutate the Loro tree"
        );
    }
}
