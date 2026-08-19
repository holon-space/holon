//! Dogfood row 32: wiki-links dead end-to-end in the prod GPUI app —
//! ZERO blocks with non-null `marks` in an ingested-vault DB, `block_links`
//! junction EMPTY, `[[link]]` renders as literal text.
//!
//! This drives the REAL prod ingest path (`FileSyncController` over the app
//! wiring, SqlOnly/`.without_loro()`, the mode Martin runs) with an org file
//! carrying a `[[Linked Page]]` wiki-link, then asserts against the store:
//!   (a) `block_raw.marks` is non-null for the linking block,
//!   (b) the `block_links` junction has a row for it.
//!
//! The unit test
//! `block_params::ingested_inline_marks_survive_store_write_params`
//! proves `build_block_params` EMITS marks; this test proves whether those
//! marks + the derived junction actually reach the store through the app's
//! ingest wiring (the environment gap row 32 describes).
//!
//! @pbt kind harness
//! @pbt covers wiki-link-ingest — marks + block_links junction survive org
//! ingest through the prod FileSyncController path (dogfood row 32)

use std::sync::Arc;
use std::time::Duration;

use holon_integration_tests::TestEnvironmentBuilder;

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("error"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_test_writer()
        .try_init();
}

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create runtime"),
    )
}

// A single block whose body carries a bare wiki-link. The parser strips it to
// the label `Linked Page` in `content` and records a Link mark in `block.marks`
// (see the parser regression cited above).
const WIKI_LINK_ORG: &str = "\
* Notes
:PROPERTIES:
:ID: notes-root
:END:
See [[Linked Page]] here.
";

const LINKING_ID: &str = "notes-root";

async fn assert_marks_and_junction_present(env: &holon_integration_tests::TestEnvironment) {
    // (a) marks column non-null for the linking block.
    let marks_rows = env
        .query_sql(&format!(
            "SELECT id, marks FROM block_raw WHERE id = 'block:{LINKING_ID}'"
        ))
        .await
        .expect("query block_raw marks");
    eprintln!("marks rows: {marks_rows:?}");
    let marks = marks_rows
        .first()
        .and_then(|r| r.get("marks"))
        .cloned()
        .unwrap_or(holon_api::Value::Null);
    assert!(
        !matches!(marks, holon_api::Value::Null),
        "block_raw.marks is NULL for the linking block {LINKING_ID} — org-ingested `[[Linked \
         Page]]` link syntax was dropped between the parser and the store (dogfood row 32). \
         rows={marks_rows:?}"
    );

    // (b) block_links junction has a row for the linking block.
    let link_rows = env
        .query_sql(&format!(
            "SELECT source_block_id, target, kind, resolved_id FROM block_links WHERE \
             source_block_id = 'block:{LINKING_ID}'"
        ))
        .await
        .expect("query block_links junction");
    eprintln!("block_links rows: {link_rows:?}");
    assert!(
        !link_rows.is_empty(),
        "block_links junction is EMPTY for the linking block {LINKING_ID} — the link junction \
         was never populated at org ingest (dogfood row 32)"
    );
}

/// SqlOnly (`Consolidator::Store`) — org ingest routes params (with `marks`)
/// through the command bus to `SqlOperationProvider`, which writes the marks
/// column and derives the junction. This path was already correct.
#[test]
fn wiki_link_ingest_populates_marks_and_junction_sqlonly() {
    init_tracing();
    let rt = runtime();
    rt.clone().block_on(async {
        let env = TestEnvironmentBuilder::new()
            .without_loro()
            .with_org_file("Projects/Notes.org", WIKI_LINK_ORG)
            .build(rt.clone())
            .await
            .expect("boot");
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert_marks_and_junction_present(&env).await;
    });
}

/// Loro + Turso (`Consolidator::Upstream`) — the DEFAULT app wiring. Marks DO
/// flow on this path (`block_raw.marks` is populated by the Loro→SQL
/// projector), but the projection sink `execute_batch_with_origin` never
/// derived the `block_links` junction from them, so backlinks/resolution had no
/// rows. Verified signature pre-fix: marks present, `block_links` empty. This
/// is dogfood row 32's environment gap.
#[test]
fn wiki_link_ingest_populates_marks_and_junction_loro() {
    init_tracing();
    let rt = runtime();
    rt.clone().block_on(async {
        let env = TestEnvironmentBuilder::new()
            .with_org_file("Projects/Notes.org", WIKI_LINK_ORG)
            .build(rt.clone())
            .await
            .expect("boot");
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert_marks_and_junction_present(&env).await;
    });
}
