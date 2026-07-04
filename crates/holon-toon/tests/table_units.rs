//! Concrete examples for the generic tabular codec: the golden rendering, the
//! load-bearing absent-vs-empty distinction, typed round-trips, and fail-loud
//! error paths.

use std::collections::BTreeMap;

use holon_toon::Table;
use holon_toon::ToonError;
use holon_toon::ToonValue;

fn row(pairs: &[(&str, ToonValue)]) -> BTreeMap<String, ToonValue> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

#[test]
fn golden_uniform_table() {
    let rows = vec![
        row(&[
            ("id", ToonValue::Int(1)),
            ("name", ToonValue::Str("alice".into())),
            ("done", ToonValue::Bool(true)),
        ]),
        row(&[
            ("id", ToonValue::Int(2)),
            ("name", ToonValue::Str("carol, jr.".into())),
            ("done", ToonValue::Bool(false)),
        ]),
    ];
    let table = Table::from_rows("rows", rows).unwrap();
    let rendered = table.render().unwrap();
    // Columns are the SORTED union: done,id,name.
    assert_eq!(
        rendered,
        "rows[2]{done,id,name}:\n\
         \x20\x20true,1,alice\n\
         \x20\x20false,2,\"carol, jr.\"\n"
    );
    assert_eq!(Table::parse(&rendered).unwrap(), table);
}

#[test]
fn absent_is_distinct_from_empty_string_and_null() {
    // Three rows over the same column, exercising the three distinct states.
    let rows = vec![
        row(&[("v", ToonValue::Str(String::new()))]), // empty string
        row(&[("v", ToonValue::Null)]),               // explicit null
        BTreeMap::new(),                              // absent
    ];
    let table = Table::from_rows("rows", rows).unwrap();
    let rendered = table.render().unwrap();
    assert_eq!(
        rendered,
        "rows[3]{v}:\n\
         \x20\x20\"\"\n\
         \x20\x20null\n\
         \x20\x20\n"
    );
    let parsed = Table::parse(&rendered).unwrap();
    assert_eq!(
        parsed.rows[0].get("v"),
        Some(&ToonValue::Str(String::new()))
    );
    assert_eq!(parsed.rows[1].get("v"), Some(&ToonValue::Null));
    assert_eq!(parsed.rows[2].get("v"), None, "absent key must not appear");
    assert_eq!(parsed, table);
}

#[test]
fn string_lookalikes_of_bare_literals_roundtrip() {
    // Strings that would be mistaken for bare literals must be quoted so they
    // decode back to Str, never Int/Bool/Null.
    for s in [
        "null", "true", "false", "42", "-3", "1.5", "1.2.3", "1e", "1-2",
    ] {
        let table =
            Table::from_rows("rows", vec![row(&[("c", ToonValue::Str(s.into()))])]).unwrap();
        let rendered = table.render().unwrap();
        let parsed = Table::parse(&rendered).unwrap();
        assert_eq!(
            parsed.rows[0].get("c"),
            Some(&ToonValue::Str(s.into())),
            "string {s:?} must round-trip as Str, rendered:\n{rendered}"
        );
    }
}

#[test]
fn typed_scalars_roundtrip() {
    let table = Table::from_rows(
        "rows",
        vec![row(&[
            ("i", ToonValue::Int(-9_007)),
            ("f", ToonValue::Float(1.0)), // whole float keeps its `.0`
            ("g", ToonValue::Float(-2.5)),
            ("b", ToonValue::Bool(false)),
            ("n", ToonValue::Null),
        ])],
    )
    .unwrap();
    let parsed = Table::parse(&table.render().unwrap()).unwrap();
    assert_eq!(parsed, table);
}

#[test]
fn awkward_column_names_roundtrip() {
    let table = Table::from_rows(
        "rows",
        vec![row(&[
            ("a,b:c", ToonValue::Int(1)),
            ("with \"quote\"", ToonValue::Int(2)),
            ("", ToonValue::Int(3)),     // empty column name → "" in header
            ("null", ToonValue::Int(4)), // literal-lookalike column name
        ])],
    )
    .unwrap();
    let parsed = Table::parse(&table.render().unwrap()).unwrap();
    assert_eq!(parsed, table);
}

#[test]
fn empty_table_roundtrips() {
    let table = Table::from_rows("rows", vec![]).unwrap();
    assert_eq!(table.render().unwrap(), "rows[0]{}:\n");
    assert_eq!(Table::parse("rows[0]{}:\n").unwrap(), table);
}

#[test]
fn zero_column_rows_roundtrip() {
    // Rows that are all empty maps → zero columns, N non-zero.
    let table = Table::from_rows("rows", vec![BTreeMap::new(), BTreeMap::new()]).unwrap();
    let rendered = table.render().unwrap();
    assert_eq!(rendered, "rows[2]{}:\n  \n  \n");
    assert_eq!(Table::parse(&rendered).unwrap(), table);
}

#[test]
fn newline_in_value_is_escaped_not_structural() {
    let table = Table::from_rows(
        "rows",
        vec![row(&[("body", ToonValue::Str("line1\nline2".into()))])],
    )
    .unwrap();
    let rendered = table.render().unwrap();
    assert!(rendered.contains("\\n"));
    assert!(!rendered.contains("line1\nline2")); // no real newline leaked
    assert_eq!(Table::parse(&rendered).unwrap(), table);
}

#[test]
fn non_finite_float_fails_loud() {
    let table =
        Table::from_rows("rows", vec![row(&[("f", ToonValue::Float(f64::INFINITY))])]).unwrap();
    let err = table.render().unwrap_err();
    assert!(
        matches!(err, ToonError::NonFiniteFloat { .. }),
        "got {err:?}"
    );
}

#[test]
fn bad_table_name_fails_loud() {
    let err = Table::from_rows("bad name", vec![]).unwrap_err();
    assert!(matches!(err, ToonError::BadTableName { .. }), "got {err:?}");
}

#[test]
fn row_count_mismatch_fails_loud() {
    let err = Table::parse("rows[3]{c}:\n  1\n").unwrap_err();
    assert!(
        matches!(err, ToonError::RowCountMismatch { .. }),
        "got {err:?}"
    );
}

#[test]
fn cell_count_mismatch_fails_loud() {
    let err = Table::parse("rows[1]{a,b}:\n  1\n").unwrap_err();
    assert!(
        matches!(err, ToonError::CellCountMismatch { .. }),
        "got {err:?}"
    );
}

#[test]
fn bad_header_fails_loud() {
    let err = Table::parse("not a header\n").unwrap_err();
    assert!(
        matches!(err, ToonError::BadTableHeader { .. }),
        "got {err:?}"
    );
}

#[cfg(feature = "serde-json")]
#[test]
fn nested_json_becomes_a_string_cell() {
    use serde_json::json;
    let v = json!({"tags": ["a", "b"], "n": 3});
    let tv = ToonValue::from_json(&v).unwrap();
    // Nested object → compact JSON string.
    assert_eq!(tv, ToonValue::Str(serde_json::to_string(&v).unwrap()));

    let table = Table::from_rows(
        "rows",
        vec![row(&[
            ("id", ToonValue::from_json(&json!(7)).unwrap()),
            ("meta", tv),
        ])],
    )
    .unwrap();
    let parsed = Table::parse(&table.render().unwrap()).unwrap();
    assert_eq!(parsed, table);
    // The JSON survives verbatim in the cell.
    if let Some(ToonValue::Str(s)) = parsed.rows[0].get("meta") {
        assert_eq!(serde_json::from_str::<serde_json::Value>(s).unwrap(), v);
    } else {
        panic!("meta cell should be a Str");
    }
}
