//! `inv-advice-rows-woven` — snapshot-level advice weave (ADR 0021/0022/0023).
//!
//! @pbt oracle correspondence
//! @pbt covers advice-weave — top-K suppression-filtered lessons per anchor,
//!   in non-increasing score order with top-K boundary dominance, vs the ref
//!   expectation
//! @pbt slips-if-removed renderer weaves a suppressed lesson, the wrong top-K,
//!   or ascending-score order under an anchor; the advice section shows wrong
//!   recommendations and nothing else observes it
//!
//! The rendered widget tree must weave, under each anchor node, exactly the
//! advice rows the reference expects for that anchor: the top-K
//! suppression-filtered lessons, in non-increasing score order, each stamped
//! with the placed-occurrence key derived from `(lesson, anchor)`.
//!
//! ## Per-anchor attribution over the multi-node render (step 6)
//!
//! The weave is live: the renderer stamps each placed advice row with the
//! `Occurrence::Placed` key derived from `(lesson, anchor)` and appends it
//! under the anchor. A single block id decorates MANY nodes in the snapshot
//! (tree_item, column, selectable, state_toggle, rendered_text, drop_zone,
//! draggable), so we cannot demand every id-bearing node carry the advice — the
//! woven rows are direct children of only ONE of them, and the childless leaves
//! would each observe `[]` against a non-empty expectation and fail "wove 0".
//! Instead, mirroring `inv-viewmodel-tree-virtual-slots`' effective-entity-id
//! dedup, we collapse the multi-node decoration to one entry per DISTINCT
//! anchor id and attribute each placed row to its anchor by the one-way
//! `for_placement(lesson, anchor)` suffix (a placed row matches AT MOST one
//! anchor). A placed row matching NO rendered anchor is surfaced as a foreign /
//! mis-placed row (see the claim-all caveat) rather than silently absorbed.
//!
//! ## Expansion-state caveat (revisit at step 6)
//!
//! ADR 0023 wants advice sections collapsed-by-default in production. This
//! invariant asserts the STRONG (always-woven) form: every rendered occurrence
//! of an anchor must carry its full expected advice. Step 6 must either
//! default-expand advice in the keystone config or add an expand transition +
//! model expansion state and weaken this to the visible-when-expanded form.
//!
//! ## v1 claim-all simplification (revisit when B-track placement lands)
//!
//! Today NOTHING else in the keystone mints `Occurrence::Placed`, so every
//! placed child under an anchor is claimed as an advice row here. When the
//! B-track transclusion/placement transition lands its own placed rows, this
//! claim-all must be revisited — a placed child whose stamp does not match
//! `for_placement(lesson, anchor)` is already reported as a foreign placed row
//! rather than silently absorbed.
//!
//! ## Bound
//!
//! `R: RefAdvice` for the per-anchor expectation; `S: SutRenderer` for the
//! widget tree.

use holon_api::EntityUri;
use holon_api::Occurrence;
use holon_api::OccurrenceId;
use holon_pbt_core::capabilities::AdviceExpectation;
use holon_pbt_core::capabilities::RefAdvice;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvAdviceRowsWoven;

impl InvAdviceRowsWoven {
    pub const ID: InvariantId = InvariantId("inv-advice-rows-woven");
}

/// Whether an entity id is a creation-placeholder slot (single source of the
/// `:__virtual:` detection is `RowOrigin`, as in
/// `viewmodel_tree_virtual_slots`).
fn is_creation_placeholder(id: &str) -> bool {
    holon_frontend::row_origin::RowOrigin::from_id(id).is_creation_placeholder()
}

/// The relation between the reference expectation and the advice lesson ids
/// observed in render order under one anchor. Pure — unit-tested below.
///
/// - observed ids pairwise distinct;
/// - every observed id is one of `exp.scored`;
/// - `|observed| == min(exp.k, exp.scored.len())` (this also forces observed
///   empty for an empty expectation — teardown: rule deleted / inactive /
///   unparseable, lesson deleted, or anchor untagged);
/// - scores of observed, in render order, are non-increasing;
/// - `min(observed scores) >= max score among exp.scored NOT observed` (top-K
///   boundary dominance).
///
/// Recency order within equal scores is deliberately NOT asserted: `updated_at`
/// is SUT wall-clock, unobservable to the reference, and ADR 0023's app-layer
/// reranker supersedes it anyway.
pub fn check_advice_relation(exp: &AdviceExpectation, observed: &[String]) -> Result<(), String> {
    // pairwise distinct
    for (i, a) in observed.iter().enumerate() {
        if observed[i + 1..].contains(a) {
            return Err(format!(
                "[inv-advice-rows-woven] duplicate woven advice row '{a}' (observed order: \
                 {observed:?})"
            ));
        }
    }

    let score_of = |id: &str| exp.scored.iter().find(|(s, _)| s == id).map(|(_, n)| *n);

    // every observed id is an expected candidate
    for id in observed {
        if score_of(id).is_none() {
            return Err(format!(
                "[inv-advice-rows-woven] woven advice row '{id}' is not in the reference \
                 expectation (scored candidates: {:?})",
                exp.scored,
            ));
        }
    }

    // cardinality: exactly the top-K (or all candidates when fewer than K)
    let want = (exp.k as usize).min(exp.scored.len());
    if observed.len() != want {
        return Err(format!(
            "[inv-advice-rows-woven] wove {} advice rows but expected {want} (k={}, {} scored \
             candidates); observed={observed:?}, scored={:?}",
            observed.len(),
            exp.k,
            exp.scored.len(),
            exp.scored,
        ));
    }

    // observed scores in render order (each id proven present above)
    let observed_scores: Vec<u32> = observed.iter().map(|id| score_of(id).unwrap()).collect();

    // non-increasing render order
    for w in observed_scores.windows(2) {
        if w[0] < w[1] {
            return Err(format!(
                "[inv-advice-rows-woven] woven advice not in non-increasing score order: \
                 scores={observed_scores:?} for observed={observed:?}"
            ));
        }
    }

    // top-K boundary dominance: every observed score >= every non-observed score
    let min_observed = observed_scores.iter().copied().min();
    let max_dropped = exp
        .scored
        .iter()
        .filter(|(id, _)| !observed.contains(id))
        .map(|(_, n)| *n)
        .max();
    if let (Some(min_obs), Some(max_drop)) = (min_observed, max_dropped) {
        if min_obs < max_drop {
            return Err(format!(
                "[inv-advice-rows-woven] top-K boundary violated: a dropped candidate scores \
                 {max_drop} but a woven row scores only {min_obs} (observed={observed:?}, \
                 scored={:?})",
                exp.scored,
            ));
        }
    }

    Ok(())
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvAdviceRowsWoven
where
    R: RefAdvice,
    S: SutRenderer,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let root = sut.widget_tree_snapshot().await;

        // Gate: nothing rendered yet / renderer unavailable — no structure to
        // assert on (same not-ready convention as the sibling ViewModel bodies).
        if root.collect_entity_ids().is_empty() {
            return InvariantResult::Ok;
        }

        // ── Placed advice rows, in render (pre-order) order ─────────────────
        // A placed row is ONE node carrying both `props["occurrence"]` (the
        // occurrence key suffix) and `entity_id` (the lesson id). Collecting
        // occurrence-stamped nodes directly over the whole tree avoids the
        // per-child-subtree scan and captures every woven row regardless of the
        // wrapper level it sits at.
        let placed: Vec<(&str, &str)> = root
            .walk()
            .filter_map(|n| {
                n.props.get("occurrence").map(|suffix| {
                    let lesson = n.entity_id.as_deref().expect(
                        "a placed advice row stamps props[\"occurrence\"] on its entity_id \
                         (lesson) node",
                    );
                    (lesson, suffix.as_str())
                })
            })
            .collect();

        // ── Distinct candidate anchors, first-seen (render) order ───────────
        // A single block id decorates ~7 nested nodes; collapse to one entry per
        // id (mirroring `viewmodel_tree_virtual_slots`' effective-id dedup). Keep
        // the first-seen node's kind for error context. Anchors never carry an
        // occurrence stamp (that marks placed rows), and must be domain
        // EntityUris that are not creation placeholders.
        let mut anchors: Vec<(EntityUri, String)> = Vec::new();
        let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for node in root.walk() {
            if node.props.contains_key("occurrence") {
                continue;
            }
            let Some(id) = node.entity_id.as_deref() else {
                continue;
            };
            if is_creation_placeholder(id) {
                continue;
            }
            let Ok(uri) = EntityUri::parse(id) else {
                continue;
            };
            if seen.insert(uri.as_str().to_string()) {
                anchors.push((uri, node.kind.clone()));
            }
        }

        // ── Attribute + check per distinct anchor ───────────────────────────
        for (anchor_uri, kind) in &anchors {
            // Placed rows whose (lesson, anchor) suffix matches THIS anchor, in
            // render order. The suffix is a one-way hash of (lesson, anchor), so
            // each placed row matches at most one anchor.
            let observed: Vec<String> = placed
                .iter()
                .filter_map(|(lesson, suffix)| {
                    let lesson_uri = EntityUri::parse(lesson).unwrap_or_else(|_| {
                        panic!("placed advice row carried a non-EntityUri lesson id '{lesson}'")
                    });
                    let expected =
                        Occurrence::Placed(OccurrenceId::for_placement(&lesson_uri, anchor_uri))
                            .key_suffix();
                    (*suffix == expected).then(|| lesson.to_string())
                })
                .collect();

            let exp = ref_.advice_expectation(anchor_uri.as_str());
            // Fast path: no expectation and nothing woven — the common case for
            // every non-anchor block.
            if exp.scored.is_empty() && observed.is_empty() {
                continue;
            }
            if let Err(msg) = check_advice_relation(&exp, &observed) {
                return InvariantResult::Fail(format!(
                    "{msg}\n  anchor='{}' (kind '{kind}')",
                    anchor_uri.as_str(),
                ));
            }
        }

        // ── Foreign / orphan placed-row tooth (ADR 0015 claim-all caveat) ───
        // Every placed row MUST attribute to exactly one rendered anchor. A
        // placed row that matches no rendered anchor's `for_placement` key is a
        // foreign / B-track / mis-placed row this invariant must surface rather
        // than silently absorb.
        for (lesson, suffix) in &placed {
            let lesson_uri = EntityUri::parse(lesson).unwrap_or_else(|_| {
                panic!("placed advice row carried a non-EntityUri lesson id '{lesson}'")
            });
            let matched = anchors.iter().any(|(anchor_uri, _)| {
                Occurrence::Placed(OccurrenceId::for_placement(&lesson_uri, anchor_uri))
                    .key_suffix()
                    == *suffix
            });
            if !matched {
                let anchor_ids: Vec<&str> = anchors.iter().map(|(u, _)| u.as_str()).collect();
                return InvariantResult::Fail(format!(
                    "[inv-advice-rows-woven] placed advice row '{lesson}' (occurrence suffix \
                     {suffix:?}) matches no rendered anchor's for_placement(lesson, anchor) key — \
                     a placed row under no rendered anchor is a foreign / B-track / mis-placed \
                     row this advice invariant must surface (distinct rendered anchors: \
                     {anchor_ids:?})"
                ));
            }
        }

        InvariantResult::Ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exp(scored: &[(&str, u32)], k: u8) -> AdviceExpectation {
        AdviceExpectation {
            scored: scored.iter().map(|(s, n)| (s.to_string(), *n)).collect(),
            k,
        }
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn empty_expectation_requires_no_woven_rows() {
        assert!(check_advice_relation(&exp(&[], 0), &ids(&[])).is_ok());
        assert!(check_advice_relation(&exp(&[], 0), &ids(&["block:x"])).is_err());
    }

    #[test]
    fn exact_topk_in_descending_order_passes() {
        let e = exp(&[("block:a", 3), ("block:b", 2), ("block:c", 1)], 2);
        assert!(check_advice_relation(&e, &ids(&["block:a", "block:b"])).is_ok());
    }

    #[test]
    fn all_candidates_when_k_exceeds_candidate_count() {
        let e = exp(&[("block:a", 3), ("block:b", 1)], 5);
        assert!(check_advice_relation(&e, &ids(&["block:a", "block:b"])).is_ok());
    }

    #[test]
    fn too_few_woven_fails() {
        // The step-4→step-6 red case: expectation non-empty, weave absent.
        let e = exp(&[("block:a", 3), ("block:b", 2)], 2);
        assert!(check_advice_relation(&e, &ids(&[])).is_err());
        assert!(check_advice_relation(&e, &ids(&["block:a"])).is_err());
    }

    #[test]
    fn too_many_woven_fails() {
        let e = exp(&[("block:a", 3), ("block:b", 2), ("block:c", 1)], 2);
        assert!(check_advice_relation(&e, &ids(&["block:a", "block:b", "block:c"])).is_err());
    }

    #[test]
    fn foreign_id_fails() {
        let e = exp(&[("block:a", 3), ("block:b", 2)], 2);
        assert!(check_advice_relation(&e, &ids(&["block:a", "block:zzz"])).is_err());
    }

    #[test]
    fn duplicate_woven_fails() {
        let e = exp(&[("block:a", 3), ("block:b", 2)], 2);
        assert!(check_advice_relation(&e, &ids(&["block:a", "block:a"])).is_err());
    }

    #[test]
    fn ascending_render_order_fails() {
        let e = exp(&[("block:a", 3), ("block:b", 2)], 2);
        assert!(check_advice_relation(&e, &ids(&["block:b", "block:a"])).is_err());
    }

    #[test]
    fn topk_boundary_dominance_enforced() {
        // Two candidates tie at 2 above one at 1; K=2. Weaving the low pair
        // {a(2), c(1)} instead of {a(2), b(2)} drops b(2) below a woven c(1).
        let e = exp(&[("block:a", 2), ("block:b", 2), ("block:c", 1)], 2);
        assert!(check_advice_relation(&e, &ids(&["block:a", "block:c"])).is_err());
        // Weaving the two top-scorers passes regardless of which equal-score
        // pair order (recency not asserted).
        assert!(check_advice_relation(&e, &ids(&["block:a", "block:b"])).is_ok());
        assert!(check_advice_relation(&e, &ids(&["block:b", "block:a"])).is_ok());
    }

    #[test]
    fn equal_scores_within_k_allow_either_recency_order() {
        let e = exp(&[("block:a", 2), ("block:b", 2)], 2);
        assert!(check_advice_relation(&e, &ids(&["block:a", "block:b"])).is_ok());
        assert!(check_advice_relation(&e, &ids(&["block:b", "block:a"])).is_ok());
    }
}
