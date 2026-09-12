//! The origin and hosts an introduced connection's row carries must reach the
//! table the Settings surface reads.
//!
//! `IntegrationRow` has carried `origin` and `hosts` since the disclosure
//! landed, and `introduced_connection_row_disclosure.rs` pins that they are
//! correct. What nobody checked is that they travel: the Settings modal renders
//! a `live_query` over `integration_state`, and that mirror had no column for
//! either, so the two fields were computed on every projection and dropped on
//! the floor. A disclosure that never leaves the view model discloses nothing —
//! the dogfood pass saw an introduced connection's row with the same five
//! columns as a bundled one.
//!
//! Bundled connections project both as the empty string rather than NULL: the
//! surface tells "shipped with this build" from "introduced by a file" by the
//! origin being empty, and a NULL would make every reader handle two spellings
//! of the same fact.
//!
//! @pbt kind harness
//! @pbt covers introduced-connection-disclosure-mirrored — origin and hosts
//! reach `integration_state`, which is the only thing the Settings list reads
//! @pbt overlaps introduced_connection_row_disclosure — kept: that file pins
//! that the VALUES are right; this one pins that they travel

use std::sync::Arc;

use fluxdi::Module;
use fluxdi::Provider;
use holon_app::integration_projection::IntegrationStateProjector;
use holon_app::integration_projection::TABLE_COLUMNS;
use holon_app::integrations_settings::IntegrationsSettingsVm;
use holon_loro_wiring::EventInfraModule;
use holon_mcp_client::IntegrationConfigStore;

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build test runtime"),
    )
}

/// A connection whose manual calls exactly `hosts`, in the order given.
fn sidecar_calling(hosts: &[&str]) -> String {
    let tools: String = hosts
        .iter()
        .enumerate()
        .map(|(i, h)| {
            format!(
                "    - name: list{i}\n      tool_call_template:\n        call_template_type: \
                 http\n        http_method: GET\n        url: https://{h}/things\n"
            )
        })
        .collect();
    let holon_tools: String = (0..hosts.len())
        .map(|i| format!("    list{i}: {{}}\n"))
        .collect();
    format!(
        "schema_version: {}\ndisplay_name: \"Calendar\"\nutcp:\n  utcp_version: \"1.1.3\"\n  \
         manual_version: \"1.0.0\"\n  tools:\n{tools}holon:\n  tools:\n{holon_tools}entities: \
         {{}}\ntools: {{}}\n",
        holon_mcp_client::SIDECAR_SCHEMA_VERSION
    )
}

async fn fresh_engine(db_path: std::path::PathBuf) -> Arc<holon::api::BackendEngine> {
    let (engine, ()) = holon::di::create_backend_engine_with_extras(
        db_path,
        |injector| {
            EventInfraModule.configure(injector).map_err(|e| {
                anyhow::anyhow!("configure EventInfraModule for the disclosure-mirror test: {e}")
            })?;
            injector.provide_into_set::<dyn holon_core::OperationProvider>(Provider::root(
                |resolver| {
                    let db = resolver
                        .resolve::<dyn holon::di::DbHandleProvider>()
                        .handle();
                    Arc::new(holon::core::SqlOperationProvider::new(
                        db,
                        holon::storage::BLOCK_WRITE_TABLE.to_string(),
                        "block".to_string(),
                        "block".to_string(),
                    )) as Arc<dyn holon_core::OperationProvider>
                },
            ));
            Ok(())
        },
        |_| async {},
    )
    .await
    .expect("fresh-db lazy DI graph must build");
    engine
}

/// The projected row for `provider`, as `(origin, hosts)`.
async fn projected_disclosure(db: &holon::storage::DbHandle, provider: &str) -> (String, String) {
    let rows = db
        .query(
            "SELECT provider_name, origin, hosts FROM integration_state",
            std::collections::HashMap::new(),
        )
        .await
        .expect(
            "`integration_state` must carry the disclosure the Settings row is supposed to show — \
             without these columns the `live_query` behind that surface has nothing to paint",
        );
    let row = rows
        .iter()
        .find(|r| r.get("provider_name").and_then(|v| v.as_string()) == Some(provider))
        .unwrap_or_else(|| panic!("the projector must write a row for '{provider}'"));
    let text = |c: &str| {
        row.get(c)
            .and_then(|v| v.as_string())
            .unwrap_or_default()
            .to_string()
    };
    (text("origin"), text("hosts"))
}

#[test]
fn an_introduced_connections_origin_and_hosts_reach_integration_state() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("calendar.yaml"),
            sidecar_calling(&["api.evil.example", "cdn.evil.example"]),
        )
        .expect("write the connection file");

        let engine = fresh_engine(dir.path().join("mirror.db")).await;
        let db = engine.db_handle().clone();
        let store = Arc::new(IntegrationConfigStore::load(dir.path()).expect("store loads"));
        IntegrationStateProjector::new(
            db.clone(),
            Arc::new(IntegrationsSettingsVm::new(
                store,
                holon_mcp_client::CredentialRoot::new(dir.path()),
            )),
        )
        .project()
        .await
        .expect("the projector must run");

        let (origin, hosts) = projected_disclosure(&db, "calendar").await;
        assert!(
            origin.contains("calendar.yaml"),
            "the mirror must carry the FILE the connection came from — it is the one thing the \
             file's author cannot rename away from the user's own directory; got {origin:?}"
        );
        assert_eq!(
            hosts, "api.evil.example, cdn.evil.example",
            "the mirror must carry every host the manual calls, in the row's own order, so the \
             surface renders the list without re-deriving it"
        );
    });
}

/// The other half of the same disclosure: a bundled connection must project
/// both as empty, so "this came from a file" stays a readable signal instead of
/// a line every row carries.
#[test]
fn a_bundled_connection_projects_no_origin_and_no_hosts() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let engine = fresh_engine(dir.path().join("mirror.db")).await;
        let db = engine.db_handle().clone();
        let store = Arc::new(IntegrationConfigStore::load(dir.path()).expect("store loads"));
        IntegrationStateProjector::new(
            db.clone(),
            Arc::new(IntegrationsSettingsVm::new(
                store,
                holon_mcp_client::CredentialRoot::new(dir.path()),
            )),
        )
        .project()
        .await
        .expect("the projector must run");

        let (origin, hosts) = projected_disclosure(&db, "todoist").await;
        assert_eq!(
            (origin.as_str(), hosts.as_str()),
            ("", ""),
            "a connection this build ships has no user file to name and no disclosure to make"
        );
    });
}

/// The column set is pinned so a credential field cannot arrive in this
/// user-queryable table unnoticed (§8 R1). Adding two columns is a deliberate
/// act, and this states which two.
#[test]
fn the_pinned_column_set_names_the_two_disclosure_columns() {
    assert!(
        TABLE_COLUMNS.contains(&"origin") && TABLE_COLUMNS.contains(&"hosts"),
        "the disclosure columns must be declared in the pinned set, not added behind it; got \
         {TABLE_COLUMNS:?}"
    );
}
