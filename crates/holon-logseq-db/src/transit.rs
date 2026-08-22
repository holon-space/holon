//! Transit-JSON reader for LogSeq `kvs` nodes.
//!
//! A direct port of the stage-0 spike's Python `_Reader`
//! (`~/.claude/plans/logseq-db-spike-2026-08-20/transit_decode.py`), which was
//! proven byte-exact on the HolonTest fixture. The semantics that must match
//! exactly are the per-document **write cache**: `^0`..`^zz` back-references
//! index a list that grows as cacheable scalars are read, so an off-by-one
//! resolves an attribute to the wrong keyword — silently. That is the #1
//! corruption hazard for this import, hence the port is literal rather than
//! idiomatic where the two disagree.

use std::collections::HashMap;

use serde_json::Value;

use crate::F64Bits;
use crate::TransitNode;

const MAP_MARKER: &str = "^ ";
const ESC: char = '~';
const SUB: char = '^';
const RESERVED: char = '`';
const BASE_CHAR_INDEX: u32 = 48;
const CACHE_CODE_DIGITS: u32 = 44;
const MIN_SIZE_CACHEABLE: usize = 4;

/// Everything the Transit reader can refuse. Every variant is a loud stop:
/// the reader never guesses past malformed input, because a guess here
/// mis-attributes datoms instead of failing.
#[derive(Debug, thiserror::Error)]
pub enum TransitError {
    #[error("document is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error(
        "cache back-reference {code:?} resolves to index {index}, but only {len} entries are cached"
    )]
    CacheMiss {
        code: String,
        index: usize,
        len: usize,
    },
    #[error("unknown Transit ground-type prefix {prefix:?}")]
    UnknownGroundType { prefix: String },
    #[error("malformed Transit integer {text:?}: {source}")]
    BadInt {
        text: String,
        #[source]
        source: std::num::ParseIntError,
    },
    #[error("malformed Transit float {text:?}: {source}")]
    BadFloat {
        text: String,
        #[source]
        source: std::num::ParseFloatError,
    },
    #[error(
        "Transit map has {len} elements after the {MAP_MARKER:?} marker; keys and values must pair up"
    )]
    UnpairedMap { len: usize },
    /// `~b` base64 bytes. No LogSeq datom value is binary (assets live in
    /// files), so a `~b` here means the document is not what we think it is.
    #[error("Transit base64 bytes ({text:?}) are not a LogSeq datom value")]
    BytesUnsupported { text: String },
    /// `~#tag` outside the `["~#tag", value]` pair position — it tags nothing.
    #[error("Transit tag marker {tag:?} appears outside a tagged pair")]
    BareTagMarker { tag: String },
    /// `~n` bigint / `~f` bigdec. Accepting them would silently narrow the
    /// value on the way back out; see the reader's ground-type table.
    #[error(
        "Transit arbitrary-precision value ({text:?}) cannot be re-encoded without \
         narrowing it; this build refuses to read a graph containing one"
    )]
    ArbitraryPrecisionUnsupported { text: String },
}

/// What decoding one JSON string yields. A `~#tag` marker is not a value —
/// it only means something as the head of a `["~#tag", v]` pair — but it *is*
/// cacheable, so the cache stores this wider type, exactly as the Python
/// cache stores the `("tag", name)` tuple.
#[derive(Debug, Clone)]
enum Scalar {
    Node(TransitNode),
    TagMarker(String),
}

impl Scalar {
    fn into_node(self) -> Result<TransitNode, TransitError> {
        match self {
            Scalar::Node(n) => Ok(n),
            Scalar::TagMarker(tag) => Err(TransitError::BareTagMarker { tag }),
        }
    }
}

/// `"^0".."^zz"` — but never the map marker `"^ "`, whose space sorts below
/// `BASE_CHAR_INDEX`.
fn is_cache_code(s: &str) -> bool {
    let mut chars = s.chars();
    let (Some(first), Some(second)) = (chars.next(), chars.next()) else {
        return false;
    };
    first == SUB && s != MAP_MARKER && second as u32 >= BASE_CHAR_INDEX
}

fn code_to_index(code: &str) -> usize {
    let chars: Vec<char> = code.chars().collect();
    let hi = chars[1] as u32 - BASE_CHAR_INDEX;
    if chars.len() == 2 {
        hi as usize
    } else {
        (hi * CACHE_CODE_DIGITS + (chars[2] as u32 - BASE_CHAR_INDEX)) as usize
    }
}

fn is_cacheable(s: &str, as_map_key: bool) -> bool {
    if s.chars().count() < MIN_SIZE_CACHEABLE {
        return false;
    }
    if as_map_key {
        return true;
    }
    let mut chars = s.chars();
    chars.next() == Some(ESC) && matches!(chars.next(), Some(':' | '$' | '#'))
}

/// One document's read state. The cache is per top-level document: LogSeq
/// resets it for every `kvs` row, so a [`Reader`] is used once and dropped.
struct Reader {
    cache: Vec<Scalar>,
}

impl Reader {
    fn new() -> Self {
        Self { cache: Vec::new() }
    }

    fn cache_read(&mut self, s: &str, as_map_key: bool) -> Result<Scalar, TransitError> {
        if is_cache_code(s) {
            let index = code_to_index(s);
            return self
                .cache
                .get(index)
                .cloned()
                .ok_or_else(|| TransitError::CacheMiss {
                    code: s.to_string(),
                    index,
                    len: self.cache.len(),
                });
        }
        let val = decode_string(s)?;
        if is_cacheable(s, as_map_key) {
            self.cache.push(val.clone());
        }
        Ok(val)
    }

    fn decode(&mut self, node: &Value, as_map_key: bool) -> Result<Scalar, TransitError> {
        match node {
            Value::String(s) => self.cache_read(s, as_map_key),
            Value::Array(items) => self.decode_array(items).map(Scalar::Node),
            Value::Null => Ok(Scalar::Node(TransitNode::Nil)),
            Value::Bool(b) => Ok(Scalar::Node(TransitNode::Bool(*b))),
            Value::Number(n) => Ok(Scalar::Node(match n.as_i64() {
                Some(i) => TransitNode::Int(i),
                // A JSON number too wide for i64 is still a number; carry its
                // f64 reading rather than refusing a well-formed document.
                None => TransitNode::Float(F64Bits::new(n.as_f64().expect("json number is f64"))),
            })),
            Value::Object(_) => unreachable!("Transit-JSON encodes maps as arrays, never objects"),
        }
    }

    fn decode_array(&mut self, items: &[Value]) -> Result<TransitNode, TransitError> {
        if items.first().and_then(Value::as_str) == Some(MAP_MARKER) {
            let entries = &items[1..];
            if !entries.len().is_multiple_of(2) {
                return Err(TransitError::UnpairedMap { len: entries.len() });
            }
            let mut pairs = Vec::with_capacity(entries.len() / 2);
            for pair in entries.as_chunks::<2>().0 {
                let key = self.decode(&pair[0], true)?.into_node()?;
                let value = self.decode(&pair[1], false)?.into_node()?;
                pairs.push((key, value));
            }
            return Ok(TransitNode::Map(pairs));
        }

        // A tagged value `["~#tag", v]`. The head may itself be a cache code,
        // in which case it is only a tag if it resolves to one — otherwise
        // this is a plain 2-element array. Reading a cache code appends
        // nothing, so the fall-through re-read below stays side-effect free.
        if let [head, tagged_value] = items {
            if let Some(head_str) = head.as_str() {
                if head_str.starts_with("~#") || is_cache_code(head_str) {
                    if let Scalar::TagMarker(tag) = self.decode(head, false)? {
                        let inner = self.decode(tagged_value, false)?.into_node()?;
                        return Ok(TransitNode::Tagged(tag, Box::new(inner)));
                    }
                }
            }
        }

        let mut out = Vec::with_capacity(items.len());
        for item in items {
            out.push(self.decode(item, false)?.into_node()?);
        }
        Ok(TransitNode::List(out))
    }
}

fn decode_string(s: &str) -> Result<Scalar, TransitError> {
    let mut chars = s.chars();
    if chars.next() != Some(ESC) {
        return Ok(Scalar::Node(TransitNode::Str(s.to_string())));
    }
    let Some(kind) = chars.next() else {
        // A bare "~" is itself.
        return Ok(Scalar::Node(TransitNode::Str(s.to_string())));
    };
    let rest: String = chars.collect();

    let node = match kind {
        // Escaped leading marker: `~~`, `~^`, `~\``.
        ESC | SUB | RESERVED => TransitNode::Str(format!("{kind}{rest}")),
        ':' => TransitNode::Keyword(rest),
        '$' => TransitNode::Symbol(rest),
        'u' => TransitNode::Uuid(rest),
        'i' => TransitNode::Int(rest.parse().map_err(|source| TransitError::BadInt {
            text: s.to_string(),
            source,
        })?),
        'd' => TransitNode::Float(F64Bits::new(rest.parse().map_err(|source| {
            TransitError::BadFloat {
                text: s.to_string(),
                source,
            }
        })?)),
        't' => TransitNode::Instant(rest),
        'm' => TransitNode::InstantMillis(rest),
        // `~n` bigint and `~f` bigdec are arbitrary-precision. Reading them
        // into `i64`/`f64` would round-trip back out as `~i`/`~d`, narrowing
        // the value's type under LogSeq without a word — so they stop here
        // instead, alongside `~b`. No LogSeq DB graph is known to write them.
        'n' | 'f' => {
            return Err(TransitError::ArbitraryPrecisionUnsupported {
                text: s.to_string(),
            });
        }
        '#' => return Ok(Scalar::TagMarker(rest)),
        'b' => {
            return Err(TransitError::BytesUnsupported {
                text: s.to_string(),
            });
        }
        _ => {
            return Err(TransitError::UnknownGroundType {
                prefix: s.chars().take(3).collect(),
            });
        }
    };
    Ok(Scalar::Node(node))
}

/// Decode one Transit-JSON document into a [`TransitNode`].
///
/// The write cache starts empty and dies with the call, matching LogSeq's
/// per-`kvs`-row encoding.
pub fn decode_document(doc: &str) -> Result<TransitNode, TransitError> {
    let value: Value = serde_json::from_str(doc)?;
    Reader::new().decode(&value, false)?.into_node()
}

/// `"^0".."^zz"` — the inverse of [`code_to_index`].
fn index_to_code(index: usize) -> String {
    let digit =
        |v: usize| char::from_u32(BASE_CHAR_INDEX + v as u32).expect("cache digit is ASCII");
    let base = CACHE_CODE_DIGITS as usize;
    if index < base {
        format!("{SUB}{}", digit(index))
    } else {
        format!("{SUB}{}{}", digit(index / base), digit(index % base))
    }
}

/// One document's write state — the mirror image of [`Reader`].
///
/// The cache is the whole difficulty. A back-reference is positional, so the
/// writer must append a string to its cache in exactly the places the reader
/// would append it, or every `^N` after the first divergence resolves to the
/// wrong value. Both sides therefore ask the same [`is_cacheable`] question
/// about the same string in the same position, and nothing else decides.
struct Writer {
    cache: HashMap<String, usize>,
}

impl Writer {
    fn new() -> Self {
        Self {
            cache: HashMap::new(),
        }
    }

    /// Emit one already-encoded Transit string, as a cache code if the reader
    /// will have it cached by the time it gets here.
    fn scalar(&mut self, encoded: String, as_map_key: bool) -> Value {
        if let Some(&index) = self.cache.get(&encoded) {
            return Value::String(index_to_code(index));
        }
        if is_cacheable(&encoded, as_map_key) {
            let next = self.cache.len();
            self.cache.insert(encoded.clone(), next);
        }
        Value::String(encoded)
    }

    fn encode(&mut self, node: &TransitNode, as_map_key: bool) -> Value {
        match node {
            TransitNode::Nil => Value::Null,
            TransitNode::Bool(b) => Value::Bool(*b),
            // Transit stringifies map keys, so the same integer is `32` in
            // value position and `"~i32"` as a key.
            TransitNode::Int(i) => {
                if as_map_key {
                    self.scalar(format!("{ESC}i{i}"), true)
                } else {
                    Value::from(*i)
                }
            }
            // Always the `~d` form, in both positions: a raw JSON `1` would
            // read back as an integer, turning a float into an int silently.
            TransitNode::Float(f) => self.scalar(format!("{ESC}d{}", f.get()), as_map_key),
            // A string whose first character is a Transit marker is escaped by
            // doubling the marker, which is what the reader's `~~`/`~^`/`` ~` ``
            // arm undoes.
            TransitNode::Str(s) => {
                let encoded = match s.chars().next() {
                    Some(ESC | SUB | RESERVED) => format!("{ESC}{s}"),
                    _ => s.clone(),
                };
                self.scalar(encoded, as_map_key)
            }
            TransitNode::Keyword(k) => self.scalar(format!("{ESC}:{k}"), as_map_key),
            TransitNode::Symbol(s) => self.scalar(format!("{ESC}${s}"), as_map_key),
            TransitNode::Uuid(u) => self.scalar(format!("{ESC}u{u}"), as_map_key),
            TransitNode::Instant(t) => self.scalar(format!("{ESC}t{t}"), as_map_key),
            TransitNode::InstantMillis(t) => self.scalar(format!("{ESC}m{t}"), as_map_key),
            TransitNode::List(items) => Value::Array(
                items
                    .iter()
                    .map(|item| self.encode(item, false))
                    .collect::<Vec<_>>(),
            ),
            TransitNode::Map(pairs) => {
                let mut out = Vec::with_capacity(pairs.len() * 2 + 1);
                out.push(Value::String(MAP_MARKER.to_string()));
                for (k, v) in pairs {
                    out.push(self.encode(k, true));
                    out.push(self.encode(v, false));
                }
                Value::Array(out)
            }
            TransitNode::Tagged(tag, inner) => Value::Array(vec![
                self.scalar(format!("{ESC}#{tag}"), false),
                self.encode(inner, false),
            ]),
        }
    }
}

/// Encode one [`TransitNode`] back to a Transit-JSON document.
///
/// `decode_document(&encode_document(x)) == x` for every node this crate can
/// read. The bytes need not match LogSeq's own for the same value — the write
/// cache is emission-order dependent — which is why the round-trip, not
/// byte-equality, is what callers assert.
pub fn encode_document(node: &TransitNode) -> String {
    let value = Writer::new().encode(node, false);
    serde_json::to_string(&value).expect("a serde_json::Value always serializes")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dec(doc: &str) -> TransitNode {
        decode_document(doc).expect("decode")
    }

    #[test]
    fn decodes_scalars_and_escapes() {
        assert_eq!(dec("null"), TransitNode::Nil);
        assert_eq!(dec("true"), TransitNode::Bool(true));
        assert_eq!(dec(r#""~i42""#), TransitNode::Int(42));
        assert_eq!(dec(r#""~d1.5""#), TransitNode::Float(F64Bits::new(1.5)));
        assert_eq!(
            dec(r#""~:block/uuid""#),
            TransitNode::Keyword("block/uuid".into())
        );
        assert_eq!(dec(r#""~$sym""#), TransitNode::Symbol("sym".into()));
        assert_eq!(dec(r#""~uabc""#), TransitNode::Uuid("abc".into()));
        // `~t` and `~m` are the same instant in two ground types, and stay
        // apart so a re-encode cannot hand LogSeq the other one.
        assert_eq!(
            dec(r#""~m1787349600000""#),
            TransitNode::InstantMillis("1787349600000".into())
        );
        assert_eq!(
            dec(r#""~t2026-08-22T09:00:00Z""#),
            TransitNode::Instant("2026-08-22T09:00:00Z".into())
        );
        assert_eq!(dec(r#""plain""#), TransitNode::Str("plain".into()));
        // `~~x` is the escaped literal `~x`, not a ground type.
        assert_eq!(dec(r#""~~x""#), TransitNode::Str("~x".into()));
        assert_eq!(dec(r#""~^x""#), TransitNode::Str("^x".into()));
        assert_eq!(dec(r#""~""#), TransitNode::Str("~".into()));
    }

    #[test]
    fn map_marker_is_not_a_cache_code() {
        assert!(!is_cache_code(MAP_MARKER));
        assert_eq!(
            dec(r#"["^ ","~:a","~i1"]"#),
            TransitNode::Map(vec![(
                TransitNode::Keyword("a".into()),
                TransitNode::Int(1)
            )])
        );
    }

    /// The load-bearing property: a keyword read in a cacheable position is
    /// appended to the write cache, and `^0` resolves back to it.
    #[test]
    fn cache_code_resolves_earlier_keyword() {
        assert_eq!(
            dec(r#"["^ ","~:block/uuid","~i1","^0","~i2"]"#),
            TransitNode::Map(vec![
                (
                    TransitNode::Keyword("block/uuid".into()),
                    TransitNode::Int(1)
                ),
                (
                    TransitNode::Keyword("block/uuid".into()),
                    TransitNode::Int(2)
                ),
            ])
        );
    }

    /// Cache indices count *cacheable reads in order*, so a non-cacheable
    /// value between two keywords must not consume a slot. An off-by-one here
    /// is exactly the silent attribute-misattribution hazard.
    #[test]
    fn short_and_non_cacheable_values_do_not_take_cache_slots() {
        // "~i1" is not cacheable (value position, not ~:/~$/~#) and "ab" is
        // too short, so ^0 is the first keyword and ^1 the second.
        let node = dec(r#"["^ ","~:aaa","ab","~:bbb","~i1","^0","~i2","^1","~i3"]"#);
        let TransitNode::Map(pairs) = node else {
            panic!("expected a map")
        };
        let keys: Vec<&TransitNode> = pairs.iter().map(|(k, _)| k).collect();
        assert_eq!(keys[2], keys[0], "^0 is the first cached keyword");
        assert_eq!(keys[3], keys[1], "^1 is the second cached keyword");
    }

    /// Map *keys* are cacheable regardless of prefix (`as_map_key`), while the
    /// same string in value position is not — the asymmetry the Python reader
    /// encodes and the one most likely to drift in a port.
    #[test]
    fn plain_map_keys_are_cacheable_but_plain_values_are_not() {
        assert!(is_cacheable("hello", true));
        assert!(!is_cacheable("hello", false));
        assert!(
            !is_cacheable("abc", true),
            "shorter than MIN_SIZE_CACHEABLE"
        );
        assert!(is_cacheable("~:kw", false));

        // The plain key "hello" takes slot 0; the plain value "world" takes none.
        assert_eq!(
            dec(r#"["^ ","hello","world","^0","~i1"]"#),
            TransitNode::Map(vec![
                (
                    TransitNode::Str("hello".into()),
                    TransitNode::Str("world".into())
                ),
                (TransitNode::Str("hello".into()), TransitNode::Int(1)),
            ])
        );
    }

    #[test]
    fn two_char_cache_codes_span_the_digit_base() {
        // 44 cacheable keywords fill ^0..^z; the 45th is reachable as ^10
        // ((1 - 0) * 44 + 0 = 44).
        let mut parts = vec![r#""^ ""#.to_string()];
        for i in 0..45 {
            parts.push(format!(r#""~:k{i:03}""#));
            parts.push(r#""~i0""#.to_string());
        }
        parts.push(r#""^10""#.to_string());
        parts.push(r#""~i1""#.to_string());
        let node = dec(&format!("[{}]", parts.join(",")));
        let TransitNode::Map(pairs) = node else {
            panic!("expected a map")
        };
        assert_eq!(pairs[45].0, TransitNode::Keyword("k044".into()));
    }

    #[test]
    fn tagged_values_decode_and_the_tag_is_cacheable() {
        assert_eq!(
            dec(r#"["~#list",["~i1"]]"#),
            TransitNode::Tagged(
                "list".into(),
                Box::new(TransitNode::List(vec![TransitNode::Int(1)]))
            )
        );
        // The tag marker itself occupies cache slot 0, so `^0` re-tags.
        assert_eq!(
            dec(r#"["^ ","~:a",["~#set",["~i1"]],"~:b",["^0",["~i2"]]]"#),
            TransitNode::Map(vec![
                (
                    TransitNode::Keyword("a".into()),
                    TransitNode::Tagged(
                        "set".into(),
                        Box::new(TransitNode::List(vec![TransitNode::Int(1)]))
                    )
                ),
                (
                    TransitNode::Keyword("b".into()),
                    TransitNode::Tagged(
                        "set".into(),
                        Box::new(TransitNode::List(vec![TransitNode::Int(2)]))
                    )
                ),
            ])
        );
    }

    /// A 2-element array whose head is a cache code resolving to a non-tag is
    /// a plain array, and re-reading the head must not disturb the cache.
    #[test]
    fn cache_code_head_that_is_not_a_tag_yields_a_plain_array() {
        assert_eq!(
            dec(r#"["^ ","~:aaa",["^0","~i2"]]"#),
            TransitNode::Map(vec![(
                TransitNode::Keyword("aaa".into()),
                TransitNode::List(vec![
                    TransitNode::Keyword("aaa".into()),
                    TransitNode::Int(2)
                ])
            )])
        );
    }

    #[test]
    fn unknown_ground_type_is_a_loud_error() {
        let err = decode_document(r#""~qzzz""#).expect_err("unknown prefix must fail");
        assert!(
            matches!(err, TransitError::UnknownGroundType { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn dangling_cache_reference_is_a_loud_error() {
        let err = decode_document(r#"["^ ","^5","~i1"]"#).expect_err("empty cache must fail");
        assert!(matches!(err, TransitError::CacheMiss { .. }), "got {err:?}");
    }

    #[test]
    fn unpaired_map_is_a_loud_error() {
        let err = decode_document(r#"["^ ","~:a"]"#).expect_err("odd map must fail");
        assert!(
            matches!(err, TransitError::UnpairedMap { len: 1 }),
            "got {err:?}"
        );
    }

    #[test]
    fn the_cache_does_not_survive_a_document() {
        decode_document(r#"["^ ","~:aaa","~i1"]"#).expect("first document");
        let err = decode_document(r#"["^ ","^0","~i1"]"#)
            .expect_err("a fresh document starts with an empty cache");
        assert!(matches!(err, TransitError::CacheMiss { .. }), "got {err:?}");
    }
}
