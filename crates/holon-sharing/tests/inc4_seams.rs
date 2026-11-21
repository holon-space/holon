//! Unit gates for the boundary-enforcement seam and the SQL-projection
//! orphan-row tripwire (ADR 0028).

use std::collections::BTreeSet;

use holon_api::BoundaryBehavior;
use holon_sharing::BlockId;
use holon_sharing::ContainerId;
use holon_sharing::assert_no_orphan_rows;
use holon_sharing::boundary::BoundaryDecision;
use holon_sharing::boundary::check_boundary;

fn c(s: &str) -> ContainerId {
    ContainerId(s.to_string())
}

/// The decision table `check_boundary` must implement, `None` = reject loud.
fn expected_decision(
    behavior: &BoundaryBehavior,
    crosses: bool,
    touches_shared: bool,
) -> Option<BoundaryDecision> {
    let in_place_or = |crossing: Option<BoundaryDecision>| {
        if crosses {
            crossing
        } else {
            Some(BoundaryDecision::AllowPrivate)
        }
    };
    match behavior {
        BoundaryBehavior::PrivateOnly | BoundaryBehavior::ForbiddenAtPageBoundary => {
            in_place_or(None)
        }
        BoundaryBehavior::Crossing { widens_audience } => {
            in_place_or(Some(BoundaryDecision::AllowCrossing {
                widens_audience: *widens_audience,
                requires_confirm: *widens_audience,
            }))
        }
        BoundaryBehavior::PolicyEdit => Some(BoundaryDecision::AllowPolicyEdit),
        BoundaryBehavior::IdentityOp => in_place_or(Some(BoundaryDecision::AllowCrossing {
            widens_audience: true,
            requires_confirm: true,
        })),
        BoundaryBehavior::Unclassified if crosses || touches_shared => None,
        BoundaryBehavior::Unclassified => Some(BoundaryDecision::AllowPrivate),
    }
}

/// Every (classification, crosses, source shared, target shared) input against
/// the decision table. The domain has 56 points, so it is enumerated instead of
/// sampled.
#[test]
fn check_boundary_follows_the_decision_table_on_every_input() {
    let behaviors = [
        BoundaryBehavior::PrivateOnly,
        BoundaryBehavior::Crossing {
            widens_audience: false,
        },
        BoundaryBehavior::Crossing {
            widens_audience: true,
        },
        BoundaryBehavior::ForbiddenAtPageBoundary,
        BoundaryBehavior::PolicyEdit,
        BoundaryBehavior::IdentityOp,
        BoundaryBehavior::Unclassified,
    ];
    let source = c("a");
    for behavior in &behaviors {
        for crosses in [false, true] {
            let target = if crosses { c("b") } else { source.clone() };
            for (source_is_shared, target_is_shared) in
                [(false, false), (false, true), (true, false), (true, true)]
            {
                let decision = check_boundary(
                    "op_under_test",
                    behavior,
                    &source,
                    source_is_shared,
                    &target,
                    target_is_shared,
                );
                let case = format!(
                    "{behavior:?} crosses={crosses} shared=({source_is_shared}, {target_is_shared})"
                );
                match expected_decision(behavior, crosses, source_is_shared || target_is_shared) {
                    Some(expected) => assert_eq!(decision, expected, "{case}"),
                    None => match decision {
                        BoundaryDecision::RejectLoud { reason } => assert!(
                            reason.contains("op_under_test"),
                            "{case}: the rejection must name the op, got `{reason}`"
                        ),
                        other => panic!("{case}: expected a loud rejection, got {other:?}"),
                    },
                }
            }
        }
    }
}

#[test]
fn orphan_tripwire_passes_when_all_containers_registered() {
    let registered: BTreeSet<ContainerId> = [c("x"), c("y")].into_iter().collect();
    let b1 = BlockId("b1".into());
    let b2 = BlockId("b2".into());
    let cx = c("x");
    let cy = c("y");
    let rows = vec![(&b1, &cx), (&b2, &cy)];
    assert!(assert_no_orphan_rows(rows, &registered).is_ok());
}

#[test]
fn orphan_tripwire_fires_on_unregistered_container() {
    let registered: BTreeSet<ContainerId> = [c("x")].into_iter().collect();
    let b1 = BlockId("b1".into());
    let ghost = c("ghost");
    let rows = vec![(&b1, &ghost)];
    let err = assert_no_orphan_rows(rows, &registered).unwrap_err();
    assert_eq!(err.block, b1);
    assert_eq!(err.container, ghost);
}
