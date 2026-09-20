#![cfg(feature = "pbt")]
//! Can the WRITE AUTHORITY answer a path-shaped existence
//! test while the SQL projection lags?
//!
//! The inhibitor the arc language needs is the shape of
//! `not block_exists("Journals/{today}")` — emptiness of a place, named by a
//! PATH rather than by an identifier. `WriteAuthorityReads`
//! (`crates/holon-core/src/traits.rs:875`) answers `block_exists(id)` only, so
//! the open question was whether an authoritative path read exists at all. If
//! it does not, an inhibitor reading the projection would refuse legitimate
//! work under lag — the D148 `convert_block_to_page` race.
//!
//! It does. `DocumentManager::find_by_name_chain`
//! (`crates/holon-filesystem/src/sync_ports.rs:359`) is a trait default over
//! `find_by_parent_and_name`, and under Loro authority the injector binds that
//! trait to `LoroDocumentManager`
//! (`crates/holon-integration-tests/src/test_environment.rs:1172`), which IS
//! the write authority. This test holds the projection back and shows the two
//! sides disagreeing in exactly the direction that matters.
//!
//! ## Why this binary holds exactly ONE test
//!
//! Same reason as `projector_lag_lock.rs`: the lag is a process-global
//! environment variable read by the projector on every pass, so a second test
//! here would silently run under it.
//!
//! @pbt kind harness
//! @pbt covers arc-inhibitor-authority-read — a path-shaped existence test
//! answered by the write authority under projection lag

use std::sync::Arc;
use std::time::Duration;

use holon_filesystem::DocumentManager;
use holon_integration_tests::TestEnvironmentBuilder;

/// Far wider than the race needs; the point is determinism, not calibration.
const LAG_MS: &str = "600";

/// The page the test mints inside the lag window.
const PAGE_NAME: &str = "RdInhibitor";

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build runtime"),
    )
}

#[test]
fn the_write_authority_answers_a_path_shaped_existence_test_under_lag() {
    // SAFETY: single-threaded test entry, before any engine, runtime or
    // projector task exists, and this binary holds no other test (module docs).
    unsafe {
        std::env::set_var("HOLON_TEST_PROJECTOR_LAG_MS", LAG_MS);
    }

    let rt = runtime();
    rt.clone().block_on(async move {
        let world = TestEnvironmentBuilder::new()
            .with_org_file(
                "RdSeed.org".to_string(),
                "#+TITLE: Rd Seed\n* Seeded\n".to_string(),
            )
            .build(rt.clone())
            .await
            .expect("boot the vault");

        let manager = world
            .injector()
            .expect("the composition root is wired")
            .resolve_async::<dyn DocumentManager>()
            .await;

        // The inhibitor's TRUE branch: a page nobody minted is absent, and the
        // authority says so rather than declining to answer.
        let missing = manager
            .find_by_name_chain(&["RdNoSuchPageAnywhere"])
            .await
            .expect("the authority answers a name chain");
        assert!(
            missing.is_none(),
            "an unminted page must read as absent, got {missing:?}"
        );

        // Mint a page THROUGH THE AUTHORITY. `create_document` cannot serve
        // here: it polls until the page resolves, and that poll can be
        // satisfied by the projection, which closes the very window this test
        // needs open.
        let minted = manager
            .get_or_create_by_name_chain(&[PAGE_NAME])
            .await
            .expect("mint the page through the write authority");
        let doc_uri = minted.id.clone();

        // The projection, which an inhibitor must NOT read.
        let projected = world
            .query_sql(&format!("SELECT id FROM block_raw WHERE id = '{doc_uri}'"))
            .await
            .expect("query the projection");
        eprintln!(
            "[R-D] inside the {LAG_MS}ms lag window, the SQL projection holds {} row(s) for the \
             freshly minted page",
            projected.len()
        );
        assert!(
            projected.is_empty(),
            "the projection already caught up, so this run does not exercise the lag window at \
             all — raise HOLON_TEST_PROJECTOR_LAG_MS"
        );

        // The authority, which an inhibitor MUST read.
        let found = manager
            .find_by_name_chain(&[PAGE_NAME])
            .await
            .expect("the authority answers a name chain")
            .unwrap_or_else(|| {
                panic!(
                    "the write authority could not resolve `{PAGE_NAME}` by name chain, although \
                     it had just accepted the write of {doc_uri}"
                )
            });
        assert_eq!(
            found.id, doc_uri,
            "the authority resolved the name chain to a different page"
        );
        eprintln!(
            "[R-D] the write authority resolved the same page by PATH: {PAGE_NAME} -> {}",
            found.id
        );

        // And the projection catches up eventually, which is what makes the
        // disagreement above a lag rather than a loss.
        world
            .wait_for_loro_quiescence(Duration::from_secs(120))
            .await;
        world
            .wait_for_cdc_quiescent(Duration::from_millis(300), Duration::from_secs(120))
            .await;
        let settled = world
            .query_sql(&format!("SELECT id FROM block_raw WHERE id = '{doc_uri}'"))
            .await
            .expect("query the projection");
        eprintln!(
            "[R-D] after quiescence the projection holds {} row(s) for the same page",
            settled.len()
        );
        assert_eq!(
            settled.len(),
            1,
            "the page never reached the projection, so the earlier absence was a loss and not a \
             lag"
        );
    });
}
