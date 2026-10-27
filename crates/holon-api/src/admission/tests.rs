use futures::FutureExt;

use super::*;

fn block() -> EntityName {
    EntityName::new("block")
}

fn on(ids: &[&str]) -> Footprint {
    Footprint::Subjects {
        relation: block(),
        subjects: ids.iter().map(|id| EntityUri::block(id)).collect(),
    }
}

fn is_released(claim: &mut Claim) -> bool {
    claim.released().now_or_never().is_some()
}

#[test]
fn disjoint_subjects_go_at_once() {
    let overlay = AdmissionOverlay::default();
    let mut a = overlay.admit(on(&["a"]));
    let mut b = overlay.admit(on(&["b"]));
    let mut c = overlay.admit(Footprint::Relation(EntityName::new("document")));
    assert!(is_released(&mut a) && is_released(&mut b) && is_released(&mut c));
}

#[test]
fn census_counts_live_claims_and_skips_abandoned_ones() {
    let overlay = AdmissionOverlay::default();
    let head = overlay.admit(on(&["x"]));
    let behind = overlay.admit(on(&["x"]));
    let _also_behind = overlay.admit(on(&["x"]));
    let _elsewhere = overlay.admit(on(&["y"]));
    assert_eq!(
        overlay.census(),
        AdmissionCensus {
            released: 2,
            waiting: 2
        }
    );
    drop(behind);
    assert_eq!(
        overlay.census(),
        AdmissionCensus {
            released: 2,
            waiting: 1
        }
    );
    drop(head);
    assert_eq!(
        overlay.census(),
        AdmissionCensus {
            released: 2,
            waiting: 0
        }
    );
}

#[test]
fn same_subject_chains_in_admission_order_and_a_settle_releases_exactly_the_next() {
    let overlay = AdmissionOverlay::default();
    let k1 = overlay.admit(on(&["x"]));
    let mut k2 = overlay.admit(on(&["x"]));
    let mut k3 = overlay.admit(on(&["x", "y"]));
    let mut other = overlay.admit(on(&["y"]));
    assert!(!is_released(&mut k2));
    assert!(!is_released(&mut k3));
    assert!(!is_released(&mut other), "y is held by k3");

    drop(k1);
    assert!(is_released(&mut k2));
    assert!(!is_released(&mut k3));
    drop(k2);
    assert!(is_released(&mut k3));
    assert!(!is_released(&mut other));
    drop(k3);
    assert!(is_released(&mut other));
}

#[test]
fn a_fence_waits_for_all_and_all_later_wait_for_it() {
    let overlay = AdmissionOverlay::default();
    let a = overlay.admit(on(&["a"]));
    let b = overlay.admit(Footprint::Relation(EntityName::new("document")));
    let mut fence = overlay.admit(Footprint::Fence);
    let mut later = overlay.admit(on(&["z"]));
    let mut later_fence = overlay.admit(Footprint::Fence);

    drop(a);
    assert!(!is_released(&mut fence));
    drop(b);
    assert!(is_released(&mut fence));
    assert!(!is_released(&mut later));
    drop(fence);
    assert!(is_released(&mut later));
    assert!(!is_released(&mut later_fence));
    drop(later);
    assert!(is_released(&mut later_fence));
}

#[test]
fn relation_and_subjects_of_the_same_relation_order_both_ways() {
    let overlay = AdmissionOverlay::default();
    let a = overlay.admit(on(&["a"]));
    let b = overlay.admit(on(&["b"]));
    let mut whole = overlay.admit(Footprint::Relation(block()));
    let mut after = overlay.admit(on(&["c"]));
    let mut elsewhere = overlay.admit(on(&["c"]));
    let mut other_relation = overlay.admit(Footprint::Relation(EntityName::new("document")));
    assert!(is_released(&mut other_relation));

    drop(a);
    assert!(!is_released(&mut whole));
    drop(b);
    assert!(is_released(&mut whole));
    assert!(!is_released(&mut after));
    drop(whole);
    assert!(is_released(&mut after));
    assert!(!is_released(&mut elsewhere));
}

#[test]
fn a_dropped_waiting_claim_keeps_its_place_then_releases_its_dependents() {
    let overlay = AdmissionOverlay::default();
    let running = overlay.admit(on(&["x"]));
    let waiting = overlay.admit(on(&["x"]));
    let mut behind = overlay.admit(on(&["x"]));
    drop(waiting);
    assert!(
        !is_released(&mut behind),
        "behind stays ordered after running"
    );
    let mut newcomer = overlay.admit(on(&["x"]));
    drop(running);
    assert!(is_released(&mut behind));
    assert!(!is_released(&mut newcomer));
    drop(behind);
    assert!(is_released(&mut newcomer));
    drop(newcomer);
    let state = overlay.state.lock().unwrap();
    assert!(state.inflight.is_empty() && state.latest_by_subject.is_empty());
    assert!(state.by_relation.is_empty() && state.latest_fence.is_none());
}

#[test]
fn a_ticket_dropped_without_run_releases_the_ops_behind_it() {
    let overlay = AdmissionOverlay::default();
    let request = OpRequest {
        entity_name: block(),
        op_name: "set_field".into(),
        params: StorageEntity::default(),
        origin: OpOrigin::User,
    };
    let ticket = Ticket::new(overlay.admit(on(&["x"])), request);
    let mut behind = overlay.admit(on(&["x"]));
    drop(ticket);
    assert!(is_released(&mut behind));
}

#[test]
#[should_panic(expected = "settled twice")]
fn settle_twice_panics() {
    let mut state = OverlayState::default();
    let (first, _) = state.admit(on(&["x"]));
    let (second, _release) = state.admit(on(&["x"]));
    state.settle(second);
    state.settle(second);
    state.settle(first);
}

#[test]
fn the_union_covers_both_footprints_and_no_more_than_their_relations() {
    let doc = Footprint::Relation(EntityName::new("document"));
    let cases = [
        (on(&["x"]), on(&["y"]), on(&["x", "y"])),
        (
            on(&["x"]),
            Footprint::Relation(block()),
            Footprint::Relation(block()),
        ),
        (
            Footprint::Relation(block()),
            on(&["x"]),
            Footprint::Relation(block()),
        ),
        (on(&["x"]), doc.clone(), Footprint::Fence),
        (Footprint::Fence, on(&["x"]), Footprint::Fence),
        (doc.clone(), doc.clone(), doc),
    ];
    for (a, b, union) in cases {
        let got = a.clone().union(b.clone());
        assert_eq!(got, union, "{a:?} ∪ {b:?}");
        assert!(
            got.covers(&a) && got.covers(&b),
            "{got:?} covers {a:?}, {b:?}"
        );
    }
    assert!(!on(&["x", "y"]).covers(&on(&["z"])));
}
