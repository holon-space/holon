//! The neutral contract carries every row a producer may emit, unchanged.
//!
//! Emit → parse is the identity on legal row sets. `Value` is `untagged`, so a
//! bare JSON row cannot say whether a string is a `String`, a `DateTime` or a
//! `Json` document; the envelope carries that beside the rows, the way the SQL
//! leg's `property_kinds` column already does.
//!
//! The generator therefore fixes ONE non-null variant per column, because the
//! kind map is per column: a scope whose rows disagreed about a column's kind
//! is a producer bug the contract must refuse, not round-trip.

use std::collections::BTreeMap;
use std::sync::Arc;

use holon_api::StorageEntity;
use holon_api::Value;
use holon_core::file_format::TypedRowSet;
use holon_rows::emit_row_sets;
use holon_rows::parse_row_sets;
use proptest::prelude::*;

/// Which non-null variant one column holds, across every row of one scope.
#[derive(Debug, Clone, Copy)]
enum Kind {
    Str,
    Int,
    Float,
    Bool,
    DateTime,
    Json,
    Array,
    Object,
}

fn kind() -> impl Strategy<Value = Kind> {
    prop_oneof![
        Just(Kind::Str),
        Just(Kind::Int),
        Just(Kind::Float),
        Just(Kind::Bool),
        Just(Kind::DateTime),
        Just(Kind::Json),
        Just(Kind::Array),
        Just(Kind::Object),
    ]
}

/// A value nestable inside an `Array` or `Object` column. The kind map names
/// columns, not paths inside them, so a nested `DateTime`/`Json` has nowhere
/// to record its kind — the same boundary the properties bag already draws.
fn nested_value() -> impl Strategy<Value = Value> {
    prop_oneof![
        "[a-z ]{0,8}".prop_map(Value::String),
        any::<i64>().prop_map(Value::Integer),
        (-1e6f64..1e6f64).prop_map(Value::Float),
        any::<bool>().prop_map(Value::Boolean),
        Just(Value::Null),
    ]
}

/// The documents a `Json` column may hold. Every JSON scalar is a document in
/// its own right, `null` among them — a filter or a remote API emits them
/// routinely, and none of them is a NULL.
///
/// Canonical spellings only: a `Json` value is a DOCUMENT, and both this
/// contract and the SQL leg re-serialize it, so authored whitespace is not
/// something either promises to keep. No leading zeros either: `037` is not
/// JSON, and the emitter is right to refuse it.
fn json_doc() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("null".to_string()),
        Just("false".to_string()),
        Just("0".to_string()),
        Just("\"s\"".to_string()),
        Just("[]".to_string()),
        Just("1.5".to_string()),
        "\\{\"k\":(0|[1-9][0-9]{0,2})\\}",
    ]
}

fn value_of(kind: Kind) -> BoxedStrategy<Value> {
    match kind {
        Kind::Str => "[\\PC]{0,12}".prop_map(Value::String).boxed(),
        Kind::Int => any::<i64>().prop_map(Value::Integer).boxed(),
        Kind::Float => (-1e9f64..1e9f64).prop_map(Value::Float).boxed(),
        Kind::Bool => any::<bool>().prop_map(Value::Boolean).boxed(),
        Kind::DateTime => "20[0-9]{2}-01-0[1-9]T00:00:00Z"
            .prop_map(Value::DateTime)
            .boxed(),
        Kind::Json => json_doc().prop_map(Value::Json).boxed(),
        Kind::Array => proptest::collection::vec(nested_value(), 0..3)
            .prop_map(Value::Array)
            .boxed(),
        Kind::Object => proptest::collection::hash_map("[a-z]{1,4}", nested_value(), 0..3)
            .prop_map(|m| Value::Object(m.into_iter().collect()))
            .boxed(),
    }
}

/// What one row states for one column: a value, an explicit NULL, or nothing
/// at all.
///
/// A `Json` column draws no NULL: the document `null` and a NULL share one
/// wire form, so the contract refuses a scope that mixes them rather than pick
/// one when reading it back.
fn cell_of(kind: Kind) -> BoxedStrategy<Option<Value>> {
    match kind {
        Kind::Json => prop_oneof![
            2 => value_of(kind).prop_map(Some),
            1 => Just(None),
        ]
        .boxed(),
        _ => prop_oneof![
            2 => value_of(kind).prop_map(Some),
            1 => Just(Some(Value::Null)),
            1 => Just(None),
        ]
        .boxed(),
    }
}

/// One scope: a column schema, then rows that each carry a subset of it.
/// A column a row omits stays omitted; a column a row sets to `Null` stays
/// present-and-null. Those are different rows and both must survive.
fn scope(type_name: String) -> impl Strategy<Value = TypedRowSet> {
    proptest::collection::btree_map("[a-z_]{1,6}", kind(), 1..5).prop_flat_map(
        move |schema: BTreeMap<String, Kind>| {
            let columns: Vec<(String, Kind)> = schema.into_iter().collect();
            let row = columns
                .iter()
                .map(|(name, k)| {
                    let name = name.clone();
                    cell_of(*k).prop_map(move |v| (name.clone(), v))
                })
                .collect::<Vec<_>>();
            let type_name = type_name.clone();
            proptest::collection::vec(row, 0..4).prop_map(move |rows| TypedRowSet {
                type_name: type_name.clone(),
                owner_column: "owner".to_string(),
                owner_value: "vault/file".to_string(),
                rows: rows
                    .into_iter()
                    .map(|cells| {
                        cells
                            .into_iter()
                            .filter_map(|(name, value)| {
                                value.map(|v| (Arc::from(name.as_str()), v))
                            })
                            .collect::<StorageEntity>()
                    })
                    .collect(),
            })
        },
    )
}

fn row_sets() -> impl Strategy<Value = Vec<TypedRowSet>> {
    proptest::collection::vec(0usize..8, 1..4).prop_flat_map(|seeds| {
        // Distinct type names: two scopes sharing one would make a row line's
        // `type` ambiguous, which the contract refuses outright.
        seeds
            .into_iter()
            .enumerate()
            .map(|(i, s)| scope(format!("t{i}_{s}")))
            .collect::<Vec<_>>()
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Emit → parse is the identity, including on scopes with no rows.
    #[test]
    fn a_row_stream_round_trips(sets in row_sets()) {
        let text = emit_row_sets(&sets).expect("legal row sets must emit");
        let back = parse_row_sets(&text).expect("our own output must parse");
        prop_assert_eq!(back, sets);
    }

    /// Every line is one self-contained JSON object, so a consumer can stream
    /// it into a `jaq` filter line by line.
    #[test]
    fn every_line_is_one_json_object(sets in row_sets()) {
        let text = emit_row_sets(&sets).expect("legal row sets must emit");
        prop_assert!(text.ends_with('\n'));
        for line in text.lines() {
            let parsed: serde_json::Value = serde_json::from_str(line)
                .map_err(|e| TestCaseError::fail(format!("line {line:?}: {e}")))?;
            prop_assert!(parsed.is_object());
        }
    }
}

fn one_row_scope(type_name: &str, row: StorageEntity) -> Vec<TypedRowSet> {
    vec![TypedRowSet {
        type_name: type_name.to_string(),
        owner_column: "owner".to_string(),
        owner_value: "vault/file".to_string(),
        rows: vec![row],
    }]
}

#[test]
fn an_empty_scope_survives_the_round_trip() {
    let sets = vec![TypedRowSet {
        type_name: "ingredient_use".to_string(),
        owner_column: "recipe_id".to_string(),
        owner_value: "recipe:Rezepte/Brot.cook".to_string(),
        rows: Vec::new(),
    }];
    let text = emit_row_sets(&sets).unwrap();
    assert_eq!(parse_row_sets(&text).unwrap(), sets);
}

#[test]
fn a_null_column_stays_null_and_never_becomes_a_zero() {
    let mut row = StorageEntity::new();
    row.insert("id".into(), Value::String("iu-0".into()));
    row.insert("quantity".into(), Value::Null);
    let sets = one_row_scope("ingredient_use", row);
    let text = emit_row_sets(&sets).unwrap();
    assert!(text.contains("\"quantity\":null"), "wire form: {text}");
    assert_eq!(parse_row_sets(&text).unwrap(), sets);
}

#[test]
fn a_datetime_column_does_not_decay_into_a_string() {
    let mut row = StorageEntity::new();
    row.insert("id".into(), Value::String("e-0".into()));
    row.insert(
        "due".into(),
        Value::DateTime("2026-09-03T10:00:00Z".to_string()),
    );
    let sets = one_row_scope("event", row);
    let text = emit_row_sets(&sets).unwrap();
    assert!(
        text.contains("\"due\":\"date_time\""),
        "the envelope must name the column's kind: {text}"
    );
    assert_eq!(parse_row_sets(&text).unwrap(), sets);
}

#[test]
fn a_json_column_does_not_decay_into_a_string() {
    let mut row = StorageEntity::new();
    row.insert("id".into(), Value::String("e-0".into()));
    row.insert("payload".into(), Value::Json("{\"a\":1}".to_string()));
    let sets = one_row_scope("event", row);
    let text = emit_row_sets(&sets).unwrap();
    assert_eq!(parse_row_sets(&text).unwrap(), sets);
}

/// The `Json` variant carries a document, not its spelling — the same
/// contract `PropertyKinds::retype` gives the SQL leg.
#[test]
fn a_json_column_comes_back_canonically_spelled() {
    let mut row = StorageEntity::new();
    row.insert("payload".into(), Value::Json("{\"a\":   1}".to_string()));
    let sets = one_row_scope("event", row);
    let back = parse_row_sets(&emit_row_sets(&sets).unwrap()).unwrap();
    assert_eq!(
        back[0].rows[0].get("payload"),
        Some(&Value::Json("{\"a\":1}".to_string()))
    );
}

/// `null` is a JSON document, and a column the envelope types as `json` holds
/// documents — so the kind decides what the cell means, not its nullness.
#[test]
fn a_json_column_holding_the_document_null_does_not_decay_into_a_null() {
    let mut row = StorageEntity::new();
    row.insert("payload".into(), Value::Json("null".to_string()));
    let sets = one_row_scope("event", row);
    let text = emit_row_sets(&sets).unwrap();
    assert_eq!(parse_row_sets(&text).unwrap(), sets);
}

#[test]
fn a_json_column_carries_every_scalar_document_unchanged() {
    for doc in ["false", "0", "\"s\"", "[]", "1.5", "null"] {
        let mut row = StorageEntity::new();
        row.insert("payload".into(), Value::Json(doc.to_string()));
        let sets = one_row_scope("event", row);
        let back = parse_row_sets(&emit_row_sets(&sets).unwrap()).unwrap();
        assert_eq!(back, sets, "document {doc}");
    }
}

/// Both reach the wire as `null`, so one scope cannot state both and still be
/// read back as what it stated.
#[test]
fn a_null_beside_a_json_document_in_one_column_is_refused_naming_the_column() {
    let mut first = StorageEntity::new();
    first.insert("payload".into(), Value::Json("{\"a\":1}".to_string()));
    let mut second = StorageEntity::new();
    second.insert("payload".into(), Value::Null);
    let sets = vec![TypedRowSet {
        type_name: "event".to_string(),
        owner_column: "owner".to_string(),
        owner_value: "cal".to_string(),
        rows: vec![first, second],
    }];
    let error = format!("{:#}", emit_row_sets(&sets).unwrap_err());
    assert!(error.contains("payload"), "{error}");
    assert!(error.contains("event"), "{error}");
}

/// A column a row set to NULL and a column it never mentioned are different
/// statements about that row, and both survive the wire.
#[test]
fn a_null_column_is_distinct_from_an_absent_one() {
    let mut stated = StorageEntity::new();
    stated.insert("id".into(), Value::String("iu-0".into()));
    stated.insert("unit".into(), Value::Null);
    let mut absent = StorageEntity::new();
    absent.insert("id".into(), Value::String("iu-1".into()));
    let sets = vec![TypedRowSet {
        type_name: "ingredient_use".to_string(),
        owner_column: "recipe_id".to_string(),
        owner_value: "recipe:a.cook".to_string(),
        rows: vec![stated, absent],
    }];
    let text = emit_row_sets(&sets).unwrap();
    assert!(text.contains("\"unit\":null"), "wire form: {text}");
    let back = parse_row_sets(&text).unwrap();
    assert_eq!(back[0].rows[0].get("unit"), Some(&Value::Null));
    assert_eq!(back[0].rows[1].get("unit"), None);
    assert_eq!(back, sets);
}

/// serde_json's default float parser lands up to 1 ULP from the float that was
/// written; `float_roundtrip` is on for the workspace so a quantity does not
/// come back silently changed.
#[test]
fn a_float_comes_back_to_the_last_bit() {
    let drifter = -57093562.380749084_f64;
    let mut row = StorageEntity::new();
    row.insert("quantity".into(), Value::Float(drifter));
    let sets = one_row_scope("ingredient_use", row);
    let back = parse_row_sets(&emit_row_sets(&sets).unwrap()).unwrap();
    let Some(Value::Float(got)) = back[0].rows[0].get("quantity") else {
        panic!("quantity must come back a float: {back:?}");
    };
    assert_eq!(got.to_bits(), drifter.to_bits(), "got {got:?}");
}

/// `serde_json::Map` keeps the last of two same-named cells, so a producer
/// that stated a column twice would be read as having stated it once, with a
/// value it never unambiguously gave.
#[test]
fn a_duplicated_column_is_an_error_naming_the_line_column_and_position() {
    let stream = concat!(
        "{\"holon_rows\":1,\"scopes\":[{\"type\":\"event\",\"owner_column\":\"owner\",",
        "\"owner_value\":\"cal\"}]}\n",
        "{\"type\":\"event\",\"row\":{\"x\":1,\"id\":\"e-0\",\"x\":2}}\n"
    );
    let error = format!("{:#}", parse_row_sets(stream).unwrap_err());
    assert!(error.contains("line 2"), "{error}");
    assert!(error.contains("\"x\""), "{error}");
    assert!(error.contains('1') && error.contains('3'), "{error}");
}

#[test]
fn a_non_finite_float_is_refused_by_name_rather_than_written_as_null() {
    let mut row = StorageEntity::new();
    row.insert("quantity".into(), Value::Float(f64::NAN));
    let error = format!(
        "{:#}",
        emit_row_sets(&one_row_scope("ingredient_use", row)).unwrap_err()
    );
    assert!(
        error.contains("quantity"),
        "error must name the column: {error}"
    );
    assert!(
        error.contains("ingredient_use"),
        "error must name the type: {error}"
    );
}

#[test]
fn a_json_column_holding_invalid_json_is_refused_rather_than_written_as_null() {
    let mut row = StorageEntity::new();
    row.insert("payload".into(), Value::Json("not json".to_string()));
    let error = format!(
        "{:#}",
        emit_row_sets(&one_row_scope("event", row)).unwrap_err()
    );
    assert!(error.contains("payload"), "{error}");
}

#[test]
fn a_removal_marker_is_refused_because_it_is_a_write_leg_instruction() {
    let mut row = StorageEntity::new();
    row.insert("course".into(), Value::REMOVED);
    let error = format!(
        "{:#}",
        emit_row_sets(&one_row_scope("recipe", row)).unwrap_err()
    );
    assert!(error.contains("course"), "{error}");
}

#[test]
fn two_scopes_sharing_a_type_are_refused_because_a_row_line_could_not_be_routed() {
    let sets = vec![
        TypedRowSet {
            type_name: "recipe".to_string(),
            owner_column: "source_path".to_string(),
            owner_value: "a.cook".to_string(),
            rows: Vec::new(),
        },
        TypedRowSet {
            type_name: "recipe".to_string(),
            owner_column: "source_path".to_string(),
            owner_value: "b.cook".to_string(),
            rows: Vec::new(),
        },
    ];
    let error = format!("{:#}", emit_row_sets(&sets).unwrap_err());
    assert!(error.contains("recipe"), "{error}");
}

#[test]
fn a_column_two_rows_disagree_about_is_refused_naming_both_kinds() {
    let mut first = StorageEntity::new();
    first.insert("due".into(), Value::DateTime("2026-01-01T00:00:00Z".into()));
    let mut second = StorageEntity::new();
    second.insert("due".into(), Value::String("soon".into()));
    let sets = vec![TypedRowSet {
        type_name: "event".to_string(),
        owner_column: "owner".to_string(),
        owner_value: "cal".to_string(),
        rows: vec![first, second],
    }];
    let error = format!("{:#}", emit_row_sets(&sets).unwrap_err());
    assert!(error.contains("due"), "{error}");
    assert!(error.contains("date_time"), "{error}");
}

#[test]
fn a_blank_line_is_an_error_naming_the_line() {
    let stream = "{\"holon_rows\":1,\"scopes\":[]}\n\n";
    let error = format!("{:#}", parse_row_sets(stream).unwrap_err());
    assert!(error.contains("line 2"), "{error}");
}

#[test]
fn an_undeclared_type_is_an_error_naming_the_line_and_the_type() {
    let stream = "{\"holon_rows\":1,\"scopes\":[]}\n{\"type\":\"recipe\",\"row\":{}}\n";
    let error = format!("{:#}", parse_row_sets(stream).unwrap_err());
    assert!(error.contains("line 2"), "{error}");
    assert!(error.contains("recipe"), "{error}");
}

#[test]
fn a_value_that_cannot_inhabit_its_declared_kind_is_an_error_naming_line_and_column() {
    let stream = concat!(
        "{\"holon_rows\":1,\"scopes\":[{\"type\":\"event\",\"owner_column\":\"owner\",",
        "\"owner_value\":\"cal\",\"kinds\":{\"due\":\"date_time\"}}]}\n",
        "{\"type\":\"event\",\"row\":{\"due\":7}}\n"
    );
    let error = format!("{:#}", parse_row_sets(stream).unwrap_err());
    assert!(error.contains("line 2"), "{error}");
    assert!(error.contains("due"), "{error}");
}

/// A stream a `jaq` filter wrote carries no kind map, and every column then
/// means exactly what JSON says — no kind is invented for it.
#[test]
fn an_envelope_without_a_kind_map_reads_every_column_as_json_spells_it() {
    let stream = concat!(
        "{\"holon_rows\":1,\"scopes\":[{\"type\":\"event\",\"owner_column\":\"owner\",",
        "\"owner_value\":\"cal\"}]}\n",
        "{\"type\":\"event\",\"row\":{\"due\":\"2026-09-03T10:00:00Z\",\"n\":3,\"f\":3.5}}\n"
    );
    let sets = parse_row_sets(stream).unwrap();
    let row = &sets[0].rows[0];
    assert_eq!(
        row.get("due"),
        Some(&Value::String("2026-09-03T10:00:00Z".into()))
    );
    assert_eq!(row.get("n"), Some(&Value::Integer(3)));
    assert_eq!(row.get("f"), Some(&Value::Float(3.5)));
}

#[test]
fn a_stream_from_a_future_contract_version_is_refused_rather_than_guessed_at() {
    let stream = "{\"holon_rows\":2,\"scopes\":[]}\n";
    let error = format!("{:#}", parse_row_sets(stream).unwrap_err());
    assert!(error.contains('2'), "{error}");
}
