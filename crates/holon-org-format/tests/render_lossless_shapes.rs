//! Adversarial shape sweep over `render_lossless` — the render half's
//! store→disk contract.
//!
//! Two obligations, and NO input may escape either by taking a short path:
//! 1. Every shape must be emittable. A shape that cannot be quoted is a
//!    disclosed degradation in prod, so any new bail here is a regression.
//! 2. Emphasis-shaped literals survive byte-for-byte; raw link syntax is
//!    allowed to ADOPT into a `Link` mark (intended product behavior).

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
