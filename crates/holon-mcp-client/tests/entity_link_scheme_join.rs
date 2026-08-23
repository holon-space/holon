//! A `[[<entity>:<id>]]` link must resolve for an entity a YAML sidecar
//! declares — through the REAL registration path, not a hand-built registry.
//!
//! The two sides of this join spell the entity differently: `TypeRegistry` is
//! keyed by SQL table name (underscored) while a URI scheme is hyphenated.
//! Built-in entities are all single-word, so ONLY a multi-word sidecar entity
//! exercises the fold — which is why the fixture below is `t_widget`.

use std::sync::Arc;

use holon_api::link_parser::LinkTarget;
use holon_mcp_client::mcp_sidecar::McpSidecar;
use holon_profiles::TypeRegistry;

/// A sidecar declaring one multi-word entity, exactly as an integration ships.
const SIDECAR_YAML: &str = r#"
entities:
  t_widget:
    id_column: id
    schema:
      - name: id
        sql_type: TEXT
        primary_key: true
      - name: title
        sql_type: TEXT
"#;

/// Drive the SAME chain `McpIntegration::register_entity_types` drives:
/// sidecar entity → `prefixed_name().table_name()` → `to_type_definition` →
/// `TypeRegistry::register`.
fn registry_with_sidecar_entities(yaml: &str) -> Arc<TypeRegistry> {
    let sidecar = McpSidecar::from_yaml(yaml).expect("sidecar YAML parses");
    let registry = Arc::new(TypeRegistry::new());
    for (entity_name, entity_config) in &sidecar.entities {
        let table_name = sidecar.prefixed_name(entity_name).table_name();
        let td = entity_config
            .to_type_definition(
                &table_name,
                "test-provider",
                sidecar.write_ownership(entity_name),
            )
            .expect("entity with a schema yields a TypeDefinition");
        registry.register(td).expect("registration succeeds");
    }
    registry
}

#[test]
fn yaml_declared_multi_word_entity_resolves_as_an_entity_link() {
    let registry = registry_with_sidecar_entities(SIDECAR_YAML);
    let classifier = registry.link_target_classifier();

    let target = classifier.classify("t-widget:abc123");

    match &target {
        LinkTarget::Resolved(uri) => assert_eq!(uri.as_str(), "t-widget:abc123"),
        other => panic!(
            "a YAML-declared entity must classify as a resolved entity URI, got {other:?} — the \
             registry is keyed by table name (`t_widget`) but the scheme is hyphenated \
             (`t-widget`), so the join must fold one to the other"
        ),
    }
}

/// The classifier must track the registry LIVE: before the sidecar's entity is
/// registered the same target is an unknown scheme, and it must never have been
/// a page-creation intent at any point.
#[test]
fn the_same_target_is_unknown_scheme_until_the_entity_registers() {
    let empty = Arc::new(TypeRegistry::new());
    let before = empty.link_target_classifier().classify("t-widget:abc123");
    assert!(
        matches!(before, LinkTarget::UnknownScheme(_)),
        "an unregistered scheme must be a disclosed unknown-scheme link, never a page, got \
         {before:?}"
    );

    let registry = registry_with_sidecar_entities(SIDECAR_YAML);
    assert!(
        matches!(
            registry
                .link_target_classifier()
                .classify("t-widget:abc123"),
            LinkTarget::Resolved(_)
        ),
        "registering the entity must flip the SAME target to resolved"
    );
}
