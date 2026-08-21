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
            if entries.len() % 2 != 0 {
                return Err(TransitError::UnpairedMap { len: entries.len() });
            }
            let mut pairs = Vec::with_capacity(entries.len() / 2);
            for pair in entries.chunks_exact(2) {
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
        // `~i` integer, `~n` bigint — a bigint past i64 is a datom we cannot
        // represent, so it stops the import rather than wrapping.
        'i' | 'n' => TransitNode::Int(rest.parse().map_err(|source| TransitError::BadInt {
            text: s.to_string(),
            source,
        })?),
        // `~d` double, `~f` bigdec.
        'd' | 'f' => TransitNode::Float(F64Bits::new(rest.parse().map_err(|source| {
            TransitError::BadFloat {
                text: s.to_string(),
                source,
            }
        })?)),
        // `~t` instant string, `~m` instant millis.
        't' | 'm' => TransitNode::Instant(rest),
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
        assert_eq!(
            dec(r#""~m1787349600000""#),
            TransitNode::Instant("1787349600000".into())
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
