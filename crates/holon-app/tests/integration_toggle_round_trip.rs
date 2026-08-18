//! The whole click round-trip, through the REAL parts (design §4.4).
//!
//! ```text
//! integration.set_field  →  IntegrationsOperationProvider
//!                        →  the .state.toml file            [AUTHORITY]
//!                        →  the store's Mutable fires
//!                        →  IntegrationStateProjector       [PROJECTION]
//!                        →  integration_state
//!                        →  the Settings list's own query
//! ```
//!
//! Every neighbouring rung covers one arrow: `integration_set_field_op` the
//! operation, `integration_state_projection` the mirror,
//! `state_toggle_switch_windowed` the paint. What none of them can see is the
//! JOIN — a chain in which every link works and the whole does not is exactly
//! the shape a per-link suite passes. So this file uses one container: the
//! module's own store, the module's own provider, and the projector's own
//! signal watchers, with nothing re-projected by hand.
//!
//! The query is the Settings modal's own `SETTINGS_SQL` rather than a restated
//! copy — the switch lives on that surface, so that is where the round trip
//! has to land.
//!
//! @pbt kind harness
//! @pbt covers integration-toggle-round-trip — dispatching
//! `integration.set_field` reaches the Settings list's rows without any
//! manual re-projection
//! @pbt slips-if-removed the file and the table agree, the widget and the
//! operation agree, and the section still shows the previous switch state
//! because nothing joined the store's signal to the mirror

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use fluxdi::Module;
use holon_api::EntityName;
use holon_api::OpOrigin;
use holon_api::Value;
use holon_app::integration_projection::IntegrationStateProjector;
use holon_core::storage::types::StorageEntity;
use holon_mcp_client::IntegrationConfigStore;

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create runtime"),
    )
}

/// `provider`'s switch, read the way the bool-bound toggle reads it.
async fn section_switch(db: &holon::storage::DbHandle, sql: &str, provider: &str) -> bool {
    let row = db
        .query(sql, HashMap::new())
        .await
        .unwrap_or_else(|e| panic!("the seeded Integrations query must run: {sql}\n{e}"))
        .iter()
        .find(|r| r.get("provider_name").and_then(|v| v.as_string()) == Some(provider))
        .cloned()
        .unwrap_or_else(|| panic!("the section must carry a row for '{provider}'"));
    holon_frontend::view_model::bool_from_row_value("enabled", row.get("enabled"))
        .unwrap_or_else(|e| panic!("{e}"))
}

/// Wait for the section to show `want`, or fail naming what it showed instead.
///
/// The projector re-projects on a signal, so the mirror converges a moment
/// after the operation returns. Polling to a deadline is what makes this a test
/// of the JOIN rather than of a sleep long enough to hide a missing one.
async fn await_switch(db: &holon::storage::DbHandle, sql: &str, provider: &str, want: bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last = !want;
    while Instant::now() < deadline {
        last = section_switch(db, sql, provider).await;
        if last == want {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!(
        "the seeded Integrations section never showed '{provider}' as {want:?} — it is still \
         {last:?}. The operation wrote the state file, so the break is between the store's signal \
         and the mirror."
    );
}

#[test]
fn a_dispatched_switch_reaches_the_seeded_section_without_a_manual_reprojection() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let state_dir = dir.path().join("integrations");
        std::fs::create_dir_all(&state_dir).expect("state dir");

        // ONE container: the module registers the store, the settings list and
        // the operation provider; the projector below watches that SAME store.
        let state_dir_for_module = state_dir.clone();
        let (engine, ordering, store) = holon::di::create_backend_engine_with_extras(
            dir.path().join("fresh.db"),
            move |injector| {
                // The same block-CRUD + event wiring `integration_state_projection`
                // builds, so the seed has an ordering authority to write through.
                holon::sync::EventInfraModule
                    .configure(injector)
                    .map_err(|e| {
                        anyhow::anyhow!("configure EventInfraModule for the round-trip test: {e}")
                    })?;
                injector.provide_into_set::<dyn holon_core::OperationProvider>(
                    fluxdi::Provider::root(|resolver| {
                        let db = resolver
                            .resolve::<dyn holon::di::DbHandleProvider>()
                            .handle();
                        Arc::new(holon::core::SqlOperationProvider::new(
                            db,
                            holon::storage::BLOCK_WRITE_TABLE.to_string(),
                            "block".to_string(),
                            "block".to_string(),
                        )) as Arc<dyn holon_core::OperationProvider>
                    }),
                );
                holon_app::mcp_integrations::McpIntegrationsModule::from_dir(&state_dir_for_module)
                    .configure(injector)
                    .map_err(|e| anyhow::anyhow!("configure McpIntegrationsModule: {e}"))
            },
            |injector| async move {
                (
                    injector
                        .resolve_async::<dyn holon_core::block_ordering::BlockOrdering>()
                        .await,
                    injector.resolve_async::<IntegrationConfigStore>().await,
                )
            },
        )
        .await
        .map(|(e, (o, s))| (e, o, s))
        .expect("fresh-db lazy DI graph must build");

        holon_app::seed_default_layout(&engine, ordering, false, false)
            .await
            .expect("seed_default_layout must complete on a fresh file DB");
        let db = engine.db_handle();

        // The projector's own watchers — the leg that carries a store change
        // into the mirror with nobody asking it to.
        Arc::new(IntegrationStateProjector::new(db.clone(), store))
            .start()
            .await
            .expect("the projector must build the mirror and start watching");

        let sql = holon_app::integrations_section::SETTINGS_SQL;
        assert_eq!(
            section_switch(db, sql, "todoist").await,
            false,
            "precondition: a clean vault shows todoist switched off"
        );

        // The click, as `state_toggle` sends it: a typed decision at the row's
        // `integration:` id, through the ordinary operation door.
        let mut params: StorageEntity = HashMap::new();
        params.insert("id".into(), Value::String("integration:todoist".into()));
        params.insert("field".into(), Value::String("enabled".into()));
        params.insert("value".into(), Value::Boolean(true));
        engine
            .execute_operation(
                &EntityName::new("integration"),
                "set_field",
                params,
                OpOrigin::User,
            )
            .await
            .expect("dispatching the toggle's intent must succeed");

        await_switch(db, sql, "todoist", true).await;

        // The authority moved too — the mirror is not the only thing that
        // changed, which is what makes the decision survive a restart.
        assert!(
            IntegrationConfigStore::load(&state_dir)
                .expect("store reloads from disk")
                .get("todoist")
                .expect("todoist state")
                .enabled,
            "the state file is the authority and must carry the decision"
        );

        // And back off, so the round trip is proven in both directions rather
        // than only in the one a first-run fixture happens to take.
        let mut params: StorageEntity = HashMap::new();
        params.insert("id".into(), Value::String("integration:todoist".into()));
        params.insert("field".into(), Value::String("enabled".into()));
        params.insert("value".into(), Value::Boolean(false));
        engine
            .execute_operation(
                &EntityName::new("integration"),
                "set_field",
                params,
                OpOrigin::User,
            )
            .await
            .expect("dispatching the off direction must succeed");

        await_switch(db, sql, "todoist", false).await;
    });
}
