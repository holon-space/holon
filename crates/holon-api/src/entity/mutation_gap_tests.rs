use super::*;

#[test]
fn field_schema_builder_chain_preserves_everything() {
    let f = FieldSchema::new("score", "INTEGER")
        .nullable()
        .indexed()
        .default_value("0")
        .lifetime(FieldLifetime::Transient)
        .edge_name("SCORED_BY")
        .reference_target("block");

    assert_eq!(f.name, "score");
    assert_eq!(f.sql_type, "INTEGER");
    assert!(f.nullable);
    assert!(f.indexed);
    assert!(!f.primary_key);
    assert_eq!(f.default_value.as_deref(), Some("0"));
    assert!(matches!(f.lifetime, FieldLifetime::Transient));
    assert_eq!(f.edge_name.as_deref(), Some("SCORED_BY"));
    assert_eq!(f.reference_target.as_deref(), Some("block"));
}

#[test]
fn type_definition_lifetime_projections_and_index_sql() {
    let engine = rhai::Engine::new();
    let spec = ComputedSpec::parse(
        "score",
        "a + 1",
        ComputedTier::ComputedLive,
        &FieldTypes::new(),
        &engine,
    )
    .unwrap();

    let td = TypeDefinition::new(
        "thing",
        vec![
            FieldSchema::new("id", "TEXT").primary_key().indexed(),
            FieldSchema::new("title", "TEXT").indexed(),
            FieldSchema::new("score", "INTEGER").lifetime(FieldLifetime::Computed {
                spec: Box::new(spec),
            }),
            FieldSchema::new("cache", "TEXT")
                .indexed()
                .lifetime(FieldLifetime::Transient),
        ],
    );

    let persistent: Vec<&str> = td
        .persistent_fields()
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(persistent, vec!["id", "title"]);

    let computed: Vec<&str> = td.computed_fields().iter().map(|(n, _)| *n).collect();
    assert_eq!(computed, vec!["score"]);

    let transient: Vec<&str> = td
        .transient_fields()
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(transient, vec!["cache"]);

    // Indexed non-PK fields only: no idx for the primary key.
    let idx = td.to_index_sql();
    assert_eq!(
        idx,
        vec![
            "CREATE INDEX IF NOT EXISTS idx_thing_title ON \"thing\" (\"title\")".to_string(),
            "CREATE INDEX IF NOT EXISTS idx_thing_cache ON \"thing\" (\"cache\")".to_string(),
        ]
    );
}

#[test]
fn a_misspelled_key_in_a_field_entry_is_refused() {
    let err = serde_json::from_str::<TypeDefinition>(
        r#"{"name":"t","fields":[{"name":"id","sql_typ":"TEXT"}]}"#,
    )
    .expect_err("an unknown key inside a field entry must be refused")
    .to_string();
    assert!(err.contains("sql_typ"), "got: {err}");
}

#[test]
fn a_field_entry_without_a_name_is_refused() {
    for fields in [
        r#"[{"sql_type":"TEXT"}]"#,
        r#"[{"name":"","sql_type":"TEXT"}]"#,
    ] {
        let err =
            serde_json::from_str::<TypeDefinition>(&format!(r#"{{"name":"t","fields":{fields}}}"#))
                .expect_err("a field entry with no name must be refused")
                .to_string();
        assert!(err.contains("name"), "{fields}: got: {err}");
    }
}

#[test]
fn a_type_name_that_is_no_entity_name_is_refused() {
    let quoted = r#"t\" (id TEXT); DROP TABLE block; --"#;
    for name in [quoted, "", "9lives", "a b"] {
        let json = format!(r#"{{"name":"{name}","fields":[{{"name":"id","sql_type":"TEXT"}}]}}"#);
        let err = match serde_json::from_str::<TypeDefinition>(&json) {
            Ok(td) => panic!(
                "{name:?} must be refused, got DDL: {}",
                td.to_create_table_sql()
            ),
            Err(e) => e.to_string(),
        };
        assert!(
            err.contains("is not a valid entity name"),
            "{name:?}: got: {err}"
        );
    }
}

#[test]
fn primary_key_defaults_to_id_on_deserialize() {
    let td: TypeDefinition = serde_json::from_str(r#"{"name":"t","fields":[]}"#).unwrap();
    assert_eq!(td.primary_key.as_str(), "id");
}

#[test]
fn dynamic_entity_typed_getters() {
    let mut e = DynamicEntity::new("block")
        .with_field("s", "str")
        .with_field("i", 42i64)
        .with_field("b", true)
        .with_field("f", 1.5f64);

    assert_eq!(e.type_name, "block");
    assert!(e.has_field("s"));
    assert!(!e.has_field("nope"));
    assert_eq!(e.get_string("s"), Some("str".to_string()));
    assert_eq!(e.get_i64("i"), Some(42));
    assert_eq!(e.get_bool("b"), Some(true));
    assert_eq!(e.get_f64("f"), Some(1.5));
    assert_eq!(e.get_string("i"), None);
    assert_eq!(e.get_i64("missing"), None);

    e.set("s", "new");
    assert_eq!(e.get_string("s"), Some("new".to_string()));
    *e.get_mut("i").unwrap() = Value::Integer(7);
    assert_eq!(e.get_i64("i"), Some(7));
    assert_eq!(e.remove("b"), Some(Value::Boolean(true)));
    assert!(!e.has_field("b"));
    assert_eq!(e.remove("b"), None);
}
