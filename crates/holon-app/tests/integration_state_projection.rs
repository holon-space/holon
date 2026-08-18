//! The left-sidebar **Integrations** discovery section lists the integrations
//! the user has switched ON, with the status each one reached at boot.
//!
//! Entry `2026-08-18-integrations-discovery-section-lists-only-orgmode`: the
//! section used to query `sync_states`, the sync-CURSOR table. Only a provider
//! that persists a cursor ever lands there, under a `provider.entity` token
//! key — so `claude-history` (resource-only), `gcal` and `gmail` (no `cursor:`
//! config) could never appear at all, and `todoist` would have appeared as two
//! entity rows. The one provider that wrote a bare provider name was `orgmode`,
//! which is exactly what the live app showed.
//!
//! The fix projects [`IntegrationConfigStore`] — the enablement authority —
//! into a queryable `integration_state` table, and points the section at that.
//! The projection is a MIRROR, never a second authority: it is rebuilt from the
//! store, so a torn or stale row is repaired by the next re-projection.
//!
//! These tests drive the section's OWN sql, extracted from the seeded render
//! block rather than restated here. A test carrying its own copy of the query
//! would keep passing after the seed drifted — which is the shape of the
//! original escape.
//!
//! @pbt kind harness
//! @pbt covers integrations-enablement-projection — the seeded Integrations
//! section lists exactly the enabled integrations, each with its boot status
//! @pbt overlaps integrations_section_seed — kept: that file pins the section's
//! PLACEMENT (below the hierarchy, behind a divider) and its deletion
//! stickiness; this one pins what the section RESOLVES TO

use std::collections::HashMap;
use std::sync::Arc;

use fluxdi::Module;
use fluxdi::Provider;
use holon::storage::BLOCK_READ_TABLE;
use holon::sync::EventInfraModule;
use holon_app::integration_projection::IntegrationStateProjector;
use holon_app::integration_projection::IntegrationStatus;
use holon_app::integration_projection::TABLE_COLUMNS;
use holon_app::integration_projection::integration_row_id;
use holon_app::integration_projection::set_integration_status;
use holon_mcp_client::IntegrationConfigStore;
use holon_mcp_client::integration_state::Configuration;
use holon_mcp_client::integration_state::CredentialRef;
use holon_mcp_client::integration_state::Credentials;
use holon_mcp_client::integration_state::IntegrationState;

const LEFT_SIDEBAR_ID: &str = "block:default-left-sidebar";

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create runtime"),
    )
}

/// Fresh file-backed SqlOnly engine + `BlockOrdering` through the lazy-DI entry
/// the gpui frontend uses — the same fixture as `integrations_section_seed.rs`.
/// `integration_state` is an eager schema root of the `BackendEngine` factory,
/// so the seeded section's `live_query` has a real table to read.
async fn fresh_engine(
    db_path: std::path::PathBuf,
) -> (
    Arc<holon::api::BackendEngine>,
    Arc<dyn holon_core::block_ordering::BlockOrdering>,
) {
    holon::di::create_backend_engine_with_extras(
        db_path,
        |injector| {
            EventInfraModule.configure(injector).map_err(|e| {
                anyhow::anyhow!("configure EventInfraModule for integrations-projection test: {e}")
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
        |injector| async move {
            injector
                .resolve_async::<dyn holon_core::block_ordering::BlockOrdering>()
                .await
        },
    )
    .await
    .expect("fresh-db lazy DI graph must build (BackendEngine + BlockOrdering)")
}

/// The left-sidebar render block's content — the sole `render`-language child
/// of `block:default-left-sidebar`.
async fn left_sidebar_render(db: &holon::storage::DbHandle) -> String {
    let rows = db
        .query(
            &format!(
                "SELECT content FROM {BLOCK_READ_TABLE} \
                 WHERE parent_id = '{LEFT_SIDEBAR_ID}' AND source_language = 'render'"
            ),
            HashMap::new(),
        )
        .await
        .expect("query left-sidebar render block");
    rows.first()
        .and_then(|r| r.get("content"))
        .and_then(|v| v.as_string())
        .expect("left sidebar must have a seeded render block")
        .to_string()
}

/// The sql the seeded Integrations section actually carries.
///
/// Extracted from the render rather than restated, so these tests exercise the
/// query the app runs. The section is the LAST `live_query` in the left-sidebar
/// render (the page-hierarchy `tree(...)` above it is not a `live_query`), and
/// its sql is a plain double-quoted literal.
fn integrations_section_sql(render: &str) -> String {
    let header = render
        .find("Integrations")
        .expect("render must contain the Integrations section header");
    let lq = render[header..]
        .find("live_query(#{sql: \"")
        .map(|i| header + i + "live_query(#{sql: \"".len())
        .expect("Integrations section must be a live_query");
    let end = render[lq..]
        .find('"')
        .map(|i| lq + i)
        .expect("live_query sql literal must be terminated");
    render[lq..end].to_string()
}

/// Run `sql` and return its `provider_name` column, in row order.
async fn providers_of(db: &holon::storage::DbHandle, sql: &str) -> Vec<String> {
    db.query(sql, HashMap::new())
        .await
        .unwrap_or_else(|e| panic!("the seeded Integrations query must run: {sql}\n{e}"))
        .iter()
        .map(|r| {
            r.get("provider_name")
                .and_then(|v| v.as_string())
                .expect("Integrations query must project provider_name")
                .to_string()
        })
        .collect()
}

/// Run `sql` and return its `(provider_name, status)` pairs, in row order.
async fn providers_and_statuses(db: &holon::storage::DbHandle, sql: &str) -> Vec<(String, String)> {
    db.query(sql, HashMap::new())
        .await
        .unwrap_or_else(|e| panic!("the seeded Integrations query must run: {sql}\n{e}"))
        .iter()
        .map(|r| {
            let provider = r
                .get("provider_name")
                .and_then(|v| v.as_string())
                .expect("Integrations query must project provider_name")
                .to_string();
            let status = r
                .get("status")
                .and_then(|v| v.as_string())
                .expect("the Integrations section must project a status column")
                .to_string();
            (provider, status)
        })
        .collect()
}

/// Run `sql` and return its `(provider_name, enabled)` pairs, in row order.
/// `enabled` is parsed through the widget's own strict read.
async fn providers_and_switches(db: &holon::storage::DbHandle, sql: &str) -> Vec<(String, bool)> {
    db.query(sql, HashMap::new())
        .await
        .unwrap_or_else(|e| panic!("the seeded Integrations query must run: {sql}\n{e}"))
        .iter()
        .map(|r| {
            let provider = r
                .get("provider_name")
                .and_then(|v| v.as_string())
                .expect("Integrations query must project provider_name")
                .to_string();
            let enabled =
                holon_frontend::view_model::bool_from_row_value("enabled", r.get("enabled"))
                    .unwrap_or_else(|e| panic!("{e}"));
            (provider, enabled)
        })
        .collect()
}

/// The providers this build bundles, alphabetically — the order the section
/// lists them in. Spelled out so the assertions state what a user must see.
const BUNDLED: &[&str] = &[
    "claude-history",
    "gcal",
    "gmail",
    "jsonplaceholder",
    "todoist",
];

/// Every bundled provider paired with its expected switch position.
fn all_switches(on: &[&str]) -> Vec<(String, bool)> {
    BUNDLED
        .iter()
        .map(|p| (p.to_string(), on.contains(p)))
        .collect()
}

/// A store over `dir` with exactly `enabled` switched on.
fn store_with(dir: &std::path::Path, enabled: &[&str]) -> Arc<IntegrationConfigStore> {
    let store = IntegrationConfigStore::load(dir).expect("store loads");
    for provider in enabled {
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
    Arc::new(IntegrationConfigStore::load(dir).expect("store reloads"))
}

/// The sidebar is the DISCOVERY surface: the integrations that are on.
#[test]
fn seeded_section_lists_exactly_the_enabled_integrations() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let (engine, ordering) = fresh_engine(dir.path().join("fresh.db")).await;
        holon_app::seed_default_layout(&engine, ordering, false, false)
            .await
            .expect("seed_default_layout must complete on a fresh file DB");
        let db = engine.db_handle();

        let state_dir = dir.path().join("integrations");
        std::fs::create_dir_all(&state_dir).expect("state dir");
        let store = store_with(&state_dir, &["todoist", "claude-history", "gcal", "gmail"]);

        IntegrationStateProjector::new(db.clone(), store)
            .project()
            .await
            .expect("projecting the enablement store must succeed");

        let sql = integrations_section_sql(&left_sidebar_render(db).await);
        assert_eq!(
            providers_of(db, &sql).await,
            vec![
                "claude-history".to_string(),
                "gcal".to_string(),
                "gmail".to_string(),
                "todoist".to_string(),
            ],
            "the seeded Integrations section must list exactly the enabled integrations, \
             alphabetically — query was: {sql}"
        );
    });
}

/// The other half: switching one OFF removes it from the sidebar. Re-projection
/// is a full mirror rebuild, so a provider that was listed and is no longer
/// enabled must not linger as a stale row.
#[test]
fn disabling_an_integration_removes_it_from_the_section() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let (engine, ordering) = fresh_engine(dir.path().join("fresh.db")).await;
        holon_app::seed_default_layout(&engine, ordering, false, false)
            .await
            .expect("seed");
        let db = engine.db_handle();

        let state_dir = dir.path().join("integrations");
        std::fs::create_dir_all(&state_dir).expect("state dir");
        let store = store_with(&state_dir, &["todoist", "gcal"]);
        let projector = IntegrationStateProjector::new(db.clone(), store.clone());
        projector.project().await.expect("first projection");

        let sql = integrations_section_sql(&left_sidebar_render(db).await);
        assert_eq!(
            providers_of(db, &sql).await,
            vec!["gcal".to_string(), "todoist".to_string()],
            "sanity: both enabled integrations listed"
        );

        store
            .set(
                "gcal",
                IntegrationState {
                    enabled: false,
                    configuration: Configuration::Unconfigured,
                },
            )
            .expect("disable gcal");
        projector.project().await.expect("re-projection");

        assert_eq!(
            providers_of(db, &sql).await,
            vec!["todoist".to_string()],
            "a disabled integration must leave the sidebar — re-projection rebuilds the mirror"
        );
    });
}

/// The Settings modal is the CONTROL surface: every bundled provider with its
/// switch word, enabled or not. Filtered to the enabled ones it would offer no
/// way to switch a first integration on — which is exactly the state a clean
/// vault is in.
#[test]
fn the_settings_query_lists_every_provider_with_its_switch_state() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let (engine, _ordering) = fresh_engine(dir.path().join("fresh.db")).await;
        let db = engine.db_handle();

        let state_dir = dir.path().join("integrations");
        std::fs::create_dir_all(&state_dir).expect("state dir");
        let store = store_with(&state_dir, &["todoist", "gcal"]);
        IntegrationStateProjector::new(db.clone(), store)
            .project()
            .await
            .expect("projection");

        assert_eq!(
            providers_and_switches(db, holon_app::integrations_section::SETTINGS_SQL).await,
            all_switches(&["todoist", "gcal"]),
            "the Settings list must carry every bundled provider, each with its own switch state"
        );
    });
}

/// Nothing enabled ⇒ an empty sidebar section. The list is never fabricated,
/// and a vault with every integration off renders no rows rather than all of
/// them. Switching a first one on is the Settings modal's job.
#[test]
fn nothing_enabled_lists_nothing() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let (engine, ordering) = fresh_engine(dir.path().join("fresh.db")).await;
        holon_app::seed_default_layout(&engine, ordering, false, false)
            .await
            .expect("seed");
        let db = engine.db_handle();

        let state_dir = dir.path().join("integrations");
        std::fs::create_dir_all(&state_dir).expect("state dir");
        let store = store_with(&state_dir, &[]);
        IntegrationStateProjector::new(db.clone(), store)
            .project()
            .await
            .expect("projection");

        let sql = integrations_section_sql(&left_sidebar_render(db).await);
        assert!(
            providers_of(db, &sql).await.is_empty(),
            "no integration enabled ⇒ the sidebar section lists nothing"
        );
    });
}

/// The recreate path an EXISTING vault needs, pinned so the instructions given
/// to a user cannot silently stop working.
///
/// Layout is seeded fresh-only, and `integrations_section_seed.rs` pins that
/// deleting the section itself STICKS across a reseed. So a vault seeded before
/// this change does not pick the new section up by deleting the section — the
/// re-seed condition is the ROOT LAYOUT being absent
/// (`seed.rs:50`: `fresh = !user_index_org_exists && root-layout not in the
/// db`), with the default layout re-seeding from the bundled asset as long as
/// the user has not materialized it (no `__default__.org`, no vault
/// `index.org`).
///
/// Removing `block:root-layout` is therefore the whole gesture: the next boot
/// re-seeds the layout from the CURRENT bundled `index.org`, new section
/// included.
#[test]
fn removing_the_root_layout_reseeds_the_current_section_form() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let (engine, ordering) = fresh_engine(dir.path().join("fresh.db")).await;
        holon_app::seed_default_layout(&engine, ordering.clone(), false, false)
            .await
            .expect("first seed");
        let db = engine.db_handle();
        assert!(
            left_sidebar_render(db).await.contains("integration_state"),
            "sanity: the first seed carries the current section form"
        );

        // A plain reseed changes nothing — the root layout is still present, so
        // the boot is not fresh. This is what makes the gesture necessary.
        holon_app::seed_default_layout(&engine, ordering.clone(), false, false)
            .await
            .expect("reseed with the root layout still present");

        // The gesture: drop the root layout.
        engine
            .execute_operation(
                &holon_api::EntityName::from("block"),
                "delete",
                {
                    let mut p = holon_api::StorageEntity::new();
                    p.insert(
                        "id".into(),
                        holon_api::Value::String(holon_api::ROOT_LAYOUT_BLOCK_ID.to_string()),
                    );
                    p
                },
                holon_api::OpOrigin::User,
            )
            .await
            .expect("delete the root layout block");

        // Next boot: the layout re-seeds from the bundled asset.
        holon_app::seed_default_layout(&engine, ordering, false, false)
            .await
            .expect("reseed after the root layout was removed");

        let render = left_sidebar_render(db).await;
        assert!(
            render.contains("integration_state"),
            "removing the root layout must re-seed the CURRENT section form; got: {render}"
        );
        assert!(
            !render.contains("sync_states"),
            "the re-seeded section must not carry the retired sync_states query; got: {render}"
        );
    });
}

/// Increment 2, the REGISTRY axis — the section's second column carries how far
/// each enabled integration's boot connect got.
///
/// Freshly projected it is `Pending` (enabled, not yet resolved) and the
/// registry's outcome replaces it. This is what keeps the section honest under
/// "fail loud, never fake": an integration that is switched on but could not
/// connect stays VISIBLE and is visibly broken, rather than vanishing from the
/// list or rendering identically to a healthy one.
#[test]
fn the_section_carries_each_integrations_boot_status() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let (engine, ordering) = fresh_engine(dir.path().join("fresh.db")).await;
        holon_app::seed_default_layout(&engine, ordering, false, false)
            .await
            .expect("seed");
        let db = engine.db_handle();

        let state_dir = dir.path().join("integrations");
        std::fs::create_dir_all(&state_dir).expect("state dir");
        let store = store_with(&state_dir, &["todoist", "gcal", "gmail"]);
        IntegrationStateProjector::new(db.clone(), store)
            .project()
            .await
            .expect("projection");

        let sql = integrations_section_sql(&left_sidebar_render(db).await);
        assert_eq!(
            providers_and_statuses(db, &sql).await,
            vec![
                ("gcal".to_string(), "Pending".to_string()),
                ("gmail".to_string(), "Pending".to_string()),
                ("todoist".to_string(), "Pending".to_string()),
            ],
            "a freshly projected, not-yet-connected integration reads as Pending"
        );

        // The three boot outcomes the registry distinguishes.
        set_integration_status(db, "todoist", IntegrationStatus::Connected)
            .await
            .expect("record connected");
        set_integration_status(db, "gcal", IntegrationStatus::NeedsAuth)
            .await
            .expect("record needs-auth");
        set_integration_status(db, "gmail", IntegrationStatus::Unavailable)
            .await
            .expect("record unavailable");

        assert_eq!(
            providers_and_statuses(db, &sql).await,
            vec![
                ("gcal".to_string(), "Needs auth".to_string()),
                ("gmail".to_string(), "Unavailable".to_string()),
                ("todoist".to_string(), "Connected".to_string()),
            ],
            "each enabled integration shows the status its boot connect reached"
        );
    });
}

/// A status write for a provider with no enabled row is a wiring bug, not a
/// row: only the enablement store decides who is in the mirror, so a status
/// arriving for anyone else means the registry and the store have diverged.
#[test]
fn a_status_for_a_disabled_integration_is_refused() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let (engine, ordering) = fresh_engine(dir.path().join("fresh.db")).await;
        holon_app::seed_default_layout(&engine, ordering, false, false)
            .await
            .expect("seed");
        let db = engine.db_handle();

        let state_dir = dir.path().join("integrations");
        std::fs::create_dir_all(&state_dir).expect("state dir");
        let store = store_with(&state_dir, &["todoist"]);
        IntegrationStateProjector::new(db.clone(), store)
            .project()
            .await
            .expect("projection");

        let err = set_integration_status(db, "gcal", IntegrationStatus::Connected)
            .await
            .expect_err("a status for a non-enabled provider must fail loud");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("gcal"),
            "the error must name the provider: {msg}"
        );
    });
}

/// The STORE axis — `config_status` tracks whether the one-time credential
/// setup has run, independently of both `enabled` and the registry's `status`.
///
/// The three axes are deliberately separate: switched on, credentials set up,
/// and actually connected are different questions, and collapsing any two of
/// them is what made the old section lie.
#[test]
fn config_status_tracks_the_stores_configuration_axis() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let (engine, _ordering) = fresh_engine(dir.path().join("fresh.db")).await;
        let db = engine.db_handle();

        let state_dir = dir.path().join("integrations");
        std::fs::create_dir_all(&state_dir).expect("state dir");
        let store = store_with(&state_dir, &["todoist", "gcal"]);
        let projector = IntegrationStateProjector::new(db.clone(), store.clone());
        projector.project().await.expect("projection");

        let read = |db: &holon::storage::DbHandle| {
            let db = db.clone();
            async move {
                db.query(
                    "SELECT provider_name, config_status FROM integration_state \
                     WHERE enabled = 1 ORDER BY provider_name ASC",
                    HashMap::new(),
                )
                .await
                .expect("read config_status")
                .iter()
                .map(|r| {
                    (
                        r.get("provider_name")
                            .and_then(|v| v.as_string())
                            .expect("provider_name")
                            .to_string(),
                        r.get("config_status")
                            .and_then(|v| v.as_string())
                            .expect("config_status")
                            .to_string(),
                    )
                })
                .collect::<Vec<_>>()
            }
        };

        assert_eq!(
            read(db).await,
            vec![
                ("gcal".to_string(), "unconfigured".to_string()),
                ("todoist".to_string(), "unconfigured".to_string()),
            ],
            "an enabled integration whose credential setup has not run reads as unconfigured"
        );

        // gcal completes its one-time OAuth bootstrap.
        store
            .set(
                "gcal",
                IntegrationState {
                    enabled: true,
                    configuration: Configuration::Configured(Credentials {
                        client_id: CredentialRef::Env {
                            var: "GCAL_CLIENT_ID".to_string(),
                        },
                        client_secret: CredentialRef::Env {
                            var: "GCAL_CLIENT_SECRET".to_string(),
                        },
                        refresh_token_file: std::path::PathBuf::from("gcal-refresh-token"),
                    }),
                },
            )
            .expect("configure gcal");
        projector.project().await.expect("re-projection");

        assert_eq!(
            read(db).await,
            vec![
                ("gcal".to_string(), "configured".to_string()),
                ("todoist".to_string(), "unconfigured".to_string()),
            ],
            "a completed credential setup must reach the mirror"
        );
    });
}

/// §8 R1 — the column set is pinned, so a later field addition is a FAILING
/// TEST rather than a silent credential leak into a table any user query and
/// the `holon` MCP can read.
#[test]
fn the_mirror_exposes_exactly_the_designed_columns() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let (engine, _ordering) = fresh_engine(dir.path().join("fresh.db")).await;
        let db = engine.db_handle();

        let columns: Vec<String> = db
            .query(
                "SELECT name FROM pragma_table_info('integration_state')",
                HashMap::new(),
            )
            .await
            .expect("read integration_state column set")
            .iter()
            .map(|r| {
                r.get("name")
                    .and_then(|v| v.as_string())
                    .expect("pragma_table_info projects name")
                    .to_string()
            })
            .collect();

        assert_eq!(
            columns, TABLE_COLUMNS,
            "integration_state must expose exactly the designed columns. A new column here is a \
             deliberate act: `Configuration` carries credential LOCATIONS and must never reach \
             this table (design §8 R1)."
        );
    });
}

/// §8 R4 — the convergence contract. A state file edited outside the app (or a
/// mirror that drifted for any other reason) must re-converge on the next
/// projection, because the projector re-derives from the store rather than
/// accumulating deltas. This is the case a naive delta-applying projector
/// fails.
#[test]
fn a_drifted_mirror_reconverges_on_the_next_projection() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let (engine, ordering) = fresh_engine(dir.path().join("fresh.db")).await;
        holon_app::seed_default_layout(&engine, ordering, false, false)
            .await
            .expect("seed");
        let db = engine.db_handle();

        let state_dir = dir.path().join("integrations");
        std::fs::create_dir_all(&state_dir).expect("state dir");
        let store = store_with(&state_dir, &["todoist"]);
        IntegrationStateProjector::new(db.clone(), store)
            .project()
            .await
            .expect("first projection");

        // Corrupt the mirror behind the projector's back: flip todoist off and
        // invent a row for a provider nobody enabled.
        db.execute_values(
            "UPDATE integration_state SET enabled = 0 WHERE id = ?",
            vec![holon_api::Value::String(integration_row_id("todoist"))],
        )
        .await
        .expect("corrupt the enabled bit");
        db.execute_values(
            "INSERT INTO integration_state \
             (id, provider_name, enabled, status, config_status, updated_at) \
             VALUES ('integration:ghost', 'ghost', 1, 'Connected', 'configured', \
             '2026-01-01 00:00:00')",
            vec![],
        )
        .await
        .expect("insert a ghost row");

        let sql = integrations_section_sql(&left_sidebar_render(db).await);
        assert_eq!(
            providers_of(db, &sql).await,
            vec!["ghost".to_string()],
            "sanity: the mirror really is wrong before the repair"
        );

        // A fresh store over the same files — the every-boot path — re-derives.
        let reloaded = store_with(&state_dir, &[]);
        IntegrationStateProjector::new(db.clone(), reloaded)
            .project()
            .await
            .expect("re-projection repairs the mirror");

        assert_eq!(
            providers_of(db, &sql).await,
            vec!["todoist".to_string()],
            "re-projection must restore the store's truth and drop the unbundled ghost row"
        );
    });
}
