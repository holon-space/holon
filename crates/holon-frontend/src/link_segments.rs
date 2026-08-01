//! Split a block's `content` into alternating plain-text and link runs from
//! its inline `Link` marks, for read-only ("rendered_text") rendering.
//!
//! This is the syntax-neutral, frontend-agnostic core of link rendering. GPUI
//! keeps its own copy inline (it needs `gpui`-specific paint types); the
//! wasm/dioxus-web frontend — which has no native test harness — consumes this
//! so the interesting logic (byte-offset splitting, multi-link ordering,
//! multibyte safety) is unit-tested here in a crate that builds and tests on
//! the host.
//!
//! Mark offsets are Unicode-scalar (`char`) positions (see
//! `holon_api::MarkSpan`); the returned segment text is sliced from `content`
//! at the corresponding byte offsets.

use holon_api::EntityRef;
use holon_api::InlineMark;
use holon_api::MarkSpan;

/// One run of a block's content: either plain text or a link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentSegment {
    /// The exact substring of `content` this run covers.
    pub text: String,
    /// `Some` iff this run is a link; carries the link target.
    pub link_target: Option<EntityRef>,
}

impl ContentSegment {
    pub fn is_link(&self) -> bool {
        self.link_target.is_some()
    }
}

/// Convert a Unicode-scalar range `[start, end)` to a byte range within `text`.
/// Asserts the range fits within `text` (fail-loud: a mark offset past the end
/// of its own block's content is a corruption, not something to paper over).
fn scalar_range_to_bytes(text: &str, start: usize, end: usize) -> std::ops::Range<usize> {
    let mut char_to_byte: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    char_to_byte.push(text.len());
    let total = char_to_byte.len() - 1;
    assert!(
        start <= total && end <= total,
        "link mark range {start}..{end} exceeds content length {total} chars"
    );
    char_to_byte[start]..char_to_byte[end]
}

/// Split `content` at `Link`-mark boundaries into non-overlapping, in-order
/// segments. Non-link marks are ignored (read-only link rendering only cares
/// about links; other styling is applied by the text builders). Returns a
/// single plain segment when there are no link marks.
pub fn link_content_segments(content: &str, marks: &[MarkSpan]) -> Vec<ContentSegment> {
    let mut link_spans: Vec<(std::ops::Range<usize>, &EntityRef)> = marks
        .iter()
        .filter_map(|ms| match &ms.mark {
            InlineMark::Link { target, .. } => {
                Some((scalar_range_to_bytes(content, ms.start, ms.end), target))
            }
            _ => None,
        })
        .collect();

    if link_spans.is_empty() {
        return vec![ContentSegment {
            text: content.to_string(),
            link_target: None,
        }];
    }

    link_spans.sort_by_key(|(r, _)| (r.start, r.end));

    let mut segments = Vec::new();
    let mut pos = 0;
    for (range, target) in &link_spans {
        if range.start > pos {
            segments.push(ContentSegment {
                text: content[pos..range.start].to_string(),
                link_target: None,
            });
        }
        segments.push(ContentSegment {
            text: content[range.clone()].to_string(),
            link_target: Some((*target).clone()),
        });
        pos = range.end;
    }
    if pos < content.len() {
        segments.push(ContentSegment {
            text: content[pos..].to_string(),
            link_target: None,
        });
    }
    segments
}

#[cfg(test)]
mod tests {
    use holon_api::EntityUri;

    use super::*;

    fn mark_link(start: usize, end: usize, target: EntityRef) -> MarkSpan {
        MarkSpan::new(
            start,
            end,
            InlineMark::Link {
                target,
                label: String::new(),
            },
        )
    }

    fn internal(id: &str) -> EntityRef {
        EntityRef::from_uri(&EntityUri::parse(id).unwrap())
    }

    #[test]
    fn no_links_yields_single_plain_segment() {
        let segs = link_content_segments("Hello world", &[]);
        assert_eq!(segs.len(), 1);
        assert!(!segs[0].is_link());
        assert_eq!(segs[0].text, "Hello world");
    }

    #[test]
    fn non_link_marks_are_ignored() {
        let segs = link_content_segments("bold text", &[MarkSpan::new(0, 4, InlineMark::Bold)]);
        assert_eq!(segs.len(), 1);
        assert!(!segs[0].is_link());
        assert_eq!(segs[0].text, "bold text");
    }

    #[test]
    fn single_link_splits_into_three() {
        let content = "before [[link]] after";
        let start = content.find("[[link]]").unwrap();
        let end = start + "[[link]]".len();
        let segs = link_content_segments(content, &[mark_link(start, end, internal("block:abc"))]);
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0].text, "before ");
        assert!(!segs[0].is_link());
        assert_eq!(segs[1].text, "[[link]]");
        assert_eq!(segs[1].link_target, Some(internal("block:abc")));
        assert_eq!(segs[2].text, " after");
        assert!(!segs[2].is_link());
    }

    #[test]
    fn multiple_links_kept_in_order() {
        let content = "A [[one]] B [[two]] C";
        let o = content.find("[[one]]").unwrap();
        let t = content.find("[[two]]").unwrap();
        let segs = link_content_segments(
            content,
            &[
                mark_link(t, t + "[[two]]".len(), internal("block:two")),
                mark_link(o, o + "[[one]]".len(), internal("block:one")),
            ],
        );
        assert_eq!(
            segs.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(),
            vec!["A ", "[[one]]", " B ", "[[two]]", " C"],
        );
        assert_eq!(segs[1].link_target, Some(internal("block:one")));
        assert_eq!(segs[3].link_target, Some(internal("block:two")));
    }

    #[test]
    fn link_at_start_and_end() {
        let content = "[[x]] mid [[y]]";
        let x = 0;
        let y = content.find("[[y]]").unwrap();
        let segs = link_content_segments(
            content,
            &[
                mark_link(x, "[[x]]".len(), internal("block:x")),
                mark_link(y, content.chars().count(), internal("block:y")),
            ],
        );
        assert_eq!(segs.len(), 3);
        assert!(segs[0].is_link());
        assert_eq!(segs[0].text, "[[x]]");
        assert!(!segs[1].is_link());
        assert_eq!(segs[1].text, " mid ");
        assert!(segs[2].is_link());
        assert_eq!(segs[2].text, "[[y]]");
    }

    #[test]
    fn multibyte_content_slices_on_char_boundaries() {
        // "ab\u{00FC}c": a=0 b=1 ü=2(2 bytes) c=3 → 4 scalars, 5 bytes.
        let content = "ab\u{00FC}c";
        let segs = link_content_segments(content, &[mark_link(2, 3, internal("block:u"))]);
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0].text, "ab");
        assert_eq!(segs[1].text, "\u{00FC}");
        assert!(segs[1].is_link());
        assert_eq!(segs[2].text, "c");
    }

    #[test]
    fn dangling_name_target_preserved() {
        let content = "see [[Some Page]]";
        let start = content.find("[[Some Page]]").unwrap();
        let segs = link_content_segments(
            content,
            &[mark_link(
                start,
                content.chars().count(),
                EntityRef::Name {
                    name: "Some Page".to_string(),
                },
            )],
        );
        assert_eq!(segs.len(), 2);
        assert_eq!(
            segs[1].link_target,
            Some(EntityRef::Name {
                name: "Some Page".to_string()
            })
        );
    }
}
