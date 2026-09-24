//! Sharing a page and revoking it by deleting the page cannot be undone, and
//! both still land in the queryable op history (`block_history`, ADR 0024 P8).

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

use holon::api::BackendEngine;
use holon_api::EntityName;
use holon_api::HistoryQuery;
use holon_api::HistoryStore;
use holon_api::OpOrigin;
use holon_api::StorageEntity;
use holon_api::Value;
use holon_frontend::FrontendSession;
use holon_frontend::config::HolonConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::config::VaultConfig;
use holon_loro::share_credentials::ShareCredentials;

const PAGE: &str = "block:revoke-probe-page";

const VAULT_ORG: &str = "#+ID: revoke-probe-doc\n* Revoke probe :Page:\n:PROPERTIES:\n:ID: \
                         revoke-probe-page\n:END:\n";

async fn boot(dir: &std::path::Path) -> (Arc<BackendEngine>, Arc<FrontendSession>) {
    let config = HolonConfig {
        db_path: Some(dir.join("revoke.db")),
        vault: VaultConfig {
            root: Some(dir.to_path_buf()),
        },
        ..Default::default()
    };
    let credentials = Arc::new(ShareCredentials::in_memory(&dir.to_string_lossy()));
    let (session, engine, ()) = holon_app::new_from_config_with_di(
        config,
        SessionConfig::new(holon_api::UiInfo::permissive()),
        dir.to_path_buf(),
        HashSet::new(),
        move |injector| {
            injector
                .provide::<ShareCredentials>(fluxdi::Provider::root(move |_| credentials.clone()));
            Ok(())
        },
        |_| (),
    )
    .await
    .expect("the shared wiring must boot a session");
    (engine, session)
}

async fn user_op(engine: &BackendEngine, entity: &str, op: &str, params: &[(&str, &str)]) {
    let params: StorageEntity = params
        .iter()
        .map(|(k, v)| ((*k).into(), Value::String((*v).to_string())))
        .collect::<HashMap<_, _>>();
    engine
        .execute_operation(&EntityName::new(entity), op, params, OpOrigin::User)
        .await
        .unwrap_or_else(|e| panic!("{entity}.{op}: {e:#}"));
}

async fn ops_recorded_for(engine: &BackendEngine, block: &str) -> Vec<String> {
    holon::api::TursoHistoryStore::new(engine.db_handle().clone())
        .query(&HistoryQuery::for_block(block))
        .await
        .expect("query block_history")
        .into_iter()
        .map(|event| event.op_name)
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn sharing_a_page_and_revoking_it_are_both_in_the_history() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("probe.org"), VAULT_ORG).expect("write the probe vault");
    let (engine, _session) = boot(dir.path()).await;

    user_op(
        &engine,
        "tree",
        "share_subtree",
        &[("id", PAGE), ("retention", "none")],
    )
    .await;
    let shared = ops_recorded_for(&engine, PAGE).await;
    assert!(
        shared.iter().any(|op| op == "share_subtree"),
        "the share left no history row: {shared:?}"
    );

    user_op(&engine, "block", "delete", &[("id", PAGE)]).await;
    let revoked = ops_recorded_for(&engine, PAGE).await;
    assert!(
        revoked.iter().any(|op| op == "delete"),
        "the revoking delete left no history row: {revoked:?}"
    );
    assert!(
        !engine.can_undo().await,
        "neither the share nor its revoke can be undone"
    );
}
