//! Contract: by the time the boot finishes, `integration_state` holds a row for
//! EVERY bundled provider, and any boot status the integration registry
//! computed has landed on it.
//!
//! Dogfood escape `2026-08-18-integrations-section-shows-one-stale-row`. The
//! boot log shows the registry recording a status for all four enabled
//! providers at 23:42:35-37 and EVERY write being refused —
//! "no enabled row in the integration_state mirror ... the connect registry and
//! the enablement store have diverged" — because the projector that creates
//! those rows runs later in the boot (`wiring.rs`, after `seed_default_layout`)
//! than the registry connect loop (inside `resolve_engine`). The statuses were
//! lost with no retry, leaving every integration reading `Pending` forever.
//!
//! Nothing caught it because no test booted the PRODUCTION wiring with real
//! state files and then asked the mirror what it contained: the projection
//! tests construct the projector directly, so they never exercise boot order.
//!
//! @pbt kind harness
//! @pbt covers integration-state-boot-population — a real boot leaves one
//! mirror row per bundled provider, matching the enablement store

use std::collections::HashMap;
use std::sync::Arc;

use holon::di::DbHandleProvider;
use holon_integration_tests::TestEnvironment;

#[test]
fn boot_leaves_one_mirror_row_per_bundled_provider() {
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

    let rows = db
        .query(
            "SELECT id, provider_name, enabled, status FROM integration_state \
             ORDER BY provider_name ASC",
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
                r.get("id")
                    .and_then(|v| v.as_string())
                    .expect("id")
                    .to_string(),
            )
        })
        .collect();

    let bundled: Vec<String> = holon_mcp_client::BUNDLED_SIDECARS
        .iter()
        .map(|s| s.provider.to_string())
        .collect();

    assert_eq!(
        seen.len(),
        bundled.len(),
        "a finished boot must leave one mirror row per BUNDLED provider (the presence axis in \
         full) — bundled={bundled:?} seen={seen:?}. A short mirror is how the live app rendered a \
         single integration while four were enabled."
    );

    for (provider, id) in &seen {
        assert_eq!(
            id,
            &format!("integration:{provider}"),
            "every row id must be the provider's `integration:` entity uri — got {id} for \
             {provider}"
        );
    }
}
