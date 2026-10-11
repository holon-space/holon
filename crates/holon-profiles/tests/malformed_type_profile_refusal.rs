//! A type profile whose variant does not parse is refused as a typed
//! `MalformedTypeProfile` naming the type, its origin, the variant, the part
//! and the text; every other type profile is still built.

use holon_api::ConditionBus;
use holon_api::ConditionKind;
use holon_api::MalformedTypeProfile;
use holon_api::TypeDefinition;
use holon_api::TypeSource;
use holon_api::VariantPart;
use holon_profiles::create_default_registry;
use holon_profiles::profile_from_type_def;
use holon_profiles::type_profiles_from_registry;

fn type_with_variant(name: &str, condition: &str, render: &str) -> TypeDefinition {
    serde_yaml::from_str(&format!(
        "\
name: {name}
fields:
  - name: id
    sql_type: TEXT
    primary_key: true
profile_variants:
  - name: {name}_card
    condition: '{condition}'
    render: '{render}'
"
    ))
    .expect("the test type parses")
}

const RENDER: &str = r#"text(col("id"))"#;

#[test]
fn a_condition_that_does_not_split_is_refused_naming_it() {
    let condition = "flagged || archived && is_focused";
    let refusal = profile_from_type_def(&type_with_variant("gadget", condition, RENDER))
        .expect_err("a condition that does not split must refuse the profile");
    assert_eq!(
        (
            refusal.entity.as_str(),
            &refusal.origin,
            refusal.variant.as_str(),
            refusal.part,
            refusal.text.as_str()
        ),
        (
            "gadget",
            &TypeSource::UserDefined,
            "gadget_card",
            VariantPart::Condition,
            condition
        )
    );
    assert!(refusal.reason.contains("`||`"), "{refusal}");
}

#[test]
fn a_render_that_does_not_parse_is_refused_naming_it() {
    let render = r#"text(col("id")"#;
    let refusal = profile_from_type_def(&type_with_variant("gizmo", "id != ()", render))
        .expect_err("a render that does not parse must refuse the profile");
    assert_eq!(
        (
            refusal.variant.as_str(),
            refusal.part,
            refusal.text.as_str()
        ),
        ("gizmo_card", VariantPart::Render, render)
    );
}

#[test]
fn the_registry_bridge_discloses_the_refusal_and_builds_the_rest() {
    let registry = create_default_registry().expect("default registry loads");
    registry
        .register(type_with_variant(
            "gadget",
            "flagged || archived && is_focused",
            RENDER,
        ))
        .expect("register the malformed type");
    registry
        .register(type_with_variant("widget", "id != ()", RENDER))
        .expect("register the well-formed type");
    let bus = ConditionBus::new();

    let built: Vec<String> = type_profiles_from_registry(&registry, &bus)
        .iter()
        .map(|p| p.entity_name.as_str().to_string())
        .collect();

    assert!(!built.contains(&"gadget".to_string()), "{built:?}");
    for entity in ["widget", "block", "person"] {
        assert!(
            built.contains(&entity.to_string()),
            "{entity} missing: {built:?}"
        );
    }
    let refused: Vec<MalformedTypeProfile> = bus
        .current()
        .into_iter()
        .filter_map(|c| match c.reason {
            ConditionKind::TypeProfileRefused(refusal) => {
                assert_eq!(c.subject, refusal.entity);
                Some(refusal)
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        refused
            .iter()
            .map(|r| r.entity.as_str())
            .collect::<Vec<_>>(),
        ["gadget"]
    );
}

#[test]
fn every_bundled_type_profile_parses() {
    let registry = create_default_registry().expect("default registry loads");
    for type_def in registry.all() {
        if let Err(refusal) = profile_from_type_def(&type_def) {
            panic!("a bundled type profile is refused: {refusal}");
        }
    }
}
