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
//! 2. Walk the document tree skipping non-paragraph wrappers; emit text
//!    tokens directly to the output.
//! 3. On encountering a mark node (BOLD/ITALIC/.../LINK/SUB/SUPER), strip its
//!    delimiters, recurse on the inner string for nested marks, then emit a
//!    `MarkSpan` covering the inner (already-stripped) range plus any nested
//!    spans shifted by the outer offset.
//!
//! Known limitations (per `docs/orgize_inline_audit.md`):
//! - Backslash escapes (`\*not bold\*`) are not honored by orgize
//!   0.10.0-alpha.10; the locked-in regression test asserts the current
//!   lossy behavior so a future orgize bump will surface the change.
//! - Sub/Super only match orgize's `_{…}` / `^{…}` form; bare `_{` is not a
//!   mark — that's correct Org behavior.

use holon_api::link_parser::{classify_link, LinkTarget};
use holon_api::{EntityRef, InlineMark, MarkSpan};
use orgize::rowan::ast::AstNode;
use orgize::rowan::NodeOrToken;
use orgize::{Org, SyntaxKind, SyntaxNode};

/// Parse `text` as inline org content. Returns `(rendered_text, marks)` where
/// `rendered_text` has all mark delimiters stripped and `marks` carries
/// Unicode-scalar offsets into the rendered text.
pub fn extract_inline_marks(text: &str) -> (String, Vec<MarkSpan>) {
    let org = Org::parse(text);
    let mut state = ExtractState::default();
    walk_node(org.document().syntax(), &mut state);
    (state.out, state.marks)
}

#[derive(Default)]
struct ExtractState {
    out: String,
    marks: Vec<MarkSpan>,
    char_pos: usize,
}

fn walk_node(node: &SyntaxNode, state: &mut ExtractState) {
    for child in node.children_with_tokens() {
        match child {
            NodeOrToken::Node(child_node) => {
                if let Some(kind_hint) = inline_mark_kind(child_node.kind()) {
                    emit_mark(child_node, kind_hint, state);
                } else {
                    walk_node(&child_node, state);
                }
            }
            NodeOrToken::Token(tok) => {
                let txt = tok.text();
                state.out.push_str(txt);
                state.char_pos += txt.chars().count();
            }
        }
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
            let (text, mark) = strip_link(&raw);
            push_with_inner_marks(state, &text, vec![], mark);
        }
        MarkKindHint::Sub | MarkKindHint::Super => {
            // SUBSCRIPT / SUPERSCRIPT: `_{…}` / `^{…}` — strip 2-char prefix + 1-char suffix.
            // No nested marks supported in sub/super for Phase 1 (rare in practice).
            let inner = strip_prefix_suffix(&raw, 2, 1);
            let mark = match kind_hint {
                MarkKindHint::Sub => InlineMark::Sub,
                MarkKindHint::Super => InlineMark::Super,
                _ => unreachable!(),
            };
            push_with_inner_marks(state, &inner, vec![], mark);
        }
        _ => {
            // BOLD/ITALIC/UNDERLINE/VERBATIM/CODE/STRIKE: 1-char delimiter each side.
            let inner = strip_prefix_suffix(&raw, 1, 1);
            // Recurse into the inner string for nested marks. orgize re-parses
            // the substring fresh; nested mark offsets are scalar offsets
            // within `inner`, ready to be shifted by the outer start.
            let (nested_text, nested_marks) = extract_inline_marks(&inner);
            // The text from recursion may differ from `inner` if it had nested
            // marks (delimiters were stripped). Use nested_text as the actual
            // emitted content.
            let outer_mark = match kind_hint {
                MarkKindHint::Bold => InlineMark::Bold,
                MarkKindHint::Italic => InlineMark::Italic,
                MarkKindHint::Underline => InlineMark::Underline,
                MarkKindHint::Verbatim => InlineMark::Verbatim,
                MarkKindHint::Code => InlineMark::Code,
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

/// Parse a `[[…][…]]` or `[[…]]` link literal. Returns `(rendered_label, Link mark)`.
///
/// - `[[uri][label]]` → label is the rendered text; uri is classified into `EntityRef`.
/// - `[[uri]]` (bare) → rendered text is the uri itself; classified the same way.
fn strip_link(raw: &str) -> (String, InlineMark) {
    // Strip outer `[[` and `]]`.
    let inside = raw
        .strip_prefix("[[")
        .and_then(|s| s.strip_suffix("]]"))
        .unwrap_or(raw);
    // Split on `][` to separate uri and label, if present.
    let (uri, label) = match inside.split_once("][") {
        Some((u, l)) => (u.to_string(), l.to_string()),
        None => (inside.to_string(), inside.to_string()),
    };
    let target = match classify_link(&uri) {
        LinkTarget::External(s) => EntityRef::External { url: s },
        LinkTarget::Resolved(uri) => EntityRef::Internal { id: uri },
        LinkTarget::CreationIntent { target_id, .. } => EntityRef::Internal { id: target_id },
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

    if marks.is_empty() {
        return text.to_string();
    }

    detect_crossing_marks(marks);

    // Bucket events by char position. At each position we may emit several
    // closes (in inverse opening order) and several opens (outer-first).
    let mut opens_at: BTreeMap<usize, Vec<&MarkSpan>> = BTreeMap::new();
    let mut closes_at: BTreeMap<usize, Vec<&MarkSpan>> = BTreeMap::new();
    for m in marks {
        opens_at.entry(m.start).or_default().push(m);
        closes_at.entry(m.end).or_default().push(m);
    }
    // Sort opens at same position: longer marks (later end) open first → outer.
    for v in opens_at.values_mut() {
        v.sort_by(|a, b| b.end.cmp(&a.end));
    }
    // Sort closes at same position: most-recently-opened (later start) closes first.
    for v in closes_at.values_mut() {
        v.sort_by(|a, b| b.start.cmp(&a.start));
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

/// Open delimiter for a mark. For Link, this is `[[uri][` (the label and
/// closing `]]` come at the close position).
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
        InlineMark::Link { target, .. } => {
            let uri = match target {
                EntityRef::External { url } => url.clone(),
                EntityRef::Internal { id } => id.as_str().to_string(),
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
        InlineMark::Link { .. } => "]]".into(),
    }
}

/// Log a tracing warning if `marks` contains crossing pairs (A.start <
/// B.start < A.end < B.end). Org can't represent crossing inline marks.
fn detect_crossing_marks(marks: &[MarkSpan]) {
    for (i, a) in marks.iter().enumerate() {
        for b in marks.iter().skip(i + 1) {
            if a.start < b.start && b.start < a.end && a.end < b.end {
                tracing::warn!(
                    "render_inline_marks: crossing marks detected — {a:?} crosses {b:?}; org output may be lossy"
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
}
