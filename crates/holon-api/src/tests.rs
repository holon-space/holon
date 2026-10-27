use super::*;

#[test]
fn test_value_accessors() {
    let v = Value::Boolean(true);
    assert_eq!(v.as_bool(), Some(true));
    assert_eq!(v.as_i64(), None);

    let v = Value::Integer(42);
    assert_eq!(v.as_i64(), Some(42));
    assert_eq!(v.as_f64(), Some(42.0));

    let v = Value::String("hello".to_string());
    assert_eq!(v.as_string(), Some("hello"));

    let v = Value::Null;
    assert!(v.is_null());
}

/// Wire-format lockdown: `Value` is `#[serde(untagged)]`, so plain JSON
/// primitives round-trip without discriminator tags. If this ever breaks
/// — e.g. someone adds `#[serde(tag = "type")]` — every frontend that
/// deserializes operation params or query rows starts failing. Lock it
/// in with concrete JSON so the test fails at the tag site, not later
/// in an unrelated frontend.
#[test]
fn value_serde_wire_format_is_untagged() {
    // Primitives: serialize as bare JSON, no wrapping object.
    assert_eq!(
        serde_json::to_string(&Value::String("hi".into())).unwrap(),
        r#""hi""#,
    );
    assert_eq!(serde_json::to_string(&Value::Integer(42)).unwrap(), "42",);
    assert_eq!(serde_json::to_string(&Value::Float(3.5)).unwrap(), "3.5",);
    assert_eq!(
        serde_json::to_string(&Value::Boolean(true)).unwrap(),
        "true",
    );
    assert_eq!(serde_json::to_string(&Value::Null).unwrap(), "null");

    // Bare primitives deserialize into the expected variant.
    assert_eq!(
        serde_json::from_str::<Value>(r#""hi""#).unwrap(),
        Value::String("hi".into()),
    );
    assert_eq!(
        serde_json::from_str::<Value>("42").unwrap(),
        Value::Integer(42),
    );
    assert_eq!(
        serde_json::from_str::<Value>("3.5").unwrap(),
        Value::Float(3.5),
    );
    assert_eq!(
        serde_json::from_str::<Value>("true").unwrap(),
        Value::Boolean(true),
    );
    assert_eq!(serde_json::from_str::<Value>("null").unwrap(), Value::Null);

    // Critically: there is NO `{"Text": {"value": "..."}}` or
    // `{"String": "..."}` tagged representation. Any frontend that
    // probes for such a shape is working from a wrong mental model.
    assert!(
        !serde_json::to_string(&Value::String("hi".into()))
            .unwrap()
            .contains("Text"),
    );
    assert!(
        !serde_json::to_string(&Value::String("hi".into()))
            .unwrap()
            .contains("String"),
    );
}

/// Operation params come in as a flat JSON object from frontends:
/// `{"id": "block:foo", "content": "hello"}`. Each value must
/// deserialize into the matching `Value` variant without any tagging.
#[test]
fn value_map_params_round_trip_untagged() {
    use std::collections::HashMap;
    let js_input = r#"{
            "id": "block:foo",
            "content": "hello world",
            "priority": 3,
            "done": true,
            "ratio": 0.5,
            "deleted": null
        }"#;
    let parsed: HashMap<String, Value> = serde_json::from_str(js_input).expect("parse params_json");
    assert_eq!(parsed["id"], Value::String("block:foo".into()));
    assert_eq!(parsed["content"], Value::String("hello world".into()));
    assert_eq!(parsed["priority"], Value::Integer(3));
    assert_eq!(parsed["done"], Value::Boolean(true));
    assert_eq!(parsed["ratio"], Value::Float(0.5));
    assert_eq!(parsed["deleted"], Value::Null);
}

#[test]
fn test_value_from() {
    let v: Value = true.into();
    assert_eq!(v, Value::Boolean(true));

    let v: Value = 42i64.into();
    assert_eq!(v, Value::Integer(42));

    let v: Value = "test".into();
    assert_eq!(v, Value::String("test".to_string()));

    let v: Value = None::<i64>.into();
    assert_eq!(v, Value::Null);

    let v: Value = Some(42).into();
    assert_eq!(v, Value::Integer(42));
}

#[test]
fn test_value_json() {
    let v = Value::Object(
        vec![
            ("name".to_string(), Value::String("test".to_string())),
            ("count".to_string(), Value::Integer(5)),
        ]
        .into_iter()
        .collect(),
    );

    let json = v.to_json_string().expect("finite JSON");
    let parsed = Value::from_json_str(&json).unwrap();
    assert_eq!(v, parsed);
}

#[test]
fn test_value_array() {
    let arr = vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)];
    let v = Value::Array(arr.clone());
    assert_eq!(v.as_array(), Some(&arr));
}

/// Behavioral lockdown for the `Value` accessors that mutation testing
/// found unguarded. Each assertion pins the exact returned payload so a
/// replacement with `None`, `Some(Default)`, a stubbed literal, or a
/// deleted match arm is caught.
#[test]
fn value_accessor_payloads_are_exact() {
    // as_json_value: real parse on the Json variant, None otherwise.
    let jv = Value::Json(r#"{"a":1}"#.to_string());
    assert_eq!(
        jv.as_json_value(),
        Some(serde_json::json!({"a": 1})),
        "must actually parse the Json string, not None/Default",
    );
    assert_eq!(Value::Integer(1).as_json_value(), None);

    // as_string_owned: clones the actual string, None otherwise.
    assert_eq!(
        Value::String("hello".to_string()).as_string_owned(),
        Some("hello".to_string()),
    );
    assert_eq!(Value::Integer(1).as_string_owned(), None);

    // as_datetime_string: returns the stored RFC3339 text verbatim.
    let dt_str = "2020-01-02T03:04:05+00:00";
    assert_eq!(
        Value::DateTime(dt_str.to_string()).as_datetime_string(),
        Some(dt_str),
    );
    assert_eq!(Value::Integer(1).as_datetime_string(), None);

    // as_datetime: parses to the corresponding chrono instant.
    let expected = chrono::DateTime::parse_from_rfc3339(dt_str)
        .unwrap()
        .with_timezone(&chrono::Utc);
    assert_eq!(
        Value::DateTime(dt_str.to_string()).as_datetime(),
        Some(expected),
    );
    assert_eq!(Value::Integer(1).as_datetime(), None);

    // as_object: returns the real map (non-empty), None otherwise.
    let mut m = std::collections::HashMap::new();
    m.insert("k".to_string(), Value::Integer(7));
    let obj = Value::Object(m.clone());
    assert_eq!(obj.as_object(), Some(&m));
    assert_eq!(obj.as_object().map(|o| o.len()), Some(1));
    assert_eq!(Value::Integer(1).as_object(), None);
}

/// Behavioral lockdown for the `TryFrom<Value>` conversions flagged by
/// mutation testing. Pins both the success payload and the boolean
/// integer-truthiness comparison.
#[test]
fn tryfrom_value_conversions_are_exact() {
    // bool: Boolean arm and the `!= 0` truthiness of the Integer arm.
    assert!(bool::try_from(Value::Boolean(true)).unwrap());
    assert!(!bool::try_from(Value::Boolean(false)).unwrap());
    assert!(!bool::try_from(Value::Integer(0)).unwrap());
    assert!(bool::try_from(Value::Integer(5)).unwrap());
    assert!(bool::try_from(Value::String("x".into())).is_err());

    // i32: real value, not Default.
    assert_eq!(i32::try_from(Value::Integer(42)).unwrap(), 42);

    // f64: real value, not Default.
    assert_eq!(f64::try_from(Value::Float(3.5)).unwrap(), 3.5);

    // Vec<T>: real contents, not Default (empty).
    let v: Vec<i64> =
        Vec::try_from(Value::Array(vec![Value::Integer(1), Value::Integer(2)])).unwrap();
    assert_eq!(v, vec![1, 2]);
}
