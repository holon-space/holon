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
/// full serialized form as a total-order tiebreaker. Zero-width marks
/// (`start == end`) are DROPPED.
///
/// Storage backends emit equal mark *sets* in different *orders* — the Loro
/// Peritext reader closes overlapping runs in `HashMap` iteration order, while
/// the SQL JSON column preserves insertion order. The projection's content
/// diff compares `Block.marks` as a `Vec`, so without a canonical order two
/// faithful round-trips of the same block would diff spuriously and fire a
/// redundant update. Canonicalizing at every read boundary makes that compare
/// order-insensitive.
///
/// Dropping zero-width marks here is the choke point for the "zero-width mark
/// must not survive an edit" invariant: when a user deletes the entire text
/// under a link (or any mark), the Loro Peritext layer retains a collapsed
/// `start == end` anchor. Every read boundary (`read_marks_from_text`,
/// `marks_from_json`, the PBT block-compare normalizer) routes through here,
/// so a collapsed mark is stripped before it can reach any projection or the
/// org writeback that would otherwise serialize it as reversed brackets.
pub fn canonicalize_marks(marks: &mut Vec<MarkSpan>) {
    marks.retain(|m| m.start != m.end);
    marks.sort_by(|a, b| {
        (a.start, a.end)
            .cmp(&(b.start, b.end))
            .then_with(|| a.mark.loro_key().cmp(b.mark.loro_key()))
            .then_with(|| {
                marks_to_json(std::slice::from_ref(a)).cmp(&marks_to_json(std::slice::from_ref(b)))
            })
    });
}

/// Clamp every mark span into the bounds of `content`, then canonicalize.
///
/// This is the SINGLE read-boundary choke point that guarantees a corrupt
/// persisted `(content, marks)` pair — a mark whose `end` exceeds
/// `content.chars().count()` — can never reach a projection or the GPUI
/// renderer, where an out-of-bounds span aborts EVERY paint of the block in
/// `scalar_range_to_bytes` (the crash that made a page permanently
/// un-openable). Such a mark arises when a producer writes a full-span mark
/// decoupled from the content (`convert_block_to_page`) or a later content-only
/// trim shortens the text without adjusting marks.
///
/// Fail-loud, not silent (project error philosophy): each clamp emits a loud
/// `warn!` naming the offending span and the content length — a disclosed
/// degraded read, not a cosmetic fix. A span clamped to empty is then dropped
/// by [`canonicalize_marks`]. Callers that also know the block id should log it
/// at the call site for row-level attribution.
pub fn canonicalize_marks_against(content: &str, marks: &mut Vec<MarkSpan>) {
    let len = content.chars().count();
    for m in marks.iter_mut() {
        if m.start > len || m.end > len {
            tracing::warn!(
                mark_start = m.start,
                mark_end = m.end,
                content_chars = len,
                "canonicalize_marks_against: mark span exceeds content length; \
                 clamping (corrupt persisted marks)"
            );
            m.start = m.start.min(len);
            m.end = m.end.min(len);
        }
    }
    canonicalize_marks(marks);
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

/// One side of a `split_block` — the resulting content string and the marks
/// that survived the split on that side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitSide {
    pub content: String,
    pub marks: Vec<MarkSpan>,
}

/// Result of splitting a block's `(content, marks)` at a cursor: the retained
/// (left) half and the split-off (right) half.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitContentMarks {
    pub left: SplitSide,
    pub right: SplitSide,
}

/// Split a block's stripped-label `content` and its `marks` at byte offset
/// `split_byte`, returning the two halves each with correctly partitioned
/// marks.
///
/// This is the SINGLE source of truth for how `split_block` divides marks. Both
/// the production `BlockOperations::split_block` and the keystone reference
/// model call it, so model and prod cannot silently disagree about the fate of
/// a mark that spans a split point (the model-first invariant).
///
/// Mirrors `split_block`'s whitespace normalization: the LEFT half is
/// `trim_end`ed and the RIGHT half is `trim_start`ed. Mark offsets are Unicode
/// scalar `[start, end)` (matching `MarkSpan` and the Loro Peritext
/// convention), while `split_byte` is a BYTE offset into `content` (matching
/// the editor's cursor position and `content[..split_byte]` slicing) — the two
/// coordinate spaces are reconciled internally.
///
/// Partition rule per mark `[s, e)` against the split scalar `p`:
/// - **entirely left** (`e <= p`): stays on the left, ends clamped into the
///   trimmed left length so a mark that ran into trimmed trailing whitespace
///   never dangles out of bounds.
/// - **entirely right** (`s >= p`): moves to the right, rebased by `p` plus any
///   leading whitespace the right half trimmed.
/// - **straddling** (`s < p < e`): kind-differentiated, because the two mark
///   families round-trip through org differently —
///   - a **link** degrades to plain text on BOTH halves (the mark is DROPPED).
///     A partial link label is not a resolvable reference: org-rendering a
///     truncated `[[Ada Lo]]` and its remainder `[[velace]]` would fabricate
///     two phantom pages. Dropping matches editor behavior (splitting inside a
///     wiki-link breaks it) and is the org round-trip fixed point — plain text
///     re-extracts to plain text, no phantom `block_links`.
///   - a **formatting** mark (bold/italic/code/verbatim/strike/underline/
///     sub/super) is TRUNCATED: the left keeps `[s, p)` and the right keeps
///     `[0, e-p)`. `*bo|ld*` splits to `*bo*` + `*ld*`, both valid org and
///     round-trip stable.
///
/// Both halves are `canonicalize_marks`d before return (zero-width remnants —
/// e.g. a mark wholly inside trimmed whitespace — are dropped there).
pub fn split_content_marks(
    content: &str,
    marks: &[MarkSpan],
    split_byte: usize,
) -> SplitContentMarks {
    assert!(
        split_byte <= content.len(),
        "split_content_marks: byte offset {split_byte} exceeds content length {}",
        content.len()
    );
    assert!(
        content.is_char_boundary(split_byte),
        "split_content_marks: byte offset {split_byte} is not a char boundary of {content:?}"
    );

    let left_raw = &content[..split_byte];
    let right_raw = &content[split_byte..];

    // Split point expressed in Unicode-scalar space, where marks live.
    let split_scalar = left_raw.chars().count();

    let left_content = left_raw.trim_end();
    let right_content = right_raw.trim_start();

    // Scalar length of the left half AFTER trimming trailing whitespace; left
    // marks are clamped into this so nothing dangles past the truncated content.
    let left_len = left_content.chars().count();
    // Leading whitespace scalars trimmed off the right half — right marks rebase
    // by the split point plus this amount.
    let right_lead_trim = right_raw.chars().count() - right_content.chars().count();
    let right_shift = split_scalar + right_lead_trim;

    let mut left_marks: Vec<MarkSpan> = Vec::new();
    let mut right_marks: Vec<MarkSpan> = Vec::new();

    for m in marks {
        let (s, e) = (m.start, m.end);
        if e <= split_scalar {
            // Entirely left: keep, clamping the end into the trimmed left length.
            left_marks.push(MarkSpan::new(
                s.min(left_len),
                e.min(left_len),
                m.mark.clone(),
            ));
        } else if s >= split_scalar {
            // Entirely right: rebase left by the split + trimmed leading ws.
            right_marks.push(MarkSpan::new(
                s.saturating_sub(right_shift),
                e.saturating_sub(right_shift),
                m.mark.clone(),
            ));
        } else {
            // Straddling the split point.
            match &m.mark {
                InlineMark::Link { .. } => {
                    // Degrade to plain text on both halves — drop the mark.
                }
                _ => {
                    // Formatting mark: truncate to each side.
                    left_marks.push(MarkSpan::new(
                        s.min(left_len),
                        split_scalar.min(left_len),
                        m.mark.clone(),
                    ));
                    right_marks.push(MarkSpan::new(
                        0,
                        e.saturating_sub(right_shift),
                        m.mark.clone(),
                    ));
                }
            }
        }
    }

    canonicalize_marks(&mut left_marks);
    canonicalize_marks(&mut right_marks);

    SplitContentMarks {
        left: SplitSide {
            content: left_content.to_string(),
            marks: left_marks,
        },
        right: SplitSide {
            content: right_content.to_string(),
            marks: right_marks,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(name: &str) -> InlineMark {
        InlineMark::Link {
            target: EntityRef::Name {
                name: name.to_string(),
            },
            label: name.to_string(),
        }
    }

    #[test]
    fn split_keeps_link_entirely_left_and_leaves_right_bare() {
        // Dogfood 2026-07-20 repro: "Owner is Ada Lovelace and reviewer",
        // link over "Ada Lovelace" = scalars [9, 21). Split after "and"
        // (byte 25, before the space). Link is wholly left → preserved in
        // bounds; right half "reviewer" carries no marks.
        let content = "Owner is Ada Lovelace and reviewer";
        let marks = vec![MarkSpan::new(9, 21, link("Ada Lovelace"))];
        let out = split_content_marks(content, &marks, 25);
        assert_eq!(out.left.content, "Owner is Ada Lovelace and");
        assert_eq!(
            out.left.marks,
            vec![MarkSpan::new(9, 21, link("Ada Lovelace"))]
        );
        assert_eq!(out.right.content, "reviewer");
        assert!(out.right.marks.is_empty());
    }

    #[test]
    fn split_moves_link_entirely_right_rebased() {
        // "see Ada Lovelace" split after "see " (byte 4): link over
        // "Ada Lovelace" [4, 16) moves right, rebased to [0, 12).
        let content = "see Ada Lovelace";
        let marks = vec![MarkSpan::new(4, 16, link("Ada Lovelace"))];
        let out = split_content_marks(content, &marks, 4);
        assert_eq!(out.left.content, "see");
        assert!(out.left.marks.is_empty());
        assert_eq!(out.right.content, "Ada Lovelace");
        assert_eq!(
            out.right.marks,
            vec![MarkSpan::new(0, 12, link("Ada Lovelace"))]
        );
    }

    #[test]
    fn split_inside_link_degrades_to_plain_text_both_sides() {
        // Split INSIDE "Ada Lovelace" (byte 12 = after "Ada Lovel") — the link
        // straddles the split, so it degrades to plain text on BOTH halves.
        let content = "Ada Lovelace";
        let marks = vec![MarkSpan::new(0, 12, link("Ada Lovelace"))];
        let out = split_content_marks(content, &marks, 5); // "Ada L" | "ovelace"
        assert_eq!(out.left.content, "Ada L");
        assert!(
            out.left.marks.is_empty(),
            "left link remnant must be dropped, got {:?}",
            out.left.marks
        );
        assert_eq!(out.right.content, "ovelace");
        assert!(
            out.right.marks.is_empty(),
            "right link remnant must be dropped, got {:?}",
            out.right.marks
        );
    }

    #[test]
    fn split_inside_formatting_mark_truncates_both_sides() {
        // Bold over all of "bold" [0,4); split after "bo" (byte 2).
        let content = "bold";
        let marks = vec![MarkSpan::new(0, 4, InlineMark::Bold)];
        let out = split_content_marks(content, &marks, 2);
        assert_eq!(out.left.content, "bo");
        assert_eq!(out.left.marks, vec![MarkSpan::new(0, 2, InlineMark::Bold)]);
        assert_eq!(out.right.content, "ld");
        assert_eq!(out.right.marks, vec![MarkSpan::new(0, 2, InlineMark::Bold)]);
    }

    #[test]
    fn split_at_start_moves_all_marks_right() {
        let content = "Ada";
        let marks = vec![MarkSpan::new(0, 3, InlineMark::Italic)];
        let out = split_content_marks(content, &marks, 0);
        assert_eq!(out.left.content, "");
        assert!(out.left.marks.is_empty());
        assert_eq!(out.right.content, "Ada");
        assert_eq!(
            out.right.marks,
            vec![MarkSpan::new(0, 3, InlineMark::Italic)]
        );
    }

    #[test]
    fn split_at_end_keeps_all_marks_left() {
        let content = "Ada";
        let marks = vec![MarkSpan::new(0, 3, InlineMark::Italic)];
        let out = split_content_marks(content, &marks, 3);
        assert_eq!(out.left.content, "Ada");
        assert_eq!(
            out.left.marks,
            vec![MarkSpan::new(0, 3, InlineMark::Italic)]
        );
        assert_eq!(out.right.content, "");
        assert!(out.right.marks.is_empty());
    }

    #[test]
    fn split_clamps_left_mark_into_trimmed_trailing_whitespace() {
        // Bold covers "ab  " [0,4) including two trailing spaces; splitting at
        // byte 4 trims them off the left, so the bold clamps to [0,2).
        let content = "ab  cd";
        let marks = vec![MarkSpan::new(0, 4, InlineMark::Bold)];
        let out = split_content_marks(content, &marks, 4);
        assert_eq!(out.left.content, "ab");
        assert_eq!(out.left.marks, vec![MarkSpan::new(0, 2, InlineMark::Bold)]);
        assert_eq!(out.right.content, "cd");
    }

    #[test]
    fn split_boundary_marks_do_not_cross() {
        // Two adjacent bolds [0,3) and [3,6); split exactly at scalar 3.
        let content = "abcdef";
        let marks = vec![
            MarkSpan::new(0, 3, InlineMark::Bold),
            MarkSpan::new(3, 6, InlineMark::Italic),
        ];
        let out = split_content_marks(content, &marks, 3);
        assert_eq!(out.left.marks, vec![MarkSpan::new(0, 3, InlineMark::Bold)]);
        assert_eq!(
            out.right.marks,
            vec![MarkSpan::new(0, 3, InlineMark::Italic)]
        );
    }

    #[test]
    fn split_multibyte_prefix_rebases_right_mark_in_scalar_space() {
        // "über Ada tail": ü is 2 bytes, so BYTE and SCALAR offsets diverge.
        // Link over "Ada" = scalars [5,8). Split after "über " (BYTE 6, which
        // is 5 SCALARS). The link is entirely right and must rebase to [0,3) in
        // scalar space — a byte-space rebase (by 6) would give [-1,2)/[0,2).
        let content = "über Ada tail";
        assert_eq!(content.len(), 14, "ü is 2 bytes: 14 bytes, 13 scalars");
        let marks = vec![MarkSpan::new(5, 8, link("Ada"))];
        let out = split_content_marks(content, &marks, 6);
        assert_eq!(out.left.content, "über");
        assert!(out.left.marks.is_empty());
        assert_eq!(out.right.content, "Ada tail");
        assert_eq!(out.right.marks, vec![MarkSpan::new(0, 3, link("Ada"))]);
    }

    #[test]
    fn split_end_clamps_left_mark_spanning_multibyte_chars() {
        // "café  bar": é is 2 bytes. Bold covers "café  " = scalars [0,6)
        // (including the two trailing spaces). Splitting after the spaces
        // (BYTE 7, = 6 SCALARS) trims them off the left, clamping the bold to
        // [0,4) over "café" — the clamp is computed in scalar space across é.
        let content = "café  bar";
        assert_eq!(content.len(), 10, "café=5 bytes + 2 spaces + bar=3");
        let marks = vec![MarkSpan::new(0, 6, InlineMark::Bold)];
        let out = split_content_marks(content, &marks, 7);
        assert_eq!(out.left.content, "café");
        assert_eq!(out.left.marks, vec![MarkSpan::new(0, 4, InlineMark::Bold)]);
        assert_eq!(out.right.content, "bar");
        assert!(out.right.marks.is_empty());
    }

    #[test]
    fn split_at_byte_boundary_right_after_multibyte_char() {
        // Split lands on the BYTE boundary immediately after é (byte 5), which
        // is SCALAR 4. A bold ending exactly at é stays fully left (end-clamped
        // in scalar space); a link in the right half rebases to [0,3).
        let content = "café bar";
        assert_eq!(content.len(), 9, "café=5 bytes + space + bar=3");
        assert!(
            content.is_char_boundary(5),
            "byte 5 is the boundary after é"
        );
        let marks = vec![
            MarkSpan::new(0, 4, InlineMark::Bold), // "café"
            MarkSpan::new(5, 8, link("bar")),      // "bar"
        ];
        let out = split_content_marks(content, &marks, 5);
        assert_eq!(out.left.content, "café");
        assert_eq!(out.left.marks, vec![MarkSpan::new(0, 4, InlineMark::Bold)]);
        assert_eq!(out.right.content, "bar");
        assert_eq!(out.right.marks, vec![MarkSpan::new(0, 3, link("bar"))]);
    }

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
    fn canonicalize_drops_zero_width_marks() {
        // A collapsed mark (start == end) is left behind when an edit deletes
        // the entire text under a mark (e.g. a link's label). Every read
        // boundary routes through canonicalize_marks, which must strip it so it
        // never reaches a projection or the org writeback (`]][[` root cause).
        let mut marks = vec![
            MarkSpan::new(0, 4, InlineMark::Bold),
            MarkSpan::new(2, 2, InlineMark::Italic),
            MarkSpan::new(
                4,
                4,
                InlineMark::Link {
                    target: EntityRef::Name {
                        name: String::new(),
                    },
                    label: String::new(),
                },
            ),
        ];
        canonicalize_marks(&mut marks);
        assert_eq!(marks, vec![MarkSpan::new(0, 4, InlineMark::Bold)]);

        // marks_from_json (a read boundary) inherits the drop.
        let json = marks_to_json(&[MarkSpan::new(0, 0, InlineMark::Bold)]);
        let back = marks_from_json(&json).expect("parse");
        assert!(
            back.is_empty(),
            "zero-width mark survived json read: {back:?}"
        );
    }

    #[test]
    fn canonicalize_against_clamps_out_of_bounds_end() {
        // A mark whose end outlives the content (a decoupled marks-only write,
        // or a later content-only trim) must be clamped to the content length —
        // otherwise it aborts every render in scalar_range_to_bytes.
        let mut marks = vec![MarkSpan::new(0, 5, InlineMark::Bold)];
        canonicalize_marks_against("abc", &mut marks); // "abc" = 3 scalars
        assert_eq!(marks, vec![MarkSpan::new(0, 3, InlineMark::Bold)]);
    }

    #[test]
    fn canonicalize_against_drops_fully_out_of_bounds_mark() {
        // start and end both past the content clamp to (len, len) = empty, which
        // canonicalize then drops.
        let mut marks = vec![MarkSpan::new(4, 6, InlineMark::Italic)];
        canonicalize_marks_against("ab", &mut marks); // "ab" = 2 scalars
        assert!(
            marks.is_empty(),
            "fully out-of-bounds mark survived: {marks:?}"
        );
    }

    #[test]
    fn canonicalize_against_uses_scalar_not_byte_length() {
        // Multi-byte content: a 3-scalar string ("a你好") whose byte length is 7.
        // A mark 0..3 is in-bounds by SCALARS and must be preserved untouched.
        let mut marks = vec![MarkSpan::new(0, 3, InlineMark::Bold)];
        canonicalize_marks_against("a你好", &mut marks);
        assert_eq!(marks, vec![MarkSpan::new(0, 3, InlineMark::Bold)]);
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
