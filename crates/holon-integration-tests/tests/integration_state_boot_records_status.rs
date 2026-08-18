//! Contract: a boot with integrations ENABLED by real state files leaves the
//! mirror agreeing with the enablement store, and records whatever boot status
//! the integration registry reached.
//!
//! Dogfood escape `2026-08-18-integrations-section-shows-one-stale-row`. The
//! live boot log (23:42:35-37) shows the registry computing a status for all
//! four enabled providers and EVERY write being refused:
//!
//! > integration 'gcal' has no enabled row in the integration_state mirror, so
//! > its boot status (Connected) cannot be recorded — the connect registry and
//! > the enablement store have diverged
//!
//! The refusal is correct in isolation — a status for a provider the store
//! never enabled IS a wiring bug — but the rows genuinely did not exist yet:
//! the registry connect loop runs inside `resolve_engine`, while the projector
//! that creates the rows ran later in the same factory (`wiring.rs`, after
//! `seed_default_layout`). Nothing retries, so every status was lost and the
//! section read `Pending` forever.
//!
//! THE ENVIRONMENT GAP this closes: no test booted the PRODUCTION wiring with
//! real `.state.toml` files present. The projection tests construct
//! `IntegrationStateProjector` directly and call `project()` themselves, so
//! they can never observe boot ORDER; and with no provider enabled, the
//! registry never connects and never records a status, leaving the whole
//! interaction unexercised.
//!
//! @pbt kind harness
//! @pbt covers integration-state-boot-status — a boot with enabled state files
//! leaves every enabled provider present in the mirror with a recorded status

use std::collections::HashMap;
use std::sync::Arc;

use holon::di::DbHandleProvider;
use holon_integration_tests::TestEnvironment;
use holon_mcp_client::IntegrationConfigStore;
use holon_mcp_client::integration_state::Configuration;
use holon_mcp_client::integration_state::IntegrationState;

const ENABLED: &[&str] = &["gcal", "todoist"];

#[test]
fn a_boot_with_enabled_state_files_records_every_status() {
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

    // Enable providers the way the user does — state files in the config dir's
    // `integrations/`, written through the production store, BEFORE boot.
    let integrations_dir = env.temp_dir.path().join("integrations");
    std::fs::create_dir_all(&integrations_dir).expect("create integrations dir");
    {
        let store = IntegrationConfigStore::load(&integrations_dir).expect("load store");
        for provider in ENABLED {
            store
                .set(
                    provider,
                    IntegrationState {
                        enabled: true,
                        configuration: Configuration::Unconfigured,
                    },
                )
                .expect("enable provider");
        }
    }

    env.start_app(false).await.expect("start_app");

    let db = env
        .injector()
        .expect("start_app must capture the injector")
        .resolve::<dyn DbHandleProvider>()
        .handle();

    let rows = db
        .query(
            "SELECT provider_name, enabled, status FROM integration_state \
             WHERE enabled = 1 ORDER BY provider_name ASC",
            HashMap::new(),
        )
        .await
        .expect("query the mirror after boot");

    let seen: Vec<(String, String)> = rows
        .iter()
        .map(|r| {
            (
                r.get("provider_name")
                    .and_then(|v| v.as_string())
                    .expect("provider_name")
                    .to_string(),
                r.get("status")
                    .and_then(|v| v.as_string())
                    .expect("status")
                    .to_string(),
            )
        })
        .collect();

    // 1. The mirror agrees with the store: every enabled provider is present.
    let providers: Vec<&str> = seen.iter().map(|(p, _)| p.as_str()).collect();
    assert_eq!(
        providers, ENABLED,
        "a boot whose state files enable {ENABLED:?} must leave exactly those rows enabled in the \
         mirror — got {seen:?}"
    );

    // 2. THE RED. Whatever the registry decided must have LANDED. `Pending` means
    //    the row did not exist when the registry tried to write, the refusal threw
    //    the outcome away, and nothing ever retried — the live bug. Any other value
    //    (including a connect failure) is a recorded outcome and therefore correct
    //    here: this rung is about the status ARRIVING, not about which one it is.
    let stuck: Vec<&(String, String)> = seen.iter().filter(|(_, s)| s == "Pending").collect();
    assert!(
        stuck.is_empty(),
        "every enabled integration must carry the boot status the registry computed, but {stuck:?} \
         are still Pending — the registry ran before the projector created their rows, its status \
         writes were refused, and nothing retried. Full mirror: {seen:?}"
    );
}
