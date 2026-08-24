//! The block profile's render-requirement manifest.
//!
//! `RenderRequirements` is derived, not authored: a `col()` under a widget
//! parameter that declares a default is optional, everything else a variant
//! condition or template binds is required, and a computed field stands for the
//! columns its expression reads. This pins the result for the shipped block
//! profile so a change to `block_profile.yaml` or to a widget's `#[default]`
//! shows up as a deliberate edit here.

use std::collections::BTreeSet;

/// The widget parameter table lives in the frontend's builder registry, so the
/// derivation is only decidable once it is published.
fn registered_block_profile() -> holon_api::entity_profile::EntityProfile {
    holon_frontend::shadow_builders::register_render_dsl_widget_names();
    let registry =
        holon_profiles::type_registry::create_default_registry().expect("default type registry");
    let block = registry
        .all()
        .into_iter()
        .find(|td| td.name == "block")
        .expect("the default registry defines the block type");
    holon_profiles::profile_from_type_def(&block)
        .expect("the block type definition yields a profile")
}

fn owned(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|s| s.to_string()).collect()
}

#[test]
fn the_block_profile_requires_what_its_conditions_and_undefaulted_bindings_read() {
    let profile = registered_block_profile();
    let reqs = &profile.render_requirements;

    let declared_required: BTreeSet<String> = reqs
        .required()
        .intersection(&profile.declared_columns)
        .cloned()
        .collect();
    let declared_optional: BTreeSet<String> = reqs
        .optional()
        .intersection(&profile.declared_columns)
        .cloned()
        .collect();

    println!("REQUIRED (declared): {declared_required:?}");
    println!("OPTIONAL (declared): {declared_optional:?}");
    println!("REQUIRED (all):      {:?}", reqs.required());
    println!("OPTIONAL (all):      {:?}", reqs.optional());

    assert_eq!(
        declared_optional,
        owned(&["collapsed"]),
        "`collapsed` reaches the render only through `icon(col(\"bullet_shape\"))`, whose first \
         parameter declares the \"circle\" default, so its absence draws the plain bullet \
         instead of silencing nothing"
    );
    assert!(
        !reqs.requires("collapsed"),
        "an optional-with-default column must not also be required"
    );
    for column in [
        "content_type",
        "source_language",
        "parent_id",
        "widget_only",
    ] {
        assert!(
            reqs.requires(column),
            "`{column}` is read by a variant condition, which has no default to fall back on: \
             {declared_required:?}"
        );
    }
}

/// `is_legacy_rule` is evaluated at the enrich seat but no variant condition or
/// template binds it — the `action`-language banner reads `source_language`
/// directly through `if_col`. A column reachable only through an unbound
/// computed field is nobody's requirement.
#[test]
fn a_computed_field_no_template_binds_contributes_no_requirement() {
    let profile = registered_block_profile();
    let bound: BTreeSet<String> = profile
        .render_requirements
        .required()
        .union(profile.render_requirements.optional())
        .cloned()
        .collect();
    assert!(
        !bound.contains("is_legacy_rule"),
        "computed field names are expanded to the columns beneath them, never kept: {bound:?}"
    );
}
