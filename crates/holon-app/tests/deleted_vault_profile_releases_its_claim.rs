//! A vault profile block that is deleted no longer blocks a type declaration
//! it would have overridden (ruling D66.a). The deletion travels the real
//! vault file -> database -> CDC leg.
//!
//! @pbt kind harness
//! @pbt covers deleted-vault-profile-releases-claim — a profile deleted from
//! the vault stops refusing later typed-computed-field registrations
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone declares no
//! types at runtime

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use holon_api::EntityName;
use holon_api::OpOrigin;
use holon_api::Value;
use holon_api::render_requirements::RenderRequirements;
use holon_conditions::ConditionBus;
use holon_conditions::ConditionKind;
use holon_frontend::config::HolonConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::config::VaultConfig;
use holon_profiles::TypeRegistry;
use holon_profiles::TypedComputedFieldOverride;

const PROFILE_ORG: &str = "\
* Late profile
:PROPERTIES:
:ID: late-profile
:END:
#+begin_src holon_entity_profile_yaml
entity_name: personlate
computed:
  display_name: '\"x\"'
#+end_src
";

const DEADLINE: Duration = Duration::from_secs(30);

fn late_person(registry: &TypeRegistry) -> holon_api::TypeDefinition {
    let mut late = registry.get("person").expect("person is bundled");
    late.name = "personlate".to_string();
    late
}

struct Booted {
    _dir: tempfile::TempDir,
    _session: Arc<holon_frontend::FrontendSession>,
    engine: Arc<holon::api::BackendEngine>,
    registry: Arc<TypeRegistry>,
    bus: Arc<ConditionBus>,
}

impl Booted {
    fn profile_file(&self) -> std::path::PathBuf {
        self._dir.path().join("profiles.org")
    }

    /// Whether the vault profile computes `display_name` for a `personlate`
    /// row.
    fn profile_in_effect(&self) -> bool {
        let row = HashMap::from([("id".to_string(), Value::String("personlate:row".into()))]);
        self.engine
            .profile_resolver()
            .resolve_computed_only(&row, &RenderRequirements::none())
            .contains_key("display_name")
    }
}

async fn boot() -> Booted {
    let dir = tempfile::tempdir().expect("tempdir");
    let holon_config = HolonConfig {
        db_path: Some(dir.path().join("late-profile.db")),
        vault: VaultConfig {
            root: Some(dir.path().to_path_buf()),
        },
        ..Default::default()
    };
    let (session, engine, (registry, bus)) = holon_app::new_from_config_with_di(
        holon_config,
        SessionConfig::new(holon_api::UiInfo::permissive()),
        dir.path().to_path_buf(),
        HashSet::new(),
        std::sync::Arc::new(holon_conditions::ConditionBus::new()),
        |_| Ok(()),
        |injector| {
            (
                injector.resolve::<TypeRegistry>(),
                (*injector.resolve::<Arc<ConditionBus>>()).clone(),
            )
        },
    )
    .await
    .expect("session boots");
    Booted {
        _dir: dir,
        _session: session,
        engine,
        registry,
        bus,
    }
}

async fn boot_with_late_profile() -> Booted {
    let booted = boot().await;
    std::fs::write(booted.profile_file(), PROFILE_ORG).expect("write the vault");
    let start = Instant::now();
    while !(booted.registry.has_vault_profile_for("personlate") && booted.profile_in_effect()) {
        assert!(start.elapsed() < DEADLINE, "the profile never loaded");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    booted
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build test runtime")
}

async fn profile_block(booted: &Booted) -> (String, String) {
    let rows = booted
        .engine
        .db_handle()
        .query(
            "SELECT id, content FROM block WHERE source_language = 'holon_entity_profile_yaml'",
            HashMap::new(),
        )
        .await
        .expect("query profile blocks");
    let [row] = rows.as_slice() else {
        panic!("expected exactly one profile block, got {rows:?}");
    };
    let text = |column: &str| {
        row.get(column)
            .and_then(|v| v.as_string())
            .unwrap_or_else(|| panic!("profile block has no {column}"))
            .to_string()
    };
    (text("id"), text("content"))
}

async fn user_op(booted: &Booted, op: &str, params: &[(&str, &str)]) {
    let params = params
        .iter()
        .map(|(k, v)| ((*k).into(), Value::String((*v).to_string())))
        .collect();
    booted
        .engine
        .execute_operation(&EntityName::new("block"), op, params, OpOrigin::User)
        .await
        .unwrap_or_else(|e| panic!("block.{op}: {e:#}"));
}

#[test]
fn a_deleted_vault_profile_stops_refusing_the_type_it_overrode() {
    runtime().block_on(async {
        let booted = Arc::new(boot_with_late_profile().await);
        let (id, _) = profile_block(&booted).await;

        let declarer = {
            let booted = Arc::clone(&booted);
            std::thread::spawn(move || {
                let start = Instant::now();
                loop {
                    match booted.registry.register(late_person(&booted.registry)) {
                        Ok(()) => return booted.profile_in_effect(),
                        Err(e) if e.downcast_ref::<TypedComputedFieldOverride>().is_some() => {
                            assert!(start.elapsed() < DEADLINE, "the type was never accepted");
                        }
                        Err(e) => panic!("register failed for another reason: {e:#}"),
                    }
                }
            })
        };
        user_op(&booted, "delete", &[("id", &id)]).await;

        let in_effect_when_accepted = tokio::task::spawn_blocking(move || declarer.join())
            .await
            .expect("join task")
            .expect("declarer thread");
        assert!(
            !in_effect_when_accepted,
            "the typed display_name was accepted while the deleted vault profile still computed it"
        );
    });
}

#[test]
fn a_refused_edit_releases_the_profiles_claim() {
    runtime().block_on(async {
        let booted = boot_with_late_profile().await;
        let (id, content) = profile_block(&booted).await;
        let refused_edit = content.replace(
            "display_name: '\"x\"'",
            "display_name: '\"x\"'\n  broken: 'no_such_column'",
        );
        assert_ne!(refused_edit, content);
        user_op(
            &booted,
            "set_field",
            &[("id", &id), ("field", "content"), ("value", &refused_edit)],
        )
        .await;
        let start = Instant::now();
        while !booted
            .bus
            .current()
            .iter()
            .any(|c| matches!(c.reason, ConditionKind::ProfileRefused { .. }))
        {
            assert!(start.elapsed() < DEADLINE, "the edit was never refused");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        let start = Instant::now();
        loop {
            match booted.registry.register(late_person(&booted.registry)) {
                Ok(()) => break,
                Err(e) if e.downcast_ref::<TypedComputedFieldOverride>().is_some() => {
                    assert!(
                        start.elapsed() < DEADLINE,
                        "the refused profile still claims the type: {e:#}"
                    );
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(e) => panic!("register failed for another reason: {e:#}"),
            }
        }
        assert!(
            !booted.profile_in_effect(),
            "the type was accepted while a version of the refused profile still computed it"
        );
    });
}

#[test]
fn deleting_a_refused_profile_block_clears_its_refusal() {
    runtime().block_on(async {
        let booted = boot_with_late_profile().await;
        let (id, content) = profile_block(&booted).await;
        let refused_edit = content.replace(
            "display_name: '\"x\"'",
            "display_name: '\"x\"'\n  broken: 'no_such_column'",
        );
        user_op(
            &booted,
            "set_field",
            &[("id", &id), ("field", "content"), ("value", &refused_edit)],
        )
        .await;
        let refusal_of_block = || {
            booted.bus.current().iter().any(|c| {
                c.subject == id && matches!(c.reason, ConditionKind::ProfileRefused { .. })
            })
        };
        let start = Instant::now();
        while !refusal_of_block() {
            assert!(start.elapsed() < DEADLINE, "the edit was never refused");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        user_op(&booted, "delete", &[("id", &id)]).await;
        let start = Instant::now();
        while refusal_of_block() {
            assert!(
                start.elapsed() < DEADLINE,
                "the refusal of a deleted block is still raised"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    });
}

const DUPLICATE_INVARIANT_ORG: &str = "\
* Profile a
:PROPERTIES:
:ID: pa
:END:
#+begin_src holon_entity_profile_yaml
entity_name: block
invariants:
  inv_y:
    holds: 'block.task_state == \"DONE\" => not child(block.task_state == \"TODO\")'
    on_violation: { repair: reopen_parent, set: { task_state: DOING } }
#+end_src
* Profile b
:PROPERTIES:
:ID: pb
:END:
#+begin_src holon_entity_profile_yaml
entity_name: block
invariants:
  inv_y:
    holds: 'block.task_state == \"DONE\" => not child(block.task_state == \"TODO\")'
    on_violation: { repair: reopen_parent, set: { task_state: DOING } }
#+end_src
";

/// Over the Turso CDC leg: of two vault profiles declaring one invariant name
/// the smaller block id wins, and the loser loads once the winner is deleted
/// — the state a fresh boot of the remaining vault reaches.
#[test]
fn the_loser_of_a_duplicate_invariant_loads_once_the_winner_is_deleted() {
    runtime().block_on(async {
        let booted = boot().await;
        let state = || {
            let declared: Vec<String> = booted
                .registry
                .declared_invariants("block")
                .iter()
                .map(|i| i.name.to_string())
                .collect();
            let mut refused: Vec<String> = booted
                .bus
                .current()
                .into_iter()
                .filter(|c| matches!(c.reason, ConditionKind::ProfileRefused { .. }))
                .map(|c| c.subject)
                .collect();
            refused.sort();
            (declared, refused)
        };
        let wait_for = |want: (Vec<String>, Vec<String>), label: &'static str| {
            let state = &state;
            async move {
                let start = Instant::now();
                while state() != want {
                    assert!(
                        start.elapsed() < DEADLINE,
                        "[{label}] (declared, refused) = {:?}, expected {want:?}",
                        state()
                    );
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        };
        std::fs::write(booted.profile_file(), DUPLICATE_INVARIANT_ORG).expect("write the vault");
        wait_for(
            (vec!["inv_y".into()], vec!["block:pb::src::0".into()]),
            "pa wins the duplicate",
        )
        .await;
        user_op(&booted, "delete", &[("id", "block:pa::src::0")]).await;
        wait_for(
            (vec!["inv_y".into()], vec![]),
            "pb loads once pa is deleted",
        )
        .await;
    });
}
