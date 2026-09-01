//! Inc A rung 2 — the kitchen datatypes declare cleanly and the recipe page
//! exists as a RENDER PROFILE on the `recipe` type (its default view), not as a
//! block seeded under the root layout.

use holon_kitchen::INGREDIENT_USE_TYPE_YAML;
use holon_kitchen::RECIPE_PROFILE_YAML;
use holon_kitchen::RECIPE_TYPE_YAML;
use holon_kitchen::register_kitchen_types;
use holon_profiles::parse_profile_yaml;
use holon_profiles::type_registry::TypeRegistry;

#[test]
fn recipe_type_yaml_parses_and_declares_its_columns() {
    let td: holon_api::entity::TypeDefinition = serde_yaml::from_str(RECIPE_TYPE_YAML).unwrap();
    assert_eq!(td.name, "recipe");
    let names: Vec<&str> = td.fields.iter().map(|f| f.name.as_str()).collect();
    for expected in ["id", "source_path", "title", "servings", "course"] {
        assert!(
            names.contains(&expected),
            "missing column {expected}: {names:?}"
        );
    }
}

#[test]
fn ingredient_use_keeps_an_unbound_product_slot() {
    let td: holon_api::entity::TypeDefinition =
        serde_yaml::from_str(INGREDIENT_USE_TYPE_YAML).unwrap();
    assert_eq!(td.name, "ingredient_use");
    let product = td
        .fields
        .iter()
        .find(|f| f.name == "product_id")
        .expect("no product_id column");
    assert!(
        product.nullable,
        "product_id must be nullable — Inc A leaves it unbound and VISIBLY unmatched"
    );
}

#[test]
fn recipe_profile_parses_and_its_lookups_are_registered() {
    let profile = parse_profile_yaml(RECIPE_PROFILE_YAML).expect("recipe profile parses");
    assert_eq!(profile.entity_name, "recipe");
    // Same boot-time guard the bundled profiles get: a render calling a lookup
    // the engine never registers must fail HERE, not degrade to () at eval.
    holon_profiles::validate_lookups_registered(&profile)
        .expect("recipe profile references only registered lookups");
}

#[test]
fn the_recipe_page_is_the_types_default_variant() {
    let profile = parse_profile_yaml(RECIPE_PROFILE_YAML).unwrap();
    let default = profile
        .variants
        .iter()
        .find(|v| v.name == "default")
        .expect("recipe has no default variant — the page IS the default view");
    assert!(
        default.render.contains("title"),
        "recipe page must render its title: {:?}",
        default.render
    );
}

#[test]
fn kitchen_types_register_into_a_registry() {
    let registry = TypeRegistry::new();
    register_kitchen_types(&registry).expect("kitchen types register");
    assert!(registry.get("recipe").is_some(), "recipe not registered");
    assert!(
        registry.get("ingredient_use").is_some(),
        "ingredient_use not registered"
    );
}

#[test]
fn kitchen_types_coexist_with_the_bundled_types() {
    // The empty-registry case above cannot catch a name collision or a lookup
    // the real boot registers differently. Register into the SAME registry
    // production boots with.
    let registry =
        holon_profiles::type_registry::create_default_registry().expect("default registry builds");
    register_kitchen_types(&registry).expect("kitchen types register alongside bundled types");
    for bundled in ["block", "person", "organization"] {
        assert!(
            registry.get(bundled).is_some(),
            "kitchen registration displaced bundled type {bundled}"
        );
    }
    assert!(registry.get("recipe").is_some());
    assert!(registry.get("ingredient_use").is_some());
}
