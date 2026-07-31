//! Adversarial shape sweep over `render_lossless` — the render half's
//! store→disk contract.
//!
//! Two obligations, and NO input may escape either by taking a short path:
//! 1. Every shape must be emittable. A shape that cannot be quoted is a
//!    disclosed degradation in prod, so any new bail here is a regression.
//! 2. Emphasis-shaped literals survive byte-for-byte; raw link syntax is
//!    allowed to ADOPT into a `Link` mark (intended product behavior).

use holon_api::EntityRef;
use holon_api::InlineMark;
use holon_api::MarkSpan;
use holon_org_format::extract_inline_marks;
use holon_org_format::render_lossless;

/// Shapes with no link syntax: the round trip must be byte-identical.
const LITERAL_SHAPES: &[&str] = &[
    // Both quote delimiters appearing inside the span being quoted.
    "*a=b~c*",
    "*a=b*",
    "*a~b*",
    "/x=y~z/",
    "~a=b~",
    "=a~b=",
    "*a=*",
    "*=*",
    // Bare delimiter pairs that org does NOT read as emphasis.
    "**",
    "__",
    "//",
    "~~",
    "==",
    "++",
    "* leading star",
    // Several independent spans in one string, including one that already
    // contains the quote delimiter.
    "a *b* =c= ~d~",
    "*multi\nline*",
    "*a\n\nb*",
    // Neighbouring org constructs that must not be disturbed.
    "footnote [fn:1] and *x*",
    "<2026-07-31 Fri> and *x*",
    "|table| and *x*",
    "*x* https://bare.url",
    "https://bare.url and *x*",
    "call_fn() and *x*",
    "src_sh{ls} and *x*",
    "*a_b_c*",
    "_a*b*c_",
    "((550e8400-e29b-41d4-a716-446655440000)) and *x*",
];

/// `(input, expected parse-back)` — link syntax adopts, so the parse-back is
/// the post-adoption form, not the input bytes.
const LINK_SHAPES: &[(&str, &str)] = &[
    ("text with [[link]] only", "text with link only"),
    ("a *b* and [[c]]", "a *b* and c"),
    ("[[block:xyz][lbl]] and __y__", "lbl and __y__"),
    ("[[https://e.com][l]] and __y__", "l and __y__"),
    // A link LABEL is not emphasis-parsed, so its bytes survive adoption.
    ("[[https://example.com][__lbl__]]", "__lbl__"),
];

#[test]
fn literal_shapes_round_trip_byte_identically() {
    for shape in LITERAL_SHAPES {
        let emitted = render_lossless(shape, &[])
            .unwrap_or_else(|e| panic!("render_lossless({shape:?}) bailed: {e}"));
        assert_eq!(
            extract_inline_marks(&emitted).0,
            *shape,
            "shape {shape:?} emitted as {emitted:?}"
        );
    }
}

/// KILL 1 (verifier round 3, from Martin's live vault — 18 occurrences of the
/// shape). A `Verbatim` mark over raw link syntax is the store saying "this
/// span is LITERAL TEXT, it is documentation about link syntax". Emitting
/// `=[[uuid][Label]]=` is genuinely lossless and was byte-stable before task
/// #67. If the expectation lets links adopt unconditionally it judges that
/// correct emission wrong, degrades, strips the quoting — and the next cycle
/// turns a documented example into a live link to a nonexistent page.
#[test]
fn verbatim_over_raw_link_syntax_stays_literal() {
    let content = "Rule fork F5: raw link form — bare [[uuid][Label]] vs page-name sugar";
    let literal_start = content[..content.find("[[uuid][Label]]").expect("literal is present")]
        .chars()
        .count();
    let marks = vec![MarkSpan {
        start: literal_start,
        end: literal_start + "[[uuid][Label]]".chars().count(),
        mark: InlineMark::Verbatim,
    }];
    let emitted = render_lossless(content, &marks).expect("this shape IS representable");
    let (back, back_marks) = extract_inline_marks(&emitted);
    assert_eq!(back, content, "emitted {emitted:?}");
    assert!(
        back_marks.iter().any(|m| m.mark == InlineMark::Verbatim),
        "the protective mark must survive; got {back_marks:?}"
    );
    assert!(
        !back_marks
            .iter()
            .any(|m| matches!(m.mark, InlineMark::Link { .. })),
        "the literal must NOT have adopted into a link; got {back_marks:?}"
    );
}

/// KILL 2 (verifier round 3). A `Link` mark's target exists ONLY in the mark —
/// the content carries the label alone. Quoting a markup-shaped literal that
/// sits INSIDE the label breaks the emission, and degrading by dropping marks
/// then deletes the URL from disk and store, unrecoverably. Org parses no
/// emphasis inside a link label, so the quoting has nothing to do there.
#[test]
fn markup_shaped_text_inside_a_link_label_keeps_the_url() {
    let content = "the __init__ method";
    let marks = vec![MarkSpan {
        start: 0,
        end: content.chars().count(),
        mark: InlineMark::Link {
            target: EntityRef::External {
                url: "https://example.com".to_string(),
            },
            label: content.to_string(),
        },
    }];
    let emitted = render_lossless(content, &marks).expect("this shape IS representable");
    assert!(
        emitted.contains("https://example.com"),
        "the URL must reach disk; emitted {emitted:?}"
    );
    let (back, back_marks) = extract_inline_marks(&emitted);
    assert_eq!(back, content, "emitted {emitted:?}");
    let target = back_marks
        .iter()
        .find_map(|m| match &m.mark {
            InlineMark::Link { target, .. } => Some(target),
            _ => None,
        })
        .unwrap_or_else(|| panic!("the Link mark must survive; got {back_marks:?}"));
    assert_eq!(
        target,
        &EntityRef::External {
            url: "https://example.com".to_string()
        }
    );
}

#[test]
fn link_shapes_adopt_and_keep_every_other_byte() {
    for (shape, expected) in LINK_SHAPES {
        let emitted = render_lossless(shape, &[])
            .unwrap_or_else(|e| panic!("render_lossless({shape:?}) bailed: {e}"));
        assert_eq!(
            extract_inline_marks(&emitted).0,
            *expected,
            "shape {shape:?} emitted as {emitted:?}"
        );
    }
}
