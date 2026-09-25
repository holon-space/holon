//! How a block-profile condition sees the keys and the tag of a decision
//! block. Rows carry drawer keys in the `properties` object and the tags as
//! the JSON text of the `block` matview's `tags` column.

use std::collections::HashMap;

use holon_api::Value;
use holon_profiles::parse_entity_profile;
use rhai::Engine as RhaiEngine;

fn resolve(condition: &str, row: &HashMap<String, Value>) -> String {
    let yaml = format!(
        "entity_name: block\n\
         computed: {{}}\n\
         variants:\n  \
         - name: matched\n    \
         condition: '{condition}'\n    \
         render: 'row(col(\"content\"))'\n  \
         - name: default\n    \
         render: 'row(col(\"content\"))'\n"
    );
    parse_entity_profile(&yaml)
        .unwrap_or_else(|e| panic!("profile with condition {condition:?}: {e:#}"))
        .resolve(row, &RhaiEngine::new())
        .expect("a variant resolves")
        .name
        .clone()
}

fn row(properties: &[(&str, &str)], tags: &str) -> HashMap<String, Value> {
    let properties = properties
        .iter()
        .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
        .collect();
    HashMap::from([
        ("id".to_string(), Value::String("block:sharing-12-a".into())),
        ("content".to_string(), Value::String("Delete it".into())),
        ("tags".to_string(), Value::String(tags.to_string())),
        ("properties".to_string(), Value::Object(properties)),
    ])
}

fn option_row() -> HashMap<String, Value> {
    row(&[("option", "a")], "[]")
}

fn decision_row() -> HashMap<String, Value> {
    row(
        &[
            ("asked-by", "agent:claude-orch-0924"),
            ("decision-prefix", "sharing"),
            ("chosen", "a c"),
        ],
        r#"["decision"]"#,
    )
}

#[test]
fn plain_drawer_key_is_a_variable() {
    let condition = r#"is_def_var("option") && option != ()"#;
    assert_eq!(resolve(condition, &option_row()), "matched");
    assert_eq!(resolve(condition, &decision_row()), "default");
}

#[test]
fn multi_value_key_is_one_string() {
    assert_eq!(
        resolve(
            r#"is_def_var("chosen") && chosen == "a c""#,
            &decision_row()
        ),
        "matched"
    );
}

/// `asked-by` reads as `asked - by`: two unbound names, so the condition is a
/// structural non-match. No error names the hyphenated key.
#[test]
fn hyphenated_key_as_a_variable_never_matches() {
    assert_eq!(resolve("asked-by != ()", &decision_row()), "default");
}

#[test]
fn hyphenated_key_is_reachable_through_the_properties_map() {
    assert_eq!(
        resolve(r#"properties["asked-by"] != ()"#, &decision_row()),
        "matched"
    );
    assert_eq!(
        resolve(r#"properties["asked-by"] != ()"#, &option_row()),
        "default"
    );
}

#[test]
fn tag_is_matched_like_the_page_tag() {
    let condition = r#"tags != () && tags.contains("\"decision\"")"#;
    assert_eq!(resolve(condition, &decision_row()), "matched");
    assert_eq!(resolve(condition, &option_row()), "default");
    assert_eq!(
        resolve(condition, &row(&[], r#"["decision-log"]"#)),
        "default",
        "the quotes keep a longer tag from matching"
    );
}
