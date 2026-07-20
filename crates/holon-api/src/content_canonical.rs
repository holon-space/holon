//! The single source of truth for the content canonicalization the SQL write
//! path applies before persisting a block's `content`.
//!
//! Both the storage layer (`SqlOperationProvider::trimmed_content` in the
//! `holon` crate) and the GPUI editor's echo-suppression discriminator
//! (`evaluate_data_sync_echo` in `holon-gpui`) MUST agree byte-for-byte on this
//! transform: the editor recognizes the SQL-canonicalized echo of its OWN
//! in-flight write by testing `canonicalize_stored_content(buffer) == echo`. If
//! the two definitions drifted, a class of typed whitespace (e.g. a space at
//! the end of a multiline block's FIRST line) would be canonicalized away by
//! the store yet not recognized by the editor, and the echo would delete it
//! from the focused buffer. Keeping one function both call closes that gap for
//! good.

/// Canonicalize a block's `content` exactly as the SQL write path stores it.
///
/// - Trailing whitespace on the whole string is ALWAYS stripped.
/// - `is_source` blocks otherwise preserve the body verbatim — their content is
///   a code/source body, not remodeled as an org headline.
/// - Text blocks additionally trim the FIRST line on both ends (it becomes the
///   org headline, which the parser `.trim()`s on re-ingest); the remaining
///   lines are preserved verbatim. A single-line text block is therefore fully
///   trimmed (leading and trailing).
///
/// Mirrors `normalize_content_for_org_roundtrip` in the PBT `pbt/types.rs`.
pub fn canonicalize_stored_content(content: &str, is_source: bool) -> String {
    let trimmed_end = content.trim_end();
    if is_source {
        return trimmed_end.to_string();
    }
    match trimmed_end.split_once('\n') {
        Some((first, rest)) => format!("{}\n{}", first.trim(), rest),
        None => trimmed_end.trim_start().to_string(),
    }
}

#[cfg(test)]
mod tests {
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
}
