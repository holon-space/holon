//! A profile expression that reads a name no row of its entity can carry is
//! refused when the profile loads (ruling D171.a), naming the expression, the
//! name and the key the author most likely meant.

use holon_profiles::create_default_registry;
use holon_profiles::parse_profile_yaml;

fn block_profile_with_condition(condition: &str) -> String {
    format!(
        "entity_name: block\nvariants:\n  - name: asked\n    priority: 5\n    condition: '{condition}'\n    render: 'text(col(\"content\"))'\n"
    )
}

fn load(yaml: &str) -> anyhow::Result<()> {
    let registry = create_default_registry().expect("default registry loads");
    registry.apply_parsed_profile(parse_profile_yaml(yaml).expect("profile parses"))
}

#[test]
fn a_hyphenated_property_key_is_refused_with_the_intended_key() {
    let err = load(&block_profile_with_condition("asked-by != ()"))
        .expect_err("`asked-by` parses as `asked - by` and can never match");
    let msg = format!("{err:#}");
    for needle in [
        "asked-by != ()",
        "`asked`",
        "`by`",
        "properties[\"asked-by\"]",
        "variant 'asked'",
    ] {
        assert!(msg.contains(needle), "missing {needle:?} in: {msg}");
    }
}

#[test]
fn an_undeclared_identifier_is_refused() {
    let err = load(&block_profile_with_condition("asker == \"martin\""))
        .expect_err("`asker` is neither a block column, a computed field nor UI state");
    let msg = format!("{err:#}");
    for needle in ["asker == \"martin\"", "`asker`", "is_def_var(\"asker\")"] {
        assert!(msg.contains(needle), "missing {needle:?} in: {msg}");
    }
}

#[test]
fn a_read_the_optimizer_folds_away_is_refused() {
    let err = load(&block_profile_with_condition(
        "false && asker == \"martin\"",
    ))
    .expect_err("the source reads `asker` unguarded");
    assert!(format!("{err:#}").contains("`asker`"), "{err:#}");
}

#[test]
fn an_undeclared_identifier_in_a_computed_field_is_refused() {
    let err = load("entity_name: block\ncomputed:\n  is_asked: 'asker == \"martin\"'\n")
        .expect_err("a computed field is as silent as a condition when its input never exists");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("computed field 'is_asked'") && msg.contains("`asker`"),
        "{msg}"
    );
}

#[test]
fn guarded_and_declared_reads_load() {
    for condition in [
        "properties[\"asked-by\"] != ()",
        "is_def_var(\"asker\") && asker == \"martin\"",
        "asker != () && asker == \"martin\"",
        "asker != ()",
        "asker == ()",
        "asker != () && true",
        "true && asker != ()",
        "is_task && content_type == \"text\" && tags != ()",
        "is_focused && role == \"page_title\"",
    ] {
        load(&block_profile_with_condition(condition))
            .unwrap_or_else(|e| panic!("`{condition}` must load: {e:#}"));
    }
}

#[test]
fn an_org_embedded_profile_is_checked_against_the_registered_types() {
    let check = create_default_registry()
        .expect("default registry loads")
        .profile_scope_check();
    let refused = parse_profile_yaml(&block_profile_with_condition("asked-by != ()"))
        .expect("profile parses");
    let msg = format!("{:#}", check(&refused).expect_err("hyphenated key refused"));
    assert!(msg.contains("properties[\"asked-by\"]"), "{msg}");

    // A bundled computed field (`is_task`), a lone guard and a profile named
    // after no type.
    for yaml in [
        block_profile_with_condition("is_task"),
        block_profile_with_condition("asker != ()"),
        "entity_name: kanban_collection\nvariants:\n  - name: board\n    render: 'text(\"b\")'\n"
            .to_string(),
    ] {
        check(&parse_profile_yaml(&yaml).expect("profile parses"))
            .unwrap_or_else(|e| panic!("must load: {e:#}"));
    }
}
