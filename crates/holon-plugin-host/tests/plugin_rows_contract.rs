//! The stream types a guest writes and the host reads, from the host side.
//! `guests/plugin-rows` is outside the workspace, so its contract is pinned
//! here, where the gates run.

use holon_plugin_rows::BlockKey;
use holon_plugin_rows::BlockRow;
use holon_plugin_rows::DocumentRow;
use holon_plugin_rows::EntityRef;
use holon_plugin_rows::Fields;
use holon_plugin_rows::IdError;
use holon_plugin_rows::Line;
use holon_plugin_rows::LocalId;
use holon_plugin_rows::Owner;
use holon_plugin_rows::Row;
use holon_plugin_rows::Scope;
use holon_plugin_rows::Stream;
use serde_json::Value;
use serde_json::json;

#[test]
fn a_stream_reads_back_as_written() {
    let recipe = LocalId::from_path("Rezepte/Grüne Soße.cook").unwrap();
    let mut cells = Fields::new();
    cells.insert("raw_name", json!("Mehl"));
    let mut refs = Fields::new();
    refs.insert(
        "recipe_id",
        EntityRef {
            type_name: "recipe".into(),
            id: recipe.clone(),
        },
    );
    let stream = Stream {
        scopes: vec![Scope {
            type_name: "ingredient_use".into(),
            owner_column: "recipe_id".into(),
            owner: Owner::Ref(EntityRef {
                type_name: "recipe".into(),
                id: recipe.clone(),
            }),
        }],
        lines: vec![
            Line::Document(DocumentRow {
                title: "Grüne Soße".into(),
                properties: Fields::new(),
            }),
            Line::Block(BlockRow {
                key: BlockKey::new(["b", "0"]).unwrap(),
                content: "Kräuter hacken.".into(),
                properties: Fields::new(),
            }),
            Line::Row(Row {
                type_name: "ingredient_use".into(),
                id: recipe.part("iu").unwrap().part("mehl-0").unwrap(),
                refs,
                cells,
            }),
        ],
    };
    assert_eq!(Stream::from_jsonl(&stream.to_jsonl()).unwrap(), stream);
}

#[test]
fn a_malformed_id_is_refused_by_name() {
    for (id, expected) in [
        (json!({"path": []}), IdError::NoPath),
        (
            json!({"path": ["a", ""]}),
            IdError::EmptySegment {
                path: vec!["a".into(), "".into()],
            },
        ),
        (
            json!({"path": ["a/b"]}),
            IdError::SlashInSegment {
                segment: "a/b".into(),
            },
        ),
        (
            json!({"path": ["a"], "parts": [""]}),
            IdError::EmptyPart {
                parts: vec!["".into()],
            },
        ),
    ] {
        let error = serde_json::from_value::<LocalId>(id)
            .unwrap_err()
            .to_string();
        assert!(error.contains(&expected.to_string()), "{error}");
    }
    assert!(serde_json::from_value::<LocalId>(json!("already:schemed")).is_err());
}

#[test]
fn a_field_stated_twice_is_refused() {
    let error = serde_json::from_str::<Fields<Value>>(r#"{"a": 1, "a": 2}"#).unwrap_err();
    assert!(error.to_string().contains("stated twice"), "{error}");
}

#[test]
fn another_contract_version_is_refused() {
    let error = Stream::from_jsonl("{\"holon_rows\":1,\"scopes\":[]}\n").unwrap_err();
    assert_eq!(error.line, 1);
    assert!(error.message.contains("speaks 2"), "{error}");
}
