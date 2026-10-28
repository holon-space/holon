//! A profile file in the vault's own type home, `<vault>/.holon/types/*.yaml`,
//! takes effect through the SAME admitter the org-embedded profile blocks go
//! through (ruling PROFILE-home.a), and a malformed file is refused whole and
//! disclosed as a condition instead of aborting the boot (ruling
//! PROFILE-panic.a).
//!
//! @pbt kind harness
//! @pbt covers vault-home-type-profile — a profile file in
//! `<vault>/.holon/types` takes effect through the vault profile admitter, and
//! a malformed file is refused whole and disclosed
//! @pbt overlaps vault-profile-admission-history-free — kept: that property
//! drives the admitter directly and never reads a file

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use holon_conditions::ConditionBus;
use holon_conditions::ConditionKind;
use holon_frontend::config::HolonConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::config::VaultConfig;
use holon_profiles::TypeRegistry;

const GOOD_FILE: &str = "home_invariant.yaml";
const BROKEN_FILE: &str = "broken.yaml";
const INVARIANT: &str = "home_parent_reopens";

/// A `block` profile declaring one invariant, the shape
/// `vault_profile_admission_is_history_free` admits.
fn good_profile() -> String {
    format!(
        "entity_name: block\ninvariants:\n  {INVARIANT}:\n    holds: 'block.task_state == \
         \"DONE\" => not child(block.task_state == \"TODO\")'\n    on_violation: {{ repair: \
         reopen_parent, set: {{ task_state: DOING }} }}\n"
    )
}

/// Write the vault's type home and return the vault root.
fn seed_type_home(dir: &Path) {
    let home = dir.join(".holon").join("types");
    std::fs::create_dir_all(&home).expect("create the vault type home");
    std::fs::write(home.join(GOOD_FILE), good_profile()).expect("write the good profile");
    std::fs::write(home.join(BROKEN_FILE), "entity_name: [").expect("write the broken profile");
}

struct Booted {
    types: Arc<TypeRegistry>,
    bus: Arc<ConditionBus>,
    _session: Arc<holon_frontend::FrontendSession>,
}

async fn boot(dir: &Path) -> Booted {
    let holon_config = HolonConfig {
        db_path: Some(dir.join("vault-home-type-profile.db")),
        vault: VaultConfig {
            root: Some(dir.to_path_buf()),
        },
        ..Default::default()
    };
    let (session, _engine, (bus, types)) = holon_app::new_from_config_with_di(
        holon_config,
        SessionConfig::new(holon_api::UiInfo::permissive()),
        dir.to_path_buf(),
        HashSet::new(),
        Arc::new(ConditionBus::new()),
        |injector| {
            holon::testing::database_stuck_guard::report_database_stuck_in(injector);
            Ok(())
        },
        |injector| {
            (
                (*injector.resolve::<Arc<ConditionBus>>()).clone(),
                injector.resolve::<TypeRegistry>(),
            )
        },
    )
    .await
    .expect("a malformed profile file is the user's to fix, so the session must boot");
    Booted {
        types,
        bus,
        _session: session,
    }
}

#[test]
fn a_vault_home_profile_takes_effect_and_a_malformed_one_is_refused_and_disclosed() {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build test runtime")
        .block_on(async {
            let dir = tempfile::tempdir().expect("tempdir");
            seed_type_home(dir.path());
            let booted = boot(dir.path()).await;

            let declared: Vec<String> = booted
                .types
                .declared_invariants("block")
                .iter()
                .map(|i| i.name.to_string())
                .collect();
            assert!(
                declared.contains(&INVARIANT.to_string()),
                "the profile file in the vault's type home must take effect: {declared:?}"
            );

            let refusals: Vec<(String, String)> = booted
                .bus
                .current()
                .into_iter()
                .filter_map(|c| match c.reason {
                    ConditionKind::ProfileRefused { error } => Some((c.subject, error)),
                    _ => None,
                })
                .collect();
            assert_eq!(
                refusals.len(),
                1,
                "exactly the malformed file must be disclosed: {refusals:?}"
            );
            let (subject, error) = &refusals[0];
            assert!(
                subject.ends_with(BROKEN_FILE),
                "the disclosure must name the refused file: {subject}"
            );
            assert!(
                !error.is_empty(),
                "the disclosure must carry why the file is refused"
            );
        });
}
