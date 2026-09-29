//! A session's shutdown leaves every acknowledged edit on disk, and the next
//! writer — with a DB of its own — keeps it.
//!
//! The write-back loop lets shutdown win over its backlog, so a shutdown that
//! did not first wait for the write-back chain left an edit in `.loro` but not
//! in its org file. The next boot with a fresh DB (a DB drop, `holon-mcp` on
//! `:memory:`, a hand-over between builds) then ingested the stale file and
//! deleted the edit from Loro (bugfunnel
//! `2026-09-29-a-fresh-db-boot-deletes-an-edit-whose-write-back-shutdown-dropped`).
//!
//! @pbt kind harness
//! @pbt covers shutdown-flushes-writeback — `shutdown_session` returns only
//! after the org write-back wrote every edit, so a hand-over to a fresh DB
//! loses nothing

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use holon_api::EntityName;
use holon_api::OpOrigin;
use holon_api::Value;
use holon_core::storage::types::StorageEntity;
use holon_frontend::config::HolonConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::config::VaultConfig;
use holon_loro::LoroBackend;
use holon_loro::LoroDocument;

const VAULT_ORG: &str = "\
* Hand-over page
:PROPERTIES:
:ID: hand-over-page
:END:
";

struct Booted {
    engine: Arc<holon::api::BackendEngine>,
    injector: fluxdi::Injector,
    _session: Arc<holon_frontend::FrontendSession>,
}

/// One state dir (DB + config) per session, as each build keeps its own DB.
async fn boot(vault: &Path, state: &Path) -> Booted {
    let holon_config = HolonConfig {
        db_path: Some(state.join("holon.db")),
        vault: VaultConfig {
            root: Some(vault.to_path_buf()),
        },
        ..Default::default()
    };
    let (session, engine, injector) = holon_app::new_from_config_with_di(
        holon_config,
        SessionConfig::new(holon_api::UiInfo::permissive()),
        state.to_path_buf(),
        HashSet::new(),
        |_| Ok(()),
        |injector| injector.clone(),
    )
    .await
    .expect("the shared wiring boots a session");
    Booted {
        engine,
        injector,
        _session: session,
    }
}

async fn create_block(engine: &holon::api::BackendEngine, bare_id: &str) {
    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String(format!("block:{bare_id}")));
    params.insert(
        "parent_id".into(),
        Value::String("block:hand-over-page".to_string()),
    );
    params.insert("content".into(), Value::String(bare_id.to_string()));
    engine
        .execute_operation(&EntityName::new("block"), "create", params, OpOrigin::User)
        .await
        .unwrap_or_else(|e| panic!("creating `{bare_id}`: {e}"));
}

async fn snapshot_holds(vault: &Path, bare_id: &str) -> bool {
    let path = vault.join(".loro").join(holon_loro::GLOBAL_SNAPSHOT_NAME);
    let doc = LoroDocument::load_from_file(&path, "probe".to_string())
        .unwrap_or_else(|e| panic!("the snapshot at {} does not load: {e:#}", path.display()));
    LoroBackend::from_document(Arc::new(doc))
        .resolve_to_tree_id(&format!("block:{bare_id}"))
        .await
        .is_some()
}

fn page_file(vault: &Path) -> String {
    std::fs::read_to_string(vault.join("page.org")).expect("read the page file")
}

#[tokio::test(flavor = "multi_thread")]
async fn an_edit_made_just_before_shutdown_survives_a_hand_over_to_a_fresh_db() {
    let vault = tempfile::tempdir().expect("create the vault dir");
    std::fs::write(vault.path().join("page.org"), VAULT_ORG).expect("write the vault page");
    let state_a = tempfile::tempdir().expect("state dir A");
    let state_b = tempfile::tempdir().expect("state dir B");

    // No settle between the edit and the shutdown: waiting for the write-back
    // is the shutdown's job, not the caller's.
    let a = boot(vault.path(), state_a.path()).await;
    create_block(&a.engine, "written-by-a").await;
    holon_app::shutdown_session(&a.injector)
        .await
        .expect("the first writer shuts down");

    assert!(
        page_file(vault.path()).contains(":ID: written-by-a"),
        "shutdown returned with an acknowledged edit missing from its org file:\n{}",
        page_file(vault.path())
    );

    let b = boot(vault.path(), state_b.path()).await;
    holon_app::shutdown_session(&b.injector)
        .await
        .expect("the next writer shuts down");

    assert!(
        snapshot_holds(vault.path(), "written-by-a").await,
        "the hand-over to a fresh DB deleted the first writer's edit from Loro"
    );
    assert!(
        page_file(vault.path()).contains(":ID: written-by-a"),
        "the hand-over to a fresh DB removed the first writer's edit from its org file:\n{}",
        page_file(vault.path())
    );
}
