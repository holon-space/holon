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

use holon_filesystem::ThreeWayTextMerge;
use holon_loro::TransientLoroTextMerge;
use proptest::prelude::*;

const BASE_POOL: &str = "abcdefghijkl𝔞𝔟";
const THEIRS_POOL: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ𝔸";
const MINE_POOL: &str = "0123456789αβγδεζηθικλμνξ😀";

fn merge(base: &str, theirs: &str, mine: &str) -> String {
    TransientLoroTextMerge
        .merge_text(base, theirs, mine)
        .unwrap_or_else(|e| panic!("merge_text({base:?}, {theirs:?}, {mine:?}) failed: {e:#}"))
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
    let merged = merge(base, theirs, mine);
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

const WORDS: &[&str] = &["a", "b", " ", "  ", "ab", "ba", "the", "at", "cat"];

fn repeated_text() -> impl Strategy<Value = String> {
    prop_oneof![
        "[ab ]{0,16}",
        prop::collection::vec(prop::sample::select(WORDS), 0..8).prop_map(|w| w.concat()),
    ]
}

fn fragment() -> impl Strategy<Value = String> {
    prop_oneof![
        3 => Just(String::new()),
        1 => prop::sample::select(WORDS).prop_map(str::to_owned),
        1 => "[abc ]{1,3}",
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
        (base, edited, inserted) in base_with(|b| (rewrite(b), insert_into(b)))
    ) {
        let mut want = char_counts(&base);
        for (side, sign) in [(&edited, 1), (&inserted, 1), (&base, -2)] {
            for (c, n) in char_counts(side) {
                *want.entry(c).or_insert(0) += sign * n;
            }
        }
        want.retain(|_, n| *n != 0);
        for (theirs, mine) in [(&edited, &inserted), (&inserted, &edited)] {
            let merged = merge(&base, theirs, mine);
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
