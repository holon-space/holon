//! Reproducer for BugFunnel row 25 (dogfood #6, 2026-07-16):
//! "Journals seed/file same-id collision at real-vault boot".
//!
//! Symptom (LIVE, real vault): the vault's `Journals.org` carries `#+ID:
//! journals` and a LEGACY `* Journal Auto-Create` heading (a RANDOM `:ID:`,
//! holon_sql `journals::trigger::0` + Rhai `journals::action::0`) plus its day
//! entries. On a fresh boot the programmatic seed unconditionally creates the
//! `block:journals` Page and the NEW machinery under a heading whose id is the
//! deterministic `block:journals::auto-create` (holon_rule
//! `journals::action::0`).
//!
//! Two failures result, both observed in dogfood #6. (A) DUPLICATE — the seed's
//! `block:journals::auto-create` heading and the file's random-id legacy
//! heading coexist (two page-child headings with the same content, side by
//! side), and the block id `journals::action::0` is claimed by BOTH
//! definitions. (B) DATA LOSS — the file's day-entry headings (2026-07-11 …)
//! never land in the DB: the seed pre-creates the `block:journals` Page, so the
//! file-authority ingest of `Journals.org` collides with the already-seeded
//! page and drops the file's own children (dogfood observed 4 children vs the
//! file's ~10).
//!
//! ROOT CAUSE: the seed-vs-file authority for `block:journals` is unresolved.
//! `seed_default_layout` guards the `index.org`/`root-layout` case
//! (`user_index_org_exists`) but there is NO equivalent handling for a
//! user-authored `Journals.org` that also defines the journals machinery.
//!
//! This test is `#[ignore]`d: making it green requires a PRODUCT RULING on
//! seed-vs-file authority + legacy-rule migration (see the session report /
//! BugFunnel row 25). The oracle it encodes is fork-independent: after ANY fix,
//! (A) there must be exactly one "Journal Auto-Create" heading and (B) the
//! file's day entries must survive. Un-ignore when the fork is decided.
//!
//! @pbt kind harness
//! @pbt covers seed-file-identity(journals) — programmatic journal seed must
//! not duplicate or destroy a user-authored Journals.org carrying `#+ID:
//! journals` (BugFunnel 25) @pbt overlaps general_e2e_composed_pbt — kept: the
//! keystone never boots the seed over a pre-existing user Journals.org with
//! divergent same-id content

use std::sync::Arc;
use std::time::Duration;

use holon_integration_tests::TestEnvironment;

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("build runtime"),
    )
}

const JOURNALS_ACTION: &str = "block:journals::action::0";
// A file-authored journal day entry (2026-07-11) — deterministic id so we can
// assert it survives the seed collision.
const FILE_DAY_ENTRY: &str = "block:8d42906e-6f9d-5597-95b9-ce453fbd2d49";

/// A legacy real-vault `Journals.org`: same `#+ID: journals` as the seed's
/// `block:journals` Page, a random-id `Journal Auto-Create` heading with the
/// old holon_sql trigger + Rhai action, and two day entries.
const LEGACY_JOURNALS_ORG: &str = "\
#+ID: journals
* Journal Auto-Create
:PROPERTIES:
:ID: d67e1f08-b730-4e34-9c49-cf4ef304a19a
:END:
#+BEGIN_SRC holon_sql :id journals::trigger::0
SELECT today as name FROM clock WHERE grain = 'day'
#+END_SRC
#+BEGIN_SRC holon_rule :id journals::action::0
block.create(#{parent_id: \"block:journals\", content: col(\"name\")})
#+END_SRC
* 2026-07-11
:PROPERTIES:
:ID: 8d42906e-6f9d-5597-95b9-ce453fbd2d49
:END:
* 2026-07-12
:PROPERTIES:
:ID: fdda212e-4a47-5aaa-936a-9c39b992f525
:END:
";

async fn wait_for_seed(env: &TestEnvironment) {
    let deadline = std::time::Instant::now() + Duration::from_secs(25);
    loop {
        let rows = env
            .query_sql(&format!(
                "SELECT id FROM block_raw WHERE id = '{JOURNALS_ACTION}'"
            ))
            .await
            .expect("query block_raw");
        if !rows.is_empty() {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "seed never populated {JOURNALS_ACTION}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// RED until the seed/file authority fork is resolved (BugFunnel row 25).
#[test]
#[ignore = "RED: BugFunnel row 25 — seed/file `journals` authority fork unresolved (product ruling required)"]
fn seed_over_legacy_journals_org_no_dup_no_dataloss() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = TestEnvironment::new(rt).expect("TestEnvironment::new");
        // Desktop dogfood shape: SqlOnly (Loro off).
        env.set_enable_loro(false);
        env.write_org_file("Journals.org", LEGACY_JOURNALS_ORG)
            .await
            .expect("write legacy Journals.org");

        env.start_app(true).await.expect("start_app");
        wait_for_seed(&env).await;
        // Let the org scan + writeback settle.
        tokio::time::sleep(Duration::from_millis(800)).await;

        // (A) exactly ONE "Journal Auto-Create" heading — no seed/file duplicate.
        let auto_create_rows = env
            .query_sql("SELECT id FROM block_raw WHERE content = 'Journal Auto-Create'")
            .await
            .expect("query auto-create headings");
        assert_eq!(
            auto_create_rows.len(),
            1,
            "expected exactly ONE 'Journal Auto-Create' heading; found {} (seed's \
             block:journals::auto-create + the file's legacy random-id heading coexist)",
            auto_create_rows.len()
        );

        // (B) the file's day entries must survive the seed collision.
        let day = env
            .query_sql(&format!(
                "SELECT id FROM block WHERE id = '{FILE_DAY_ENTRY}'"
            ))
            .await
            .expect("query file day entry");
        assert!(
            !day.is_empty(),
            "the file-authored journal day entry {FILE_DAY_ENTRY} was DESTROYED by the seed \
             collision (data loss: file's day entries never land when the seed pre-creates \
             block:journals)"
        );

        env.stop_app().await.expect("stop_app");
    });
}
