//! A tag carrying a separator character makes a block lose its WHOLE tag set
//! at the next boot — silently, with the write having reported success.
//!
//! Found by adversarial verification of the edge-array multiset detector
//! (bugfunnel `2026-08-30-tag-with-separator-char-loses-whole-set-on-reboot`),
//! not by any test. The reported observation — `block_tags` EMPTY for a block
//! whose tag list contained `"a,b"` — is real and reproduces. Only the
//! mechanism first attributed to it was wrong: the write itself succeeds and
//! reads back complete (all four tags, `"a,b"` included, are in `block_tags`
//! immediately after settling), so this is not a failed or rejected write. The
//! set is destroyed one BOOT later, which is where the reported empty read
//! landed. A comma-free control survives the identical reboot.
//!
//! Mechanism: tags render into the org tag group `:M:a,b:proj:zzz:`
//! (`Tags::to_org`), and org "has no escape for it" (`split_headline_tags`,
//! crates/holon-org-format/src/parser.rs:556-559) — a comma is not in the tag
//! grammar, so the trailing group stops parsing as tags at all and the
//! re-ingest drops EVERY tag, not just the offending one. `Tags::to_csv` /
//! `from_csv` (crates/holon-api/src/types.rs) carry the same hazard for the
//! comma specifically.
//!
//! The contract asserted here is the fail-loud one: store the tag faithfully or
//! REFUSE the write naming the offending tag — never report success and then
//! discard the caller's data.
//!
//! @pbt kind harness
//! @pbt covers tag-separator-whole-set-loss — a separator char in one tag
//! drops every tag on the block at the next re-ingest
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone's tag alphabet
//! never generates a separator character inside a tag

use std::sync::Arc;
use std::time::Duration;

use holon_api::QueryLanguage;
use holon_integration_tests::TestEnvironment;
use holon_loro::DocScope;
use holon_loro::LoroBackend;

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("build runtime"),
    )
}

const VAULT_ORG: &str = "\
* Alpha
:PROPERTIES:
:ID: blk-a
:END:
";

async fn wait_for_seed(env: &TestEnvironment) {
    for _ in 0..100 {
        let rows = env
            .query(
                "SELECT id FROM block_raw WHERE id = 'block:blk-a'",
                QueryLanguage::HolonSql,
            )
            .await
            .expect("seed query");
        if !rows.is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("seed block never landed in block_raw");
}

async fn settle(env: &TestEnvironment) {
    env.wait_for_loro_quiescence(Duration::from_secs(15)).await;
    env.wait_for_cdc_quiescent(Duration::from_millis(250), Duration::from_secs(15))
        .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
}

async fn edge_backend(env: &TestEnvironment) -> LoroBackend {
    let store = env
        .loro_doc_store()
        .expect("loro_doc_store present")
        .clone();
    let global_doc = store
        .read()
        .await
        .get_doc(DocScope::Global)
        .await
        .expect("global Loro doc");
    LoroBackend::from_document(global_doc)
}

/// The tags `block_tags` holds for `blk-a`, sorted.
async fn stored_tags(env: &TestEnvironment) -> Vec<String> {
    let rows = env
        .query(
            "SELECT tag FROM block_tags WHERE block_id = 'block:blk-a' ORDER BY tag",
            QueryLanguage::HolonSql,
        )
        .await
        .expect("read block_tags");
    rows.iter()
        .filter_map(|r| r.get("tag"))
        .filter_map(|v| v.as_string().map(|s| s.to_string()))
        .collect()
}

/// Control: the same call WITHOUT a comma-carrying tag lands every tag. Without
/// this, a failure below could be "tags never land in this harness" rather than
/// "the comma is what kills them".
#[test]
fn tags_without_a_comma_all_land() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = TestEnvironment::new(rt).expect("TestEnvironment::new");
        assert!(env.loro_enabled(), "repro needs the composed Loro wiring");
        env.write_org_file("vault.org", VAULT_ORG)
            .await
            .expect("write vault.org");
        env.start_app(true).await.expect("start_app");
        wait_for_seed(&env).await;

        let backend = edge_backend(&env).await;
        backend
            .set_block_tags(
                "block:blk-a",
                &["zzz".to_string(), "proj".to_string(), "M".to_string()],
            )
            .await
            .expect("set_block_tags without a comma");
        settle(&env).await;

        assert_eq!(
            stored_tags(&env).await,
            vec!["M".to_string(), "proj".to_string(), "zzz".to_string()],
            "the comma-free control must land all three tags"
        );
    });
}

/// The bug: one comma-carrying tag and the ENTIRE set disappears, with `Ok`
/// returned to the caller.
#[test]
fn a_comma_carrying_tag_must_not_silently_drop_the_whole_set() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = TestEnvironment::new(rt).expect("TestEnvironment::new");
        assert!(env.loro_enabled(), "repro needs the composed Loro wiring");
        env.write_org_file("vault.org", VAULT_ORG)
            .await
            .expect("write vault.org");
        env.start_app(true).await.expect("start_app");
        wait_for_seed(&env).await;

        let backend = edge_backend(&env).await;
        // Establish a prior non-empty set first: the reported repro hit the
        // REPLACE path (the block already carried tags), not a first write.
        backend
            .set_block_tags("block:blk-a", &["proj".to_string()])
            .await
            .expect("prior tag write");
        settle(&env).await;
        assert_eq!(
            stored_tags(&env).await,
            vec!["proj".to_string()],
            "the prior set must land before the comma write is exercised"
        );

        let outcome = backend
            .set_block_tags(
                "block:blk-a",
                &[
                    "zzz".to_string(),
                    "a,b".to_string(),
                    "proj".to_string(),
                    "M".to_string(),
                ],
            )
            .await;
        settle(&env).await;

        let stored = stored_tags(&env).await;

        // Refusal IS the contract now, so require it. An `Ok` here means the
        // guard was bypassed and the block is carrying a tag that will corrupt
        // the whole set at the next round-trip.
        let err = outcome.expect_err(
            "set_block_tags must REFUSE a tag carrying a separator, not accept it — an accepted \
             write reads back fine and destroys the set one boot later",
        );
        let msg = format!("{err:#}");
        assert!(
            msg.contains("a,b"),
            "a refusal must name the offending tag, got: {msg}"
        );
        assert_eq!(
            stored,
            vec!["proj".to_string()],
            "a refused write must leave the block's PRIOR tags untouched, not half-apply"
        );
    });
}

/// The create path reaches `tags` through `BlockEdges`, never through
/// `set_block_tags` — so guarding only the setters left
/// `create_block_with_properties` (and `add_subtask`'s create params) able to
/// store an unrepresentable tag. `write_new_node` is the sole writer of a new
/// node's meta, which is where the guard sits.
#[test]
fn creating_a_block_with_a_separator_carrying_tag_is_refused() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = TestEnvironment::new(rt).expect("TestEnvironment::new");
        assert!(env.loro_enabled(), "repro needs the composed Loro wiring");
        env.write_org_file("vault.org", VAULT_ORG)
            .await
            .expect("write vault.org");
        env.start_app(true).await.expect("start_app");
        wait_for_seed(&env).await;

        let backend = edge_backend(&env).await;
        let mut edges = holon_api::BlockEdges::default();
        edges.tags = holon_api::types::Tags::from_tag_iter(vec![
            "zzz".to_string(),
            "a,b".to_string(),
            "M".to_string(),
        ]);
        let err = backend
            .create_block_with_properties(
                holon_api::EntityUri::block("blk-a"),
                holon_api::BlockContent::text("child"),
                Some(holon_api::EntityUri::block("comma-child")),
                &std::collections::HashMap::new(),
                &edges,
            )
            .await
            .expect_err(
                "create_block_with_properties must REFUSE a separator-carrying tag — the create \
                 path bypasses set_block_tags entirely",
            );
        let msg = format!("{err:#}");
        assert!(
            msg.contains("a,b"),
            "a refusal must name the offending tag, got: {msg}"
        );
    });
}

/// The loss this bug is actually about: the write SUCCEEDS and reads back
/// correctly, then the whole set evaporates on the next boot, because the org
/// tag group `:M:a,b:proj:zzz:` does not parse and the re-ingest drops every
/// tag. The comma-free control in `tags_without_a_comma_all_land` survives the
/// same reboot, so the separator is the variable.
///
/// With the boundary rejection in place the write never lands, so the block
/// keeps its prior tags across the reboot instead of silently losing them.
#[test]
fn a_separator_carrying_tag_must_not_lose_the_set_across_a_reboot() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = TestEnvironment::new(rt).expect("TestEnvironment::new");
        assert!(env.loro_enabled(), "repro needs the composed Loro wiring");
        env.write_org_file("vault.org", VAULT_ORG)
            .await
            .expect("write vault.org");
        env.start_app(true).await.expect("boot-1 start_app");
        wait_for_seed(&env).await;

        let backend = edge_backend(&env).await;
        backend
            .set_block_tags("block:blk-a", &["proj".to_string()])
            .await
            .expect("prior tag write");
        let _ = backend
            .set_block_tags(
                "block:blk-a",
                &["zzz".to_string(), "a,b".to_string(), "M".to_string()],
            )
            .await;
        settle(&env).await;
        let before = stored_tags(&env).await;

        env.stop_app().await.expect("stop_app after boot-1");
        env.start_app(true).await.expect("boot-2 start_app");
        wait_for_seed(&env).await;
        settle(&env).await;

        assert_eq!(
            stored_tags(&env).await,
            before,
            "the block's tags changed across a reboot with nothing writing to them — a tag \
             carrying a separator makes the org tag group unparseable, so the re-ingest drops \
             the WHOLE set (before the reboot the block held {before:?})"
        );
        assert!(
            !before.is_empty(),
            "the block must still carry its prior tags: an accepted-then-lost set and a \
             refused-then-empty set would both satisfy an equality on two empties"
        );
    });
}
