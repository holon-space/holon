//! Pure reference-side advice-weave expectation (ADR 0021/0022) over the
//! reference blocks map — no `ReferenceState` methods, no I/O.
//!
//! These functions encode EXACTLY the read-time contract pinned in
//! `crates/holon-advice/tests/matview_build.rs` and the synthesized DDL in
//! `crates/holon-advice/src/synthesis.rs`:
//!
//! - The matview (`advice_matview_rows` / [`matview_rows_for`]) is the
//!   anchor-denormalized `block_tags` self-join: one row per (anchor, candidate)
//!   with `shared_tag_count = COUNT(*)` over shared tags — the FULL intersection,
//!   marker/selector tags included when genuinely shared. No suppression, no K.
//! - The read-time weave ([`expectation_for`]) drops candidates the anchor has
//!   suppressed (the `advice_suppressed(anchor_id, lesson_id)` anti-join),
//!   sorts by count DESC, and carries the rule's K.
//!
//! v1 narrowings (all liftable, all fail loud): the keystone generator mints a
//! single active rule whose anchor and candidate source are both
//! `AnchorSelector::HasTag` — any other selector variant `unreachable!`s.

use std::collections::BTreeMap;

use holon_advice::{
    ADVICE_RULE_SOURCE_LANGUAGE, AdviceRule, AnchorSelector, ScoringTemplate, parse_advice_rule,
};
use holon_api::Block;
use holon_api::entity_uri::EntityUri;
use holon_pbt_core::capabilities::AdviceExpectation;

/// Blocks whose `source_language` marks them advice-rule source blocks. Mirrors
/// the `source_language` comparison in `ReferenceState::rebuild_profile_tracking`.
pub fn advice_rule_blocks(blocks: &BTreeMap<EntityUri, Block>) -> Vec<(EntityUri, &Block)> {
    blocks
        .iter()
        .filter(|(_, b)| {
            b.source_language
                .as_ref()
                .map(|sl| sl.to_string())
                .as_deref()
                == Some(ADVICE_RULE_SOURCE_LANGUAGE)
        })
        .map(|(k, b)| (k.clone(), b))
        .collect()
}

/// Advice-rule blocks EXCLUDING seed-classified ones. The bundled INACTIVE
/// `index.org` rule seeds into every vault AND enters the reference
/// (`seed_booted_layout_into_ref`), where `block_documents` maps it to the
/// no-parent/sentinel doc — the same seed classification
/// `ReferenceState::rebuild_profile_tracking` skips. The `WriteOrgFile`
/// mint-a-rule gate (generation arm + shrink-time precondition) must count
/// THESE, not all rule blocks: counting the seed would leave the arm
/// permanently vacuous. The oracle (`active_rule`) deliberately still scans
/// ALL blocks — the seed is inactive and contributes nothing, and a
/// user-activated seed rule must not be invisible to the oracle.
pub fn non_seed_advice_rule_blocks(
    block_state: &crate::pbt::reference_state::BlockState,
) -> Vec<(EntityUri, &Block)> {
    advice_rule_blocks(&block_state.blocks)
        .into_iter()
        .filter(|(id, _)| {
            !block_state
                .block_documents
                .get(id)
                .is_some_and(|doc| doc.is_no_parent() || doc.is_sentinel())
        })
        .collect()
}

/// The single active advice rule, if any. Unparseable rule blocks contribute
/// nothing (prod refusal semantics: a rule that fails to parse builds no
/// matview), so `.ok()` here DROPS refused rules by design — it is not an error
/// swallow. The generator guarantees ≤1 active rule; more is a harness bug.
pub fn active_rule(blocks: &BTreeMap<EntityUri, Block>) -> Option<AdviceRule> {
    let active: Vec<AdviceRule> = advice_rule_blocks(blocks)
        .into_iter()
        .filter_map(|(_, b)| parse_advice_rule(&b.content).ok())
        .filter(|r| r.active)
        .collect();
    assert!(
        active.len() <= 1,
        "v1 keystone generator guarantees at most one active advice rule per run; \
         found {} (liftable narrowing) — harness bug",
        active.len(),
    );
    active.into_iter().next()
}

/// The tag of a `HasTag` selector. v1 mints only `HasTag`; any other variant is
/// a generator/harness bug and fails loud (liftable narrowing).
fn has_tag_only(selector: &AnchorSelector) -> &str {
    match selector {
        AnchorSelector::HasTag(tag) => tag,
        _ => unreachable!("v1 keystone generator mints has_tag selectors only"),
    }
}

/// Candidates for one anchor WITHOUT suppression: every block `c != anchor`
/// carrying `candidate_tag` that shares ≥1 tag with the anchor, scored by the
/// full shared-tag count (the DDL's `COUNT(*)` over the `block_tags` self-join —
/// no tag is excluded).
fn scored_candidates(
    blocks: &BTreeMap<EntityUri, Block>,
    anchor_id: &EntityUri,
    anchor: &Block,
    candidate_tag: &str,
) -> Vec<(EntityUri, u32)> {
    let anchor_tags = anchor.tags.to_set();
    let mut out = Vec::new();
    for (cid, c) in blocks {
        if cid == anchor_id || !c.tags.contains(candidate_tag) {
            continue;
        }
        let count = c
            .tags
            .iter()
            .filter(|t| anchor_tags.contains(t.as_str()))
            .count() as u32;
        if count >= 1 {
            out.push((cid.clone(), count));
        }
    }
    out
}

/// Read-time expectation for one anchor: empty when the anchor block is absent
/// or lacks the rule's anchor tag; otherwise the scored candidates with
/// anchor-suppressed lessons removed, sorted by count DESC, carrying the rule K.
pub fn expectation_for(
    blocks: &BTreeMap<EntityUri, Block>,
    rule: &AdviceRule,
    anchor_id: &EntityUri,
) -> AdviceExpectation {
    let anchor_tag = has_tag_only(&rule.anchor);
    let Some(anchor) = blocks.get(anchor_id) else {
        return AdviceExpectation::default();
    };
    if !anchor.tags.contains(anchor_tag) {
        return AdviceExpectation::default();
    }
    let ScoringTemplate::TagOverlapRecency(spec) = &rule.candidates;
    let candidate_tag = has_tag_only(&spec.source);

    // Suppression direction: the ANCHOR carries the edge, the candidate (lesson)
    // is the target — the `advice_suppressed(anchor_id, lesson_id)` junction.
    let mut scored: Vec<(String, u32)> =
        scored_candidates(blocks, anchor_id, anchor, candidate_tag)
            .into_iter()
            .filter(|(cid, _)| !anchor.advice_suppressed.contains(cid))
            .map(|(cid, n)| (cid.as_str().to_string(), n))
            .collect();
    scored.sort_by(|a, b| b.1.cmp(&a.1));
    AdviceExpectation {
        scored,
        k: rule.k.get(),
    }
}

/// The un-capped, un-suppressed matview contract: all (anchor, candidate, count)
/// triples of the rule over every anchor block. No suppression filter, no K.
pub fn matview_rows_for(
    blocks: &BTreeMap<EntityUri, Block>,
    rule: &AdviceRule,
) -> Vec<(EntityUri, EntityUri, u32)> {
    let anchor_tag = has_tag_only(&rule.anchor);
    let ScoringTemplate::TagOverlapRecency(spec) = &rule.candidates;
    let candidate_tag = has_tag_only(&spec.source);

    let mut rows = Vec::new();
    for (aid, a) in blocks {
        if !a.tags.contains(anchor_tag) {
            continue;
        }
        for (cid, n) in scored_candidates(blocks, aid, a, candidate_tag) {
            rows.push((aid.clone(), cid, n));
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use holon_api::Tags;

    const RULE_YAML: &str = "name: lessons_for_tasks\nactive: true\nk: 5\n\
        anchor:\n  has_tag: task\ncandidates:\n  tag_overlap_recency:\n    source:\n      has_tag: lesson\n";

    fn parent() -> EntityUri {
        EntityUri::parse("block:root").unwrap()
    }

    fn block(id: &str, tags: &[&str]) -> Block {
        let mut b = Block::new_text(EntityUri::parse(id).unwrap(), parent(), String::new());
        b.tags = Tags::from_tag_iter(tags.iter().map(|t| t.to_string()));
        b
    }

    fn rule_block(id: &str, yaml: &str) -> Block {
        Block::new_source(
            EntityUri::parse(id).unwrap(),
            parent(),
            ADVICE_RULE_SOURCE_LANGUAGE,
            yaml,
        )
    }

    fn map(blocks: Vec<Block>) -> BTreeMap<EntityUri, Block> {
        blocks.into_iter().map(|b| (b.id.clone(), b)).collect()
    }

    fn rule() -> AdviceRule {
        parse_advice_rule(RULE_YAML).unwrap()
    }

    fn anchor(id: &str) -> EntityUri {
        EntityUri::parse(id).unwrap()
    }

    #[test]
    fn tag_overlap_counts_full_intersection_including_marker_tags() {
        // task1 ∩ lessonA = {proj, urgent} = 2; ∩ lessonB = {proj} = 1.
        let blocks = map(vec![
            block("block:task1", &["task", "proj", "urgent"]),
            block("block:lessonA", &["lesson", "proj", "urgent"]),
            block("block:lessonB", &["lesson", "proj"]),
            block("block:lessonC", &["lesson", "other"]),
        ]);
        let exp = expectation_for(&blocks, &rule(), &anchor("block:task1"));
        assert_eq!(
            exp.scored,
            vec![("block:lessonA".to_string(), 2), ("block:lessonB".to_string(), 1)],
        );
        assert_eq!(exp.k, 5);
    }

    #[test]
    fn shared_marker_tag_counts() {
        // Both carry `lesson`, and they share `lesson` genuinely → counts toward
        // overlap. anchorL(lesson,task) ∩ cand(lesson) = {lesson} = 1.
        let blocks = map(vec![
            block("block:a", &["task", "lesson"]),
            block("block:c", &["lesson"]),
        ]);
        let exp = expectation_for(&blocks, &rule(), &anchor("block:a"));
        assert_eq!(exp.scored, vec![("block:c".to_string(), 1)]);
    }

    #[test]
    fn suppression_drops_only_the_anchor_dismissed_lesson() {
        let mut blocks = map(vec![
            block("block:task1", &["task", "proj", "urgent"]),
            block("block:lessonA", &["lesson", "proj", "urgent"]),
            block("block:lessonB", &["lesson", "proj"]),
        ]);
        blocks
            .get_mut(&anchor("block:task1"))
            .unwrap()
            .advice_suppressed = vec![anchor("block:lessonA")];
        let exp = expectation_for(&blocks, &rule(), &anchor("block:task1"));
        assert_eq!(exp.scored, vec![("block:lessonB".to_string(), 1)]);
    }

    #[test]
    fn suppression_is_directional_candidate_side_does_not_suppress() {
        // A suppression edge on the CANDIDATE (not the anchor) must NOT remove it.
        let mut blocks = map(vec![
            block("block:task1", &["task", "proj"]),
            block("block:lessonA", &["lesson", "proj"]),
        ]);
        blocks
            .get_mut(&anchor("block:lessonA"))
            .unwrap()
            .advice_suppressed = vec![anchor("block:task1")];
        let exp = expectation_for(&blocks, &rule(), &anchor("block:task1"));
        assert_eq!(exp.scored, vec![("block:lessonA".to_string(), 1)]);
    }

    #[test]
    fn anchor_excludes_itself_even_when_it_carries_the_candidate_tag() {
        // A block tagged BOTH task and lesson must not weave itself.
        let blocks = map(vec![
            block("block:both", &["task", "lesson", "proj"]),
            block("block:l", &["lesson", "proj"]),
        ]);
        // both ∩ l = {lesson, proj} = 2 (full intersection, shared marker included);
        // the point of the test is `both` does NOT weave itself despite carrying `lesson`.
        let exp = expectation_for(&blocks, &rule(), &anchor("block:both"));
        assert_eq!(exp.scored, vec![("block:l".to_string(), 2)]);
    }

    #[test]
    fn non_matching_anchor_has_empty_total_expectation() {
        let blocks = map(vec![
            block("block:note1", &["note", "proj"]),
            block("block:lessonA", &["lesson", "proj"]),
        ]);
        let exp = expectation_for(&blocks, &rule(), &anchor("block:note1"));
        assert_eq!(exp, AdviceExpectation::default());
        assert_eq!(exp.k, 0);
    }

    #[test]
    fn absent_anchor_has_empty_total_expectation() {
        let blocks = map(vec![block("block:lessonA", &["lesson", "proj"])]);
        let exp = expectation_for(&blocks, &rule(), &anchor("block:ghost"));
        assert_eq!(exp, AdviceExpectation::default());
    }

    #[test]
    fn sorted_desc_by_shared_tag_count() {
        let blocks = map(vec![
            block("block:task1", &["task", "a", "b", "c"]),
            block("block:one", &["lesson", "a"]),
            block("block:three", &["lesson", "a", "b", "c"]),
            block("block:two", &["lesson", "a", "b"]),
        ]);
        let exp = expectation_for(&blocks, &rule(), &anchor("block:task1"));
        let counts: Vec<u32> = exp.scored.iter().map(|(_, n)| *n).collect();
        assert_eq!(counts, vec![3, 2, 1]);
    }

    #[test]
    fn unparseable_rule_block_yields_no_active_rule() {
        let blocks = map(vec![rule_block("block:r", "name: broken\nk: 99\n")]);
        assert!(active_rule(&blocks).is_none());
    }

    #[test]
    fn inactive_rule_yields_no_active_rule() {
        let inactive = RULE_YAML.replace("active: true", "active: false");
        let blocks = map(vec![rule_block("block:r", &inactive)]);
        assert!(active_rule(&blocks).is_none());
    }

    #[test]
    fn active_rule_discovered_by_source_language() {
        let blocks = map(vec![
            rule_block("block:r", RULE_YAML),
            block("block:plain", &["task"]),
        ]);
        let r = active_rule(&blocks).expect("one active rule");
        assert_eq!(r.name.as_str(), "lessons_for_tasks");
    }

    /// The bundled INACTIVE rule (assets/default/index.org) must round-trip
    /// into the booted reference: present as a rule block, seed-classified
    /// (so the WriteOrgFile mint gate ignores it — no silent vacuity), YAML
    /// parsing Ok with `active: false` (a parse failure would be silently
    /// dropped by refusal semantics — this test is the loud tripwire), and
    /// contributing no active rule (so the ≤1-ACTIVE assert cannot trip when
    /// a minted active rule coexists with the seed).
    #[test]
    fn seeded_bundled_rule_roundtrips_inactive_and_is_seed_classified() {
        let state = crate::pbt::composed::wide_e2e::wide_e2e_ref();
        let bs = &state.domain.block_state;

        let rules = advice_rule_blocks(&bs.blocks);
        assert_eq!(
            rules.len(),
            1,
            "bundled index.org rule must round-trip into the reference blocks map"
        );
        let (id, block) = &rules[0];
        let parsed = parse_advice_rule(&block.content).unwrap_or_else(|e| {
            panic!(
                "seeded bundled rule YAML must parse (block {id}): {e:#}\ncontent:\n{}",
                block.content
            )
        });
        assert!(!parsed.active, "bundled rule must ship INACTIVE");
        assert_eq!(parsed.name.as_str(), "lessons_for_tasks");
        assert!(
            active_rule(&bs.blocks).is_none(),
            "inactive seed must contribute no active rule"
        );
        assert!(
            non_seed_advice_rule_blocks(bs).is_empty(),
            "seed rule must be seed-classified so the WriteOrgFile mint gate stays live"
        );
    }

    #[test]
    fn matview_rows_span_all_anchors_without_suppression_or_k() {
        let mut blocks = map(vec![
            block("block:task1", &["task", "proj"]),
            block("block:task2", &["task", "urgent"]),
            block("block:lessonA", &["lesson", "proj", "urgent"]),
        ]);
        // A suppression on task1 must NOT affect the matview rows.
        blocks
            .get_mut(&anchor("block:task1"))
            .unwrap()
            .advice_suppressed = vec![anchor("block:lessonA")];
        let mut rows = matview_rows_for(&blocks, &rule());
        rows.sort();
        assert_eq!(
            rows,
            vec![
                (anchor("block:task1"), anchor("block:lessonA"), 1),
                (anchor("block:task2"), anchor("block:lessonA"), 1),
            ],
        );
    }
}
