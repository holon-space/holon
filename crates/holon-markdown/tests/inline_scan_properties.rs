//! Properties of the inline scanner over EVERY string of up to four tokens
//! from an alphabet of markup fragments. Exhaustive enumeration stands in for
//! a random generator: the space is small enough to cover in full.

use holon_markdown::inline::scan_inline;

const TOKENS: &[&str] = &[
    "[[", "]]", "[", "]", "(", ")", "#", "*", "_", "`", "=", "~", "!", "|", "a", " ", "é",
];

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
