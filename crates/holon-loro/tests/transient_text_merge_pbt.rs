//! The 3-way text merge contract of [`TransientLoroTextMerge`], checked over
//! generated `(base, theirs, mine)` triples.
//!
//! Every char is unique across the three strings, so the strings alone say
//! which base chars each side kept and which chars it inserted. That makes the
//! contract checkable from the inputs:
//! - a char inserted by either side survives, in its side's order;
//! - a base char survives iff neither side deleted it;
//! - inserts of both sides into the same gap of `base` come out mine first,
//!   then theirs. An insert's gap is the one after the last base char its side
//!   kept before it.
//!
//! @pbt kind harness
//! @pbt covers file-sync-3way-text-merge — a concurrent file-vs-UI edit of one
//! block never resurrects deleted text nor loses either side's edit

use std::collections::HashSet;

use holon_filesystem::TextMergeOutcome as Outcome;
use holon_filesystem::ThreeWayTextMerge;
use holon_loro::TransientLoroTextMerge;
use proptest::prelude::*;

const BASE_POOL: &str = "abc\ndefghijkl𝔞𝔟";
const THEIRS_POOL: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ𝔸";
const MINE_POOL: &str = "0123456789αβγδεζηθικλμνξ😀";

fn outcome_within(limit: u64, base: &str, theirs: &str, mine: &str) -> Outcome {
    TransientLoroTextMerge::with_work_limit(limit)
        .merge_text(base, theirs, mine)
        .unwrap_or_else(|e| panic!("merge_text({base:?}, {theirs:?}, {mine:?}) failed: {e:#}"))
}

fn outcome(base: &str, theirs: &str, mine: &str) -> Outcome {
    TransientLoroTextMerge::default()
        .merge_text(base, theirs, mine)
        .unwrap_or_else(|e| panic!("merge_text({base:?}, {theirs:?}, {mine:?}) failed: {e:#}"))
}

fn merge(base: &str, theirs: &str, mine: &str) -> String {
    match outcome(base, theirs, mine) {
        Outcome::Merged(merged) => merged,
        Outcome::TooLarge => panic!("merge_text({base:?}, {theirs:?}, {mine:?}) is too large"),
    }
}

/// One side's edit of `base`: which base chars it keeps, and how many fresh
/// chars it inserts into each of the `base.len() + 1` gaps.
#[derive(Clone, Debug)]
struct Edit {
    keep: Vec<bool>,
    inserts: Vec<usize>,
}

impl Edit {
    fn apply(&self, base: &[char], pool: &str) -> String {
        let mut fresh = pool.chars();
        let mut out = String::new();
        for gap in 0..=base.len() {
            for _ in 0..self.inserts[gap] {
                out.push(fresh.next().expect("pool covers the largest edit"));
            }
            if gap < base.len() && self.keep[gap] {
                out.push(base[gap]);
            }
        }
        out
    }
}

fn edit(len: usize) -> impl Strategy<Value = Edit> {
    (
        prop::collection::vec(prop::bool::weighted(0.7), len),
        prop::collection::vec(prop_oneof![3 => Just(0usize), 1 => 1..=2usize], len + 1),
    )
        .prop_map(|(keep, inserts)| Edit { keep, inserts })
}

fn triple() -> impl Strategy<Value = (String, String, String)> {
    (0..=12usize)
        .prop_flat_map(|len| (Just(len), edit(len), edit(len)))
        .prop_map(|(len, theirs, mine)| {
            let base: Vec<char> = BASE_POOL.chars().take(len).collect();
            (
                base.iter().collect(),
                theirs.apply(&base, THEIRS_POOL),
                mine.apply(&base, MINE_POOL),
            )
        })
}

/// The gap of `base` each char of `side` that is not in `base` was inserted
/// into, paired with the char.
fn inserts_by_gap(base: &[char], side: &str) -> Vec<(usize, char)> {
    let mut gap = 0;
    let mut out = Vec::new();
    for c in side.chars() {
        match base.iter().position(|b| *b == c) {
            Some(i) => gap = i + 1,
            None => out.push((gap, c)),
        }
    }
    out
}

/// The merge the contract fixes: per gap, mine's inserts, then theirs', then
/// the base char after the gap if both sides kept it.
fn expected(base: &str, theirs: &str, mine: &str) -> String {
    let base: Vec<char> = base.chars().collect();
    let kept = |side: &str| side.chars().collect::<HashSet<char>>();
    let (kept_theirs, kept_mine) = (kept(theirs), kept(mine));
    let (ins_theirs, ins_mine) = (inserts_by_gap(&base, theirs), inserts_by_gap(&base, mine));
    let mut out = String::new();
    for gap in 0..=base.len() {
        out.extend(ins_mine.iter().filter(|(g, _)| *g == gap).map(|(_, c)| c));
        out.extend(ins_theirs.iter().filter(|(g, _)| *g == gap).map(|(_, c)| c));
        if gap < base.len() && kept_theirs.contains(&base[gap]) && kept_mine.contains(&base[gap]) {
            out.push(base[gap]);
        }
    }
    out
}

fn subsequence_of(s: &str, keep: &HashSet<char>) -> String {
    s.chars().filter(|c| keep.contains(c)).collect()
}

fn check(base: &str, theirs: &str, mine: &str) -> Result<(), TestCaseError> {
    check_merged(base, theirs, mine, merge(base, theirs, mine))
}

fn check_merged(base: &str, theirs: &str, mine: &str, merged: String) -> Result<(), TestCaseError> {
    let chars = |s: &str| s.chars().collect::<HashSet<char>>();
    let (b, t, m, r) = (chars(base), chars(theirs), chars(mine), chars(&merged));

    let survivors: HashSet<char> = t
        .union(&m)
        .copied()
        .filter(|c| !b.contains(c) || (t.contains(c) && m.contains(c)))
        .collect();
    prop_assert_eq!(
        merged.chars().count(),
        r.len(),
        "a char appears twice in {:?}",
        merged
    );
    let resurrected: Vec<&char> = r.difference(&survivors).collect();
    prop_assert!(
        resurrected.is_empty(),
        "deleted chars {:?} came back in {:?}",
        resurrected,
        merged
    );
    let lost: Vec<&char> = survivors.difference(&r).collect();
    prop_assert!(
        lost.is_empty(),
        "chars {:?} one side wrote and no side deleted are missing from {:?}",
        lost,
        merged
    );
    prop_assert_eq!(
        subsequence_of(&merged, &t),
        subsequence_of(theirs, &r),
        "theirs' order lost"
    );
    prop_assert_eq!(
        subsequence_of(&merged, &m),
        subsequence_of(mine, &r),
        "mine's order lost"
    );
    prop_assert_eq!(
        merged,
        expected(base, theirs, mine),
        "same-gap inserts out of order"
    );
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 2000,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn merge_keeps_every_edit_and_no_deleted_char((base, theirs, mine) in triple()) {
        check(&base, &theirs, &mine)?;
    }

    #[test]
    fn under_any_work_limit_a_merge_keeps_every_edit_or_merges_nothing(
        (base, theirs, mine) in triple(),
        limit in prop_oneof![Just(0u64), 0..400u64, Just(u64::MAX)],
    ) {
        let forward = outcome_within(limit, &base, &theirs, &mine);
        let swapped = outcome_within(limit, &base, &mine, &theirs);
        prop_assert_eq!(
            forward == Outcome::TooLarge,
            swapped == Outcome::TooLarge,
            "whether the merge is too large must not depend on which side is which"
        );
        if let Outcome::Merged(merged) = forward {
            check_merged(&base, &theirs, &mine, merged)?;
        }
    }

    #[test]
    fn mine_unchanged_gives_theirs((base, theirs, _) in triple()) {
        prop_assert_eq!(merge(&base, &theirs, &base), theirs);
    }

    #[test]
    fn theirs_unchanged_gives_mine((base, _, mine) in triple()) {
        prop_assert_eq!(merge(&base, &base, &mine), mine);
    }
}

#[test]
fn a_char_mine_deleted_does_not_come_back() {
    assert_eq!(merge("abcdef", "abcABCeDf", "abf"), "abABCDf");
}

#[test]
fn concurrent_inserts_into_one_gap_put_mine_first() {
    assert_eq!(merge("", "SS", "k"), "kSS");
    assert_eq!(merge("ab", "abSS", "abk"), "abkSS");
}

/// Strings do not say WHICH of equal chars a side deleted. Both sides deleting
/// one `b` of a run is one edit, as when both sides make the same change.
#[test]
fn equal_chars_deleted_by_both_sides_are_one_delete() {
    assert_eq!(merge("bbbaaa", "bbaaa", "bbaaa"), "bbaaa");
}

// Repeated chars: the strings no longer say which base char a side kept, so
// only properties that hold for ANY minimal diff are checked:
// - a side left at `base` yields the other side exactly;
// - against a side that only inserts, the merge holds exactly the chars of
//   `base`, plus what each side added, minus what the editing side removed
//   (counted as a multiset);
// - two insert-only sides both survive whole, in their order.

const WORDS: &[&str] = &["a", "b", " ", "  ", "\n", "ab", "ba", "the", "at", "cat\n"];

fn repeated_text() -> impl Strategy<Value = String> {
    prop_oneof![
        "[ab \n]{0,16}",
        prop::collection::vec(prop::sample::select(WORDS), 0..8).prop_map(|w| w.concat()),
    ]
}

fn fragment() -> impl Strategy<Value = String> {
    prop_oneof![
        3 => Just(String::new()),
        1 => prop::sample::select(WORDS).prop_map(str::to_owned),
        1 => "[abc \n]{1,3}",
    ]
}

/// `base` with each char kept with probability 0.7 and a fragment put into each
/// gap.
fn rewrite(base: &str) -> BoxedStrategy<String> {
    let len = base.chars().count();
    edit_with(base, prop::collection::vec(prop::bool::weighted(0.7), len))
}

/// `base` with a fragment put into each gap. Keeping every char is a constant,
/// not a generated value, so shrinking cannot turn it into a delete.
fn insert_into(base: &str) -> BoxedStrategy<String> {
    let len = base.chars().count();
    edit_with(base, Just(vec![true; len]))
}

fn edit_with(
    base: &str,
    keep: impl Strategy<Value = Vec<bool>> + 'static,
) -> BoxedStrategy<String> {
    let chars: Vec<char> = base.chars().collect();
    (keep, prop::collection::vec(fragment(), chars.len() + 1))
        .prop_map(move |(kept, inserts)| {
            let mut out = String::new();
            for gap in 0..=chars.len() {
                out.push_str(&inserts[gap]);
                if gap < chars.len() && kept[gap] {
                    out.push(chars[gap]);
                }
            }
            out
        })
        .boxed()
}

fn base_with(
    sides: impl Fn(&str) -> (BoxedStrategy<String>, BoxedStrategy<String>) + 'static,
) -> impl Strategy<Value = (String, String, String)> {
    repeated_text().prop_flat_map(move |base| {
        let (a, b) = sides(&base);
        (Just(base), a, b)
    })
}

fn char_counts(s: &str) -> std::collections::BTreeMap<char, i64> {
    let mut counts = std::collections::BTreeMap::new();
    for c in s.chars() {
        *counts.entry(c).or_insert(0) += 1;
    }
    counts
}

fn is_subsequence(small: &str, big: &str) -> bool {
    let mut big = big.chars();
    small.chars().all(|c| big.any(|d| d == c))
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 2000,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn repeated_chars_theirs_unchanged_gives_mine(
        (base, mine, _) in base_with(|b| (rewrite(b), Just(String::new()).boxed()))
    ) {
        prop_assert_eq!(merge(&base, &base, &mine), mine);
    }

    #[test]
    fn repeated_chars_mine_unchanged_gives_theirs(
        (base, theirs, _) in base_with(|b| (rewrite(b), Just(String::new()).boxed()))
    ) {
        prop_assert_eq!(merge(&base, &theirs, &base), theirs);
    }

    #[test]
    fn repeated_chars_against_an_insert_only_side_count_every_edit(
        (base, edited, inserted) in base_with(|b| (rewrite(b), insert_into(b))),
        limit in prop_oneof![Just(0u64), 0..400u64, Just(u64::MAX)],
    ) {
        let mut want = char_counts(&base);
        for (side, sign) in [(&edited, 1), (&inserted, 1), (&base, -2)] {
            for (c, n) in char_counts(side) {
                *want.entry(c).or_insert(0) += sign * n;
            }
        }
        want.retain(|_, n| *n != 0);
        for (theirs, mine) in [(&edited, &inserted), (&inserted, &edited)] {
            let Outcome::Merged(merged) = outcome_within(limit, &base, theirs, mine) else {
                continue;
            };
            prop_assert_eq!(
                char_counts(&merged),
                want.clone(),
                "merge({:?}, {:?}, {:?}) = {:?}",
                base,
                theirs,
                mine,
                merged
            );
        }
    }

    #[test]
    fn repeated_chars_insert_only_sides_both_survive(
        (base, theirs, mine) in base_with(|b| (insert_into(b), insert_into(b)))
    ) {
        let merged = merge(&base, &theirs, &mine);
        prop_assert!(is_subsequence(&theirs, &merged), "theirs {:?} lost in {:?}", theirs, merged);
        prop_assert!(is_subsequence(&mine, &merged), "mine {:?} lost in {:?}", mine, merged);
        prop_assert_eq!(
            merged.chars().count() + base.chars().count(),
            theirs.chars().count() + mine.chars().count()
        );
    }
}

/// `theirs` keeps both base chars, though its line `\n` equals the base's
/// first line.
#[test]
fn an_insert_only_side_keeps_the_chars_of_a_line_it_extends() {
    let (base, theirs, mine) = ("\n ", "a\n \n\n", "\n\n ");
    let merged = merge(base, theirs, mine);
    assert!(
        is_subsequence(theirs, &merged) && is_subsequence(mine, &merged),
        "both insert-only sides must survive whole in {merged:?}"
    );
    assert_eq!(merged.chars().count(), 6, "{merged:?}");
}

#[test]
fn an_unchanged_side_yields_the_edit_despite_repeated_chars() {
    for (base, edited) in [
        ("a ", " b "),
        ("  bdca  ", "        "),
        ("ec   ", " ba    "),
        ("sat and", "milk DONE eggs buy a"),
        ("and eggs mat", "call DONE the buy milk"),
    ] {
        assert_eq!(
            merge(base, base, edited),
            edited,
            "theirs unchanged, base {base:?}"
        );
        assert_eq!(
            merge(base, edited, base),
            edited,
            "mine unchanged, base {base:?}"
        );
    }
}

/// Sides are diffed by grapheme cluster: replacing `e` by `e` + combining acute
/// replaces the whole cluster, so the other side's delete of `e` cannot leave
/// the accent dangling.
#[test]
fn a_combining_mark_stays_with_its_base_char() {
    assert_eq!(merge("e", "", "e\u{301}"), "e\u{301}");
}

/// A cluster's chars move as one unit, but a merge can still put two clusters
/// next to each other: theirs replaces the family, mine appends a child.
#[test]
fn concurrent_edits_of_one_emoji_cluster_keep_both_clusters() {
    assert_eq!(merge("👨‍👩‍👧", "👨‍👩", "👨‍👩‍👧👦"), "👨‍👩👦");
}

const MARKER: &str = "ZZSECRETZZ";
const MARKER_AT: usize = 100;

/// xorshift64*, so the large texts below are the same on every run.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 33) as usize % n
    }
}

/// `len` chars over `abcd` with [`MARKER`] at [`MARKER_AT`], and a line break
/// after every `line_len` chars when given.
fn noise(len: usize, line_len: Option<usize>) -> Vec<char> {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut text: Vec<char> = (0..len)
        .map(|i| match line_len {
            Some(n) if i % n == n - 1 => '\n',
            _ => b"abcd"[rng.below(4)] as char,
        })
        .collect();
    text.splice(MARKER_AT..MARKER_AT + MARKER.len(), MARKER.chars());
    text
}

/// `text` with `count` letters at scattered positions replaced by one of
/// `wxyz`, none within two chars of [`MARKER`].
fn substitute(text: &[char], count: usize) -> String {
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let mut out = text.to_vec();
    let near_marker = MARKER_AT - 2..MARKER_AT + MARKER.len() + 2;
    let mut done = 0;
    while done < count {
        let at = rng.below(out.len());
        if near_marker.contains(&at) || !"abcd".contains(out[at]) {
            continue;
        }
        out[at] = b"wxyz"[rng.below(4)] as char;
        done += 1;
    }
    out.into_iter().collect()
}

#[test]
fn a_delete_holds_against_a_side_with_many_edits() {
    for (line_len, edits) in [(None, 3_000), (Some(60), 50)] {
        let base = noise(60_000, line_len);
        let theirs = substitute(&base, edits);
        let base: String = base.into_iter().collect();
        let mine = base.replacen(MARKER, "", 1);
        let want = theirs.replacen(MARKER, "", 1);
        for (theirs, mine) in [(&theirs, &mine), (&mine, &theirs)] {
            match outcome(&base, theirs, mine) {
                Outcome::Merged(merged) => assert!(
                    merged == want,
                    "lines of {line_len:?}: the merge must drop the marker one side deleted and \
                     keep the other side's {edits} substitutions; marker in merge: {}",
                    merged.contains(MARKER)
                ),
                Outcome::TooLarge => assert!(edits > 50, "{edits} edits are merged"),
            }
        }
    }
}

#[test]
fn a_side_rewriting_a_long_text_is_too_large_to_merge() {
    let base: String = noise(60_000, None).into_iter().collect();
    let theirs: String = base.chars().rev().collect();
    let mine = format!("{base}Q");
    for (theirs, mine) in [(&theirs, &mine), (&mine, &theirs)] {
        assert_eq!(outcome(&base, theirs, mine), Outcome::TooLarge);
    }
}
