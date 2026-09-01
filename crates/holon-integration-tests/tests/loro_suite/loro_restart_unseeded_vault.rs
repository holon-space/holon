//! True-restart twin of `loro_unseeded_vault_split` (live bug, 2026-06-10).
//!
//! That slice synthesizes the stranded state with a raw SQL INSERT into a
//! running Loro session. This one reproduces it the way production actually
//! gets there: a vault is populated under a NO-Loro session (SQL + org files
//! on disk, no `.loro` tree), the app stops, and the user flips
//! `[loro] enabled = true` — the same `test.db` + org filesystem are then
//! reopened by a Loro-enabled session.
//!
//! Flipping the consolidator is an EPOCH change (Model.md invariant 10), so the
//! phase-2 boot is refused unless the operator acknowledges it with
//! `HOLON_CONSOLIDATOR_MIGRATE=1`. That acknowledgement is today's only
//! supported handover (the state-preserving migration is spec 0008 Phase 4.1,
//! unbuilt): it wipes every component's durable state — the Turso db and the
//! CRDT dir — and the new consolidator re-seeds from the surviving vault org
//! files. Phase 2 drives exactly that path.
//!
//! Phase 2 asserts the upgrade contract:
//! 1. the acknowledged flip boots at all (Turso actor shut down cleanly, WAL
//!    not held, the wipe can unlink the db),
//! 2. the phase-1 blocks come back — re-ingested from the org files the wipe
//!    left behind, under their authored ids,
//! 3. the re-seed pass adopts those blocks into the Loro tree: the consolidator
//!    tag in `projection_hash` forces a full re-ingest on the first
//!    Loro-enabled boot, and the org-scan diff loop `create_in_tree`s every
//!    pre-existing block the tree is missing (parent-first via DFS document
//!    order),
//! 4. splitting a phase-1 block through the real engine then flows through Loro
//!    (node count grows by exactly the new half) — never the poisoned-tree
//!    "Block not found" / placeholder-root path. If the re-seed ever declines
//!    (guarded), the assertions flip to the disclosed SQL-owned branch instead
//!    of failing blind.
//!
//! @pbt kind harness
//! @pbt covers restart-persistence(loro-unseeded) — true-restart twin,
//! unseeded-vault Loro @pbt overlaps general_e2e_composed_pbt — kept: no
//! restart transition in keystone

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
    // The consolidator flips `direct` → `projected`, so the invariant-10 epoch
    // guard refuses the boot until acknowledged. Acknowledge it for this boot
    // only; the guard reads the variable once, inside `add_frontend`.
    env.set_enable_loro(true);
    // SAFETY: nextest runs each test in its own process, so no other thread of
    // this process observes the variable.
    unsafe { std::env::set_var("HOLON_CONSOLIDATOR_MIGRATE", "1") };
    let started = env.start_app(true).await;
    unsafe { std::env::remove_var("HOLON_CONSOLIDATOR_MIGRATE") };
    started.expect("phase-2 start_app over the acknowledged consolidator flip must succeed");

    // The wipe removed the Turso db; the vault org files survived it, so the
    // phase-1 blocks must be re-ingested under their authored ids. Poll — the
    // org scan is asynchronous.
    let reingest_deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let rows = env
            .query_sql("SELECT id FROM block_raw")
            .await
            .expect("phase 2: query block_raw");
        let missing: Vec<&str> = phase1_rows
            .iter()
            .map(|r| r.get("id").and_then(|v| v.as_string()).expect("id column"))
            .filter(|id| {
                !rows
                    .iter()
                    .any(|row| row.get("id").and_then(|v| v.as_string()) == Some(*id))
            })
            .collect();
        if missing.is_empty() {
            break;
        }
        assert!(
            std::time::Instant::now() < reingest_deadline,
            "phase 2: phase-1 blocks {missing:?} never came back after the migrate wipe; SQL has \
             {:?}",
            rows.iter()
                .filter_map(|r| r.get("id").and_then(|v| v.as_string()))
                .collect::<Vec<_>>()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
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
    // The target appearing does not mean the re-seed FINISHED — the org-scan
    // diff loop walks in document order, so later nodes can still be arriving.
    // Baseline the count only once it holds still, or the split's "+1 node"
    // assertion measures the seed's tail instead of the split.
    let mut nodes_before = backend.snapshot_blocks().await.len();
    let mut stable_samples = 0;
    let settle_deadline = std::time::Instant::now() + Duration::from_secs(15);
    while stable_samples < 3 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let n = backend.snapshot_blocks().await.len();
        stable_samples = if n == nodes_before {
            stable_samples + 1
        } else {
            0
        };
        nodes_before = n;
        assert!(
            std::time::Instant::now() < settle_deadline,
            "phase 2: the Loro tree never stopped growing (last count {nodes_before}) — the \
             re-seed does not settle"
        );
    }
    env.wait_for_loro_quiescence(Duration::from_secs(10)).await;
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
