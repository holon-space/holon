//! Value generators shared by every executor PBT, so each property draws the
//! whole value space the executor holds.

use std::collections::HashMap;

use holon_api::FieldFormat;
use holon_api::QuantityCell;
use holon_api::Value;
use proptest::prelude::*;
use proptest::sample::select;

fn quantity() -> impl Strategy<Value = Value> {
    select(vec!["2kg", "2 kg", "1kg", "3"]).prop_map(|text| {
        let format = FieldFormat::named("tsl_count").expect("bundled format");
        Value::Quantity(QuantityCell::parse(format, text.to_owned()).expect("a count text"))
    })
}

pub fn value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Boolean),
        any::<i64>().prop_map(Value::Integer),
        any::<f64>().prop_map(Value::Float),
        select(vec![0.0, -0.0]).prop_map(Value::Float),
        ".{0,6}".prop_map(Value::String),
        ".{0,6}".prop_map(Value::DateTime),
        ".{0,6}".prop_map(Value::Json),
        quantity(),
    ];
    leaf.prop_recursive(2, 6, 3, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..3).prop_map(Value::Array),
            prop::collection::hash_map(".{0,3}", inner, 0..3).prop_map(Value::Object),
        ]
    })
}

/// A value `==` to `v` that is built another way: every object map is
/// rebuilt in reverse insertion order, and every zero float has the other
/// sign.
pub fn equal_twin(v: &Value) -> Value {
    match v {
        Value::Array(items) => Value::Array(items.iter().map(equal_twin).collect()),
        Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.reverse();
            let mut twin = HashMap::with_capacity(map.capacity() + 7);
            for (k, item) in entries {
                twin.insert(k.clone(), equal_twin(item));
            }
            Value::Object(twin)
        }
        Value::Float(f) if *f == 0.0 => Value::Float(-f),
        other => other.clone(),
    }
}
