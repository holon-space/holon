//! The closer lookup of the inline scanner answers like a linear search.

use holon_markdown::inline::NextIndex;

const ALPHABET: &[char] = &['[', ']', '(', ')', '*', '_', 'a'];
const NEEDLES: &[&[char]] = &[
    &[']', ']'],
    &[']'],
    &[')', ')'],
    &[')'],
    &['*', '*'],
    &['*'],
    &['_'],
];

fn linear_find(src: &[char], needle: &[char], from: usize) -> Option<usize> {
    if needle.is_empty() || from + needle.len() > src.len() {
        return None;
    }
    (from..=src.len() - needle.len()).find(|&j| src[j..j + needle.len()] == *needle)
}

#[test]
fn the_lookup_returns_the_first_match_at_or_after_from() {
    for len in 0..=7u32 {
        for mut n in 0..ALPHABET.len().pow(len) {
            let src: Vec<char> = (0..len)
                .map(|_| {
                    let c = ALPHABET[n % ALPHABET.len()];
                    n /= ALPHABET.len();
                    c
                })
                .collect();
            let mut index = NextIndex::default();
            for needle in NEEDLES {
                for from in 0..=src.len() + 1 {
                    assert_eq!(
                        index.find(&src, needle, from),
                        linear_find(&src, needle, from),
                        "{src:?} {needle:?} from {from}"
                    );
                }
            }
        }
    }
}
