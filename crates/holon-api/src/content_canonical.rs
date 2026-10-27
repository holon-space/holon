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
mod tests;
