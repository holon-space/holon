//! Property: the Transit-JSON reader round-trips.
//!
//! For any generated [`TransitNode`], encoding it to Transit-JSON (test-local
//! encoder) and decoding it back with the crate's [`decode`] yields the same
//! node. This is the structural guard on the decoder. The `^N` write-cache
//! back-reference — the #1 silent-corruption hazard — is exercised separately:
//! this naive encoder never emits `^N`, so cache-ref resolution is proven by
//! the real-fixture identity gate (`holontest_import.rs`) plus targeted `^N`
//! unit tests added with the real decoder (increment 1).
//!
//! Red-first (holon-feature): against the increment-0 `decode` stub this
//! failed on the value comparison (`Nil != Bool(false)`) — a wrong decode, not
//! a missing symbol. Green from increment 1.

use holon_logseq_db::F64Bits;
use holon_logseq_db::TransitNode;
use holon_logseq_db::decode;
use holon_logseq_db::encode_document;
use proptest::prelude::*;

fn json_str(s: &str) -> String {
    serde_json::to_string(s).expect("json-encode string")
}

/// Encode a node to the Transit-JSON subset LogSeq emits (no `^N` caching).
fn encode(n: &TransitNode) -> String {
    match n {
        TransitNode::Nil => "null".to_string(),
        TransitNode::Bool(b) => b.to_string(),
        TransitNode::Int(i) => json_str(&format!("~i{i}")),
        TransitNode::Float(f) => json_str(&format!("~d{}", f.get())),
        TransitNode::Str(s) => json_str(s),
        TransitNode::Keyword(k) => json_str(&format!("~:{k}")),
        TransitNode::Symbol(s) => json_str(&format!("~${s}")),
        TransitNode::Uuid(u) => json_str(&format!("~u{u}")),
        TransitNode::Instant(t) => json_str(&format!("~t{t}")),
        TransitNode::InstantMillis(t) => json_str(&format!("~m{t}")),
        TransitNode::List(xs) => {
            let inner: Vec<String> = xs.iter().map(encode).collect();
            format!("[{}]", inner.join(","))
        }
        TransitNode::Map(kvs) => {
            let mut parts = vec![json_str("^ ")];
            for (k, v) in kvs {
                parts.push(encode(k));
                parts.push(encode(v));
            }
            format!("[{}]", parts.join(","))
        }
        TransitNode::Tagged(tag, inner) => {
            format!("[{},{}]", json_str(&format!("~#{tag}")), encode(inner))
        }
    }
}

fn node_strategy() -> impl Strategy<Value = TransitNode> {
    let leaf = prop_oneof![
        Just(TransitNode::Nil),
        any::<bool>().prop_map(TransitNode::Bool),
        any::<i64>().prop_map(TransitNode::Int),
        any::<f64>()
            .prop_filter("finite", |f| f.is_finite())
            .prop_map(|f| TransitNode::Float(F64Bits::new(f))),
        "[a-z0-9]{0,8}".prop_map(TransitNode::Str),
        "[a-z][a-z0-9./-]{0,8}".prop_map(TransitNode::Keyword),
        "[a-z][a-z0-9./-]{0,8}".prop_map(TransitNode::Symbol),
        "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}".prop_map(TransitNode::Uuid),
        "[0-9]{10,13}".prop_map(TransitNode::Instant),
        "[0-9]{10,13}".prop_map(TransitNode::InstantMillis),
    ];
    leaf.prop_recursive(4, 32, 5, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..5).prop_map(TransitNode::List),
            prop::collection::vec((inner.clone(), inner.clone()), 0..4).prop_map(TransitNode::Map),
            ("[a-z]{1,6}", inner).prop_map(|(t, x)| TransitNode::Tagged(t, Box::new(x))),
        ]
    })
}

proptest! {
    #[test]
    fn transit_decode_round_trips(node in node_strategy()) {
        let encoded = encode(&node);
        let decoded = decode(&encoded).expect("decode well-formed Transit");
        prop_assert_eq!(decoded, node, "round-trip mismatch for: {}", encoded);
    }
}

proptest! {
    /// The property W0 rests on: the REAL encoder, cache and all, is the
    /// inverse of the reader.
    ///
    /// Stronger than the fixture can be. The `^N` write cache only misbehaves
    /// when a cacheable string REPEATS, and generated trees repeat keywords
    /// far more densely than 456 hand-grown rows do — an off-by-one in cache
    /// indexing resolves a back-reference to the wrong earlier value, which
    /// still parses and is therefore invisible to anything but this equality.
    #[test]
    fn transit_encode_round_trips_through_the_write_cache(node in node_strategy()) {
        let encoded = encode_document(&node);
        let decoded = decode(&encoded).expect("the encoder emits well-formed Transit");
        prop_assert_eq!(decoded, node, "round-trip mismatch for: {}", encoded);
    }
}
