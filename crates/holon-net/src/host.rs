//! Holon's side of the executor's value contract: the token value is
//! [`Value`], and these are the functions the executor reaches it through.

use std::collections::BTreeMap;

use holon_pattern::Value;

use crate::exec::Host;
use crate::exec::NetDef;

pub const HOST: Host<Value> = Host {
    encode: encode_value,
};

/// An empty net over Holon's [`Value`].
pub fn new_net() -> NetDef {
    NetDef::new(HOST)
}

/// The canonical bytes of a value in a mark key and an ask hash. Panics on
/// [`Value::Removed`]: a token never holds a removal write intent.
pub fn encode_value(v: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    encode_into(&mut out, v);
    out
}

fn encode_str_into(h: &mut Vec<u8>, s: &str) {
    h.extend_from_slice(&(s.len() as u64).to_le_bytes());
    h.extend_from_slice(s.as_bytes());
}

fn encode_into(h: &mut Vec<u8>, v: &Value) {
    match v {
        Value::Removed(_) => panic!("a token holds a removal write intent"),
        Value::String(s) => {
            h.extend_from_slice(&[1]);
            encode_str_into(h, s);
        }
        Value::Integer(i) => {
            h.extend_from_slice(&[2]);
            h.extend_from_slice(&i.to_le_bytes());
        }
        Value::Float(f) => {
            h.extend_from_slice(&[3]);
            // `-0.0 == 0.0`, so both encode as `0.0`.
            h.extend_from_slice(&(f + 0.0).to_bits().to_le_bytes());
        }
        Value::Boolean(b) => {
            h.extend_from_slice(&[4, u8::from(*b)]);
        }
        Value::DateTime(s) => {
            h.extend_from_slice(&[5]);
            encode_str_into(h, s);
        }
        Value::Json(s) => {
            h.extend_from_slice(&[6]);
            encode_str_into(h, s);
        }
        Value::Array(items) => {
            h.extend_from_slice(&[7]);
            h.extend_from_slice(&(items.len() as u64).to_le_bytes());
            for item in items {
                encode_into(h, item);
            }
        }
        Value::Object(map) => {
            h.extend_from_slice(&[8]);
            h.extend_from_slice(&(map.len() as u64).to_le_bytes());
            let sorted: BTreeMap<_, _> = map.iter().collect();
            for (k, item) in sorted {
                encode_str_into(h, k);
                encode_into(h, item);
            }
        }
        Value::Null => {
            h.extend_from_slice(&[9]);
        }
        Value::Quantity(cell) => {
            h.extend_from_slice(&[10]);
            encode_str_into(h, cell.format().format().name());
            encode_str_into(h, cell.original());
        }
    }
}

#[cfg(test)]
mod tests {
    use holon_api::FieldFormat;
    use holon_api::QuantityCell;

    use super::*;

    fn hash_of(v: &Value) -> blake3::Hash {
        blake3::hash(&encode_value(v))
    }

    fn count(original: &str) -> Value {
        let format = FieldFormat::named("tsl_count").unwrap();
        Value::Quantity(QuantityCell::parse(format, original.to_string()).unwrap())
    }

    #[test]
    fn quantities_with_different_text_hash_differently() {
        assert_eq!(hash_of(&count("2kg")), hash_of(&count("2kg")));
        assert_ne!(hash_of(&count("2kg")), hash_of(&count("2 kg")));
        assert_ne!(
            hash_of(&count("2kg")),
            hash_of(&Value::String("2kg".into()))
        );
    }
}
