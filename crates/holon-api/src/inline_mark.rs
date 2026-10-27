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
/// `Scheme` holds a colon-bearing target EXACTLY as authored —
/// `[[block:uuid]]`, `[[cc-session:8f21]]`, `[[Areas:Work]]`, and also
/// `[[Meeting/Notes:2026]]`. Two things are deliberately NOT stored:
///
/// 1. Whether any entity claims the scheme. That is a property of the live
///    `TypeRegistry`, not of the text; storing it would encode the registry
///    contents at the ingest instant — a hidden mutable input that makes the
///    projection irreproducible from the org bytes (Model.md invariants 3, 4).
/// 2. Whether the target is a WHOLE scheme-shaped URI at all. The classifier
///    returns `UnknownScheme` for two different shapes — a whole scheme-shaped
///    target AND a wiki path whose later segment carries a colon
///    (`link_parser.rs:172` and `:196`) — and only the first names an entity.
///
/// Hence a `String`, not an `EntityUri`: `Meeting/Notes:2026` is not a valid
/// absolute URI, so parsing at STORE time either loses the mark or (worse)
/// coerces it to `block:Meeting/Notes:2026`, silently rewriting authored link
/// text. Use [`EntityRef::entity_uri`] to ask, at each use site, whether this
/// target names an entity.
///
/// `External` carries a raw URL string — we don't pull in the `url` crate
/// since the URL is presented verbatim in both org `[[uri][label]]` and the
/// editor's link popover. `Name` is a DANGLING wiki-name target (links
/// increment 2): the page it names may not exist yet — pages are created
/// lazily, never as placeholders — so the mark keeps the user's exact target
/// string (`[[Linked Page]]`, or a `parent/leaf` name-chain used as a suffix
/// resolution hint). Resolution state lives in the `block_links` junction
/// (`resolved_id`); the dangling→resolved mark upgrade rides the org
/// writeback (which emits the resolved `[[id][label]]` form) and the normal
/// re-ingest of that file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
// `snake_case` (not `lowercase`) so the `unknown_scheme` compat alias matches
// the tag the Loro backend reads.
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EntityRef {
    External {
        url: String,
    },
    /// Colon-bearing target, verbatim — see the type doc. `internal` (key
    /// `id`) and `unknown_scheme` (key `uri`) are the two pre-merge wire
    /// spellings; both stay readable, and because this is a plain `String`
    /// even the non-URI targets `unknown_scheme` could hold load unchanged.
    #[serde(rename = "scheme", alias = "internal", alias = "unknown_scheme")]
    Scheme {
        #[serde(alias = "id", alias = "uri")]
        raw: String,
    },
    Name {
        name: String,
    },
}

impl EntityRef {
    /// A scheme-shaped target built from an already-parsed URI.
    pub fn from_uri(uri: &EntityUri) -> Self {
        Self::Scheme {
            raw: uri.as_str().to_string(),
        }
    }

    /// The entity this target names, or `None` when it names no entity.
    ///
    /// `Some` only for a WHOLE scheme-shaped target (`t-widget:abc`). A wiki
    /// path that merely carries a colon in a later segment
    /// (`Meeting/Notes:2026`) is a page path — minting
    /// `block:Meeting/Notes:2026` for it would rewrite the user's link text
    /// — and every non-`Scheme` variant names no entity either.
    ///
    /// The shape check is the same one the classifier uses to tell the two
    /// cases apart, so this is that parse decision made once. A target that is
    /// scheme-SHAPED but not a parseable URI (`t-widget:a b`) also yields
    /// `None`: the conservative direction, since the alternative is inventing
    /// an entity reference the author never wrote.
    pub fn entity_uri(&self) -> Option<EntityUri> {
        let Self::Scheme { raw } = self else {
            return None;
        };
        crate::link_parser::link_scheme_shape(raw)?;
        // ALLOW(ok): scheme-shaped but unparseable (`t-widget:a b`) means "no
        // entity", which is the answer every caller needs; the raw text is
        // still rendered verbatim from `raw`, so nothing is lost.
        EntityUri::parse(raw).ok()
    }
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

/// What a mark MEANS, and therefore what a renderer is allowed to do with it
/// when it cannot emit the block perfectly. The three classes are not
/// interchangeable and the difference is not cosmetic:
///
/// - [`Styling`](MarkClass::Styling) decorates text that reads the same without
///   it.
/// - [`Protective`](MarkClass::Protective) is a claim ABOUT the text — "this
///   span is literal". Dropping it does not lose decoration, it un-protects the
///   span, so the next parse reinterprets those bytes as live markup.
/// - [`DataBearing`](MarkClass::DataBearing) holds payload that exists NOWHERE
///   else. A `Link`'s target lives only in the mark; the content carries the
///   label alone. Dropping it deletes data outright.
///
/// Written as a classification on the type rather than as conditions at each
/// call site so a new `InlineMark` variant cannot silently inherit the wrong
/// policy — the match below stops compiling until it is classified.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkClass {
    Styling,
    Protective,
    DataBearing,
}

impl InlineMark {
    /// This mark's [`MarkClass`] — see that type for what each class permits.
    pub fn class(&self) -> MarkClass {
        match self {
            InlineMark::Bold
            | InlineMark::Italic
            | InlineMark::Strike
            | InlineMark::Underline
            | InlineMark::Sub
            | InlineMark::Super => MarkClass::Styling,
            // Org parses no emphasis and no link syntax inside these.
            InlineMark::Code | InlineMark::Verbatim => MarkClass::Protective,
            // The target string is not recoverable from the content.
            InlineMark::Link { .. } => MarkClass::DataBearing,
        }
    }

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
/// `MarkSpan`s of DIFFERENT marks may overlap; the renderer is responsible for
/// coalescing per output format (org cannot represent arbitrary overlap;
/// markdown can via nesting).
///
/// Two spans of the SAME mark may not. Loro keeps marks as a Peritext
/// attribute set keyed by `(range, key, value)`, so it merges same-mark spans
/// across an overlap and gives the pair back as their union — an identical
/// triple twice being the degenerate case, where the renderer then emits one
/// delimiter pair for what the author wrote as two.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
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

/// The wire shape, deserialized structurally before `start <= end` is checked.
/// Deriving `Deserialize` on `MarkSpan` itself would reconstruct the struct
/// field-by-field and bypass every constructor.
#[derive(Deserialize)]
struct MarkSpanWire {
    start: usize,
    end: usize,
    #[serde(flatten)]
    mark: InlineMark,
}

/// Parse, don't validate: an inverted span is not a `MarkSpan` and never
/// becomes one. `start <= end` is the half of the invariant a span can prove
/// about ITSELF, so it belongs here, at the point the value enters the type.
///
/// The other half — `end <= content.chars().count()` — is deliberately NOT
/// checked here and cannot be: a `MarkSpan` does not know its content. That
/// stays a disclosed heal at the one read boundary that holds both halves of
/// the pair ([`canonicalize_marks_against`], called from the SQL row
/// deserializer), which is where writes we do not author — MCP
/// `execute_raw_sql` and `insert_data` accept arbitrary `marks` JSON — land.
impl<'de> Deserialize<'de> for MarkSpan {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = MarkSpanWire::deserialize(deserializer)?;
        if wire.start > wire.end {
            return Err(serde::de::Error::custom(format!(
                "MarkSpan: inverted span — start ({}) must be <= end ({}) for a {} mark",
                wire.start,
                wire.end,
                wire.mark.loro_key(),
            )));
        }
        Ok(Self {
            start: wire.start,
            end: wire.end,
            mark: wire.mark,
        })
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
/// This is the ONLY range-vs-content heal in the system (D27.a), and it lives
/// here because this is the one boundary that holds BOTH halves of the pair. A
/// mark whose `end` exceeds `content.chars().count()` aborts every paint of the
/// block in `scalar_range_to_bytes` — the crash that made a page permanently
/// un-openable — so the pair is repaired once, on the way in, rather than
/// guarded at each of the renderers downstream.
///
/// It is a heal rather than a hard `Err` because this boundary is where writes
/// we do not author land: MCP `execute_raw_sql` and `insert_data` accept
/// arbitrary `marks` JSON against any table, so a corrupt row here is not
/// necessarily a bug in our code and must not brick the page that holds it.
/// Spans we DO author are kept honest by construction instead — `MarkSpan`'s
/// `Deserialize` rejects an inverted span outright, and `MarkSpan::new`
/// asserts.
///
/// Fail-loud, not silent (project error philosophy): every clamp emits one
/// `warn!` naming `block_id`, the offending span and the content length — a
/// disclosed degraded read, not a cosmetic fix. `block_id` is what makes the
/// line actionable, so it is a parameter rather than something the caller is
/// asked to remember to log. A span clamped to empty is then dropped by
/// [`canonicalize_marks`].
pub fn canonicalize_marks_against(content: &str, marks: &mut Vec<MarkSpan>, block_id: &str) {
    let len = content.chars().count();
    for m in marks.iter_mut() {
        if m.start > len || m.end > len {
            tracing::warn!(
                block_id,
                mark_start = m.start,
                mark_end = m.end,
                content_chars = len,
                "mark span exceeds content length; clamping to render degraded \
                 (corrupt persisted marks for this block)"
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
        let json = v.try_into_json()?;
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
    /// Foreign-entity target (`[[cc-session:abc]]`, `[[person:alice]]`): a
    /// registered entity scheme that is neither `block` nor `tag`. Kept
    /// distinct from `Block` so `block` keeps meaning "the target is a block
    /// id" and downstream consumers get a clean predicate.
    Entity,
}

impl LinkKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            LinkKind::Page => "page",
            LinkKind::Block => "block",
            LinkKind::Tag => "tag",
            LinkKind::Entity => "entity",
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
/// External URL links are NOT block links (they never name an entity) and are
/// skipped. Duplicate `(target, kind)` pairs collapse to one row (junction PK
/// is `(source, target, kind)`).
///
/// Registry-INDEPENDENT on purpose: a target that NAMES an entity yields a row
/// whether or not an entity currently claims its scheme. The junction is a
/// soft-target relation (no FK, dangling representable — see
/// `sql/schema/block_links.sql`), so a row whose target does not exist yet is
/// already a supported state, and it is what lets a link survive being
/// ingested before its integration registers: nothing has to re-derive the
/// junction later. Classifying by REGISTRATION here instead would make the
/// junction a function of WHEN the block was written.
///
/// Registration-independent is not shape-independent: a `Scheme` target that
/// names no entity (a wiki path with a colon in a later segment) yields no row,
/// exactly as it did before it shared a variant with entity targets.
pub fn derive_block_links(marks: &[MarkSpan]) -> Vec<DerivedLink> {
    let mut out: Vec<DerivedLink> = Vec::new();
    for span in marks {
        let derived = match &span.mark {
            InlineMark::Link { target, .. } => match target {
                EntityRef::External { .. } => continue,
                EntityRef::Scheme { .. } => {
                    let Some(uri) = target.entity_uri() else {
                        continue;
                    };
                    DerivedLink {
                        target: uri.as_str().to_string(),
                        kind: match uri.scheme() {
                            "tag" => LinkKind::Tag,
                            "block" => LinkKind::Block,
                            _ => LinkKind::Entity,
                        },
                        resolved: Some(uri),
                    }
                }
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

// --- Read-mode styling fingerprint (paint-observable single source) ---
//
// The gpui read-mode renderer turns `[MarkSpan]` into styled text runs
// (`build_highlights` -> `StyledText::with_highlights`). That styling is
// otherwise pinned only by gpui unit tests and is structurally invisible to the
// headless keystone. `style_fingerprint` is the SINGLE oracle both the renderer
// (whose `merge_marks` layers theme colors on top of these booleans) and the
// GPUI-tier composed PBT (`inv-paint-text-styling`) agree on: the
// theme-independent PAINT signature — weight / slant / decoration / background
// — per byte-range run. Exact colors are the frontend's concern and
// deliberately NOT part of the fingerprint.

/// The theme-independent paint attributes covering one styled run. `false`
/// everywhere = plain text (no mark contributes a visible attribute, e.g. a
/// `Sub`/`Super`-only run today).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyleFlags {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    /// A background fill is painted (Code / Verbatim). The exact color is
    /// theme-dependent and not compared.
    pub background: bool,
}

impl StyleFlags {
    /// No visible attribute — the run paints as plain body text.
    pub fn is_plain(&self) -> bool {
        !(self.bold || self.italic || self.underline || self.strikethrough || self.background)
    }

    /// Union: any attribute set by either side is set in the result. Used to
    /// fold the active marks covering a segment into one run signature.
    pub fn or(self, o: StyleFlags) -> StyleFlags {
        StyleFlags {
            bold: self.bold || o.bold,
            italic: self.italic || o.italic,
            underline: self.underline || o.underline,
            strikethrough: self.strikethrough || o.strikethrough,
            background: self.background || o.background,
        }
    }
}

/// The paint attributes a single mark kind contributes. SINGLE SOURCE for the
/// read-mode renderer's boolean styling and the PBT oracle alike. `Link`
/// underlines (its distinguishing color is not a fingerprint attribute);
/// `Code`/`Verbatim` fill a background; `Sub`/`Super` have no paint attribute
/// in the current `HighlightStyle` model (no baseline-shift field).
pub fn mark_style_flags(mark: &InlineMark) -> StyleFlags {
    let mut f = StyleFlags::default();
    match mark {
        InlineMark::Bold => f.bold = true,
        InlineMark::Italic => f.italic = true,
        InlineMark::Underline => f.underline = true,
        InlineMark::Strike => f.strikethrough = true,
        InlineMark::Code | InlineMark::Verbatim => f.background = true,
        InlineMark::Link { .. } => f.underline = true,
        InlineMark::Sub | InlineMark::Super => {}
    }
    f
}

/// One styled run of a block's content: a half-open BYTE range and the paint
/// attributes covering it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyledRun {
    /// Byte offset into the block content (gpui `TextRun`/highlight ranges are
    /// byte-based).
    pub start: usize,
    pub end: usize,
    pub flags: StyleFlags,
}

/// Partition `content` by its marks into the styled runs a read-mode renderer
/// must paint, in ascending order. Mirrors the gpui renderer's
/// `compute_segments`/`build_highlights` exactly: boundaries are the mark edges
/// (clamped into the content's scalar length), only segments covered by at
/// least one mark are emitted (unstyled gaps are omitted), and offsets are
/// converted scalar -> byte at the segment level. A segment whose only active
/// marks are attribute-less (`Sub`/`Super`) is still emitted, with plain flags
/// — matching the renderer, which emits a default highlight for it.
pub fn style_fingerprint(content: &str, marks: &[MarkSpan]) -> Vec<StyledRun> {
    let mut char_to_byte: Vec<usize> = content.char_indices().map(|(i, _)| i).collect();
    char_to_byte.push(content.len());
    let total_chars = char_to_byte.len() - 1;

    let mut boundaries: Vec<usize> = marks
        .iter()
        .flat_map(|m| [m.start, m.end])
        .filter(|&b| b <= total_chars)
        .collect();
    boundaries.sort_unstable();
    boundaries.dedup();

    let mut runs = Vec::with_capacity(boundaries.len());
    for w in boundaries.windows(2) {
        let (start_char, end_char) = (w[0], w[1]);
        let mut flags = StyleFlags::default();
        let mut any = false;
        for m in marks {
            if m.start <= start_char && m.end >= end_char && m.start < m.end {
                flags = flags.or(mark_style_flags(&m.mark));
                any = true;
            }
        }
        if !any {
            continue;
        }
        runs.push(StyledRun {
            start: char_to_byte[start_char],
            end: char_to_byte[end_char],
            flags,
        });
    }
    runs
}

#[cfg(test)]
mod tests;
