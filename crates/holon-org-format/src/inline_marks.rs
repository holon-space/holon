//! Org inline-markup → `holon_api::MarkSpan` extraction.
//!
//! Public entry point: [`extract_inline_marks`] takes a paragraph-shaped
//! string of Org inline content and returns:
//! - the **rendered text** (delimiters stripped — `*bold*` → `bold`)
//! - a `Vec<MarkSpan>` whose `start`/`end` are **Unicode scalar offsets** into
//!   the rendered text (matches Loro's default `LoroText::mark` API and the
//!   convention documented in `holon_api::inline_mark`).
//!
//! Algorithm (recursive on orgize's syntax tree):
//! 1. Parse `text` with `orgize::Org::parse`.
//! 2. Walk the document tree skipping non-paragraph wrappers; emit text tokens
//!    directly to the output.
//! 3. On encountering a mark node (BOLD/ITALIC/.../LINK/SUB/SUPER), strip its
//!    delimiters, recurse on the inner string for nested marks, then emit a
//!    `MarkSpan` covering the inner (already-stripped) range plus any nested
//!    spans shifted by the outer offset.
//!
//! Known limitations (per `docs/orgize_inline_audit.md`):
//! - Backslash escapes (`\*not bold\*`) are not honored by orgize
//!   0.10.0-alpha.10; the locked-in regression test asserts the current lossy
//!   behavior so a future orgize bump will surface the change.
//! - Sub/Super only match orgize's `_{…}` / `^{…}` form; bare `_{` is not a
//!   mark — that's correct Org behavior.

use holon_api::EntityRef;
use holon_api::EntityUri;
use holon_api::InlineMark;
use holon_api::MarkClass;
use holon_api::MarkSpan;
use holon_api::link_parser::LinkTarget;
use holon_api::link_parser::LinkTargetClassifier;
use orgize::ParseConfig;
use orgize::SyntaxKind;
use orgize::SyntaxNode;
use orgize::config::UseSubSuperscript;
use orgize::rowan::NodeOrToken;
use orgize::rowan::ast::AstNode;
use uuid::Uuid;

/// Parse `text` as inline org content. Returns `(rendered_text, marks)` where
/// `rendered_text` has all mark delimiters stripped and `marks` carries
/// Unicode-scalar offsets into the rendered text.
///
/// Parses with `use_sub_superscript: Brace` (the org `#+OPTIONS: ^:{}`
/// semantics): only the braced `_{…}` / `^{…}` forms are sub/superscript
/// marks. A bare `_` in `focused_block` or a lone `^` is literal text — the
/// default orgize setting (`True`) parses those as subscripts and the Sub/Super
/// `strip_prefix_suffix(raw, 2, 1)` (which assumes the braced shape) then
/// destroys the surrounding characters (`focused_block` → `focusedloc`). Brace
/// mode is the only shape the emit path can round-trip losslessly, and it is
/// what this module's contract has always documented.
pub fn extract_inline_marks(text: &str) -> (String, Vec<MarkSpan>) {
    extract_inline_marks_with(text, &LinkTargetClassifier::default())
}

/// [`extract_inline_marks`] against an explicit classifier — the form every
/// PRODUCTION parse boundary uses, so link targets are classified against the
/// entity schemes actually registered rather than the built-ins alone.
pub fn extract_inline_marks_with(
    text: &str,
    classifier: &LinkTargetClassifier,
) -> (String, Vec<MarkSpan>) {
    let mut state = ExtractState {
        classifier: classifier.clone(),
        ..Default::default()
    };
    walk_node(&parse_inline(text), &mut state);
    (state.out, state.marks)
}

fn parse_inline(text: &str) -> SyntaxNode {
    let config = ParseConfig {
        use_sub_superscript: UseSubSuperscript::Brace,
        ..Default::default()
    };
    config.parse(text).document().syntax().clone()
}

/// Delimiters tried, in order, to quote a markup-shaped literal. `=` is
/// Verbatim, `~` is Code; org parses no emphasis inside either.
const QUOTE_DELIMS: [char; 2] = ['=', '~'];

/// Emit `content` + `marks` as the org bytes that belong on disk, quoting
/// every literal span org would otherwise eat as emphasis.
///
/// The contract — the store→disk invariant this crate must not violate — is
/// stated by [`expected_reparse`]: parsing the returned bytes reproduces every
/// literal byte of `content`, with raw link syntax allowed to adopt into a
/// `Link` mark (that adoption is intended product behavior, not loss) UNLESS a
/// protective or data-bearing mark already covers it.
///
/// The check is TOTAL: it runs on the fully emitted string for every input,
/// marks or no marks, with no shape short-circuiting it. `Err` means the
/// content defeats both quote delimiters; the caller must disclose rather than
/// write bytes known to be lossy.
pub fn render_lossless(content: &str, marks: &[MarkSpan]) -> anyhow::Result<String> {
    render_expecting(content, marks, &expected_reparse(content, marks))
}

/// [`render_lossless`] with the expectation supplied separately, so a degraded
/// emission can be judged against what the block ORIGINALLY meant.
///
/// This separation is the whole point. Degrading changes `emit_marks`, and
/// `expected_reparse` reads marks — so re-deriving the expectation from the
/// reduced set would make every degradation self-justifying. Dropping a
/// `Verbatim` that seals `[[uuid][Label]]`, for instance, also drops the reason
/// that span must not adopt, and the check would wave through the very link
/// adoption the mark existed to prevent.
pub(crate) fn render_expecting(
    content: &str,
    emit_marks: &[MarkSpan],
    expected: &str,
) -> anyhow::Result<String> {
    for candidate in render_candidates(content, emit_marks) {
        if extract_inline_marks(&candidate).0 == expected {
            return Ok(candidate);
        }
    }
    anyhow::bail!(
        "no quote delimiter in {QUOTE_DELIMS:?} renders content {content:?} (marks {emit_marks:?}) \
         back to {expected:?}"
    )
}

/// The emissions this module can produce for `(content, emit_marks)`, one per
/// quote delimiter, in preference order and WITHOUT any verification.
///
/// Exposed unverified because the caller has two independent things to check —
/// that an emission preserves the content, and that it is a fixed point — and
/// a candidate can satisfy the second while failing the first. Collapsing them
/// here would hide the emissions that settle but lose content, which are the
/// only thing standing between an unrepresentable block and an echo loop.
pub(crate) fn render_candidates(content: &str, emit_marks: &[MarkSpan]) -> Vec<String> {
    let emit_marks = drop_duplicate_protective_marks(emit_marks);
    let spans = quotable_markup_spans(content, &emit_marks);
    QUOTE_DELIMS
        .iter()
        .map(|delim| {
            let (quoted, shifted) = quote_spans(content, &spans, *delim, &emit_marks);
            render_inline_marks(&quoted, &shifted)
        })
        .collect()
}

/// The parse-back form the render half is contractually required to produce —
/// THE contract, in one function, so tests can assert against it directly
/// rather than restating it (and drifting from it).
///
/// The ONLY transformation allowed between stored content and re-parsed
/// content is **link adoption**: raw `[[…]]` syntax sitting in plain content
/// becomes a `Link` mark, and the content keeps just the label. Everything
/// else — emphasis-shaped text above all — must come back byte-identical.
///
/// Adoption is suppressed inside any span already covered by a protective or
/// data-bearing mark, because for those spans the store has ALREADY said what
/// the bytes mean. A `Verbatim` over `[[uuid][Label]]` says "this is literal
/// text"; letting it adopt would silently convert a documented example into a
/// live link to a page that does not exist. A `Link`'s own span holds its
/// label, which org never re-parses.
pub fn expected_reparse(content: &str, marks: &[MarkSpan]) -> String {
    let sealed = sealed_char_ranges(marks);
    let mut out = String::with_capacity(content.len());
    let mut cursor = 0usize;
    for (range, adopted) in link_adoptions(content) {
        let start = content[..range.start].chars().count();
        let end = content[..range.end].chars().count();
        if sealed.iter().any(|(s, e)| *s <= start && end <= *e) {
            continue;
        }
        out.push_str(&content[cursor..range.start]);
        out.push_str(&adopted);
        cursor = range.end;
    }
    out.push_str(&content[cursor..]);
    out
}

/// Collapse repeated identical PROTECTIVE marks to one.
///
/// "This span is literal" said twice says nothing more than said once, and the
/// doubled delimiters do not survive: `==x==` re-parses to the content `=x=`,
/// gaining bytes. Styling marks are left alone — a repeated `Underline` is how
/// org encodes the doubled `__x__` form, where the second pair is real syntax
/// rather than a restatement.
fn drop_duplicate_protective_marks(marks: &[MarkSpan]) -> Vec<MarkSpan> {
    let mut seen: Vec<&MarkSpan> = Vec::new();
    let mut kept = Vec::with_capacity(marks.len());
    for m in marks {
        let redundant = m.mark.class() == MarkClass::Protective
            && seen
                .iter()
                .any(|s| s.start == m.start && s.end == m.end && s.mark == m.mark);
        if !redundant {
            seen.push(m);
            kept.push(m.clone());
        }
    }
    kept
}

/// Char ranges the store has already assigned a meaning to, so the renderer
/// must not let the parser reinterpret them: protective marks (the span is
/// literal) and data-bearing marks (the span is a link's label).
fn sealed_char_ranges(marks: &[MarkSpan]) -> Vec<(usize, usize)> {
    marks
        .iter()
        .filter(|m| m.mark.class() != MarkClass::Styling)
        .map(|m| (m.start, m.end))
        .collect()
}

/// Every raw link literal in `content`, as `(source byte range, adopted
/// label)` — what that link would contribute to the content if the parser
/// were allowed to adopt it.
fn link_adoptions(content: &str) -> Vec<(std::ops::Range<usize>, String)> {
    let mut found = Vec::new();
    collect_link_nodes(&parse_inline(content), &mut found);
    found
        .into_iter()
        .map(|range| {
            let adopted = extract_inline_marks(&content[range.clone()]).0;
            (range, adopted)
        })
        .collect()
}

fn collect_link_nodes(node: &SyntaxNode, found: &mut Vec<std::ops::Range<usize>>) {
    for child in node.children() {
        match inline_mark_kind(child.kind()) {
            Some(MarkKindHint::Link) => {
                let range = child.text_range();
                found.push(usize::from(range.start())..usize::from(range.end()));
            }
            // An emphasis node's bytes stay literal, so a link nested inside
            // one never adopts either.
            Some(_) => {}
            None => collect_link_nodes(&child, found),
        }
    }
}

/// Quote each span of `content` in `delim`, returning the quoted text and
/// `marks` re-anchored onto it.
///
/// A quote inserts one char at the span's start and one at its end. A mark
/// boundary moves past every insertion before it, and past a span-CLOSE that
/// lands exactly on it — so a quote opening where a mark opens sits inside the
/// mark, and a quote closing where a mark closes sits inside it too. Nested
/// that way, both delimiter pairs strip cleanly on re-parse.
fn quote_spans(
    content: &str,
    spans: &[std::ops::Range<usize>],
    delim: char,
    marks: &[MarkSpan],
) -> (String, Vec<MarkSpan>) {
    let mut quoted = String::with_capacity(content.len() + spans.len() * 2);
    let mut opens: Vec<usize> = Vec::with_capacity(spans.len());
    let mut closes: Vec<usize> = Vec::with_capacity(spans.len());
    let mut cursor = 0usize;
    for span in spans {
        quoted.push_str(&content[cursor..span.start]);
        quoted.push(delim);
        quoted.push_str(&content[span.clone()]);
        quoted.push(delim);
        cursor = span.end;
        opens.push(content[..span.start].chars().count());
        closes.push(content[..span.end].chars().count());
    }
    quoted.push_str(&content[cursor..]);

    let shift = |pos: usize| -> usize {
        let before = opens
            .iter()
            .chain(closes.iter())
            .filter(|p| **p < pos)
            .count();
        let closing_here = closes.iter().filter(|p| **p == pos).count();
        pos + before + closing_here
    };
    let shifted = marks
        .iter()
        .map(|m| MarkSpan {
            start: shift(m.start),
            end: shift(m.end),
            mark: m.mark.clone(),
        })
        .collect();
    (quoted, shifted)
}

/// The markup-shaped spans of `content` that still need quoting: everything
/// [`markup_source_spans`] found, minus whatever a non-styling mark already
/// covers.
///
/// Both exclusions are necessary, for different reasons:
/// - PROTECTIVE marks emit their own `=…=` / `~…~`, so quoting inside one
///   doubles the delimiters and corrupts the span.
/// - DATA-BEARING marks hold a link label, and org parses no emphasis inside a
///   link label. Quoting there adds `=` bytes to the LABEL, which then fails
///   the check and costs the block its URL on the degraded rung.
fn quotable_markup_spans(content: &str, marks: &[MarkSpan]) -> Vec<std::ops::Range<usize>> {
    let sealed = sealed_char_ranges(marks);
    markup_source_spans(content)
        .into_iter()
        .filter(|span| {
            let start = content[..span.start].chars().count();
            let end = content[..span.end].chars().count();
            !sealed.iter().any(|(s, e)| *s <= start && end <= *e)
        })
        .collect()
}

/// Byte ranges of the outermost emphasis-markup nodes org finds in `text`, in
/// source order. Uses the same parser and config as [`extract_inline_marks`] —
/// there is exactly one emphasis grammar in this crate, and it is orgize's.
fn markup_source_spans(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut spans = Vec::new();
    collect_markup_spans(&parse_inline(text), &mut spans);
    spans
}

fn collect_markup_spans(node: &SyntaxNode, spans: &mut Vec<std::ops::Range<usize>>) {
    for child in node.children() {
        match inline_mark_kind(child.kind()) {
            // Link delimiters carry the target; quoting them would turn a link
            // into literal text. Their subtree is not descended into for the
            // same reason — see `expected_reparse`, which lets links adopt.
            Some(MarkKindHint::Link) => {}
            Some(_) => {
                let range = child.text_range();
                spans.push(usize::from(range.start())..usize::from(range.end()));
            }
            None => collect_markup_spans(&child, spans),
        }
    }
}

#[derive(Default)]
struct ExtractState {
    /// Classifies each `[[…]]` target. Owned (a cheap `Option<Arc<…>>` clone)
    /// rather than borrowed, so this stays `Default` and picks up main's
    /// `keep_emphasis_raw` field without a lifetime rippling through
    /// `walk_node` / `emit_mark` / `push_with_inner_marks`.
    classifier: LinkTargetClassifier,
    out: String,
    marks: Vec<MarkSpan>,
    char_pos: usize,
    /// Emit emphasis nodes as their literal source bytes and mint no mark —
    /// the [`expected_reparse`] policy. Links still adopt.
    keep_emphasis_raw: bool,
}

fn walk_node(node: &SyntaxNode, state: &mut ExtractState) {
    for child in node.children_with_tokens() {
        match child {
            NodeOrToken::Node(child_node) => match inline_mark_kind(child_node.kind()) {
                Some(kind_hint) => {
                    if state.keep_emphasis_raw && kind_hint != MarkKindHint::Link {
                        let raw = child_node.text().to_string();
                        state.char_pos += raw.chars().count();
                        state.out.push_str(&raw);
                    } else {
                        emit_mark(child_node, kind_hint, state);
                    }
                }
                None => walk_node(&child_node, state),
            },
            NodeOrToken::Token(tok) => {
                scan_text_for_block_refs(tok.text(), state);
            }
        }
    }
}

/// Scan `text` for `((uuid))` block-ref patterns. Valid UUIDs become
/// `InlineMark::Link` with `EntityRef::Internal`; non-UUID `((...))` is
/// emitted as plain text.
fn scan_text_for_block_refs(text: &str, state: &mut ExtractState) {
    let mut pos = 0usize;
    while let Some(open) = text[pos..].find("((") {
        let abs_open = pos + open;
        // Emit plain text before the `((`.
        if open > 0 {
            let before = &text[pos..abs_open];
            state.out.push_str(before);
            state.char_pos += before.chars().count();
        }
        // Look for `))` after the `((`.
        let after_open = &text[abs_open + 2..];
        if let Some(close) = after_open.find("))") {
            let inner = after_open[..close].trim();
            let abs_close = abs_open + 2 + close + 2;
            if !inner.is_empty() && Uuid::parse_str(inner).is_ok() {
                let label = format!("(({inner}))");
                let mark = InlineMark::Link {
                    target: EntityRef::Internal {
                        id: EntityUri::block(inner),
                    },
                    label: label.clone(),
                };
                push_with_inner_marks(state, &label, vec![], mark);
            } else {
                // Emit the full `((...))` as plain text (non-UUID or empty).
                let full = &text[abs_open..abs_close];
                state.out.push_str(full);
                state.char_pos += full.chars().count();
            }
            pos = abs_close;
        } else {
            // No closing `))` — emit `((` as plain text and continue.
            state.out.push_str("((");
            state.char_pos += 2;
            pos = abs_open + 2;
        }
    }
    // Emit remaining text.
    let remainder = &text[pos..];
    if !remainder.is_empty() {
        state.out.push_str(remainder);
        state.char_pos += remainder.chars().count();
    }
}

/// Discriminator for how to strip delimiters and what `InlineMark` to emit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MarkKindHint {
    Bold,
    Italic,
    Underline,
    Verbatim,
    Code,
    Strike,
    Sub,
    Super,
    Link,
}

fn inline_mark_kind(kind: SyntaxKind) -> Option<MarkKindHint> {
    Some(match kind {
        SyntaxKind::BOLD => MarkKindHint::Bold,
        SyntaxKind::ITALIC => MarkKindHint::Italic,
        SyntaxKind::UNDERLINE => MarkKindHint::Underline,
        SyntaxKind::VERBATIM => MarkKindHint::Verbatim,
        SyntaxKind::CODE => MarkKindHint::Code,
        SyntaxKind::STRIKE => MarkKindHint::Strike,
        SyntaxKind::SUBSCRIPT => MarkKindHint::Sub,
        SyntaxKind::SUPERSCRIPT => MarkKindHint::Super,
        SyntaxKind::LINK => MarkKindHint::Link,
        _ => return None,
    })
}

fn emit_mark(node: SyntaxNode, kind_hint: MarkKindHint, state: &mut ExtractState) {
    let raw = node.text().to_string();

    match kind_hint {
        MarkKindHint::Link => {
            let (text, mark) = strip_link(&raw, &state.classifier);
            // Empty link (`[[]]` / `[[][]]`): the rendered label is empty, so
            // the mark would span zero characters (start == end). A zero-width
            // Link mark is an illegal state — it has no visible content and no
            // meaningful target, and it renders back as reversed brackets
            // (`]][[`), the on-disk corruption confirmed by dogfood #4. Parse,
            // don't validate: drop it at the boundary so it is never created,
            // emitting no mark and no content for the empty literal.
            if text.is_empty() {
                return;
            }
            push_with_inner_marks(state, &text, vec![], mark);
        }
        MarkKindHint::Sub | MarkKindHint::Super => {
            // SUBSCRIPT / SUPERSCRIPT: `_{…}` / `^{…}` — strip 2-char prefix + 1-char
            // suffix. No nested marks supported in sub/super for Phase 1 (rare
            // in practice).
            let inner = strip_prefix_suffix(&raw, 2, 1);
            let mark = match kind_hint {
                MarkKindHint::Sub => InlineMark::Sub,
                MarkKindHint::Super => InlineMark::Super,
                _ => unreachable!(),
            };
            push_with_inner_marks(state, &inner, vec![], mark);
        }
        MarkKindHint::Verbatim | MarkKindHint::Code => {
            // Org verbatim/code objects are literal: they cannot contain other
            // objects, so the inner text is emitted verbatim with no recursion
            // (otherwise `=a *b* c=` would strip the user's literal asterisks).
            let inner = strip_prefix_suffix(&raw, 1, 1);
            let mark = match kind_hint {
                MarkKindHint::Verbatim => InlineMark::Verbatim,
                MarkKindHint::Code => InlineMark::Code,
                _ => unreachable!(),
            };
            push_with_inner_marks(state, &inner, vec![], mark);
        }
        _ => {
            // BOLD/ITALIC/UNDERLINE/STRIKE: 1-char delimiter each side.
            let inner = strip_prefix_suffix(&raw, 1, 1);
            // Recurse into the inner string for nested marks. orgize re-parses
            // the substring fresh; nested mark offsets are scalar offsets
            // within `inner`, ready to be shifted by the outer start.
            let (nested_text, nested_marks) = extract_inline_marks_with(&inner, &state.classifier);
            // The text from recursion may differ from `inner` if it had nested
            // marks (delimiters were stripped). Use nested_text as the actual
            // emitted content.
            let outer_mark = match kind_hint {
                MarkKindHint::Bold => InlineMark::Bold,
                MarkKindHint::Italic => InlineMark::Italic,
                MarkKindHint::Underline => InlineMark::Underline,
                MarkKindHint::Strike => InlineMark::Strike,
                _ => unreachable!(),
            };
            push_with_inner_marks(state, &nested_text, nested_marks, outer_mark);
        }
    }
}

/// Append `text` to `state.out`, shifting any `inner_marks` by the current
/// char position, then emit `outer_mark` covering the full appended range.
fn push_with_inner_marks(
    state: &mut ExtractState,
    text: &str,
    inner_marks: Vec<MarkSpan>,
    outer_mark: InlineMark,
) {
    let start = state.char_pos;
    state.out.push_str(text);
    state.char_pos += text.chars().count();
    let end = state.char_pos;

    state.marks.push(MarkSpan::new(start, end, outer_mark));
    for span in inner_marks {
        state.marks.push(MarkSpan::new(
            start + span.start,
            start + span.end,
            span.mark,
        ));
    }
}

/// Strip `prefix_chars` characters off the front and `suffix_chars` off the
/// back of `s`, counting Unicode scalars (not bytes). Returns the original
/// string if it's too short to strip.
fn strip_prefix_suffix(s: &str, prefix_chars: usize, suffix_chars: usize) -> String {
    let mut chars: Vec<char> = s.chars().collect();
    if chars.len() < prefix_chars + suffix_chars {
        return s.to_string();
    }
    chars.drain(..prefix_chars);
    chars.truncate(chars.len() - suffix_chars);
    chars.into_iter().collect()
}

/// Parse a `[[…][…]]` or `[[…]]` link literal. Returns `(rendered_label, Link
/// mark)`.
///
/// - `[[uri][label]]` → label is the rendered text; uri is classified into
///   `EntityRef`.
/// - `[[uri]]` (bare) → rendered text is the uri itself; classified the same
///   way.
fn strip_link(raw: &str, classifier: &LinkTargetClassifier) -> (String, InlineMark) {
    // Strip outer `[[` and `]]`.
    let inside = raw
        .strip_prefix("[[")
        .and_then(|s| s.strip_suffix("]]"))
        .unwrap_or(raw);
    // Split on `][` to separate uri and label, if present. Link labels must not
    // carry leading/trailing whitespace (product rule): `[[a ]]` displays as `a`,
    // not `a ` — so the extracted label (the content emitted for this link) is
    // trimmed at the parse boundary, keeping every projection (Loro CONTENT_RAW /
    // editor cell, SQL block_raw, org re-render) consistent. The no-explicit-label
    // form `[[a ]]` uses the same trimmed string as both display and target.
    let (uri, label) = match inside.split_once("][") {
        Some((u, l)) => (u.to_string(), l.trim().to_string()),
        None => {
            let t = inside.trim().to_string();
            (t.clone(), t)
        }
    };
    let target = match classifier.classify(&uri) {
        LinkTarget::External(s) => EntityRef::External { url: s },
        LinkTarget::Resolved(uri) => EntityRef::Internal { id: uri },
        LinkTarget::UnknownScheme(uri) => EntityRef::UnknownScheme { uri },
        // Links increment 2: a wiki-name target stays DANGLING (`Name`) — no
        // deterministic-id minting into the mark at parse time. Pages are
        // created lazily; the exact target string (possibly a `parent/leaf`
        // suffix-resolution chain) is preserved for `block_links` resolution
        // and byte-stable re-render (`[[name]]` / `[[name][label]]`).
        LinkTarget::CreationIntent { path, .. } => EntityRef::Name { name: path },
    };
    let mark = InlineMark::Link {
        target,
        label: label.clone(),
    };
    (label, mark)
}

// =============================================================================
// Renderer: marks → org syntax (inverse of `extract_inline_marks`)
// =============================================================================

/// Render `text` with `marks` back to Org syntax. Mirror of
/// [`extract_inline_marks`] for the round-trip.
///
/// For each mark, emits the appropriate Org delimiters (`*…*`, `/…/`, `=…=`,
/// `~…~`, `+…+`, `_…_`, `_{…}`, `^{…}`, `[[uri][label]]`) at the mark's
/// scalar boundaries. Mark events at the same position are ordered so that
/// outer (longer) marks open first and close last — this produces correct
/// nested output like `*bold _under_*` for properly-nested marks.
///
/// **Overlap policy**: marks that *cross* (`A.start < B.start < A.end < B.end`)
/// cannot be represented in Org without nesting changes. The renderer emits
/// them best-effort by treating each event in order; the result may not
/// round-trip cleanly back to the same mark set. Phase 1 logs a tracing
/// warning when crossing is detected so callers see the lossy case.
pub fn render_inline_marks(text: &str, marks: &[MarkSpan]) -> String {
    use std::collections::BTreeMap;

    // A zero-length span (start == end) wraps no content: it has nothing to
    // delimit. Emitting its events at a single position would push the CLOSE
    // delimiter before the OPEN (`emit_events` closes-then-opens), producing
    // reversed output like `]][[` for an empty link (the on-disk corruption
    // class from dogfood #4, which then COMPOUNDS across writeback cycles).
    // Such marks carry nothing, so dropping them at render loses nothing and
    // is the inverse of parsing: `extract_inline_marks` never yields them and
    // `canonicalize_marks` strips them at every read boundary — this is the
    // final safety net so no zero-width mark can ever reach disk.
    let marks: Vec<&MarkSpan> = marks.iter().filter(|m| m.start != m.end).collect();

    if marks.is_empty() {
        return text.to_string();
    }

    detect_crossing_marks(marks.iter().copied());

    // Bucket events by char position, then order them so delimiters nest
    // strictly LIFO — every close is the mirror of the most recent open.
    // `nesting_key` is one total order over the marks; opens ascend it
    // (outermost first) and closes descend it (innermost first), so a
    // non-LIFO emit like `*=x*=` is unrepresentable by construction.
    let mut opens_at: BTreeMap<usize, Vec<&MarkSpan>> = BTreeMap::new();
    let mut closes_at: BTreeMap<usize, Vec<&MarkSpan>> = BTreeMap::new();
    for &m in &marks {
        opens_at.entry(m.start).or_default().push(m);
        closes_at.entry(m.end).or_default().push(m);
    }
    let key = |m: &MarkSpan| nesting_key(m, &marks);
    for v in opens_at.values_mut() {
        v.sort_by_key(|m| key(m));
    }
    for v in closes_at.values_mut() {
        v.sort_by_key(|m| std::cmp::Reverse(key(m)));
    }

    let mut out = String::with_capacity(text.len() + marks.len() * 4);
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();

    let emit_events = |pos: usize, out: &mut String| {
        if let Some(v) = closes_at.get(&pos) {
            for m in v {
                out.push_str(&close_delim(&m.mark));
            }
        }
        if let Some(v) = opens_at.get(&pos) {
            for m in v {
                out.push_str(&open_delim(&m.mark));
            }
        }
    };

    for (i, ch) in chars.iter().enumerate() {
        emit_events(i, &mut out);
        out.push(*ch);
    }
    // Closing events at the end-of-text position.
    emit_events(n, &mut out);

    out
}

/// Total order deciding which of two marks nests OUTSIDE the other. Ascending
/// = outermost first; a mark's close event is emitted in the exact reverse.
///
/// Beyond the obvious span ordering, two ties matter because org's own parser
/// is not symmetric about them:
/// - `Verbatim`/`Code` must sit INSIDE ordinary emphasis. They suppress
///   emphasis parsing, so `=*x*=` comes back as the literal content `*x*` — the
///   content would GAIN bytes. `*=x=*` comes back as `x` + both marks.
/// - `Link` must be innermost. Org does not parse emphasis inside a link label,
///   so an outer link swallows the emphasis delimiters into the label.
///
/// The final `position` tiebreak keeps the order total (and therefore the
/// output deterministic) for marks that are identical in every other respect.
fn nesting_key(m: &MarkSpan, all: &[&MarkSpan]) -> (usize, std::cmp::Reverse<usize>, u8, usize) {
    let depth = match m.mark {
        InlineMark::Link { .. } => 2,
        InlineMark::Verbatim | InlineMark::Code => 1,
        _ => 0,
    };
    let position = all
        .iter()
        .position(|other| std::ptr::eq(*other, m))
        .unwrap_or(0);
    (m.start, std::cmp::Reverse(m.end), depth, position)
}

/// Open delimiter for a mark. For Link, this is `[[uri][` (the label and
/// closing `]]` come at the close position). Block-refs (`((uuid))`) get
/// `((` as open and `))` as close.
fn open_delim(mark: &InlineMark) -> String {
    match mark {
        InlineMark::Bold => "*".into(),
        InlineMark::Italic => "/".into(),
        InlineMark::Underline => "_".into(),
        InlineMark::Verbatim => "=".into(),
        InlineMark::Code => "~".into(),
        InlineMark::Strike => "+".into(),
        InlineMark::Sub => "_{".into(),
        InlineMark::Super => "^{".into(),
        InlineMark::Link { target, label } => {
            if is_block_ref_link(mark) {
                return String::new();
            }
            let uri = match target {
                EntityRef::External { url } => url.clone(),
                // Bare form `[[uri]]` when the label IS the uri, as the user
                // typed it; explicit `[[uri][label]]` otherwise. Every target
                // variant shares this rule, so no id form gains a spurious
                // label on a round trip.
                EntityRef::Internal { id } => {
                    if id.as_str() == label {
                        return "[[".into();
                    }
                    id.as_str().to_string()
                }
                // Dangling wiki link: `[[name]]` when the label IS the name
                // (the bare form the user typed), `[[name][label]]` otherwise.
                EntityRef::Name { name } => {
                    if name == label {
                        return "[[".into();
                    }
                    name.clone()
                }
                // Bytes are preserved exactly: an unknown scheme must survive
                // an edit cycle untouched so registering its integration
                // restores the link.
                EntityRef::UnknownScheme { uri } => {
                    if uri == label {
                        return "[[".into();
                    }
                    uri.clone()
                }
            };
            format!("[[{uri}][")
        }
    }
}

fn close_delim(mark: &InlineMark) -> String {
    match mark {
        InlineMark::Bold => "*".into(),
        InlineMark::Italic => "/".into(),
        InlineMark::Underline => "_".into(),
        InlineMark::Verbatim => "=".into(),
        InlineMark::Code => "~".into(),
        InlineMark::Strike => "+".into(),
        InlineMark::Sub | InlineMark::Super => "}".into(),
        InlineMark::Link { .. } => {
            if is_block_ref_link(mark) {
                return String::new();
            }
            "]]".into()
        }
    }
}

/// Returns `true` when the mark is a block-ref link: `EntityRef::Internal`
/// whose label starts with `((`, ends with `))`, AND the inner text matches
/// the id (stripped of its `block:` scheme). This heuristically distinguishes
/// `((uuid))` from `[[block:uuid][label]]` for round-trip fidelity.
fn is_block_ref_link(mark: &InlineMark) -> bool {
    match mark {
        InlineMark::Link {
            target: EntityRef::Internal { id },
            label,
        } if label.starts_with("((") && label.ends_with("))") && label.len() > 4 => {
            let inner = &label[2..label.len() - 2];
            id.as_str()
                .strip_prefix("block:")
                .is_some_and(|uuid| inner.trim() == uuid)
        }
        _ => false,
    }
}

/// Log a tracing warning if `marks` contains crossing pairs (A.start <
/// B.start < A.end < B.end). Org can't represent crossing inline marks.
fn detect_crossing_marks<'a>(marks: impl Iterator<Item = &'a MarkSpan>) {
    let marks: Vec<&MarkSpan> = marks.collect();
    for (i, a) in marks.iter().enumerate() {
        for b in marks.iter().skip(i + 1) {
            if a.start < b.start && b.start < a.end && a.end < b.end {
                tracing::warn!(
                    "render_inline_marks: crossing marks detected — {a:?} crosses {b:?}; org \
                     output may be lossy"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extract(text: &str) -> (String, Vec<MarkSpan>) {
        extract_inline_marks(text)
    }

    #[test]
    fn bold_alone() {
        let (out, marks) = extract("*bold*");
        assert_eq!(out, "bold");
        assert_eq!(marks, vec![MarkSpan::new(0, 4, InlineMark::Bold)]);
    }

    #[test]
    fn italic_alone() {
        let (out, marks) = extract("/italic/");
        assert_eq!(out, "italic");
        assert_eq!(marks, vec![MarkSpan::new(0, 6, InlineMark::Italic)]);
    }

    #[test]
    fn underline_alone() {
        let (out, marks) = extract("_under_");
        assert_eq!(out, "under");
        assert_eq!(marks, vec![MarkSpan::new(0, 5, InlineMark::Underline)]);
    }

    #[test]
    fn verbatim_alone() {
        let (out, marks) = extract("=verbatim=");
        assert_eq!(out, "verbatim");
        assert_eq!(marks, vec![MarkSpan::new(0, 8, InlineMark::Verbatim)]);
    }

    #[test]
    fn code_alone() {
        let (out, marks) = extract("~code~");
        assert_eq!(out, "code");
        assert_eq!(marks, vec![MarkSpan::new(0, 4, InlineMark::Code)]);
    }

    #[test]
    fn strike_alone() {
        let (out, marks) = extract("+strike+");
        assert_eq!(out, "strike");
        assert_eq!(marks, vec![MarkSpan::new(0, 6, InlineMark::Strike)]);
    }

    #[test]
    fn sub_empty_braces_strip_to_empty_inner() {
        // `_{}` is the minimal SUBSCRIPT node (len == prefix+suffix in
        // strip_prefix_suffix); the braces must still be stripped.
        let (out, marks) = extract("a_{} b");
        assert_eq!(out, "a b");
        assert_eq!(marks, vec![MarkSpan::new(1, 1, InlineMark::Sub)]);
    }

    #[test]
    fn sub_strips_braces() {
        let (out, marks) = extract("a_{sub}");
        // `a` literal + Sub("sub")
        assert_eq!(out, "asub");
        assert_eq!(marks, vec![MarkSpan::new(1, 4, InlineMark::Sub)]);
    }

    #[test]
    fn super_strips_braces() {
        let (out, marks) = extract("a^{super}");
        assert_eq!(out, "asuper");
        assert_eq!(marks, vec![MarkSpan::new(1, 6, InlineMark::Super)]);
    }

    #[test]
    fn link_external_with_label() {
        let (out, marks) = extract("[[https://example.com][demo]]");
        assert_eq!(out, "demo");
        assert_eq!(marks.len(), 1);
        let MarkSpan { start, end, mark } = marks[0].clone();
        assert_eq!((start, end), (0, 4));
        match mark {
            InlineMark::Link { target, label } => {
                assert_eq!(label, "demo");
                match target {
                    EntityRef::External { url } => assert_eq!(url, "https://example.com"),
                    other => panic!("expected External, got {other:?}"),
                }
            }
            other => panic!("expected Link, got {other:?}"),
        }
    }

    #[test]
    fn link_bare_uses_uri_as_label() {
        let (out, marks) = extract("[[https://example.com]]");
        assert_eq!(out, "https://example.com");
        assert_eq!(marks.len(), 1);
        match &marks[0].mark {
            InlineMark::Link { target, label } => {
                assert_eq!(label, "https://example.com");
                match target {
                    EntityRef::External { url } => assert_eq!(url, "https://example.com"),
                    other => panic!("expected External, got {other:?}"),
                }
            }
            other => panic!("expected Link, got {other:?}"),
        }
    }

    #[test]
    fn link_internal_block_uri() {
        // `block:uuid` is a Resolved link target → Internal EntityRef.
        let (out, marks) = extract("[[block:abc-123][see also]]");
        assert_eq!(out, "see also");
        assert_eq!(marks.len(), 1);
        match &marks[0].mark {
            InlineMark::Link { target, label } => {
                assert_eq!(label, "see also");
                match target {
                    EntityRef::Internal { id } => {
                        assert_eq!(id.as_str(), "block:abc-123");
                    }
                    other => panic!("expected Internal, got {other:?}"),
                }
            }
            other => panic!("expected Link, got {other:?}"),
        }
    }

    #[test]
    fn link_bare_wiki_name_stays_dangling_and_byte_stable() {
        // Links increment 2: a bare wiki-name link is a DANGLING `Name`
        // target (no deterministic-id minting at parse) and re-renders
        // byte-identically as `[[name]]` until it resolves.
        let (out, marks) = extract("see [[Linked Page]] here");
        assert_eq!(out, "see Linked Page here");
        assert_eq!(marks.len(), 1);
        match &marks[0].mark {
            InlineMark::Link { target, label } => {
                assert_eq!(label, "Linked Page");
                match target {
                    EntityRef::Name { name } => assert_eq!(name, "Linked Page"),
                    other => panic!("expected dangling Name, got {other:?}"),
                }
            }
            other => panic!("expected Link, got {other:?}"),
        }
        let re = render_inline_marks(&out, &marks);
        assert_eq!(
            re, "see [[Linked Page]] here",
            "dangling bare form must be a fixed point"
        );
    }

    #[test]
    fn empty_link_is_dropped_at_extraction_no_zero_width_mark() {
        // `[[]]` and `[[][]]` have an empty label — the Link mark would span
        // zero characters. Parse, don't validate: the boundary drops them so a
        // zero-width Link mark is never created (the dogfood #4 `]][[` root).
        for input in ["[[]]", "[[][]]"] {
            let (out, marks) = extract(input);
            assert_eq!(out, "", "empty link `{input}` must leave no content");
            assert!(
                marks.is_empty(),
                "empty link `{input}` must produce no mark, got {marks:?}"
            );
        }
        // Surrounding text is preserved; only the empty link contributes nothing.
        let (out, marks) = extract("a[[]]b");
        assert_eq!(out, "ab");
        assert!(marks.is_empty(), "got {marks:?}");
    }

    #[test]
    fn render_zero_length_link_span_emits_nothing_never_reversed_brackets() {
        // A zero-width Link mark (e.g. left behind when an edit deletes a
        // link's entire text before `canonicalize_marks` strips it) must NOT
        // render as `]][[` (close-before-open). It carries no content, so it
        // renders to nothing.
        let zl = vec![MarkSpan::new(
            0,
            0,
            InlineMark::Link {
                target: EntityRef::Name {
                    name: String::new(),
                },
                label: String::new(),
            },
        )];
        assert_eq!(render_inline_marks("", &zl), "");
        assert_eq!(render_inline_marks("abcd", &zl), "abcd");

        // Mid-string zero-width span must not corrupt the surrounding text.
        let mid = vec![MarkSpan::new(
            2,
            2,
            InlineMark::Link {
                target: EntityRef::Name {
                    name: "Page".into(),
                },
                label: "Page".into(),
            },
        )];
        let out = render_inline_marks("abcd", &mid);
        assert_eq!(out, "abcd");
        assert!(!out.contains("]]["), "reversed brackets in {out:?}");

        // A zero-width mark alongside a real one: only the real one renders.
        let mixed = vec![
            MarkSpan::new(0, 4, InlineMark::Bold),
            MarkSpan::new(4, 4, InlineMark::Italic),
        ];
        assert_eq!(render_inline_marks("word", &mixed), "*word*");
    }

    #[test]
    fn empty_link_typed_round_trip_is_stable_no_doubling() {
        // The dogfood #4 compounding class: `[[]]` typed → disk → re-ingest →
        // disk must reach a fixed point WITHOUT growing `]][[` each cycle.
        let mut text = "[[]]".to_string();
        let mut marks: Vec<MarkSpan> = Vec::new();
        let mut seen = Vec::new();
        for _ in 0..5 {
            let on_disk = render_inline_marks(&text, &marks);
            assert!(
                !on_disk.contains("]]["),
                "reversed-bracket corruption on disk: {on_disk:?}"
            );
            seen.push(on_disk.clone());
            let (rt, sp) = extract(&on_disk);
            text = rt;
            marks = sp;
        }
        // Converged and never doubled: every disk iterate after the first is
        // identical (empty), so no growth across cycles.
        assert!(
            seen.iter().skip(1).all(|d| d == &seen[1]),
            "disk form not stable across cycles: {seen:?}"
        );
    }

    #[test]
    fn link_name_chain_with_label_round_trips() {
        // `parent/leaf` name chains are kept verbatim as the suffix
        // resolution hint; labelled form re-renders as `[[chain][label]]`.
        let (out, marks) = extract("[[Projects/Linked Page][the label]]");
        assert_eq!(out, "the label");
        match &marks[0].mark {
            InlineMark::Link { target, label } => {
                assert_eq!(label, "the label");
                match target {
                    EntityRef::Name { name } => assert_eq!(name, "Projects/Linked Page"),
                    other => panic!("expected dangling Name, got {other:?}"),
                }
            }
            other => panic!("expected Link, got {other:?}"),
        }
        let re = render_inline_marks(&out, &marks);
        assert_eq!(re, "[[Projects/Linked Page][the label]]");
        let (out2, marks2) = extract_inline_marks(&re);
        assert_eq!(
            (out2, marks2),
            (out, marks),
            "render∘extract must be a fixed point"
        );
    }

    #[test]
    fn nested_bold_underline() {
        // `*bold _under_*` → "bold under" with Bold@0..10, Underline@5..10.
        let (out, marks) = extract("*bold _under_*");
        assert_eq!(out, "bold under");
        // Marks come back in emit order: outer mark, then inner shifted.
        let bold = marks
            .iter()
            .find(|m| m.mark == InlineMark::Bold)
            .expect("bold present");
        let underline = marks
            .iter()
            .find(|m| m.mark == InlineMark::Underline)
            .expect("underline present");
        assert_eq!((bold.start, bold.end), (0, 10));
        assert_eq!((underline.start, underline.end), (5, 10));
    }

    #[test]
    fn two_adjacent_marks() {
        let (out, marks) = extract("*one* and /two/");
        assert_eq!(out, "one and two");
        let bold = marks.iter().find(|m| m.mark == InlineMark::Bold).unwrap();
        let italic = marks.iter().find(|m| m.mark == InlineMark::Italic).unwrap();
        assert_eq!((bold.start, bold.end), (0, 3));
        assert_eq!((italic.start, italic.end), (8, 11));
    }

    #[test]
    fn plain_text_no_marks() {
        let (out, marks) = extract("just plain text");
        assert_eq!(out, "just plain text");
        assert_eq!(marks, Vec::<MarkSpan>::new());
    }

    #[test]
    fn word_boundary_no_bold() {
        // orgize correctly enforces that `a*not bold*b` is plain text.
        let (out, marks) = extract("a*not bold*b");
        assert_eq!(out, "a*not bold*b");
        assert_eq!(marks, Vec::<MarkSpan>::new());
    }

    /// The render-half invariant, stated directly and totally: for ANY
    /// content, what `render_lossless` emits must parse back to
    /// `expected_reparse` of that content — every literal byte intact, links
    /// free to adopt.
    fn assert_lossless(content: &str, marks: &[MarkSpan]) -> String {
        let emitted = render_lossless(content, marks)
            .unwrap_or_else(|e| panic!("render_lossless({content:?}, {marks:?}) bailed: {e}"));
        assert_eq!(
            extract_inline_marks(&emitted).0,
            expected_reparse(content, marks),
            "content {content:?} marks {marks:?} emitted {emitted:?}"
        );
        emitted
    }

    #[test]
    fn markup_shaped_literals_survive_render() {
        for literal in [
            "__default__",
            "_default_",
            "*bold-looking*",
            "/italic-looking/",
            "~tilde-looking~",
            // Content that already ends in the quote delimiter: the quote has
            // to survive `==x==`, which org reads as verbatim `=x=`.
            "=verbatim-looking=",
            "+strike-looking+",
            "the __default__ profile is used",
            "a =b= and __c__ together",
            "_{braced-sub}",
            // No markup: must pass through byte-identically, no stray quotes.
            "plain words only",
            "snake_case_ident and a_b_c",
            "",
        ] {
            assert_eq!(
                extract_inline_marks(&assert_lossless(literal, &[])).0,
                literal
            );
        }
    }

    /// Emphasis mixed with raw link syntax. The link ADOPTS (its `[[…]]` bytes
    /// become a Link mark — intended product behavior); the emphasis-shaped
    /// literal must not.
    #[test]
    fn link_mixed_with_emphasis_keeps_the_emphasis_literal() {
        for content in [
            "a *b* and [[c]]",
            "[[https://e.com][l]] and __y__",
            "[[block:xyz][lbl]] and __y__",
            "see [[foo]] here",
            "text with [[link]] only",
            "footnote [fn:1] and *x*",
        ] {
            assert_lossless(content, &[]);
        }
    }

    /// A link's own label bytes survive: org does not parse emphasis inside a
    /// link label, so `__lbl__` comes back whole once the link adopts.
    #[test]
    fn link_label_bytes_survive_adoption() {
        let emitted = assert_lossless("[[https://example.com][__lbl__]]", &[]);
        assert_eq!(extract_inline_marks(&emitted).0, "__lbl__");
    }

    /// A block that ALREADY carries marks still has literal gaps between them,
    /// and those gaps are exposed to exactly the same loss.
    #[test]
    fn literal_gaps_between_marks_survive() {
        // `bold` is marked; the `__y__` sitting after it is a literal.
        let marks = vec![MarkSpan {
            start: 0,
            end: 4,
            mark: InlineMark::Bold,
        }];
        let emitted = assert_lossless("bold and __y__ after", &marks);
        let (text, back) = extract_inline_marks(&emitted);
        assert_eq!(text, "bold and __y__ after");
        assert!(
            back.iter().any(|m| m.mark == InlineMark::Bold),
            "the Bold mark must survive too; got {back:?}"
        );
    }

    /// The shape this fix MINTS: a healed block comes back with a Verbatim
    /// mark, and a later edit appends new literal markup next to it. That
    /// block now takes the marks-present path and must still be lossless.
    #[test]
    fn healed_block_extended_with_new_literal_markup_survives() {
        let marks = vec![MarkSpan {
            start: 0,
            end: 11,
            mark: InlineMark::Verbatim,
        }];
        assert_lossless("__default__ and __init__", &marks);
    }

    /// Marks minted outside the org parser (Peritext reads, block-split,
    /// templates) carry no guarantee that their content is org-clean.
    #[test]
    fn externally_minted_mark_over_markup_shaped_text_survives() {
        // Italic over `the`, with a markup-shaped literal beside it.
        let marks = vec![MarkSpan {
            start: 0,
            end: 3,
            mark: InlineMark::Italic,
        }];
        assert_lossless("the __default__ profile", &marks);
        // Italic over the whole literal: the quote nests INSIDE the emphasis
        // delimiters (`/=__default__=/`), so both strip cleanly.
        let marks = vec![MarkSpan {
            start: 4,
            end: 15,
            mark: InlineMark::Italic,
        }];
        assert_lossless("the __default__ profile", &marks);
    }

    /// A mark that CROSSES a markup-shaped literal — covering part of it — is
    /// not expressible in org at all (the emphasis delimiter would land inside
    /// the quote). The render half must say so, not emit corrupt bytes: before
    /// this check existed the emit was `the /__default/__ profile`, which
    /// re-parses with two stray `/` in the content.
    #[test]
    fn mark_crossing_a_markup_literal_is_refused_not_corrupted() {
        let marks = vec![MarkSpan {
            start: 4,
            end: 13,
            mark: InlineMark::Italic,
        }];
        let err = render_lossless("the __default__ profile", &marks)
            .expect_err("a crossing mark is unrepresentable and must be refused");
        let msg = format!("{err}");
        assert!(
            msg.contains("__default__"),
            "error must name the content: {msg}"
        );
    }

    #[test]
    fn backslash_escape_lossy_regression() {
        // Phase 0.3 audit finding: orgize 0.10.0-alpha.10 does NOT honor
        // `\*…\*` escapes — the `\` is included in the BOLD range. This test
        // locks the current lossy behavior; a future orgize bump that fixes
        // this will fail this test as a signal to revisit the docs.
        let (out, marks) = extract("\\*not bold\\*");
        // Bold mark should still be produced (lossy), with `\` chars present
        // in the inner text.
        assert!(
            marks.iter().any(|m| m.mark == InlineMark::Bold),
            "expected lossy Bold mark to be emitted; got {marks:?}"
        );
        // Output retains the inner content including the trailing `\`.
        assert!(out.contains("not bold"), "got {out:?}");
    }

    #[test]
    fn multibyte_unicode_offsets_are_scalar() {
        // 你好 = 2 chars but 6 bytes in UTF-8. Bold over a multi-byte word
        // must produce scalar offsets, not byte offsets.
        let (out, marks) = extract("*你好* world");
        assert_eq!(out, "你好 world");
        let bold = marks.iter().find(|m| m.mark == InlineMark::Bold).unwrap();
        // 你好 is 2 scalars wide. Mark covers [0..2).
        assert_eq!((bold.start, bold.end), (0, 2));
    }

    // -- Renderer tests (inverse / round-trip) ---------------------------

    fn round_trip(text: &str) -> (String, Vec<MarkSpan>) {
        let (rendered_text, marks) = extract_inline_marks(text);
        let re_org = render_inline_marks(&rendered_text, &marks);
        // The re-emitted org should re-parse to the same (text, marks).
        let (text2, marks2) = extract_inline_marks(&re_org);
        assert_eq!(rendered_text, text2, "text differs after round-trip");
        assert_eq!(marks, marks2, "marks differ after round-trip");
        (re_org, marks)
    }

    #[test]
    fn render_bold_round_trip() {
        let (org, _) = round_trip("*bold*");
        assert_eq!(org, "*bold*");
    }

    #[test]
    fn render_italic_round_trip() {
        let (org, _) = round_trip("/italic/");
        assert_eq!(org, "/italic/");
    }

    #[test]
    fn render_link_external_round_trip() {
        let (org, _) = round_trip("[[https://example.com][demo]]");
        assert_eq!(org, "[[https://example.com][demo]]");
    }

    #[test]
    fn render_sub_round_trip() {
        let (org, _) = round_trip("a_{sub}");
        assert_eq!(org, "a_{sub}");
    }

    #[test]
    fn render_super_round_trip() {
        let (org, _) = round_trip("a^{super}");
        assert_eq!(org, "a^{super}");
    }

    #[test]
    fn render_two_adjacent_round_trip() {
        let (org, _) = round_trip("*one* and /two/");
        assert_eq!(org, "*one* and /two/");
    }

    #[test]
    fn render_nested_bold_underline_round_trip() {
        let (org, _) = round_trip("*bold _under_*");
        assert_eq!(org, "*bold _under_*");
    }

    #[test]
    fn render_plain_text_passthrough() {
        let out = render_inline_marks("just plain text", &[]);
        assert_eq!(out, "just plain text");
    }

    #[test]
    fn render_multibyte_unicode() {
        let marks = vec![MarkSpan::new(0, 2, InlineMark::Bold)];
        let out = render_inline_marks("你好 world", &marks);
        assert_eq!(out, "*你好* world");
    }

    #[test]
    fn render_link_internal() {
        // Internal block link round-trip.
        let (org, _) = round_trip("[[block:abc-123][see also]]");
        assert_eq!(org, "[[block:abc-123][see also]]");
    }

    /// Regression for the vault data-loss bug: bare (unbraced) `_`/`^` in
    /// snake_case identifiers must NOT be parsed as sub/superscript, so they
    /// survive the extract→render round-trip byte-for-byte. Before the
    /// `UseSubSuperscript::Brace` fix, orgize parsed `focused_block` as
    /// `focused` + subscript `_block`, and the braced-form strip mangled it to
    /// `focusedloc`. Each string here is a real token from the user's PKM
    /// vault.
    #[test]
    fn bare_underscore_identifiers_survive_round_trip() {
        for input in [
            "focused_block",
            "set_focus_with_caret",
            "virtual_parent",
            "sort_key",
            "keyed_rows_signal_vec",
            "watch_changes_since",
            "change_set.rs",
            "vector_distance",
            "model_version",
            "a_b_c",
            "jxa_sbzys",
            // bare superscripts too
            "E=mc^2",
            "x^y_z",
        ] {
            let (text, marks) = extract_inline_marks(input);
            assert!(
                marks.is_empty(),
                "bare `_`/`^` must not produce marks: input={input:?} text={text:?} \
                 marks={marks:?}",
            );
            assert_eq!(
                text, input,
                "ingest must preserve bare-underscore identifier"
            );
            let rendered = render_inline_marks(&text, &marks);
            assert_eq!(rendered, input, "round-trip must be identity for {input:?}");
        }
    }

    /// The braced sub/superscript forms remain real marks and round-trip.
    #[test]
    fn braced_sub_super_still_marks() {
        let (text, marks) = extract_inline_marks("a_{sub} b^{sup}");
        assert_eq!(text, "asub bsup");
        assert_eq!(
            marks,
            vec![
                MarkSpan::new(1, 4, InlineMark::Sub),
                MarkSpan::new(6, 9, InlineMark::Super),
            ]
        );
        assert_eq!(render_inline_marks(&text, &marks), "a_{sub} b^{sup}");
    }

    // -- Block-ref `((uuid))` tests ----------------------------------------

    const BLOCK_REF_UUID: &str = "550e8400-e29b-41d4-a716-446655440000";
    const BLOCK_REF_ORG: &str = "((550e8400-e29b-41d4-a716-446655440000))";

    #[test]
    fn block_ref_extracts_internal_link() {
        let (out, marks) = extract_inline_marks(BLOCK_REF_ORG);
        assert_eq!(out, BLOCK_REF_ORG, "rendered text includes the parens");
        assert_eq!(marks.len(), 1);
        let m = &marks[0];
        assert_eq!(m.start, 0);
        assert_eq!(m.end, BLOCK_REF_ORG.chars().count());
        match &m.mark {
            InlineMark::Link { target, label } => {
                assert_eq!(label, BLOCK_REF_ORG);
                match target {
                    EntityRef::Internal { id } => {
                        assert_eq!(id.as_str(), format!("block:{BLOCK_REF_UUID}"));
                    }
                    other => panic!("expected Internal, got {other:?}"),
                }
            }
            other => panic!("expected Link, got {other:?}"),
        }
    }

    #[test]
    fn block_ref_round_trip_byte_stable() {
        let (org, marks) = round_trip(BLOCK_REF_ORG);
        assert_eq!(org, BLOCK_REF_ORG);
        // Also verify direct render for correct delimiter choice.
        let (text, _) = extract_inline_marks(BLOCK_REF_ORG);
        assert_eq!(render_inline_marks(&text, &marks), BLOCK_REF_ORG);
    }

    #[test]
    fn block_ref_surrounded_by_text() {
        let input = format!("see {} here", BLOCK_REF_ORG);
        let (out, marks) = extract_inline_marks(&input);
        assert_eq!(out, input);
        assert_eq!(marks.len(), 1);
        let m = &marks[0];
        assert_eq!(m.start, 4, "block-ref mark starts after 'see '");
        assert_eq!(
            m.end,
            4 + BLOCK_REF_ORG.chars().count(),
            "block-ref mark ends before ' here'"
        );
    }

    #[test]
    fn non_uuid_double_parens_stays_plain_text() {
        let input = "((not a ref))";
        let (out, marks) = extract_inline_marks(input);
        assert_eq!(out, input);
        assert!(marks.is_empty(), "non-UUID ((...)) must not produce a mark");
    }

    #[test]
    fn empty_double_parens_plain_text() {
        let input = "prefix (()) suffix";
        let (out, marks) = extract_inline_marks(input);
        assert_eq!(out, input);
        assert!(marks.is_empty(), "empty (()) must not produce a mark");
    }

    #[test]
    fn unclosed_double_paren_plain_text() {
        let input = "before ((no-close after";
        let (out, marks) = extract_inline_marks(input);
        assert_eq!(out, input);
        assert!(marks.is_empty(), "unclosed (( must stay plain text");
    }

    // --- F1a: three-state link-target classification at the parse boundary ---

    fn only_link_target(text: &str) -> EntityRef {
        let (_, marks) = extract_inline_marks(text);
        marks
            .into_iter()
            .find_map(|m| match m.mark {
                InlineMark::Link { target, .. } => Some(target),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no Link mark extracted from {text:?}"))
    }

    #[test]
    fn registered_scheme_target_is_a_resolved_entity_uri() {
        let input = "See [[person:alice][Alice]] here";
        let (text, marks) = extract_inline_marks(input);
        assert_eq!(text, "See Alice here");
        assert_eq!(
            only_link_target(input),
            EntityRef::Internal {
                // ALLOW(entity_uri_from_raw): test expectation literal
                id: EntityUri::from_raw("person:alice")
            }
        );
        assert_eq!(
            render_inline_marks(&text, &marks),
            input,
            "entity link must re-render byte-stably"
        );
    }

    /// The bare form is byte-stable: an entity link the user typed without a
    /// label must not sprout one on a round trip.
    #[test]
    fn bare_registered_scheme_target_round_trips() {
        let input = "[[person:alice]]";
        let (text, marks) = extract_inline_marks(input);
        assert_eq!(text, "person:alice");
        assert_eq!(render_inline_marks(&text, &marks), input);
    }

    /// `Areas:Work` is scheme-SHAPED but its scheme is not registered. The
    /// reservation rule says it is never a page-creation intent — it degrades
    /// to a disclosed unknown-scheme link, bytes untouched.
    #[test]
    fn unregistered_scheme_target_is_not_a_page_name() {
        let input = "[[Areas:Work][work]]";
        let target = only_link_target(input);
        assert!(
            !matches!(target, EntityRef::Name { .. }),
            "a scheme-shaped target must never be classified as a page name, got {target:?}"
        );
        let (text, marks) = extract_inline_marks(input);
        assert_eq!(render_inline_marks(&text, &marks), input);
    }

    /// A colon FOLLOWED BY A SPACE is not an RFC 3986 scheme, so ordinary
    /// titles keep page semantics with no capitalization heuristics.
    #[test]
    fn colon_space_target_keeps_page_semantics() {
        let input = "[[Ketosis: How to lose weight]]";
        assert_eq!(
            only_link_target(input),
            EntityRef::Name {
                name: "Ketosis: How to lose weight".to_string()
            }
        );
    }

    #[test]
    fn slash_hierarchy_target_keeps_page_semantics() {
        assert_eq!(
            only_link_target("[[Areas/Work][work]]"),
            EntityRef::Name {
                name: "Areas/Work".to_string()
            }
        );
    }

    #[test]
    fn web_and_mail_links_stay_external() {
        for (input, url) in [
            ("[[https://example.com][site]]", "https://example.com"),
            ("[[http://example.com][site]]", "http://example.com"),
            ("[[mailto:a@b.c][mail]]", "mailto:a@b.c"),
        ] {
            assert_eq!(
                only_link_target(input),
                EntityRef::External {
                    url: url.to_string()
                },
                "web/mail links must keep External classification: {input}"
            );
        }
    }

    #[test]
    fn block_scheme_target_unchanged() {
        assert_eq!(
            only_link_target("[[block:abc-123][see also]]"),
            EntityRef::Internal {
                // ALLOW(entity_uri_from_raw): test expectation literal
                id: EntityUri::from_raw("block:abc-123")
            }
        );
    }
}
