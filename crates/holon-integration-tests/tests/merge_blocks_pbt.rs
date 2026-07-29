//! `merge_blocks` Inc 1 — the duplicate-identity merge, op-level.
//!
//! The motivating shape is the split doc root (see
//! `split_doc_root_idless_duplicates`): two blocks carry the same identity,
//! one holding the real children and content, the other a husk. `merge_blocks`
//! folds the duplicate into the canonical and leaves a REPLICATED redirect so
//! the duplicate's id keeps resolving.
//!
//! Six ratified properties, all asserted per generated case:
//!   (a) resolving the duplicate's id yields the canonical id;
//!   (b) the normalized non-husk content multiset survives, up to dedupe
//!       collapse — every collapsed group keeps at least one member;
//!   (c) child order is deterministic: canonical's children, then the
//!       duplicate's, dedupe keepers in place;
//!   (d) undo restores the exact pre-merge state (one gesture);
//!   (e) every inbound link that resolved to the duplicate resolves to the
//!       canonical afterwards;
//!   (f) merging the same pair twice fails loud.
//!
//! Design: docs/Plans/MergeBlocksInc1-2026-07-30.md
//!
//! @pbt kind harness
//! @pbt covers merge-blocks-inc1 — duplicate-identity merge: replicated
//! redirect, one-level dedupe, deterministic order, one-group undo, inbound
//! link re-point

#![cfg(feature = "pbt")]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon_api::Value;
use holon_api::inline_mark::EntityRef;
use holon_api::inline_mark::InlineMark;
use holon_api::inline_mark::MarkSpan;
use holon_api::inline_mark::marks_to_json;
use holon_integration_tests::TestEnvironment;
use holon_integration_tests::TestEnvironmentBuilder;
use proptest::prelude::*;

const ROOT: &str = "11111111-0000-0000-0000-000000000001";
/// The surviving identity. In the husk case it starts empty.
const CANON: &str = "11111111-0000-0000-0000-000000000002";
/// The identity folded away; its id must keep resolving afterwards.
const DUP: &str = "11111111-0000-0000-0000-000000000003";
/// Carries the inbound link that property (e) follows.
const LINKER: &str = "11111111-0000-0000-0000-000000000004";

const SYNC_TIMEOUT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(25);

fn root_org() -> String {
    format!("#+ID: {ROOT}\n#+TITLE: Merge\n")
}

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build runtime"),
    )
}

/// One generated merge. Empty content on either side is the husk arm; the
/// child lists are drawn from a small alphabet with whitespace decoration so
/// normalization-equal duplicates arise across BOTH sides.
#[derive(Debug, Clone)]
struct MergeCase {
    canonical_content: String,
    duplicate_content: String,
    canonical_children: Vec<String>,
    duplicate_children: Vec<String>,
}

/// Two words joined by at least one space, optionally lead-padded — so
/// trim + whitespace-collapse maps many raw strings onto four normal forms.
fn child_content() -> impl Strategy<Value = String> {
    (
        prop::sample::select(vec!["alpha", "beta"]),
        prop::sample::select(vec!["one", "two"]),
        prop::sample::select(vec!["", " ", "  "]),
        prop::sample::select(vec![" ", "  ", " \t "]),
    )
        .prop_map(|(a, b, lead, mid)| format!("{lead}{a}{mid}{b}"))
}

fn merge_case() -> impl Strategy<Value = MergeCase> {
    (
        prop::sample::select(vec!["", "canonical body"]),
        prop::sample::select(vec!["", "duplicate body"]),
        prop::collection::vec(child_content(), 0..3),
        prop::collection::vec(child_content(), 0..4),
    )
        .prop_map(
            |(canonical_content, duplicate_content, canonical_children, duplicate_children)| {
                MergeCase {
                    canonical_content: canonical_content.to_string(),
                    duplicate_content: duplicate_content.to_string(),
                    canonical_children,
                    duplicate_children,
                }
            },
        )
}

fn canon_child_id(i: usize) -> String {
    format!("11111111-0000-0000-0001-{i:012}")
}

fn dup_child_id(i: usize) -> String {
    format!("11111111-0000-0000-0002-{i:012}")
}

/// The dedupe key: trim, then collapse every whitespace run to one space.
/// Mirrors the production normalizer; a divergence here is a real red.
fn normalize(content: &str) -> String {
    content.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn uri(id: &str) -> String {
    format!("block:{id}")
}

/// Every block row, keyed by id — the pre/post snapshot property (d) compares.
///
/// `sort_key` is deliberately EXCLUDED: structural ops recompute the positional
/// columns from the live tree rather than restoring the captured value (the
/// same rule the block→page transform's undo fingerprint follows), so sibling
/// ORDER is what must survive an undo, not the fractional key that encodes it.
/// Order is asserted separately via [`ordered_children`].
async fn snapshot(env: &TestEnvironment) -> Vec<(String, String, String)> {
    let rows = env
        .query_sql("SELECT id, parent_id, content FROM block_raw ORDER BY id")
        .await
        .expect("snapshot query failed");
    rows.iter()
        .map(|r| {
            let field = |name: &str| {
                r.get(name)
                    .and_then(|v| v.as_string())
                    .unwrap_or_default()
                    .to_string()
            };
            (field("id"), field("parent_id"), field("content"))
        })
        .collect()
}

/// `parent`'s children in sibling order, as (id, content).
async fn ordered_children(env: &TestEnvironment, parent: &str) -> Vec<(String, String)> {
    let rows = env
        .query_sql(&format!(
            "SELECT id, content FROM block_raw WHERE parent_id = '{}' ORDER BY sort_key",
            uri(parent)
        ))
        .await
        .expect("children query failed");
    rows.iter()
        .map(|r| {
            let field = |name: &str| {
                r.get(name)
                    .and_then(|v| v.as_string())
                    .unwrap_or_default()
                    .to_string()
            };
            (field("id"), field("content"))
        })
        .collect()
}

/// The `resolved_id` of every inbound link, keyed by source block.
async fn link_resolutions(env: &TestEnvironment) -> Vec<(String, String)> {
    let rows = env
        .query_sql("SELECT source_block_id, resolved_id FROM block_links ORDER BY source_block_id")
        .await
        .expect("block_links query failed");
    rows.iter()
        .filter_map(|r| {
            let source = r.get("source_block_id").and_then(|v| v.as_string())?;
            let resolved = r.get("resolved_id").and_then(|v| v.as_string())?;
            Some((source.to_string(), resolved.to_string()))
        })
        .collect()
}

/// Build the pre-merge tree: canonical + its children, duplicate + its
/// children, and a linker block whose only mark is an internal link to the
/// duplicate (so `block_links.resolved_id` starts at the duplicate).
async fn seed_tree(env: &TestEnvironment, case: &MergeCase) {
    env.create_block(CANON, ROOT, &case.canonical_content)
        .await
        .expect("create canonical");
    for (i, content) in case.canonical_children.iter().enumerate() {
        env.create_block(&canon_child_id(i), CANON, content)
            .await
            .expect("create canonical child");
    }
    env.create_block(DUP, ROOT, &case.duplicate_content)
        .await
        .expect("create duplicate");
    for (i, content) in case.duplicate_children.iter().enumerate() {
        env.create_block(&dup_child_id(i), DUP, content)
            .await
            .expect("create duplicate child");
    }

    let label = "see the duplicate";
    env.create_block(LINKER, ROOT, label)
        .await
        .expect("create linker");
    let mut marks = vec![MarkSpan::new(
        0,
        label.chars().count(),
        InlineMark::Link {
            target: EntityRef::Internal {
                id: holon_api::EntityUri::block(DUP),
            },
            label: label.to_string(),
        },
    )];
    holon_api::canonicalize_marks_against(label, &mut marks);
    let mut params: HashMap<String, Value> = HashMap::new();
    params.insert("id".into(), Value::String(uri(LINKER)));
    params.insert("field".into(), Value::String("marks".into()));
    params.insert("value".into(), Value::String(marks_to_json(&marks)));
    env.execute_operation("block", "set_field", params)
        .await
        .expect("link the linker at the duplicate");

    env.wait_for_loro_quiescence(SYNC_TIMEOUT).await;
    // The seed is authored in Loro; the properties this test reads live in the
    // SQL projection, so wait for the projection to actually carry the seeded
    // content rather than for a fixed interval.
    settle_until(env, |rows| {
        let content_of = |id: &str| {
            rows.iter()
                .find(|(rid, _, _)| rid == &uri(id))
                .map(|(_, _, c)| c.clone())
        };
        content_of(CANON).as_deref() == Some(case.canonical_content.trim_end())
            && content_of(DUP).as_deref() == Some(case.duplicate_content.trim_end())
            && rows.iter().filter(|(_, p, _)| p == &uri(CANON)).count()
                == case.canonical_children.len()
            && rows.iter().filter(|(_, p, _)| p == &uri(DUP)).count()
                == case.duplicate_children.len()
    })
    .await;
}

/// Poll the block projection until `done` holds. Fails loud on timeout — a
/// silent proceed would turn a projection stall into a bogus property failure.
async fn settle_until(env: &TestEnvironment, done: impl Fn(&[(String, String, String)]) -> bool) {
    let deadline = tokio::time::Instant::now() + SYNC_TIMEOUT;
    loop {
        let rows = snapshot(env).await;
        if done(&rows) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "block projection never settled; last rows: {rows:?}"
        );
        tokio::time::sleep(POLL).await;
    }
}

async fn merge(env: &TestEnvironment) -> anyhow::Result<()> {
    let mut params: HashMap<String, Value> = HashMap::new();
    params.insert("canonical".into(), Value::String(uri(CANON)));
    params.insert("duplicate".into(), Value::String(uri(DUP)));
    env.execute_operation("block", "merge_blocks", params).await
}

async fn run_case(rt: Arc<tokio::runtime::Runtime>, case: MergeCase) {
    let env = TestEnvironmentBuilder::new()
        .with_org_file("Merge.org", root_org())
        .build(rt)
        .await
        .expect("boot");
    env.wait_for_org_files_stable(25, SYNC_TIMEOUT).await;

    seed_tree(&env, &case).await;

    let before = snapshot(&env).await;
    let root_order_before = ordered_children(&env, ROOT).await;
    let canonical_order_before = ordered_children(&env, CANON).await;
    let duplicate_order_before = ordered_children(&env, DUP).await;
    let links_before = link_resolutions(&env).await;
    let inbound_at_duplicate: Vec<String> = links_before
        .iter()
        .filter(|(_, resolved)| resolved == &uri(DUP))
        .map(|(source, _)| source.clone())
        .collect();
    assert!(
        !inbound_at_duplicate.is_empty(),
        "precondition: the linker must resolve to the duplicate before the merge, got \
         {links_before:?}"
    );

    // The pre-merge content multiset the merge must preserve, husks excluded.
    let mut expected_normalized: Vec<String> = case
        .canonical_children
        .iter()
        .chain(case.duplicate_children.iter())
        .map(|c| normalize(c))
        .filter(|c| !c.is_empty())
        .collect();
    expected_normalized.sort();
    expected_normalized.dedup();

    merge(&env).await.expect("merge_blocks");
    env.wait_for_loro_quiescence(SYNC_TIMEOUT).await;
    // The duplicate's row disappearing is the merge's last projected effect.
    settle_until(&env, |rows| !rows.iter().any(|(id, _, _)| id == &uri(DUP))).await;

    // (a) the duplicate's id still resolves — to the canonical.
    let resolved = env
        .engine()
        .resolve_block_id(&holon_api::EntityUri::block(DUP))
        .await
        .expect("resolve the merged-away id");
    assert_eq!(
        resolved.to_string(),
        uri(CANON),
        "(a) the duplicate's id must resolve to the canonical after the merge"
    );

    let after_children = ordered_children(&env, CANON).await;
    let after_normalized: Vec<String> = after_children
        .iter()
        .map(|(_, content)| normalize(content))
        .filter(|c| !c.is_empty())
        .collect();

    // (b) every pre-merge normalized content still has a survivor, and the
    // merge invented nothing.
    for expected in &expected_normalized {
        assert!(
            after_normalized.contains(expected),
            "(b) normalized content {expected:?} lost by the merge; survivors \
             {after_normalized:?}, case {case:?}"
        );
    }
    let mut invented: Vec<&String> = after_normalized
        .iter()
        .filter(|c| {
            !expected_normalized.contains(c)
                && *c != &normalize(&case.duplicate_content)
                && *c != &normalize(&case.canonical_content)
        })
        .collect();
    invented.sort();
    assert!(
        invented.is_empty(),
        "(b) the merge invented content {invented:?}; case {case:?}"
    );

    // (b, dedupe) one survivor per collapsed group — no normalization-equal
    // siblings remain.
    let mut seen = after_normalized.clone();
    seen.sort();
    let deduped = {
        let mut d = seen.clone();
        d.dedup();
        d
    };
    assert_eq!(
        seen, deduped,
        "(b) normalization-equal siblings survived the dedupe: {after_children:?}"
    );

    // (c) deterministic order: canonical's own children keep their relative
    // order and precede the duplicate's surviving children.
    let surviving_canonical: Vec<String> = after_children
        .iter()
        .map(|(id, _)| id.clone())
        .filter(|id| (0..case.canonical_children.len()).any(|i| id == &uri(&canon_child_id(i))))
        .collect();
    let expected_canonical_order: Vec<String> = (0..case.canonical_children.len())
        .map(|i| uri(&canon_child_id(i)))
        .filter(|id| surviving_canonical.contains(id))
        .collect();
    assert_eq!(
        surviving_canonical, expected_canonical_order,
        "(c) the canonical's own children must keep their relative order"
    );
    let first_dup_slot = after_children.iter().position(|(id, _)| {
        (0..case.duplicate_children.len()).any(|i| id == &uri(&dup_child_id(i)))
    });
    let last_canonical_slot = after_children.iter().rposition(|(id, _)| {
        (0..case.canonical_children.len()).any(|i| id == &uri(&canon_child_id(i)))
    });
    if let (Some(first_dup), Some(last_canon)) = (first_dup_slot, last_canonical_slot) {
        assert!(
            last_canon < first_dup,
            "(c) surviving duplicate children must follow the canonical's own: {after_children:?}"
        );
    }

    // (e) the inbound link now resolves to the canonical.
    let links_after = link_resolutions(&env).await;
    for source in &inbound_at_duplicate {
        let resolved = links_after
            .iter()
            .find(|(s, _)| s == source)
            .map(|(_, r)| r.clone())
            .unwrap_or_else(|| panic!("(e) inbound link from {source} vanished: {links_after:?}"));
        assert_eq!(
            resolved,
            uri(CANON),
            "(e) inbound link from {source} must resolve to the canonical"
        );
    }

    // (f) the pair is already merged — a second merge must fail loud.
    let second = merge(&env).await;
    assert!(
        second.is_err(),
        "(f) merging an already-merged pair must fail loud, got Ok"
    );

    // (d) one undo gesture restores the exact pre-merge state.
    let outcome = env.engine().undo().await.expect("undo the merge");
    assert!(
        outcome.applied(),
        "(d) the merge must undo as one applied gesture, got {outcome:?}"
    );
    env.wait_for_loro_quiescence(SYNC_TIMEOUT).await;
    settle_until(&env, |rows| rows == before).await;

    let restored = snapshot(&env).await;
    assert_eq!(
        restored, before,
        "(d) undo must restore the exact pre-merge block state"
    );
    assert_eq!(
        ordered_children(&env, ROOT).await,
        root_order_before,
        "(d) undo must restore the pre-merge sibling order under the root"
    );
    assert_eq!(
        ordered_children(&env, CANON).await,
        canonical_order_before,
        "(d) undo must restore the canonical's pre-merge child order"
    );
    assert_eq!(
        ordered_children(&env, DUP).await,
        duplicate_order_before,
        "(d) undo must restore the duplicate's pre-merge child order"
    );
    let restored_links = link_resolutions(&env).await;
    assert_eq!(
        restored_links, links_before,
        "(d) undo must restore the exact pre-merge link resolutions"
    );
    let redirects = env
        .query_sql("SELECT from_id FROM block_redirects")
        .await
        .expect("redirect query failed");
    assert!(
        redirects.is_empty(),
        "(d) undo must retract the merge's redirect, got {redirects:?}"
    );
}

/// The shrunk shape that caught the dedupe-collapse undo defect: two children
/// whose normalized content is IDENTICAL, so the merge collapses one and undo
/// must both re-create it and put the pair back in their original order.
/// Deterministic, so the regression cannot hide behind generator luck.
#[test]
fn merge_blocks_undo_restores_order_after_identical_child_collapse() {
    let rt = runtime();
    rt.block_on(run_case(
        rt.clone(),
        MergeCase {
            canonical_content: String::new(),
            duplicate_content: String::new(),
            canonical_children: vec![],
            duplicate_children: vec!["alpha one".to_string(), "alpha one".to_string()],
        },
    ));
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: std::env::var("PROPTEST_CASES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(6),
        max_shrink_iters: 32,
        .. ProptestConfig::default()
    })]

    /// The six ratified merge properties over generated husk / both-non-empty
    /// shapes with normalization-colliding children on both sides.
    #[test]
    fn merge_blocks_preserves_identity_content_and_order(case in merge_case()) {
        let rt = runtime();
        rt.block_on(run_case(rt.clone(), case));
    }
}
