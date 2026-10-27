use super::canonicalize_stored_content;

#[test]
fn single_line_text_trims_both_ends() {
    assert_eq!(canonicalize_stored_content("  foo  ", false), "foo");
    assert_eq!(canonicalize_stored_content("foo ", false), "foo");
}

#[test]
fn multiline_text_trims_first_line_both_ends_body_verbatim() {
    // The first line (headline) is fully trimmed; the body keeps its exact
    // shape, including interior/trailing whitespace on non-first lines that
    // is not at the very end of the whole string.
    assert_eq!(canonicalize_stored_content("foo \nbar", false), "foo\nbar");
    assert_eq!(
        canonicalize_stored_content("  foo  \n  bar  ", false),
        "foo\n  bar"
    );
}

#[test]
fn source_only_strips_overall_trailing() {
    // A source body preserves leading + interior whitespace; only overall
    // trailing whitespace goes.
    assert_eq!(
        canonicalize_stored_content("  foo \nbar  ", true),
        "  foo \nbar"
    );
    assert_eq!(canonicalize_stored_content("  foo  ", true), "  foo");
}

#[test]
fn idempotent() {
    for s in ["foo", "foo\nbar", "  x  \n y ", ""] {
        let once = canonicalize_stored_content(s, false);
        assert_eq!(
            canonicalize_stored_content(&once, false),
            once,
            "text {s:?}"
        );
        let once_src = canonicalize_stored_content(s, true);
        assert_eq!(
            canonicalize_stored_content(&once_src, true),
            once_src,
            "source {s:?}"
        );
    }
}
