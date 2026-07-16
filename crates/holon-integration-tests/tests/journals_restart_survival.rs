//! Reproducer for BugFunnel row 144 (dogfood #5, 2026-07-13):
//! "Restart-over-existing-vault LOSES the Journals page view definition".
//!
//! Symptom: the seeded `block:journals::src::0` (display query) +
//! `block:journals::render::0` (render spec) are DIRECT children of the
//! programmatic `block:journals` Page. On first boot they land in `block_raw`,
//! but org writeback emits them into NEITHER `__default__.org` NOR a
//! `Journals.org` (the SqlOnly seed routes them to `_routing_doc_uri =
//! block:__default__` while their parent is `block:journals` — routing doc and
//! parent doc disagree). On the SECOND boot the file-authority ingest finds no
//! file that owns them and DELETES both from `block_raw` — the Journals page
//! silently loses its view definition after one restart.
//!
//! Secondary: `sentinel:no_parent` must NEVER materialize as a `block_raw` row.
//!
//! This is a FULL-BOOT, writeback→re-ingest-across-restart property the
//! keystone never exercises. Reuses the two-boot persistent-DB harness shape
//! from `matview_reboot_duplicate_repro.rs`. Expected to FAIL until the seed
//! routing is fixed.
//!
//! @pbt kind harness
//! @pbt covers restart-persistence(journals) — Journals view definition survives restart-over-existing-vault (BugFunnel 144)
//! @pbt overlaps general_e2e_composed_pbt — kept: keystone has NO restart transition, cannot reach this class

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

const JOURNALS_SRC: &str = "block:journals::src::0";
const JOURNALS_RENDER: &str = "block:journals::render::0";
const JOURNALS_PAGE: &str = "block:journals";

/// Wait until the seed has landed the journals view-definition blocks in
/// `block_raw`.
async fn wait_for_journals_seed(env: &TestEnvironment) {
    let deadline = std::time::Instant::now() + Duration::from_secs(25);
    loop {
        let rows = env
            .query_sql(&format!(
                "SELECT id FROM block_raw WHERE id IN ('{JOURNALS_SRC}', '{JOURNALS_RENDER}')"
            ))
            .await
            .expect("query block_raw");
        if rows.len() >= 2 {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "seed never populated the journals view blocks (have {} rows)",
            rows.len()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Present in the READ surface — the `block` matview every consumer (UI, MCP,
/// writeback) reads. `block_raw` additionally holds the self-parented
/// `sentinel:no_parent` FK-anchor row by design; the matview excludes it
/// (`WHERE b.id != 'sentinel:no_parent'`), so this is the surface that matters
/// for "is X a real block".
async fn present(env: &TestEnvironment, id: &str) -> bool {
    let rows = env
        .query(
            &format!("SELECT id FROM block WHERE id = '{id}'"),
            holon_api::QueryLanguage::HolonSql,
        )
        .await
        .expect("query block matview");
    !rows.is_empty()
}

/// Direct `block_raw` presence — includes the FK-anchor sentinel row. Used only
/// to document that the sentinel lives in the base table by design.
async fn present_raw(env: &TestEnvironment, id: &str) -> bool {
    let rows = env
        .query_sql(&format!("SELECT id FROM block_raw WHERE id = '{id}'"))
        .await
        .expect("query block_raw");
    !rows.is_empty()
}

/// Dump every org file the vault currently has, with its content — for
/// diagnosing which file (if any) the journals view blocks landed in.
async fn dump_org_files(env: &TestEnvironment) -> String {
    use holon_filesystem::FileSystem;
    let scanned = env
        .org_fs
        .scan_directory(env.org_root())
        .await
        .expect("scan org root");
    let mut out = String::new();
    let mut files = scanned.files.clone();
    files.sort();
    for f in files {
        let content = env.org_fs.read_to_string(&f).await.unwrap_or_default();
        out.push_str(&format!("\n===== {} =====\n{content}", f.display()));
    }
    out
}

#[test]
fn journals_view_definition_survives_restart() {
    // Empty (fresh) vault: `__default__` page IS seeded.
    run_restart_survival(None);
}

#[test]
fn journals_view_definition_survives_restart_over_existing_vault() {
    // Prod-faithful dogfood #5 shape: the vault ALREADY has a user org file, so
    // the seed runs the `fresh=false` path — the `__default__` page is NOT
    // seeded, so a `_routing_doc_uri = block:__default__` set on the journals
    // view blocks is a DANGLING reference (no such document/file). Writeback
    // that trusts it strands the blocks in no file → boot-2 ingest deletes them.
    run_restart_survival(Some((
        "Inbox.org",
        "#+TITLE: Inbox\n* a user note\n:PROPERTIES:\n:ID: user-note-1\n:END:\n",
    )));
}

fn run_restart_survival(existing_vault_file: Option<(&str, &str)>) {
    let rt = runtime();
    let existing = existing_vault_file.map(|(f, c)| (f.to_string(), c.to_string()));
    rt.clone().block_on(async move {
        let mut env = TestEnvironment::new(rt).expect("TestEnvironment::new");
        // Match the dogfood: desktop runs SqlOnly (Loro off), so the seed takes
        // the `build_block_params` SqlOnly fallback whose routing metadata is
        // the reported defect.
        env.set_enable_loro(false);
        if let Some((f, c)) = &existing {
            env.write_org_file(f, c)
                .await
                .expect("write existing vault file");
        }

        // ── Boot 1 ──────────────────────────────────────────────────────
        env.start_app(true).await.expect("boot-1 start_app");
        wait_for_journals_seed(&env).await;
        // Let writeback settle so the org files are on disk.
        tokio::time::sleep(Duration::from_millis(500)).await;
        eprintln!(
            "[journals-restart] boot-1 org files:{}",
            dump_org_files(&env).await
        );
        assert!(
            present(&env, JOURNALS_SRC).await && present(&env, JOURNALS_RENDER).await,
            "boot-1 must seed both journals view blocks"
        );

        env.stop_app().await.expect("stop_app after boot-1");

        // ── Boot 2 over the SAME test.db + vault ────────────────────────
        env.start_app(true).await.expect("boot-2 start_app");
        wait_for_journals_seed(&env).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        eprintln!(
            "[journals-restart] boot-2 org files:{}",
            dump_org_files(&env).await
        );

        let src_alive = present(&env, JOURNALS_SRC).await;
        let render_alive = present(&env, JOURNALS_RENDER).await;

        assert!(
            src_alive && render_alive,
            "[journals-restart] Journals view definition LOST after restart: \
             src_alive={src_alive} render_alive={render_alive}. The seeded {JOURNALS_SRC} / \
             {JOURNALS_RENDER} (children of the {JOURNALS_PAGE} Page) were deleted by boot-2 \
             file-authority ingest because writeback put them in no org file.{}",
            dump_org_files(&env).await
        );

        // Secondary (BugFunnel row 144): `sentinel:no_parent` must NEVER surface
        // as a real block. It lives in `block_raw` as the self-parented FK anchor
        // (schema bootstrap, `INSERT OR IGNORE`, every boot) — that is by design
        // and SYMMETRIC across boots — but every read surface (`block` matview,
        // get_blocks, blocks_with_paths) excludes it by id. Assert it never
        // reaches the read surface on the second boot (the reported asymmetry),
        // and document that the base-table anchor is present on both boots.
        assert!(
            !present(&env, "sentinel:no_parent").await,
            "[journals-restart] sentinel:no_parent leaked into the `block` read surface after \
             restart — it must only ever exist as the excluded block_raw FK anchor"
        );
        assert!(
            present_raw(&env, "sentinel:no_parent").await,
            "[journals-restart] the sentinel FK anchor row must exist in block_raw \
             (self-parented, satisfies the parent FK) — its absence would break root-block inserts"
        );
    });
}
