//! Properties of the inline scanner over EVERY string of up to four tokens
//! from an alphabet of markup fragments. Exhaustive enumeration stands in for
//! a random generator: the space is small enough to cover in full.

use holon_api::EntityUri;
use holon_api::inline_mark::EntityRef;
use holon_api::inline_mark::InlineMark;
use holon_markdown::inline::scan_inline;
use holon_org_format::DrawerId;

const TOKENS: &[&str] = &[
    "[[", "]]", "[", "]", "(", ")", "#", "*", "_", "`", "=", "~", "!", "|", "a", " ", "é", "((",
    "))",
];

/// Inner texts of a block ref a URI cannot carry as an id.
const AUTHORED_INNERS: &[&str] = &["a b", "é", "a|b", "a%zz", "x\"y", "a{b}", "block:x"];

const MARKUP: &[char] = &['[', ']', '(', ')', '#', '*', '_', '`', '=', '~', '!', '|'];

fn every_input(max_tokens: usize) -> impl Iterator<Item = String> {
    (0..=max_tokens).flat_map(|len| {
        (0..TOKENS.len().pow(len as u32)).map(move |mut n| {
            let mut s = String::new();
            for _ in 0..len {
                s.push_str(TOKENS[n % TOKENS.len()]);
                n /= TOKENS.len();
            }
            s
        })
    })
}

#[test]
fn marks_are_non_empty_ranges_inside_the_content() {
    for input in every_input(4) {
        let parsed = scan_inline(&input);
        let len = parsed.content.chars().count();
        for m in &parsed.marks {
            assert!(
                m.start < m.end && m.end <= len,
                "{input:?}: mark {}..{} outside content {:?} ({len} chars)",
                m.start,
                m.end,
                parsed.content
            );
        }
    }
}

#[test]
fn text_without_markup_is_kept_verbatim_and_unmarked() {
    for input in every_input(4).filter(|s| !s.contains(MARKUP)) {
        let parsed = scan_inline(&input);
        assert_eq!(parsed.content, input);
        assert!(
            parsed.marks.is_empty(),
            "{input:?}: {:?}",
            parsed.marks.len()
        );
        assert!(parsed.tags.is_empty(), "{input:?}: {:?}", parsed.tags);
    }
}

/// Text a block ref can hold that no markup rule reads: every string of up to
/// four of these between `((` and `))` either becomes a block link or stays
/// verbatim text.
const INNER_TOKENS: &[&str] = &["a", "1", "-", " ", "é", "|", "{", "\"", "%", "block:", ":"];

fn every_inner(max_tokens: usize) -> impl Iterator<Item = String> {
    (0..=max_tokens).flat_map(|len| {
        (0..INNER_TOKENS.len().pow(len as u32)).map(move |mut n| {
            let mut s = String::new();
            for _ in 0..len {
                s.push_str(INNER_TOKENS[n % INNER_TOKENS.len()]);
                n /= INNER_TOKENS.len();
            }
            s
        })
    })
}

#[test]
fn a_block_ref_is_a_block_link_exactly_when_its_inner_is_a_block_id() {
    let statics = AUTHORED_INNERS.iter().map(|s| s.to_string());
    for inner in statics.chain(every_inner(4)) {
        let text = format!("(({inner}))");
        let parsed = scan_inline(&text);
        let trimmed = inner.trim();
        if trimmed.is_empty() {
            continue;
        }
        if DrawerId::parse(trimmed).is_ok() {
            assert_eq!(parsed.marks.len(), 1, "{text:?}");
            let InlineMark::Link { target, .. } = &parsed.marks[0].mark else {
                panic!("{text:?}: {:?}", parsed.marks[0].mark)
            };
            assert_eq!(
                *target,
                EntityRef::from_uri(&EntityUri::block(trimmed)),
                "{text:?}"
            );
        } else {
            assert_eq!(parsed.content, text, "an unusable id stays text");
            assert!(parsed.marks.is_empty(), "{text:?}");
        }
    }
}
