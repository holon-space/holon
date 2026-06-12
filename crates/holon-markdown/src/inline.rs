//! Inline Obsidian syntax extracted into sidecar metadata.
//!
//! Like wikilinks, the source spans are left **verbatim** in `block.content`
//! (so round-trips are lossless regardless of the dialect), and the parsed
//! values are projected into per-feature sidecar properties only when the
//! matching switch is on. Each extractor here is independent of the others:
//!
//! - `==text==` highlights → `highlights`
//! - `%%inline%%` / `%%\n…\n%%` comments → `comments`
//! - `#tag`, `#area/sub` inline tags → `inline_tags`
//!
//! Extraction is best-effort lightweight scanning (it does not skip code
//! spans), which is fine for advisory metadata — the authoritative text is
//! always the preserved content.

use std::collections::HashSet;

/// `==highlighted==` spans, in order, without the `==` delimiters. Empty
/// spans (`====`) are ignored; matching does not cross a line.
pub fn extract_highlights(text: &str) -> Vec<String> {
    extract_paired(text, "==", false)
}

/// `%%comment%%` spans, in order, without the `%%` delimiters. A comment may
/// span multiple lines (Obsidian's block-comment form).
pub fn extract_comments(text: &str) -> Vec<String> {
    extract_paired(text, "%%", true)
}

/// `#tag` inline tags, deduped in first-occurrence order, without the `#`.
/// When `nested` is true a `/` continues the tag (`#area/sub` is one tag);
/// otherwise the tag stops at the `/`. A tag must contain a letter and is
/// never purely numeric, matching Obsidian's rule that `#123` is not a tag.
pub fn extract_inline_tags(text: &str, nested: bool) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'#' || (i > 0 && !bytes[i - 1].is_ascii_whitespace()) {
            i += 1;
            continue;
        }
        let start = i + 1;
        let mut j = start;
        while j < bytes.len() && is_tag_char(bytes[j], nested) {
            j += 1;
        }
        let tag = text[start..j].trim_end_matches('/');
        if is_valid_tag(tag) && seen.insert(tag.to_string()) {
            out.push(tag.to_string());
        }
        i = j.max(i + 1);
    }
    out
}

fn extract_paired(text: &str, delim: &str, allow_newline: bool) -> Vec<String> {
    let dlen = delim.len();
    let bytes = text.as_bytes();
    let dbytes = delim.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + dlen <= bytes.len() {
        if &bytes[i..i + dlen] != dbytes {
            i += 1;
            continue;
        }
        let inner_start = i + dlen;
        let mut j = inner_start;
        let mut close = None;
        while j + dlen <= bytes.len() {
            if !allow_newline && bytes[j] == b'\n' {
                break;
            }
            if &bytes[j..j + dlen] == dbytes {
                close = Some(j);
                break;
            }
            j += 1;
        }
        match close {
            Some(end) => {
                let inner = &text[inner_start..end];
                if !inner.is_empty() {
                    out.push(inner.to_string());
                }
                i = end + dlen;
            }
            None => i += 1,
        }
    }
    out
}

fn is_tag_char(b: u8, nested: bool) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || (nested && b == b'/')
}

fn is_valid_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag.bytes().any(|b| b.is_ascii_alphabetic())
        && tag.bytes().any(|b| !b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_extracted_in_order() {
        assert_eq!(
            extract_highlights("a ==first== then ==second== end"),
            vec!["first".to_string(), "second".to_string()]
        );
    }

    #[test]
    fn highlight_does_not_cross_newline() {
        assert!(extract_highlights("==open\nclose==").is_empty());
    }

    #[test]
    fn empty_highlight_ignored() {
        assert!(extract_highlights("==== nothing").is_empty());
    }

    #[test]
    fn inline_comment_extracted() {
        assert_eq!(
            extract_comments("visible %%hidden%% text"),
            vec!["hidden".to_string()]
        );
    }

    #[test]
    fn block_comment_spans_lines() {
        assert_eq!(
            extract_comments("before %%\nblock\ncomment\n%% after"),
            vec!["\nblock\ncomment\n".to_string()]
        );
    }

    #[test]
    fn flat_tags_extracted() {
        assert_eq!(
            extract_inline_tags("see #project and #urgent here", false),
            vec!["project".to_string(), "urgent".to_string()]
        );
    }

    #[test]
    fn nested_tag_depends_on_switch() {
        assert_eq!(
            extract_inline_tags("#area/sub", false),
            vec!["area".to_string()]
        );
        assert_eq!(
            extract_inline_tags("#area/sub", true),
            vec!["area/sub".to_string()]
        );
    }

    #[test]
    fn hash_in_word_is_not_a_tag() {
        assert!(extract_inline_tags("color#fff and C#", false).is_empty());
    }

    #[test]
    fn purely_numeric_is_not_a_tag() {
        assert!(extract_inline_tags("issue #123 closed", false).is_empty());
        assert_eq!(
            extract_inline_tags("#v2 release", false),
            vec!["v2".to_string()]
        );
    }

    #[test]
    fn tags_deduped_in_order() {
        assert_eq!(
            extract_inline_tags("#a #b #a", false),
            vec!["a".to_string(), "b".to_string()]
        );
    }
}
