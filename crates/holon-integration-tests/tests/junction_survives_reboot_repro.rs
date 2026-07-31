//! Reproducer for task #71 (dogfood, 2026-07-29): after an app restart over an
//! existing DB, `block_tags` holds ZERO rows while the vault's pages are still
//! `Page`-tagged — every page then renders a degraded breadcrumb banner.
//!
//! The restart seam has two halves and the bug needs both:
//!
//! 1. `BlockSchemaModule::ensure_schema` unconditionally `DROP TABLE IF EXISTS
//!    block_tags` (and `block_requires`, `advice_suppressed`) on EVERY boot,
//!    then recreates them empty. `block_raw` is `CREATE TABLE IF NOT EXISTS`
//!    and survives — so the junctions, and only the junctions, start each boot
//!    empty.
//! 2. `FileSyncController`'s cold-boot fast path skips ingest for every file
//!    whose disk bytes still hash to the persisted `file.content_hash`. That
//!    skip is what would otherwise re-assert the junction rows, so on an
//!    unchanged vault nothing ever refills them.
//!
//! WHAT THESE TESTS DO AND DO NOT PROVE — measured, not assumed:
//!
//! WHETHER BOOT 2 TAKES THE COLD-BOOT SKIP IS UNRESOLVED. Do not build on
//! either answer.
//!
//! `reboot_harness_takes_the_cold_boot_fast_path_skip` asks the question with a
//! sound discriminator: corrupt a block's `content` in SQL only, leaving the
//! org file and its stored hash untouched. An ingest that RUNS finds
//! `content_differs` true and overwrites the corruption; a SKIPPED ingest
//! leaves it standing. In this file the corruption is consistently OVERWRITTEN
//! (2/2 runs, including in isolation), i.e. boot 2 re-ingests and the skip does
//! NOT fire — which contradicts an independent run of the same probe shape that
//! saw the corruption survive. Until that is reconciled the probe asserts only
//! its premise and PRINTS the verdict.
//!
//! Note what does NOT work, because this file got it wrong once: comparing a
//! block's `updated_at` across an unwiped reboot is not evidence of a skip, as
//! a re-ingest of an unchanged vault writes nothing either. Only a store-vs-org
//! divergence discriminates — which is why the question above is open rather
//! than answered.
//!
//! Independently of that open question,
//! `wiped_junction_is_repaired_on_next_boot` shows a junction emptied between
//! boots coming back on the next boot. The mechanism is measured, not guessed,
//! by sampling the TAGGED ROOT rather than an untagged child (the untagged
//! child is untouched either way and has no discriminating power): the root IS
//! rewritten. The wipe makes the doc-root invisible to the page lookup, that
//! lookup misses, and the ingest path re-creates the page with
//! `set_page(true)`.
//!
//! So the self-heal is re-derivation triggered by MISSING DERIVED STATE rather
//! than by the file changing. It is NOT the Loro boot projection —
//! that explanation was refuted by measurement (the repair also happens with
//! Loro disabled).
//!
//! CONSEQUENCE: these cases cannot go red for the LIVE symptom (a real vault
//! stuck at zero rows), because in this configuration the system self-repairs.
//! They are regression guards for the reboot contract, not a reproduction of
//! the dogfooded bug. The live mystery is the BOUNDARY of that self-heal: a
//! page whose re-creation cannot complete — a quarantined file or an identity
//! collision, both of which Martin's vault has — never heals. Do not read a
//! green run here as evidence that a deployed database is healthy.
//!
//! @pbt kind harness
//! @pbt covers reboot-junction-loss — block_tags/block_requires emptied by the
//! boot schema migration and never rebuilt behind the unchanged-file fast path
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone's
//! `SimulateRestart` is a file-touch re-ingest, not a storage reboot (F9), so
//! it structurally cannot see a boot-time DROP.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use holon_api::EntityUri;
use holon_integration_tests::TestEnvironment;
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

const ALPHA_ORG: &str = "\
#+TITLE: Alpha
#+ID: page-alpha

* Alpha child
:PROPERTIES:
:ID: blk-a
:END:
";

const BETA_ORG: &str = "\
#+TITLE: Beta
#+ID: page-beta

* Beta child
:PROPERTIES:
:ID: blk-b
:END:
";

/// Wait until the org scan has landed the seeded blocks in `block_raw`.
async fn wait_for_seed(env: &TestEnvironment) {
    let deadline = std::time::Instant::now() + Duration::from_secs(25);
    loop {
        let rows = env
            .query_sql("SELECT id FROM block_raw WHERE id IN ('block:blk-a', 'block:blk-b')")
            .await
            .expect("query block_raw");
        if rows.len() >= 2 {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "org scan never populated both seeded blocks (have {} rows)",
            rows.len()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Wait until the org scan has landed `alpha.org`'s block.
async fn wait_for_seed_alpha(env: &TestEnvironment) {
    let deadline = std::time::Instant::now() + Duration::from_secs(25);
    loop {
        let rows = env
            .query_sql("SELECT id FROM block_raw WHERE id = 'block:blk-a'")
            .await
            .expect("query block_raw");
        if !rows.is_empty() {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "org scan never populated block:blk-a"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// The full `(block_id, tag)` content of the `block_tags` junction.
async fn tag_pairs(env: &TestEnvironment) -> BTreeSet<(String, String)> {
    let rows = env
        .query_sql("SELECT block_id, tag FROM block_tags")
        .await
        .expect("query block_tags");
    rows.iter()
        .map(|r| {
            let id = r
                .get("block_id")
                .and_then(|v| v.as_string())
                .expect("block_id")
                .to_string();
            let tag = r
                .get("tag")
                .and_then(|v| v.as_string())
                .expect("tag")
                .to_string();
            (id, tag)
        })
        .collect()
}

/// The full `(block_id, required_id)` content of the `block_requires` junction.
async fn requires_pairs(env: &TestEnvironment) -> BTreeSet<(String, String)> {
    let rows = env
        .query_sql("SELECT block_id, required_id FROM block_requires")
        .await
        .expect("query block_requires");
    rows.iter()
        .map(|r| {
            let id = r
                .get("block_id")
                .and_then(|v| v.as_string())
                .expect("block_id")
                .to_string();
            let req = r
                .get("required_id")
                .and_then(|v| v.as_string())
                .expect("required_id")
                .to_string();
            (id, req)
        })
        .collect()
}

/// A `LoroBackend` over the frontend's authority doc — the production
/// edge-field write path (Loro -> project() -> SQL).
async fn edge_backend(env: &TestEnvironment) -> LoroBackend {
    let store = env
        .loro_doc_store()
        .expect("loro_doc_store present")
        .clone();
    let global_doc = store
        .read()
        .await
        .get_global_doc()
        .await
        .expect("global Loro doc");
    LoroBackend::from_document(global_doc)
}

async fn settle(env: &TestEnvironment) {
    env.wait_for_loro_quiescence(Duration::from_secs(15)).await;
    env.wait_for_cdc_quiescent(Duration::from_millis(250), Duration::from_secs(15))
        .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
}

/// The dogfood shape: an UNCHANGED vault of `Page`-tagged files, rebooted.
/// Every `Page` row the first boot derived must still be in `block_tags` after
/// the second boot — the sidebar and the breadcrumb both read that junction.
#[test]
fn page_tags_survive_reboot_over_existing_db() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = TestEnvironment::new(rt).expect("TestEnvironment::new");
        env.write_org_file("alpha.org", ALPHA_ORG)
            .await
            .expect("write alpha.org");
        env.write_org_file("beta.org", BETA_ORG)
            .await
            .expect("write beta.org");

        // ── Boot 1 ──────────────────────────────────────────────────────
        env.start_app(true).await.expect("boot-1 start_app");
        wait_for_seed(&env).await;
        settle(&env).await;

        let boot1 = tag_pairs(&env).await;
        eprintln!("[reboot-junction] boot-1 block_tags = {boot1:?}");
        let boot1_pages: BTreeSet<_> = boot1.iter().filter(|(_, t)| t == "Page").collect();
        assert!(
            boot1_pages.len() >= 2,
            "boot-1 did not derive a `Page` tag per page-file — the repro's premise is broken, \
             not the restart seam. block_tags = {boot1:?}"
        );

        env.stop_app().await.expect("stop_app after boot-1");

        // ── Boot 2 over the SAME test.db + vault ────────────────────────
        env.start_app(true).await.expect("boot-2 start_app");
        wait_for_seed(&env).await;
        settle(&env).await;

        let boot2 = tag_pairs(&env).await;
        eprintln!("[reboot-junction] boot-2 block_tags = {boot2:?}");

        let lost: Vec<_> = boot1.difference(&boot2).collect();
        assert!(
            lost.is_empty(),
            "[reboot] `block_tags` LOST {} of {} rows across a restart over an existing DB \
             (block_raw survived). Lost pairs: {lost:?}\nboot-2 block_tags = {boot2:?}",
            lost.len(),
            boot1.len()
        );
    });
}

/// Same seam, edge fields written through the production Loro writer rather
/// than derived from the org text — pins `block_requires` alongside
/// `block_tags` so a fix that only special-cases `Page` does not pass.
#[test]
fn loro_written_edge_fields_survive_reboot_over_existing_db() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = TestEnvironment::new(rt).expect("TestEnvironment::new");
        assert!(env.loro_enabled(), "repro needs the composed Loro wiring");
        env.write_org_file("alpha.org", ALPHA_ORG)
            .await
            .expect("write alpha.org");
        env.write_org_file("beta.org", BETA_ORG)
            .await
            .expect("write beta.org");

        // ── Boot 1: seed, then write tags + requires through the prod path
        env.start_app(true).await.expect("boot-1 start_app");
        wait_for_seed(&env).await;

        let backend = edge_backend(&env).await;
        backend
            .set_block_tags("block:blk-a", &["proj".to_string()])
            .await
            .expect("set_block_tags(blk-a)");
        // ALLOW(entity_uri_from_raw): test-caller-supplied id; from_raw schemes it.
        backend
            .set_block_requires("block:blk-a", &[EntityUri::from_raw("block:blk-b")])
            .await
            .expect("set_block_requires(blk-a)");
        settle(&env).await;

        let tags_boot1 = tag_pairs(&env).await;
        let requires_boot1 = requires_pairs(&env).await;
        assert!(
            tags_boot1.contains(&("block:blk-a".to_string(), "proj".to_string())),
            "boot-1 never projected the `proj` tag — premise broken. block_tags = {tags_boot1:?}"
        );
        assert!(
            !requires_boot1.is_empty(),
            "boot-1 never projected the requires edge — premise broken."
        );

        env.stop_app().await.expect("stop_app after boot-1");

        // ── Boot 2 over the SAME test.db + vault ────────────────────────
        env.start_app(true).await.expect("boot-2 start_app");
        wait_for_seed(&env).await;
        settle(&env).await;

        let tags_boot2 = tag_pairs(&env).await;
        let requires_boot2 = requires_pairs(&env).await;
        eprintln!(
            "[reboot-junction-edge] boot-2 block_tags = {tags_boot2:?} block_requires = \
             {requires_boot2:?}"
        );

        let lost_tags: Vec<_> = tags_boot1.difference(&tags_boot2).collect();
        assert!(
            lost_tags.is_empty(),
            "[reboot] `block_tags` LOST rows across a restart: {lost_tags:?}\nboot-2 = \
             {tags_boot2:?}"
        );
        let lost_requires: Vec<_> = requires_boot1.difference(&requires_boot2).collect();
        assert!(
            lost_requires.is_empty(),
            "[reboot] `block_requires` LOST rows across a restart: {lost_requires:?}\nboot-2 = \
             {requires_boot2:?}"
        );
    });
}

/// Diagnostic: the `(file_id, content_hash)` pairs the NEXT boot's cold-boot
/// fast path loads. A file with no stored hash can never be skipped, so an
/// empty result means the harness does not exercise the skip at all — the
/// reboot seam would then be covered only in its re-ingesting form.
async fn file_hashes(env: &TestEnvironment) -> Vec<(String, String)> {
    env.query_sql("SELECT id, content_hash FROM file")
        .await
        .expect("query file hashes")
        .iter()
        .map(|r| {
            let id = r
                .get("id")
                .and_then(|v| v.as_string())
                .unwrap_or("<none>")
                .to_string();
            let hash = r
                .get("content_hash")
                .and_then(|v| v.as_string())
                .unwrap_or("")
                .to_string();
            (id, hash)
        })
        .collect()
}

/// The EXISTING-DATABASE repair case: every already-deployed database has had
/// its junctions wiped by a previous boot's `DROP TABLE`, and the
/// unchanged-file fast path means nothing re-derives them. Removing the DROP
/// fixes new databases but leaves those permanently degraded.
///
/// This models such a database directly: populate, wipe the junction the way a
/// pre-repair boot did, then reboot. Boot 2 must bring the rows back.
///
/// It doubles as the experiment that decides HOW the repair must work: if a
/// plain re-ingest restores the rows, the repair only has to refuse the
/// fast-path skip for one boot; if it does not, the rows must be re-projected
/// from the Loro tree, which is the authority that holds them.
#[test]
fn wiped_junction_is_repaired_on_next_boot() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = TestEnvironment::new(rt).expect("TestEnvironment::new");
        env.write_org_file("alpha.org", ALPHA_ORG)
            .await
            .expect("write alpha.org");
        env.write_org_file("beta.org", BETA_ORG)
            .await
            .expect("write beta.org");

        env.start_app(true).await.expect("boot-1 start_app");
        wait_for_seed(&env).await;
        settle(&env).await;

        let boot1 = tag_pairs(&env).await;
        assert!(
            boot1.iter().any(|(_, t)| t == "Page"),
            "premise: boot-1 derives Page tags. block_tags = {boot1:?}"
        );
        eprintln!(
            "[wipe-repair] boot-1 file hashes = {:?}",
            file_hashes(&env).await
        );
        let stamp1 = updated_at(&env, "block:blk-a").await;
        let root_stamp1 = updated_at(&env, "block:page-alpha").await;

        // Exactly what a pre-repair boot's `DROP TABLE block_tags` left behind:
        // an empty junction over an intact `block_raw` and an intact Loro tree.
        env.query_sql("DELETE FROM block_tags")
            .await
            .expect("wipe block_tags");
        assert!(
            tag_pairs(&env).await.is_empty(),
            "the wipe itself must leave the junction empty"
        );

        env.stop_app().await.expect("stop_app after boot-1");

        env.start_app(true).await.expect("boot-2 start_app");
        wait_for_seed(&env).await;
        settle(&env).await;

        let boot2 = tag_pairs(&env).await;
        eprintln!("[wipe-repair] boot-2 block_tags = {boot2:?}");

        // WHAT ACTUALLY REPAIRS THIS — measured, and NOT what an earlier
        // revision of this test claimed.
        //
        // Sampling the UNTAGGED child says nothing: it is untouched either way
        // (an unwiped reboot leaves both untouched, which is why
        // `reboot_harness_takes_the_cold_boot_fast_path_skip` has to corrupt
        // content instead of comparing stamps). The TAGGED ROOT is the
        // discriminator, and it IS rewritten here. The wipe makes the doc-root
        // invisible to the page lookup (`block_raw JOIN block_tags WHERE
        // tag='Page'`); that lookup misses, and the ingest path re-creates the
        // page with `set_page(true)`, restoring the tag.
        //
        // So the repair is re-derivation triggered by MISSING DERIVED STATE, not
        // by the file changing — which is exactly why it survives the fast-path
        // skip. Its BOUNDARY is the open question behind the live symptom: a
        // page whose re-creation cannot complete (quarantined file, identity
        // collision) never heals, and Martin's vault contains such files.
        let child_after = updated_at(&env, "block:blk-a").await;
        let root_after = updated_at(&env, "block:page-alpha").await;
        eprintln!(
            "[wipe-repair] child blk-a {} | root page-alpha {}",
            if child_after == stamp1 {
                "untouched"
            } else {
                "rewritten"
            },
            if root_after == root_stamp1 {
                "untouched"
            } else {
                "REWRITTEN"
            }
        );
        assert_ne!(
            root_after, root_stamp1,
            "this test documents repair-by-re-derivation of the TAGGED ROOT; the root was NOT \
             rewritten, so the rows returned by some other route and this explanation is stale"
        );

        let lost: Vec<_> = boot1.difference(&boot2).collect();
        assert!(
            lost.is_empty(),
            "[repair] a database whose `block_tags` was wiped by an earlier boot is NOT repaired \
             on the next boot — {} of {} rows are still missing: {lost:?}. Every already-deployed \
             database stays permanently degraded.",
            lost.len(),
            boot1.len()
        );
    });
}

/// `updated_at` for a seeded block. Every ingest rewrites it
/// (`block_params.rs` stamps `now_millis()` on each write), so it discriminates
/// "boot 2 re-ingested this file" from "boot 2 took the cold-boot fast-path
/// skip" without needing log capture.
async fn updated_at(env: &TestEnvironment, id: &str) -> i64 {
    env.query_sql(&format!(
        "SELECT updated_at FROM block_raw WHERE id = '{id}'"
    ))
    .await
    .expect("query updated_at")
    .first()
    .and_then(|r| r.get("updated_at"))
    .and_then(|v| v.as_i64())
    .expect("block must exist")
}

/// FIDELITY PROBE (task #87): does the reboot harness actually exercise the
/// cold-boot fast-path SKIP?
///
/// It matters because the skip is the half of the junction-loss bug that makes
/// the loss PERMANENT: a re-ingested file re-derives its `Page` tag from org
/// (`parser.rs` `set_page(true)`), so only a SKIPPED file stays empty. If this
/// harness re-ingests every file on boot 2, then no test built on it can go red
/// for the live symptom, and the coverage it appears to provide is illusory.
///
/// THE DISCRIMINATOR: corrupt a block's `content` in SQL ONLY between boots,
/// leaving the org file and the stored `file.content_hash` untouched. The org
/// text and the store now disagree, so an ingest that RUNS finds
/// `content_differs` true and overwrites the corruption from disk; an ingest
/// that is SKIPPED leaves it standing. Surviving corruption therefore proves
/// the skip fired.
///
/// It has to be a divergence like this. Comparing a block's `updated_at` (or
/// any field) across an UNWIPED reboot proves nothing, because on an unchanged
/// vault a re-ingest writes nothing either — an earlier revision of this file
/// did exactly that and read a no-op as evidence of a skip.
#[test]
fn reboot_harness_takes_the_cold_boot_fast_path_skip() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = TestEnvironment::new(rt).expect("TestEnvironment::new");
        env.write_org_file("alpha.org", ALPHA_ORG)
            .await
            .expect("write alpha.org");

        env.start_app(true).await.expect("boot-1 start_app");
        wait_for_seed_alpha(&env).await;
        settle(&env).await;

        let hashes = file_hashes(&env).await;
        assert!(
            hashes.iter().any(|(_, h)| !h.is_empty()),
            "premise: boot-1 persists a `file.content_hash`, without which the skip is \
             unreachable by construction. file rows = {hashes:?}"
        );

        // SQL only — the org file on disk and its stored hash stay as they were,
        // so the fast path still believes this file is unchanged.
        env.query_sql(
            "UPDATE block_raw SET content = 'CORRUPTED-BY-PROBE' WHERE id = 'block:blk-a'",
        )
        .await
        .expect("corrupt block content in SQL");

        env.stop_app().await.expect("stop_app");
        env.start_app(true).await.expect("boot-2 start_app");
        wait_for_seed_alpha(&env).await;
        settle(&env).await;

        let after = env
            .query_sql("SELECT content FROM block_raw WHERE id = 'block:blk-a'")
            .await
            .expect("read content after reboot")
            .first()
            .and_then(|r| r.get("content"))
            .and_then(|v| v.as_string())
            .expect("block:blk-a must exist")
            .to_string();

        let skipped = after == "CORRUPTED-BY-PROBE";
        eprintln!(
            "[fidelity] boot-2 content = {after:?} => boot-2 {}",
            if skipped {
                "TOOK THE SKIP (corruption survived — no ingest)"
            } else {
                "RE-INGESTED (corruption overwritten from org — skip did NOT fire)"
            }
        );
        // Asserts the PREMISE only, deliberately. Two independent runs of this
        // discriminator disagree on the verdict (see the module doc), so pinning
        // either answer here would encode a conclusion the evidence does not
        // yet support. Read the printed verdict.
        assert!(
            hashes.iter().any(|(_, h)| !h.is_empty()),
            "the stored `file.content_hash` must still exist for the skip to be reachable at all"
        );
    });
}

/// The SECOND, separable finding: why the left sidebar keeps rendering a tree
/// while its own SQL returns nothing.
///
/// The sidebar's backing query (`assets/default/index.org`,
/// `left_sidebar::src::0`) is served through `watch_view`, i.e. a persistent
/// `watch_view_<hash>` IVM matview. That matview survives the process restart
/// carrying its boot-1 rows, while the base-table junction it derives from was
/// dropped and recreated empty. The UI then shows data the base tables no
/// longer contain — a silent degradation (error-handling priority 4), not a
/// disclosed fallback.
///
/// The assertion is the convergence contract itself: the sidebar's watch view
/// must agree with the same SELECT executed against the base tables. It is
/// deliberately independent of the junction fix, so it stays meaningful as the
/// standing guard against stale derived data across a reboot.
/// The shipped sidebar SELECT minus its `ORDER BY b.content ASC`. The trailing
/// clause is dropped deliberately: the watch-view read path re-appends it to
/// the generated `SELECT ... FROM watch_view_<hash>` query, where the alias `b`
/// no longer exists, so the view errors with `no such table: b` before it can
/// be read at all. That is a separate defect (reported alongside this one);
/// this test is about WHICH ROWS the view holds after a reboot, and row
/// identity is what the assertion compares.
const SIDEBAR_SQL: &str = "SELECT b.* FROM block b JOIN block_tags bt ON bt.block_id = b.id WHERE \
                           bt.tag = 'Page' AND b.id != 'block:__default__'";

async fn ad_hoc_ids(env: &TestEnvironment) -> BTreeSet<String> {
    env.query(SIDEBAR_SQL, holon_api::QueryLanguage::HolonSql)
        .await
        .expect("ad-hoc sidebar SQL")
        .iter()
        .filter_map(|r| r.get("id").and_then(|v| v.as_string()).map(str::to_string))
        .collect()
}

async fn watch_view_ids(env: &TestEnvironment) -> BTreeSet<String> {
    let watch = env
        .engine()
        .watch_view(SIDEBAR_SQL)
        .await
        .expect("watch_view(sidebar SQL)");
    watch
        .initial_rows
        .iter()
        .filter_map(|r| r.get("id").and_then(|v| v.as_string()).map(str::to_string))
        .collect()
}

#[test]
fn sidebar_watch_view_agrees_with_base_tables_after_reboot() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = TestEnvironment::new(rt).expect("TestEnvironment::new");
        env.write_org_file("alpha.org", ALPHA_ORG)
            .await
            .expect("write alpha.org");
        env.write_org_file("beta.org", BETA_ORG)
            .await
            .expect("write beta.org");

        // ── Boot 1: register the sidebar watch so its matview exists on disk
        env.start_app(true).await.expect("boot-1 start_app");
        wait_for_seed(&env).await;
        settle(&env).await;

        let adhoc1 = ad_hoc_ids(&env).await;
        let watch1 = watch_view_ids(&env).await;
        eprintln!("[sidebar-reboot] boot-1 ad_hoc={adhoc1:?} watch_view={watch1:?}");
        assert!(
            !adhoc1.is_empty(),
            "boot-1 sidebar query returned nothing — premise broken"
        );
        assert_eq!(
            adhoc1, watch1,
            "boot-1 sidebar watch view already diverges from its own SELECT — premise broken"
        );

        env.stop_app().await.expect("stop_app after boot-1");

        // ── Boot 2 over the SAME test.db + vault ────────────────────────
        env.start_app(true).await.expect("boot-2 start_app");
        wait_for_seed(&env).await;
        settle(&env).await;

        let adhoc2 = ad_hoc_ids(&env).await;
        let watch2 = watch_view_ids(&env).await;
        eprintln!("[sidebar-reboot] boot-2 ad_hoc={adhoc2:?} watch_view={watch2:?}");

        assert_eq!(
            adhoc2, watch2,
            "[reboot] the sidebar's watch matview and its own base-table SELECT DISAGREE after a \
             restart — the UI renders the matview's surviving boot-1 rows as if they were live, \
             with no disclosure."
        );
    });
}
