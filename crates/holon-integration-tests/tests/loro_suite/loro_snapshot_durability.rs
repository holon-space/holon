//! A block that arrives through org ingest must reach the on-disk `.loro`
//! snapshot, and a snapshot that lost such a block must not lose it for good.
//!
//! Bugfunnel `2026-09-22-org-ingest-loro-writes-never-reach-disk` and
//! `2026-09-22-stale-loro-snapshot-drops-blocks-on-boot`.
//!
//! @pbt kind harness
//! @pbt covers restart-persistence(loro-snapshot) — ingest durability and
//!   stale-snapshot recovery over a real stop/start
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone has no
//!   transition that swaps the snapshot for an older one

use std::sync::Arc;
use std::time::Duration;

use holon_filesystem::FileSystem;
use holon_integration_tests::TestEnvironment;
use holon_loro::DocScope;
use holon_loro::LoroBackend;
use holon_loro::LoroDocument;

const VAULT_ORG: &str = "\
#+TITLE: Vault
#+ID: page-vault

* First
:PROPERTIES:
:ID: blk-first
:END:
";

const APPENDED: &str = "\
* Appended by another editor
:PROPERTIES:
:ID: blk-appended
:END:
";

const APPENDED_ID: &str = "block:blk-appended";

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("tokio runtime"),
    )
}

async fn wait_for_sql_row(env: &TestEnvironment, id: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let rows = env
            .query_sql(&format!("SELECT id FROM block_raw WHERE id = '{id}'"))
            .await
            .expect("query block_raw");
        if !rows.is_empty() {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "org ingest never landed {id} in block_raw"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn settle(env: &TestEnvironment) {
    env.wait_for_loro_quiescence(Duration::from_secs(15)).await;
    env.wait_for_cdc_quiescent(Duration::from_millis(250), Duration::from_secs(15))
        .await;
}

fn snapshot_path(env: &TestEnvironment) -> std::path::PathBuf {
    let store = env.loro_doc_store().expect("Loro-enabled session");
    let dir = store
        .try_read()
        .expect("doc store lock is free between steps")
        .storage_dir()
        .to_path_buf();
    dir.join(holon_loro::GLOBAL_SNAPSHOT_NAME)
}

fn sidecar_path(snapshot: &std::path::Path) -> std::path::PathBuf {
    snapshot.with_file_name(holon_loro::loro_sync_controller::SIDECAR_FILENAME)
}

async fn snapshot_holds(path: &std::path::Path, id: &str) -> bool {
    let doc = LoroDocument::load_from_file(path, "probe".to_string())
        .unwrap_or_else(|e| panic!("the snapshot at {} does not load: {e:#}", path.display()));
    LoroBackend::from_document(Arc::new(doc))
        .resolve_to_tree_id(id)
        .await
        .is_some()
}

async fn live_tree_holds(env: &TestEnvironment, id: &str) -> bool {
    let doc = env
        .loro_doc_store()
        .expect("Loro-enabled session")
        .read()
        .await
        .get_doc(DocScope::Global)
        .await
        .expect("global Loro doc");
    LoroBackend::from_document(doc)
        .resolve_to_tree_id(id)
        .await
        .is_some()
}

async fn append_to_vault(env: &TestEnvironment) {
    env.write_org_file("vault.org", &format!("{VAULT_ORG}\n{APPENDED}"))
        .await
        .expect("append to vault.org");
    wait_for_sql_row(env, APPENDED_ID).await;
    settle(env).await;
}

/// The ingest's Loro write is on disk once SQL shows it, so a crash at that
/// point loses nothing; and it is still there after a clean quit.
#[test]
fn an_ingested_block_is_on_disk_once_sql_shows_it() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = TestEnvironment::new(rt).expect("TestEnvironment::new");
        env.write_org_file("vault.org", VAULT_ORG)
            .await
            .expect("write vault.org");
        env.start_app(true).await.expect("start_app");
        wait_for_sql_row(&env, "block:blk-first").await;
        settle(&env).await;

        append_to_vault(&env).await;
        let path = snapshot_path(&env);
        assert!(
            path.exists() && snapshot_holds(&path, APPENDED_ID).await,
            "block_raw holds {APPENDED_ID} but the .loro snapshot at {} does not: a crash now \
             rolls the Loro authority back behind SQL",
            path.display()
        );

        env.stop_app().await.expect("stop_app");
        assert!(
            snapshot_holds(&path, APPENDED_ID).await,
            "{APPENDED_ID} is missing from the .loro snapshot after a clean quit"
        );
    });
}

/// A snapshot older than the vault (written before the ingest that appended a
/// block, as any lost save leaves it) is reloaded at boot next to an org file
/// and a `file.content_hash` that already include the block. The boot must
/// bring the block back into the authority from the file.
#[test]
fn a_stale_snapshot_gets_its_missing_ingested_block_back_at_boot() {
    boot_over_a_stale_snapshot(SidecarAtBoot::Kept);
}

/// Without the sidecar nothing records what SQL was synced from, so the boot
/// must not trust the stale snapshot either.
#[test]
fn a_stale_snapshot_without_its_sidecar_gets_its_missing_ingested_block_back_at_boot() {
    boot_over_a_stale_snapshot(SidecarAtBoot::Deleted);
}

enum SidecarAtBoot {
    Kept,
    Deleted,
}

fn boot_over_a_stale_snapshot(sidecar: SidecarAtBoot) {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = TestEnvironment::new(rt).expect("TestEnvironment::new");
        env.write_org_file("vault.org", VAULT_ORG)
            .await
            .expect("write vault.org");
        env.start_app(true).await.expect("boot-1 start_app");
        wait_for_sql_row(&env, "block:blk-first").await;
        settle(&env).await;

        env.loro_doc_store()
            .expect("Loro-enabled session")
            .read()
            .await
            .save_all()
            .await
            .expect("save the pre-append snapshot");
        let path = snapshot_path(&env);
        let stale = std::fs::read(&path).expect("read the pre-append snapshot");
        assert!(
            !snapshot_holds(&path, APPENDED_ID).await,
            "premise: the pre-append snapshot must not hold {APPENDED_ID}"
        );

        append_to_vault(&env).await;
        env.stop_app().await.expect("stop_app after boot-1");
        // The file row's hash only matches the disk bytes after an ordinary
        // reboot has re-stamped it, and that match is what arms the boot skip.
        env.start_app(true).await.expect("boot-2 start_app");
        wait_for_sql_row(&env, APPENDED_ID).await;
        settle(&env).await;
        env.stop_app().await.expect("stop_app after boot-2");
        std::fs::write(&path, &stale).expect("put the stale snapshot back");
        if let SidecarAtBoot::Deleted = sidecar {
            std::fs::remove_file(sidecar_path(&path)).expect("delete the watermark sidecar");
        }

        env.start_app(true).await.expect("boot-3 start_app");
        assert_eq!(
            snapshot_path(&env)
                .canonicalize()
                .expect("boot-3 snapshot path"),
            path.canonicalize().expect("boot-1 snapshot path"),
            "premise: boot 3 must reload the snapshot boot 1 wrote"
        );
        wait_for_sql_row(&env, "block:blk-first").await;
        settle(&env).await;

        let on_disk = String::from_utf8(
            env.org_fs
                .read(&env.org_root().join("vault.org"))
                .await
                .expect("read vault.org"),
        )
        .expect("vault.org is UTF-8");
        let in_sql = !env
            .query_sql(&format!(
                "SELECT id FROM block_raw WHERE id = '{APPENDED_ID}'"
            ))
            .await
            .expect("query block_raw")
            .is_empty();
        let in_loro = live_tree_holds(&env, APPENDED_ID).await;
        assert!(
            in_loro && in_sql && on_disk.contains("blk-appended"),
            "after booting over the stale snapshot: Loro tree holds {APPENDED_ID}: {in_loro}, \
             block_raw holds it: {in_sql}, vault.org still carries it: {}\nvault.org:\n{on_disk}",
            on_disk.contains("blk-appended")
        );
    });
}

/// A sidecar that exists but does not decode is refused at boot, naming the
/// file.
#[test]
fn a_corrupt_sidecar_fails_the_boot() {
    const GARBAGE: [u8; 3] = [0xFF, 0xFF, 0xFF];
    assert!(
        loro::Frontiers::decode(&GARBAGE).is_err(),
        "premise: the garbage bytes must not decode as frontiers"
    );
    let rt = runtime();
    let mut env = TestEnvironment::new(rt.clone()).expect("TestEnvironment::new");
    let sidecar = rt.block_on(async {
        env.write_org_file("vault.org", VAULT_ORG)
            .await
            .expect("write vault.org");
        env.start_app(true).await.expect("boot-1 start_app");
        wait_for_sql_row(&env, "block:blk-first").await;
        settle(&env).await;
        let sidecar = sidecar_path(&snapshot_path(&env));
        env.stop_app().await.expect("stop_app after boot-1");
        sidecar
    });
    assert!(
        sidecar.exists(),
        "premise: boot 1 wrote its sidecar at {}",
        sidecar.display()
    );
    std::fs::write(&sidecar, GARBAGE).expect("corrupt the sidecar");

    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rt.block_on(env.start_app(true))
    }));
    let failure = match outcome {
        Ok(Ok(())) => panic!(
            "boot 2 succeeded over the corrupt sidecar at {}",
            sidecar.display()
        ),
        Ok(Err(e)) => format!("{e:#}"),
        Err(panic) => panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_else(|| "<non-string panic>".to_string()),
    };
    assert!(
        failure.contains(&sidecar.display().to_string()),
        "the boot failure must name the corrupt sidecar {}: {failure}",
        sidecar.display()
    );
}
