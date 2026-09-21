//! A census, not a gate: it MEASURES the two populations findings F-A and F-B
//! named but left unmeasured, and prints them. It asserts only what it needs
//! to not be vacuous (a booted vault has blocks), so it can never go red on a
//! number changing — the numbers belong in
//! `lane-logs/f1a-findings-for-ruling.md` and are re-derived by running this.
//!
//! * **F-A** — `block_raw` rows whose id has the `<file id>::b::<n>` shape: ids
//!   minted by a FORMAT ADAPTER for a source block that authored no `:ID:`
//!   (`crates/holon-markdown/src/logseq.rs:246`,
//!   `crates/holon-markdown/src/obsidian.rs:206`, and the cooklang guest in
//!   `crates/holon-plugin-host/plugins/cooklang.wasm`). The question is how
//!   many of them the Loro authority does NOT hold.
//! * **F-B** — Loro blocks whose content the SQL write path canonicalizes
//!   (`holon_api::content_canonical::canonicalize_stored_content`). Each one
//!   makes `blocks_differ` fire on EVERY full reseed, because a reseed diffs
//!   the canonicalized SQL `before` against the raw Loro `after`, so the count
//!   is also the number of content updates a reseed re-emits forever.

use std::sync::Arc;
use std::time::Duration;

use holon_integration_tests::test_environment::TestEnvironment;
use holon_integration_tests::test_environment::TestEnvironmentBuilder;

struct Census {
    total: usize,
    minted: Vec<String>,
    minted_absent_from_loro: Vec<String>,
    canonicalized: Vec<(String, String)>,
}

async fn census(env: &TestEnvironment) -> Census {
    let projection = env
        .injector()
        .expect("a booted environment has an injector")
        .try_resolve::<holon_loro::loro_sync_controller::LoroProjection>()
        .expect("the Loro projection must be wired for this census");
    let live = projection.live_snapshot();

    let rows = env
        .query_sql("SELECT id, content FROM block_raw")
        .await
        .expect("reading block_raw must succeed");
    let ids: Vec<String> = rows
        .iter()
        .filter_map(|r| match r.get("id") {
            Some(holon_api::Value::String(s)) => Some(s.clone()),
            _ => None,
        })
        .filter(|id| id.starts_with("block:"))
        .collect();

    let minted: Vec<String> = ids
        .iter()
        .filter(|id| id.contains("::b::"))
        .cloned()
        .collect();
    let minted_absent_from_loro: Vec<String> = minted
        .iter()
        .filter(|id| !live.contains_key(*id))
        .cloned()
        .collect();

    let canonicalized: Vec<(String, String)> = live
        .iter()
        .filter(|(_, snap)| {
            let raw = &snap.block.content;
            let is_source = snap.block.content_type == holon_api::ContentType::Source;
            holon_api::content_canonical::canonicalize_stored_content(raw, is_source) != *raw
        })
        .map(|(id, snap)| (id.clone(), snap.block.content.clone()))
        .collect();

    Census {
        total: ids.len(),
        minted,
        minted_absent_from_loro,
        canonicalized,
    }
}

fn report(label: &str, c: &Census) {
    println!("=== CENSUS {label} ===");
    println!("  block_raw block: rows                     {}", c.total);
    println!(
        "  F-A  `::b::<n>` rows                      {}  {:?}",
        c.minted.len(),
        c.minted
    );
    println!(
        "  F-A  …of those, ABSENT from the authority {}  {:?}",
        c.minted_absent_from_loro.len(),
        c.minted_absent_from_loro.iter().take(8).collect::<Vec<_>>()
    );
    println!(
        "  F-B  Loro blocks the store canonicalizes  {}  {:?}",
        c.canonicalized.len(),
        c.canonicalized.iter().take(8).collect::<Vec<_>>()
    );
    println!(
        "  F-B  → content updates re-emitted per reseed {}",
        c.canonicalized.len()
    );
}

#[test]
fn f_a_and_f_b_populations_over_the_shipped_assets_and_a_source_file() {
    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    runtime.clone().block_on(async move {
        // Leg 1 — the shipped default vault alone (`assets/default/`), which is
        // what every fresh boot seeds.
        let env = TestEnvironment::new(runtime.clone()).unwrap();
        env.start_app(true).await.expect("start_app");
        env.wait_for_loro_quiescence(Duration::from_secs(20)).await;
        let shipped = census(&env).await;
        report("shipped assets/default", &shipped);
        assert!(
            shipped.total > 0,
            "the shipped vault produced no blocks — the census would be vacuous"
        );
        drop(env);

        // Leg 2 — the same, plus one adapter-ingested source file with no
        // authored ids, the shape the keystone corpus carries.
        let env = TestEnvironmentBuilder::new()
            .with_vault_file(
                holon_integration_tests::pbt::composed::wide_e2e::READ_ONLY_RECIPE_FILE,
                holon_integration_tests::pbt::composed::wide_e2e::KEYSTONE_RECIPE_COOK,
            )
            .build(runtime.clone())
            .await
            .expect("builder");
        // `build` already ran the boot — a second `start_app` asserts.
        env.wait_for_loro_quiescence(Duration::from_secs(20)).await;
        let with_source = census(&env).await;
        report(
            "shipped assets/default + keystone-recipe.cook",
            &with_source,
        );
        assert!(
            with_source.total >= shipped.total,
            "adding a source file removed blocks — the census is measuring the wrong thing"
        );
    });
}
