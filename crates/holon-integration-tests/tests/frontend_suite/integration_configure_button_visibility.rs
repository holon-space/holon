//! The `Configure…` button appears on the rows that can actually run a consent
//! flow, and nowhere else.
//!
//! `begin_oauth` carries a declared relation guard over `config_status`,
//! `configurable` and `configure_progress`; `ops_of` evaluates it against the
//! rendered row, so what the Settings list offers is decided by the same
//! predicate the dispatcher enforces. This rung drives the real settings query
//! and the real entity profile, so the guard, the projection and the layout all
//! have to agree for it to pass.
//!
//! The separating property is the PROVIDER'S OWN CAPABILITY: `gcal` declares an
//! OAuth2 arm with an authorization endpoint, `todoist` does not. A rung that
//! only distinguished a registered provider from an unregistered one would say
//! nothing about the button a user actually sees.
//!
//! @pbt kind harness
//! @pbt covers integration-configure-button-visibility — an integration row
//! offers `begin_oauth` iff its consent flow could run
//! @pbt slips-if-removed a dead-end `Configure…` appears on providers that
//! authenticate with a static token, and clicking it fails for reasons the user
//! cannot act on

use std::collections::BTreeMap;
use std::sync::Arc;

use holon::di::DbHandleProvider;
use holon_api::widget_spec::DataRow;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive::ReactiveEngine;
use holon_frontend::value_fns::ops_of::ops_rows_for_uri;
use holon_integration_tests::TestEnvironment;

/// The op names `ops_of` offers for `row`, through the REAL profile resolver.
fn offered_ops(services: &dyn BuilderServices, row: &DataRow) -> Vec<String> {
    let uri = row
        .get("id")
        .and_then(|v| v.as_string())
        .expect("a mirror row carries its id")
        .to_string();
    ops_rows_for_uri(&uri, services, row)
        .iter()
        .map(|r| {
            r.get("name")
                .and_then(|v| v.as_string())
                .expect("an op row carries its name")
                .to_string()
        })
        .collect()
}

#[test]
fn only_providers_with_a_consent_flow_offer_the_configure_button() {
    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .unwrap(),
    );
    runtime.clone().block_on(run(runtime.clone()));
}

async fn run(runtime: Arc<tokio::runtime::Runtime>) {
    let env = TestEnvironment::new(runtime).expect("new TestEnvironment");
    env.start_app(false).await.expect("start_app");

    let db = env
        .injector()
        .expect("start_app must capture the injector")
        .resolve::<dyn DbHandleProvider>()
        .handle();
    db.transition_to_ready()
        .await
        .expect("transition the actor to Ready");

    // The rows the Settings surface renders, straight from its own query — so
    // the guard is evaluated against exactly the columns a user's row carries.
    let rows: BTreeMap<String, DataRow> = db
        .query(
            holon_app::integrations_section::SETTINGS_SQL,
            Default::default(),
        )
        .await
        .expect("the Settings query must run")
        .iter()
        .map(|r| {
            let provider = r
                .get("provider_name")
                .and_then(|v| v.as_string())
                .expect("provider_name")
                .to_string();
            let row: DataRow = r.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
            (provider, row)
        })
        .collect();

    let gcal = rows.get("gcal").expect("gcal is bundled");
    let todoist = rows.get("todoist").expect("todoist is bundled");
    assert_eq!(
        gcal.get("config_status").and_then(|v| v.as_string()),
        Some("unconfigured"),
        "precondition: gcal has not run its consent flow in this rig"
    );
    assert_eq!(
        gcal.get("configurable").and_then(|v| v.as_i64()),
        Some(1),
        "precondition: gcal declares an OAuth2 consent flow"
    );
    assert_eq!(
        todoist.get("configurable").and_then(|v| v.as_i64()),
        Some(0),
        "precondition: todoist authenticates without a consent flow"
    );

    let reactive: Arc<ReactiveEngine> = env
        .reactive_engine
        .get()
        .expect("start_app must resolve a ReactiveEngine")
        .clone();
    let services: &dyn BuilderServices = reactive.as_ref();

    assert!(
        offered_ops(services, gcal).contains(&"begin_oauth".to_string()),
        "gcal declares a consent flow and has not run it, so its row must offer \
         it.\n  offered: {:?}",
        offered_ops(services, gcal)
    );
    assert!(
        !offered_ops(services, todoist).contains(&"begin_oauth".to_string()),
        "todoist has no consent flow to run, so its row must offer no \
         `Configure…`.\n  offered: {:?}",
        offered_ops(services, todoist)
    );

    // The one-time half: a configured provider withdraws it.
    let mut configured = gcal.clone();
    configured.insert(
        "config_status".to_string(),
        holon_api::Value::String("configured".to_string()),
    );
    assert!(
        !offered_ops(services, &configured).contains(&"begin_oauth".to_string()),
        "the consent flow is one-time; a configured provider must not offer it again"
    );

    // The in-flight half: a running flow withdraws it too.
    let mut in_flight = gcal.clone();
    in_flight.insert(
        "configure_progress".to_string(),
        holon_api::Value::String("Waiting for you to finish in the browser.".to_string()),
    );
    assert!(
        !offered_ops(services, &in_flight).contains(&"begin_oauth".to_string()),
        "a second click would start a second browser hop and a second loopback listener"
    );
}

/// The button must reach the RENDERED TREE, not just `ops_of`'s row set.
///
/// The row's ops cluster is a collection nested inside the Settings
/// `live_query`'s item_template. `ops_of` returning the operation and the row
/// PAINTING it are two different facts, and only this one is what a user sees.
#[test]
fn the_offered_operation_reaches_the_rendered_row() {
    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .unwrap(),
    );
    runtime.clone().block_on(rendered(runtime.clone()));
}

async fn rendered(runtime: Arc<tokio::runtime::Runtime>) {
    use holon_frontend::view_model::ViewKind;
    use holon_frontend::view_model::ViewModel;

    fn op_targets(vm: &ViewModel, out: &mut Vec<String>) {
        if let ViewKind::OpButton {
            op_name, target_id, ..
        } = &vm.kind
        {
            out.push(format!("{op_name}@{target_id}"));
        }
        for c in vm.children() {
            op_targets(c, out);
        }
    }

    let harness = TestEnvironment::new(runtime).expect("new TestEnvironment");
    harness.start_app(false).await.expect("start_app");
    let db = harness
        .injector()
        .expect("start_app must capture the injector")
        .resolve::<dyn DbHandleProvider>()
        .handle();
    db.transition_to_ready().await.expect("ready");

    let reactive: Arc<ReactiveEngine> = harness
        .reactive_engine
        .get()
        .expect("start_app must resolve a ReactiveEngine")
        .clone();
    let services: Arc<dyn BuilderServices> = reactive.clone();
    let item_template = holon_api::render_dsl::parse_render_dsl(
        holon_app::integrations_section::SETTINGS_ITEM_TEMPLATE,
    )
    .expect("the Settings item_template must parse");
    let (key, live) = reactive.watch_query_live(
        holon_app::integrations_section::SETTINGS_SQL.to_string(),
        holon_api::QueryLanguage::HolonSql,
        item_template,
        None,
        services,
    );

    let rows = reactive.ensure_watching(&key);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while std::time::Instant::now() < deadline && rows.snapshot().1.len() < 5 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let mut found = Vec::new();
    while std::time::Instant::now() < deadline {
        found.clear();
        op_targets(&live.tree.snapshot(), &mut found);
        if found.iter().any(|f| f.ends_with("@integration:gcal")) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    assert!(
        found.contains(&"begin_oauth@integration:gcal".to_string()),
        "gcal's row must RENDER the operation `ops_of` offers it; the rendered tree carried: \
         {found:?}"
    );
    assert!(
        !found.contains(&"begin_oauth@integration:todoist".to_string()),
        "todoist has no consent flow, so its row must render none: {found:?}"
    );
}
