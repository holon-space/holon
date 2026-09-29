//! What a judged write claims of a decision while it runs: every block whose
//! change can alter the decision's parse, which reads the root and its direct
//! children before and after the write.

use std::collections::BTreeSet;

use holon_core::shape_gate::neighbourhood;

use super::*;

async fn claim_of(authority: MemAuthority, op: (&'static str, StorageEntity)) -> BTreeSet<String> {
    let ops = [PlanOp {
        op_name: op.0,
        params: &op.1,
    }];
    let judged = judge_plan(
        &ShapeValidators::new(vec![Arc::new(DecisionShape)]),
        Arc::new(authority),
        &ops,
    )
    .await
    .expect("the in-memory authority answers every read");
    if let Err(refused) = &judged.verdict {
        panic!("the write is legal: {refused}");
    }
    judged
        .shape_claim()
        .into_iter()
        .map(|id| id.id().to_string())
        .collect()
}

fn ids(ids: &[&str]) -> BTreeSet<String> {
    ids.iter().map(|id| id.to_string()).collect()
}

fn assert_claims(claimed: BTreeSet<String>, expected: &[&str]) {
    let expected = ids(expected);
    let missing: Vec<&String> = expected.difference(&claimed).collect();
    assert!(
        missing.is_empty(),
        "the claim {claimed:?} misses {missing:?}"
    );
}

#[tokio::test]
async fn a_child_that_moves_out_is_claimed_with_the_decision() {
    let outdent = ("outdent", params(&[("id", s("block:d-c"))]));
    assert_claims(
        claim_of(vault(), outdent).await,
        &["d", "d-a", "d-b", "d-c"],
    );
}

#[tokio::test]
async fn a_deleted_child_is_claimed_with_the_decision() {
    let delete = ("delete", params(&[("id", s("block:d-b"))]));
    assert_claims(claim_of(vault(), delete).await, &["d", "d-a", "d-b", "d-c"]);
}

#[tokio::test]
async fn a_child_whose_tags_change_is_claimed_with_the_decision() {
    let tags = set("d-b", "tags", Value::Array(vec![s("note")]));
    assert_claims(claim_of(vault(), tags).await, &["d", "d-a", "d-b", "d-c"]);
}

#[tokio::test]
async fn a_child_whose_keyword_changes_is_claimed_with_the_decision() {
    let keyword = set("d-a", "task_state", keyword("TODO"));
    assert_claims(
        claim_of(vault(), keyword).await,
        &["d", "d-a", "d-b", "d-c"],
    );
}

#[tokio::test]
async fn a_block_indented_into_the_decision_is_claimed_with_it() {
    let indent = ("indent", params(&[("id", s("block:after"))]));
    assert_claims(
        claim_of(vault(), indent).await,
        &["d", "d-a", "d-b", "d-c", "after"],
    );
}

/// The simulator mints a new id for the block a split creates, so each
/// judgement names a different one. A claim that held it would never be the
/// same twice and a nested write could never hold it.
#[tokio::test]
async fn a_block_the_write_creates_is_not_claimed() {
    let split = || {
        (
            "split_block",
            params(&[("id", s("block:d-a")), ("position", Value::Integer(4))]),
        )
    };
    let first = claim_of(vault(), split()).await;
    assert_eq!(first, ids(&["d", "d-a", "d-b", "d-c"]));
    assert_eq!(claim_of(vault(), split()).await, first);
}

#[tokio::test]
async fn a_write_with_unknown_follow_ups_claims_the_decision_its_target_is_in() {
    let validators = ShapeValidators::new(vec![Arc::new(DecisionShape)]);
    let claim = |target: &'static str| {
        let validators = &validators;
        async move {
            neighbourhood(validators, &vault(), &[EntityUri::block(target)], &[])
                .await
                .expect("the in-memory authority answers every read")
                .into_iter()
                .map(|id| id.id().to_string())
                .collect::<BTreeSet<String>>()
        }
    };
    assert_eq!(claim("d-a").await, ids(&["d", "d-a", "d-b", "d-c"]));
    assert_eq!(claim("d").await, ids(&["d", "d-a", "d-b", "d-c"]));
    assert_eq!(claim("before").await, ids(&[]));
}
