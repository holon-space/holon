//! The properties boundary had SIX independent JSON→`Value` converters and
//! they disagreed. This pins the single parser that replaced them, and pins
//! what the properties blob actually round-trips.
//!
//! Agreement is the real contract, not a stylistic preference: the same stored
//! blob is read back by the SQL merge leg, by MCP, and by the certification
//! harness. A kind that survives one leg and is stringified by another means
//! the value a caller sees depends on which door it came through — and the
//! capability profile can only declare ONE answer for `property_values.types`.

use holon_api::Value;
use proptest::prelude::*;

use super::value_to_json;

/// JSON values covering every kind the blob can hold, nested two deep. `u64`
/// beyond `i64::MAX` is drawn deliberately: it is the one numeric input the six
/// converters answered four different ways.
fn any_json() -> impl Strategy<Value = serde_json::Value> {
    let leaf = prop_oneof![
        Just(serde_json::Value::Null),
        any::<bool>().prop_map(serde_json::Value::Bool),
        any::<i64>().prop_map(|i| serde_json::json!(i)),
        (i64::MAX as u64 + 1..=u64::MAX).prop_map(|u| serde_json::json!(u)),
        any::<f64>()
            .prop_filter("JSON has no NaN/Inf", |f| f.is_finite())
            .prop_map(|f| serde_json::json!(f)),
        ".{0,12}".prop_map(serde_json::Value::String),
    ];
    leaf.prop_recursive(3, 12, 3, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..3).prop_map(serde_json::Value::Array),
            prop::collection::hash_map("[a-z]{1,4}", inner, 0..3)
                .prop_map(|m| serde_json::Value::Object(m.into_iter().collect())),
        ]
    })
}

proptest! {
    /// Every JSON shape reaches a `Value` that carries it back unchanged.
    ///
    /// This is the property the six converters violated: each stringified some
    /// arm, so `json → Value → json` lost the kind for arrays, objects, or
    /// both. It holds only because the parser is TOTAL over `serde_json::Value`
    /// — there is no `_ =>` arm left to hide a shape in.
    #[test]
    fn the_parser_carries_every_json_shape_back_unchanged(v in any_json()) {
        let parsed = Value::from_json_value(v.clone());
        let back = value_to_json(&parsed);
        prop_assert_eq!(&back, &v, "properties blob re-typed {} on the way through", v);
    }
}

/// An integer too large for `i64` keeps its DIGITS, and pays for that with a
/// changed SQL literal.
///
/// Base rounded it into an `f64` — `18446744073709551615` reached SQL as the
/// bare literal `18446744073709552000`, digits silently gone. It is now carried
/// exactly, as `Value::Json`, which `value_to_sql_literal` emits QUOTED. Both
/// halves are pinned here because the second half is a real, reachable
/// behaviour change: MCP tool args and worker params both become op params
/// (`frontends/mcp/src/tools.rs:741`,
/// `frontends/holon-worker/src/lib.rs:1595`), and a column-bound param is
/// rendered by `value_to_sql_literal`.
///
/// The trade is deliberate, per the error philosophy's priority order: a quoted
/// literal in a numeric column is visibly odd, a silently-rounded number is
/// not.
#[test]
fn an_integer_too_large_for_i64_keeps_its_digits() {
    let huge = serde_json::json!(18446744073709551615u64);
    let parsed = Value::from_json_value(huge.clone());

    assert_eq!(
        parsed,
        Value::Json("18446744073709551615".into()),
        "a u64 beyond i64::MAX must be carried exactly, not rounded into an f64"
    );
    assert_eq!(value_to_json(&parsed), huge, "and it must round-trip");
    assert_eq!(
        holon_turso::sql_utils::value_to_sql_literal(&parsed),
        "'18446744073709551615'",
        "the exact carrier is rendered as a QUOTED literal — the disclosed cost of not rounding"
    );
}

/// What the blob does to each `Value` kind, stated once so a change is visible.
///
/// The last two rows are the losses the `holon-native` capability profile
/// DECLARES (`property_values.types` omits `date_time` and `json`). They are
/// pinned here rather than left implicit so that fixing them has to come
/// through this table — an increment that adds a typed envelope reds here
/// first, which is exactly where the reader should find out.
#[test]
fn each_value_kind_reaches_a_known_json_shape() {
    let cases: Vec<(Value, serde_json::Value)> = vec![
        (Value::Null, serde_json::json!(null)),
        (Value::Boolean(true), serde_json::json!(true)),
        (Value::Integer(7), serde_json::json!(7)),
        (Value::Float(1.5), serde_json::json!(1.5)),
        (Value::String("t".into()), serde_json::json!("t")),
        (
            Value::Array(vec![Value::Integer(1), Value::Integer(2)]),
            serde_json::json!([1, 2]),
        ),
        (
            Value::Object([("a".to_string(), Value::Integer(1))].into_iter().collect()),
            serde_json::json!({"a": 1}),
        ),
        // DECLARED LOSS: the kind is dropped, the text survives.
        (
            Value::DateTime("2026-08-22T10:00:00Z".into()),
            serde_json::json!("2026-08-22T10:00:00Z"),
        ),
        // DECLARED LOSS: the document is parsed into the blob, not carried
        // opaquely.
        (
            Value::Json(r#"{"a":1}"#.into()),
            serde_json::json!({"a": 1}),
        ),
    ];
    for (value, expected) in cases {
        assert_eq!(
            value_to_json(&value),
            expected,
            "properties blob changed what it stores for {value:?}"
        );
    }
}
