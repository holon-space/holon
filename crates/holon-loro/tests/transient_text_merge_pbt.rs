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
