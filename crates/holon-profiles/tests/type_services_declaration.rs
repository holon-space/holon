//! A type declares the services it wants under `services:` in its YAML. The
//! declaration parses into a typed value, round-trips, and is refused by name
//! when a key is unknown, ill-typed, or names a field the type does not have.

use std::path::Path;
use std::path::PathBuf;

use holon_api::TypeDefinition;
use holon_profiles::TypeRegistry;
use holon_profiles::create_default_registry;
use holon_profiles::parse_profile_yaml;

const STEP_FIELDS: &str = "\
name: step
primary_key: id
fields:
  - name: id
    sql_type: TEXT
    primary_key: true
  - name: recipe_id
    sql_type: TEXT
  - name: position
    sql_type: INTEGER
  - name: text
    sql_type: TEXT
  - name: count
    sql_type: INTEGER
";

const STEP_SERVICES: &str = "\
services:
  title: text
  searchable: [text]
  linkable: true
  embeddable: true
  dense_view: { body: text }
  hierarchy: { parent: recipe_id, order: position }
  rich_text: [text]
";

fn step_yaml(services: &str) -> String {
    format!("{STEP_FIELDS}{services}")
}

fn parse(yaml: &str) -> anyhow::Result<TypeDefinition> {
    Ok(serde_yaml::from_str(yaml)?)
}

fn register(yaml: &str) -> anyhow::Result<TypeDefinition> {
    let registry = TypeRegistry::new();
    registry.register(parse(yaml)?)?;
    Ok(registry.get("step").expect("registered type is found"))
}

fn services_of(type_def: &TypeDefinition) -> serde_yaml::Value {
    serde_yaml::to_value(type_def).expect("type definition serializes")["services"].clone()
}

fn refusal(yaml: &str) -> String {
    match register(yaml) {
        Ok(td) => panic!("declaration accepted, services = {:?}", services_of(&td)),
        Err(e) => format!("{e:#}"),
    }
}

#[test]
fn a_declared_type_round_trips() {
    let declared: serde_yaml::Value = serde_yaml::from_str(STEP_SERVICES).unwrap();
    let registered = register(&step_yaml(STEP_SERVICES)).unwrap();
    assert_eq!(services_of(&registered), declared["services"]);

    let reparsed = parse(&serde_yaml::to_string(&registered).unwrap()).unwrap();
    assert_eq!(services_of(&reparsed), declared["services"]);
}

#[test]
fn an_unknown_service_key_is_refused_by_name() {
    let msg = refusal(&step_yaml(
        "services:\n  title: text\n  searchable_by: [text]\n",
    ));
    assert!(msg.contains("searchable_by"), "{msg}");
}

#[test]
fn an_unknown_key_inside_a_service_is_refused_by_name() {
    let msg = refusal(&step_yaml(
        "services:\n  title: text\n  dense_view: { body: text, heading: text }\n",
    ));
    assert!(msg.contains("heading"), "{msg}");
}

#[test]
fn an_ill_typed_service_key_is_refused_by_name() {
    let msg = refusal(&step_yaml(
        "services:\n  title: text\n  linkable: \"yes\"\n",
    ));
    assert!(msg.contains("linkable"), "{msg}");
}

#[test]
fn a_field_reference_that_is_not_an_identifier_is_refused() {
    let msg = refusal(&step_yaml("services:\n  title: \"first line\"\n"));
    assert!(msg.contains("first line"), "{msg}");
}

#[test]
fn services_without_a_title_are_refused() {
    let msg = refusal(&step_yaml("services:\n  searchable: [text]\n"));
    assert!(msg.contains("title"), "{msg}");
}

#[test]
fn a_service_naming_a_missing_field_is_refused_with_the_field() {
    let msg = refusal(&step_yaml(
        "services:\n  title: text\n  hierarchy: { parent: recipe, order: position }\n",
    ));
    for needle in ["step", "hierarchy.parent", "`recipe`"] {
        assert!(msg.contains(needle), "missing {needle:?} in: {msg}");
    }
}

#[test]
fn a_text_service_naming_a_non_text_field_is_refused() {
    let msg = refusal(&step_yaml(
        "services:\n  title: text\n  searchable: [text, count]\n",
    ));
    for needle in ["searchable", "`count`", "INTEGER"] {
        assert!(msg.contains(needle), "missing {needle:?} in: {msg}");
    }
}

#[test]
fn block_declares_its_own_services() {
    let block = create_default_registry().unwrap().get("block").unwrap();
    let expected: serde_yaml::Value = serde_yaml::from_str(
        "title: content\n\
         searchable: [content]\n\
         linkable: true\n\
         embeddable: true\n\
         dense_view: { body: content }\n\
         hierarchy: { parent: parent_id }\n\
         rich_text: [content]\n",
    )
    .unwrap();
    assert_eq!(services_of(&block), expected);
}

fn shipped_type_dirs() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    vec![
        root.join("assets/default/types"),
        root.join("crates/holon-kitchen/assets/types"),
    ]
}

#[test]
fn every_shipped_type_yaml_still_loads() {
    create_default_registry().expect("default registry loads");

    let mut loaded = Vec::new();
    for dir in shipped_type_dirs() {
        for entry in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let yaml = std::fs::read_to_string(&path).unwrap();
            if name.ends_with("_profile.yaml") {
                parse_profile_yaml(&yaml).unwrap_or_else(|e| panic!("{name}: {e:#}"));
            } else if name.ends_with("_services.yaml") {
                continue;
            } else {
                TypeRegistry::new()
                    .register(parse(&yaml).unwrap_or_else(|e| panic!("{name}: {e:#}")))
                    .unwrap_or_else(|e| panic!("{name}: {e:#}"));
            }
            loaded.push(name);
        }
    }
    assert!(loaded.len() >= 10, "found only {loaded:?}");
}
