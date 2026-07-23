//! Inc B — first/last-child generation bias for block-modifying transitions.
//!
//! Block-modifying transitions (edit-via-click / toggle / split / indent /
//! outdent) pick their target from a DOCUMENT-ORDERED candidate list. The
//! drive layer must scroll a target into view before interacting with it
//! (fail-loud since this week's scroll fixes), so preferring the extreme rows
//! exercises below-fold (last) and top-of-fold (first) reachability as a
//! precondition of real actions on every composed run — the interior stays
//! reachable, the extremes (most likely off-screen) just get hit more often.

use proptest::strategy::BoxedStrategy;
use proptest::strategy::Strategy;

/// Baseline copies of every candidate. Scales the denominator so the edge
/// boost is a SLIGHT relative nudge rather than a coarse one.
const BASE_COPIES: usize = 4;
/// Extra copies of the first AND last candidate on top of `BASE_COPIES`.
const EDGE_EXTRA_COPIES: usize = 1;

/// Select a target from a document-ordered `candidates` list with a SLIGHT
/// bias toward the first and last element.
///
/// Construction: a single uniform `select` over a vector that repeats every
/// candidate `BASE_COPIES` times, with `EDGE_EXTRA_COPIES` additional copies
/// of the first and last. So each edge carries `BASE_COPIES +
/// EDGE_EXTRA_COPIES` weight and each interior element `BASE_COPIES`. With the
/// defaults (`4` base, `+1` edge) and `N=6` real candidates: each edge's mass
/// is `5/26 ≈ 0.192` and each interior's is `4/26 ≈ 0.154` (uniform would be
/// `0.167`) — a ~+15% edge lift, ~−8% interior, i.e. slight and symmetric.
/// Below 3 candidates there is no distinct interior to distort, so a plain
/// uniform `select` is returned unchanged.
///
/// Shrinking is preserved BY CONSTRUCTION: this is one `select` whose index
/// value-tree shrinks monotonically toward index 0. The vector is laid out in
/// document order (first candidate at the front), so every draw — interior or
/// last-edge — minimises back through the interior to the first candidate.
/// (An earlier `Union` of `Just(first)`/`Just(last)` arms was rejected: those
/// arms are shrink-inert and proptest's `Union` shrinks only WITHIN the chosen
/// arm, trapping a last-edge failure at the last element — caught by
/// `edge_bias_shrinks_across_arms_to_smallest_failing_candidate` below.)
pub(crate) fn select_with_edge_bias<T>(candidates: Vec<T>) -> BoxedStrategy<T>
where
    T: Clone + std::fmt::Debug + 'static,
{
    let n = candidates.len();
    if n < 3 {
        return proptest::sample::select(candidates).boxed();
    }
    let mut weighted: Vec<T> = Vec::with_capacity(n * BASE_COPIES + 2 * EDGE_EXTRA_COPIES);
    for _ in 0..EDGE_EXTRA_COPIES {
        weighted.push(candidates[0].clone());
    }
    for c in &candidates {
        for _ in 0..BASE_COPIES {
            weighted.push(c.clone());
        }
    }
    for _ in 0..EDGE_EXTRA_COPIES {
        weighted.push(candidates[n - 1].clone());
    }
    proptest::sample::select(weighted).boxed()
}

#[cfg(test)]
mod tests {
    use proptest::prop_assert_eq;
    use proptest::strategy::Strategy;
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestError;
    use proptest::test_runner::TestRunner;

    use super::select_with_edge_bias;

    /// Shrinking must keep working (Inc B invariant). Property `v == 0` fails
    /// for every non-first candidate, so a failing case must minimise DOWN to
    /// candidate `1` (candidate `0` passes). We assert the minimised value is
    /// `1` — never the last element — proving a last-edge draw shrinks back
    /// through the interior to the front rather than getting trapped.
    #[test]
    fn edge_bias_shrinks_across_arms_to_smallest_failing_candidate() {
        let n = 6u32;
        let mut exercised = 0u32;
        for _ in 0..300 {
            let strat = select_with_edge_bias((0..n).collect::<Vec<u32>>());
            let mut runner = TestRunner::default();
            match runner.run(&strat, |v| {
                prop_assert_eq!(v, 0);
                Ok(())
            }) {
                Err(TestError::Fail(_, minimal)) => {
                    exercised += 1;
                    assert_eq!(
                        minimal, 1,
                        "edge-bias shrink trapped at {minimal}; failed to minimise \
                         toward the first candidate",
                    );
                }
                Ok(()) => {}
                Err(e) => panic!("unexpected runner error: {e:?}"),
            }
        }
        assert!(
            exercised > 0,
            "no failing case was ever generated — test is vacuous"
        );
    }

    /// Sanity: every produced value is a member of the candidate set (the bias
    /// never invents an out-of-range target).
    #[test]
    fn edge_bias_only_produces_candidates() {
        let candidates: Vec<u32> = vec![10, 20, 30, 40, 50];
        let strat = select_with_edge_bias(candidates.clone());
        let mut runner = TestRunner::default();
        runner
            .run(&strat, |v| {
                prop_assert_eq!(candidates.contains(&v), true);
                Ok(())
            })
            .expect("all produced values must be candidates");
    }

    /// The edge elements really are boosted (distribution check): over many
    /// draws the first and last are each sampled MORE than a mid element.
    #[test]
    fn edge_bias_boosts_first_and_last_over_interior() {
        use std::collections::BTreeMap;
        let n = 6usize;
        let strat = select_with_edge_bias((0..n as u32).collect::<Vec<u32>>());
        let mut runner = TestRunner::default();
        let mut counts: BTreeMap<u32, u32> = BTreeMap::new();
        for _ in 0..6000 {
            let v = strat.new_tree(&mut runner).expect("new_tree").current();
            *counts.entry(v).or_default() += 1;
        }
        let first = counts[&0];
        let last = counts[&(n as u32 - 1)];
        let mid = counts[&3];
        assert!(
            first > mid && last > mid,
            "edges must outweigh interior: first={first} last={last} mid={mid}",
        );
    }
}
