//! A vault profile that declares a computed field under the name of a TYPED
//! computed field of its entity is refused at load (ruling D66.a): the typed
//! field's one semantics (eval, planted SQL, live seat) would be replaced by a
//! Rhai script on the live seat alone.

use holon_profiles::TypedComputedFieldOverride;
use holon_profiles::create_default_registry;
use holon_profiles::parse_profile_yaml;

fn check(yaml: &str) -> anyhow::Result<()> {
    let check = create_default_registry()
        .expect("default registry loads")
        .profile_load_check();
    check(
        "block:vault-profile",
        &parse_profile_yaml(yaml).expect("profile parses"),
    )
}

#[test]
fn a_vault_profile_overriding_a_typed_computed_field_is_refused() {
    let err = check("entity_name: person\ncomputed:\n  display_name: 'email'\n")
        .expect_err("person.display_name is typed");
    let refusal = err
        .downcast_ref::<TypedComputedFieldOverride>()
        .unwrap_or_else(|| panic!("must be the typed refusal, got: {err:#}"));
    assert_eq!(refusal.profile, "block:vault-profile");
    assert_eq!(refusal.entity, "person");
    assert_eq!(refusal.field, "display_name");
}

#[test]
fn a_vault_profile_adding_a_new_computed_field_loads() {
    check("entity_name: person\ncomputed:\n  shout: 'email'\n")
        .unwrap_or_else(|e| panic!("a new field must load: {e:#}"));
}

fn late_person(registry: &holon_profiles::TypeRegistry, name: &str) -> holon_api::TypeDefinition {
    let mut late = registry.get("person").expect("person is bundled");
    late.name = name.to_string();
    late
}

#[test]
fn a_type_declaring_a_typed_field_a_loaded_vault_profile_redeclares_is_refused() {
    let registry = create_default_registry().expect("default registry loads");
    let check = registry.profile_load_check();
    check(
        "block:vault-profile",
        &parse_profile_yaml("entity_name: person_late\ncomputed:\n  display_name: '\"x\"'\n")
            .expect("profile parses"),
    )
    .expect("person_late is unregistered, so the profile cannot override a typed field yet");

    let err = registry
        .register(late_person(&registry, "person_late"))
        .expect_err("the profile already redeclares display_name");
    let refusal = err
        .downcast_ref::<TypedComputedFieldOverride>()
        .unwrap_or_else(|| panic!("must be the typed refusal, got: {err:#}"));
    assert_eq!(refusal.profile, "block:vault-profile");
    assert_eq!(refusal.entity, "person_late");
    assert_eq!(refusal.field, "display_name");
    assert!(
        !registry.contains("person_late"),
        "registry must stay unchanged"
    );
}

#[test]
fn a_profile_edited_to_drop_the_field_releases_its_claim() {
    let registry = create_default_registry().expect("default registry loads");
    let check = registry.profile_load_check();
    let parse = |yaml| parse_profile_yaml(yaml).expect("profile parses");
    check(
        "block:vault-profile",
        &parse("entity_name: person_late\ncomputed:\n  display_name: '\"x\"'\n"),
    )
    .expect("loads");
    check(
        "block:vault-profile",
        &parse("entity_name: person_late\ncomputed:\n  shout: '\"x\"'\n"),
    )
    .expect("loads");
    registry
        .register(late_person(&registry, "person_late"))
        .expect("the edited profile no longer redeclares display_name");
}

#[test]
fn a_profile_loaded_after_a_runtime_type_is_checked_against_that_type() {
    let registry = create_default_registry().expect("default registry loads");
    let check = registry.profile_load_check();
    registry
        .register(late_person(&registry, "person_late"))
        .expect("registers");
    let err = check(
        "block:vault-profile",
        &parse_profile_yaml("entity_name: person_late\ncomputed:\n  display_name: '\"x\"'\n")
            .expect("profile parses"),
    )
    .expect_err("typed field declared after the check closure was built");
    assert!(
        err.downcast_ref::<TypedComputedFieldOverride>().is_some(),
        "{err:#}"
    );
}

/// A type and a profile that disagree about a typed field must not both be
/// admitted, however their check and record steps interleave.
#[test]
fn a_type_and_a_profile_admitted_concurrently_are_never_both_accepted() {
    const TRIALS: usize = 500;
    let registry = std::sync::Arc::new(create_default_registry().expect("default registry loads"));
    let check = std::sync::Arc::new(registry.profile_load_check());
    let mut both_accepted = 0;
    for trial in 0..TRIALS {
        let entity = format!("person_race_{trial}");
        let profile = parse_profile_yaml(&format!(
            "entity_name: {entity}\ncomputed:\n  display_name: '\"x\"'\n"
        ))
        .expect("profile parses");
        let type_def = late_person(&registry, &entity);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let loader = {
            let (check, barrier) = (check.clone(), barrier.clone());
            std::thread::spawn(move || {
                barrier.wait();
                check("block:vault-profile", &profile).is_ok()
            })
        };
        barrier.wait();
        let type_accepted = registry.register(type_def).is_ok();
        let profile_accepted = loader.join().expect("loader thread");
        if type_accepted && profile_accepted {
            both_accepted += 1;
        }
    }
    assert_eq!(
        both_accepted, 0,
        "in {both_accepted} of {TRIALS} trials the registry holds a typed display_name AND a vault profile redeclares it"
    );
}
