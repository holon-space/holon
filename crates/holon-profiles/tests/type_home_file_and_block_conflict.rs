//! The same invariant declared by a profile file in the vault's type home and
//! by an org-embedded profile block is a conflict: neither source wins, both
//! are refused, and ONE condition names both locations so the user can remove
//! one. Removing either declaration lifts the conflict.
//!
//! @pbt kind harness
//! @pbt covers type-home-file-and-block-conflict — a type-home file and a
//!   profile block declaring the same invariant are both refused under one
//!   condition naming both
//! @pbt overlaps vault-profile-admission-history-free — kept: that property
//!   drives blocks only and never reads a type-home file

use std::collections::BTreeMap;
use std::sync::Arc;

use holon_api::ConditionBus;
use holon_api::ConditionKind;
use holon_profiles::EntityProfile;
use holon_profiles::TypeRegistry;
use holon_profiles::VAULT_PROFILE_HOME;
use holon_profiles::vault_profiles::VaultProfileDerivation;

const INVARIANT: &str = "inv_x";
const BLOCK: &str = "block:a";

fn profile() -> String {
    format!(
        "entity_name: block\ninvariants:\n  {INVARIANT}:\n    holds: 'block.task_state == \
         \"DONE\" => not child(block.task_state == \"TODO\")'\n    on_violation: {{ repair: \
         reopen_parent, set: {{ task_state: DOING }} }}\n"
    )
}

#[test]
fn a_type_home_file_and_a_block_declaring_one_invariant_are_both_refused() {
    let vault = tempfile::tempdir().expect("tempdir");
    let home = vault.path().join(VAULT_PROFILE_HOME);
    std::fs::create_dir_all(&home).expect("create the vault type home");
    let file = home.join("home.yaml");
    std::fs::write(&file, profile()).expect("write the type-home profile");
    let file = file.display().to_string();

    let registry = Arc::new(TypeRegistry::new());
    registry
        .register(holon_api::block::Block::type_definition())
        .expect("register block");
    let bus = Arc::new(ConditionBus::new());
    registry.load_vault_profile_home(vault.path(), &bus);
    let mut derivation =
        VaultProfileDerivation::new(registry.vault_profile_admitter(), bus.clone());
    let mut in_effect: BTreeMap<String, Arc<EntityProfile>> = BTreeMap::new();
    let mut feed = |block: Option<String>| {
        let mut rows: BTreeMap<String, Arc<String>> = registry
            .vault_profile_file_rows()
            .iter()
            .map(|row| {
                let field = |name: &str| row[name].as_string().expect("string").to_string();
                (field("id"), Arc::new(field("content")))
            })
            .collect();
        if let Some(content) = block {
            rows.insert(BLOCK.to_string(), Arc::new(content));
        }
        if let Some(admitted) = derivation.derive(&rows) {
            in_effect = admitted;
        }
        in_effect.keys().cloned().collect::<Vec<_>>()
    };
    let declared = || -> Vec<String> {
        registry
            .declared_invariants("block")
            .iter()
            .map(|i| i.name.to_string())
            .collect()
    };
    let refusals = || -> Vec<(String, String)> {
        bus.current()
            .into_iter()
            .map(|c| match c.reason {
                ConditionKind::ProfileRefused { error } => (c.subject, error),
                other => panic!("unexpected condition {other:?}"),
            })
            .collect()
    };

    let admitted = feed(Some(profile()));
    assert_eq!(
        (admitted, declared()),
        (vec![], vec![]),
        "a file and a block declaring '{INVARIANT}' conflict: neither may take effect"
    );
    let conflict = refusals();
    assert_eq!(
        conflict.len(),
        1,
        "one condition must disclose the conflict: {conflict:?}"
    );
    let (subject, error) = &conflict[0];
    assert!(
        subject.contains(&file) && subject.contains(BLOCK),
        "the condition must name both the file and the block: {subject}"
    );
    assert!(
        error.contains(INVARIANT),
        "the condition must name the conflicting invariant: {error}"
    );

    let admitted = feed(None);
    assert_eq!(
        (admitted, declared(), refusals()),
        (vec![file], vec![INVARIANT.to_string()], vec![]),
        "removing the block lifts the conflict and clears its condition"
    );
}
