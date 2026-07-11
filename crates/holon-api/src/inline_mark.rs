//! Inline rich-text marks: syntax-neutral model used by the rich-text editor,
//! the org parser/renderer, the SQL `marks` projection, and (via a separate
//! adapter in `holon_loro`) Loro Peritext.
//!
//! # Indexing
//!
//! `MarkSpan::start` / `end` are **Unicode scalar offsets** (Rust `char`
//! positions), matching Loro's default `LoroText::mark` API. The org parser
//! converts from orgize's byte ranges at the boundary; the renderer converts
//! back to bytes at the boundary. Frontends should not need to convert.
//!
//! # Why JSON, not Loro, here
//!
//! `holon-api` is the abstract layer; it must not depend on Loro. The JSON
//! shape defined below is the wire format used by:
//! - the SQL `blocks.marks` column (`Value::Json` payload),
//! - PRQL queries that surface marks alongside content,
//! - the FRB bridge to Flutter (which round-trips the JSON string).
//!
//! Loro `LoroValue::Map` conversion lives in
//! `crates/holon/src/api/loro_backend.rs` where the Loro dependency is in
//! scope.
//!
//! # Boundary expansion
//!
//! Whether typing at a mark boundary continues the mark is **not** stored on
//! the mark span — it's a per-key `LoroDoc::config_text_style` decision set
//! once at LoroDoc creation. See `loro_backend.rs` for the policy
//! (Bold/Italic/Code/Strike/Underline/Sub/Super = After; Link/Verbatim = None).

use serde::Deserialize;
use serde::Serialize;

use crate::EntityUri;
use crate::Value;

/// Target of a `Link` mark.
///
/// `Internal` references a block by its `EntityUri` (so renames update the
/// label via a Phase 6 hook). `External` carries a raw URL string — we don't
/// pull in the `url` crate since the URL is presented verbatim in both org
/// `[[uri][label]]` and the editor's link popover. `Name` is a DANGLING
/// wiki-name target (links increment 2): the page it names may not exist yet
/// — pages are created lazily, never as placeholders — so the mark keeps the
/// user's exact target string (`[[Linked Page]]`, or a `parent/leaf`
/// name-chain used as a suffix resolution hint). Resolution state lives in
/// the `block_links` junction (`resolved_id`); the dangling→resolved mark
/// upgrade rides the org writeback (which emits the resolved `[[id][label]]`
/// form) and the normal re-ingest of that file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum EntityRef {
    External { url: String },
    Internal { id: EntityUri },
    Name { name: String },
}

/// One inline mark kind. The `Link` variant carries its target inline so a
/// `MarkSpan` is fully self-describing.
///
/// Ordering note: variants are listed in plan-document order (Bold, Italic,
/// Code, Verbatim, Strike, Underline, Link, Sub, Super). Renderer
/// precedence is independent — see `Block::to_org` for the emit order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum InlineMark {
    Bold,
    Italic,
    Code,
    Verbatim,
    Strike,
    Underline,
    Link { target: EntityRef, label: String },
    Sub,
    Super,
}

impl InlineMark {
    /// The mark "key" used as the Loro `config_text_style` map key. Keys must
    /// be stable across versions because they're embedded in the persisted
    /// Loro document.
    pub fn loro_key(&self) -> &'static str {
        match self {
            InlineMark::Bold => "bold",
            InlineMark::Italic => "italic",
            InlineMark::Code => "code",
            InlineMark::Verbatim => "verbatim",
            InlineMark::Strike => "strike",
            InlineMark::Underline => "underline",
            InlineMark::Link { .. } => "link",
            InlineMark::Sub => "sub",
            InlineMark::Super => "super",
        }
    }

    /// All `loro_key` values in a stable order.
    /// `loro_backend::config_text_style` iterates this to install the
    /// per-key `ExpandType` policy at LoroDoc creation. Keep this in sync
    /// with `expand_after` below.
    pub fn all_loro_keys() -> &'static [&'static str] {
        &[
            "bold",
            "italic",
            "code",
            "verbatim",
            "strike",
            "underline",
            "link",
            "sub",
            "super",
        ]
    }

    /// Whether typing at the trailing edge of this mark should inherit it.
    /// `true` ⇒ `ExpandType::After`; `false` ⇒ `ExpandType::None`. Per Phase
    /// 0.1 spike S3, this must be set once at `LoroDoc::config_text_style`
    /// time and never re-configured (silent no-op otherwise).
    pub fn expand_after(key: &str) -> bool {
        match key {
            "bold" | "italic" | "code" | "strike" | "underline" | "sub" | "super" => true,
            "link" | "verbatim" => false,
            _ => true,
        }
    }
}

/// One inline mark applied to a half-open `[start, end)` range of Unicode
/// scalar offsets within a single block's `content`.
///
/// Multiple `MarkSpan`s with overlapping ranges are allowed; the renderer
/// is responsible for coalescing per output format (org cannot represent
/// arbitrary overlap; markdown can via nesting).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkSpan {
    pub start: usize,
    pub end: usize,
    #[serde(flatten)]
    pub mark: InlineMark,
}

impl MarkSpan {
    /// Construct a span. Asserts `start <= end`.
    pub fn new(start: usize, end: usize, mark: InlineMark) -> Self {
        assert!(
            start <= end,
            "MarkSpan: start ({start}) must be <= end ({end})"
        );
        Self { start, end, mark }
    }
}

/// Serialize a slice of marks to the JSON wire format used by:
/// - the SQL `blocks.marks` column,
/// - the FRB bridge to Flutter,
/// - PRQL surface fields.
///
/// Stable across versions; compact (no whitespace).
pub fn marks_to_json(marks: &[MarkSpan]) -> String {
    serde_json::to_string(marks).expect("MarkSpan serialization is total")
}

/// Parse marks from the JSON wire format. Errors surface as `ApiError`
/// rather than silently returning empty — fail-loud per project policy.
///
/// The result is canonicalized (see [`canonicalize_marks`]) so that equal mark
/// sets compare equal regardless of the order the storage backend emitted them.
pub fn marks_from_json(s: &str) -> Result<Vec<MarkSpan>, serde_json::Error> {
    let mut marks: Vec<MarkSpan> = serde_json::from_str(s)?;
    canonicalize_marks(&mut marks);
    Ok(marks)
}

/// Sort marks into a canonical order: by `(start, end)`, then mark kind, then
/// full serialized form as a total-order tiebreaker.
///
/// Storage backends emit equal mark *sets* in different *orders* — the Loro
/// Peritext reader closes overlapping runs in `HashMap` iteration order, while
/// the SQL JSON column preserves insertion order. The projection's content
/// diff compares `Block.marks` as a `Vec`, so without a canonical order two
/// faithful round-trips of the same block would diff spuriously and fire a
/// redundant update. Canonicalizing at every read boundary makes that compare
/// order-insensitive.
pub fn canonicalize_marks(marks: &mut [MarkSpan]) {
    marks.sort_by(|a, b| {
        (a.start, a.end)
            .cmp(&(b.start, b.end))
            .then_with(|| a.mark.loro_key().cmp(b.mark.loro_key()))
            .then_with(|| {
                marks_to_json(std::slice::from_ref(a)).cmp(&marks_to_json(std::slice::from_ref(b)))
            })
    });
}

// --- Value <-> MarkSpan conversions for the entity framework ---
//
// The `Block.marks: Option<Vec<MarkSpan>>` field flows through the
// `IntoEntity` machinery, which serializes each field to a `Value`.
// The framework already has generic `TryFrom<Value> for Option<T>` and
// `Vec<T>` impls — they just need `MarkSpan` to be Value-convertible.

impl From<MarkSpan> for Value {
    fn from(span: MarkSpan) -> Self {
        let json = serde_json::to_value(&span).expect("MarkSpan serialization is total");
        Value::from(json)
    }
}

impl TryFrom<Value> for MarkSpan {
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn try_from(v: Value) -> Result<Self, Self::Error> {
        let json: serde_json::Value = v.into();
        serde_json::from_value(json).map_err(|e| Box::new(e) as Self::Error)
    }
}

// --- block_links derivation (links increment 2) ---

/// Kind of a `block_links` junction row. Stored as its `as_str` form in the
/// `block_links.kind` column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkKind {
    /// Wiki-name target (`[[Page Name]]` / `[[parent/leaf]]`): names a page.
    Page,
    /// Direct entity-id target (`[[block:...][label]]`).
    Block,
    /// Tag target (`tag:` scheme).
    Tag,
}

impl LinkKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            LinkKind::Page => "page",
            LinkKind::Block => "block",
            LinkKind::Tag => "tag",
        }
    }
}

/// One derived `block_links` row (source id supplied by the writer).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DerivedLink {
    /// The target as authored: a wiki name / name-chain for `Page`, the full
    /// schemed URI string for `Block`/`Tag`.
    pub target: String,
    pub kind: LinkKind,
    /// Already-resolved target id (id-form links resolve trivially; name-form
    /// links start dangling — the SQL write boundary fills this by page-name
    /// lookup).
    pub resolved: Option<EntityUri>,
}

/// Derive the `block_links` rows implied by a block's inline marks.
///
/// External URL links are NOT block links (they never resolve to an entity)
/// and are skipped. Duplicate `(target, kind)` pairs collapse to one row
/// (junction PK is `(source, target, kind)`).
pub fn derive_block_links(marks: &[MarkSpan]) -> Vec<DerivedLink> {
    let mut out: Vec<DerivedLink> = Vec::new();
    for span in marks {
        let derived = match &span.mark {
            InlineMark::Link { target, .. } => match target {
                EntityRef::External { .. } => continue,
                EntityRef::Internal { id } => DerivedLink {
                    target: id.as_str().to_string(),
                    kind: if id.scheme() == "tag" {
                        LinkKind::Tag
                    } else {
                        LinkKind::Block
                    },
                    resolved: Some(id.clone()),
                },
                EntityRef::Name { name } => DerivedLink {
                    target: name.clone(),
                    kind: LinkKind::Page,
                    resolved: None,
                },
            },
            _ => continue,
        };
        if !out
            .iter()
            .any(|d| d.target == derived.target && d.kind == derived.kind)
        {
            out.push(derived);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_simple_marks() {
        let marks = vec![
            MarkSpan::new(0, 5, InlineMark::Bold),
            MarkSpan::new(5, 10, InlineMark::Italic),
        ];
        let json = marks_to_json(&marks);
        let back = marks_from_json(&json).expect("round-trip");
        assert_eq!(marks, back);
    }

    #[test]
    fn canonicalize_makes_equal_sets_compare_equal_regardless_of_input_order() {
        // Same mark set, two emission orders (as a Loro HashMap scan vs a SQL
        // JSON column might produce). After canonicalization they must be
        // identical so the projection's `Vec<MarkSpan>` compare is a no-op.
        let a = vec![
            MarkSpan::new(6, 11, InlineMark::Italic),
            MarkSpan::new(0, 5, InlineMark::Bold),
            MarkSpan::new(0, 5, InlineMark::Underline),
        ];
        let b = vec![
            MarkSpan::new(0, 5, InlineMark::Underline),
            MarkSpan::new(0, 5, InlineMark::Bold),
            MarkSpan::new(6, 11, InlineMark::Italic),
        ];
        let mut a_canon = a.clone();
        let mut b_canon = b.clone();
        canonicalize_marks(&mut a_canon);
        canonicalize_marks(&mut b_canon);
        assert_eq!(a_canon, b_canon);
        // And the order is the expected (start, end, kind) order.
        assert_eq!(a_canon[0], MarkSpan::new(0, 5, InlineMark::Bold));
        assert_eq!(a_canon[1], MarkSpan::new(0, 5, InlineMark::Underline));
        assert_eq!(a_canon[2], MarkSpan::new(6, 11, InlineMark::Italic));
    }

    #[test]
    fn round_trip_link_external() {
        let marks = vec![MarkSpan::new(
            0,
            4,
            InlineMark::Link {
                target: EntityRef::External {
                    url: "https://example.com".to_string(),
                },
                label: "demo".to_string(),
            },
        )];
        let json = marks_to_json(&marks);
        let back = marks_from_json(&json).expect("round-trip");
        assert_eq!(marks, back);
    }

    #[test]
    fn round_trip_link_internal() {
        let marks = vec![MarkSpan::new(
            10,
            20,
            InlineMark::Link {
                target: EntityRef::Internal {
                    id: EntityUri::block("abc-123"),
                },
                label: "see also".to_string(),
            },
        )];
        let json = marks_to_json(&marks);
        let back = marks_from_json(&json).expect("round-trip");
        assert_eq!(marks, back);
    }

    #[test]
    fn loro_keys_cover_all_variants() {
        // Every variant must produce a key, and `all_loro_keys` must list
        // every key exactly once. This is the gate for `config_text_style`
        // installing the right `ExpandType` for every variant the editor
        // can produce.
        let cases = [
            InlineMark::Bold,
            InlineMark::Italic,
            InlineMark::Code,
            InlineMark::Verbatim,
            InlineMark::Strike,
            InlineMark::Underline,
            InlineMark::Link {
                target: EntityRef::External { url: String::new() },
                label: String::new(),
            },
            InlineMark::Sub,
            InlineMark::Super,
        ];
        let keys: Vec<&'static str> = cases.iter().map(|m| m.loro_key()).collect();
        let all = InlineMark::all_loro_keys();
        assert_eq!(keys.len(), all.len(), "key count mismatch");
        for k in &keys {
            assert!(all.contains(k), "{k} missing from all_loro_keys");
        }
    }

    #[test]
    fn expand_after_policy_matches_plan() {
        // Plan policy: Bold/Italic/Code/Strike/Underline/Sub/Super = After;
        // Link/Verbatim = None.
        for k in [
            "bold",
            "italic",
            "code",
            "strike",
            "underline",
            "sub",
            "super",
        ] {
            assert!(InlineMark::expand_after(k), "{k} should expand after");
        }
        for k in ["link", "verbatim"] {
            assert!(!InlineMark::expand_after(k), "{k} should NOT expand after");
        }
    }

    #[test]
    #[should_panic(expected = "start (5) must be <= end (3)")]
    fn span_rejects_inverted_range() {
        MarkSpan::new(5, 3, InlineMark::Bold);
    }
}
