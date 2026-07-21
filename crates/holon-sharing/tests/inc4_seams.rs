//! Unit gates for the boundary-enforcement seam and the SQL-projection
//! orphan-row tripwire (ADR 0028 Inc 4).

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

#[test]
fn private_only_rejects_a_crossing_but_allows_in_place() {
    let src = c("a");
    let tgt = c("b");
    assert!(matches!(
        check_boundary(
            "set_field",
            &BoundaryBehavior::PrivateOnly,
            &src,
            false,
            &tgt,
            false
        ),
        BoundaryDecision::RejectLoud { .. }
    ));
    // In-place (same container), even a shared one, is fine.
    assert_eq!(
        check_boundary(
            "set_field",
            &BoundaryBehavior::PrivateOnly,
            &src,
            true,
            &src,
            true
        ),
        BoundaryDecision::AllowPrivate
    );
}

#[test]
fn crossing_widen_requires_confirm() {
    let src = c("a");
    let tgt = c("b");
    let d = check_boundary(
        "drag_drop_block",
        &BoundaryBehavior::Crossing {
            widens_audience: true,
        },
        &src,
        false,
        &tgt,
        true,
    );
    assert_eq!(
        d,
        BoundaryDecision::AllowCrossing {
            widens_audience: true,
            requires_confirm: true
        }
    );
}

#[test]
fn crossing_narrow_does_not_require_confirm() {
    let src = c("a");
    let tgt = c("b");
    let d = check_boundary(
        "drag_drop_block",
        &BoundaryBehavior::Crossing {
            widens_audience: false,
        },
        &src,
        true,
        &tgt,
        false,
    );
    assert_eq!(
        d,
        BoundaryDecision::AllowCrossing {
            widens_audience: false,
            requires_confirm: false
        }
    );
}

#[test]
fn unclassified_is_fail_closed_at_any_boundary() {
    let src = c("a");
    let tgt = c("b");
    // Crossing → reject loud, op name present.
    match check_boundary(
        "mystery_op",
        &BoundaryBehavior::Unclassified,
        &src,
        false,
        &tgt,
        false,
    ) {
        BoundaryDecision::RejectLoud { reason } => assert!(reason.contains("mystery_op")),
        other => panic!("expected loud reject, got {other:?}"),
    }
    // Touching a shared container (even without crossing) → reject loud.
    assert!(matches!(
        check_boundary(
            "mystery_op",
            &BoundaryBehavior::Unclassified,
            &src,
            true,
            &src,
            true
        ),
        BoundaryDecision::RejectLoud { .. }
    ));
    // Purely private, same container → allowed.
    assert_eq!(
        check_boundary(
            "mystery_op",
            &BoundaryBehavior::Unclassified,
            &src,
            false,
            &src,
            false
        ),
        BoundaryDecision::AllowPrivate
    );
}

#[test]
fn forbidden_at_page_boundary_rejects_crossing() {
    let src = c("page");
    let tgt = c("other");
    assert!(matches!(
        check_boundary(
            "outdent",
            &BoundaryBehavior::ForbiddenAtPageBoundary,
            &src,
            false,
            &tgt,
            false
        ),
        BoundaryDecision::RejectLoud { .. }
    ));
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
