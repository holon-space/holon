//! A claim nested under a running write's own claim: the write learns the
//! blocks it shapes only while it runs, and takes them then.

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

fn nested(parent: &Claim, ids: &[&str]) -> Claim {
    parent
        .handle()
        .nest(on(ids))
        .expect("no cycle")
        .expect("the parent does not cover it")
}

/// `d` was admitted after the write and overlaps it, so it runs after the
/// whole write: the nested claim neither waits on it nor takes its place as
/// the latest holder of `t`.
#[test]
fn a_dependent_of_the_write_runs_after_its_effect_and_keeps_its_place_on_the_block() {
    let overlay = AdmissionOverlay::default();
    let mut write = overlay.admit(on(&["x"]));
    assert!(is_released(&mut write));
    let mut d = overlay.admit(on(&["x", "t"]));

    let mut shape = nested(&write, &["t"]);
    assert!(
        is_released(&mut shape),
        "the nested claim waits on a claim that waits on its own write"
    );
    let mut y = overlay.admit(on(&["t"]));
    assert!(!is_released(&mut d));
    assert!(!is_released(&mut y));

    drop(shape);
    assert!(!is_released(&mut d), "d runs only after the whole write");
    assert!(
        !is_released(&mut y),
        "y on t was admitted after d on t, so it runs after d"
    );
    drop(write);
    assert!(is_released(&mut d));
    assert!(!is_released(&mut y));
    drop(d);
    assert!(is_released(&mut y));
}

/// The latest holder of `t` is a dependent of the write; an earlier holder
/// that is not one must still be waited on.
#[test]
fn a_nested_claim_waits_on_every_independent_holder_not_only_the_latest() {
    let overlay = AdmissionOverlay::default();
    let earlier = overlay.admit(on(&["t"]));
    let mut write = overlay.admit(on(&["x"]));
    assert!(is_released(&mut write));
    let _d = overlay.admit(on(&["x", "t"]));

    let mut shape = nested(&write, &["t"]);
    assert!(!is_released(&mut shape), "`earlier` still holds t");
    drop(earlier);
    assert!(is_released(&mut shape));
}

/// A write on `t` admitted after the write but not overlapping it runs first,
/// and the nested claim waits for it.
#[test]
fn a_later_write_that_does_not_overlap_the_write_is_waited_on() {
    let overlay = AdmissionOverlay::default();
    let mut write = overlay.admit(on(&["x"]));
    assert!(is_released(&mut write));
    let mut later = overlay.admit(on(&["t"]));
    assert!(is_released(&mut later));

    let mut shape = nested(&write, &["t"]);
    assert!(!is_released(&mut shape));
    drop(later);
    assert!(is_released(&mut shape));
}

/// `a` holds `o` and waits for `t` through its nested claim; `b` holds `t`
/// and asks for `o`. Granting `b`'s nest would deadlock both.
#[test]
fn a_nest_that_closes_a_cycle_is_refused() {
    let overlay = AdmissionOverlay::default();
    let mut a = overlay.admit(on(&["o"]));
    let mut b = overlay.admit(on(&["t"]));
    assert!(is_released(&mut a) && is_released(&mut b));

    let mut a_shape = nested(&a, &["t"]);
    assert!(!is_released(&mut a_shape));
    let refused = b
        .handle()
        .nest(on(&["t", "o"]))
        .expect_err("b's nest waits on a, whose nested claim waits on b");
    assert_eq!(
        refused,
        NestRefused {
            parent: b.seq(),
            through: a.seq()
        }
    );

    drop(b);
    assert!(is_released(&mut a_shape));
}

#[test]
fn a_nest_the_write_already_covers_takes_no_claim() {
    let overlay = AdmissionOverlay::default();
    let write = overlay.admit(on(&["t", "a"]));
    assert!(write.handle().nest(on(&["a"])).expect("no cycle").is_none());
    let fence = {
        drop(write);
        overlay.admit(Footprint::Fence)
    };
    assert!(
        fence
            .handle()
            .nest(on(&["t", "a", "b"]))
            .expect("no cycle")
            .is_none()
    );
    let whole = {
        drop(fence);
        overlay.admit(Footprint::Relation(block()))
    };
    assert!(whole.handle().nest(on(&["t"])).expect("no cycle").is_none());
}

/// Two nested claims of one write do not wait on each other, and the second
/// does not wait on a claim that waits on the first.
#[test]
fn nested_claims_of_one_write_do_not_wait_on_each_other() {
    let overlay = AdmissionOverlay::default();
    let write = overlay.admit(on(&["x"]));
    let mut first = nested(&write, &["t"]);
    assert!(is_released(&mut first));
    let _behind_first = overlay.admit(on(&["t", "u"]));
    let mut second = nested(&write, &["t", "u"]);
    assert!(is_released(&mut second));
}

#[test]
#[should_panic(expected = "before its nested claim")]
fn a_write_settles_after_its_nested_claim() {
    let mut state = OverlayState::default();
    let (write, _) = state.admit(on(&["x"]));
    state.nest(write, on(&["t"])).expect("no cycle");
    state.settle(write);
}

#[test]
fn every_entry_is_gone_once_a_nested_write_settles() {
    let overlay = AdmissionOverlay::default();
    let write = overlay.admit(on(&["x"]));
    let d = overlay.admit(on(&["x", "t"]));
    let shape = nested(&write, &["t", "u"]);
    drop(shape);
    drop(write);
    drop(d);
    let state = overlay.state.lock().unwrap();
    assert!(state.inflight.is_empty() && state.latest_by_subject.is_empty());
    assert!(state.by_relation.is_empty() && state.latest_fence.is_none());
}

/// `queued` waits on the nested claim of `t`; the claim grows to `u` and
/// keeps its place, so `queued` still waits for it.
#[test]
fn a_grown_claim_keeps_the_claims_queued_behind_it() {
    let overlay = AdmissionOverlay::default();
    let write = overlay.admit(on(&["x"]));
    let mut shape = nested(&write, &["t"]);
    assert!(is_released(&mut shape));
    let mut queued = overlay.admit(on(&["t"]));

    shape.grow(on(&["t", "u"])).expect("no cycle");
    assert!(is_released(&mut shape));
    assert!(!is_released(&mut queued));
    let mut on_u = overlay.admit(on(&["u"]));
    assert!(!is_released(&mut on_u), "the grown claim holds u");

    drop(shape);
    assert!(is_released(&mut queued) && is_released(&mut on_u));
}

/// The growth waits on an independent holder of the new block, like a nest.
#[test]
fn a_grown_claim_waits_on_an_independent_holder_of_what_it_adds() {
    let overlay = AdmissionOverlay::default();
    let write = overlay.admit(on(&["x"]));
    let mut shape = nested(&write, &["t"]);
    let mut later = overlay.admit(on(&["u"]));
    assert!(is_released(&mut later));

    shape.grow(on(&["t", "u"])).expect("no cycle");
    assert!(!is_released(&mut shape));
    drop(later);
    assert!(is_released(&mut shape));
}

/// `a`'s claim on `t` waits on `b`; `b`'s nested claim on `x` grows to `o`,
/// which `a` holds: refused, and `b`'s claim is unchanged.
#[test]
fn a_growth_that_closes_a_cycle_is_refused_and_keeps_the_claim() {
    let overlay = AdmissionOverlay::default();
    let mut a = overlay.admit(on(&["o"]));
    let mut b = overlay.admit(on(&["t"]));
    assert!(is_released(&mut a) && is_released(&mut b));
    let mut b_shape = nested(&b, &["x"]);
    assert!(is_released(&mut b_shape));
    let mut a_shape = nested(&a, &["t"]);
    assert!(!is_released(&mut a_shape));

    let refused = b_shape
        .grow(on(&["x", "o"]))
        .expect_err("b's growth waits on a, whose nested claim waits on b");
    assert_eq!(
        refused,
        NestRefused {
            parent: b.seq(),
            through: a.seq()
        }
    );
    let mut on_o = overlay.admit(on(&["o"]));
    let mut on_x = overlay.admit(on(&["x"]));
    drop(b_shape);
    assert!(is_released(&mut on_x), "b's claim still covered only x");
    drop(b);
    assert!(is_released(&mut a_shape));
    assert!(!is_released(&mut on_o));
}
