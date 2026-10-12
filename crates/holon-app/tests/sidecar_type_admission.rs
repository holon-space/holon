//! A sidecar whose entity names a home that cannot carry it refuses its whole
//! integration at load, before anything of it is built.
//!
//! The sidecar is user data, so the boot degrades and discloses (directive
//! boot-always); the integration is refused as ONE unit, because syncing the
//! entities that pass would be a silent subset. Admission sits at sidecar load,
//! ahead of `build_mcp_integration`, so a refused integration leaves no cache
//! table, no `integration_cache` row, no FDW table and no view behind.
//!
//! @pbt kind harness
//! @pbt covers sidecar-type-admission — an inadmissible sidecar entity refuses
//! its integration at load and derives nothing

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use holon_api::ConditionBus;
use holon_app::integration_projection::IntegrationStatus;
use holon_frontend::config::HolonConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::config::VaultConfig;

const PROVIDER: &str = "gadgets";
const ENTITY: &str = "gadget_items";
const FIELD: &str = "flag";

/// A REST connection with one schema'd entity whose `flag` field names a home
/// no capability profile defines. REST needs no peer to connect, so without
/// load-time admission the connect builds the entity's cache.
fn sidecar_with_unknown_home() -> String {
    sidecar_with_unknown_homes(&[(ENTITY, FIELD)])
}

/// Like [`sidecar_with_unknown_home`], one (entity, bad field) per pair.
fn sidecar_with_unknown_homes(entities: &[(&str, &str)]) -> String {
    let entities: String = entities
        .iter()
        .map(|(entity, field)| {
            format!(
                "  {entity}:\n    short_name: item\n    id_column: id\n    schema:\n      - {{ \
                 name: id, sql_type: TEXT, primary_key: true }}\n      - {{ name: {field}, \
                 sql_type: TEXT, home: no-such-home }}\n    sync:\n      list_tool: \
                 list-items\n      extract_path: items\n"
            )
        })
        .collect();
    format!(
        "schema_version: {}\ndisplay_name: \"Gadgets\"\nutcp:\n  utcp_version: \"1.1.3\"\n  \
         manual_version: \"1.0.0\"\n  tools:\n    - name: list-items\n      \
         tool_call_template:\n        call_template_type: http\n        http_method: GET\n        \
         url: https://gadgets.invalid/items\nholon:\n  tools:\n    list-items:\n      \
         result_key: items\nentities:\n{entities}tools: {{}}\n",
        holon_mcp_client::SIDECAR_SCHEMA_VERSION
    )
}

fn enable(dir: &Path, provider: &str) {
    std::fs::write(
        dir.join(format!("{provider}.state.toml")),
        "schema_version = 1\nenabled = true\n\n[configuration]\nstatus = \"unconfigured\"\n",
    )
    .expect("write the state file");
}

async fn status_of(db: &holon::storage::DbHandle, provider: &str) -> Option<String> {
    db.query(
        "SELECT provider_name, status FROM integration_state",
        HashMap::new(),
    )
    .await
    .expect("read integration_state")
    .iter()
    .find(|r| r.get("provider_name").and_then(|v| v.as_string()) == Some(provider))
    .and_then(|r| {
        r.get("status")
            .and_then(|v| v.as_string())
            .map(str::to_string)
    })
}

async fn sqlite_master_names(db: &holon::storage::DbHandle) -> Vec<String> {
    db.query("SELECT name FROM sqlite_master", HashMap::new())
        .await
        .expect("read sqlite_master")
        .iter()
        .filter_map(|r| {
            r.get("name")
                .and_then(|v| v.as_string())
                .map(str::to_string)
        })
        .collect()
}

#[test]
fn a_sidecar_entity_its_home_cannot_carry_refuses_the_integration_and_builds_nothing() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build test runtime");
    let dir = tempfile::tempdir().expect("tempdir");
    let integrations = dir.path().join("integrations");
    std::fs::create_dir_all(&integrations).expect("integrations dir");
    std::fs::write(
        integrations.join(format!("{PROVIDER}.yaml")),
        sidecar_with_unknown_home(),
    )
    .expect("write the connection file");
    enable(&integrations, PROVIDER);

    rt.block_on(async {
        let holon_config = HolonConfig {
            db_path: Some(dir.path().join("sidecar-admission.db")),
            vault: VaultConfig {
                root: Some(dir.path().to_path_buf()),
            },
            ..Default::default()
        };
        let (session, engine, bus) = holon_app::new_from_config_with_di(
            holon_config,
            SessionConfig::new(holon_api::UiInfo::permissive()).without_wait(),
            dir.path().to_path_buf(),
            HashSet::new(),
            Arc::new(ConditionBus::new()),
            |injector| {
                holon::testing::database_stuck_guard::report_database_stuck_in(injector);
                Ok(())
            },
            |injector| {
                injector
                    .try_resolve::<Arc<ConditionBus>>()
                    .map(|b| (*b).clone())
            },
        )
        .await
        .expect("a sidecar the app refuses must not stop the boot");
        let bus = bus.expect("the composition root registers a ConditionBus");
        let db = engine.db_handle().clone();

        let unavailable = IntegrationStatus::Unavailable.label();
        let mut status = None;
        for _ in 0..300 {
            status = status_of(&db, PROVIDER).await;
            if status.as_deref() == Some(unavailable) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(
            status.as_deref(),
            Some(unavailable),
            "the refused integration must read Unavailable, never Connecting or Connected"
        );

        let derived: Vec<String> = sqlite_master_names(&db)
            .await
            .into_iter()
            .filter(|name| name.contains(ENTITY))
            .collect();
        assert!(
            derived.is_empty(),
            "a refused integration derives nothing: no cache table, FDW table or view of \
             `{ENTITY}` may exist, found {derived:?}"
        );
        let cache_rows = db
            .query(
                &format!("SELECT table_name FROM integration_cache WHERE provider = '{PROVIDER}'"),
                HashMap::new(),
            )
            .await
            .expect("read integration_cache");
        assert!(
            cache_rows.is_empty(),
            "a refused integration registers no cache, found {cache_rows:?}"
        );

        let current = bus.subscribe().current;
        let refusal = current
            .iter()
            .find(|c| c.reason.condition_kind() == "integration-type-refused")
            .unwrap_or_else(|| {
                panic!(
                    "the refusal must be disclosed as `integration-type-refused`; the bus carries \
                     {:?}",
                    current
                        .iter()
                        .map(|c| (&c.subject, c.reason.condition_kind()))
                        .collect::<Vec<_>>()
                )
            });
        let detail = refusal.reason.detail(&refusal.subject);
        let text = format!("{} {}", detail.headline, detail.body.join(" "));
        for name in [PROVIDER, ENTITY, FIELD, "no-such-home"] {
            assert!(
                text.contains(name),
                "the disclosure must name the integration, the entity, the field and the home; \
                 `{name}` is missing from {text:?}"
            );
        }
        drop(session);
    });
}

/// Boots a vault holding one enabled sidecar per `(provider, entities)` and
/// returns the detail text of each `integration-type-refused` condition,
/// keyed by condition subject.
fn refusals_of(sidecars: &[(&str, &[(&str, &str)])]) -> Vec<(String, String)> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build test runtime");
    let dir = tempfile::tempdir().expect("tempdir");
    let integrations = dir.path().join("integrations");
    std::fs::create_dir_all(&integrations).expect("integrations dir");
    for (provider, entities) in sidecars {
        std::fs::write(
            integrations.join(format!("{provider}.yaml")),
            sidecar_with_unknown_homes(entities),
        )
        .expect("write the connection file");
        enable(&integrations, provider);
    }
    rt.block_on(async {
        let holon_config = HolonConfig {
            db_path: Some(dir.path().join("sidecar-admission.db")),
            vault: VaultConfig {
                root: Some(dir.path().to_path_buf()),
            },
            ..Default::default()
        };
        let (session, engine, bus) = holon_app::new_from_config_with_di(
            holon_config,
            SessionConfig::new(holon_api::UiInfo::permissive()).without_wait(),
            dir.path().to_path_buf(),
            HashSet::new(),
            Arc::new(ConditionBus::new()),
            |injector| {
                holon::testing::database_stuck_guard::report_database_stuck_in(injector);
                Ok(())
            },
            |injector| {
                injector
                    .try_resolve::<Arc<ConditionBus>>()
                    .map(|b| (*b).clone())
            },
        )
        .await
        .expect("a sidecar the app refuses must not stop the boot");
        let bus = bus.expect("the composition root registers a ConditionBus");
        let db = engine.db_handle().clone();
        let unavailable = IntegrationStatus::Unavailable.label();
        for (provider, _) in sidecars {
            let mut status = None;
            for _ in 0..300 {
                status = status_of(&db, provider).await;
                if status.as_deref() == Some(unavailable) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            assert_eq!(
                status.as_deref(),
                Some(unavailable),
                "{provider} must read Unavailable"
            );
        }
        let refusals = bus
            .subscribe()
            .current
            .iter()
            .filter(|c| c.reason.condition_kind() == "integration-type-refused")
            .map(|c| {
                let detail = c.reason.detail(&c.subject);
                (
                    c.subject.clone(),
                    format!("{} {}", detail.headline, detail.body.join(" ")),
                )
            })
            .collect();
        drop(session);
        refusals
    })
}

#[test]
fn two_refused_integrations_sharing_a_datatype_name_raise_one_condition_each() {
    let one: &[(&str, &str)] = &[("items", "flag")];
    let refusals = refusals_of(&[("gadgets", one), ("widgets", one)]);
    let mut subjects: Vec<&str> = refusals.iter().map(|(s, _)| s.as_str()).collect();
    subjects.sort();
    assert_eq!(
        subjects,
        ["gadgets", "widgets"],
        "one condition per refused integration, keyed by the integration; got {refusals:?}"
    );
}

#[test]
fn one_refused_integration_lists_every_refused_type_in_one_condition() {
    let refusals = refusals_of(&[("gadgets", &[("alpha", "f1"), ("beta", "f2")])]);
    assert_eq!(
        refusals.len(),
        1,
        "one condition for the integration; got {refusals:?}"
    );
    let (subject, text) = &refusals[0];
    assert_eq!(subject, "gadgets");
    for name in ["alpha", "f1", "beta", "f2"] {
        assert!(text.contains(name), "`{name}` is missing from {text:?}");
    }
}
