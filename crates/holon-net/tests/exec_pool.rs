//! A pool place holds N interchangeable tokens that every subject shares
//! (`NetDef::pool`): a firing that consumes it takes one, a put returns one,
//! the oldest waiter takes a returned token, and the free count is readable
//! by a guard.

use std::collections::BTreeSet;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use holon_net::exec::Body;
use holon_net::exec::Cause;
use holon_net::exec::Class;
use holon_net::exec::Content;
use holon_net::exec::EffectResult;
use holon_net::exec::Executor;
use holon_net::exec::NetDef;
use holon_net::exec::OccurrenceId;
use holon_net::exec::Output;
use holon_net::exec::Read;
use holon_net::exec::Receiver;
use holon_net::exec::Subject;
use holon_net::exec::TransitionDef;
use holon_net::exec::Trigger;
use proptest::prelude::*;

const NOW: Duration = Duration::ZERO;

fn always() -> Read {
    Arc::new(|_, _| Some(Content::new()))
}

/// `Take` moves a waiting subject into `Held` while it holds a slot;
/// `Release` hands the slot back.
fn slot_net(slots: impl FnOnce(NetDef) -> NetDef, bind: Read) -> NetDef {
    slots(NetDef::default().fifo("Waiting", Class::Process))
        .place("Held", Class::Process)
        .transition(
            TransitionDef::new(
                "Take",
                Trigger::Auto,
                bind,
                Body::pure(|s, _| Ok(vec![Output::new("Held", s, Content::new())])),
            )
            .consumes("Waiting")
            .consumes("Slots"),
        )
        .transition(
            TransitionDef::new(
                "Release",
                Trigger::Manual,
                always(),
                Body::pure(|s, _| Ok(vec![Output::new("Slots", s, Content::new())])),
            )
            .consumes("Held"),
        )
}

fn pool_net(size: u32) -> NetDef {
    slot_net(|n| n.pool("Slots", Class::Process, size), always())
}

fn arrive(ex: &mut Executor, subject: &str) {
    drop(ex.write("Waiting", subject, Content::new(), NOW).unwrap());
}

fn release(ex: &mut Executor, subject: &str) {
    drop(
        ex.fire(
            "Release",
            subject,
            OccurrenceId::Caller(format!("release-{subject}-{}", ex.tokens().free("Slots")).into()),
            Content::new(),
            NOW,
        )
        .unwrap(),
    );
}

fn held(ex: &Executor) -> Vec<String> {
    ex.tokens()
        .subjects("Held")
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// Spike: a plain fifo place keeps one queue per subject, so tokens put under
/// a pool subject never reach a waiter of another subject.
#[test]
fn a_plain_fifo_place_is_a_queue_per_subject_not_a_pool() {
    let net = slot_net(|n| n.fifo("Slots", Class::Process), always());
    let mut ex = Executor::new(net).unwrap();
    for _ in 0..2 {
        drop(ex.write("Slots", "pool", Content::new(), NOW).unwrap());
    }
    arrive(&mut ex, "a");
    assert_eq!(held(&ex), Vec::<String>::new());
}

#[test]
fn a_pool_starts_with_all_its_tokens_free() {
    let ex = Executor::new(pool_net(3)).unwrap();
    assert_eq!(ex.tokens().free("Slots"), 3);
}

#[test]
fn at_most_n_subjects_hold_a_token() {
    let mut ex = Executor::new(pool_net(2)).unwrap();
    for s in ["a", "b", "c", "d"] {
        arrive(&mut ex, s);
    }
    assert_eq!(held(&ex), ["a", "b"]);
    assert_eq!(ex.tokens().free("Slots"), 0);
}

#[test]
fn a_returned_token_goes_to_the_oldest_waiter() {
    let mut ex = Executor::new(pool_net(1)).unwrap();
    for s in ["c", "b", "a"] {
        arrive(&mut ex, s);
    }
    assert_eq!(held(&ex), ["c"]);
    release(&mut ex, "c");
    assert_eq!(held(&ex), ["b"]);
    release(&mut ex, "b");
    assert_eq!(held(&ex), ["a"]);
}

#[test]
fn a_guard_reads_the_free_count() {
    let reserve_one: Read = Arc::new(|r, _| (r.free("Slots") > 1).then(Content::new));
    let mut ex = Executor::new(slot_net(
        |n| n.pool("Slots", Class::Process, 2),
        reserve_one,
    ))
    .unwrap();
    for s in ["a", "b"] {
        arrive(&mut ex, s);
    }
    assert_eq!(held(&ex), ["a"], "the guard keeps the last token free");
    assert_eq!(ex.tokens().free("Slots"), 1);
}

#[test]
#[should_panic(expected = "pool")]
fn a_put_beyond_the_pool_size_is_refused() {
    let mut ex = Executor::new(pool_net(1)).unwrap();
    drop(ex.write("Slots", "x", Content::new(), NOW));
}

#[derive(Clone, Debug)]
enum Op {
    Arrive,
    Release(usize),
}

fn ops() -> impl Strategy<Value = Vec<Op>> {
    prop::collection::vec(
        prop_oneof![Just(Op::Arrive), (0usize..8).prop_map(Op::Release)],
        0..40,
    )
}

proptest! {
    /// The executor agrees with a queue-and-counter model after every
    /// operation: the free count stays within 0..=N, `held + free == N`, and
    /// waiters take tokens in arrival order.
    #[test]
    fn a_pool_never_goes_below_zero_or_above_n(size in 1u32..5, ops in ops()) {
        let mut ex = Executor::new(pool_net(size)).unwrap();
        let mut waiting: VecDeque<String> = VecDeque::new();
        let mut holding: BTreeSet<String> = BTreeSet::new();
        let mut next = 0;
        for op in ops {
            match op {
                Op::Arrive => {
                    let s = format!("w{next}");
                    next += 1;
                    arrive(&mut ex, &s);
                    waiting.push_back(s);
                }
                Op::Release(i) if !holding.is_empty() => {
                    let s = holding.iter().nth(i % holding.len()).unwrap().clone();
                    release(&mut ex, &s);
                    holding.remove(&s);
                }
                Op::Release(_) => {}
            }
            while holding.len() < size as usize && let Some(s) = waiting.pop_front() {
                holding.insert(s);
            }
            let free = ex.tokens().free("Slots");
            prop_assert!(free <= size as usize);
            prop_assert_eq!(free + held(&ex).len(), size as usize);
            prop_assert_eq!(held(&ex).into_iter().collect::<BTreeSet<_>>(), holding.clone());
            prop_assert_eq!(
                ex.tokens().subjects("Waiting").into_iter().map(|s: Subject| s.to_string()).collect::<BTreeSet<_>>(),
                waiting.iter().cloned().collect::<BTreeSet<_>>()
            );
        }
    }
}

/// `Take` runs an effect, so its pool token stays held across calls.
fn effect_net() -> NetDef {
    NetDef::default()
        .fifo("Waiting", Class::Process)
        .pool("Slots", Class::Process, 1)
        .transition(
            TransitionDef::new(
                "Take",
                Trigger::Auto,
                always(),
                Body::Effectful(Receiver::None),
            )
            .consumes("Waiting")
            .consumes("Slots"),
        )
}

#[test]
#[should_panic(expected = "pool")]
fn a_put_while_a_firing_holds_a_token_is_refused() {
    let mut ex = Executor::new(effect_net()).unwrap();
    arrive(&mut ex, "a");
    assert_eq!(ex.tokens().free("Slots"), 0);
    drop(ex.write("Slots", "x", Content::new(), NOW));
}

#[test]
fn a_failed_firing_gives_its_token_back_once() {
    let mut ex = Executor::new(effect_net()).unwrap();
    let q = ex.write("Waiting", "a", Content::new(), NOW).unwrap();
    let attempt = q.requests[0].attempt;
    drop(
        ex.complete(attempt, EffectResult::Failed(Cause::definite("no")), NOW)
            .unwrap(),
    );
    assert_eq!(ex.tokens().free("Slots"), 1);
}

/// Two effectful firings start while a guard sees a free token; the first
/// completion moves the pool row the second firing's guard read.
#[test]
fn concurrent_effects_behind_a_free_guard_both_land() {
    let free: Read = Arc::new(|r, _| (r.free("Slots") > 0).then(Content::new));
    let net = NetDef::default()
        .fifo("Waiting", Class::Process)
        .pool("Slots", Class::Process, 2)
        .place("Held", Class::Process)
        .transition(
            TransitionDef::new("Take", Trigger::Auto, free, Body::Effectful(Receiver::None))
                .consumes("Waiting")
                .consumes("Slots"),
        );
    let mut ex = Executor::new(net).unwrap();
    let mut attempts = Vec::new();
    for s in ["a", "b"] {
        let q = ex.write("Waiting", s, Content::new(), NOW).unwrap();
        attempts.push((s, q.requests[0].attempt));
    }
    for (s, attempt) in attempts {
        let held = Output::new("Held", &Subject::new(s), Content::new());
        drop(
            ex.complete(attempt, EffectResult::Done(vec![held]), NOW)
                .unwrap(),
        );
    }
    assert_eq!(held(&ex), ["a", "b"]);
}
