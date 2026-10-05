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
