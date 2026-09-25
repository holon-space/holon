//! An org-embedded entity profile refused at load (ruling D171.a) is not
//! applied and is disclosed on the condition bus, and the app keeps running —
//! whether the profile block is in the vault at boot or arrives after it.
//!
//! @pbt kind harness
//! @pbt covers refused-profile-boots-degraded — a profile refused at load is
//! disclosed as a condition and never applied, at boot and after boot
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone authors no
//! refused profile

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use holon_api::Condition;
use holon_api::ConditionBus;
use holon_api::ConditionKind;
use holon_api::Value;
use holon_frontend::config::HolonConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::config::VaultConfig;

const PROFILE_ORG: &str = "\
* Asked profile
:PROPERTIES:
:ID: asked-profile
:END:
#+begin_src holon_entity_profile_yaml
entity_name: block
variants:
  - name: asked
    priority: 5
    condition: 'asker == \"martin\"'
    render: 'text(col(\"content\"))'
#+end_src
";

const DEADLINE: Duration = Duration::from_secs(30);

struct Booted {
    engine: Arc<holon::api::BackendEngine>,
    bus: Arc<ConditionBus>,
    _session: Arc<holon_frontend::FrontendSession>,
}

async fn boot(dir: &Path) -> Booted {
    let holon_config = HolonConfig {
        db_path: Some(dir.join("refused-profile.db")),
        vault: VaultConfig {
            root: Some(dir.to_path_buf()),
        },
        ..Default::default()
    };
    let (session, engine, bus) = holon_app::new_from_config_with_di(
        holon_config,
        SessionConfig::new(holon_api::UiInfo::permissive()),
        dir.to_path_buf(),
        HashSet::new(),
        |_| Ok(()),
        |injector| (*injector.resolve::<Arc<ConditionBus>>()).clone(),
    )
    .await
    .expect("a refused profile is the user's to fix, so the session must boot");
    Booted {
        engine,
        bus,
        _session: session,
    }
}

async fn wait_for_profile_row(engine: &holon::api::BackendEngine) {
    let start = Instant::now();
    loop {
        let rows = engine
            .db_handle()
            .query(
                "SELECT id FROM block WHERE source_language = 'holon_entity_profile_yaml'",
                HashMap::new(),
            )
            .await
            .expect("query profile blocks");
        if !rows.is_empty() {
            return;
        }
        assert!(
            start.elapsed() < DEADLINE,
            "the vault's profile block never reached the database"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn current_refusal(bus: &ConditionBus) -> Option<Condition> {
    bus.current()
        .into_iter()
        .find(|c| matches!(c.reason, ConditionKind::ProfileRefused { .. }))
}

async fn wait_for_refusal(bus: &ConditionBus) -> Option<Condition> {
    let start = Instant::now();
    while start.elapsed() < DEADLINE {
        if let Some(c) = current_refusal(bus) {
            return Some(c);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    None
}

fn assert_disclosed_and_not_applied(when: &str, booted: &Booted, refusal: Option<Condition>) {
    let refusal = refusal.unwrap_or_else(|| {
        panic!(
            "{when}: the refused profile must be disclosed on the condition bus; it carries {:?}",
            booted
                .bus
                .current()
                .iter()
                .map(|c| (c.subject.clone(), c.reason.condition_kind()))
                .collect::<Vec<_>>()
        )
    });
    let ConditionKind::ProfileRefused { error } = &refusal.reason else {
        unreachable!()
    };
    for needle in [
        "profile 'block'",
        "variant 'asked'",
        "asker == \"martin\"",
        "`asker`",
    ] {
        assert!(
            error.contains(needle),
            "{when}: the disclosure must name {needle:?}: {error}"
        );
    }

    let row: HashMap<String, Value> = HashMap::from([
        ("id".to_string(), Value::String("block:probe".into())),
        ("content".to_string(), Value::String("q".into())),
        (
            "properties".to_string(),
            Value::Object(HashMap::from([(
                "asker".to_string(),
                Value::String("martin".into()),
            )])),
        ),
    ]);
    let resolved = booted.engine.profile_resolver().resolve(&row);
    assert_ne!(
        resolved.name, "asked",
        "{when}: a refused profile must not be applied"
    );
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build test runtime")
}

#[test]
fn a_refused_profile_in_the_vault_at_boot_is_disclosed() {
    runtime().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("profiles.org"), PROFILE_ORG).expect("write the vault");

        let booted = boot(dir.path()).await;
        let refusal = wait_for_refusal(&booted.bus).await;
        assert_disclosed_and_not_applied("at boot", &booted, refusal);
    });
}

#[test]
fn a_refused_profile_written_after_boot_is_disclosed_until_fixed() {
    runtime().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");

        let booted = boot(dir.path()).await;
        std::fs::write(dir.path().join("profiles.org"), PROFILE_ORG).expect("write the vault");
        wait_for_profile_row(&booted.engine).await;
        let refusal = wait_for_refusal(&booted.bus).await;
        assert_disclosed_and_not_applied("after boot", &booted, refusal);

        let fixed = PROFILE_ORG.replace(
            "'asker == \"martin\"'",
            "'is_def_var(\"asker\") && asker == \"martin\"'",
        );
        assert_ne!(fixed, PROFILE_ORG);
        std::fs::write(dir.path().join("profiles.org"), fixed).expect("fix the profile");
        let start = Instant::now();
        while current_refusal(&booted.bus).is_some() {
            assert!(
                start.elapsed() < DEADLINE,
                "loading the fixed profile must clear its refusal"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    });
}
