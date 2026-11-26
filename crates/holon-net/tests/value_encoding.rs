//! The value encoder the executor hashes mark keys and ask hashes with
//! (`holon_net::exec::encode_value`, passed to the executor as a host
//! function).

use holon_api::Value;
use holon_net::exec::encode_value;
use proptest::prelude::*;

use crate::exec_support::values::equal_twin;
use crate::exec_support::values::value;

fn has_nan(v: &Value) -> bool {
    match v {
        Value::Float(f) => f.is_nan(),
        Value::Array(items) => items.iter().any(has_nan),
        Value::Object(map) => map.values().any(has_nan),
        _ => false,
    }
}

fn concatenated(values: &[Value]) -> Vec<u8> {
    values.iter().flat_map(encode_value).collect()
}

/// A list near `a`: one value replaced, two neighbours folded into an array,
/// or any other list. Independent draws almost never encode alike.
fn near(a: Vec<Value>) -> impl Strategy<Value = Vec<Value>> {
    let len = a.len();
    let replaced = {
        let a = a.clone();
        (0..len.max(1), value()).prop_map(move |(i, v)| {
            let mut b = a.clone();
            if i < b.len() {
                b[i] = v;
            }
            b
        })
    };
    let folded = {
        let a = a.clone();
        (0..len.max(1)).prop_map(move |i| {
            let mut b = a.clone();
            if i + 1 < b.len() {
                let pair = vec![b.remove(i), b.remove(i)];
                b.insert(i, Value::Array(pair));
            }
            b
        })
    };
    prop_oneof![replaced, folded, prop::collection::vec(value(), 0..4)]
}

proptest! {
    /// Two equal values give the same mark key: the encoder law.
    #[test]
    fn equal_values_encode_equal(a in value()) {
        let b = equal_twin(&a);
        prop_assert_eq!(&a, &b);
        prop_assert_eq!(encode_value(&a), encode_value(&b));
    }

    /// Fields are encoded back to back, so a value's bytes must say where it
    /// ends: different value lists never encode alike.
    #[test]
    fn encodings_are_self_delimiting(
        (a, b) in prop::collection::vec(value(), 0..4)
            .prop_flat_map(|a| (Just(a.clone()), near(a)))
    ) {
        prop_assume!(!a.iter().chain(&b).any(has_nan));
        if concatenated(&a) == concatenated(&b) {
            prop_assert_eq!(a, b);
        }
    }
}
